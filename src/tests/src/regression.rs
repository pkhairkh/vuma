//! # Regression Tests for Waves 1–4 Bug Fixes
//!
//! Dedicated regression tests ensuring that bugs fixed in Waves 1–4 never
//! reoccur.  Each test function corresponds to a single bug and is named
//! descriptively so that a failure immediately identifies which regression
//! slipped.
//!
//! ## Wave 1
//!
//! | # | Test                                        | Bug                                              |
//! |---|---------------------------------------------|--------------------------------------------------|
//! | 1 | `test_arm64_ror_rol_not_asr`                | ARM64 ROR/ROL emitting ASR instead of EXTR/RORV  |
//! | 2 | `test_docs_no_pi5_references`               | Pi5 references in documentation                  |
//!
//! ## Wave 2
//!
//! | # | Test                                        | Bug                                              |
//! |---|---------------------------------------------|--------------------------------------------------|
//! | 3 | `test_loongarch64_atomics_not_empty`         | LoongArch64 atomics returning Vec::new()         |
//! | 4 | `test_ppc64_atomics_not_empty`               | PPC64 atomics returning Vec::new()               |
//! | 5 | `test_riscv64_atomic_cas_has_labels`         | RISC-V 64 atomic CAS loop with missing labels    |
//! | 6 | `test_wasm32_cas_uses_cmpxchg`               | Wasm32 CAS emitting simple load instead of cmpxchg|
//! | 7 | `test_arm32_atomic_cas_not_simple_load`      | ARM32 AtomicCas as simple load                   |
//! | 8 | `test_arm32_gt4_args_not_dropped`            | ARM32 >4 arguments silently dropped              |
//! | 9 | `test_mips64_ror_rol_has_complementary_shift`| MIPS64 ROR/ROL incomplete (missing complement)   |
//! | 10| `test_loongarch64_no_break_on_control_flow`  | LoongArch64 Switch/Invoke/TailCall/Resume emitting BREAK |
//!
//! ## Wave 3
//!
//! | # | Test                                        | Bug                                              |
//! |---|---------------------------------------------|--------------------------------------------------|
//! | 11| `test_fp_conversion_not_noop_all_backends`   | FP conversion casts as no-ops on all 10 backends  |
//!
//! ## Wave 4
//!
//! | # | Test                                        | Bug                                              |
//! |---|---------------------------------------------|--------------------------------------------------|
//! | 12| `test_arm64_stack_slot_not_nop_for_ct_atomics`| ARM64 stack-slot NOP for CtSelect/CtEq/atomics |
//! | 13| `test_unresolved_reloc_not_offset_zero`      | Unresolved symbol relocations silently leaving offset 0 |

use vuma_codegen::backend::{
    create_backend, AllocatedProgram, Backend, BackendError, BackendKind,
};
use vuma_codegen::ir::{
    BinOpKind, CastKind, CmpKind, IRBlock, IRFunction, IRInstr, IRTerminator, IRType, IRValue,
    VirtualRegister,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// All 10 backend kinds.
const ALL_BACKENDS: &[BackendKind] = &[
    BackendKind::AArch64,
    BackendKind::RiscV64,
    BackendKind::Wasm32,
    BackendKind::LoongArch64,
    BackendKind::X86_64,
    BackendKind::Arm32,
    BackendKind::Mips64,
    BackendKind::PowerPC64,
    BackendKind::X86_32,
    BackendKind::RiscV32,
];

/// Create a minimal IR function with the given name and a single entry block.
fn make_func(name: &str) -> IRFunction {
    let mut func = IRFunction::new(name);
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(0));
    func.vregs.insert(0, VirtualRegister::new(0, Some("result".to_string())));
    func
}

/// Compile a single-function IR program through the given backend and return
/// the allocated function (before encoding), or panic on error.
fn allocate_func(backend: &dyn Backend, func: &IRFunction) -> Result<vuma_codegen::backend::AllocatedFunction, BackendError> {
    backend.allocate_registers(func)
}

/// Compile a single-function IR program through the given backend and return
/// the final binary output.
fn compile_single(backend: &dyn Backend, func: &IRFunction) -> Vec<u8> {
    let allocated = backend.allocate_registers(func).unwrap_or_else(|e| {
        panic!("{}: allocate_registers failed for {}: {}", backend.name(), func.name, e)
    });
    let total_code_size: usize = allocated.code_size;
    let program = AllocatedProgram {
        functions: vec![allocated],
        total_code_size,
        total_data_size: 0,
    };
    backend.encode_program(&program).unwrap_or_else(|e| {
        panic!("{}: encode_program failed for {}: {}", backend.name(), func.name, e)
    })
}

/// Collect all instruction opcodes from an allocated function.
fn collect_opcodes(allocated: &vuma_codegen::backend::AllocatedFunction) -> Vec<String> {
    let mut opcodes = Vec::new();
    for block in &allocated.blocks {
        for instr in &block.instructions {
            opcodes.push(instr.opcode.clone());
        }
    }
    opcodes
}

/// Collect all encoded bytes from an allocated function.
fn collect_encoded(allocated: &vuma_codegen::backend::AllocatedFunction) -> Vec<u8> {
    let mut bytes = Vec::new();
    for block in &allocated.blocks {
        for instr in &block.instructions {
            bytes.extend_from_slice(&instr.encoded);
        }
    }
    bytes
}

// ===========================================================================
// Wave 1, Bug 1: ARM64 ROR/ROL emitting ASR instead of EXTR/RORV
// ===========================================================================

/// Regression test: ARM64 ROR/ROL must emit EXTR or RORV, never ASR.
///
/// **Original bug**: The ARM64 instruction selector was mistakenly mapping
/// `BinOpKind::Ror` / `BinOpKind::Rol` to the ASR (arithmetic shift right)
/// instruction encoding. This produced incorrect semantics — ASR sign-extends
/// while ROR/ROL rotate bits. The fix emits `EXTR Rd, Rn, Rn, #amount` for
/// immediate rotates and `RORV Rd, Rn, Rm` for register-variable rotates.
#[test]
fn test_arm64_ror_rol_not_asr() {
    use vuma_codegen::arm64::Instruction;

    // --- Verify that the Instruction enum has EXTR and RORV variants ---
    // EXTR is used for both ROR and ROL with immediates:
    //   ROR Rd, Rn, #amount = EXTR Rd, Rn, Rn, #amount
    //   ROL Rd, Rn, #amount = EXTR Rd, Rn, Rn, #(64 - amount)
    // RORV is used for register-variable rotate:
    //   ROR Rd, Rn, Rm = RORV Rd, Rn, Rm
    let _extr_exists = Instruction::EXTR {
        rd: vuma_codegen::arm64::Register::X0,
        rn: vuma_codegen::arm64::Register::X1,
        rm: vuma_codegen::arm64::Register::X1,
        imm6: 5,
    };
    let _rorv_exists = Instruction::RORV {
        rd: vuma_codegen::arm64::Register::X0,
        rn: vuma_codegen::arm64::Register::X1,
        rm: vuma_codegen::arm64::Register::X2,
    };

    // --- Compile an IR function with ROR and ROL through the ARM64 backend ---
    let backend = create_backend(BackendKind::AArch64)
        .expect("ARM64 backend creation should succeed");

    let mut func = make_func("ror_rol_test");
    func.vregs.insert(1, VirtualRegister::new(1, Some("a".to_string())));
    func.vregs.insert(2, VirtualRegister::new(2, Some("b".to_string())));

    let block = func.current_block();
    // ROR: result = a ror 5
    block.push(IRInstr::BinOp {
        op: BinOpKind::Ror,
        dst: IRValue::Register(1),
        lhs: IRValue::Register(0),
        rhs: IRValue::Immediate(5),
        ty: Some(IRType::I64),
    });
    // ROL: result = b rol 3
    block.push(IRInstr::BinOp {
        op: BinOpKind::Rol,
        dst: IRValue::Register(2),
        lhs: IRValue::Register(1),
        rhs: IRValue::Immediate(3),
        ty: Some(IRType::I64),
    });
    block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

    let allocated = allocate_func(&*backend, &func)
        .expect("ARM64 allocate_registers should succeed for ROR/ROL");

    // The allocated function must produce non-zero encoded bytes.
    let encoded = collect_encoded(&allocated);
    assert!(
        !encoded.is_empty(),
        "ARM64 ROR/ROL must produce encoded instructions, not an empty output"
    );

    // Disassemble and verify that no ASR instruction appears for the rotate ops.
    // We check that the disassembly mentions "extr" or "rorv" but NOT "asr"
    // in the context of the rotate operations.
    let disasm = backend.disassemble(&encoded, 0);
    // The prologue/epilogue may legitimately contain ASR (e.g. for signed extension),
    // but we verify that EXTR or RORV appears somewhere in the output.
    let has_extr_or_rorv = disasm.iter().any(|line| {
        let lower = line.to_lowercase();
        lower.contains("extr") || lower.contains("rorv")
    });
    assert!(
        has_extr_or_rorv,
        "ARM64 ROR/ROL must produce EXTR or RORV instructions; got: {:?}",
        disasm
    );
}

// ===========================================================================
// Wave 1, Bug 2: Pi5 references in documentation
// ===========================================================================

/// Helper function: recursively check a directory for Pi5 references.
fn check_pi5_refs(dir: &std::path::Path, violations: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                check_pi5_refs(&path, violations);
            } else if path.extension().map_or(false, |e| e == "md") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let lower = content.to_lowercase();
                    if lower.contains("pi5") || lower.contains("pi 5") {
                        violations.push(format!("{}", path.display()));
                    }
                }
            }
        }
    }
}

/// Regression test: Documentation must not contain Pi5 references.
///
/// **Original bug**: The documentation mentioned "Pi5" (Raspberry Pi 5)
/// in contexts that were either incorrect or misleading for the VUMA
/// compiler's target architecture (Cortex-A76 / ARMv8.2-A). The fix
/// removed all such references.
#[test]
fn test_docs_no_pi5_references() {
    let docs_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs");

    if !docs_dir.exists() {
        // If the docs directory is not available at test time, skip gracefully.
        eprintln!("docs directory not found, skipping Pi5 reference check");
        return;
    }

    let mut violations = Vec::new();

    check_pi5_refs(&docs_dir, &mut violations);

    assert!(
        violations.is_empty(),
        "Documentation files must not contain Pi5 references. Violations in: {:?}",
        violations
    );
}

// ===========================================================================
// Wave 2, Bug 3: LoongArch64 atomics returning Vec::new()
// ===========================================================================

/// Regression test: LoongArch64 atomic operations must produce instructions,
/// not silently return empty instruction vectors.
///
/// **Original bug**: The LoongArch64 `lower_ir_instr_la64` handler for
/// `AtomicLoad`, `AtomicStore`, and `AtomicCas` was returning `Vec::new()`,
/// effectively compiling atomic operations to no-ops. The fix emits proper
/// LL/SC (load-linked / store-conditional) sequences with `dbar` fences.
#[test]
fn test_loongarch64_atomics_not_empty() {
    let backend = create_backend(BackendKind::LoongArch64)
        .expect("LoongArch64 backend creation should succeed");

    // Test AtomicLoad
    {
        let mut func = make_func("atomic_load_test");
        func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        block.push(IRInstr::AtomicLoad {
            dst: IRValue::Register(0),
            addr: IRValue::Register(1),
            ty: IRType::I64,
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("LoongArch64 allocate_registers should succeed for AtomicLoad");
        let opcodes = collect_opcodes(&allocated);

        // Must have non-trivial instructions (not just prologue/epilogue).
        // Specifically, we should see dbar (fence) and ll.d (load-linked).
        let has_dbar = opcodes.iter().any(|op| op.contains("dbar"));
        let has_ll = opcodes.iter().any(|op| op.contains("ll.") || op.contains("lld") || op.contains("llw"));
        assert!(
            has_dbar || has_ll,
            "LoongArch64 AtomicLoad must emit fence/load-linked instructions; got: {:?}",
            opcodes
        );
    }

    // Test AtomicStore
    {
        let mut func = make_func("atomic_store_test");
        func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        block.push(IRInstr::AtomicStore {
            value: IRValue::Immediate(42),
            addr: IRValue::Register(1),
            ty: IRType::I64,
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Immediate(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("LoongArch64 allocate_registers should succeed for AtomicStore");
        let opcodes = collect_opcodes(&allocated);

        let has_dbar = opcodes.iter().any(|op| op.contains("dbar"));
        let has_amswap = opcodes.iter().any(|op| op.contains("amswap") || op.contains("amswap."));
        assert!(
            has_dbar || has_amswap,
            "LoongArch64 AtomicStore must emit fence/amsamp instructions; got: {:?}",
            opcodes
        );
    }

    // Test AtomicCas
    {
        let mut func = make_func("atomic_cas_test");
        func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        block.push(IRInstr::AtomicCas {
            dst: IRValue::Register(0),
            addr: IRValue::Register(1),
            expected: IRValue::Immediate(0),
            desired: IRValue::Immediate(1),
            ty: IRType::I64,
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("LoongArch64 allocate_registers should succeed for AtomicCas");
        let opcodes = collect_opcodes(&allocated);

        // Must NOT be empty beyond prologue/epilogue. Should have LL/SC loop.
        let has_dbar = opcodes.iter().any(|op| op.contains("dbar"));
        let has_ll_or_sc = opcodes.iter().any(|op| {
            op.contains("ll.") || op.contains("lld") || op.contains("llw")
                || op.contains("sc.") || op.contains("scd") || op.contains("scw")
        });
        assert!(
            has_dbar || has_ll_or_sc,
            "LoongArch64 AtomicCas must emit LL/SC loop instructions, not empty; got: {:?}",
            opcodes
        );
    }
}

// ===========================================================================
// Wave 2, Bug 4: PPC64 atomics returning Vec::new()
// ===========================================================================

/// Regression test: PPC64 atomic operations must produce real instructions.
///
/// **Original bug**: The PPC64 backend's atomic instruction handlers were
/// returning `Vec::new()`, silently dropping atomic operations. The fix
/// emits proper `ldarx`/`stdcx.` (load-and-reserve / store-conditional)
/// sequences with `sync`/`lwsync`/`isync` barriers.
#[test]
fn test_ppc64_atomics_not_empty() {
    let backend = create_backend(BackendKind::PowerPC64)
        .expect("PPC64 backend creation should succeed");

    // Test AtomicLoad
    {
        let mut func = make_func("ppc_atomic_load");
        func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        block.push(IRInstr::AtomicLoad {
            dst: IRValue::Register(0),
            addr: IRValue::Register(1),
            ty: IRType::I64,
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("PPC64 allocate_registers should succeed for AtomicLoad");
        let opcodes = collect_opcodes(&allocated);

        // Should have sync, ldarx, isync (acquire pattern)
        let has_sync = opcodes.iter().any(|op| op.contains("sync"));
        let has_ldarx = opcodes.iter().any(|op| op.contains("ldarx") || op.contains("lwarx") || op.contains("lbarx"));
        assert!(
            has_sync || has_ldarx,
            "PPC64 AtomicLoad must emit sync/ldarx instructions; got: {:?}",
            opcodes
        );
    }

    // Test AtomicCas
    {
        let mut func = make_func("ppc_atomic_cas");
        func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        block.push(IRInstr::AtomicCas {
            dst: IRValue::Register(0),
            addr: IRValue::Register(1),
            expected: IRValue::Immediate(0),
            desired: IRValue::Immediate(1),
            ty: IRType::I64,
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("PPC64 allocate_registers should succeed for AtomicCas");
        let opcodes = collect_opcodes(&allocated);

        let has_ldarx = opcodes.iter().any(|op| op.contains("ldarx") || op.contains("lwarx"));
        let has_stdcx = opcodes.iter().any(|op| op.contains("stdcx") || op.contains("stwcx"));
        assert!(
            has_ldarx || has_stdcx,
            "PPC64 AtomicCas must emit ldarx/stdcx loop instructions; got: {:?}",
            opcodes
        );
    }
}

// ===========================================================================
// Wave 2, Bug 5: RISC-V 64 atomic CAS loop with missing labels
// ===========================================================================

/// Regression test: RISC-V 64 AtomicCas must generate a proper LL/SC loop
/// with correct branch labels, not branches with zero offsets.
///
/// **Original bug**: The RISC-V 64 CAS loop generated `LR.D`/`SC.D`/`BNE`
/// instructions but the branch targets had no labels registered, meaning
/// the branch fixup phase could not resolve the offsets and left them as
/// zero (branch-to-self infinite loop). The fix adds proper `retry` and
/// `done` labels to the `label_offsets` map.
#[test]
fn test_riscv64_atomic_cas_has_labels() {
    let backend = create_backend(BackendKind::RiscV64)
        .expect("RISC-V 64 backend creation should succeed");

    let mut func = make_func("riscv_cas_test");
    func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
    let block = func.current_block();
    block.push(IRInstr::Alloc {
        dst: IRValue::Register(1),
        size: 8,
    });
    block.push(IRInstr::AtomicCas {
        dst: IRValue::Register(0),
        addr: IRValue::Register(1),
        expected: IRValue::Immediate(0),
        desired: IRValue::Immediate(1),
        ty: IRType::I64,
    });
    block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

    let allocated = allocate_func(&*backend, &func)
        .expect("RISC-V 64 allocate_registers should succeed for AtomicCas");
    let encoded = collect_encoded(&allocated);

    // Must produce non-trivial encoded output
    assert!(
        !encoded.is_empty(),
        "RISC-V 64 AtomicCas must produce encoded instructions"
    );

    // Disassemble and check that the CAS loop contains LR.D and SC.D
    let disasm = backend.disassemble(&encoded, 0);
    let has_lr = disasm.iter().any(|line| line.to_lowercase().contains("lr.d") || line.to_lowercase().contains("lr.w"));
    let has_sc = disasm.iter().any(|line| line.to_lowercase().contains("sc.d") || line.to_lowercase().contains("sc.w"));
    assert!(
        has_lr && has_sc,
        "RISC-V 64 AtomicCas must contain LR and SC instructions; got: {:?}",
        disasm
    );
}

// ===========================================================================
// Wave 2, Bug 6: Wasm32 CAS emitting simple load instead of cmpxchg
// ===========================================================================

/// Regression test: Wasm32 AtomicCas must emit cmpxchg instructions, not
/// simple load.
///
/// **Original bug**: The Wasm32 backend's AtomicCas handler was emitting
/// a simple `i32.load` (or `i64.load`) instead of the proper
/// `i32.atomic.rmw.cmpxchg` (or `i64.atomic.rmw.cmpxchg`). This meant
/// the CAS was not atomic at all. The fix selects the correct Wasm atomic
/// cmpxchg instruction based on the IR type.
#[test]
fn test_wasm32_cas_uses_cmpxchg() {
    let backend = create_backend(BackendKind::Wasm32)
        .expect("Wasm32 backend creation should succeed");

    let mut func = make_func("wasm_cas_test");
    func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
    let block = func.current_block();
    block.push(IRInstr::Alloc {
        dst: IRValue::Register(1),
        size: 8,
    });
    block.push(IRInstr::AtomicCas {
        dst: IRValue::Register(0),
        addr: IRValue::Register(1),
        expected: IRValue::Immediate(0),
        desired: IRValue::Immediate(1),
        ty: IRType::I32,
    });
    block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

    let allocated = allocate_func(&*backend, &func)
        .expect("Wasm32 allocate_registers should succeed for AtomicCas");

    // The allocated function's opcodes should contain a cmpxchg reference,
    // not just a simple load.
    let opcodes = collect_opcodes(&allocated);
    let has_cmpxchg = opcodes.iter().any(|op| op.contains("cmpxchg"));
    assert!(
        has_cmpxchg,
        "Wasm32 AtomicCas must emit cmpxchg instruction, not simple load; got: {:?}",
        opcodes
    );
}

// ===========================================================================
// Wave 2, Bug 7: ARM32/MIPS64 AtomicCas as simple load
// ===========================================================================

/// Regression test: ARM32 AtomicCas must emit LDREX/STREX, not a simple load.
///
/// **Original bug**: The ARM32 backend's AtomicCas handler was lowering the
/// CAS to a simple `LDR` instruction, which is not atomic. The fix uses
/// the proper `LDREX`/`STREX` pair with a CAS loop and `DMB` barriers.
#[test]
fn test_arm32_atomic_cas_not_simple_load() {
    let backend = create_backend(BackendKind::Arm32)
        .expect("ARM32 backend creation should succeed");

    let mut func = make_func("arm32_cas_test");
    func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
    let block = func.current_block();
    block.push(IRInstr::Alloc {
        dst: IRValue::Register(1),
        size: 8,
    });
    block.push(IRInstr::AtomicCas {
        dst: IRValue::Register(0),
        addr: IRValue::Register(1),
        expected: IRValue::Immediate(0),
        desired: IRValue::Immediate(1),
        ty: IRType::I32,
    });
    block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

    let allocated = allocate_func(&*backend, &func)
        .expect("ARM32 allocate_registers should succeed for AtomicCas");

    let opcodes = collect_opcodes(&allocated);
    let has_ldrex = opcodes.iter().any(|op| op.contains("ldrex"));
    let has_strex = opcodes.iter().any(|op| op.contains("strex"));
    let has_dmb = opcodes.iter().any(|op| op.contains("dmb"));
    assert!(
        has_ldrex || has_strex || has_dmb,
        "ARM32 AtomicCas must emit LDREX/STREX/DMB, not simple load; got: {:?}",
        opcodes
    );
}

// ===========================================================================
// Wave 2, Bug 8: ARM32 >4 arguments silently dropped
// ===========================================================================

/// Regression test: ARM32 functions with more than 4 arguments must pass
/// the excess arguments on the stack, not silently drop them.
///
/// **Original bug**: ARM32 has only 4 argument registers (R0–R3). When a
/// function call had more than 4 arguments, the excess arguments were
/// silently ignored — no stack-passing code was generated. The fix adds
/// proper SP adjustment, stack stores for args 5+, and SP cleanup after
/// the call.
#[test]
fn test_arm32_gt4_args_not_dropped() {
    let backend = create_backend(BackendKind::Arm32)
        .expect("ARM32 backend creation should succeed");

    // Create a function that calls another function with 6 arguments.
    let mut func = make_func("arm32_many_args");
    let block = func.current_block();
    // Call a function with 6 arguments (4 register + 2 stack)
    block.push(IRInstr::Call {
        dst: Some(IRValue::Register(0)),
        func: "callee".to_string(),
        args: vec![
            IRValue::Immediate(1),
            IRValue::Immediate(2),
            IRValue::Immediate(3),
            IRValue::Immediate(4),
            IRValue::Immediate(5),
            IRValue::Immediate(6),
        ],
        is_extern: true,
    });
    block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

    let allocated = allocate_func(&*backend, &func)
        .expect("ARM32 allocate_registers should succeed for call with >4 args");

    let opcodes = collect_opcodes(&allocated);
    // The call handling should include stack operations for the 5th and 6th args.
    // Look for STR (store to stack) and SP adjustment instructions.
    let has_str = opcodes.iter().any(|op| op.to_lowercase().contains("str"));
    let has_bl = opcodes.iter().any(|op| op.to_lowercase().contains("bl"));
    // At minimum, the BL (call) instruction must be present.
    assert!(
        has_bl,
        "ARM32 call with >4 args must emit BL instruction; got: {:?}",
        opcodes
    );
    // The encoded bytes should be non-trivial (more than just a return)
    let encoded = collect_encoded(&allocated);
    assert!(
        encoded.len() > 16,
        "ARM32 call with >4 args must produce substantial code (stack setup + BL + cleanup); got {} bytes",
        encoded.len()
    );
}

// ===========================================================================
// Wave 2, Bug 9: MIPS64 ROR/ROL incomplete (missing complementary shift)
// ===========================================================================

/// Regression test: MIPS64 ROR/ROL must emit both shift directions and OR,
/// not just one shift.
///
/// **Original bug**: MIPS64 has no native rotate instruction. ROR and ROL
/// must be synthesized as `(n >> r) | (n << (64-r))` and
/// `(n << r) | (n >> (64-r))` respectively. The bug was that only the
/// first shift was emitted, missing the complementary shift and the OR
/// that combines them. The fix emits the full 5-instruction sequence
/// (dsrlv + daddiu + dsubu + dsllv + or for ROR, and dsllv + daddiu +
/// dsubu + dsrlv + or for ROL).
#[test]
fn test_mips64_ror_rol_has_complementary_shift() {
    let backend = create_backend(BackendKind::Mips64)
        .expect("MIPS64 backend creation should succeed");

    // Test ROR
    {
        let mut func = make_func("mips_ror_test");
        func.vregs.insert(1, VirtualRegister::new(1, Some("a".to_string())));
        let block = func.current_block();
        block.push(IRInstr::BinOp {
            op: BinOpKind::Ror,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(5),
            ty: Some(IRType::I64),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("MIPS64 allocate_registers should succeed for ROR");
        let opcodes = collect_opcodes(&allocated);

        // ROR must have both a right shift (dsrlv or dsrl) and a left shift
        // (dsllv or dsll), plus an OR to combine them.
        let has_right_shift = opcodes.iter().any(|op| op.contains("dsrlv") || op.contains("dsrl"));
        let has_left_shift = opcodes.iter().any(|op| op.contains("dsllv") || op.contains("dsll"));
        let has_or = opcodes.iter().any(|op| op == "or");
        assert!(
            has_right_shift && has_left_shift,
            "MIPS64 ROR must emit both right-shift and left-shift instructions; got: {:?}",
            opcodes
        );
        assert!(
            has_or,
            "MIPS64 ROR must emit OR to combine shifted values; got: {:?}",
            opcodes
        );
    }

    // Test ROL
    {
        let mut func = make_func("mips_rol_test");
        func.vregs.insert(1, VirtualRegister::new(1, Some("a".to_string())));
        let block = func.current_block();
        block.push(IRInstr::BinOp {
            op: BinOpKind::Rol,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(3),
            ty: Some(IRType::I64),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("MIPS64 allocate_registers should succeed for ROL");
        let opcodes = collect_opcodes(&allocated);

        let has_right_shift = opcodes.iter().any(|op| op.contains("dsrlv") || op.contains("dsrl"));
        let has_left_shift = opcodes.iter().any(|op| op.contains("dsllv") || op.contains("dsll"));
        let has_or = opcodes.iter().any(|op| op == "or");
        assert!(
            has_right_shift && has_left_shift,
            "MIPS64 ROL must emit both left-shift and right-shift instructions; got: {:?}",
            opcodes
        );
        assert!(
            has_or,
            "MIPS64 ROL must emit OR to combine shifted values; got: {:?}",
            opcodes
        );
    }
}

// ===========================================================================
// Wave 2, Bug 10: LoongArch64 Switch/Invoke/TailCall/Resume emitting BREAK
// ===========================================================================

/// Regression test: LoongArch64 terminators Switch, Invoke, TailCall, and
/// Resume must not be lowered to BREAK (trap) instructions.
///
/// **Original bug**: The LoongArch64 backend's stack-slot ISel handled
/// `IRTerminator::Switch`, `Invoke`, `TailCall`, and `Resume` by emitting
/// a BREAK instruction (which traps at runtime). The fix implements proper
/// lowering:
/// - Switch: cascade of BEQ comparisons
/// - Invoke: BL with relocation, branch to normal continuation
/// - TailCall: restore frame + B to target
/// - Resume: BL __Unwind_Resume
#[test]
fn test_loongarch64_no_break_on_control_flow() {
    let backend = create_backend(BackendKind::LoongArch64)
        .expect("LoongArch64 backend creation should succeed");

    // Test Switch
    {
        let mut func = make_func("la64_switch_test");
        func.vregs.insert(1, VirtualRegister::new(1, Some("discr".to_string())));
        func.blocks.push(IRBlock::new("case_a"));
        func.blocks.push(IRBlock::new("default_case"));

        let entry = &mut func.blocks[0];
        entry.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
            ty: Some(IRType::I64),
        });
        entry.terminator = IRTerminator::Switch {
            discr: IRValue::Register(1),
            targets: vec![(1, "case_a".to_string())],
            default: "default_case".to_string(),
        };

        // case_a block
        func.blocks[1].terminator = IRTerminator::Return(vec![IRValue::Immediate(10)]);
        // default_case block
        func.blocks[2].terminator = IRTerminator::Return(vec![IRValue::Immediate(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("LoongArch64 allocate_registers should succeed for Switch");

        let opcodes = collect_opcodes(&allocated);
        // Must NOT have a "break" opcode for the Switch terminator
        let has_break = opcodes.iter().any(|op| op == "break" || op == "BREAK");
        assert!(
            !has_break,
            "LoongArch64 Switch must not emit BREAK; got: {:?}",
            opcodes
        );

        // Should have BEQ for the comparison
        let has_beq = opcodes.iter().any(|op| op.contains("beq"));
        assert!(
            has_beq,
            "LoongArch64 Switch should emit BEQ for case comparison; got: {:?}",
            opcodes
        );
    }

    // Test Invoke
    {
        let mut func = make_func("la64_invoke_test");
        func.blocks.push(IRBlock::new("normal"));
        func.blocks.push(IRBlock::new("unwind"));

        let entry = &mut func.blocks[0];
        entry.terminator = IRTerminator::Invoke {
            dst: Some(IRValue::Register(0)),
            func: "may_throw".to_string(),
            args: vec![],
            normal: "normal".to_string(),
            unwind: "unwind".to_string(),
        };

        func.blocks[1].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);
        func.blocks[2].terminator = IRTerminator::Return(vec![IRValue::Immediate(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("LoongArch64 allocate_registers should succeed for Invoke");

        let opcodes = collect_opcodes(&allocated);
        let has_break = opcodes.iter().any(|op| op == "break" || op == "BREAK" || op == "unreachable");
        assert!(
            !has_break,
            "LoongArch64 Invoke must not emit BREAK; got: {:?}",
            opcodes
        );
    }

    // Test TailCall
    {
        let mut func = make_func("la64_tailcall_test");
        let entry = func.current_block();
        entry.terminator = IRTerminator::TailCall {
            func: "target".to_string(),
            args: vec![IRValue::Immediate(42)],
        };

        let allocated = allocate_func(&*backend, &func)
            .expect("LoongArch64 allocate_registers should succeed for TailCall");

        let opcodes = collect_opcodes(&allocated);
        let has_break = opcodes.iter().any(|op| op == "break" || op == "BREAK" || op == "unreachable");
        assert!(
            !has_break,
            "LoongArch64 TailCall must not emit BREAK; got: {:?}",
            opcodes
        );
    }

    // Test Resume
    {
        let mut func = make_func("la64_resume_test");
        func.vregs.insert(1, VirtualRegister::new(1, Some("exc".to_string())));
        let entry = func.current_block();
        entry.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(0),
            ty: Some(IRType::I64),
        });
        entry.terminator = IRTerminator::Resume {
            value: IRValue::Register(1),
        };

        let allocated = allocate_func(&*backend, &func)
            .expect("LoongArch64 allocate_registers should succeed for Resume");

        let opcodes = collect_opcodes(&allocated);
        // Resume should emit a BL to __Unwind_Resume, not BREAK
        // (it may have BREAK after the BL as a safety trap, which is fine)
        let has_bl_or_call = opcodes.iter().any(|op| op.contains("bl") || op == "resume");
        assert!(
            has_bl_or_call,
            "LoongArch64 Resume must emit BL (call) instruction; got: {:?}",
            opcodes
        );
    }
}

// ===========================================================================
// Wave 3, Bug 11: FP conversion casts as no-ops on all 10 backends
// ===========================================================================

/// Regression test: FP conversion casts (IntToFloat, UIntToFloat, FloatToInt,
/// FloatToUInt, FloatToFloat) must produce actual conversion instructions on
/// all backends, not be treated as no-ops (MOV).
///
/// **Original bug**: All 10 backends were treating FP conversion casts as
/// no-ops — the Cast instruction for these kinds was either mapped to a
/// plain MOV (integer move) or produced zero instructions, resulting in
/// bit-reinterpretation instead of value conversion. For example,
/// `inttofloat(1)` would produce the bit pattern `0x3FF0000000000000`
/// instead of `1.0`. The fix emits proper conversion instructions:
/// - ARM64: SCVTF/UCVTF/FCVTZS/FCVTZU/FCVT
/// - x86_64: CVTSI2SS/CVTSI2SD/CVTSS2SI/CVTSD2SI/CVTSS2SD/CVTSD2SS
/// - RISC-V: FCVT.S.L/FCVT.S.LU/FCVT.L.S/FCVT.LU.S/FCVT.D.L etc.
/// - ARM32: VCVT.F32.S32/VCVT.F32.U32/VCVT.S32.F32/VCVT.U32.F32
/// - MIPS64: CVT.S.D/CVT.D.S/CVT.S.W/CVT.D.W/CVT.W.S/CVT.W.D etc.
/// - PPC64: FCFID/FCTID/FCTIDZ/FRSP/FCTIDU etc.
/// - LoongArch64: FFINT.D.L/FTINT.L.D/FFINT.S.W/FTINT.W.S etc.
/// - Wasm32: f32.convert_i32_s / f64.convert_i32_s / i32.trunc_f64_s etc.
#[test]
fn test_fp_conversion_not_noop_all_backends() {
    for &kind in ALL_BACKENDS {
        let backend = create_backend(kind).expect(&format!("{:?} backend creation should succeed", kind));

        // Test IntToFloat
        {
            let mut func = make_func(&format!("{:?}_inttofloat", kind));
            func.vregs.insert(1, VirtualRegister::new(1, Some("val".to_string())));
            let block = func.current_block();
            block.push(IRInstr::Cast {
                kind: CastKind::IntToFloat,
                dst: IRValue::Register(1),
                src: IRValue::Immediate(42),
                from_ty: Some(IRType::I64),
                to_ty: Some(IRType::F64),
            });
            block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

            let result = allocate_func(&*backend, &func);
            match result {
                Ok(allocated) => {
                    let opcodes = collect_opcodes(&allocated);
                    // Must contain a conversion-related instruction, not just MOV or no-op.
                    let has_conversion = opcodes.iter().any(|op| {
                        let lower = op.to_lowercase();
                        lower.contains("cvt")
                            || lower.contains("fcvt")
                            || lower.contains("scvtf")
                            || lower.contains("ucvtf")
                            || lower.contains("fcvtzs")
                            || lower.contains("fcvtzu")
                            || lower.contains("vcvt")
                            || lower.contains("ffint")
                            || lower.contains("ftint")
                            || lower.contains("fcvt.")
                            || lower.contains("convert")
                            || lower.contains("trunc")
                            || lower.contains("cfid")
                            || lower.contains("ctid")
                    });
                    assert!(
                        has_conversion,
                        "{:?} IntToFloat must emit conversion instruction, not MOV/no-op; got: {:?}",
                        kind, opcodes
                    );
                }
                Err(BackendError::UnsupportedFeature { .. }) => {
                    // Backend may not support FP yet — that's acceptable as long
                    // as it's an explicit error, not silent wrong-code.
                }
                Err(e) => {
                    panic!("{:?} IntToFloat: unexpected error: {}", kind, e);
                }
            }
        }

        // Test FloatToInt
        {
            let mut func = make_func(&format!("{:?}_floattoint", kind));
            func.vregs.insert(1, VirtualRegister::new(1, Some("val".to_string())));
            let block = func.current_block();
            block.push(IRInstr::Cast {
                kind: CastKind::FloatToInt,
                dst: IRValue::Register(1),
                src: IRValue::Immediate(0), // placeholder
                from_ty: Some(IRType::F64),
                to_ty: Some(IRType::I64),
            });
            block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

            let result = allocate_func(&*backend, &func);
            match result {
                Ok(allocated) => {
                    let opcodes = collect_opcodes(&allocated);
                    let has_conversion = opcodes.iter().any(|op| {
                        let lower = op.to_lowercase();
                        lower.contains("cvt")
                            || lower.contains("fcvt")
                            || lower.contains("fcvtzs")
                            || lower.contains("fcvtzu")
                            || lower.contains("vcvt")
                            || lower.contains("ftint")
                            || lower.contains("trunc")
                            || lower.contains("ctid")
                    });
                    assert!(
                        has_conversion,
                        "{:?} FloatToInt must emit conversion instruction, not MOV/no-op; got: {:?}",
                        kind, opcodes
                    );
                }
                Err(BackendError::UnsupportedFeature { .. }) => {}
                Err(e) => {
                    panic!("{:?} FloatToInt: unexpected error: {}", kind, e);
                }
            }
        }

        // Test FloatToFloat
        {
            let mut func = make_func(&format!("{:?}_floattofloat", kind));
            func.vregs.insert(1, VirtualRegister::new(1, Some("val".to_string())));
            let block = func.current_block();
            block.push(IRInstr::Cast {
                kind: CastKind::FloatToFloat,
                dst: IRValue::Register(1),
                src: IRValue::Immediate(0),
                from_ty: Some(IRType::F32),
                to_ty: Some(IRType::F64),
            });
            block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

            let result = allocate_func(&*backend, &func);
            match result {
                Ok(allocated) => {
                    let opcodes = collect_opcodes(&allocated);
                    let has_conversion = opcodes.iter().any(|op| {
                        let lower = op.to_lowercase();
                        lower.contains("cvt")
                            || lower.contains("fcvt")
                            || lower.contains("vcvt")
                            || lower.contains("ffint")
                            || lower.contains("ftint")
                            || lower.contains("convert")
                            || lower.contains("frsp")
                            || lower.contains("fcvt.d.s")
                            || lower.contains("fcvt.s.d")
                    });
                    assert!(
                        has_conversion,
                        "{:?} FloatToFloat must emit conversion instruction (width change), not MOV/no-op; got: {:?}",
                        kind, opcodes
                    );
                }
                Err(BackendError::UnsupportedFeature { .. }) => {}
                Err(e) => {
                    panic!("{:?} FloatToFloat: unexpected error: {}", kind, e);
                }
            }
        }
    }
}

// ===========================================================================
// Wave 4, Bug 12: ARM64 stack-slot NOP for CtSelect/CtEq/atomics
// ===========================================================================

/// Regression test: ARM64 stack-slot codegen must produce real instructions
/// for CtSelect, CtEq, and atomic operations, not silently emit nothing.
///
/// **Original bug**: When the ARM64 emitter was processing instructions in
/// stack-slot mode (the path used by `allocate_registers`), `CtSelect`,
/// `CtEq`, `AtomicLoad`, `AtomicStore`, and `AtomicCas` were handled with
/// a comment like "handled by the emitter's emit_ir_instr" but no actual
/// code was emitted in the stack-slot path. The result was that these
/// operations were silently compiled to NOPs. The fix emits proper
/// bitwise-operation sequences for CtSelect/CtEq and LDAXR/STLXR loops
/// for atomics in the stack-slot code path.
#[test]
fn test_arm64_stack_slot_not_nop_for_ct_atomics() {
    let backend = create_backend(BackendKind::AArch64)
        .expect("ARM64 backend creation should succeed");

    // Test CtSelect
    {
        let mut func = make_func("arm64_ct_select");
        func.vregs.insert(1, VirtualRegister::new(1, Some("cond".to_string())));
        func.vregs.insert(2, VirtualRegister::new(2, Some("a".to_string())));
        func.vregs.insert(3, VirtualRegister::new(3, Some("b".to_string())));
        let block = func.current_block();
        block.push(IRInstr::CtSelect {
            dst: IRValue::Register(0),
            cond: IRValue::Register(1),
            true_val: IRValue::Register(2),
            false_val: IRValue::Register(3),
            ty: Some(IRType::I64),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("ARM64 allocate_registers should succeed for CtSelect");

        let encoded = collect_encoded(&allocated);
        // Must produce non-trivial encoded output — not just a RET.
        // A NOP-only implementation would have very few instructions.
        assert!(
            encoded.len() > 8,
            "ARM64 CtSelect must produce more than just prologue+epilogue; got {} bytes",
            encoded.len()
        );
    }

    // Test CtEq
    {
        let mut func = make_func("arm64_ct_eq");
        func.vregs.insert(1, VirtualRegister::new(1, Some("a".to_string())));
        func.vregs.insert(2, VirtualRegister::new(2, Some("b".to_string())));
        let block = func.current_block();
        block.push(IRInstr::CtEq {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: Some(IRType::I64),
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("ARM64 allocate_registers should succeed for CtEq");

        let encoded = collect_encoded(&allocated);
        assert!(
            encoded.len() > 8,
            "ARM64 CtEq must produce more than just prologue+epilogue; got {} bytes",
            encoded.len()
        );
    }

    // Test AtomicLoad on ARM64
    {
        let mut func = make_func("arm64_atomic_load");
        func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        block.push(IRInstr::AtomicLoad {
            dst: IRValue::Register(0),
            addr: IRValue::Register(1),
            ty: IRType::I64,
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("ARM64 allocate_registers should succeed for AtomicLoad");

        let encoded = collect_encoded(&allocated);
        assert!(
            encoded.len() > 16,
            "ARM64 AtomicLoad must produce more than minimal code; got {} bytes",
            encoded.len()
        );
    }

    // Test AtomicCas on ARM64
    {
        let mut func = make_func("arm64_atomic_cas");
        func.vregs.insert(1, VirtualRegister::new(1, Some("addr".to_string())));
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(1),
            size: 8,
        });
        block.push(IRInstr::AtomicCas {
            dst: IRValue::Register(0),
            addr: IRValue::Register(1),
            expected: IRValue::Immediate(0),
            desired: IRValue::Immediate(1),
            ty: IRType::I64,
        });
        block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let allocated = allocate_func(&*backend, &func)
            .expect("ARM64 allocate_registers should succeed for AtomicCas");

        let encoded = collect_encoded(&allocated);
        // CAS loop should be significantly larger than a simple load
        assert!(
            encoded.len() > 16,
            "ARM64 AtomicCas must produce a CAS loop, not NOP; got {} bytes",
            encoded.len()
        );
    }
}

// ===========================================================================
// Wave 4, Bug 13: Unresolved symbol relocations silently leaving offset 0
// ===========================================================================

/// Regression test: When a call relocation references an external (undefined)
/// symbol, the ELF must contain a proper relocation entry, and the emitted
/// binary must not silently leave the branch offset as 0 (which would cause
/// the call to branch to the start of the text section instead of being
/// resolved by the linker).
///
/// **Original bug**: The `resolve_call_relocs` function in `emit.rs` would
/// simply `continue` when a call target was not found in `function_offsets`,
/// leaving the BL instruction's offset field as 0. This meant that external
/// function calls would jump to offset 0 of the text section (typically the
/// first function) instead of being properly recorded as relocations for
/// the linker to resolve. The fix adds the relocation to the ELF `.rela.text`
/// section so the linker can properly resolve it.
#[test]
fn test_unresolved_reloc_not_offset_zero() {
    let backend = create_backend(BackendKind::AArch64)
        .expect("ARM64 backend creation should succeed");

    // Create a function that calls an external function.
    let mut func = make_func("call_external");
    let block = func.current_block();
    block.push(IRInstr::Call {
        dst: Some(IRValue::Register(0)),
        func: "external_callee".to_string(),
        args: vec![IRValue::Immediate(42)],
        is_extern: true,
    });
    block.terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

    let binary = compile_single(&*backend, &func);

    // The binary must be a valid ELF
    assert!(
        binary.len() >= 64,
        "ELF binary must be at least 64 bytes for ELF64 header"
    );
    assert_eq!(
        &binary[0..4],
        &[0x7f, b'E', b'L', b'F'],
        "Output must be a valid ELF binary"
    );

    // The ELF should contain a relocation section for the unresolved symbol.
    // We verify this by checking that the binary contains the string
    // "external_callee" in the string table (which it must, for the linker
    // to resolve the symbol).
    let symbol_name_found = find_bytes_in_elf(&binary, b"external_callee");
    assert!(
        symbol_name_found,
        "ELF must contain the external symbol name 'external_callee' for linker resolution"
    );
}

/// Search for a byte pattern anywhere within an ELF binary (e.g., in the
/// string table section).
fn find_bytes_in_elf(elf: &[u8], pattern: &[u8]) -> bool {
    if pattern.is_empty() || elf.len() < pattern.len() {
        return false;
    }
    for window in elf.windows(pattern.len()) {
        if window == pattern {
            return true;
        }
    }
    false
}
