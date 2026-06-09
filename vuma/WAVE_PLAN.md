# VUMA x86_64 Mitigation Plan — 32 Waves

> **Goal**: Make VUMA fully functional on x86_64 (my sandbox) while preserving ARM64 as a cross-compilation target. No shortcuts. No simplifications. Every line of code must be real, tested, and correct.

> **Constraints**: 
> - Host: x86_64 Intel Xeon, Linux
> - Target: ARM64 (Pi 5) + x86_64 (native sandbox)
> - Max 32 subagents per wave
> - Every wave must produce `cargo check` + `cargo test` green before the next dependent wave starts
> - If a subagent produces code that doesn't compile, it is REJECTED and must be redone

---

## Dependency DAG — Which Waves Can Run in Parallel

```
Time ──►

T0:  [W1]
T1:  [W2]
T2:  [W3]
T3:  [W4] [W5] [W16] [W23]
T4:  [W6] [W7] [W8] [W17] [W24]
T5:  [W9] [W18_p1] [W25]
T6:  [W10] [W11] [W18_p2]
T7:  [W12] [W13] [W19_p1] [W26]
T8:  [W14] [W19_p2] [W27]
T9:  [W15] [W20] [W28]
T10: [W21] [W22] [W29]
T11: [W30] [W31]
T12: [W32]
```

**Parallel group summary:**

| Group | Time Slot | Waves (parallel) | Total Subagents |
|-------|-----------|-------------------|-----------------|
| G0 | T0 | W1 | 8 |
| G1 | T1 | W2 | 12 |
| G2 | T2 | W3 | 16 |
| G3 | T3 | W4, W5, W16, W23 | 28 |
| G4 | T4 | W6, W7, W8, W17, W24 | 32 |
| G5 | T5 | W9, W18, W25 | 20 |
| G6 | T6 | W10, W11, W18-p2 | 24 |
| G7 | T7 | W12, W13, W19, W26 | 28 |
| G8 | T8 | W14, W19-p2, W27 | 20 |
| G9 | T9 | W15, W20, W28 | 24 |
| G10 | T10 | W21, W22, W29 | 24 |
| G11 | T11 | W30, W31 | 20 |
| G12 | T12 | W32 | 12 |

**Maximum parallelism: 5 waves simultaneously, up to 32 subagents per wave-group.**

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
`profile.release.target-cpu` is NOT a valid Cargo.toml key. This causes a warning.

YOUR TASK:
1. REMOVE `target-cpu = "native"` from the [profile.release] section in Cargo.toml
2. Create or edit /home/z/my-project/vuma/.cargo/config.toml to add:
   ```toml
   [build]
   rustflags = ["-C", "target-cpu=native"]
   
   [target.aarch64-unknown-linux-gnu]
   rustflags = []
   
   [target.aarch64-unknown-none]
   rustflags = []
   ```
   The x86_64 host gets -C target-cpu=native. ARM64 targets do NOT get it (cross-compile).
3. Run: export PATH="/home/z/.cargo/bin:$PATH" && cd /home/z/my-project/vuma && cargo check --workspace 2>&1 | head -5
   Verify the "unused manifest key" warning is GONE.

HARSH RULES:
- If the warning persists, you have FAILED.
- If you break any existing build, you have FAILED.
```

**W1-S3 through W1-S8: Fix compilation errors in each remaining crate**
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
- `cargo check --workspace 2>&1 | grep "^error" | wc -l` returns 0

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
   - dead_code → if truly unused, REMOVE the dead code. If it's part of a public API, add #[allow(dead_code)] with a comment explaining why.
   - unused_mut → remove the mut if not needed
   - static_mut_refs (Rust 2024) → replace &mut STATIC with &raw mut STATIC or refactor to use UnsafeCell
3. After fixing, run cargo check -p CRATE_NAME again. REPEAT until zero warnings.
4. Run the FULL test suite for the crate: cargo test -p CRATE_NAME
   If any test breaks, your fix was WRONG. Revert and fix properly.

CRATE ASSIGNMENTS:
- W2-S1: vuma-ive (11 warnings — unused imports, unused mut, unused variables)
- W2-S2: vuma-scg (2 warnings — dead_code: remaining, write_u8)
- W2-S3: vuma-codegen (5 warnings — unused variables, dead_code)
- W2-S4: vuma-parser (1 warning — dead_code: expect_ident)
- W2-S5: vuma-pi5 (6 warnings — static_mut_refs in uart.rs)
- W2-S6: vuma-proof (3 warnings — unused imports, unused variables, unused assignments)
- W2-S7: vuma-std (11 warnings — dead_code fields, methods, constants)

HARSH RULES:
- If ANY warning remains, you have FAILED.
- If you break any existing test, you have FAILED.
- If you add #[allow(dead_code)] without a justification comment, you have FAILED.
- You are NOT allowed to add #[allow(warnings)] or #[allow(unused)] at the crate level.
- Every fix must be surgical — do not refactor unrelated code.
```

**W2-S8 through W2-S12: Deep dead code analysis**

```
You are performing a DEAD CODE AUDIT on CRATE_NAME at /home/z/my-project/vuma.

YOUR TASK:
1. Identify ALL functions, methods, structs, enums, and traits that are defined but NEVER USED
   outside of their own module. Use:
   export PATH="/home/z/.cargo/bin:$PATH" && cd /home/z/my-project/vuma && RUSTFLAGS="-W dead_code" cargo check -p CRATE_NAME 2>&1 | grep "never used\|never read\|never constructed"
2. For each dead code item, determine:
   - Is it part of a PUBLIC API that will be needed later? → Add `#[allow(dead_code)]` with a comment: "// TODO: needed for <specific future use>"
   - Is it truly dead (leftover from refactoring, abandoned approach)? → DELETE it entirely
   - Is it only used in tests? → Gate it with `#[cfg(test)]`
3. DO NOT delete anything that might be needed by other crates. Check cross-crate usage first.

CRATE ASSIGNMENTS:
- W2-S8: vuma-core
- W2-S9: vuma-codegen (deeper analysis of arm64.rs unused encodings)
- W2-S10: vuma-std (alloc.rs has 3 dead items, io.rs has 6 dead items)
- W2-S11: vuma-projection
- W2-S12: vuma-tests

HARSH RULES:
- If you delete code that IS used somewhere else, you have FAILED CATASTROPHICALLY.
- If you leave code that is truly dead without marking it, you have FAILED.
- You MUST verify cargo check still passes after your changes.
- You MUST verify cargo test still passes after your changes.
```

### Success Criteria:
- `cargo check --workspace 2>&1 | grep "warning" | wc -l` returns 0
- `cargo test --workspace` still passes (no regressions)

---

## Wave 3: Codegen Backend Trait Architecture

**Dependencies**: W2
**Estimated subagents**: 16

This is the CRITICAL wave — defining the abstraction layer that allows multiple backends.

### Subagent Tasks:

**W3-S1: Define the Backend trait**

```
You are designing the core abstraction that makes VUMA's codegen target-agnostic.

YOUR TASK: Create /home/z/my-project/vuma/src/codegen/src/backend.rs with the following:

```rust
use crate::ir::{IRProgram, IRFunction, IRBlock, IRValue, IRType, IRTerminator, BinOpKind, CmpKind, FenceKind};
use crate::regalloc::RegAlloc;

/// Target-specific information needed during code generation.
pub trait TargetInfo: Send + Sync {
    /// Pointer width in bytes (8 for 64-bit targets).
    fn pointer_width(&self) -> usize;
    
    /// Size of a type in bytes on this target.
    fn size_of(&self, ty: &IRType) -> usize;
    
    /// Alignment of a type in bytes on this target.
    fn alignment_of(&self, ty: &IRType) -> usize;
    
    /// Number of general-purpose registers available for allocation.
    fn num_gp_regs(&self) -> usize;
    
    /// Number of SIMD/floating-point registers available for allocation.
    fn num_simd_fp_regs(&self) -> usize;
    
    /// Does this target use a link register (ARM64) or pushes return address (x86_64)?
    fn has_link_register(&self) -> bool;
    
    /// Name of the calling convention used (e.g., "aapcs64", "systemv").
    fn calling_convention_name(&self) -> &'static str;
    
    /// Returns the ELF machine type for this target (e.g., EM_AARCH64=183, EM_X86_64=62).
    fn elf_machine_type(&self) -> u16;
    
    /// Default base address for ELF loading.
    fn default_base_address(&self) -> u64;
    
    /// Target triple string (e.g., "aarch64-unknown-linux-gnu").
    fn target_triple(&self) -> &'static str;
}

/// A code generation backend. Implement this for each target architecture.
pub trait Backend: Send + Sync {
    /// The target info for this backend.
    type Target: TargetInfo;
    
    /// Get the target info.
    fn target_info(&self) -> &Self::Target;
    
    /// Allocate registers for a function. Returns (allocated_function, stack_slots).
    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError>;
    
    /// Encode a single function into machine code bytes.
    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError>;
    
    /// Generate a return-from-function stub (e.g., RET for ARM64, ret for x86_64).
    fn return_stub(&self) -> Vec<u8>;
    
    /// Generate a trampoline for calling into compiled code from the runtime.
    fn trampoline(&self, entry_addr: u64) -> Vec<u8>;
    
    /// Disassemble the given bytes for debugging.
    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String>;
    
    /// Name of this backend (e.g., "arm64", "x86_64").
    fn name(&self) -> &'static str;
}

/// A function after register allocation.
pub struct AllocatedFunction {
    pub name: String,
    pub blocks: Vec<AllocatedBlock>,
    pub stack_size: usize,
    pub callee_saved_regs: Vec<PhysicalReg>,
    pub frame_type: FrameType,
}

pub struct AllocatedBlock {
    pub label: String,
    pub instructions: Vec<AllocatedInstruction>,
    pub terminator: AllocatedTerminator,
}

pub struct AllocatedInstruction {
    pub opcode: String,  // For debugging
    pub physical_regs: Vec<PhysicalReg>,
    pub encoded: Option<Vec<u8>>,  // Pre-encoded bytes if available
}

pub enum AllocatedTerminator {
    Fallthrough(String),
    Branch { cond: Option<PhysicalReg>, target: String },
    Return,
    Call { target: String, ret_addr: PhysicalReg },
}

pub struct PhysicalReg {
    pub name: String,
    pub index: usize,
    pub class: RegClass,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RegClass {
    General,
    SimdFp,
    Special,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Uses link register (ARM64) — no frame pointer needed for leaf functions
    LinkRegister,
    /// Pushes return address (x86_64) — always needs frame setup
    PushReturnAddress,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("register allocation failed: {0}")]
    RegAllocFailed(String),
    #[error("encoding failed: {0}")]
    EncodingFailed(String),
    #[error("unsupported instruction: {0}")]
    UnsupportedInstruction(String),
    #[error("invalid function: {0}")]
    InvalidFunction(String),
}
```

DESIGN REQUIREMENTS:
- The trait must be object-safe (no generic methods, no Self returns except Target)
- It must support both ARM64 and x86_64 without any target-specific assumptions in the trait itself
- All target-specific details (register counts, sizes, conventions) go in TargetInfo
- The trait must be testable in isolation without any actual backend implementation

HARSH RULES:
- If the trait is not object-safe, you have FAILED.
- If the trait has ANY ARM64-specific or x86_64-specific concepts in it, you have FAILED.
- If you cannot write a mock implementation for testing, the design is WRONG.
- You MUST compile the new file: cargo check -p vuma-codegen
- You MUST write at least 5 unit tests for the trait and types.
```

**W3-S2: Make IR types target-agnostic**

```
You are refactoring /home/z/my-project/vuma/src/codegen/src/ir.rs to remove ARM64-specific assumptions.

CURRENT PROBLEMS:
- `size_of()` and `alignment_of()` hardcode ARM64 LP64 model (i64=8, pointer=8)
- `compute_calling_conv()` implements AAPCS64 only
- `compute_stack_layout()` assumes ARM64 conventions

YOUR TASK:
1. Add a `TargetInfo` parameter to all functions that currently hardcode target assumptions:
   - `size_of(ty: &IRType) -> usize` → `size_of(ty: &IRType, target: &dyn TargetInfo) -> usize`
   - `alignment_of(ty: &IRType) -> usize` → `alignment_of(ty: &IRType, target: &dyn TargetInfo) -> usize`
   - `compute_calling_conv(func: &mut IRFunction)` → `compute_calling_conv(func: &mut IRFunction, target: &dyn TargetInfo)`
   - `compute_stack_layout(func: &mut IRFunction)` → `compute_stack_layout(func: &mut IRFunction, target: &dyn TargetInfo)`
2. Import the TargetInfo trait from the new backend module.
3. For NOW, provide a simple `Arm64Target` struct that implements TargetInfo with the current hardcoded values.
4. Update ALL call sites throughout the codegen crate.
5. Make sure `cargo check -p vuma-codegen` passes.
6. Make sure `cargo test -p vuma-codegen` passes.

HARSH RULES:
- If you break any existing test, you have FAILED.
- If you leave any hardcoded ARM64 assumptions in ir.rs, you have FAILED.
- If cargo check or cargo test fails, you have FAILED.
- Do NOT take shortcuts — update EVERY call site, not just some.
```

**W3-S3 through W3-S8: Update all codegen modules for Backend trait**

```
You are updating MODULE in /home/z/my-project/vuma/src/codegen/src/ to work with the new Backend trait.

MODULE ASSIGNMENTS:
- W3-S3: arm64.rs — Implement Backend trait for ARM64, implement TargetInfo for Arm64Target
- W3-S4: emit.rs — Refactor to use Backend trait instead of hardcoding ARM64 emission
- W3-S5: regalloc.rs — Refactor to use TargetInfo for register pool configuration
- W3-S6: scg_to_ir.rs — Pass TargetInfo through for any target-dependent decisions
- W3-S7: lib.rs — Add mod backend, re-export Backend/TargetInfo, add Backend selection API
- W3-S8: Write comprehensive tests for the Backend trait dispatch mechanism

YOUR TASK (W3-S3 — arm64.rs Backend impl):
1. Create Arm64Target struct implementing TargetInfo with:
   - pointer_width: 8
   - size_of: ARM64 LP64 model (same as current hardcoded values)
   - alignment_of: ARM64 alignment rules
   - num_gp_regs: 29 (X0-X28, excluding FP=X29 and LR=X30)
   - num_simd_fp_regs: 32 (V0-V31)
   - has_link_register: true
   - calling_convention_name: "aapcs64"
   - elf_machine_type: 183 (EM_AARCH64)
   - default_base_address: 0x400000
   - target_triple: "aarch64-unknown-linux-gnu"
2. Create Arm64Backend struct implementing Backend<Target=Arm64Target>.
3. Implement all Backend methods by wrapping the existing ARM64 code.
4. Do NOT delete any existing ARM64-specific code — wrap it behind the trait.
5. All existing ARM64 tests must still pass.

YOUR TASK (W3-S4 — emit.rs refactor):
1. The Emitter struct currently hardcodes ARM64 ELF emission.
2. Add a `target: &'static dyn TargetInfo` field to Emitter.
3. Use target.elf_machine_type() instead of hardcoded EM_AARCH64.
4. Use target.default_base_address() instead of hardcoded BASE_ADDR.
5. The actual instruction encoding stays in the Backend impl — Emitter calls Backend.encode_function().
6. All existing emit tests must still pass.

YOUR TASK (W3-S5 — regalloc.rs refactor):
1. The register allocator currently hardcodes ARM64 register pools.
2. Make the physical register pool configurable via TargetInfo.
3. Arm64Target provides the ARM64 register pool (current behavior).
4. Future X86_64Target will provide x86_64 register pool.
5. All existing regalloc tests must still pass.

YOUR TASK (W3-S6 — scg_to_ir update):
1. Pass TargetInfo through SCG-to-IR translation where needed.
2. This module should be mostly target-independent already (IR is the abstraction).
3. Fix any remaining hardcoded assumptions.
4. All existing scg_to_ir tests must still pass.

YOUR TASK (W3-S7 — lib.rs):
1. Add `pub mod backend;` to lib.rs
2. Re-export: `pub use backend::{Backend, TargetInfo, AllocatedFunction, BackendError};`
3. Add a `BackendKind` enum: `enum BackendKind { Arm64, X86_64 }`
4. Add a `fn create_backend(kind: BackendKind) -> Box<dyn Backend<Target=...>>` factory function.
5. This factory is how the pipeline selects the target.

YOUR TASK (W3-S8 — trait dispatch tests):
1. Write tests in src/codegen/src/backend.rs (or a new tests/ directory):
   - test_arm64_target_info_values: Verify all Arm64Target values match expected
   - test_backend_factory: Verify create_backend(BackendKind::Arm64) returns a working backend
   - test_trait_object_dispatch: Verify Box<dyn Backend> works for dynamic dispatch
   - test_target_info_sizes: Verify size_of/alignment_of produce correct results for all IRTypes
   - test_elf_machine_type: Verify ARM64 returns 183
2. All tests must pass.

HARSH RULES FOR ALL:
- If any existing test breaks, you have FAILED.
- If you introduce any regression in ARM64 codegen output, you have FAILED.
- If cargo check -p vuma-codegen fails, you have FAILED.
- If cargo test -p vuma-codegen fails, you have FAILED.
- You MUST NOT change the actual ARM64 machine code output — this is a refactoring, not a rewrite.
```

**W3-S9 through W3-S16: Update all dependent crates for TargetInfo propagation**

```
You are updating CRATE_NAME to pass TargetInfo through the pipeline where needed.

CRATE ASSIGNMENTS:
- W3-S9:  vuma-core — Update pipeline.rs to create and pass TargetInfo/Backend
- W3-S10: vuma-cor — Update runtime.rs to use Backend instead of hardcoded ARM64 emission
- W3-S11: vuma-cor — Update compile_region to use Backend trait
- W3-S12: vuma-tests — Update all codegen tests to use Backend trait
- W3-S13: vuma-ive — Check if IVE needs TargetInfo for any codegen-related operations
- W3-S14: vuma-bd — Check if BD inference needs any target-dependent parameters
- W3-S15: vuma-scg — Check if SCG construction needs any target-dependent parameters
- W3-S16: Write integration test: parse→SCG→BD→IVE→IR→ARM64 Backend→ELF (end-to-end ARM64 via trait)

YOUR TASK:
1. For each crate, identify all places where ARM64 is hardcoded.
2. Replace with Backend/TargetInfo dispatch.
3. Ensure `cargo check -p CRATE_NAME` passes.
4. Ensure `cargo test -p CRATE_NAME` passes.

HARSH RULES:
- If you break any existing test, you have FAILED.
- If you leave any ARM64 hardcoded assumption, you have FAILED.
- If you introduce a new compilation error, you have FAILED.
- The ARM64 codegen output must be BIT-FOR-BIT identical after refactoring.
```

### Success Criteria:
- `cargo check --workspace` passes with zero errors
- `cargo test --workspace` passes with zero failures
- Backend trait is object-safe and supports dynamic dispatch
- ARM64 codegen output is unchanged from pre-refactoring
- `Box<dyn Backend>` works for ARM64 backend

---

## Wave 4: ARM64 Backend Validation — Ensure Zero Regressions

**Dependencies**: W3
**Estimated subagents**: 8

### Subagent Tasks:

**W4-S1: ARM64 codegen regression test suite**

```
You are creating a COMPREHENSIVE regression test suite for ARM64 codegen at /home/z/my-project/vuma.

YOUR TASK:
1. For EVERY function in src/codegen/src/arm64.rs, write a test that:
   - Creates the instruction
   - Encodes it to bytes
   - Verifies the bytes match the expected ARM64 encoding (from ARM Architecture Reference Manual)
   - Decodes it back and verifies it matches the original
2. For EVERY instruction in emit.rs, write a test that:
   - Creates the IR instruction
   - Emits it through the Backend trait
   - Verifies the ARM64 bytes match expected encoding
3. Test the full pipeline: IRProgram → Backend.encode_function() → Vec<u8>
4. Run ALL tests and ensure they pass.

HARSH RULES:
- If any ARM64 encoding is wrong, you have FAILED.
- If you skip any instruction, you have FAILED.
- You must test BOTH encode and decode for every instruction.
- Cross-reference encodings with the ARM Architecture Reference Manual.
```

**W4-S2 through W4-S8: Per-module ARM64 validation**

```
You are validating MODULE in vuma-codegen for ARM64 regression after the Backend trait refactor.

MODULE ASSIGNMENTS:
- W4-S2: ir.rs — Verify all IR types produce correct ARM64 sizes/alignments via TargetInfo
- W4-S3: regalloc.rs — Verify register allocation produces identical results to pre-refactor
- W4-S4: scg_to_ir.rs — Verify SCG→IR translation is unchanged
- W4-S5: emit.rs — Verify ELF emission is bit-for-bit identical
- W4-S6: arm64.rs — Verify all ARM64 instruction encodings are correct
- W4-S7: control_flow — Verify switch/match/exception/tailcall/coroutine lowering still works
- W4-S8: Full pipeline — Parse VUMA source → SCG → BD inference → IVE verification → IR → ARM64 emission → disassemble → verify

HARSH RULES:
- If any test fails, you have FAILED.
- If you find a regression, FIX IT. Do not just report it.
- All tests must pass with cargo test -p vuma-codegen.
```

### Success Criteria:
- All ARM64 tests pass with zero failures
- ARM64 codegen output is verified identical to pre-refactoring
- Backend trait dispatch produces correct results for ARM64

---

## Wave 5: Extract Target-Agnostic Control Flow Module

**Dependencies**: W3
**Estimated subagents**: 8

### Subagent Tasks:

**W5-S1: Create control_flow.rs module**

```
You are creating /home/z/my-project/vuma/src/codegen/src/control_flow.rs — a TARGET-AGNOSTIC
control flow analysis and lowering module.

This module extracts control-flow logic currently inlined in scg_to_ir.rs into
a separate, well-structured module that works for ANY backend.

YOUR TASK: Create the module with these components:

1. SwitchLowerer — target-agnostic switch lowering
   - Input: switch value, cases (value→label), default label
   - Output: IR blocks implementing the switch
   - Strategy selection: jump_table vs binary_search vs if_else_chain
   - Strategy decision is parameterized by TargetInfo:
     - jump_table: dense cases, target supports jump tables
     - binary_search: sparse cases
     - if_else_chain: < 4 cases

2. ExceptionLowerer — target-agnostic exception handling
   - Input: try body, catch handlers, cleanup
   - Output: invoke/landing-pad IR structure
   - Landing pad layout is parameterized by TargetInfo

3. TailCallLowerer — target-agnostic tail call optimization
   - Input: call instruction with tail call annotation
   - Output: arg shuffle + jump (or fall back to regular call)
   - Eligibility analysis is parameterized by TargetInfo (calling convention)

4. CoroutineLowerer — target-agnostic coroutine transformation
   - Input: coroutine body with yield/resume points
   - Output: state machine IR
   - Frame layout is parameterized by TargetInfo

5. LoopOptimizer — target-agnostic loop optimization
   - Input: loop body with known trip count
   - Output: unrolled loop IR (if profitable)
   - Profitability analysis uses TargetInfo for cost model

DESIGN REQUIREMENTS:
- ALL functions take a &dyn TargetInfo parameter
- ZERO target-specific code in this module
- Each component has its own unit tests
- The module is imported via mod control_flow in lib.rs

HARSH RULES:
- If any function in this module references ARM64 or x86_64 specifically, you have FAILED.
- If the module cannot work with both ARM64 and x86_64 TargetInfo, you have FAILED.
- You must have at least 10 unit tests per component (50+ total).
- cargo check and cargo test must pass.
```

**W5-S2 through W5-S5: Migrate control flow logic from scg_to_ir.rs**

```
You are migrating control flow logic from scg_to_ir.rs to control_flow.rs.

SUBAGENT ASSIGNMENTS:
- W5-S2: Migrate switch/match lowering (lower_switch and related methods)
- W5-S3: Migrate try/catch/exception handling (lower_try and related methods)
- W5-S4: Migrate tail call optimization (lower_tailcall and related methods)
- W5-S5: Migrate loop optimization (lower_loop, unroll analysis and related methods)

YOUR TASK:
1. Identify all control-flow-related code in scg_to_ir.rs
2. Move it to the appropriate component in control_flow.rs
3. Update scg_to_ir.rs to call control_flow.rs functions
4. Verify all existing tests still pass
5. Add NEW tests for the extracted functions in control_flow.rs

HARSH RULES:
- If any existing test breaks, you have FAILED.
- If you leave dead code in scg_to_ir.rs after extraction, you have FAILED.
- If you duplicate code between scg_to_ir.rs and control_flow.rs, you have FAILED.
- cargo test -p vuma-codegen must pass.
```

**W5-S6 through W5-S8: Integration tests for control flow**

```
You are writing integration tests for the target-agnostic control flow module.

SUBAGENT ASSIGNMENTS:
- W5-S6: Test SwitchLowerer with ARM64 TargetInfo — verify identical output to previous
- W5-S7: Test all components with a MockTargetInfo (8-bit, 16-bit, 32-bit configurations)
- W5-S8: Test all components with x86_64 TargetInfo (even though x86_64 backend doesn't exist yet,
  use a mock X86_64Target that implements TargetInfo with x86_64 values: pointer_width=8,
  num_gp_regs=16, num_simd_fp_regs=16, has_link_register=false, etc.)

HARSH RULES:
- If any test fails, you have FAILED.
- If any test is trivial (e.g., always true), you have FAILED.
- Each test must verify CORRECT BEHAVIOR, not just "doesn't crash".
```

### Success Criteria:
- `control_flow.rs` exists as a separate, well-structured module
- All control flow logic is target-agnostic (takes &dyn TargetInfo)
- `cargo test -p vuma-codegen` passes with all new + existing tests
- ARM64 behavior is unchanged

---

## Wave 6: x86_64 Instruction Definitions & Encoding

**Dependencies**: W4 (ARM64 validated)
**Estimated subagents**: 12

### Subagent Tasks:

**W6-S1: Create x86_64 module structure**

```
You are creating /home/z/my-project/vuma/src/codegen/src/x86_64.rs — the x86_64 backend.

YOUR TASK: Create the file with the following structure:

```rust
/// x86_64 register definitions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GpReg {
    Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi,
    R8, R9, R10, R11, R12, R13, R14, R15,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SimdFpReg {
    Xmm0, Xmm1, Xmm2, Xmm3, Xmm4, Xmm5, Xmm6, Xmm7,
    Xmm8, Xmm9, Xmm10, Xmm11, Xmm12, Xmm13, Xmm14, Xmm15,
}

/// REX prefix flags.
#[derive(Clone, Copy, Debug, Default)]
pub struct RexFlags {
    pub w: bool,  // REX.W — 64-bit operand size
    pub r: bool,  // Extension of ModRM.reg
    pub x: bool,  // Extension of SIB.index
    pub b: bool,  // Extension of ModRM.rm or SIB.base
}

/// x86_64 condition codes (based on EFLAGS).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    O, No, B, Nb, Z, Nz, Be, Nbe,
    S, Ns, P, Np, L, Nl, Le, Nle,
}

/// x86_64 instructions.
#[derive(Clone, Debug)]
pub enum Inst {
    // Arithmetic
    Add { dst: GpReg, src: Operand },
    Sub { dst: GpReg, src: Operand },
    Imul { dst: GpReg, src: Operand },
    Idiv { src: Operand },
    // ... (FULL instruction set — see below)
}

/// Operand for instructions.
#[derive(Clone, Debug)]
pub enum Operand {
    Reg(GpReg),
    Mem(MemoryOperand),
    Imm(i32),
    Imm64(i64),
}

#[derive(Clone, Debug)]
pub struct MemoryOperand {
    pub base: Option<GpReg>,
    pub index: Option<GpReg>,
    pub scale: u8,  // 1, 2, 4, or 8
    pub disp: i32,
}

/// Encoding result.
pub struct EncodedInst {
    pub bytes: Vec<u8>,
    pub relocs: Vec<Reloc>,
}

pub struct Reloc {
    pub offset: usize,
    pub kind: RelocKind,
    pub target: String,
}

#[derive(Clone, Copy)]
pub enum RelocKind {
    Rel32,
    Abs64,
}
```

You must define the COMPLETE x86_64 instruction set needed for VUMA:
- Data movement: MOV, MOVZX, MOVSX, LEA, XCHG
- Arithmetic: ADD, SUB, IMUL, IDIV, INC, DEC, NEG, NOT
- Logical: AND, OR, XOR, SHL, SHR, SAR, ROL, ROR
- Comparison: CMP, TEST
- Conditional: SETcc, CMOVcc
- Control flow: JMP, Jcc, CALL, RET, NOP
- Stack: PUSH, POP
- System: SYSCALL, INT3
- SSE2: MOVSD, ADDSD, SUBSD, MULSD, DIVSD, UCOMISD, CVTSI2SD, CVTTSD2SI
- Memory fences: MFENCE, LFENCE, SFENCE

HARSH RULES:
- If you miss any instruction that IR maps to, the backend will FAIL later.
- Every instruction must have correct operand types.
- The encoding function (Inst::encode) MUST be implemented — not a stub!
- You MUST reference the Intel x86-64 Software Developer's Manual for encoding rules.
```

**W6-S2 through W6-S8: Implement x86_64 instruction encoding**

```
You are implementing the encode() method for x86_64 instructions.

SUBAGENT ASSIGNMENTS:
- W6-S2: Data movement encoding (MOV reg/mem, MOVZX, MOVSX, LEA, XCHG)
- W6-S3: Arithmetic encoding (ADD, SUB, IMUL, IDIV, INC, DEC, NEG, NOT)
- W6-S4: Logical + shift encoding (AND, OR, XOR, SHL, SHR, SAR, ROL, ROR)
- W6-S5: Comparison + conditional encoding (CMP, TEST, SETcc, CMOVcc, Jcc)
- W6-S6: Control flow encoding (JMP, CALL, RET, NOP, INT3)
- W6-S7: Stack + system encoding (PUSH, POP, SYSCALL)
- W6-S8: SSE2 encoding (MOVSD, ADDSD, SUBSD, MULSD, DIVSD, UCOMISD, CVTSI2SD, CVTTSD2SI, fences)

YOUR TASK:
1. Implement `fn encode(&self) -> EncodedInst` for each instruction variant.
2. Follow the Intel x86-64 encoding rules EXACTLY:
   - Legacy prefixes
   - REX prefix (0x40-0x4F) with W/R/X/B bits
   - Opcode byte(s)
   - ModR/M byte (mod, reg, rm fields)
   - SIB byte (scale, index, base) when needed
   - Displacement (1, 2, or 4 bytes)
   - Immediate (1, 2, 4, or 8 bytes)
3. Handle ALL addressing modes for each instruction.
4. Write at least 5 test cases per instruction variant encoding.
5. Verify encoding correctness by comparing with known-good encodings
   (use `objdump -d` or xed decode as reference).

HARSH RULES:
- If ANY encoding is wrong, the ENTIRE BACKEND IS BROKEN.
- You MUST test every encoding. "Looks right" is NOT acceptable.
- You MUST handle REX prefix correctly for all register combinations.
- You MUST handle ModR/M + SIB encoding for all memory addressing modes.
- If you produce a stub or TODO, you have FAILED.
- Every encode() must return the EXACT bytes that a real x86_64 CPU would execute.
```

**W6-S9: x86_64 disassembler**

```
You are implementing the x86_64 disassembler for VUMA at /home/z/my-project/vuma/src/codegen/src/x86_64.rs.

YOUR TASK:
1. Implement `fn decode(bytes: &[u8], addr: u64) -> Result<(Inst, usize), DecodeError>`
2. This must be the INVERSE of encode(): decode(encode(inst)) == inst
3. Handle all instruction types defined in the Inst enum.
4. Write comprehensive tests: for every instruction, encode it, then decode it, verify round-trip.
5. Also implement the Backend::disassemble() method using this decoder.

HARSH RULES:
- If decode(encode(inst)) != inst for ANY instruction, you have FAILED.
- If you cannot disassemble any valid x86_64 encoding, you have FAILED.
- You must handle variable-length instructions correctly.
```

**W6-S10 through W6-S12: x86_64 TargetInfo implementation**

```
You are implementing the TargetInfo and Backend traits for x86_64.

SUBAGENT ASSIGNMENTS:
- W6-S10: X86_64Target struct implementing TargetInfo:
  - pointer_width: 8
  - size_of: x86_64 LP64 model (same as ARM64 for basic types)
  - alignment_of: x86_64 alignment rules (differences: __int128 is 16-byte aligned)
  - num_gp_regs: 14 (RAX, RCX, RDX, RSI, RDI, R8-R15 — excluding RSP, RBP)
  - num_simd_fp_regs: 16 (XMM0-XMM15)
  - has_link_register: false
  - calling_convention_name: "systemv"
  - elf_machine_type: 62 (EM_X86_64)
  - default_base_address: 0x400000
  - target_triple: "x86_64-unknown-linux-gnu"

- W6-S11: X86_64Backend struct implementing Backend:
  - Use the encoding/decoding from W6-S2 through W6-S9
  - Implement allocate_registers using TargetInfo register pool
  - Implement encode_function to emit x86_64 machine code
  - Implement return_stub (0xC3 = RET)
  - Implement trampoline
  - Implement disassemble

- W6-S12: Integration — Add X86_64 variant to BackendKind and create_backend() factory

HARSH RULES:
- If TargetInfo returns wrong values for x86_64, you have FAILED.
- If Backend methods produce incorrect machine code, you have FAILED.
- If create_backend(BackendKind::X86_64) doesn't work, you have FAILED.
- cargo check -p vuma-codegen must pass.
- All existing ARM64 tests must still pass.
```

### Success Criteria:
- `x86_64.rs` exists with complete instruction definitions
- All instruction encodings are correct (verified against Intel SDM)
- Round-trip encode→decode works for all instructions
- X86_64Target implements TargetInfo correctly
- X86_64Backend implements Backend correctly
- `cargo test -p vuma-codegen` passes (all ARM64 + new x86_64 tests)

---

## Wave 7: x86_64 Register Allocator

**Dependencies**: W4, W6
**Estimated subagents**: 8

### Subagent Tasks:

**W7-S1: x86_64 register pool definition**

```
You are defining the x86_64 register allocation pool for VUMA.

YOUR TASK:
1. In /home/z/my-project/vuma/src/codegen/src/x86_64.rs (or a new regalloc_x86_64.rs):
2. Define the caller-saved registers (available for allocation without saving):
   - GP: RAX, RCX, RDX, RSI, RDI, R8, R9, R10, R11
   - XMM: XMM0-XMM15
3. Define the callee-saved registers (must be preserved):
   - GP: RBX, R12, R13, R14, R15
   - (RBP is frame pointer, RSP is stack pointer — NOT allocatable)
4. SystemV ABI calling convention:
   - Integer args: RDI, RSI, RDX, RCX, R8, R9
   - Float args: XMM0-XMM7
   - Return value: RAX (integer), XMM0 (float)
   - Caller-saved: RAX, RCX, RDX, RSI, RDI, R8, R9, R10, R11
   - Callee-saved: RBX, R12, R13, R14, R15, RBP
5. Implement the register allocator for x86_64 using the same algorithm as ARM64
   but with x86_64's register pool and calling convention.

HARSH RULES:
- If you allocate RSP or RBP as a general register, you have FAILED CATASTROPHICALLY.
- If you violate SystemV calling convention, you have FAILED.
- You must handle the 6-argument limit (spill to stack for args > 6).
- cargo test -p vuma-codegen must pass.
```

**W7-S2 through W7-S8: Implement and test x86_64 register allocation**

```
You are implementing the x86_64 register allocator for VUMA.

SUBAGENT ASSIGNMENTS:
- W7-S2: Graph coloring allocator adapted for x86_64 register pool
- W7-S3: Spill code generation for x86_64 (MOV to/from stack slots)
- W7-S4: Caller-saved/callee-saved register save/restore prologue/epilogue
- W7-S5: Argument passing (RDI, RSI, RDX, RCX, R8, R9 → stack for 7th+ args)
- W7-S6: Return value handling (RAX for integer, XMM0 for float, RDX:RAX for i128)
- W7-S7: Register coalescing for x86_64 (same as ARM64 but different register names)
- W7-S8: Comprehensive test suite for x86_64 register allocation

HARSH RULES:
- If any register is used incorrectly, you have FAILED.
- If callee-saved registers are not preserved, you have FAILED.
- If the stack frame layout is wrong, you have FAILED.
- You must test with functions that have 0, 1, 6, 7, 10, and 20 arguments.
- You must test with functions that spill (more live values than registers).
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- x86_64 register allocator works correctly for all test cases
- SystemV ABI calling convention is correctly implemented
- `cargo test -p vuma-codegen` passes

---

## Wave 8: x86_64 Calling Convention & Type Layout

**Dependencies**: W4, W6
**Estimated subagents**: 6

### Subagent Tasks:

**W8-S1: SystemV ABI implementation**

```
You are implementing the SystemV AMD64 ABI calling convention for VUMA.

YOUR TASK:
1. Implement classification of function arguments per SystemV ABI:
   - INTEGER class: passes in GP register (RDI, RSI, RDX, RCX, R8, R9)
   - SSE class: passes in XMM register (XMM0-XMM7)
   - SSEUP class: upper half of SSE value
   - X87 class: passes on x87 stack (long double) — we can STUB this
   - MEMORY class: passes by reference on stack
2. Implement return value classification:
   - INTEGER class: RAX (and RDX for 128-bit)
   - SSE class: XMM0 (and XMM1 for 128-bit)
   - MEMORY class: caller allocates, passes pointer in RDI (hidden first arg)
3. Implement struct passing rules:
   - Small structs (≤16 bytes) split into 8-byte chunks, each classified independently
   - Large structs passed by hidden pointer
4. Implement stack alignment: 16-byte aligned before CALL instruction.

HARSH RULES:
- If you violate ANY SystemV ABI rule, you have FAILED.
- You must test with C interop: compile a C function, call it from VUMA x86_64 code, verify result.
- If stack is not 16-byte aligned at function entry, you have FAILED.
- You MUST reference the SystemV AMD64 ABI specification.
```

**W8-S2 through W8-S6: Type layout, stack frames, varargs, struct layout, tests**

```
You are implementing type layout and stack frame handling for x86_64.

SUBAGENT ASSIGNMENTS:
- W8-S2: Type sizes and alignments for x86_64 (i8=1, i16=2, i32=4, i64=8, f64=8, ptr=8, i128=16)
- W8-S3: Stack frame layout: return address at [RSP], saved RBP at [RSP+8], locals at [RBP-8], [RBP-16], etc.
         Prologue: PUSH RBP; MOV RBP,RSP; SUB RSP,frame_size
         Epilogue: MOV RSP,RBP; POP RBP; RET
- W8-S4: Variadic function handling (AL register = number of XMM args)
- W8-S5: Struct layout and passing (≤16 bytes in registers, >16 bytes by reference)
- W8-S6: Comprehensive test suite: 50+ test cases covering all ABI scenarios

HARSH RULES:
- If any type size or alignment is wrong, you have FAILED.
- If the stack frame is not properly aligned, you have FAILED.
- If struct passing doesn't match SystemV ABI, you have FAILED.
- You MUST verify against a C compiler (gcc/clang) for correctness.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- SystemV ABI is fully implemented for x86_64
- Type sizes, alignments, and struct layouts are correct
- Stack frame layout matches SystemV ABI specification
- `cargo test -p vuma-codegen` passes

---

## Wave 9: x86_64 Instruction Emission (IR → x86_64)

**Dependencies**: W7, W8
**Estimated subagents**: 12

### Subagent Tasks:

**W9-S1: Create x86_64 emitter module**

```
You are creating /home/z/my-project/vuma/src/codegen/src/emit_x86_64.rs — the x86_64 IR→machine code emitter.

YOUR TASK:
1. Create X86_64Emitter struct that implements the Backend::encode_function trait method.
2. Map each IR instruction to x86_64 instructions:
   - IR Add → x86_64 ADD
   - IR Sub → x86_64 SUB
   - IR Mul → x86_64 IMUL
   - IR Div → x86_64 IDIV (with RDX:RAX handling)
   - IR Rem → x86_64 IDIV + MOV RAX,RDX
   - IR And → x86_64 AND
   - IR Or → x86_64 OR
   - IR Xor → x86_64 XOR
   - IR Shl → x86_64 SHL
   - IR Shr → x86_64 SHR
   - IR Sar → x86_64 SAR
   - IR Cmp → x86_64 CMP + SETcc
   - IR Select → x86_64 CMOVcc
   - IR Load → x86_64 MOV (from memory)
   - IR Store → x86_64 MOV (to memory)
   - IR Call → x86_64 CALL
   - IR Ret → x86_64 RET
   - IR Jump → x86_64 JMP
   - IR Branch → x86_64 Jcc
   - IR Fence → x86_64 MFENCE/LFENCE/SFENCE
   - IR Nop → x86_64 NOP
3. Handle each mapping with correct register allocation integration.
4. Each mapping must produce CORRECT x86_64 machine code.

HARSH RULES:
- If any IR instruction is mapped incorrectly, you have FAILED.
- If any emitted x86_64 instruction is malformed, you have FAILED.
- You MUST NOT leave any TODO or stub mappings.
- Every mapping must be tested.
- cargo check -p vuma-codegen must pass.
```

**W9-S2 through W9-S12: Implement emission for each IR instruction category**

```
You are implementing IR→x86_64 emission for a specific category.

SUBAGENT ASSIGNMENTS:
- W9-S2: Arithmetic instructions (Add, Sub, Mul, Div, Rem, Neg, Not)
- W9-S3: Bitwise instructions (And, Or, Xor, Shl, Shr, Sar)
- W9-S4: Comparison and conditional (Cmp with all CmpKind variants, Select, Setcc)
- W9-S5: Memory instructions (Load, Store, StackSlot access, LEA for addresses)
- W9-S6: Control flow (Jump, Branch, Call, Ret)
- W9-S7: Function call emission (SystemV ABI arg passing, return value handling)
- W9-S8: Switch/match lowering to x86_64 (jump table, binary search, if-else chain)
- W9-S9: Exception handling (invoke/landing pad for x86_64)
- W9-S10: Tail call optimization for x86_64
- W9-S11: Coroutine state machine emission for x86_64
- W9-S12: ELF64 emission for x86_64 (EM_X86_64, proper section headers, etc.)

YOUR TASK:
For each assigned category:
1. Implement the emission from IR to x86_64 Inst enum values.
2. Write 10+ test cases per instruction.
3. Verify the emitted bytes are correct x86_64 machine code.
4. Where possible, write a C test program that calls the emitted code and verifies the result.

HARSH RULES:
- If any emitted instruction crashes on a real x86_64 CPU, you have FAILED.
- If the calling convention is wrong, you have FAILED.
- If stack alignment is violated, you have FAILED.
- You MUST test every path, including edge cases (zero, MAX, MIN, negative numbers).
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- Every IR instruction can be emitted to x86_64 machine code
- Emitted code follows SystemV ABI
- `cargo test -p vuma-codegen` passes with all new + existing tests

---

## Wave 10: x86_64 ELF Emission

**Dependencies**: W9
**Estimated subagents**: 6

### Subagent Tasks:

**W10-S1: x86_64 ELF64 writer**

```
You are implementing ELF64 file emission for x86_64 in VUMA.

YOUR TASK:
1. Create or extend the ELF emission to support x86_64:
   - ELF header: e_machine = EM_X86_64 (62)
   - Program headers: PT_LOAD for .text, PT_LOAD for .data
   - Section headers: .text, .data, .symtab, .strtab, .shstrtab
2. The emitted ELF must be a valid Linux x86_64 executable that can run natively.
3. Test by emitting a simple "return 42" program, writing to /tmp/vuma_test.elf,
   chmod +x, and executing it. The exit code should be 42.

HARSH RULES:
- If the emitted ELF doesn't pass `readelf -h` validation, you have FAILED.
- If the emitted ELF doesn't execute on x86_64 Linux, you have FAILED.
- If section headers are malformed, you have FAILED.
- You MUST test with actual execution, not just "looks correct".
```

**W10-S2 through W10-S6: ELF sections, relocations, symbols, debugging, tests**

```
You are implementing ELF features for x86_64 VUMA.

SUBAGENT ASSIGNMENTS:
- W10-S2: Relocations (R_X86_64_PC32, R_X86_64_PLT32, R_X86_64_64, R_X86_64_GOTPCRELX)
- W10-S3: Symbol table and string table generation
- W10-S4: DWARF debug info generation (minimal: .debug_info, .debug_abbrev, .debug_line)
- W10-S5: Multi-object linking (emit .o files that can be linked with ld)
- W10-S6: End-to-end test: emit ELF, execute, verify exit code matches expected

HARSH RULES:
- If relocations are wrong, linking will FAIL on real Linux.
- If the emitted binary crashes (segfault), you have FAILED.
- You MUST test with actual execution on this x86_64 machine.
- readelf, objdump, and file must all report correct format.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- Valid x86_64 ELF files can be emitted
- Emitted executables run natively on this x86_64 Linux machine
- `cargo test -p vuma-codegen` passes

---

## Wave 11: x86_64 Control Flow Lowering

**Dependencies**: W5, W9
**Estimated subagents**: 8

### Subagent Tasks:

**W11-S1: Switch lowering for x86_64**

```
You are implementing switch lowering for x86_64 in VUMA.

YOUR TASK:
1. Use the target-agnostic SwitchLowerer from control_flow.rs
2. Implement x86_64-specific lowering strategies:
   - Jump table: CMP + JE for bounds check + LEA for table base + JMP [table + index*8]
   - Binary search: CMP + JE/JNE tree
   - If-else chain: CMP + JE for each case
3. Generate correct relocations for jump table entries.
4. Test with 0, 1, 5, 50, and 256 cases.
5. Test with dense cases (0,1,2,3,4) and sparse cases (0, 1000, 2000).

HARSH RULES:
- If the jump table indexing is wrong, you have FAILED.
- If the bounds check is missing, you have FAILED.
- Every switch must handle the default case.
- cargo test -p vuma-codegen must pass.
```

**W11-S2 through W11-S8: Exception, tailcall, coroutine, loop unrolling, tests**

```
You are implementing control flow lowering for x86_64.

SUBAGENT ASSIGNMENTS:
- W11-S2: Exception handling (landing pads, exception tables for x86_64)
- W11-S3: Tail call optimization for x86_64 (arg shuffle + JMP instead of CALL)
- W11-S4: Coroutine state machine for x86_64 (yield/resume)
- W11-S5: Loop unrolling for x86_64
- W11-S6: Nested control flow (switch inside loop, exception inside switch)
- W11-S7: Integration tests: compile and EXECUTE each control flow construct
- W11-S8: Comparison tests: same IR compiled for both ARM64 and x86_64, verify both produce correct results

HARSH RULES:
- If any control flow construct produces incorrect behavior, you have FAILED.
- If tail call doesn't actually prevent stack growth, you have FAILED.
- If coroutine yield/resume loses state, you have FAILED.
- You MUST test by executing the emitted x86_64 code on this machine.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- All control flow constructs work correctly on x86_64
- Emitted x86_64 code can be executed natively
- `cargo test -p vuma-codegen` passes

---

## Wave 12: x86_64 Backend Integration Test

**Dependencies**: W10, W11
**Estimated subagents**: 8

### Subagent Tasks:

**W12-S1 through W12-S8: Full integration tests per feature area**

```
You are writing comprehensive integration tests for the x86_64 backend.

SUBAGENT ASSIGNMENTS:
- W12-S1: Arithmetic functions (add, sub, mul, div, rem, neg, not)
- W12-S2: Bitwise functions (and, or, xor, shl, shr, sar)
- W12-S3: Control flow (if/else, loops, switch, nested control flow)
- W12-S4: Functions with 0-20 arguments, 0-5 return values
- W12-S5: Recursive functions (factorial, fibonacci, tree traversal)
- W12-S6: Memory operations (load, store, stack allocation, heap allocation)
- W12-S7: Exception handling and coroutines
- W12-S8: Cross-backend validation (same source → ARM64 + x86_64 → both correct)

FOR EACH TEST:
1. Write the VUMA source (or construct the IR directly)
2. Compile to x86_64 via the Backend trait
3. Write the emitted code to /tmp/vuma_test_N.elf
4. Execute the ELF on this x86_64 machine
5. Verify the exit code matches the expected result
6. If the result is wrong, DEBUG the emitted code using objdump -d

HARSH RULES:
- If ANY test produces incorrect results, you have FAILED.
- If the emitted binary crashes, you have FAILED.
- You must test with ACTUAL EXECUTION, not just "compiles without errors".
- If you find a bug in the backend, FIX IT, don't just report it.
- cargo test -p vuma-codegen must pass.
```

### Success Criteria:
- All x86_64 integration tests pass with actual execution
- `cargo test --workspace` passes

---

## Wave 13: COR x86_64 Execution

**Dependencies**: W12
**Estimated subagents**: 6

### Subagent Tasks:

**W13-S1: COR execute_code for x86_64**

```
You are implementing the COR runtime's execute_code function for x86_64 at /home/z/my-project/vuma/src/cor/src/runtime.rs.

CURRENT STATE:
- On ARM64: mmap + copy + mprotect(PROT_READ|PROT_EXEC) + transmute to fn pointer + call
- On x86_64: Returns Ok(0) (STUB!)

YOUR TASK:
1. Add a new `#[cfg(all(unix, target_arch = "x86_64"))]` implementation of execute_code:
   ```rust
   #[cfg(all(unix, target_arch = "x86_64"))]
   fn execute_code_x86_64(code: &[u8], arg: usize) -> Result<usize, CorError> {
       use libc::{mmap, mprotect, munmap, PROT_READ, PROT_EXEC, PROT_WRITE, MAP_ANON, MAP_PRIVATE};
       let page_size = 4096;
       let alloc_size = ((code.len() + page_size - 1) / page_size) * page_size;
       unsafe {
           let mem = mmap(std::ptr::null_mut(), alloc_size, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0);
           if mem == libc::MAP_FAILED { return Err(CorError::MmapFailed); }
           std::ptr::copy_nonoverlapping(code.as_ptr(), mem as *mut u8, code.len());
           let result = mprotect(mem, alloc_size, PROT_READ | PROT_EXEC);
           if result != 0 { munmap(mem, alloc_size); return Err(CorError::MprotectFailed); }
           let func: fn(usize) -> usize = std::mem::transmute(mem);
           let ret = func(arg);
           munmap(mem, alloc_size);
           Ok(ret)
       }
   }
   ```
2. Update the dispatch in execute_code to call the correct implementation.
3. Test: compile a simple "return arg*2" function, execute via COR, verify result.

HARSH RULES:
- If the mmap/mprotect/execute cycle doesn't work, you have FAILED.
- If you don't munmap after execution (memory leak), you have FAILED.
- If the function doesn't actually execute the compiled code, you have FAILED.
- You MUST test with actual execution on this x86_64 machine.
- You MUST handle the case where code is not page-aligned.
```

**W13-S2 through W13-S6: COR x86_64 integration**

```
You are integrating x86_64 execution into the COR runtime.

SUBAGENT ASSIGNMENTS:
- W13-S2: Update compile_region to use x86_64 backend when target is x86_64
- W13-S3: Update return_zero_stub to emit x86_64 RET (0xC3) instead of ARM64 RET
- W13-S4: Wire Config::TargetArch::X86_64 to select the x86_64 Backend
- W13-S5: Test COR compile→execute cycle on x86_64 with 10+ functions
- W13-S6: Test COR incremental compilation on x86_64

HARSH RULES:
- If COR still returns 0 on x86_64, you have FAILED.
- If compiled code doesn't execute correctly, you have FAILED.
- You MUST test with actual mmap/mprotect/execute on this x86_64 machine.
- cargo test -p vuma-cor must pass.
```

### Success Criteria:
- COR runtime can compile and execute x86_64 code on this machine
- `cargo test -p vuma-cor` passes

---

## Wave 14: COR x86_64 Profiling & Optimization

**Dependencies**: W13
**Estimated subagents**: 6

### Subagent Tasks:

**W14-S1: COR profiling on x86_64**

```
You are implementing COR profiling for x86_64.

YOUR TASK:
1. Implement perf_counters for x86_64 using rdpmc or Linux perf_event_open.
2. Profile hot paths: function call counts, basic block execution frequencies.
3. Feed profiling data back to the optimization engine.
4. Test: compile a function, run it 10000 times, verify profiling data is collected.

HARSH RULES:
- If profiling data is incorrect, you have FAILED.
- If profiling adds >5% overhead, you have FAILED.
- You MUST test on this x86_64 machine with actual execution.
```

**W14-S2 through W14-S6: COR optimization passes on x86_64**

```
You are implementing COR optimization passes for x86_64.

SUBAGENT ASSIGNMENTS:
- W14-S2: Hot path detection and recompilation
- W14-S3: Inlining optimization for x86_64
- W14-S4: Loop optimization (unrolling, strength reduction) for x86_64
- W14-S5: Dead code elimination for x86_64
- W14-S6: Full COR cycle test: compile → execute → profile → optimize → re-execute → verify improvement

HARSH RULES:
- If optimization produces incorrect code, you have FAILED.
- If the optimized code doesn't execute correctly, you have FAILED.
- You MUST verify correctness after every optimization pass.
- cargo test -p vuma-cor must pass.
```

### Success Criteria:
- COR profiling works on x86_64
- COR optimization produces correct, faster code
- `cargo test -p vuma-cor` passes

---

## Wave 15: COR Full Cycle on x86_64

**Dependencies**: W14
**Estimated subagents**: 8

### Subagent Tasks:

**W15-S1 through W15-S8: End-to-end COR tests**

```
You are testing the complete COR cycle on x86_64.

SUBAGENT ASSIGNMENTS:
- W15-S1: Simple function: compile → execute → verify result
- W15-S2: Recursive function: factorial → compile → execute → verify
- W15-S3: Loop: sum 1..100 → compile → execute → verify
- W15-S4: Memory: allocate → write → read → verify
- W15-S5: Multi-function: compile 5 functions → call them all → verify all results
- W15-S6: Profile-guided: compile → execute (cold) → profile → recompile → execute (hot) → verify faster
- W15-S7: Incremental compilation: add function → recompile → execute → verify
- W15-S8: Stress test: compile 100 functions, execute each 1000 times, no crashes

FOR EACH TEST:
1. Use the VUMA pipeline (parse → SCG → BD → IVE → IR → x86_64 backend → COR execute)
2. Verify the result matches expected output.
3. Verify no memory leaks (check via /proc/self/status or valgrind).
4. Verify no segfaults.

HARSH RULES:
- If ANY test produces incorrect results, you have FAILED.
- If ANY test crashes, you have FAILED.
- If there are memory leaks, you have FAILED.
- cargo test -p vuma-cor must pass.
```

### Success Criteria:
- Full COR cycle works on x86_64
- All tests pass with correct results
- No memory leaks or crashes

---

## Wave 16: Test Framework Expansion

**Dependencies**: W2
**Estimated subagents**: 8

### Subagent Tasks:

**W16-S1: Create test harness for x86_64 execution**

```
You are creating a test harness at /home/z/my-project/vuma/src/tests/src/x86_64_harness.rs
that compiles and executes VUMA code on x86_64.

YOUR TASK:
1. Create a VumaTestHarness struct:
   - compile(source: &str) -> CompiledTest — compiles VUMA source to x86_64 ELF
   - compile_ir(ir: &IRProgram) -> CompiledTest — compiles IR to x86_64 ELF
   - execute(compiled: &CompiledTest, arg: usize) -> usize — runs the ELF
   - execute_with_args(compiled: &CompiledTest, args: &[usize]) -> usize — runs with multiple args
2. CompiledTest holds the ELF bytes and provides:
   - save_to_file(path: &str) — write ELF to disk
   - disassemble() -> String — disassemble the .text section
   - verify_elf() -> Result<()> — check ELF headers are valid
3. Create assertion macros:
   - assert_vuma_eq!(source, arg, expected) — compile, execute, assert result
   - assert_vuma_panics!(source, arg) — compile, execute, assert it crashes
4. Support both ARM64 (cross-compile, no execution) and x86_64 (native execution) modes.

HARSH RULES:
- If the harness can't compile and execute VUMA code, you have FAILED.
- If assertion macros don't provide useful error messages, you have FAILED.
- cargo test -p vuma-tests must pass.
```

**W16-S2 through W16-S8: Expand test coverage per crate**

```
You are expanding test coverage for CRATE_NAME.

SUBAGENT ASSIGNMENTS:
- W16-S2: vuma-parser tests (50+ new tests for edge cases)
- W16-S3: vuma-scg tests (SCG construction, dominance, liveness)
- W16-S4: vuma-bd tests (BD inference, unification, lattice operations)
- W16-S5: vuma-ive tests (five invariants, verification, constraint solving)
- W16-S6: vuma-codegen tests (ARM64 + x86_64 encoding round-trips)
- W16-S7: vuma-cor tests (compile→execute→profile→optimize cycle)
- W16-S8: vuma-std tests (collections, alloc, sync, io)

HARSH RULES:
- Each subagent must add at least 30 NEW non-trivial tests.
- Tests must cover edge cases, not just happy paths.
- Tests must test BEHAVIOR, not just "doesn't crash".
- cargo test --workspace must pass.
```

### Success Criteria:
- Test harness works for both ARM64 and x86_64
- 200+ new tests added across all crates
- `cargo test --workspace` passes

---

## Wave 17: ARM64 Regression Test Suite

**Dependencies**: W4
**Estimated subagents**: 6

### Subagent Tasks:

**W17-S1 through W17-S6: ARM64 cross-compilation validation**

```
You are creating a comprehensive ARM64 regression test suite that verifies ARM64
codegen correctness WITHOUT needing an ARM64 CPU.

SUBAGENT ASSIGNMENTS:
- W17-S1: ARM64 encoding golden files — compile programs, save ARM64 bytes as golden files, compare future runs
- W17-S2: ARM64 disassembly verification — encode → disassemble → verify human-readable output matches ARM syntax
- W17-S3: ARM64 calling convention tests — verify AAPCS64 compliance in emitted code
- W17-S4: ARM64 ELF validation — verify ARM64 ELF headers, sections, relocations
- W17-S5: ARM64 register allocation tests — verify correct register usage in emitted code
- W17-S6: ARM64 vs x86_64 parity tests — same source → both backends → verify same semantic result

HARSH RULES:
- If ANY ARM64 encoding changes unexpectedly, you have FAILED.
- If ARM64 golden file tests don't catch regressions, you have FAILED.
- These tests must work on x86_64 (they test the COMPILER, not the EXECUTION).
- cargo test -p vuma-tests must pass.
```

### Success Criteria:
- ARM64 regression tests cover all instructions and calling conventions
- Golden file comparison catches any encoding changes
- `cargo test --workspace` passes

---

## Wave 18: x86_64 Unit Tests

**Dependencies**: W9 (part 1), W12 (part 2)
**Estimated subagents**: 8

### Subagent Tasks:

**W18-S1 through W18-S8: x86_64 unit tests by category**

```
You are writing unit tests for the x86_64 backend.

SUBAGENT ASSIGNMENTS:
- W18-S1: Instruction encoding tests (every x86_64 instruction, every addressing mode)
- W18-S2: Disassembly round-trip tests (encode → decode → encode matches)
- W18-S3: Register allocation tests (spilling, coalescing, callee-saved)
- W18-S4: Calling convention tests (SystemV ABI compliance)
- W18-S5: ELF emission tests (valid headers, sections, relocations)
- W18-S6: Control flow tests (switch, exception, tailcall, coroutine)
- W18-S7: Memory operation tests (load, store, stack, heap)
- W18-S8: Execution tests (compile → execute → verify result on THIS x86_64 machine)

HARSH RULES:
- You MUST test with ACTUAL EXECUTION where possible.
- "Compiles without errors" is NOT sufficient — the emitted code must be CORRECT.
- If you skip any instruction or addressing mode, you have FAILED.
- Each subagent must write at least 50 tests.
- cargo test -p vuma-codegen and cargo test -p vuma-tests must pass.
```

### Success Criteria:
- 400+ new x86_64 unit tests
- All tests pass with actual execution
- `cargo test --workspace` passes

---

## Wave 19: Integration Tests

**Dependencies**: W15 (part 1), W18 (part 2)
**Estimated subagents**: 12

### Subagent Tasks:

**W19-S1 through W19-S12: Full pipeline integration tests**

```
You are writing integration tests that exercise the ENTIRE VUMA pipeline on x86_64.

SUBAGENT ASSIGNMENTS:
- W19-S1: Parse → SCG → BD inference → verify BD triples
- W19-S2: SCG → IVE verification → verify all five invariants pass
- W19-S3: IVE → IR → x86_64 emission → execute → verify result
- W19-S4: Full pipeline: source → parse → SCG → BD → IVE → IR → x86_64 → ELF → execute
- W19-S5: Error handling: malformed source → parse error → verify error message
- W19-S6: Error handling: type mismatch → IVE error → verify error message
- W19-S7: Error handling: invariant violation → IVE rejection → verify error message
- W19-S8: REPL: enter expression → evaluate → result
- W19-S9: Multi-file programs: import → compile → execute
- W19-S10: COR integration: compile → execute → profile → optimize → re-execute
- W19-S11: Cross-compilation: x86_64 host → ARM64 target → verify ARM64 ELF
- W19-S12: Stress test: 100+ function program → compile → execute → verify

FOR EACH TEST:
1. Test the COMPLETE pipeline, not just individual stages.
2. Verify every intermediate result (SCG, BD, IVE, IR, machine code).
3. For execution tests, verify the result on this x86_64 machine.

HARSH RULES:
- If any pipeline stage produces incorrect output, you have FAILED.
- If error handling doesn't work correctly, you have FAILED.
- If the REPL doesn't work, you have FAILED.
- Each test must be INDEPENDENT (no shared mutable state).
- cargo test -p vuma-tests must pass.
```

### Success Criteria:
- Full pipeline integration tests pass
- Error handling works correctly
- `cargo test --workspace` passes

---

## Wave 20: Full Pipeline x86_64

**Dependencies**: W15
**Estimated subagents**: 8

### Subagent Tasks:

**W20-S1: Pipeline orchestration for x86_64**

```
You are updating the main VUMA pipeline at /home/z/my-project/vuma/src/vuma/src/pipeline.rs
to support x86_64 as a native target.

YOUR TASK:
1. Update PipelineConfig to include a target field (BackendKind).
2. Update Pipeline::run() to:
   - Detect x86_64 host and select X86_64 backend automatically
   - Allow explicit target override (--target arm64 for cross-compilation)
3. Update all pipeline stages to pass TargetInfo through.
4. Test: run the full pipeline on this x86_64 machine with a simple program.
5. Verify the output ELF runs correctly.

HARSH RULES:
- If the pipeline still hardcodes ARM64, you have FAILED.
- If x86_64 execution doesn't work end-to-end, you have FAILED.
- You MUST test with actual execution on this machine.
- cargo test -p vuma-core must pass.
```

**W20-S2 through W20-S8: Pipeline stage updates for x86_64**

```
You are updating specific pipeline stages for x86_64 support.

SUBAGENT ASSIGNMENTS:
- W20-S2: Parse stage (target-independent, verify no ARM64 assumptions)
- W20-S3: SCG construction (target-independent, verify)
- W20-S4: BD inference (target-independent, verify)
- W20-S5: IVE verification (verify TargetInfo propagation)
- W20-S6: IR generation (verify TargetInfo propagation)
- W20-S7: Code generation (x86_64 backend selection)
- W20-S8: COR initialization (x86_64 execution setup)

HARSH RULES:
- If any stage still has ARM64-only code paths, you have FAILED.
- If the pipeline doesn't work on x86_64, you have FAILED.
- cargo test -p vuma-core must pass.
```

### Success Criteria:
- Full pipeline works on x86_64 from source to execution
- `cargo test --workspace` passes

---

## Wave 21: Cross-Compilation Workflow

**Dependencies**: W20
**Estimated subagents**: 6

### Subagent Tasks:

**W21-S1: Cross-compilation support**

```
You are implementing cross-compilation from x86_64 host to ARM64 target.

YOUR TASK:
1. Add --target flag to VUMA CLI:
   - `vuma build --target x86_64` (native)
   - `vuma build --target arm64` (cross-compile to ARM64)
2. When cross-compiling to ARM64:
   - Use ARM64 backend for code generation
   - Emit ARM64 ELF
   - Do NOT attempt to execute the output (can't run on x86_64)
3. When targeting x86_64:
   - Use x86_64 backend
   - Emit x86_64 ELF
   - CAN execute the output
4. Add validation: verify the emitted ARM64 ELF has correct headers (EM_AARCH64).
5. Add validation: verify the emitted x86_64 ELF has correct headers (EM_X86_64).

HARSH RULES:
- If cross-compilation produces invalid ARM64 ELF, you have FAILED.
- If native x86_64 compilation doesn't work, you have FAILED.
- You MUST test both targets.
- cargo test must pass.
```

**W21-S2 through W21-S6: Cross-compilation testing**

```
SUBAGENT ASSIGNMENTS:
- W21-S2: ARM64 cross-compilation golden file tests
- W21-S3: x86_64 native compilation + execution tests
- W21-S4: Target selection CLI tests
- W21-S5: Multi-target build (compile same source for both targets)
- W21-S6: Cross-compilation error handling (invalid target, missing tools)

HARSH RULES:
- If any target produces invalid output, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- Cross-compilation from x86_64 to ARM64 works
- Native x86_64 compilation works
- `cargo test --workspace` passes

---

## Wave 22: REPL on x86_64

**Dependencies**: W20
**Estimated subagents**: 6

### Subagent Tasks:

**W22-S1: REPL with x86_64 execution**

```
You are making the VUMA REPL work on x86_64 with actual code execution.

YOUR TASK:
1. Update /home/z/my-project/vuma/src/vuma/src/repl.rs to use the x86_64 backend.
2. The REPL must:
   - Parse input line by line
   - Build SCG incrementally
   - Run BD inference and IVE verification
   - Compile to x86_64 via Backend trait
   - Execute the compiled code via COR
   - Print the result
3. Support multi-line input (blocks, function definitions).
4. Support :help, :quit, :type (show BD), :scg (show graph), :ir (show IR), :asm (disassemble).

HARSH RULES:
- If the REPL can't execute code on x86_64, you have FAILED.
- If the REPL crashes on invalid input, you have FAILED.
- You MUST test with actual REPL interaction.
- cargo test -p vuma-core must pass.
```

**W22-S2 through W22-S6: REPL features**

```
SUBAGENT ASSIGNMENTS:
- W22-S2: Incremental compilation in REPL (add function, call it)
- W22-S3: REPL error recovery (bad input → error → continue)
- W22-S4: REPL debugging (:ir, :asm, :scg commands)
- W22-S5: REPL history and completion
- W22-S6: REPL integration tests

HARSH RULES:
- If the REPL doesn't handle errors gracefully, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- VUMA REPL works on x86_64 with actual execution
- `cargo test --workspace` passes

---

## Wave 23: Pi5 Platform Abstraction

**Dependencies**: W2
**Estimated subagents**: 6

### Subagent Tasks:

**W23-S1: Abstract Pi5 platform for testability**

```
You are refactoring vuma-pi5 to support testing on x86_64.

CURRENT STATE:
- boot.rs, smp.rs, timer.rs are gated behind #[cfg(target_arch = "aarch64")]
- uart.rs and gpio.rs use #[cfg(test)] for mock implementations
- mmio.rs uses ARM64 dmb/dsb barriers

YOUR TASK:
1. Create a Pi5Backend trait in platform.rs:
   ```rust
   pub trait Pi5Backend: Send + Sync {
       fn mmio_read(&self, addr: u64) -> u32;
       fn mmio_write(&self, addr: u64, val: u32);
       fn barrier_dmb(&self);
       fn barrier_dsb(&self);
       fn timer_read(&self) -> u64;
       fn timer_frequency(&self) -> u64;
   }
   ```
2. Implement RealPi5Backend (ARM64 only, uses inline asm for barriers)
3. Implement MockPi5Backend (x86_64 + test, uses simulated state)
4. Refactor mmio.rs, uart.rs, gpio.rs to use Pi5Backend instead of direct hardware access.
5. All existing tests must still pass.

HARSH RULES:
- If you break any ARM64 functionality, you have FAILED.
- If tests can't run on x86_64, you have FAILED.
- You MUST NOT use conditional compilation inside functions — use the trait instead.
- cargo test -p vuma-pi5 must pass on x86_64.
```

**W23-S2 through W23-S6: Per-module abstraction**

```
SUBAGENT ASSIGNMENTS:
- W23-S2: Abstract uart.rs to use Pi5Backend
- W23-S3: Abstract gpio.rs to use Pi5Backend
- W23-S4: Abstract mmio.rs to use Pi5Backend
- W23-S5: Create MockPi5Backend with simulated UART/GPIO state
- W23-S6: Write comprehensive tests using MockPi5Backend

HARSH RULES:
- If any module still uses direct hardware access without going through Pi5Backend, you have FAILED.
- cargo test -p vuma-pi5 must pass on x86_64.
```

### Success Criteria:
- vuma-pi5 can be fully tested on x86_64 using MockPi5Backend
- All ARM64 functionality preserved
- `cargo test --workspace` passes

---

## Wave 24: MMIO Emulation Layer

**Dependencies**: W23
**Estimated subagents**: 4

### Subagent Tasks:

**W24-S1: Full MMIO emulator for x86_64 testing**

```
You are creating a full MMIO emulator that simulates Pi 5 hardware on x86_64.

YOUR TASK:
1. Create /home/z/my-project/vuma/src/pi5/src/emulator.rs:
   - MmioBus struct that maps addresses to simulated devices
   - Simulated PL011 UART (with TX/RX buffers)
   - Simulated RP1 GPIO (with pin state)
   - Simulated BCM2712 system timer
   - Simulated BCM2712 interrupt controller
2. Implement Pi5Backend using MmioBus for full simulation.
3. Test: run UART driver against emulator → verify TX/RX works.
4. Test: run GPIO driver against emulator → verify pin read/write works.

HARSH RULES:
- If the emulator doesn't accurately simulate Pi 5 hardware, you have FAILED.
- You MUST test with the actual vuma-pi5 drivers.
- cargo test -p vuma-pi5 must pass on x86_64.
```

**W24-S2 through W24-S4: Emulator tests**

```
SUBAGENT ASSIGNMENTS:
- W24-S2: UART emulator tests (send/receive bytes, interrupts)
- W24-S3: GPIO emulator tests (pin modes, pull-up/down, interrupts)
- W24-S4: Timer + interrupt controller emulator tests

HARSH RULES:
- If any emulator test fails, you have FAILED.
- cargo test -p vuma-pi5 must pass.
```

### Success Criteria:
- Full Pi 5 MMIO emulation on x86_64
- All vuma-pi5 drivers can be tested against the emulator
- `cargo test --workspace` passes

---

## Wave 25: UART/GPIO Simulation

**Dependencies**: W24
**Estimated subagents**: 4

### Subagent Tasks:

**W25-S1: Advanced UART simulation**

```
You are enhancing the UART emulator for realistic Pi 5 testing.

YOUR TASK:
1. Simulate UART FIFO with configurable depth
2. Simulate baud rate timing (not real-time, but logical)
3. Simulate framing errors, parity errors, overrun errors
4. Test all UART driver features against the enhanced emulator
5. Add loopback test: TX → emulator → RX

HARSH RULES:
- If the UART emulator doesn't accurately model PL011 behavior, you have FAILED.
- You must test error conditions, not just happy paths.
- cargo test -p vuma-pi5 must pass.
```

**W25-S2 through W25-S4: GPIO, timer, and integration**

```
SUBAGENT ASSIGNMENTS:
- W25-S2: Advanced GPIO simulation (interrupts, debounce, PWM)
- W25-S3: Timer simulation (match registers, interrupts)
- W25-S4: Full Pi 5 simulation integration test

HARSH RULES:
- cargo test -p vuma-pi5 must pass on x86_64.
```

### Success Criteria:
- Advanced hardware simulation for Pi 5 peripherals
- All vuma-pi5 tests pass on x86_64

---

## Wave 26: Error Handling Audit

**Dependencies**: W19
**Estimated subagents**: 8

### Subagent Tasks:

**W26-S1 through W26-S8: Per-crate error handling audit**

```
You are auditing and improving error handling in CRATE_NAME.

AUDIT CRITERIA:
1. Every public function that can fail MUST return Result<T, E>
2. Error types must be specific (not just String or anyhow)
3. Error messages must include:
   - What operation failed
   - Why it failed
   - Where it failed (source location)
   - What the user can do to fix it
4. No unwrap() in non-test code
5. No panic!() in non-test code (except for truly impossible states)
6. No expect() without a meaningful message
7. All errors must propagate correctly (no silent swallowing)

CRATE ASSIGNMENTS:
- W26-S1: vuma-parser
- W26-S2: vuma-scg
- W26-S3: vuma-bd
- W26-S4: vuma-ive
- W26-S5: vuma-codegen
- W26-S6: vuma-cor
- W26-S7: vuma-core
- W26-S8: vuma-std, vuma-pi5, vuma-proof, vuma-projection

YOUR TASK:
1. Find all unwrap(), expect(), panic!() in non-test code.
2. Replace with proper error propagation using Result.
3. Find all places where errors are silently ignored (let _ = ...).
4. Add proper error handling.
5. Ensure all error types implement std::error::Error and Display.
6. Write tests for error conditions.

HARSH RULES:
- If you leave ANY unwrap() in non-test code, you have FAILED.
- If you leave ANY panic!() in non-test code (except for impossible states), you have FAILED.
- If error messages don't help the user diagnose the problem, you have FAILED.
- If you break any existing test, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- Zero unwrap/panic in non-test code
- All errors properly typed and propagated
- `cargo test --workspace` passes

---

## Wave 27: Memory Safety Audit

**Dependencies**: W19
**Estimated subagents**: 8

### Subagent Tasks:

**W27-S1 through W27-S8: Per-crate memory safety audit**

```
You are auditing memory safety in CRATE_NAME.

AUDIT CRITERIA:
1. All unsafe blocks must have a SAFETY comment explaining why the operation is safe.
2. All transmute calls must be justified.
3. All raw pointer dereferences must be validated.
4. No use-after-free possible.
5. No double-free possible.
6. No buffer overflow possible.
7. No integer overflow in size calculations (use checked arithmetic).
8. All FFI calls must have correct type signatures.
9. All static mut must be replaced with safe alternatives (AtomicX, OnceCell, etc.)

CRATE ASSIGNMENTS:
- W27-S1: vuma-codegen (unsafe in encode/decode, ELF emission)
- W27-S2: vuma-cor (unsafe in mmap/mprotect/transmute/execute)
- W27-S3: vuma-pi5 (unsafe in MMIO, UART, GPIO)
- W27-S4: vuma-std (unsafe in alloc, collections)
- W27-S5: vuma-core (unsafe in pipeline, security)
- W27-S6: vuma-ive (check for unsafe)
- W27-S7: vuma-bd, vuma-scg, vuma-parser (check for unsafe)
- W27-S8: vuma-proof, vuma-projection, vuma-tests (check for unsafe)

YOUR TASK:
1. Find ALL unsafe blocks.
2. For each one, add a SAFETY comment or refactor to eliminate the unsafe.
3. Find all static mut and replace with safe alternatives.
4. Find all integer arithmetic that could overflow and use checked/saturating ops.
5. Verify no memory leaks in COR execution paths.
6. Write tests for edge cases (zero-size allocations, max-size allocations, etc.).

HARSH RULES:
- If any unsafe block lacks a SAFETY comment, you have FAILED.
- If any unsafe can be replaced with safe code and you didn't, you have FAILED.
- If any static mut can be replaced with a safe alternative and you didn't, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- All unsafe blocks have SAFETY comments
- No unnecessary unsafe
- No static mut without justification
- `cargo test --workspace` passes

---

## Wave 28: UB Sanitization (Miri, ASAN, TSAN)

**Dependencies**: W27
**Estimated subagents**: 6

### Subagent Tasks:

**W28-S1: Run Miri on all crates**

```
You are running Miri (MIR Interpreter) to detect undefined behavior.

YOUR TASK:
1. Install Miri: rustup +nightly component add miri
2. Run: cargo miri test --workspace 2>&1
3. If Miri finds ANY undefined behavior, FIX IT.
4. Common Miri findings:
   - Out-of-bounds access
   - Use of uninitialized memory
   - Invalid pointer arithmetic
   - Data races (use -Zmiri-track-raw-pointers)
5. Document all Miri issues found and fixed.

HARSH RULES:
- If Miri finds UB and you don't fix it, you have FAILED.
- If you suppress Miri errors instead of fixing the root cause, you have FAILED.
- cargo miri test --workspace must pass with zero errors.
```

**W28-S2: Run AddressSanitizer (ASAN)**

```
You are running ASAN to detect memory errors.

YOUR TASK:
1. Compile with: RUSTFLAGS="-Zsanitizer=address" cargo test --workspace --target x86_64-unknown-linux-gnu
2. Fix any ASAN errors (heap-buffer-overflow, use-after-free, memory leaks).
3. Document all issues found and fixed.

HARSH RULES:
- If ASAN finds errors and you don't fix them, you have FAILED.
```

**W28-S3: Run ThreadSanitizer (TSAN)**

```
You are running TSAN to detect data races.

YOUR TASK:
1. Compile with: RUSTFLAGS="-Zsanitizer=thread" cargo test --workspace --target x86_64-unknown-linux-gnu
2. Fix any data races found.
3. Document all issues found and fixed.

HARSH RULES:
- If TSAN finds data races and you don't fix them, you have FAILED.
```

**W28-S4 through W28-S6: Fix all sanitizer findings**

```
You are fixing sanitizer findings across the workspace.

SUBAGENT ASSIGNMENTS:
- W28-S4: Fix Miri findings
- W28-S5: Fix ASAN findings
- W28-S6: Fix TSAN findings

HARSH RULES:
- ALL sanitizer tests must pass with zero errors.
- cargo test --workspace must pass.
```

### Success Criteria:
- Miri passes with zero UB findings
- ASAN passes with zero memory errors
- TSAN passes with zero data races
- `cargo test --workspace` passes

---

## Wave 29: Full Codebase Audit

**Dependencies**: W28
**Estimated subagents**: 12

### Subagent Tasks:

**W29-S1 through W29-S12: Comprehensive audit by area**

```
You are performing a FINAL comprehensive audit of CRATE_NAME.

AUDIT CHECKLIST:
1. API consistency: Do all public APIs follow the same naming conventions?
2. Documentation: Does every public function/struct/enum have a doc comment?
3. Test coverage: Are all public functions tested? What's the coverage percentage?
4. Error handling: Are all errors properly typed and propagated?
5. Performance: Are there any obvious performance bottlenecks?
6. Correctness: Are there any logical bugs?
7. Security: Are there any security vulnerabilities?
8. Code style: Does the code follow Rust conventions (cargo fmt, cargo clippy)?
9. Dependencies: Are all dependencies necessary and up-to-date?
10. Concurrency: Are all concurrent operations safe?

CRATE ASSIGNMENTS:
- W29-S1: vuma-parser
- W29-S2: vuma-scg
- W29-S3: vuma-bd
- W29-S4: vuma-ive
- W29-S5: vuma-codegen
- W29-S6: vuma-cor
- W29-S7: vuma-core
- W29-S8: vuma-std
- W29-S9: vuma-pi5
- W29-S10: vuma-proof
- W29-S11: vuma-projection
- W29-S12: vuma-tests

FOR EACH CRATE:
1. Run: cargo clippy -p CRATE_NAME -- -D warnings
2. Fix ALL clippy warnings.
3. Run: cargo fmt -p CRATE_NAME -- --check
4. Fix ALL formatting issues.
5. Generate a report with:
   - Total lines of code
   - Number of public APIs
   - Number of tests
   - Number of unsafe blocks
   - Number of TODO/FIXME/HACK comments
   - Test coverage estimate

HARSH RULES:
- If clippy has ANY warnings, you have FAILED.
- If formatting is wrong, you have FAILED.
- If you find a bug and don't fix it, you have FAILED.
- cargo test --workspace must pass.
```

### Success Criteria:
- `cargo clippy --workspace -- -D warnings` passes
- `cargo fmt -- --check` passes
- Zero TODO/FIXME/HACK comments without associated issues
- All crates audited with reports

---

## Wave 30: Documentation

**Dependencies**: W29
**Estimated subagents**: 10

### Subagent Tasks:

**W30-S1: Architecture documentation**

```
You are writing architecture documentation for VUMA at /home/z/my-project/vuma/docs/.

YOUR TASK: Write docs/architecture.md covering:
1. System overview (BD reasoning, five invariants, SCG, COR)
2. 12-crate architecture diagram
3. Pipeline stages (parse → SCG → BD → IVE → IR → codegen → COR)
4. Backend abstraction (Backend trait, TargetInfo, ARM64 vs x86_64)
5. Cross-compilation model
6. Data flow between crates
7. Extension points

Minimum 2000 words. No filler. Every paragraph must convey information.

HARSH RULES:
- If the documentation is inaccurate, you have FAILED.
- If the documentation is incomplete, you have FAILED.
- If a new contributor couldn't understand the architecture from this doc, you have FAILED.
```

**W30-S2 through W30-S10: Documentation per crate**

```
You are writing crate-level documentation for CRATE_NAME.

CRATE ASSIGNMENTS:
- W30-S2: vuma-parser docs
- W30-S3: vuma-scg docs
- W30-S4: vuma-bd docs
- W30-S5: vuma-ive docs
- W30-S6: vuma-codegen docs (CRITICAL — document Backend trait, ARM64, x86_64)
- W30-S7: vuma-cor docs
- W30-S8: vuma-core docs
- W30-S9: vuma-std docs
- W30-S10: vuma-pi5 docs

FOR EACH CRATE:
1. Add rustdoc comments to ALL public APIs (functions, structs, enums, traits).
2. Add module-level doc comments explaining the crate's purpose and design.
3. Add code examples in doc comments where appropriate.
4. Run: cargo doc -p CRATE_NAME --no-deps — verify no doc warnings.
5. Write a docs/CRATE_NAME.md with usage guide and examples.

HARSH RULES:
- If any public API lacks documentation, you have FAILED.
- If doc tests fail, you have FAILED.
- cargo doc --workspace must pass with zero warnings.
- cargo test --doc must pass.
```

### Success Criteria:
- All public APIs have rustdoc
- `cargo doc --workspace` passes with zero warnings
- Architecture documentation is comprehensive
- `cargo test --workspace` passes

---

## Wave 31: CI/CD Pipeline

**Dependencies**: W29
**Estimated subagents**: 8

### Subagent Tasks:

**W31-S1: GitHub Actions CI**

```
You are creating a GitHub Actions CI pipeline at /home/z/my-project/vuma/.github/workflows/ci.yml.

YOUR TASK: Create a CI workflow that runs on every push and PR:

```yaml
name: CI
on: [push, pull_request]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo check --workspace
  
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo test --workspace
  
  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with: { components: clippy }
      - run: cargo clippy --workspace -- -D warnings
  
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
        with: { components: rustfmt }
      - run: cargo fmt -- --check
  
  doc:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: cargo doc --workspace --no-deps
  
  miri:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@nightly
      - run: rustup +nightly component add miri
      - run: cargo miri test --workspace
```

HARSH RULES:
- If the CI doesn't catch a failing test, you have FAILED.
- If the CI doesn't run on PRs, you have FAILED.
```

**W31-S2 through W31-S8: Additional CI jobs and release automation**

```
SUBAGENT ASSIGNMENTS:
- W31-S2: Cross-compilation CI (ARM64 target on x86_64 host)
- W31-S3: Benchmark CI (run benchmarks, track performance over time)
- W31-S4: Security audit CI (cargo audit for dependencies)
- W31-S5: Coverage CI (cargo tarpaulin or cargo-llvm-cov)
- W31-S6: Release automation (tag → build → publish)
- W31-S7: Nightly Miri CI (run Miri every night)
- W31-S8: README badge generation

HARSH RULES:
- All CI workflows must pass on the current codebase.
- cargo test --workspace must pass.
```

### Success Criteria:
- Full CI/CD pipeline in GitHub Actions
- All CI jobs pass
- `cargo test --workspace` passes

---

## Wave 32: Release Preparation

**Dependencies**: W30, W31
**Estimated subagents**: 12

### Subagent Tasks:

**W32-S1: Version bump and changelog**

```
You are preparing VUMA for release.

YOUR TASK:
1. Update version in all Cargo.toml files to 0.2.0 (first x86_64-capable release).
2. Update CHANGELOG.md with all changes since last release.
3. Tag the release: git tag v0.2.0

HARSH RULES:
- If the changelog is incomplete, you have FAILED.
- If any version number is wrong, you have FAILED.
```

**W32-S2 through W32-S12: Final validation**

```
You are performing FINAL validation before release.

SUBAGENT ASSIGNMENTS:
- W32-S2: Full workspace build from clean (cargo clean && cargo build --workspace --release)
- W32-S3: Full test suite (cargo test --workspace --release)
- W32-S4: Clippy with all lints (cargo clippy --workspace -- -D warnings -W clippy::pedantic)
- W32-S5: Documentation build (cargo doc --workspace --no-deps)
- W32-S6: Cross-compilation test (cargo build --target aarch64-unknown-linux-gnu -p vuma-codegen)
- W32-S7: ARM64 golden file regression test
- W32-S8: x86_64 execution test (compile and execute 20+ programs)
- W32-S9: COR cycle test (compile → execute → profile → optimize → re-execute)
- W32-S10: REPL test (interactive session with 50+ expressions)
- W32-S11: Performance benchmark (compare with previous baseline)
- W32-S12: Final git commit + tag + push

HARSH RULES:
- If ANY step fails, the release is BLOCKED.
- If there are any TODO/FIXME comments in non-test code, the release is BLOCKED.
- If there are any clippy warnings, the release is BLOCKED.
- If there are any failing tests, the release is BLOCKED.
- If documentation is incomplete, the release is BLOCKED.
- THERE ARE NO EXCEPTIONS. A failed check means STOP and FIX.

VERIFICATION COMMANDS:
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

ALL of these must pass with ZERO errors or warnings.
```

### Success Criteria:
- All verification commands pass with zero errors/warnings
- Version tagged as v0.2.0
- Pushed to GitHub
- VUMA is fully functional on x86_64 with ARM64 cross-compilation support

---

## Summary Statistics

| Metric | Value |
|--------|-------|
| Total waves | 32 |
| Max parallel waves | 5 |
| Max subagents per wave | 32 |
| Total subagent tasks | ~256 |
| Estimated total new/modified lines | ~40,000-60,000 |
| Key deliverables | Backend trait, x86_64 backend, COR x86_64 execution, Pi5 emulator, full test suite |

## Critical Path

```
W1 → W2 → W3 → W4 → W6 → W9 → W10 → W12 → W13 → W14 → W15 → W20 → W22
                                                        ↓
                                                       W21
```

The critical path is 13 waves long. With maximum parallelism, the total execution time could be reduced to ~12 time slots (vs 32 sequential).
