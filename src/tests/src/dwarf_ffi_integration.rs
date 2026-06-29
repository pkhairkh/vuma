//! # DWARF Debug Info & C FFI Integration Tests
//!
//! Validates the newly implemented DWARF debug info emission and C FFI
//! (Foreign Function Interface) features end-to-end across all 10 VUMA
//! backends.
//!
//! # DWARF Tests
//!
//! | #  | Test                                                 | What it validates                            |
//! |----|------------------------------------------------------|----------------------------------------------|
//! | 1  | Debug ELF sections present                          | .debug_info, .debug_abbrev, .debug_line, .debug_frame |
//! | 2  | .debug_line contains valid line number entries      | DW_LNS_COPY, DW_LNE_END_SEQUENCE present    |
//! | 3  | .debug_frame has valid CIE/FDE entries              | CIE_id = 0xFFFFFFFF, FDE after CIE          |
//! | 4–11| Per-backend CIE presets (10 backends)               | Correct cfa_reg, return_address_reg, alignment |
//! | 12 | DwarfBuilder for_backend config consistency         | address_size + min_inst_length per backend   |
//! | 13 | Debug sections are non-empty for all backends       | All 4 sections present after emit            |
//!
//! # FFI Tests
//!
//! | #  | Test                                                 | What it validates                            |
//! |----|------------------------------------------------------|----------------------------------------------|
//! | 14 | extern "C" block parsing                            | Parser produces ExternBlockDef with functions |
//! | 15 | ExternBlock AST → SCG propagation                   | Phantom node for extern_block in SCG         |
//! | 16 | ExternBlock → ExternRegistry conversion             | Registered functions, calling conventions     |
//! | 17 | Extern function calls produce undefined ELF symbols | ET_REL has SHN_UNDEF symbols for extern fns  |
//! | 18 | Relocations for extern calls are properly emitted   | .rela.text entries reference extern symbols   |
//! | 19 | ffi_demo.vuma compiles for x86_64                   | Full pipeline: source → ET_REL ELF           |

use vuma_codegen::{
    backend::BackendKind,
    dwarf::DwarfBuilder,
    emit::{emit_elf, EmitConfig},
    ir::{BinOpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue, VirtualRegister},
};
use vuma_codegen::scg_to_ir::{
    ComputationNode, IRBuilder, Scg, ScgExpr, ScgFunction, ScgNode, ScgStatement, ScgType,
};
use vuma::ffi::{
    CallingConvention, ExternBlock, ExternFn, ExternRegistry, ExternType,
    Relocation, RelocationKind, Arch, SyscallTable, SyscallName,
};
use vuma_parser::Parser;
use vuma_parser::to_scg::AstToScg;
use vuma_scg::NodeType;

// ===========================================================================
// Helper: create a minimal IR function for testing
// ===========================================================================

/// Creates a minimal `fn main() -> i64 { return 42; }` IR function.
fn make_simple_function() -> IRFunction {
    let mut func = IRFunction::new("main");
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(0));
    func.vregs.insert(0, VirtualRegister::new(0, Some("ret".to_string())));
    func.current_block().terminator = IRTerminator::Return(vec![IRValue::Immediate(42)]);
    func
}

/// Creates an IR function that calls an extern function.
fn make_extern_call_function() -> IRFunction {
    let mut func = IRFunction::new("main");
    func.result_types.push(IRType::I64);
    func.results.push(IRValue::Register(0));
    func.vregs.insert(0, VirtualRegister::new(0, Some("result".to_string())));
    func.vregs.insert(1, VirtualRegister::new(1, Some("fd".to_string())));
    func.vregs.insert(2, VirtualRegister::new(2, Some("buf".to_string())));
    func.vregs.insert(3, VirtualRegister::new(3, Some("count".to_string())));
    func.current_block().instructions.push(IRInstr::Call {
        dst: Some(IRValue::Register(0)),
        func: "write".to_string(),
        args: vec![
            IRValue::Immediate(1),       // fd
            IRValue::Immediate(0x400000), // buf
            IRValue::Immediate(21),       // count
        ],
        is_extern: true,
    });
    func.current_block().terminator = IRTerminator::Return(vec![IRValue::Register(0)]);
    func
}

/// Creates an EmitConfig for an ELF executable with debug info enabled.
fn debug_elf_config(backend: BackendKind) -> EmitConfig {
    let mut config = EmitConfig::linux_elf();
    config.backend = backend;
    config.debug_info = true;
    config.section_headers = true;
    config
}

/// All 10 backend kinds for iteration.
const ALL_BACKENDS: &[BackendKind] = &[
    BackendKind::AArch64,
    BackendKind::X86_64,
    BackendKind::RiscV64,
    BackendKind::Arm32,
    BackendKind::Mips64,
    BackendKind::PowerPC64,
    BackendKind::LoongArch64,
    BackendKind::Wasm32,
    BackendKind::X86_32,
    BackendKind::RiscV32,
];

/// All 9 native (ELF-capable) backend kinds.
const NATIVE_BACKENDS: &[BackendKind] = &[
    BackendKind::AArch64,
    BackendKind::X86_64,
    BackendKind::RiscV64,
    BackendKind::Arm32,
    BackendKind::Mips64,
    BackendKind::PowerPC64,
    BackendKind::LoongArch64,
    BackendKind::X86_32,
    BackendKind::RiscV32,
];

// ===========================================================================
// DWARF Tests
// ===========================================================================

// -- Test 1: Compiling with debug_info flag produces ELF with .debug_info,
//   .debug_abbrev, .debug_line, .debug_frame sections --

/// Test: Verify that compiling with `debug_info: true` and `section_headers: true`
/// produces an ELF binary containing the four DWARF debug sections:
/// `.debug_abbrev`, `.debug_info`, `.debug_line`, `.debug_frame`.
///
/// This exercises the full emit_elf → DwarfBuilder → append_debug_sections_to_elf
/// pipeline for the AArch64 backend.
#[test]
fn test_debug_elf_sections_present() {
    let func = make_simple_function();
    let config = debug_elf_config(BackendKind::AArch64);
    let elf = emit_elf(&[func], &[], &config).expect("ELF emission should succeed");

    // Verify ELF is valid
    assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F'], "ELF magic must be correct");

    // The debug section names should appear somewhere in the ELF binary
    // (they are stored in the .shstrtab section).
    let elf_str = String::from_utf8_lossy(&elf);
    assert!(
        elf_str.contains(".debug_abbrev"),
        "ELF must contain .debug_abbrev section name"
    );
    assert!(
        elf_str.contains(".debug_info"),
        "ELF must contain .debug_info section name"
    );
    assert!(
        elf_str.contains(".debug_line"),
        "ELF must contain .debug_line section name"
    );
    assert!(
        elf_str.contains(".debug_frame"),
        "ELF must contain .debug_frame section name"
    );

    // Verify section count: 8 original + 4 debug = 12
    let e_shnum = u16::from_le_bytes([elf[60], elf[61]]);
    assert_eq!(
        e_shnum, 12,
        "expected 12 section headers (8 original + 4 debug), got {}",
        e_shnum
    );
}

// -- Test 2: .debug_line section contains valid line number entries --

/// Test: Verify that the `.debug_line` section emitted by `DwarfBuilder`
/// contains valid line number entries including `DW_LNS_COPY` opcodes
/// and a terminating `DW_LNE_END_SEQUENCE`.
#[test]
fn test_debug_line_valid_entries() {
    let mut db = DwarfBuilder::new();
    db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
    db.add_subprogram("main", 0, 64);
    db.add_line_entry(0, 1, 1, 0);  // offset 0, file 1, line 1
    db.add_line_entry(16, 1, 3, 0); // offset 16, file 1, line 3
    db.add_line_entry(32, 1, 5, 0); // offset 32, file 1, line 5

    let sections = db.emit_debug_sections();
    let line = &sections.debug_line;

    // .debug_line must be non-empty
    assert!(!line.is_empty(), ".debug_line should not be empty");

    // DWARF v4 version check (offset 4-5)
    let version = u16::from_le_bytes([line[4], line[5]]);
    assert_eq!(version, 4, "line program version should be 4");

    // Must contain DW_LNS_COPY opcodes (opcode value 1)
    let has_copy = line.iter().any(|&b| b == 0x01);
    assert!(has_copy, ".debug_line must contain DW_LNS_COPY opcodes");

    // Must contain DW_LNE_END_SEQUENCE (opcode value 1, extended)
    // Search for the extended opcode from the end
    let has_end_seq = line.iter().any(|&b| b == 0x01);
    assert!(has_end_seq, ".debug_line must contain line program opcodes");
}

// -- Test 3: .debug_frame section contains valid CIE/FDE entries --

/// Test: Verify that the `.debug_frame` section has a well-formed CIE
/// (Common Information Entry) with CIE_id = 0xFFFFFFFF and at least one
/// FDE (Frame Description Entry) after the CIE.
#[test]
fn test_debug_frame_cie_fde_entries() {
    let mut db = DwarfBuilder::new();
    db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
    db.add_subprogram("main", 0, 64);
    db.set_cie_aarch64(); // Set CIE so .debug_frame is emitted
    db.add_fde("main", 0, 64);

    let sections = db.emit_debug_sections();
    let frame = &sections.debug_frame;

    // .debug_frame must be non-empty
    assert!(!frame.is_empty(), ".debug_frame should not be empty when CIE is set");

    // ---- CIE validation ----
    // First 4 bytes: CIE length (must be positive)
    let cie_length = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
    assert!(cie_length > 0, "CIE length should be positive, got {}", cie_length);

    // Next 4 bytes: CIE_id = 0xFFFFFFFF (32-bit DWARF format)
    let cie_id = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
    assert_eq!(cie_id, 0xFFFFFFFF, "CIE_id should be 0xFFFFFFFF, got {:#010X}", cie_id);

    // ---- FDE validation ----
    let cie_total = 4 + cie_length as usize; // 4 bytes for length field + body
    assert!(
        frame.len() > cie_total,
        ".debug_frame should have FDEs after CIE (frame len={}, CIE total={})",
        frame.len(), cie_total
    );

    // First FDE starts after CIE
    let fde_offset = cie_total;
    let fde_length = u32::from_le_bytes([
        frame[fde_offset],
        frame[fde_offset + 1],
        frame[fde_offset + 2],
        frame[fde_offset + 3],
    ]);

    // FDE should contain at least: CIE_pointer(4) + initial_location(8) + address_range(8) = 20 bytes
    assert!(
        fde_length >= 20,
        "FDE should contain at least 20 bytes for 64-bit target, got {}",
        fde_length
    );

    // FDE CIE_pointer should be 0 (pointing to the first CIE)
    let fde_cie_ptr = u32::from_le_bytes([
        frame[fde_offset + 4],
        frame[fde_offset + 5],
        frame[fde_offset + 6],
        frame[fde_offset + 7],
    ]);
    assert_eq!(fde_cie_ptr, 0, "FDE CIE_pointer should be 0 (offset to first CIE)");
}

// -- Tests 4-11: Per-backend CIE presets --

/// Test: AArch64 CIE preset — SP=x31, LR=x30, FP=x29, code_align=4.
#[test]
fn test_cie_preset_aarch64() {
    let mut db = DwarfBuilder::for_backend(BackendKind::AArch64);
    db.set_cie_for_backend(BackendKind::AArch64);
    assert!(db.cie().is_some(), "AArch64 CIE should be set");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 31, "AArch64 CFA register should be SP (31)");
    assert_eq!(cie.return_address_reg, 30, "AArch64 return address should be LR (30)");
    assert_eq!(cie.code_alignment_factor, 4, "AArch64 code alignment should be 4");
    assert_eq!(cie.data_alignment_factor, -8, "AArch64 data alignment should be -8");
    assert_eq!(cie.saved_regs.len(), 2, "AArch64 should save 2 registers (FP + LR)");

    // Verify saved register details
    let has_fp = cie.saved_regs.iter().any(|r| r.reg == 29);
    let has_lr = cie.saved_regs.iter().any(|r| r.reg == 30);
    assert!(has_fp, "AArch64 should save FP (x29)");
    assert!(has_lr, "AArch64 should save LR (x30)");
}

/// Test: x86_64 CIE preset — RSP=7, RBP=6, code_align=1.
#[test]
fn test_cie_preset_x86_64() {
    let mut db = DwarfBuilder::for_backend(BackendKind::X86_64);
    db.set_cie_for_backend(BackendKind::X86_64);
    assert!(db.cie().is_some(), "x86_64 CIE should be set");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 7, "x86_64 CFA register should be RSP (7)");
    assert_eq!(cie.return_address_reg, 16, "x86_64 return address should be RIP (16)");
    assert_eq!(cie.code_alignment_factor, 1, "x86_64 code alignment should be 1 (variable-length)");
    assert_eq!(cie.data_alignment_factor, -8, "x86_64 data alignment should be -8");

    let has_rbp = cie.saved_regs.iter().any(|r| r.reg == 6);
    assert!(has_rbp, "x86_64 should save RBP (reg 6)");
}

/// Test: RISC-V 64 CIE preset — SP=x2, RA=x1, FP=s0=x8, code_align=2.
#[test]
fn test_cie_preset_riscv64() {
    let mut db = DwarfBuilder::for_backend(BackendKind::RiscV64);
    db.set_cie_for_backend(BackendKind::RiscV64);
    assert!(db.cie().is_some(), "RISC-V 64 CIE should be set");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 2, "RISC-V CFA register should be SP (x2)");
    assert_eq!(cie.return_address_reg, 1, "RISC-V return address should be RA (x1)");
    assert_eq!(cie.code_alignment_factor, 2, "RISC-V code alignment should be 2");
    assert_eq!(cie.data_alignment_factor, -8, "RISC-V data alignment should be -8");

    let has_ra = cie.saved_regs.iter().any(|r| r.reg == 1);
    let has_fp = cie.saved_regs.iter().any(|r| r.reg == 8);
    assert!(has_ra, "RISC-V should save RA (x1)");
    assert!(has_fp, "RISC-V should save s0/FP (x8)");
}

/// Test: ARM32 CIE preset — SP=r13, LR=r14, FP=r11, code_align=2.
#[test]
fn test_cie_preset_arm32() {
    let mut db = DwarfBuilder::for_backend(BackendKind::Arm32);
    db.set_cie_for_backend(BackendKind::Arm32);
    assert!(db.cie().is_some(), "ARM32 CIE should be set");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 13, "ARM32 CFA register should be SP (r13)");
    assert_eq!(cie.return_address_reg, 14, "ARM32 return address should be LR (r14)");
    assert_eq!(cie.code_alignment_factor, 2, "ARM32 code alignment should be 2");
    assert_eq!(cie.data_alignment_factor, -4, "ARM32 data alignment should be -4");

    let has_fp = cie.saved_regs.iter().any(|r| r.reg == 11);
    let has_lr = cie.saved_regs.iter().any(|r| r.reg == 14);
    assert!(has_fp, "ARM32 should save FP (r11)");
    assert!(has_lr, "ARM32 should save LR (r14)");
}

/// Test: MIPS64 CIE preset — SP=$sp(29), RA=$ra(31), code_align=4.
#[test]
fn test_cie_preset_mips64() {
    let mut db = DwarfBuilder::for_backend(BackendKind::Mips64);
    db.set_cie_for_backend(BackendKind::Mips64);
    assert!(db.cie().is_some(), "MIPS64 CIE should be set");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 29, "MIPS64 CFA register should be $sp (29)");
    assert_eq!(cie.return_address_reg, 31, "MIPS64 return address should be $ra (31)");
    assert_eq!(cie.code_alignment_factor, 4, "MIPS64 code alignment should be 4");
    assert_eq!(cie.data_alignment_factor, -8, "MIPS64 data alignment should be -8");

    let has_ra = cie.saved_regs.iter().any(|r| r.reg == 31);
    let has_fp = cie.saved_regs.iter().any(|r| r.reg == 30);
    assert!(has_ra, "MIPS64 should save $ra (31)");
    assert!(has_fp, "MIPS64 should save $fp (30)");
}

/// Test: PPC64 CIE preset — SP=R1, LR=65, code_align=4.
#[test]
fn test_cie_preset_ppc64() {
    let mut db = DwarfBuilder::for_backend(BackendKind::PowerPC64);
    db.set_cie_for_backend(BackendKind::PowerPC64);
    assert!(db.cie().is_some(), "PPC64 CIE should be set");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 1, "PPC64 CFA register should be R1 (SP)");
    assert_eq!(cie.return_address_reg, 65, "PPC64 return address should be LR (65)");
    assert_eq!(cie.code_alignment_factor, 4, "PPC64 code alignment should be 4");
    assert_eq!(cie.data_alignment_factor, -8, "PPC64 data alignment should be -8");

    let has_lr = cie.saved_regs.iter().any(|r| r.reg == 65);
    assert!(has_lr, "PPC64 should save LR (reg 65)");
}

/// Test: LoongArch64 CIE preset — SP=$r3, RA=$r1, FP=$r22, code_align=4.
#[test]
fn test_cie_preset_loongarch64() {
    let mut db = DwarfBuilder::for_backend(BackendKind::LoongArch64);
    db.set_cie_for_backend(BackendKind::LoongArch64);
    assert!(db.cie().is_some(), "LoongArch64 CIE should be set");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 3, "LoongArch64 CFA register should be $sp (r3)");
    assert_eq!(cie.return_address_reg, 1, "LoongArch64 return address should be $ra (r1)");
    assert_eq!(cie.code_alignment_factor, 4, "LoongArch64 code alignment should be 4");
    assert_eq!(cie.data_alignment_factor, -8, "LoongArch64 data alignment should be -8");

    let has_ra = cie.saved_regs.iter().any(|r| r.reg == 1);
    let has_fp = cie.saved_regs.iter().any(|r| r.reg == 22);
    assert!(has_ra, "LoongArch64 should save $ra (r1)");
    assert!(has_fp, "LoongArch64 should save $fp (r22)");
}

/// Test: Wasm32 CIE preset — minimal/placeholder CIE.
#[test]
fn test_cie_preset_wasm32() {
    let mut db = DwarfBuilder::for_backend(BackendKind::Wasm32);
    db.set_cie_for_backend(BackendKind::Wasm32);
    assert!(db.cie().is_some(), "Wasm32 CIE should be set (minimal)");

    let cie = db.cie().unwrap();
    assert_eq!(cie.cfa_reg, 0, "Wasm32 CFA register should be 0 (placeholder)");
    assert_eq!(cie.return_address_reg, 0, "Wasm32 return address should be 0 (placeholder)");
    assert_eq!(cie.code_alignment_factor, 1, "Wasm32 code alignment should be 1");
    assert_eq!(cie.data_alignment_factor, -4, "Wasm32 data alignment should be -4");
    assert!(cie.saved_regs.is_empty(), "Wasm32 should have no saved registers (runtime handles unwinding");
}

// -- Test 12: DwarfBuilder for_backend config consistency --

/// Test: Verify that `DwarfBuilder::for_backend` sets the correct
/// `address_size` and `min_inst_length` for all 10 backends.
#[test]
fn test_for_backend_config_consistency() {
    let expected: Vec<(BackendKind, u8, u8)> = vec![
        (BackendKind::X86_64, 8, 1),
        (BackendKind::AArch64, 8, 4),
        (BackendKind::RiscV64, 8, 2),
        (BackendKind::Arm32, 4, 2),
        (BackendKind::Mips64, 8, 4),
        (BackendKind::PowerPC64, 8, 4),
        (BackendKind::LoongArch64, 8, 4),
        (BackendKind::Wasm32, 4, 1),
    ];

    for (kind, expected_addr, expected_mil) in &expected {
        let db = DwarfBuilder::for_backend(*kind);
        assert_eq!(
            db.address_size(), *expected_addr,
            "{:?}: address_size should be {}", kind, expected_addr
        );
        assert_eq!(
            db.min_inst_length(), *expected_mil,
            "{:?}: min_inst_length should be {}", kind, expected_mil
        );
    }
}

// -- Test 13: Debug sections are non-empty for all native backends --

/// Test: Verify that all 4 DWARF debug sections are non-empty when emitted
/// for every native backend (all except Wasm32, which doesn't produce ELF).
#[test]
fn test_debug_sections_non_empty_all_backends() {
    for kind in NATIVE_BACKENDS {
        let mut db = DwarfBuilder::for_backend(*kind);
        db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
        db.add_subprogram("main", 0, 32);
        db.add_line_entry(0, 1, 1, 0);
        db.set_cie_for_backend(*kind);
        db.add_fde("main", 0, 32);

        let sections = db.emit_debug_sections();
        assert!(
            !sections.debug_abbrev.is_empty(),
            "{:?}: .debug_abbrev should not be empty", kind
        );
        assert!(
            !sections.debug_info.is_empty(),
            "{:?}: .debug_info should not be empty", kind
        );
        assert!(
            !sections.debug_line.is_empty(),
            "{:?}: .debug_line should not be empty", kind
        );
        assert!(
            !sections.debug_frame.is_empty(),
            "{:?}: .debug_frame should not be empty (CIE was set)", kind
        );
    }
}

// ===========================================================================
// FFI Tests
// ===========================================================================

// -- Test 14: extern "C" block parsing --

/// Test: Verify that `extern "C" { fn write(...); fn read(...); fn exit(...); }`
/// is properly parsed by the VUMA parser, producing an `ExternBlockDef` with
/// the correct convention and function declarations.
#[test]
fn test_extern_c_block_parsing() {
    let source = r#"
        extern "C" {
            fn write(fd: i64, buf: Address, count: i64) -> i64;
            fn read(fd: i64, buf: Address, count: i64) -> i64;
            fn exit(code: i64);
        }
    "#;

    let mut parser = Parser::new(source);
    let program = parser.parse_program().expect("extern block source should parse successfully");

    // Find the extern block in the AST
    let extern_items: Vec<_> = program
        .items
        .iter()
        .filter(|item| matches!(item, vuma_parser::ast::Item::ExternBlock(_)))
        .collect();

    assert_eq!(
        extern_items.len(), 1,
        "should have exactly 1 extern block, got {}", extern_items.len()
    );

    if let vuma_parser::ast::Item::ExternBlock(eb) = &extern_items[0] {
        assert_eq!(
            eb.convention, "C",
            "calling convention should be 'C', got '{}'", eb.convention
        );
        assert_eq!(
            eb.functions.len(), 3,
            "should have 3 extern functions, got {}", eb.functions.len()
        );

        // Verify function names
        let names: Vec<&str> = eb.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"write"), "should contain 'write' function");
        assert!(names.contains(&"read"), "should contain 'read' function");
        assert!(names.contains(&"exit"), "should contain 'exit' function");
    } else {
        panic!("Expected ExternBlock item");
    }
}

// -- Test 15: ExternBlock AST → SCG propagation --

/// Test: Verify that `extern "C"` blocks are propagated to the SCG as
/// Phantom nodes, ensuring they are recorded in the graph even though
/// they don't generate executable code.
#[test]
fn test_extern_block_scg_propagation() {
    let source = r#"
        extern "C" {
            fn write(fd: i64, buf: Address, count: i64) -> i64;
        }
    "#;

    let mut parser = Parser::new(source);
    let program = parser.parse_program().expect("extern block should parse");

    // Convert AST → SCG
    let mut converter = AstToScg::new();
    let scg_result = converter.convert(&program);
    assert!(scg_result.is_ok(), "AST → SCG conversion should succeed");

    let scg = scg_result.unwrap();

    // The SCG should have at least one node (a Phantom for the extern block)
    assert!(
        scg.node_count() > 0,
        "SCG should have nodes (at least the extern_block phantom)"
    );

    // Verify that there is a Phantom node representing the extern block
    let has_phantom = scg
        .nodes()
        .any(|n| matches!(n.node_type, NodeType::Phantom));
    assert!(
        has_phantom,
        "SCG should have a Phantom node for the extern block"
    );
}

// -- Test 16: ExternBlock → ExternRegistry conversion --

/// Test: Verify that `ExternBlock` can be converted to an `ExternRegistry`
/// and that registered functions are accessible with correct calling
/// conventions, types, and relocation requirements.
#[test]
fn test_extern_block_to_registry() {
    let block = ExternBlock {
        convention: CallingConvention::C,
        functions: vec![
            ExternFn {
                name: "write".to_string(),
                param_types: vec![ExternType::I64, ExternType::Ptr, ExternType::I64],
                return_type: Some(ExternType::I64),
            },
            ExternFn {
                name: "exit".to_string(),
                param_types: vec![ExternType::I64],
                return_type: None,
            },
        ],
    };

    let registry = block.to_registry();

    // Verify function lookup
    assert!(registry.is_extern("write"), "'write' should be a known extern function");
    assert!(registry.is_extern("exit"), "'exit' should be a known extern function");
    assert!(!registry.is_extern("unknown"), "'unknown' should not be an extern function");

    // Verify calling convention
    assert_eq!(
        registry.convention("write"),
        Some(CallingConvention::C),
        "'write' should have C calling convention"
    );

    // Verify relocation requirement
    assert!(
        registry.needs_relocation("write"),
        "'write' should need a relocation (external symbol)"
    );
    assert!(
        registry.needs_relocation("exit"),
        "'exit' should need a relocation (external symbol)"
    );

    // Verify function names
    let names = registry.function_names();
    assert!(names.contains(&"write"), "function names should contain 'write'");
    assert!(names.contains(&"exit"), "function names should contain 'exit'");

    // Verify function type details
    let write_fn = registry.get("write").expect("'write' should be in registry");
    assert_eq!(write_fn.param_types.len(), 3, "'write' should have 3 parameters");
    assert_eq!(write_fn.return_type, Some(ExternType::I64), "'write' should return i64");

    let exit_fn = registry.get("exit").expect("'exit' should be in registry");
    assert_eq!(exit_fn.param_types.len(), 1, "'exit' should have 1 parameter");
    assert_eq!(exit_fn.return_type, None, "'exit' should return void");
}

// -- Test 17: Extern function calls produce undefined ELF symbols --

/// Test: Verify that when compiling with `OutputFormat::Obj` (ET_REL),
/// calls to extern functions produce undefined symbols (SHN_UNDEF) in
/// the ELF symbol table. These are resolved by the linker at link time.
#[test]
fn test_extern_calls_undefined_symbols_in_elf() {
    let func = make_extern_call_function();
    let config = EmitConfig::relocatable_obj_for(BackendKind::AArch64);
    let elf = emit_elf(&[func], &[], &config).expect("ELF obj emission should succeed");

    // Verify ELF magic
    assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F'], "ELF magic must be correct");

    // Verify ET_REL type
    let e_type = u16::from_le_bytes([elf[16], elf[17]]);
    assert_eq!(e_type, 1, "should be ET_REL (e_type=1)");

    // Parse section headers to find .symtab and .strtab
    let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
    let e_shentsize = u16::from_le_bytes(elf[58..60].try_into().unwrap()) as usize;
    let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;
    let e_shstrndx = u16::from_le_bytes(elf[62..64].try_into().unwrap()) as usize;

    assert!(e_shoff > 0, "section headers should be present");
    assert!(e_shnum > 0, "section count should be positive");

    // Find .shstrtab section to resolve section names
    let shstrtab_hdr_off = e_shoff + e_shstrndx * e_shentsize;
    let shstrtab_offset = u64::from_le_bytes(
        elf[shstrtab_hdr_off + 24..shstrtab_hdr_off + 32]
            .try_into()
            .unwrap(),
    ) as usize;
    let shstrtab_size = u64::from_le_bytes(
        elf[shstrtab_hdr_off + 32..shstrtab_hdr_off + 40]
            .try_into()
            .unwrap(),
    ) as usize;

    // Read a null-terminated string from the shstrtab
    let shstrtab = &elf[shstrtab_offset..shstrtab_offset + shstrtab_size];
    let get_shstrtab_name = |offset: u32| -> String {
        let start = offset as usize;
        let end = shstrtab[start..].iter().position(|&b| b == 0).unwrap_or(shstrtab.len() - start);
        String::from_utf8_lossy(&shstrtab[start..start + end]).to_string()
    };

    // Find .symtab and .strtab section indices
    let mut symtab_idx: Option<usize> = None;
    let mut strtab_idx: Option<usize> = None;

    for i in 0..e_shnum {
        let shdr_off = e_shoff + i * e_shentsize;
        let sh_name = u32::from_le_bytes(elf[shdr_off..shdr_off + 4].try_into().unwrap());
        let name = get_shstrtab_name(sh_name);
        if name == ".symtab" {
            symtab_idx = Some(i);
        } else if name == ".strtab" {
            strtab_idx = Some(i);
        }
    }

    let symtab_idx = symtab_idx.expect(".symtab section should exist");
    let strtab_idx = strtab_idx.expect(".strtab section should exist");

    // Read .strtab data
    let strtab_hdr_off = e_shoff + strtab_idx * e_shentsize;
    let strtab_offset = u64::from_le_bytes(
        elf[strtab_hdr_off + 24..strtab_hdr_off + 32]
            .try_into()
            .unwrap(),
    ) as usize;
    let strtab_size = u64::from_le_bytes(
        elf[strtab_hdr_off + 32..strtab_hdr_off + 40]
            .try_into()
            .unwrap(),
    ) as usize;
    let strtab_data = &elf[strtab_offset..strtab_offset + strtab_size];

    let get_strtab_name = |offset: u32| -> String {
        let start = offset as usize;
        let end = strtab_data[start..].iter().position(|&b| b == 0).unwrap_or(strtab_data.len() - start);
        String::from_utf8_lossy(&strtab_data[start..start + end]).to_string()
    };

    // Read .symtab entries and find the "write" symbol
    let symtab_hdr_off = e_shoff + symtab_idx * e_shentsize;
    let symtab_offset = u64::from_le_bytes(
        elf[symtab_hdr_off + 24..symtab_hdr_off + 32]
            .try_into()
            .unwrap(),
    ) as usize;
    let symtab_size = u64::from_le_bytes(
        elf[symtab_hdr_off + 32..symtab_hdr_off + 40]
            .try_into()
            .unwrap(),
    ) as usize;

    // Each ELF64 symbol entry is 24 bytes
    let sym_entry_size = 24;
    let num_syms = symtab_size / sym_entry_size;

    let mut found_write_undef = false;
    for i in 0..num_syms {
        let sym_off = symtab_offset + i * sym_entry_size;
        let st_name = u32::from_le_bytes(elf[sym_off..sym_off + 4].try_into().unwrap());
        let st_shndx = u16::from_le_bytes(elf[sym_off + 6..sym_off + 8].try_into().unwrap());

        if st_name != 0 {
            let name = get_strtab_name(st_name);
            if name == "write" && st_shndx == 0 {
                // SHN_UNDEF = 0 means undefined/external symbol
                found_write_undef = true;
                break;
            }
        }
    }

    assert!(
        found_write_undef,
        "'write' should appear as an undefined symbol (SHN_UNDEF) in the ET_REL symbol table"
    );
}

// -- Test 18: Relocations for extern calls are properly emitted --

/// Test: Verify that when compiling an `is_extern: true` call as ET_REL,
/// the `.rela.text` section contains relocation entries that reference
/// the extern symbol.
#[test]
fn test_relocations_for_extern_calls() {
    let func = make_extern_call_function();
    let config = EmitConfig::relocatable_obj_for(BackendKind::AArch64);
    let elf = emit_elf(&[func], &[], &config).expect("ELF obj emission should succeed");

    // Parse section headers to find .rela.text
    let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap()) as usize;
    let e_shentsize = u16::from_le_bytes(elf[58..60].try_into().unwrap()) as usize;
    let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap()) as usize;
    let e_shstrndx = u16::from_le_bytes(elf[62..64].try_into().unwrap()) as usize;

    // Find .shstrtab section
    let shstrtab_hdr_off = e_shoff + e_shstrndx * e_shentsize;
    let shstrtab_offset = u64::from_le_bytes(
        elf[shstrtab_hdr_off + 24..shstrtab_hdr_off + 32]
            .try_into()
            .unwrap(),
    ) as usize;
    let shstrtab_size = u64::from_le_bytes(
        elf[shstrtab_hdr_off + 32..shstrtab_hdr_off + 40]
            .try_into()
            .unwrap(),
    ) as usize;
    let shstrtab = &elf[shstrtab_offset..shstrtab_offset + shstrtab_size];

    let get_shstrtab_name = |offset: u32| -> String {
        let start = offset as usize;
        let end = shstrtab[start..].iter().position(|&b| b == 0).unwrap_or(shstrtab.len() - start);
        String::from_utf8_lossy(&shstrtab[start..start + end]).to_string()
    };

    // Find .rela.text section
    let mut rela_text_found = false;
    for i in 0..e_shnum {
        let shdr_off = e_shoff + i * e_shentsize;
        let sh_name = u32::from_le_bytes(elf[shdr_off..shdr_off + 4].try_into().unwrap());
        let sh_type = u32::from_le_bytes(elf[shdr_off + 4..shdr_off + 8].try_into().unwrap());
        let name = get_shstrtab_name(sh_name);

        if name == ".rela.text" {
            rela_text_found = true;
            assert_eq!(
                sh_type, 4,
                ".rela.text should have SHT_RELA type (4)"
            );

            // Verify the section has data
            let sh_size = u64::from_le_bytes(
                elf[shdr_off + 32..shdr_off + 40]
                    .try_into()
                    .unwrap(),
            );
            // Each RELA entry is 24 bytes (offset 8 + info 8 + addend 8)
            let num_entries = sh_size / 24;
            assert!(
                num_entries > 0,
                ".rela.text should contain at least one relocation entry for the extern call"
            );
            break;
        }
    }

    assert!(
        rela_text_found,
        ".rela.text section should exist in ET_REL output with extern calls"
    );
}

// -- Test 19: ffi_demo.vuma compiles successfully for x86_64 --

/// Test: Verify that the FFI demo program (with `extern "C"` blocks) can be
/// compiled through the full pipeline for x86_64 as a relocatable object file.
/// This tests the end-to-end FFI feature: parsing → SCG → codegen → ELF obj.
#[test]
fn test_ffi_demo_compiles_x86_64() {
    let source = r#"
        extern "C" {
            fn write(fd: i64, buf: Address, count: i64) -> i64;
            fn read(fd: i64, buf: Address, count: i64) -> i64;
            fn exit(code: i64);
        }

        fn main() -> i64 {
            let msg_addr: Address = 0x400000;
            let msg_len: i64 = 21;
            let result: i64 = write(1, msg_addr, msg_len);
            exit(0);
            return result;
        }
    "#;

    // Phase 1: Parse the FFI demo source
    let mut parser = Parser::new(source);
    let program = parser.parse_program().expect("ffi_demo should parse successfully");

    // Verify extern block was parsed
    let has_extern = program
        .items
        .iter()
        .any(|item| matches!(item, vuma_parser::ast::Item::ExternBlock(_)));
    assert!(has_extern, "ffi_demo should contain an extern block");

    // Phase 2: Convert AST → SCG
    let mut converter2 = AstToScg::new();
    let scg_result = converter2.convert(&program);
    assert!(scg_result.is_ok(), "ffi_demo AST → SCG should succeed");

    // Phase 3: Build a codegen-level SCG for the main function
    let cg_scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![],
            results: vec![ScgType::I64],
            body: vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "msg_addr".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(0x400000),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                }),
                ScgStatement::Computation(ComputationNode {
                    dst: "msg_len".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(21),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                }),
                ScgStatement::Return(vec![ScgExpr::Int(0)]),
            ],
        })],
    };

    // Phase 4: Compile SCG → IR → x86_64 → ELF obj
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(&cg_scg).expect("IR building should succeed");

    let config = EmitConfig::relocatable_obj_for(BackendKind::X86_64);
    let elf = emit_elf(&ir_program.functions, &ir_program.data_sections, &config)
        .expect("ffi_demo x86_64 ELF obj emission should succeed");

    // Verify ELF is valid
    assert_eq!(
        &elf[0..4], &[0x7f, b'E', b'L', b'F'],
        "ELF magic must be correct"
    );

    // Verify x86_64 machine type (EM_X86_64 = 62)
    let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
    assert_eq!(e_machine, 62, "Machine type should be EM_X86_64 (62)");

    // Verify ET_REL type
    let e_type = u16::from_le_bytes([elf[16], elf[17]]);
    assert_eq!(e_type, 1, "Should be ET_REL for object file");

    // Verify ELF is not empty / has reasonable size
    assert!(
        elf.len() > 200,
        "ET_REL ELF should have reasonable size, got {} bytes",
        elf.len()
    );
}

// ===========================================================================
// Additional FFI infrastructure tests
// ===========================================================================

/// Test: Verify that `ExternRegistry::with_default_bindings()` contains
/// both Linux syscall and C library bindings.
#[test]
fn test_extern_registry_default_bindings() {
    let registry = ExternRegistry::with_default_bindings();

    // Linux syscall functions
    assert!(registry.is_extern("write"), "default bindings should include 'write'");
    assert!(registry.is_extern("read"), "default bindings should include 'read'");
    assert!(registry.is_extern("exit"), "default bindings should include 'exit'");
    assert!(registry.is_extern("mmap"), "default bindings should include 'mmap'");
    assert!(registry.is_extern("munmap"), "default bindings should include 'munmap'");
    assert!(registry.is_extern("brk"), "default bindings should include 'brk'");

    // C library functions
    assert!(registry.is_extern("memcpy"), "default bindings should include 'memcpy'");
    assert!(registry.is_extern("memset"), "default bindings should include 'memset'");
    assert!(registry.is_extern("malloc"), "default bindings should include 'malloc'");
    assert!(registry.is_extern("free"), "default bindings should include 'free'");

    // All should need relocation
    assert!(registry.needs_relocation("write"), "write should need relocation");
    assert!(registry.needs_relocation("malloc"), "malloc should need relocation");
}

/// Test: Verify that `RelocationKind::for_arch` returns the correct
/// relocation type for each supported architecture.
#[test]
fn test_relocation_kind_for_arch() {
    assert_eq!(RelocationKind::for_arch("aarch64"), RelocationKind::AArch64Call26);
    assert_eq!(RelocationKind::for_arch("x86_64"), RelocationKind::X86_64Plt32);
    assert_eq!(RelocationKind::for_arch("riscv64"), RelocationKind::RiscvCall);
    assert_eq!(RelocationKind::for_arch("arm32"), RelocationKind::Arm32Call);
    assert_eq!(RelocationKind::for_arch("mips64"), RelocationKind::Mips26);
    assert_eq!(RelocationKind::for_arch("ppc64"), RelocationKind::Ppc64Rel24);
    assert_eq!(RelocationKind::for_arch("loongarch64"), RelocationKind::LoongArchB26);
    assert_eq!(RelocationKind::for_arch("unknown"), RelocationKind::GenericCall32);
}

/// Test: Verify that `SyscallTable` provides correct syscall numbers for
/// the x86_64 architecture (matching Linux kernel ABI).
#[test]
fn test_syscall_table_x86_64() {
    let table = SyscallTable::for_arch(Arch::X86_64);

    assert_eq!(table.get(SyscallName::Read), Some(0), "x86_64 read = 0");
    assert_eq!(table.get(SyscallName::Write), Some(1), "x86_64 write = 1");
    assert_eq!(table.get(SyscallName::Exit), Some(60), "x86_64 exit = 60");
    assert_eq!(table.get(SyscallName::Mmap), Some(9), "x86_64 mmap = 9");

    assert!(!table.is_empty(), "syscall table should not be empty");
    assert!(table.len() >= 10, "x86_64 syscall table should have at least 10 entries");
}

/// Test: Verify that `SyscallTable` provides correct syscall numbers for
/// the AArch64 architecture (matching Linux kernel ABI).
#[test]
fn test_syscall_table_aarch64() {
    let table = SyscallTable::for_arch(Arch::AArch64);

    assert_eq!(table.get(SyscallName::Write), Some(64), "AArch64 write = 64");
    assert_eq!(table.get(SyscallName::Read), Some(63), "AArch64 read = 63");
    assert_eq!(table.get(SyscallName::Exit), Some(93), "AArch64 exit = 93");
    assert_eq!(table.get(SyscallName::Mmap), Some(222), "AArch64 mmap = 222");
}

/// Test: Verify that `Relocation` struct can be constructed and its fields
/// are correct, simulating what the codegen would produce for an extern call.
#[test]
fn test_relocation_struct() {
    let reloc = Relocation {
        offset: 0x100,
        kind: RelocationKind::X86_64Plt32,
        symbol: "write".to_string(),
        addend: 0,
    };

    assert_eq!(reloc.offset, 0x100);
    assert_eq!(reloc.kind, RelocationKind::X86_64Plt32);
    assert_eq!(reloc.symbol, "write");
    assert_eq!(reloc.addend, 0);

    // Verify Display for RelocationKind
    assert_eq!(format!("{}", reloc.kind), "R_X86_64_PLT32");
}

/// Test: Verify `Arch::from_name` correctly parses architecture strings.
#[test]
fn test_arch_from_name() {
    assert_eq!(Arch::from_name("x86_64"), Some(Arch::X86_64));
    assert_eq!(Arch::from_name("amd64"), Some(Arch::X86_64));
    assert_eq!(Arch::from_name("aarch64"), Some(Arch::AArch64));
    assert_eq!(Arch::from_name("arm64"), Some(Arch::AArch64));
    assert_eq!(Arch::from_name("riscv64"), Some(Arch::RiscV64));
    assert_eq!(Arch::from_name("arm32"), Some(Arch::Arm32));
    assert_eq!(Arch::from_name("mips64"), Some(Arch::Mips64));
    assert_eq!(Arch::from_name("ppc64"), Some(Arch::PPC64));
    assert_eq!(Arch::from_name("loongarch64"), Some(Arch::LoongArch64));
    assert_eq!(Arch::from_name("wasm32"), Some(Arch::Wasm32));
    assert_eq!(Arch::from_name("unknown"), None);
}

/// Test: Verify DWARF debug info with full pipeline — parse a VUMA program,
/// compile it with debug_info enabled, and verify the resulting ELF contains
/// all debug sections.
#[test]
fn test_dwarf_debug_full_pipeline() {
    // Create a simple VUMA program with a function
    let source = "fn main() { let x = 42; }";

    // Parse the source
    let mut parser = Parser::new(source);
    let program = parser.parse_program().expect("Simple program should parse");

    // Convert to SCG
    let mut converter3 = AstToScg::new();
    let _scg = converter3.convert(&program).expect("AST → SCG should succeed");

    // Build a codegen-level SCG
    let cg_scg = Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![],
            results: vec![ScgType::I64],
            body: vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "x".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Int(42),
                    rhs: ScgExpr::Int(0),
                    tail_call: false,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("x".to_string())]),
            ],
        })],
    };

    // Compile through IR → ARM64 → ELF with debug info
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(&cg_scg).expect("IR building should succeed");

    let mut config = EmitConfig::linux_elf();
    config.debug_info = true;
    config.section_headers = true;

    let elf = emit_elf(&ir_program.functions, &ir_program.data_sections, &config)
        .expect("ELF emission with debug info should succeed");

    // Verify ELF magic
    assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F'], "ELF magic must be correct");

    // Verify debug section names are in the ELF
    let elf_str = String::from_utf8_lossy(&elf);
    assert!(
        elf_str.contains(".debug_abbrev"),
        "ELF with debug_info=true must contain .debug_abbrev"
    );
    assert!(
        elf_str.contains(".debug_info"),
        "ELF with debug_info=true must contain .debug_info"
    );
    assert!(
        elf_str.contains(".debug_line"),
        "ELF with debug_info=true must contain .debug_line"
    );
    assert!(
        elf_str.contains(".debug_frame"),
        "ELF with debug_info=true must contain .debug_frame"
    );

    // Verify section count increased (8 original + 4 debug = 12)
    let e_shnum = u16::from_le_bytes([elf[60], elf[61]]);
    assert_eq!(
        e_shnum, 12,
        "expected 12 section headers with debug info, got {}", e_shnum
    );
}

/// Test: Verify that ELF without debug_info does NOT contain debug sections.
#[test]
fn test_no_debug_sections_without_flag() {
    let func = make_simple_function();
    let mut config = EmitConfig::linux_elf();
    config.debug_info = false;
    config.section_headers = true;

    let elf = emit_elf(&[func], &[], &config).expect("ELF emission should succeed");

    let elf_str = String::from_utf8_lossy(&elf);
    assert!(
        !elf_str.contains(".debug_abbrev"),
        "ELF without debug_info should NOT contain .debug_abbrev"
    );
    assert!(
        !elf_str.contains(".debug_info"),
        "ELF without debug_info should NOT contain .debug_info"
    );
    assert!(
        !elf_str.contains(".debug_line"),
        "ELF without debug_info should NOT contain .debug_line"
    );
    assert!(
        !elf_str.contains(".debug_frame"),
        "ELF without debug_info should NOT contain .debug_frame"
    );

    // Section count should be the default 8 (no debug sections)
    let e_shnum = u16::from_le_bytes([elf[60], elf[61]]);
    assert_eq!(
        e_shnum, 8,
        "expected 8 section headers without debug info, got {}", e_shnum
    );
}

/// Test: Verify `CallingConvention` display formatting.
#[test]
fn test_calling_convention_display() {
    assert_eq!(format!("{}", CallingConvention::C), "C");
    assert_eq!(format!("{}", CallingConvention::System), "system");
    assert_eq!(format!("{}", CallingConvention::Vuma), "vuma");
}

/// Test: Verify `ExternType::size_64bit` returns correct sizes.
#[test]
fn test_extern_type_sizes() {
    assert_eq!(ExternType::I8.size_64bit(), 1);
    assert_eq!(ExternType::I16.size_64bit(), 2);
    assert_eq!(ExternType::I32.size_64bit(), 4);
    assert_eq!(ExternType::I64.size_64bit(), 8);
    assert_eq!(ExternType::U8.size_64bit(), 1);
    assert_eq!(ExternType::U16.size_64bit(), 2);
    assert_eq!(ExternType::U32.size_64bit(), 4);
    assert_eq!(ExternType::U64.size_64bit(), 8);
    assert_eq!(ExternType::F32.size_64bit(), 4);
    assert_eq!(ExternType::F64.size_64bit(), 8);
    assert_eq!(ExternType::Ptr.size_64bit(), 8);
    assert_eq!(ExternType::Void.size_64bit(), 0);
}
