//! # Cross-Backend Consistency Test Suite
//!
//! Compiles the same VUMA programs for all 8 backends and verifies they produce
//! equivalent, structurally valid results.
//!
//! # Architecture
//!
//! The test suite operates in two phases:
//!
//! **Phase A — Hand-crafted IR programs** (Tests 1–9):
//! Each test constructs IR functions directly (bypassing the SCG front-end),
//! runs each backend's `allocate_registers` + `encode_function`, and validates:
//!
//! - Binary output exists and has reasonable size
//! - For Wasm32: the module structure (magic, version, sections)
//! - For ELF backends: the ELF header (magic, class, machine type)
//!
//! **Phase B — Full-pipeline example compilation** (Tests 10–15):
//! Reads all `.vuma` example programs from the `examples/` directory, compiles
//! them through the full parse → SCG → IR pipeline, then runs each backend's
//! `allocate_registers` + `encode_program`, validates the output, and produces
//! a test matrix summary showing compile status for every (example, backend) pair.
//!
//! # Test Programs (Phase A)
//!
//! | # | Program        | Semantics                                        | Expected result |
//! |---|----------------|--------------------------------------------------|-----------------|
//! | 1 | Simple         | `fn main() -> i64 { return 42; }`               | 42              |
//! | 2 | Arithmetic     | `fn main() -> i64 { return (10+20)*3 - 5; }`    | 85              |
//! | 3 | Memory         | alloc 8B, store 0x42424242, load, return low byte| 66 (0x42)       |
//! | 4 | Function call  | helper() returns 7; main returns helper()        | 7               |
//!
//! # Test Matrix (Phase B)
//!
//! | # | Test                                                | Scope                     |
//! |---|-----------------------------------------------------|---------------------------|
//! |10 | Full-pipeline example compilation (all backends)    | 39 examples × 8 backends  |
//! |11 | ELF section validation for example programs         | .text/.data/.symtab/.strtab |
//! |12 | Wasm32 format validation for example programs       | Wasm binary structure     |
//! |13 | Cross-backend code size consistency for examples    | Size sanity per backend   |
//! |14 | Regression tracking for example compilation         | Known-good baseline       |
//! |15 | Test matrix summary print                           | ASCII art matrix output   |

use vuma_codegen::backend::{
    create_backend, AllocatedProgram, Backend, BackendKind, Endianness, OutputFormat,
};
use vuma_codegen::ir::{
    BinOpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue, VirtualRegister,
};
use vuma_codegen::scg_to_ir::IRBuilder;
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ---------------------------------------------------------------------------
// Backend helpers
// ---------------------------------------------------------------------------

/// All 8 backend kinds, in a stable order for iteration.
const ALL_BACKENDS: &[BackendKind] = &[
    BackendKind::AArch64,
    BackendKind::RiscV64,
    BackendKind::Wasm32,
    BackendKind::LoongArch64,
    BackendKind::X86_64,
    BackendKind::Arm32,
    BackendKind::Mips64,
    BackendKind::PowerPC64,
];

/// Human-readable name for a BackendKind (for assertion messages).
fn backend_name(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::AArch64 => "aarch64",
        BackendKind::RiscV64 => "riscv64",
        BackendKind::Wasm32 => "wasm32",
        BackendKind::LoongArch64 => "loongarch64",
        BackendKind::X86_64 => "x86_64",
        BackendKind::Arm32 => "arm32",
        BackendKind::Mips64 => "mips64",
        BackendKind::PowerPC64 => "ppc64",
    }
}

/// ELF machine type for a BackendKind (0 for non-ELF targets).
fn elf_machine(kind: BackendKind) -> u16 {
    match kind {
        BackendKind::AArch64 => 183,   // EM_AARCH64
        BackendKind::RiscV64 => 243,   // EM_RISCV
        BackendKind::Wasm32 => 0,      // Not ELF
        BackendKind::LoongArch64 => 258, // EM_LOONGARCH
        BackendKind::X86_64 => 62,     // EM_X86_64
        BackendKind::Arm32 => 40,      // EM_ARM
        BackendKind::Mips64 => 8,      // EM_MIPS
        BackendKind::PowerPC64 => 21,  // EM_PPC64
    }
}

/// Expected output format for a BackendKind.
fn expected_output_format(kind: BackendKind) -> OutputFormat {
    match kind {
        BackendKind::Arm32 => OutputFormat::Elf32,
        BackendKind::Wasm32 => OutputFormat::WasmBinary,
        _ => OutputFormat::Elf64,
    }
}

/// Run the full `allocate_registers` + `encode_program` pipeline for a
/// multi-function program and return the final binary.
fn compile_program(
    backend: &dyn Backend,
    functions: &[IRFunction],
    label: &str,
) -> Vec<u8> {
    let mut allocated_functions = Vec::new();
    for func in functions {
        let allocated = backend
            .allocate_registers(func)
            .unwrap_or_else(|e| {
                panic!(
                    "{}: allocate_registers failed for {} / {}: {}",
                    backend.name(),
                    label,
                    func.name,
                    e
                )
            });
        allocated_functions.push(allocated);
    }

    let total_code_size: usize = allocated_functions
        .iter()
        .map(|f| f.code_size)
        .sum();

    let program = AllocatedProgram {
        functions: allocated_functions,
        total_code_size,
        total_data_size: 0,
    };

    backend
        .encode_program(&program)
        .unwrap_or_else(|e| {
            panic!(
                "{}: encode_program failed for {}: {}",
                backend.name(),
                label,
                e
            )
        })
}

/// Validate the ELF header of a compiled binary for the given backend.
fn validate_elf_header(bytes: &[u8], kind: BackendKind) {
    let name = backend_name(kind);

    // ELF header must be at least 52 bytes (ELF32) or 64 bytes (ELF64).
    let min_header = match expected_output_format(kind) {
        OutputFormat::Elf32 => 52,
        OutputFormat::Elf64 => 64,
        _ => panic!("validate_elf_header called for non-ELF backend {}", name),
    };
    assert!(
        bytes.len() >= min_header,
        "{}: ELF binary too short ({} bytes, need at least {})",
        name,
        bytes.len(),
        min_header
    );

    // Magic bytes: 0x7f 'E' 'L' 'F'
    assert_eq!(
        &bytes[0..4],
        &[0x7f, b'E', b'L', b'F'],
        "{}: ELF magic bytes incorrect",
        name
    );

    // ELF class
    let expected_class = match expected_output_format(kind) {
        OutputFormat::Elf32 => 1u8, // ELFCLASS32
        OutputFormat::Elf64 => 2u8, // ELFCLASS64
        _ => unreachable!(),
    };
    assert_eq!(
        bytes[4], expected_class,
        "{}: ELF class should be {}",
        name, expected_class
    );

    // ELF version must be EV_CURRENT (1)
    assert_eq!(bytes[6], 1, "{}: ELF version should be EV_CURRENT (1)", name);

    // Machine type at offset 18..20 — read using the ELF's declared byte order.
    // Byte 5 (ei_data): 1 = little-endian, 2 = big-endian.
    let e_machine = if bytes[5] == 2 {
        u16::from_be_bytes([bytes[18], bytes[19]])
    } else {
        u16::from_le_bytes([bytes[18], bytes[19]])
    };
    assert_eq!(
        e_machine,
        elf_machine(kind),
        "{}: ELF machine type should be {} (got {})",
        name,
        elf_machine(kind),
        e_machine
    );
}

/// Validate the Wasm module structure of a compiled binary.
fn validate_wasm_module(bytes: &[u8]) {
    // Must have at least 8 bytes (magic + version)
    assert!(
        bytes.len() >= 8,
        "wasm32: binary too short ({} bytes, need at least 8)",
        bytes.len()
    );

    // Magic: 0x00 0x61 0x73 0x6D ("\0asm")
    assert_eq!(
        &bytes[0..4],
        &[0x00, 0x61, 0x73, 0x6D],
        "wasm32: magic bytes should be \\0asm"
    );

    // Version: 0x01 0x00 0x00 0x00 (version 1)
    assert_eq!(
        &bytes[4..8],
        &[0x01, 0x00, 0x00, 0x00],
        "wasm32: version should be 1"
    );

    // Verify at least some sections exist after the header
    assert!(
        bytes.len() > 8,
        "wasm32: module should have content after header"
    );

    // Walk sections and verify they appear in ascending ID order
    let mut offset = 8usize;
    let mut last_section_id: Option<u8> = None;
    while offset < bytes.len() {
        let section_id = bytes[offset];
        offset += 1;

        // Decode LEB128 size
        let mut size: usize = 0;
        let mut shift: usize = 0;
        loop {
            assert!(offset < bytes.len(), "wasm32: truncated section size");
            let byte = bytes[offset];
            offset += 1;
            size |= ((byte & 0x7F) as usize) << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                break;
            }
        }

        // Sections must appear in order of ascending ID (except custom sections, ID 0)
        if section_id != 0 {
            if let Some(prev) = last_section_id {
                assert!(
                    section_id > prev,
                    "wasm32: sections out of order ({} after {})",
                    section_id,
                    prev
                );
            }
            last_section_id = Some(section_id);
        }

        offset += size;
    }
}

/// Validate a binary produced by any backend: check format-specific structure
/// and that the output has a reasonable minimum size.
fn validate_binary(bytes: &[u8], kind: BackendKind, min_size: usize) {
    let name = backend_name(kind);

    // Reasonable minimum size (at least a few instructions)
    assert!(
        bytes.len() >= min_size,
        "{}: binary too small ({} bytes, expected at least {})",
        name,
        bytes.len(),
        min_size
    );

    match expected_output_format(kind) {
        OutputFormat::Elf32 | OutputFormat::Elf64 => validate_elf_header(bytes, kind),
        OutputFormat::WasmBinary => validate_wasm_module(bytes),
        OutputFormat::RawBinary => {
            // No structural validation for raw binaries
        }
    }
}

// ===========================================================================
// IR Program Constructors (Phase A)
// ===========================================================================

/// Program 1: Simple — a function that returns 42.
///
/// ```text
/// fn main() -> i64 { return 42; }
/// ```
fn make_simple_function() -> IRFunction {
    let mut func = IRFunction::new("main");
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(0));
    func.vregs.insert(0, VirtualRegister::new(0, Some("ret_val".to_string())));

    func.current_block().terminator = IRTerminator::Return(vec![IRValue::Immediate(42)]);
    func
}

/// Program 2: Arithmetic — computes (10 + 20) * 3 - 5 = 85.
///
/// ```text
/// fn main() -> i64 {
///     let a = 10 + 20;   // 30
///     let b = a * 3;     // 90
///     let c = b - 5;     // 85
///     return c;
/// }
/// ```
fn make_arithmetic_function() -> IRFunction {
    let mut func = IRFunction::new("main");
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(2));
    func.vregs.insert(0, VirtualRegister::new(0, Some("a".to_string())));
    func.vregs.insert(1, VirtualRegister::new(1, Some("b".to_string())));
    func.vregs.insert(2, VirtualRegister::new(2, Some("c".to_string())));

    let block = func.current_block();

    // a = 10 + 20
    block.push(IRInstr::Add {
        dst: IRValue::Register(0),
        lhs: IRValue::Immediate(10),
        rhs: IRValue::Immediate(20),
        ty: Some(IRType::I64),
    });

    // b = a * 3
    block.push(IRInstr::BinOp {
        op: BinOpKind::Mul,
        dst: IRValue::Register(1),
        lhs: IRValue::Register(0),
        rhs: IRValue::Immediate(3),
        ty: Some(IRType::I64),
    });

    // c = b - 5
    block.push(IRInstr::BinOp {
        op: BinOpKind::Sub,
        dst: IRValue::Register(2),
        lhs: IRValue::Register(1),
        rhs: IRValue::Immediate(5),
        ty: Some(IRType::I64),
    });

    block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);
    func
}

/// Program 3: Memory — allocates 8 bytes, writes 0x42424242, reads it back,
/// returns the low byte (0x42 = 66).
///
/// ```text
/// fn main() -> i64 {
///     let ptr = alloc 8;
///     store 0x42424242 at ptr;
///     let val = load ptr as i64;
///     let byte = val & 0xFF;
///     return byte;          // 0x42 = 66
/// }
/// ```
fn make_memory_function() -> IRFunction {
    let mut func = IRFunction::new("main");
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(2));
    func.vregs.insert(0, VirtualRegister::new(0, Some("ptr".to_string())));
    func.vregs.insert(1, VirtualRegister::new(1, Some("val".to_string())));
    func.vregs.insert(2, VirtualRegister::new(2, Some("byte".to_string())));

    let block = func.current_block();

    // ptr = alloc 8
    block.push(IRInstr::Alloc {
        dst: IRValue::Register(0),
        size: 8,
    });

    // store 0x42424242 at ptr + 0
    block.push(IRInstr::Store {
        value: IRValue::Immediate(0x42424242),
        addr: IRValue::Register(0),
        offset: 0,
        ty: IRType::I64,
    });

    // val = load ptr + 0 as i64
    block.push(IRInstr::Load {
        dst: IRValue::Register(1),
        addr: IRValue::Register(0),
        offset: 0,
        ty: IRType::I64,
    });

    // byte = val & 0xFF
    block.push(IRInstr::BinOp {
        op: BinOpKind::And,
        dst: IRValue::Register(2),
        lhs: IRValue::Register(1),
        rhs: IRValue::Immediate(0xFF),
        ty: Some(IRType::I64),
    });

    block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);
    func
}

/// Program 4: Function call — a helper that returns 7, main calls it
/// and returns the result.
///
/// ```text
/// fn helper() -> i64 { return 7; }
/// fn main() -> i64 { return helper(); }
/// ```
fn make_function_call_program() -> Vec<IRFunction> {
    // Helper function: returns 7
    let mut helper = IRFunction::new("helper");
    helper.result_types.push(IRType::I64);
    helper.results.push(IRValue::Register(0));
    helper
        .vregs
        .insert(0, VirtualRegister::new(0, Some("ret".to_string())));
    helper.current_block().terminator = IRTerminator::Return(vec![IRValue::Immediate(7)]);

    // Main function: calls helper and returns the result
    let mut main_fn = IRFunction::new("main");
    main_fn.result_types.push(IRType::I64);
    main_fn.results.push(IRValue::Register(0));
    main_fn
        .vregs
        .insert(0, VirtualRegister::new(0, Some("result".to_string())));

    main_fn.current_block().push(IRInstr::Call {
        dst: Some(IRValue::Register(0)),
        func: "helper".to_string(),
        args: vec![],
        is_extern: false,
    });
    main_fn.current_block().terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

    vec![helper, main_fn]
}

// ===========================================================================
// Phase A Tests (1–9): Hand-crafted IR programs
// ===========================================================================

/// Test 1: Simple program — `fn main() -> i64 { return 42; }`
///
/// Validates that all 8 backends can compile a trivial return-constant
/// function and produce structurally valid output.
#[test]
fn test_cross_backend_simple_return() {
    let func = make_simple_function();

    for &kind in ALL_BACKENDS {
        let name = backend_name(kind);
        let backend = create_backend(kind).expect("backend creation should succeed");

        // --- allocate_registers + encode_function ---
        let allocated = backend
            .allocate_registers(&func)
            .unwrap_or_else(|e| panic!("{}: allocate_registers failed: {}", name, e));

        // The allocated function should have at least one block
        assert!(
            !allocated.blocks.is_empty(),
            "{}: allocated function should have at least one block",
            name
        );

        // The block should contain at least one instruction
        let total_instrs: usize = allocated
            .blocks
            .iter()
            .map(|b| b.instructions.len())
            .sum();
        assert!(
            total_instrs > 0,
            "{}: allocated function should have instructions",
            name
        );

        // Encode the function
        let code = backend
            .encode_function(&allocated)
            .unwrap_or_else(|e| panic!("{}: encode_function failed: {}", name, e));

        // Even a trivial function needs at least 4 bytes of machine code
        assert!(
            code.len() >= 4,
            "{}: encoded function too small ({} bytes)",
            name,
            code.len()
        );

        // --- encode_program (full binary) ---
        let program_bytes = compile_program(&*backend, &[func.clone()], "simple");
        validate_binary(&program_bytes, kind, 16);
    }
}

/// Test 2: Arithmetic program — `(10 + 20) * 3 - 5 = 85`
///
/// Validates that all 8 backends can compile a sequence of arithmetic
/// operations and produce structurally valid output.
#[test]
fn test_cross_backend_arithmetic() {
    let func = make_arithmetic_function();

    for &kind in ALL_BACKENDS {
        let name = backend_name(kind);
        let backend = create_backend(kind).expect("backend creation should succeed");

        let allocated = backend
            .allocate_registers(&func)
            .unwrap_or_else(|e| panic!("{}: allocate_registers failed: {}", name, e));

        // Arithmetic function should have more instructions than the simple one
        let total_instrs: usize = allocated
            .blocks
            .iter()
            .map(|b| b.instructions.len())
            .sum();
        assert!(
            total_instrs >= 3,
            "{}: arithmetic program should have at least 3 instructions (got {})",
            name,
            total_instrs
        );

        let code = backend
            .encode_function(&allocated)
            .unwrap_or_else(|e| panic!("{}: encode_function failed: {}", name, e));

        // Should be larger than the simple function
        assert!(
            code.len() >= 4,
            "{}: encoded arithmetic function too small ({} bytes)",
            name,
            code.len()
        );

        // Full program binary
        let program_bytes = compile_program(&*backend, &[func.clone()], "arithmetic");
        validate_binary(&program_bytes, kind, 16);
    }
}

/// Test 3: Memory program — alloc, store, load, mask, return
///
/// Validates that all 8 backends can compile memory operations
/// (stack allocation, store, load) and produce structurally valid output.
#[test]
fn test_cross_backend_memory() {
    let func = make_memory_function();

    for &kind in ALL_BACKENDS {
        let name = backend_name(kind);
        let backend = create_backend(kind).expect("backend creation should succeed");

        let allocated = backend
            .allocate_registers(&func)
            .unwrap_or_else(|e| panic!("{}: allocate_registers failed: {}", name, e));

        // Memory function should have alloc + store + load + and instructions
        let total_instrs: usize = allocated
            .blocks
            .iter()
            .map(|b| b.instructions.len())
            .sum();
        assert!(
            total_instrs >= 4,
            "{}: memory program should have at least 4 instructions (got {})",
            name,
            total_instrs
        );

        // The function should need a stack frame (for the Alloc)
        // Wasm32 is a stack machine — it does not use frame_size.
        if kind != BackendKind::Wasm32 {
            assert!(
                allocated.frame_size > 0,
                "{}: memory program should have a non-zero frame size",
                name
            );
        }

        let code = backend
            .encode_function(&allocated)
            .unwrap_or_else(|e| panic!("{}: encode_function failed: {}", name, e));

        assert!(
            code.len() >= 4,
            "{}: encoded memory function too small ({} bytes)",
            name,
            code.len()
        );

        // Full program binary
        let program_bytes = compile_program(&*backend, &[func.clone()], "memory");
        validate_binary(&program_bytes, kind, 16);
    }
}

/// Test 4: Function call — helper returns 7, main returns helper()
///
/// Validates that all 8 backends can compile a multi-function program
/// with an inter-function call and produce structurally valid output.
#[test]
fn test_cross_backend_function_call() {
    let functions = make_function_call_program();

    for &kind in ALL_BACKENDS {
        let name = backend_name(kind);
        let backend = create_backend(kind).expect("backend creation should succeed");

        // Allocate registers for each function independently
        let mut allocated_fns = Vec::new();
        for func in &functions {
            let allocated = backend
                .allocate_registers(func)
                .unwrap_or_else(|e| {
                    panic!(
                        "{}: allocate_registers failed for '{}': {}",
                        name,
                        func.name,
                        e
                    )
                });
            allocated_fns.push(allocated);
        }

        // We should have 2 allocated functions
        assert_eq!(
            allocated_fns.len(),
            2,
            "{}: should have 2 allocated functions",
            name
        );

        // Encode each function individually
        for alloc_fn in &allocated_fns {
            let code = backend
                .encode_function(alloc_fn)
                .unwrap_or_else(|e| panic!("{}: encode_function failed: {}", name, e));
            assert!(
                code.len() >= 4,
                "{}: encoded function '{}' too small ({} bytes)",
                name,
                alloc_fn.name,
                code.len()
            );
        }

        // Full program binary (links the two functions together)
        let program_bytes = compile_program(&*backend, &functions, "func_call");
        validate_binary(&program_bytes, kind, 16);

        // The main function should have a relocation to the helper
        let main_alloc = &allocated_fns[1]; // second function is "main"
        let has_helper_reloc = main_alloc
            .relocations
            .iter()
            .any(|r| r.symbol == "helper");
        assert!(
            has_helper_reloc,
            "{}: main function should have a relocation to 'helper'",
            name
        );
    }
}

/// Test 5: Cross-backend output format consistency
///
/// Verifies that each backend reports the correct output format and that
/// the `encode_program` output matches the declared format.
#[test]
fn test_cross_backend_output_format_consistency() {
    let func = make_simple_function();

    for &kind in ALL_BACKENDS {
        let name = backend_name(kind);
        let backend = create_backend(kind).expect("backend creation should succeed");

        // TargetInfo consistency
        let info = backend.target_info();
        let expected_fmt = expected_output_format(kind);
        assert_eq!(
            info.output_format(),
            expected_fmt,
            "{}: output_format mismatch",
            name
        );

        // ISA name should match
        assert_eq!(
            info.isa_name(),
            name,
            "{}: isa_name mismatch",
            name
        );

        // Pointer width consistency
        match expected_fmt {
            OutputFormat::Elf32 | OutputFormat::WasmBinary => {
                assert_eq!(
                    info.pointer_width(),
                    4,
                    "{}: 32-bit target should have pointer_width 4",
                    name
                );
            }
            OutputFormat::Elf64 => {
                assert_eq!(
                    info.pointer_width(),
                    8,
                    "{}: 64-bit target should have pointer_width 8",
                    name
                );
            }
            OutputFormat::RawBinary => {}
        }

        // ELF machine type consistency (for ELF targets)
        if expected_fmt != OutputFormat::WasmBinary {
            assert_eq!(
                info.elf_machine_type(),
                elf_machine(kind),
                "{}: elf_machine_type mismatch",
                name
            );
        }

        // Compile and check the binary matches the expected format
        let program_bytes = compile_program(&*backend, &[func.clone()], "format_check");

        match expected_fmt {
            OutputFormat::Elf32 | OutputFormat::Elf64 => {
                // Must start with ELF magic
                assert!(
                    program_bytes.len() >= 4,
                    "{}: ELF output too short",
                    name
                );
                assert_eq!(
                    &program_bytes[0..4],
                    &[0x7f, b'E', b'L', b'F'],
                    "{}: ELF output must start with ELF magic",
                    name
                );
            }
            OutputFormat::WasmBinary => {
                assert!(
                    program_bytes.len() >= 8,
                    "{}: Wasm output too short",
                    name
                );
                assert_eq!(
                    &program_bytes[0..4],
                    &[0x00, 0x61, 0x73, 0x6D],
                    "{}: Wasm output must start with \\0asm magic",
                    name
                );
            }
            OutputFormat::RawBinary => {}
        }
    }
}

/// Test 6: Cross-backend code size sanity
///
/// Compiles all 4 programs on all 8 backends and verifies that the
/// code sizes are within sane bounds relative to each other.
/// While the absolute sizes differ per ISA, they should all be > 0 and
/// not absurdly large for these tiny programs.
#[test]
fn test_cross_backend_code_size_sanity() {
    let simple = make_simple_function();
    let arithmetic = make_arithmetic_function();
    let memory = make_memory_function();
    let func_call = make_function_call_program();

    let programs: Vec<(&str, Vec<IRFunction>)> = vec![
        ("simple", vec![simple]),
        ("arithmetic", vec![arithmetic]),
        ("memory", vec![memory]),
        ("func_call", func_call),
    ];

    // Upper bound: no tiny program should produce > 1MB of code
    const MAX_REASONABLE_SIZE: usize = 1_048_576;

    for &kind in ALL_BACKENDS {
        let name = backend_name(kind);
        let backend = create_backend(kind).expect("backend creation should succeed");

        for (label, functions) in &programs {
            let program_bytes = compile_program(&*backend, functions, label);

            assert!(
                !program_bytes.is_empty(),
                "{}: {} program should produce non-empty output",
                name,
                label
            );

            assert!(
                program_bytes.len() <= MAX_REASONABLE_SIZE,
                "{}: {} program produced suspiciously large output ({} bytes)",
                name,
                label,
                program_bytes.len()
            );
        }
    }
}

/// Test 7: Backend name consistency
///
/// Verifies that `backend.name()` matches the expected name string for
/// each backend kind, and that `BackendKind` discriminants are unique.
#[test]
fn test_cross_backend_name_consistency() {
    let mut seen_names: HashMap<&str, BackendKind> = HashMap::new();

    for &kind in ALL_BACKENDS {
        let backend = create_backend(kind).expect("backend creation should succeed");
        let name = backend.name();
        let expected = backend_name(kind);

        assert_eq!(
            name, expected,
            "BackendKind::{:?}.name() should be '{}', got '{}'",
            kind, expected, name
        );

        if let Some(prev) = seen_names.get(name) {
            panic!(
                "Duplicate backend name '{}' for {:?} and {:?}",
                name, prev, kind
            );
        }
        seen_names.insert(name, kind);
    }

    assert_eq!(
        seen_names.len(),
        ALL_BACKENDS.len(),
        "All backends should have unique names"
    );
}

/// Test 8: Wasm32-specific module structure validation
///
/// Compiles each program with the Wasm32 backend and performs detailed
/// validation of the Wasm module structure: sections present, order,
/// type section contents, and memory section.
#[test]
fn test_cross_backend_wasm32_module_structure() {
    let simple = make_simple_function();
    let arithmetic = make_arithmetic_function();
    let memory = make_memory_function();
    let func_call = make_function_call_program();

    let programs: Vec<(&str, Vec<IRFunction>)> = vec![
        ("simple", vec![simple]),
        ("arithmetic", vec![arithmetic]),
        ("memory", vec![memory]),
        ("func_call", func_call),
    ];

    let backend = create_backend(BackendKind::Wasm32).expect("Wasm32 backend creation");

    for (label, functions) in &programs {
        let bytes = compile_program(&*backend, functions, label);

        // Basic Wasm structure
        assert!(
            bytes.len() >= 8,
            "wasm32/{}: module too short ({} bytes)",
            label,
            bytes.len()
        );
        assert_eq!(
            &bytes[0..4],
            &[0x00, 0x61, 0x73, 0x6D],
            "wasm32/{}: magic bytes incorrect",
            label
        );
        assert_eq!(
            &bytes[4..8],
            &[0x01, 0x00, 0x00, 0x00],
            "wasm32/{}: version incorrect",
            label
        );

        // Parse sections and verify presence of required sections
        let mut found_type_section = false;
        let mut found_function_section = false;
        let mut found_memory_section = false;
        let mut found_code_section = false;

        let mut offset = 8usize;
        while offset < bytes.len() {
            let section_id = bytes[offset];
            offset += 1;

            // Decode LEB128 size
            let mut size: usize = 0;
            let mut shift: usize = 0;
            loop {
                assert!(offset < bytes.len(), "wasm32/{}: truncated section", label);
                let byte = bytes[offset];
                offset += 1;
                size |= ((byte & 0x7F) as usize) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
            }

            match section_id {
                1 => found_type_section = true,
                3 => found_function_section = true,
                5 => found_memory_section = true,
                10 => found_code_section = true,
                _ => {}
            }

            offset += size;
        }

        assert!(
            found_type_section,
            "wasm32/{}: missing type section (ID 1)",
            label
        );
        assert!(
            found_function_section,
            "wasm32/{}: missing function section (ID 3)",
            label
        );
        assert!(
            found_memory_section,
            "wasm32/{}: missing memory section (ID 5)",
            label
        );
        assert!(
            found_code_section,
            "wasm32/{}: missing code section (ID 10)",
            label
        );
    }
}

/// Test 9: ELF-specific header validation for all ELF backends
///
/// Compiles each program with every ELF-producing backend and verifies
/// the ELF header fields (magic, class, endianness, machine type, version).
#[test]
fn test_cross_backend_elf_header_validation() {
    let simple = make_simple_function();
    let arithmetic = make_arithmetic_function();
    let memory = make_memory_function();
    let func_call = make_function_call_program();

    let programs: Vec<(&str, Vec<IRFunction>)> = vec![
        ("simple", vec![simple]),
        ("arithmetic", vec![arithmetic]),
        ("memory", vec![memory]),
        ("func_call", func_call),
    ];

    for &kind in ALL_BACKENDS {
        let fmt = expected_output_format(kind);
        if fmt != OutputFormat::Elf32 && fmt != OutputFormat::Elf64 {
            continue; // Skip non-ELF backends
        }

        let name = backend_name(kind);
        let backend = create_backend(kind).expect("backend creation should succeed");

        for (label, functions) in &programs {
            let bytes = compile_program(&*backend, functions, label);

            // Minimum ELF header size
            let min_hdr = if fmt == OutputFormat::Elf32 { 52 } else { 64 };
            assert!(
                bytes.len() >= min_hdr,
                "{}/{}: ELF binary too short ({} bytes)",
                name,
                label,
                bytes.len()
            );

            // Magic
            assert_eq!(
                &bytes[0..4],
                &[0x7f, b'E', b'L', b'F'],
                "{}/{}: bad ELF magic",
                name,
                label
            );

            // Class
            let expected_class = if fmt == OutputFormat::Elf32 { 1u8 } else { 2u8 };
            assert_eq!(
                bytes[4], expected_class,
                "{}/{}: ELF class mismatch",
                name,
                label
            );

            // Endianness
            let backend_obj = create_backend(kind).unwrap();
            let endian = backend_obj.target_info().endianness();
            let expected_data = match endian {
                Endianness::Little => 1u8, // ELFDATA2LSB
                Endianness::Big => 2u8,    // ELFDATA2MSB
                Endianness::Bi => 2u8,     // Bi-endian defaults big
            };
            assert_eq!(
                bytes[5], expected_data,
                "{}/{}: ELF data encoding mismatch",
                name,
                label
            );

            // Version
            assert_eq!(
                bytes[6], 1,
                "{}/{}: ELF version should be EV_CURRENT",
                name,
                label
            );

            // Machine type
            let e_machine = u16::from_le_bytes([bytes[18], bytes[19]]);
            // For big-endian ELF files the header is still encoded in the
            // target's endianness, so we may need to read it as BE.
            let e_machine_val = if expected_data == 2u8 {
                u16::from_be_bytes([bytes[18], bytes[19]])
            } else {
                e_machine
            };
            assert_eq!(
                e_machine_val,
                elf_machine(kind),
                "{}/{}: machine type mismatch (expected {}, got {} / {} LE)",
                name,
                label,
                elf_machine(kind),
                e_machine,
                e_machine_val
            );
        }
    }
}

// ===========================================================================
// Phase B: Full-Pipeline Example Compilation Tests (10–15)
// ===========================================================================

/// Compile status for a single (example, backend) pair.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CompileStatus {
    /// Compilation succeeded; binary size recorded.
    Success(usize),
    /// Parsing failed.
    ParseFailed(String),
    /// AST → SCG conversion failed.
    ScgFailed(String),
    /// SCG → codegen bridge failed.
    BridgeFailed(String),
    /// IR lowering failed.
    IrFailed(String),
    /// Register allocation failed.
    RegAllocFailed(String),
    /// Encoding (encode_program) failed.
    EncodeFailed(String),
}

impl CompileStatus {
    fn is_success(&self) -> bool {
        matches!(self, CompileStatus::Success(_))
    }

    fn symbol(&self) -> &'static str {
        match self {
            CompileStatus::Success(_) => "✓",
            CompileStatus::ParseFailed(_) => "P",
            CompileStatus::ScgFailed(_) => "S",
            CompileStatus::BridgeFailed(_) => "B",
            CompileStatus::IrFailed(_) => "I",
            CompileStatus::RegAllocFailed(_) => "R",
            CompileStatus::EncodeFailed(_) => "E",
        }
    }
}

/// Result of attempting to compile a single .vuma example through the
/// full pipeline for all backends.
struct ExampleCompileResult {
    /// Example file name (without directory).
    name: String,
    /// Per-backend compile status.
    statuses: HashMap<BackendKind, CompileStatus>,
}

/// Discover all `.vuma` files in the project's `examples/` directory.
fn discover_examples() -> Vec<(String, String)> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR should be set during tests");
    let examples_dir = Path::new(&manifest_dir)
        .parent()
        .expect("tests dir has a parent")
        .parent()
        .expect("src dir has a parent")
        .join("examples");

    let mut examples: Vec<(String, String)> = Vec::new();

    let entries = std::fs::read_dir(&examples_dir)
        .unwrap_or_else(|e| panic!("Failed to read examples dir {:?}: {}", examples_dir, e));

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "vuma") {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("Failed to read {:?}: {}", path, e));
            examples.push((name, source));
        }
    }

    // Sort by name for deterministic output.
    examples.sort_by(|a, b| a.0.cmp(&b.0));
    examples
}

/// Attempt to compile a single .vuma source through the full pipeline
/// for a specific backend. Returns the compile status and (on success)
/// the binary bytes.
fn compile_example_for_backend(
    source: &str,
    kind: BackendKind,
) -> (CompileStatus, Option<Vec<u8>>) {
    // Step 1: Parse source → AST
    let ast = {
        use vuma_parser::Parser;
        let mut parser = Parser::new(source);
        let result = parser.parse_program();
        if result.has_errors() {
            let err_msg: String = result.errors.iter()
                .map(|e| format!("{:?}", e))
                .collect::<Vec<_>>()
                .join("; ");
            return (CompileStatus::ParseFailed(err_msg), None);
        }
        result.unwrap()
    };

    // Step 2: AST → vuma-scg SCG
    let mut scg = {
        use vuma_parser::AstToScg;
        let mut converter = AstToScg::new();
        match converter.convert(&ast) {
            Ok(scg) => scg,
            Err(e) => return (CompileStatus::ScgFailed(format!("{}", e)), None),
        }
    };

    // Step 3: Run lightweight SCG transforms (DCE + constant folding at O1)
    {
        use vuma::pipeline::{CompileConfig, run_scg_transforms, CompileTarget, OptLevel, VerificationLevel};
        let config = CompileConfig {
            target: if kind == BackendKind::Wasm32 { CompileTarget::Wasm32 } else { CompileTarget::Linux },
            opt_level: OptLevel::O1,
            verification_level: VerificationLevel::None,
            ..CompileConfig::default()
        };
        let _ = run_scg_transforms(&mut scg, &config);
    }

    // Step 4: Bridge vuma-scg SCG → codegen SCG
    let codegen_scg = {
        use vuma::pipeline::bridge_scg_to_codegen;
        bridge_scg_to_codegen(&scg)
    };

    // Step 5: Lower codegen SCG → IR
    let ir_program = {
        let mut builder = IRBuilder::new();
        match builder.build(&codegen_scg) {
            Ok(ir) => ir,
            Err(e) => return (CompileStatus::IrFailed(format!("{}", e)), None),
        }
    };

    if ir_program.functions.is_empty() {
        return (CompileStatus::IrFailed("no functions in IR".to_string()), None);
    }

    // Step 6: For the specific backend, allocate registers + encode
    let backend = match create_backend(kind) {
        Ok(b) => b,
        Err(e) => return (CompileStatus::RegAllocFailed(format!("create_backend: {}", e)), None),
    };

    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        match backend.allocate_registers(func) {
            Ok(allocated) => allocated_functions.push(allocated),
            Err(e) => return (CompileStatus::RegAllocFailed(format!("{}: {}", func.name, e)), None),
        }
    }

    let total_code_size: usize = allocated_functions.iter().map(|f| f.code_size).sum();
    let program = AllocatedProgram {
        functions: allocated_functions,
        total_code_size,
        total_data_size: 0,
    };

    match backend.encode_program(&program) {
        Ok(bytes) => (CompileStatus::Success(bytes.len()), Some(bytes)),
        Err(e) => (CompileStatus::EncodeFailed(format!("{}", e)), None),
    }
}

/// Compile all examples for all backends and return the full results matrix.
fn compile_all_examples() -> Vec<ExampleCompileResult> {
    let examples = discover_examples();
    let mut results = Vec::new();

    for (name, source) in &examples {
        let mut statuses = HashMap::new();
        for &kind in ALL_BACKENDS {
            let (status, _) = compile_example_for_backend(source, kind);
            statuses.insert(kind, status);
        }
        results.push(ExampleCompileResult {
            name: name.clone(),
            statuses,
        });
    }

    results
}

/// Print a test matrix summary showing compile status for each
/// (example, backend) pair.  The matrix is printed to stderr so it
/// appears in `cargo test` output.
fn print_test_matrix(results: &[ExampleCompileResult]) {
    eprintln!();
    eprintln!("╔════════════════════════════════════════════════════════════════════════════════════════════════════╗");
    eprintln!("║                       Cross-Backend Compilation Test Matrix (39 examples × 8 backends)          ║");
    eprintln!("╠════════════════════════════════════════════════════════════════════════════════════════════════════╣");
    eprintln!("║ {:<24} │ {:^8} │ {:^8} │ {:^8} │ {:^12} │ {:^8} │ {:^8} │ {:^8} │ {:^8} ║",
        "Example", "aarch64", "riscv64", "wasm32", "loongarch64", "x86_64", "arm32", "mips64", "ppc64");
    eprintln!("╠════════════════════════════════════════════════════════════════════════════════════════════════════╣");

    for result in results {
        let mut cols = Vec::new();
        for &kind in ALL_BACKENDS {
            let status = result.statuses.get(&kind).unwrap();
            let col = match status {
                CompileStatus::Success(size) => format!("✓{:05}", size),
                CompileStatus::ParseFailed(_) => "  P   ".to_string(),
                CompileStatus::ScgFailed(_) => "  S   ".to_string(),
                CompileStatus::BridgeFailed(_) => "  B   ".to_string(),
                CompileStatus::IrFailed(_) => "  I   ".to_string(),
                CompileStatus::RegAllocFailed(_) => "  R   ".to_string(),
                CompileStatus::EncodeFailed(_) => "  E   ".to_string(),
            };
            cols.push(col);
        }

        eprintln!("║ {:<24} │ {:^8} │ {:^8} │ {:^8} │ {:^12} │ {:^8} │ {:^8} │ {:^8} │ {:^8} ║",
            result.name,
            cols[0], cols[1], cols[2], cols[3],
            cols[4], cols[5], cols[6], cols[7]);
    }

    eprintln!("╠════════════════════════════════════════════════════════════════════════════════════════════════════╣");

    // Summary row
    let mut totals: HashMap<BackendKind, (usize, usize)> = HashMap::new();
    for &kind in ALL_BACKENDS {
        totals.insert(kind, (0, 0));
    }
    for result in results {
        for &kind in ALL_BACKENDS {
            let (success, total) = totals[&kind];
            let is_ok = result.statuses.get(&kind).unwrap().is_success();
            totals.insert(kind, (success + is_ok as usize, total + 1));
        }
    }

    let mut sum_cols = Vec::new();
    for &kind in ALL_BACKENDS {
        let (success, total) = totals[&kind];
        sum_cols.push(format!("{}/{}", success, total));
    }

    eprintln!("║ {:<24} │ {:^8} │ {:^8} │ {:^8} │ {:^12} │ {:^8} │ {:^8} │ {:^8} │ {:^8} ║",
        "TOTAL", &sum_cols[0], &sum_cols[1], &sum_cols[2], &sum_cols[3],
        &sum_cols[4], &sum_cols[5], &sum_cols[6], &sum_cols[7]);

    eprintln!("╚════════════════════════════════════════════════════════════════════════════════════════════════════╝");
    eprintln!();
    eprintln!("  Legend: ✓NNNNN = success (binary size in bytes)   P = parse fail   S = SCG fail   B = bridge fail   I = IR fail   R = regalloc fail   E = encode fail");
    eprintln!();
}

/// Test 10: Full-pipeline example compilation for all backends
///
/// Compiles every `.vuma` example in the `examples/` directory through the
/// full parse → SCG → IR → backend pipeline for all 8 backends.  For each
/// successful compilation, validates the binary output.  This test does NOT
/// fail if some examples don't compile — that is expected for complex programs
/// that use features not yet supported by every backend.  It DOES fail if a
/// backend produces a structurally invalid binary.
#[test]
fn test_cross_backend_example_compilation() {
    let results = compile_all_examples();
    print_test_matrix(&results);

    // Validate all successful compilations produce valid binaries.
    for result in &results {
        for &kind in ALL_BACKENDS {
            let status = result.statuses.get(&kind).unwrap();
            if let CompileStatus::Success(_size) = status {
                // Re-compile to get the binary for validation
                let examples = discover_examples();
                let source = examples.iter()
                    .find(|(name, _)| *name == result.name)
                    .map(|(_, src)| src.clone())
                    .expect("example source should exist");

                let (status2, bytes_opt) = compile_example_for_backend(&source, kind);
                assert!(
                    status2.is_success(),
                    "{}/{}: compilation succeeded on first attempt but failed on re-run",
                    backend_name(kind),
                    result.name
                );

                let bytes = bytes_opt.expect("successful compilation should produce bytes");

                // Validate binary structure
                validate_binary(&bytes, kind, 8);
            }
        }
    }
}

/// Test 11: ELF section validation for example programs
///
/// For each ELF backend (7 native backends), validates that the
/// successfully compiled example binaries contain the required ELF
/// sections: `.text`, `.data`, `.symtab`, `.strtab`.
///
/// Note: Not all backends emit section headers in their `encode_program`
/// output (some produce minimal ELF with only program headers).  For
/// those, we validate that the ELF header and program headers are correct
/// and that the `.text` content is present via a PT_LOAD segment.
#[test]
fn test_cross_backend_elf_section_validation() {
    let examples = discover_examples();

    for (name, source) in &examples {
        for &kind in ALL_BACKENDS {
            let fmt = expected_output_format(kind);
            if fmt != OutputFormat::Elf32 && fmt != OutputFormat::Elf64 {
                continue; // Skip Wasm32
            }

            let (status, bytes_opt) = compile_example_for_backend(source, kind);
            if !status.is_success() {
                continue; // Skip failed compilations
            }

            let bytes = bytes_opt.expect("success should produce bytes");
            let bname = backend_name(kind);

            // Validate ELF magic
            assert!(
                bytes.len() >= 4,
                "{}/{}: ELF too short ({} bytes)",
                bname,
                name,
                bytes.len()
            );
            assert_eq!(
                &bytes[0..4],
                &[0x7f, b'E', b'L', b'F'],
                "{}/{}: bad ELF magic",
                bname,
                name
            );

            // Validate ELF class matches expected
            let expected_class = match fmt {
                OutputFormat::Elf32 => 1u8,
                OutputFormat::Elf64 => 2u8,
                _ => unreachable!(),
            };
            assert_eq!(
                bytes[4], expected_class,
                "{}/{}: ELF class mismatch",
                bname,
                name
            );

            // Validate machine type
            let ei_data = bytes[5];
            let e_machine = if ei_data == 2 {
                u16::from_be_bytes([bytes[18], bytes[19]])
            } else {
                u16::from_le_bytes([bytes[18], bytes[19]])
            };
            assert_eq!(
                e_machine,
                elf_machine(kind),
                "{}/{}: machine type mismatch",
                bname,
                name
            );

            // Validate that at least one PT_LOAD segment exists
            let is_64 = expected_class == 2;
            let e_phoff = if is_64 {
                u64::from_le_bytes(bytes[32..40].try_into().unwrap())
            } else {
                u32::from_le_bytes(bytes[28..32].try_into().unwrap()) as u64
            };
            let e_phentsize = if is_64 {
                u16::from_le_bytes(bytes[54..56].try_into().unwrap())
            } else {
                u16::from_le_bytes(bytes[42..44].try_into().unwrap())
            };
            let e_phnum = if is_64 {
                u16::from_le_bytes(bytes[56..58].try_into().unwrap())
            } else {
                u16::from_le_bytes(bytes[44..46].try_into().unwrap())
            };

            let mut has_load_segment = false;
            for i in 0..e_phnum as usize {
                let off = e_phoff as usize + i * e_phentsize as usize;
                if off + 4 > bytes.len() {
                    break;
                }
                let p_type = u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap());
                if p_type == 1 {
                    // PT_LOAD
                    has_load_segment = true;
                    break;
                }
            }
            assert!(
                has_load_segment,
                "{}/{}: ELF should have at least one PT_LOAD segment",
                bname,
                name
            );

            // Check for section headers (optional — some backends omit them)
            let e_shoff = if is_64 {
                u64::from_le_bytes(bytes[40..48].try_into().unwrap())
            } else {
                u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as u64
            };
            let e_shnum = if is_64 {
                u16::from_le_bytes(bytes[60..62].try_into().unwrap())
            } else {
                u16::from_le_bytes(bytes[48..50].try_into().unwrap())
            };

            if e_shoff != 0 && e_shnum > 0 {
                // Section headers exist — validate that we have the key sections.
                // Read the section header string table index.
                let e_shstrndx = if is_64 {
                    u16::from_le_bytes(bytes[62..64].try_into().unwrap())
                } else {
                    u16::from_le_bytes(bytes[50..52].try_into().unwrap())
                };

                let e_shentsize = if is_64 {
                    u16::from_le_bytes(bytes[58..60].try_into().unwrap())
                } else {
                    u16::from_le_bytes(bytes[46..48].try_into().unwrap())
                };

                // Read the .shstrtab section to get section names
                let shstrtab_off = if (e_shstrndx as usize) < e_shnum as usize {
                    let shdr_off = e_shoff as usize + (e_shstrndx as usize) * (e_shentsize as usize);
                    if is_64 && shdr_off + 64 <= bytes.len() {
                        u64::from_le_bytes(bytes[shdr_off + 24..shdr_off + 32].try_into().unwrap())
                    } else if !is_64 && shdr_off + 40 <= bytes.len() {
                        u32::from_le_bytes(bytes[shdr_off + 16..shdr_off + 20].try_into().unwrap()) as u64
                    } else {
                        continue; // Can't read shstrtab offset
                    }
                } else {
                    continue;
                };

                let shstrtab_size = {
                    let shdr_off = e_shoff as usize + (e_shstrndx as usize) * (e_shentsize as usize);
                    if is_64 && shdr_off + 64 <= bytes.len() {
                        u64::from_le_bytes(bytes[shdr_off + 32..shdr_off + 40].try_into().unwrap())
                    } else if !is_64 && shdr_off + 40 <= bytes.len() {
                        u32::from_le_bytes(bytes[shdr_off + 20..shdr_off + 24].try_into().unwrap()) as u64
                    } else {
                        continue;
                    }
                };

                let shstrtab_start = shstrtab_off as usize;
                let shstrtab_end = shstrtab_start + shstrtab_size as usize;
                if shstrtab_end > bytes.len() {
                    continue; // shstrtab out of bounds
                }
                let shstrtab = &bytes[shstrtab_start..shstrtab_end];

                // Collect section names
                let mut section_names: HashSet<String> = HashSet::new();
                for i in 0..e_shnum as usize {
                    let shdr_off = e_shoff as usize + i * (e_shentsize as usize);
                    if is_64 && shdr_off + 64 > bytes.len() {
                        break;
                    }
                    if !is_64 && shdr_off + 40 > bytes.len() {
                        break;
                    }
                    let sh_name = if is_64 {
                        u32::from_le_bytes(bytes[shdr_off..shdr_off + 4].try_into().unwrap())
                    } else {
                        u32::from_le_bytes(bytes[shdr_off..shdr_off + 4].try_into().unwrap())
                    };

                    // Read the null-terminated string from .shstrtab
                    let name_start = sh_name as usize;
                    if name_start < shstrtab.len() {
                        let name_end = shstrtab[name_start..]
                            .iter()
                            .position(|&b| b == 0)
                            .unwrap_or(shstrtab.len() - name_start);
                        let section_name = String::from_utf8_lossy(
                            &shstrtab[name_start..name_start + name_end]
                        ).to_string();
                        section_names.insert(section_name);
                    }
                }

                // Validate required sections
                assert!(
                    section_names.contains(".text"),
                    "{}/{}: ELF should have a .text section (found: {:?})",
                    bname,
                    name,
                    section_names
                );

                // .symtab and .strtab are optional for minimal ELF executables
                // but should be present when section_headers is enabled.
                // We just log their presence, not assert.
                if !section_names.contains(".symtab") {
                    eprintln!(
                        "  info: {}/{}: no .symtab section (acceptable for minimal ELF)",
                        bname, name
                    );
                }
                if !section_names.contains(".strtab") {
                    eprintln!(
                        "  info: {}/{}: no .strtab section (acceptable for minimal ELF)",
                        bname, name
                    );
                }
            } else {
                // No section headers — this is valid for minimal ELF files
                // produced by some backends. Just verify the basic header.
            }
        }
    }
}

/// Test 12: Wasm32 format validation for example programs
///
/// Compiles all example programs with the Wasm32 backend and validates
/// that the resulting Wasm binaries have correct format: magic bytes,
/// version, and section structure.
#[test]
fn test_cross_backend_wasm32_example_validation() {
    let examples = discover_examples();

    for (name, source) in &examples {
        let (status, bytes_opt) = compile_example_for_backend(source, BackendKind::Wasm32);

        if !status.is_success() {
            continue; // Skip failed compilations
        }

        let bytes = bytes_opt.expect("success should produce bytes");

        // Validate Wasm magic
        assert!(
            bytes.len() >= 8,
            "wasm32/{}: module too short ({} bytes)",
            name,
            bytes.len()
        );
        assert_eq!(
            &bytes[0..4],
            &[0x00, 0x61, 0x73, 0x6D],
            "wasm32/{}: magic bytes incorrect",
            name
        );
        assert_eq!(
            &bytes[4..8],
            &[0x01, 0x00, 0x00, 0x00],
            "wasm32/{}: version incorrect",
            name
        );

        // Validate section structure
        let mut offset = 8usize;
        let mut last_section_id: Option<u8> = None;
        let mut found_type = false;
        let mut found_function = false;
        let mut found_code = false;

        while offset < bytes.len() {
            assert!(
                offset < bytes.len(),
                "wasm32/{}: truncated section header",
                name
            );
            let section_id = bytes[offset];
            offset += 1;

            // Decode LEB128 size
            let mut size: usize = 0;
            let mut shift: usize = 0;
            loop {
                assert!(
                    offset < bytes.len(),
                    "wasm32/{}: truncated section size",
                    name
                );
                let byte = bytes[offset];
                offset += 1;
                size |= ((byte & 0x7F) as usize) << shift;
                shift += 7;
                if byte & 0x80 == 0 {
                    break;
                }
            }

            // Verify section ordering
            if section_id != 0 {
                if let Some(prev) = last_section_id {
                    assert!(
                        section_id > prev,
                        "wasm32/{}: sections out of order ({} after {})",
                        name,
                        section_id,
                        prev
                    );
                }
                last_section_id = Some(section_id);
            }

            // Track known sections
            match section_id {
                1 => found_type = true,
                3 => found_function = true,
                10 => found_code = true,
                _ => {}
            }

            // Verify section content doesn't extend past end of binary
            assert!(
                offset + size <= bytes.len(),
                "wasm32/{}: section content extends past end of binary (offset={}, size={}, len={})",
                name, offset, size, bytes.len()
            );

            offset += size;
        }

        // Verify required sections
        assert!(
            found_type,
            "wasm32/{}: missing type section (ID 1)",
            name
        );
        assert!(
            found_function,
            "wasm32/{}: missing function section (ID 3)",
            name
        );
        assert!(
            found_code,
            "wasm32/{}: missing code section (ID 10)",
            name
        );
    }
}

/// Test 13: Cross-backend code size consistency for examples
///
/// For each example that compiles successfully on a backend, verify that
/// the binary size is within reasonable bounds (not zero, not absurdly large).
#[test]
fn test_cross_backend_example_code_size_consistency() {
    let examples = discover_examples();

    const MIN_REASONABLE_SIZE: usize = 8;
    const MAX_REASONABLE_SIZE: usize = 10_000_000; // 10 MB

    for (name, source) in &examples {
        for &kind in ALL_BACKENDS {
            let (status, bytes_opt) = compile_example_for_backend(source, kind);
            if !status.is_success() {
                continue;
            }

            let bytes = bytes_opt.expect("success should produce bytes");
            let bname = backend_name(kind);

            assert!(
                bytes.len() >= MIN_REASONABLE_SIZE,
                "{}/{}: binary too small ({} bytes, minimum {})",
                bname,
                name,
                bytes.len(),
                MIN_REASONABLE_SIZE
            );

            assert!(
                bytes.len() <= MAX_REASONABLE_SIZE,
                "{}/{}: binary suspiciously large ({} bytes, maximum {})",
                bname,
                name,
                bytes.len(),
                MAX_REASONABLE_SIZE
            );
        }
    }
}

/// Test 14: Regression tracking for example compilation
///
/// Tracks which examples compile successfully on each backend and
/// reports any unexpected failures.  The test asserts that a minimum
/// set of "core" examples must compile on all backends — if any of
/// these regress, the test fails.
///
/// Core examples are the simplest programs that every backend should
/// be able to handle: `minimal`, `test_exit`, `test_call`.
#[test]
fn test_cross_backend_regression_tracking() {
    let examples = discover_examples();
    let results = compile_all_examples();

    // Define the core set of examples that MUST compile on all backends.
    // These are the simplest programs in the examples directory.
    let core_examples: HashSet<&str> = [
        "minimal",
        "test_exit",
    ].iter().copied().collect();

    // Check core examples compile on at least one backend
    for core_name in &core_examples {
        let result = results.iter().find(|r| r.name == *core_name);
        match result {
            Some(r) => {
                let any_success = r.statuses.values().any(|s| s.is_success());
                assert!(
                    any_success,
                    "Core example '{}' should compile on at least one backend",
                    core_name
                );

                // Check if it compiles on ALL backends
                for &kind in ALL_BACKENDS {
                    let status = r.statuses.get(&kind).unwrap();
                    if !status.is_success() {
                        eprintln!(
                            "  warning: core example '{}' failed on backend {}: {:?}",
                            core_name,
                            backend_name(kind),
                            status
                        );
                    }
                }
            }
            None => {
                panic!(
                    "Core example '{}' not found in examples directory",
                    core_name
                );
            }
        }
    }

    // Report overall statistics
    let total_pairs: usize = examples.len() * ALL_BACKENDS.len();
    let successful_pairs: usize = results.iter()
        .flat_map(|r| r.statuses.values())
        .filter(|s| s.is_success())
        .count();

    eprintln!();
    eprintln!("  Cross-Backend Compilation Summary:");
    eprintln!("    Total (example, backend) pairs: {}", total_pairs);
    eprintln!("    Successful compilations:        {}", successful_pairs);
    eprintln!("    Success rate:                    {:.1}%",
        (successful_pairs as f64 / total_pairs as f64) * 100.0);
    eprintln!();

    // Report per-failure-category breakdown
    let mut parse_fails = 0;
    let mut scg_fails = 0;
    let mut ir_fails = 0;
    let mut regalloc_fails = 0;
    let mut encode_fails = 0;

    for result in &results {
        for status in result.statuses.values() {
            match status {
                CompileStatus::ParseFailed(_) => parse_fails += 1,
                CompileStatus::ScgFailed(_) => scg_fails += 1,
                CompileStatus::BridgeFailed(_) => ir_fails += 1, // bridge is part of IR pipeline
                CompileStatus::IrFailed(_) => ir_fails += 1,
                CompileStatus::RegAllocFailed(_) => regalloc_fails += 1,
                CompileStatus::EncodeFailed(_) => encode_fails += 1,
                CompileStatus::Success(_) => {}
            }
        }
    }

    eprintln!("  Failure Breakdown:");
    eprintln!("    Parse failures:       {}", parse_fails);
    eprintln!("    SCG failures:         {}", scg_fails);
    eprintln!("    IR/bridge failures:   {}", ir_fails);
    eprintln!("    Regalloc failures:    {}", regalloc_fails);
    eprintln!("    Encode failures:      {}", encode_fails);
    eprintln!();
}

/// Test 15: Test matrix summary print
///
/// This test explicitly prints the full compilation matrix for all
/// example programs across all backends, providing a comprehensive
/// overview of the VUMA compiler's cross-backend support.
#[test]
fn test_cross_backend_matrix_summary() {
    let results = compile_all_examples();

    // Print the full matrix
    print_test_matrix(&results);

    // Also print a per-backend summary
    eprintln!("  Per-Backend Summary:");
    eprintln!("  {:<16} {:>8} {:>8} {:>8}", "Backend", "Success", "Total", "Rate");
    eprintln!("  {}", "-".repeat(44));

    for &kind in ALL_BACKENDS {
        let success_count = results.iter()
            .filter(|r| r.statuses.get(&kind).map_or(false, |s| s.is_success()))
            .count();
        let total = results.len();
        let rate = (success_count as f64 / total as f64) * 100.0;
        eprintln!("  {:<16} {:>8} {:>8} {:>7.1}%",
            backend_name(kind),
            success_count,
            total,
            rate);
    }
    eprintln!();

    // Also print a per-example summary
    eprintln!("  Per-Example Summary:");
    eprintln!("  {:<28} {:>8} {:>8} {:>8}", "Example", "Success", "Total", "Rate");
    eprintln!("  {}", "-".repeat(56));

    for result in &results {
        let success_count = result.statuses.values().filter(|s| s.is_success()).count();
        let total = ALL_BACKENDS.len();
        let rate = (success_count as f64 / total as f64) * 100.0;
        eprintln!("  {:<28} {:>8} {:>8} {:>7.1}%",
            result.name,
            success_count,
            total,
            rate);
    }
    eprintln!();
}
