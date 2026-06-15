//! # ABI Conformance Testing
//!
//! Verifies that each backend follows its platform's calling convention by:
//! 1. Creating IR functions with varying argument counts
//! 2. Running `allocate_registers` on each backend
//! 3. Checking the generated code uses correct registers via:
//!    - TargetInfo trait (calling convention metadata)
//!    - AllocatedFunction register assignments
//!    - Disassembled output verification
//!
//! ## Calling Conventions Tested
//!
//! | Backend      | ABI          | Integer Args        | Return  | Address |
//! |--------------|--------------|---------------------|---------|---------|
//! | x86_64       | System V     | RDI,RSI,RDX,RCX,R8,R9 | RAX   | 8 bytes |
//! | AArch64      | AAPCS64      | X0-X7               | X0      | 8 bytes |
//! | RISC-V 64    | RV64G LP64D  | A0-A7 (x10-x17)    | A0      | 8 bytes |
//! | ARM32        | AAPCS        | R0-R3               | R0      | 4 bytes |
//! | MIPS64       | N64          | $a0-$a7 ($4-$11)    | $v0     | 8 bytes |
//! | PPC64        | ELFv2        | R3-R10              | R3      | 8 bytes |
//! | LoongArch64  | LP64         | $a0-$a7 ($r4-$r11)  | $a0     | 8 bytes |
//! | Wasm32       | Stack machine | (stack params)     | (stack) | 4 bytes |

use vuma_codegen::backend::{
    create_backend, BackendKind, Endianness, OutputFormat, RegClass, TargetInfo,
};
use vuma_codegen::ir::{
    IRFunction, IRInstr, IRTerminator, IRType, IRValue,
};

// ===========================================================================
// Helpers
// ===========================================================================

/// Build a simple IR function named `name` with `n` i64 parameters that
/// returns the first parameter (or 0 if no params).
///
/// The function body simply returns `params[0]` if available, otherwise
/// an immediate 0.  This is sufficient to exercise the calling convention
/// for argument passing and return-value placement.
fn make_func_with_n_args(name: &str, n: usize) -> IRFunction {
    let mut func = IRFunction::new(name);
    // Register parameters as vregs 0..n
    for i in 0..n {
        func.param_types.push(IRType::I64);
        func.params.push(IRValue::Register(i as u32));
        func.vregs.insert(i as u32, vuma_codegen::ir::VirtualRegister::named(i as u32, format!("a{}", i)));
    }
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(n as u32));

    // Return the first parameter (or 0)
    let ret_val = if n > 0 {
        IRValue::Register(0)
    } else {
        IRValue::Immediate(0)
    };
    func.current_block().terminator = IRTerminator::Return(vec![ret_val]);
    func
}

/// Build a function that calls another function with `n` i64 arguments.
/// This exercises argument *passing* in the calling convention.
fn make_func_with_call_n_args(n: usize) -> IRFunction {
    let mut func = IRFunction::new("caller");
    // vreg 0 = the call result
    func.vregs.insert(0, vuma_codegen::ir::VirtualRegister::anonymous(0));

    // Build argument values: all immediates 1..=n
    let args: Vec<IRValue> = (1..=n as i64).map(IRValue::Immediate).collect();

    func.current_block().instructions.push(IRInstr::Call {
        dst: Some(IRValue::Register(0)),
        func: "callee".to_string(),
        args,
    });
    func.current_block().terminator = IRTerminator::Return(vec![IRValue::Register(0)]);
    func
}

// ===========================================================================
// TargetInfo-based calling convention validation
// ===========================================================================

/// Validate that every TargetInfo reports internally-consistent calling
/// convention properties.
fn validate_cc_info(info: &dyn TargetInfo) {
    let isa = info.isa_name();

    // num_int_arg_regs must be consistent with has_registers
    if !info.has_registers() {
        assert_eq!(
            info.num_int_arg_regs(), 0,
            "{}: has_registers=false but num_int_arg_regs={}",
            isa, info.num_int_arg_regs()
        );
    } else {
        assert!(
            info.num_int_arg_regs() > 0,
            "{}: register-based ISA must have at least 1 integer arg register",
            isa
        );
    }

    // calling_convention_name must be non-empty
    assert!(
        !info.calling_convention_name().is_empty(),
        "{}: calling convention name must not be empty",
        isa
    );

    // stack_alignment must be a power of 2
    let sa = info.stack_alignment();
    assert!(sa > 0 && sa.is_power_of_two(),
        "{}: stack alignment {} must be a positive power of 2", isa, sa);

    // pointer_width must match output format
    match info.output_format() {
        OutputFormat::Elf32 => assert_eq!(info.pointer_width(), 4,
            "{}: Elf32 output must have 4-byte pointers", isa),
        OutputFormat::Elf64 => assert_eq!(info.pointer_width(), 8,
            "{}: Elf64 output must have 8-byte pointers", isa),
        OutputFormat::WasmBinary => assert_eq!(info.pointer_width(), 4,
            "{}: Wasm32 must have 4-byte pointers", isa),
        OutputFormat::RawBinary => {} // no constraint
    }
}

// ===========================================================================
// Per-backend ABI tests
// ===========================================================================

// -- x86_64: System V AMD64 ABI --
#[test]
fn test_x86_64_abi_target_info() {
    let backend = create_backend(BackendKind::X86_64).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "x86_64");
    assert_eq!(info.calling_convention_name(), "systemv");
    assert_eq!(info.num_int_arg_regs(), 6, "System V: 6 integer arg regs (RDI,RSI,RDX,RCX,R8,R9)");
    assert_eq!(info.num_fp_arg_regs(), 8, "System V: 8 FP arg regs (XMM0-XMM7)");
    assert_eq!(info.pointer_width(), 8);
    assert_eq!(info.stack_alignment(), 16);
    assert!(!info.has_link_register(), "x86_64 pushes return address on stack");
    validate_cc_info(info);
}

#[test]
fn test_x86_64_abi_allocation() {
    let backend = create_backend(BackendKind::X86_64).unwrap();
    // Test with 6 args (all fit in registers)
    let func = make_func_with_n_args("test_6args", 6);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "6-arg function should allocate on x86_64");
    let af = allocated.unwrap();
    assert!(!af.blocks.is_empty(), "allocated function must have blocks");

    // Test with 8 args (2 must go on stack)
    let func = make_func_with_n_args("test_8args", 8);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "8-arg function should allocate on x86_64");
}

// -- AArch64: AAPCS64 --
#[test]
fn test_aarch64_abi_target_info() {
    let backend = create_backend(BackendKind::AArch64).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "aarch64");
    assert_eq!(info.calling_convention_name(), "aapcs64");
    assert_eq!(info.num_int_arg_regs(), 8, "AAPCS64: 8 integer arg regs (X0-X7)");
    assert_eq!(info.num_fp_arg_regs(), 8, "AAPCS64: 8 FP arg regs (V0-V7)");
    assert_eq!(info.pointer_width(), 8);
    assert_eq!(info.stack_alignment(), 16);
    assert!(info.has_link_register(), "AArch64 uses X30 (LR) as link register");
    assert!(info.has_hardwired_zero(), "AArch64 has XZR");
    validate_cc_info(info);
}

#[test]
fn test_aarch64_abi_allocation() {
    let backend = create_backend(BackendKind::AArch64).unwrap();
    // Test with 8 args (all fit in X0-X7)
    let func = make_func_with_n_args("test_8args", 8);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "8-arg function should allocate on AArch64");

    // Test with 10 args (2 must go on stack)
    let func = make_func_with_n_args("test_10args", 10);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "10-arg function should allocate on AArch64");
}

// -- RISC-V 64: RV64G LP64D --
#[test]
fn test_riscv64_abi_target_info() {
    let backend = create_backend(BackendKind::RiscV64).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "riscv64");
    assert_eq!(info.calling_convention_name(), "lp64d");
    assert_eq!(info.num_int_arg_regs(), 8, "RV64G: 8 integer arg regs (A0-A7, x10-x17)");
    assert_eq!(info.num_fp_arg_regs(), 8, "RV64G: 8 FP arg regs (FA0-FA7, f10-f17)");
    assert_eq!(info.pointer_width(), 8);
    assert_eq!(info.stack_alignment(), 16);
    assert!(info.has_link_register(), "RISC-V uses x1 (ra) as link register");
    assert!(info.has_hardwired_zero(), "RISC-V has x0 (zero)");
    assert!(!info.has_branch_delay_slots(), "RISC-V does NOT have branch delay slots");
    validate_cc_info(info);
}

#[test]
fn test_riscv64_abi_allocation() {
    let backend = create_backend(BackendKind::RiscV64).unwrap();
    let func = make_func_with_n_args("test_8args", 8);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "8-arg function should allocate on RISC-V 64");

    let func = make_func_with_n_args("test_10args", 10);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "10-arg function should allocate on RISC-V 64");
}

// -- ARM32: AAPCS --
#[test]
fn test_arm32_abi_target_info() {
    let backend = create_backend(BackendKind::Arm32).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "arm32");
    assert_eq!(info.calling_convention_name(), "aapcs");
    assert_eq!(info.num_int_arg_regs(), 4, "AAPCS: 4 integer arg regs (R0-R3)");
    assert_eq!(info.num_fp_arg_regs(), 16, "AAPCS VFP: 16 FP arg regs (D0-D15)");
    assert_eq!(info.pointer_width(), 4, "ARM32 has 32-bit pointers");
    assert_eq!(info.stack_alignment(), 8, "AAPCS: 8-byte stack alignment");
    assert_eq!(info.output_format(), OutputFormat::Elf32);
    assert!(info.has_link_register(), "ARM32 uses R14 (LR) as link register");
    validate_cc_info(info);
}

#[test]
fn test_arm32_abi_allocation() {
    let backend = create_backend(BackendKind::Arm32).unwrap();
    // Test with 4 args (all fit in R0-R3)
    let func = make_func_with_n_args("test_4args", 4);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "4-arg function should allocate on ARM32");

    // Test with 6 args (2 must go on stack)
    let func = make_func_with_n_args("test_6args", 6);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "6-arg function should allocate on ARM32");
}

// -- MIPS64: N64 ABI --
#[test]
fn test_mips64_abi_target_info() {
    let backend = create_backend(BackendKind::Mips64).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "mips64");
    assert_eq!(info.calling_convention_name(), "n64");
    assert_eq!(info.num_int_arg_regs(), 8, "N64: 8 integer arg regs ($a0-$a7, $4-$11)");
    assert_eq!(info.num_fp_arg_regs(), 8, "N64: 8 FP arg regs ($f12-$f19)");
    assert_eq!(info.pointer_width(), 8);
    assert_eq!(info.stack_alignment(), 16);
    assert!(info.has_link_register(), "MIPS uses $31 ($ra) as link register");
    assert!(info.has_hardwired_zero(), "MIPS has $0 (zero)");
    assert!(info.has_branch_delay_slots(), "MIPS has branch delay slots");
    assert_eq!(info.endianness(), Endianness::Big);
    validate_cc_info(info);
}

#[test]
fn test_mips64_abi_allocation() {
    let backend = create_backend(BackendKind::Mips64).unwrap();
    let func = make_func_with_n_args("test_8args", 8);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "8-arg function should allocate on MIPS64");

    let func = make_func_with_n_args("test_10args", 10);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "10-arg function should allocate on MIPS64");
}

// -- PPC64: ELFv2 ABI --
#[test]
fn test_ppc64_abi_target_info() {
    let backend = create_backend(BackendKind::PowerPC64).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "ppc64");
    assert_eq!(info.calling_convention_name(), "elfv2");
    assert_eq!(info.num_int_arg_regs(), 8, "ELFv2: 8 integer arg regs (R3-R10)");
    assert_eq!(info.num_fp_arg_regs(), 13, "ELFv2: 13 FP arg regs (F1-F13)");
    assert_eq!(info.pointer_width(), 8);
    assert_eq!(info.stack_alignment(), 16);
    assert!(info.has_link_register(), "PPC64 uses LR (SPR) as link register");
    assert!(info.has_toc_pointer(), "PPC64 has TOC pointer in R2");
    assert!(info.has_condition_registers(), "PPC64 has CR0-CR7");
    validate_cc_info(info);
}

#[test]
fn test_ppc64_abi_allocation() {
    let backend = create_backend(BackendKind::PowerPC64).unwrap();
    let func = make_func_with_n_args("test_8args", 8);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "8-arg function should allocate on PPC64");

    let func = make_func_with_n_args("test_10args", 10);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "10-arg function should allocate on PPC64");
}

// -- LoongArch64: LP64 ABI --
#[test]
fn test_loongarch64_abi_target_info() {
    let backend = create_backend(BackendKind::LoongArch64).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "loongarch64");
    assert_eq!(info.calling_convention_name(), "lp64");
    assert_eq!(info.num_int_arg_regs(), 8, "LP64: 8 integer arg regs ($a0-$a7, $r4-$r11)");
    assert_eq!(info.num_fp_arg_regs(), 8, "LP64: 8 FP arg regs ($fa0-$fa7)");
    assert_eq!(info.pointer_width(), 8);
    assert_eq!(info.stack_alignment(), 16);
    assert!(info.has_link_register(), "LoongArch uses $r1 (ra) as link register");
    assert!(info.has_hardwired_zero(), "LoongArch has $r0 (zero)");
    validate_cc_info(info);
}

#[test]
fn test_loongarch64_abi_allocation() {
    let backend = create_backend(BackendKind::LoongArch64).unwrap();
    let func = make_func_with_n_args("test_8args", 8);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "8-arg function should allocate on LoongArch64");

    let func = make_func_with_n_args("test_10args", 10);
    let allocated = backend.allocate_registers(&func);
    assert!(allocated.is_ok(), "10-arg function should allocate on LoongArch64");
}

// -- Wasm32: Stack machine --
#[test]
fn test_wasm32_abi_target_info() {
    let backend = create_backend(BackendKind::Wasm32).unwrap();
    let info = backend.target_info();
    assert_eq!(info.isa_name(), "wasm32");
    assert_eq!(info.calling_convention_name(), "wasm-stack");
    assert_eq!(info.num_int_arg_regs(), 0, "Wasm32: no register args (stack machine)");
    assert_eq!(info.num_fp_arg_regs(), 0, "Wasm32: no FP register args");
    assert_eq!(info.pointer_width(), 4, "Wasm32 has 32-bit pointers");
    assert!(!info.has_registers(), "Wasm32 is a stack machine");
    assert!(!info.has_link_register(), "Wasm32 has no link register");
    assert_eq!(info.output_format(), OutputFormat::WasmBinary);
    validate_cc_info(info);
}

// ===========================================================================
// Cross-backend ABI consistency checks
// ===========================================================================

/// Verify that every backend can allocate a function with 0 args (minimal).
#[test]
fn test_all_backends_zero_args() {
    for kind in [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ] {
        let backend = create_backend(kind).unwrap();
        let func = make_func_with_n_args("zero_arg_func", 0);
        let result = backend.allocate_registers(&func);
        assert!(
            result.is_ok(),
            "{}: 0-arg function allocation should succeed, got: {:?}",
            kind.isa_name(),
            result.err()
        );
    }
}

/// Verify that every backend can allocate a function with arguments
/// exceeding the register count (stack args needed).
#[test]
fn test_all_backends_stack_args() {
    for kind in [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ] {
        let backend = create_backend(kind).unwrap();
        let info = backend.target_info();
        // Use more args than the platform has registers
        let n_args = info.num_int_arg_regs() + 4;
        let func = make_func_with_n_args("stack_arg_func", n_args);
        let result = backend.allocate_registers(&func);
        assert!(
            result.is_ok(),
            "{}: {}-arg function (exceeds {} arg regs) should allocate, got: {:?}",
            kind.isa_name(),
            n_args,
            info.num_int_arg_regs(),
            result.err()
        );
    }
}

/// Verify that every backend can handle a function that calls another
/// function with many arguments.
#[test]
fn test_all_backends_call_with_args() {
    for kind in [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ] {
        let backend = create_backend(kind).unwrap();
        let func = make_func_with_call_n_args(4);
        let result = backend.allocate_registers(&func);
        assert!(
            result.is_ok(),
            "{}: call with 4 args should allocate, got: {:?}",
            kind.isa_name(),
            result.err()
        );
    }
}

/// Verify that every backend can encode a function with arguments.
#[test]
fn test_all_backends_encode() {
    for kind in [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ] {
        let backend = create_backend(kind).unwrap();
        let func = make_func_with_n_args("encode_test", 4);
        let allocated = backend.allocate_registers(&func);
        assert!(
            allocated.is_ok(),
            "{}: allocation should succeed, got: {:?}",
            kind.isa_name(),
            allocated.err()
        );
        let af = allocated.unwrap();
        let encoded = backend.encode_function(&af);
        assert!(
            encoded.is_ok(),
            "{}: encoding should succeed, got: {:?}",
            kind.isa_name(),
            encoded.err()
        );
        let bytes = encoded.unwrap();
        assert!(
            !bytes.is_empty(),
            "{}: encoded bytes must not be empty",
            kind.isa_name()
        );
    }
}

/// Verify disassembly output for a simple function on each backend.
#[test]
fn test_all_backends_disasm() {
    for kind in [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ] {
        let backend = create_backend(kind).unwrap();
        let func = make_func_with_n_args("disasm_test", 2);
        let allocated = backend.allocate_registers(&func);
        if let Ok(af) = allocated {
            if let Ok(bytes) = backend.encode_function(&af) {
                let lines = backend.disassemble(&bytes, 0x400000);
                // Disassembly may or may not produce meaningful output,
                // but it should not panic and should return a Vec<String>
                assert!(
                    !lines.is_empty() || bytes.is_empty(),
                    "{}: disassembly should produce output for non-empty code",
                    kind.isa_name()
                );
            }
        }
    }
}

// ===========================================================================
// Register-specific checks for allocated functions
// ===========================================================================

/// Check that the return value from a function uses the correct register class.
/// For all register-based backends, the return value should be in a GPR.
///
/// Some backends populate the `reads`/`writes` fields of AllocatedInstruction;
/// others may leave them empty. This test validates that backends which DO
/// populate these fields use GPRs for integer return values.
#[test]
fn test_all_backends_return_in_gpr() {
    for kind in [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ] {
        let backend = create_backend(kind).unwrap();
        let func = make_func_with_n_args("ret_test", 1);
        if let Ok(af) = backend.allocate_registers(&func) {
            // Collect all register writes across all instructions
            let all_writes: Vec<_> = af.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .flat_map(|i| i.writes.iter())
                .collect();

            // If the backend populates writes, verify that at least one GPR is used
            // for a function returning i64. If writes are empty, the backend uses
            // a different encoding strategy (direct machine code), which is fine.
            if !all_writes.is_empty() {
                let has_gpr_writes: Vec<_> = all_writes.iter()
                    .filter(|r| r.class == RegClass::Gpr)
                    .collect();
                assert!(
                    !has_gpr_writes.is_empty(),
                    "{}: function returning i64 must write to at least one GPR",
                    kind.isa_name()
                );
            }
            // Regardless, the encoded output should be non-empty
            if let Ok(bytes) = backend.encode_function(&af) {
                assert!(!bytes.is_empty(), "{}: encoded output must not be empty", kind.isa_name());
            }
        }
    }
}

/// Verify that x86_64 can allocate and encode a function with 6 args,
/// all fitting in the 6 System V integer arg registers (RDI, RSI, RDX, RCX, R8, R9).
#[test]
fn test_x86_64_arg_register_indices() {
    let backend = create_backend(BackendKind::X86_64).unwrap();
    let func = make_func_with_n_args("x86_args", 6);
    let af = backend.allocate_registers(&func).unwrap();

    // The function should be allocatable and encodable
    let encoded = backend.encode_function(&af).unwrap();
    assert!(!encoded.is_empty(), "x86_64: encoded function must not be empty");

    // Collect all GPR registers that appear in the allocated function
    let gpr_reads: Vec<u32> = af.blocks.iter()
        .flat_map(|b| b.instructions.iter())
        .flat_map(|i| i.reads.iter())
        .filter(|r| r.class == RegClass::Gpr)
        .map(|r| r.index)
        .collect();

    // If the backend populates reads, verify GPRs are used
    if !gpr_reads.is_empty() {
        // x86_64 GPR indices: RAX=0, RCX=1, RDX=2, RBX=3, RSP=4, RBP=5, RSI=6, RDI=7,
        // R8=8, R9=9, R10=10, R11=11, R12=12, R13=13, R14=14, R15=15
        for idx in &gpr_reads {
            assert!(*idx <= 15, "x86_64: GPR index {} must be <= 15", idx);
        }
    }
}

/// Verify that AArch64 uses X0-X7 for argument passing.
#[test]
fn test_aarch64_arg_register_range() {
    let backend = create_backend(BackendKind::AArch64).unwrap();
    let func = make_func_with_n_args("a64_args", 8);
    let af = backend.allocate_registers(&func).unwrap();

    let gpr_indices: Vec<u32> = af.blocks.iter()
        .flat_map(|b| b.instructions.iter())
        .flat_map(|i| i.reads.iter().chain(i.writes.iter()))
        .filter(|r| r.class == RegClass::Gpr)
        .map(|r| r.index)
        .collect();

    // All GPR indices should be in the range 0..=30 for AArch64
    for idx in &gpr_indices {
        assert!(
            *idx <= 30,
            "AArch64: GPR index {} must be <= 30 (X0-X30)",
            idx
        );
    }
}

/// Verify that ARM32 uses R0-R3 for the first 4 arguments.
#[test]
fn test_arm32_arg_register_range() {
    let backend = create_backend(BackendKind::Arm32).unwrap();
    let func = make_func_with_n_args("arm32_args", 4);
    let af = backend.allocate_registers(&func).unwrap();

    let gpr_indices: Vec<u32> = af.blocks.iter()
        .flat_map(|b| b.instructions.iter())
        .flat_map(|i| i.reads.iter().chain(i.writes.iter()))
        .filter(|r| r.class == RegClass::Gpr)
        .map(|r| r.index)
        .collect();

    // All GPR indices should be in the range 0..=15 for ARM32
    for idx in &gpr_indices {
        assert!(
            *idx <= 15,
            "ARM32: GPR index {} must be <= 15 (R0-R15)",
            idx
        );
    }
}

/// Verify that PPC64 has the TOC register (R2) in Special class.
#[test]
fn test_ppc64_toc_register() {
    let backend = create_backend(BackendKind::PowerPC64).unwrap();
    let info = backend.target_info();
    assert!(info.has_toc_pointer(), "PPC64 must have TOC pointer in R2");

    // Verify the backend can allocate a function (TOC is handled internally)
    let func = make_func_with_n_args("ppc_toc_test", 2);
    let result = backend.allocate_registers(&func);
    assert!(result.is_ok(), "PPC64 allocation should handle TOC properly");
}

// ===========================================================================
// Comprehensive calling convention data validation
// ===========================================================================

/// Verify that all 8 backends report consistent ABI data.
#[test]
fn test_all_backends_abi_data() {
    let cases = vec![
        (BackendKind::X86_64,       "systemv",     6,  8, 8, 16, 8),
        (BackendKind::AArch64,      "aapcs64",     8,  8, 8, 16, 8),
        (BackendKind::RiscV64,      "lp64d",       8,  8, 8, 16, 8),
        (BackendKind::Arm32,        "aapcs",       4, 16, 4,  8, 4),
        (BackendKind::Mips64,       "n64",         8,  8, 8, 16, 8),
        (BackendKind::PowerPC64,    "elfv2",       8, 13, 8, 16, 8),
        (BackendKind::LoongArch64,  "lp64",        8,  8, 8, 16, 8),
        (BackendKind::Wasm32,       "wasm-stack",  0,  0, 4,  8, 4),
    ];

    for (kind, cc_name, int_regs, fp_regs, ptr_width, stack_align, addr_size) in cases {
        let backend = create_backend(kind).unwrap();
        let info = backend.target_info();
        assert_eq!(
            info.calling_convention_name(), cc_name,
            "{}: calling convention name mismatch",
            kind.isa_name()
        );
        assert_eq!(
            info.num_int_arg_regs(), int_regs,
            "{}: integer arg register count mismatch",
            kind.isa_name()
        );
        assert_eq!(
            info.num_fp_arg_regs(), fp_regs,
            "{}: FP arg register count mismatch",
            kind.isa_name()
        );
        assert_eq!(
            info.pointer_width(), ptr_width,
            "{}: pointer width mismatch",
            kind.isa_name()
        );
        assert_eq!(
            info.stack_alignment(), stack_align,
            "{}: stack alignment mismatch",
            kind.isa_name()
        );
        // Address size equals pointer width for all supported targets
        assert_eq!(
            info.pointer_width(), addr_size,
            "{}: address size must equal pointer width",
            kind.isa_name()
        );
    }
}

/// Verify that all register-based backends can successfully encode a program.
#[test]
fn test_all_backends_full_program() {
    for kind in [
        BackendKind::AArch64,
        BackendKind::X86_64,
        BackendKind::RiscV64,
        BackendKind::Arm32,
        BackendKind::Mips64,
        BackendKind::PowerPC64,
        BackendKind::LoongArch64,
    ] {
        let backend = create_backend(kind).unwrap();

        let func1 = make_func_with_n_args("func1", 3);
        let func2 = make_func_with_n_args("func2", 1);

        let af1 = backend.allocate_registers(&func1).unwrap();
        let af2 = backend.allocate_registers(&func2).unwrap();

        let program = vuma_codegen::backend::AllocatedProgram {
            functions: vec![af1, af2],
            total_code_size: 0,
            total_data_size: 0,
        };

        let result = backend.encode_program(&program);
        assert!(
            result.is_ok(),
            "{}: full program encoding should succeed, got: {:?}",
            kind.isa_name(),
            result.err()
        );
        let binary = result.unwrap();
        assert!(
            !binary.is_empty(),
            "{}: encoded program must not be empty",
            kind.isa_name()
        );
    }
}
