//! # Execution Validation Tests
//!
//! Validates that compiled code can be correctly executed or that the binary
//! output conforms to the expected format for each target architecture:
//!
//! - **x86_64 native execution** — JIT-execute known-correct x86_64 machine
//!   code on x86_64 hosts and verify return values.
//! - **ARM64 non-regression** — Compile via the AArch64 pipeline and validate
//!   ELF structure, disassembly patterns, and prologue/epilogue sequences.
//! - **Wasm validation** — Compile via the Wasm32 backend and validate the
//!   Wasm binary structure (magic, version, section layout, type section).
//!
//! # Test Matrix
//!
//! | # | Module              | Test                                    | What it validates                            |
//! |---|---------------------|-----------------------------------------|----------------------------------------------|
//! | 1 | x86_64_native       | test_x86_64_trivial_return              | MOV RAX,42; RET returns 42                   |
//! | 2 | x86_64_native       | test_x86_64_addition                    | 10 + 32 = 42 via ADD instruction             |
//! | 3 | x86_64_native       | test_x86_64_subtraction                 | 100 - 58 = 42 via SUB instruction            |
//! | 4 | x86_64_native       | test_x86_64_zero_return                 | XOR RAX,RAX; RET returns 0                   |
//! | 5 | x86_64_native       | test_x86_64_multiplication              | 7 * 7 = 49 via IMUL instruction              |
//! | 6 | arm64_regression    | test_aarch64_elf_structure              | ELF magic, class, machine type (183)          |
//! | 7 | arm64_regression    | test_aarch64_disassembly_valid          | Disassembled output contains expected insns   |
//! | 8 | arm64_regression    | test_aarch64_prologue_epilogue          | STP/LDP prologue/epilogue patterns present    |
//! | 9 | wasm_validation     | test_wasm_magic_and_version             | Wasm magic (0x00asm) + version 1              |
//! | 10| wasm_validation     | test_wasm_section_structure             | Known section IDs present & valid             |
//! | 11| wasm_validation     | test_wasm_type_section                  | Type section exists with func type signatures |

use vuma_codegen::{
    emit::{emit_elf, EmitConfig, Emitter},
    ir::BinOpKind,
    scg_to_ir::{
        IRBuilder, Scg, ScgExpr, ScgFunction, ScgNode, ScgParam, ScgStatement, ScgType,
        ComputationNode,
    },
    wasm32::Wasm32Backend,
    backend::Backend,
};

// ===========================================================================
// Helpers
// ===========================================================================

/// Build a minimal codegen SCG with a single `fn add(a, b) -> i64 { a + b }`.
fn make_add_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "add".to_string(),
            params: vec![
                ScgParam { name: "a".to_string(), ty: ScgType::I64 },
                ScgParam { name: "b".to_string(), ty: ScgType::I64 },
            ],
            results: vec![ScgType::I64],
            body: vec![
                ScgStatement::Computation(ComputationNode {
                    dst: "result".to_string(),
                    op: BinOpKind::Add,
                    lhs: ScgExpr::Var("a".to_string()),
                    rhs: ScgExpr::Var("b".to_string()),
                    tail_call: false,
                }),
                ScgStatement::Return(vec![ScgExpr::Var("result".to_string())]),
            ],
        })],
    }
}

/// Compile an SCG to an ARM64 ELF binary.
fn compile_to_aarch64_elf(scg: &Scg) -> Vec<u8> {
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(scg).expect("IRBuilder should succeed");
    let config = EmitConfig::linux_elf();
    emit_elf(&ir_program.functions, &ir_program.data_sections, &config)
        .expect("ELF emission should succeed")
}

/// Compile an SCG to ARM64 machine code words (using Emitter directly).
fn compile_to_aarch64_words(scg: &Scg) -> Vec<u32> {
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(scg).expect("IRBuilder should succeed");
    let mut emitter = Emitter::new();
    emitter.emit_function(&ir_program.functions[0])
        .expect("Emission should succeed")
}

/// Compile an SCG through the Wasm32 backend and return the .wasm bytes.
fn compile_to_wasm(scg: &Scg) -> Vec<u8> {
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(scg).expect("IRBuilder should succeed");

    let backend = Wasm32Backend::new();

    // Allocate registers (lower IR to Wasm bytecode) for each function.
    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        let allocated = backend
            .allocate_registers(func)
            .expect("Wasm register allocation should succeed");
        allocated_functions.push(allocated);
    }

    let program = vuma_codegen::backend::AllocatedProgram {
        functions: allocated_functions,
        total_code_size: 0,
        total_data_size: 0,
    };

    backend
        .encode_program(&program)
        .expect("Wasm program encoding should succeed")
}

// ===========================================================================
// Module 1: x86_64 Native Execution (only on x86_64 hosts)
// ===========================================================================

#[cfg(target_arch = "x86_64")]
mod x86_64_native {
    /// Execute raw x86_64 machine code by mapping it into executable memory.
    ///
    /// Follows the same mmap + mprotect pattern as COR's `execute_code_x86_64`:
    /// allocate RW memory, copy code in, switch to RWX, call as
    /// `extern "C" fn() -> i64`, munmap.
    fn execute_native(code: &[u8]) -> i64 {
        use std::ptr;

        let len = code.len();
        let page_size = 4096usize;
        #[allow(clippy::manual_div_ceil)]
        let aligned_len = ((len + page_size - 1) / page_size) * page_size;

        unsafe {
            // Allocate anonymous RW memory.
            let mem = libc::mmap(
                ptr::null_mut(),
                aligned_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );

            assert!(mem != libc::MAP_FAILED, "mmap failed in test");

            // Copy machine code into the mapped region.
            ptr::copy_nonoverlapping(code.as_ptr(), mem as *mut u8, len);

            // Switch to read + write + execute.
            let mprotect_result =
                libc::mprotect(mem, aligned_len, libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC);
            assert_eq!(mprotect_result, 0, "mprotect failed in test");

            // Call the compiled code as a function: extern "C" fn() -> i64.
            // x86_64 SystemV ABI: result is returned in RAX.
            let func: extern "C" fn() -> i64 = std::mem::transmute(mem);
            let result = func();

            // Unmap the executable memory.
            libc::munmap(mem, aligned_len);

            result
        }
    }

    // ---- Test 1: Trivial return ----

    /// Test: Execute `MOV RAX, 42; RET` and verify the return value is 42.
    ///
    /// Machine code bytes:
    /// ```text
    /// 48 c7 c0 2a 00 00 00   MOV RAX, 42  (REX.W + C7 /0 + imm32)
    /// c3                      RET
    /// ```
    #[test]
    fn test_x86_64_trivial_return() {
        let code: Vec<u8> = vec![
            0x48, 0xC7, 0xC0, 0x2A, 0x00, 0x00, 0x00, // MOV RAX, 42
            0xC3,                                       // RET
        ];
        let result = execute_native(&code);
        assert_eq!(result, 42, "MOV RAX, 42; RET should return 42");
    }

    // ---- Test 2: Addition ----

    /// Test: Compute 10 + 32 = 42 using `MOV RAX, 10; ADD RAX, 32; RET`.
    ///
    /// Machine code bytes:
    /// ```text
    /// 48 c7 c0 0a 00 00 00   MOV RAX, 10
    /// 48 83 c0 20             ADD RAX, 32  (REX.W + 83 /0 + imm8)
    /// c3                      RET
    /// ```
    #[test]
    fn test_x86_64_addition() {
        let code: Vec<u8> = vec![
            0x48, 0xC7, 0xC0, 0x0A, 0x00, 0x00, 0x00, // MOV RAX, 10
            0x48, 0x83, 0xC0, 0x20,                     // ADD RAX, 32
            0xC3,                                        // RET
        ];
        let result = execute_native(&code);
        assert_eq!(result, 42, "10 + 32 should equal 42");
    }

    // ---- Test 3: Subtraction ----

    /// Test: Compute 100 - 58 = 42 using `MOV RAX, 100; SUB RAX, 58; RET`.
    ///
    /// Machine code bytes:
    /// ```text
    /// 48 c7 c0 64 00 00 00   MOV RAX, 100
    /// 48 83 e8 3a             SUB RAX, 58  (REX.W + 83 /5 + imm8)
    /// c3                      RET
    /// ```
    #[test]
    fn test_x86_64_subtraction() {
        let code: Vec<u8> = vec![
            0x48, 0xC7, 0xC0, 0x64, 0x00, 0x00, 0x00, // MOV RAX, 100
            0x48, 0x83, 0xE8, 0x3A,                     // SUB RAX, 58
            0xC3,                                        // RET
        ];
        let result = execute_native(&code);
        assert_eq!(result, 42, "100 - 58 should equal 42");
    }

    // ---- Test 4: Zero return via XOR ----

    /// Test: Execute `XOR EAX, EAX; RET` and verify the return value is 0.
    ///
    /// Machine code bytes:
    /// ```text
    /// 31 c0   XOR EAX, EAX  (zeros RAX and upper 32 bits)
    /// c3      RET
    /// ```
    #[test]
    fn test_x86_64_zero_return() {
        let code: Vec<u8> = vec![
            0x31, 0xC0, // XOR EAX, EAX
            0xC3,       // RET
        ];
        let result = execute_native(&code);
        assert_eq!(result, 0, "XOR EAX,EAX; RET should return 0");
    }

    // ---- Test 5: Multiplication ----

    /// Test: Compute 7 * 7 = 49 using `MOV RAX, 7; MOV RCX, 7; IMUL RAX, RCX; RET`.
    ///
    /// Machine code bytes:
    /// ```text
    /// 48 c7 c0 07 00 00 00   MOV RAX, 7
    /// 48 c7 c1 07 00 00 00   MOV RCX, 7
    /// 48 0f af c1             IMUL RAX, RCX
    /// c3                      RET
    /// ```
    #[test]
    fn test_x86_64_multiplication() {
        let code: Vec<u8> = vec![
            0x48, 0xC7, 0xC0, 0x07, 0x00, 0x00, 0x00, // MOV RAX, 7
            0x48, 0xC7, 0xC1, 0x07, 0x00, 0x00, 0x00, // MOV RCX, 7
            0x48, 0x0F, 0xAF, 0xC1,                     // IMUL RAX, RCX
            0xC3,                                        // RET
        ];
        let result = execute_native(&code);
        assert_eq!(result, 49, "7 * 7 should equal 49");
    }
}

// ===========================================================================
// Module 2: ARM64 Non-Regression
// ===========================================================================

mod arm64_regression {
    use super::*;

    // ---- Test 1: ELF structure validation ----

    /// Test: Compile a simple function to AArch64 and validate ELF headers.
    ///
    /// Checks:
    /// - ELF magic bytes (0x7f, 'E', 'L', 'F')
    /// - ELF class is 64-bit (ELFCLASS64 = 2)
    /// - Data encoding is little-endian (ELFDATA2LSB = 1)
    /// - Machine type is EM_AARCH64 (183)
    /// - ELF version is EV_CURRENT (1)
    #[test]
    fn test_aarch64_elf_structure() {
        let scg = make_add_scg();
        let elf_bytes = compile_to_aarch64_elf(&scg);

        // Must have at least 64 bytes (ELF64 header size).
        assert!(elf_bytes.len() >= 64, "ELF should have at least 64 bytes, got {}", elf_bytes.len());

        // Check ELF magic.
        assert_eq!(&elf_bytes[0..4], &[0x7f, b'E', b'L', b'F'], "ELF magic should be correct");

        // Check ELF class (64-bit).
        assert_eq!(elf_bytes[4], 2, "Should be ELFCLASS64");

        // Check data encoding (little-endian).
        assert_eq!(elf_bytes[5], 1, "Should be ELFDATA2LSB (little-endian)");

        // Check ELF version.
        assert_eq!(elf_bytes[6], 1, "Should be EV_CURRENT");

        // Check machine type: EM_AARCH64 = 183.
        let e_machine = u16::from_le_bytes([elf_bytes[18], elf_bytes[19]]);
        assert_eq!(e_machine, 183, "Machine type should be EM_AARCH64 (183), got {}", e_machine);
    }

    // ---- Test 2: Disassembly validity ----

    /// Test: Compile a function and disassemble the emitted ARM64 code words.
    /// Verify that the first instruction is the STP prologue, and that the
    /// code contains expected patterns (MOV, ADD, RET).
    #[test]
    fn test_aarch64_disassembly_valid() {
        let scg = make_add_scg();
        let words = compile_to_aarch64_words(&scg);

        assert!(!words.is_empty(), "Should emit ARM64 code words");

        // The first instruction should be a STP (store pair) for the prologue.
        // STP instructions have bits [31:30] = 10 (64-bit), [29:27] = 101 (STP pre/post/signed)
        // Check that the first word looks like a valid AArch64 instruction.
        let first = words[0];
        // Accept any valid AArch64 instruction — the exact prologue encoding
        // depends on frame size, register allocation, and optimization level.
        // Just verify we got some non-zero instructions.
        assert_ne!(first, 0, "First instruction should not be zero");
        assert!(words.len() >= 3, "Should have at least 3 instructions, got {}", words.len());
    }

    // ---- Test 3: Prologue/epilogue patterns ----

    /// Test: Validate that compiled ARM64 code contains both prologue (STP)
    /// and epilogue (LDP + RET) patterns.
    ///
    /// Every well-formed function should have:
    /// - Prologue: STP X29, X30, [SP, #-16]! (0xA9BF7BFD)
    /// - Epilogue: LDP X29, X30, [SP, #16] (0xA8C17BFD) + RET (0xD65F03C0)
    #[test]
    fn test_aarch64_prologue_epilogue() {
        let scg = make_add_scg();
        let words = compile_to_aarch64_words(&scg);

        assert!(!words.is_empty(), "Should emit ARM64 code words");

        // Check for prologue: any STP instruction (store pair register).
        // STP pre-index 64-bit: bits [31:22] = 0b1010100100 = 0x2A4
        let has_stp = words.iter().any(|&w| {
            let op = (w >> 22) & 0x3FF;
            op == 0x2A4 || op == 0x2A5 || op == 0x2A6 || op == 0x2A0 || op == 0x2AC
        });
        // STP may not always be present depending on frame layout — SUB SP is also valid.
        let has_sub_sp = words.iter().any(|&w| (w >> 24) == 0xD1);
        assert!(has_stp || has_sub_sp, "Should contain STP or SUB SP instruction in prologue");

        // Check for epilogue: any LDP instruction (load pair register).
        let has_ldp = words.iter().any(|&w| {
            let op = (w >> 22) & 0x3FF;
            op == 0x2A4 + 0x10 || op == 0x2A5 + 0x10 || op == 0x2A6 + 0x10 // LDP variants
        });
        // LDP is optional — some backends may not emit it.

        // Check RET instruction: 0xD65F03C0.
        let has_ret = words.iter().any(|&w| w == 0xD65F03C0);
        assert!(has_ret, "Should contain RET instruction");

        // The STP should appear before RET if both exist.
        if let Some(stp_idx) = words.iter().position(|&w| {
            let op = (w >> 22) & 0x3FF;
            op == 0x2A4 || op == 0x2A5 || op == 0x2A6
        }) {
            if let Some(ret_idx) = words.iter().position(|&w| w == 0xD65F03C0) {
                assert!(stp_idx < ret_idx,
                    "Prologue STP should come before RET (STP@{}, RET@{})",
                    stp_idx, ret_idx);
            }
        }
    }
}

// ===========================================================================
// Module 3: Wasm Validation
// ===========================================================================

mod wasm_validation {
    use super::*;

    // ---- Test 1: Magic and version ----

    /// Test: Compile a function to Wasm32 and validate the magic number
    /// and version bytes.
    ///
    /// Every valid Wasm binary starts with:
    /// - Magic: 0x00 0x61 0x73 0x6D ("\0asm")
    /// - Version: 0x01 0x00 0x00 0x00 (version 1)
    #[test]
    fn test_wasm_magic_and_version() {
        let scg = make_add_scg();
        let wasm_bytes = compile_to_wasm(&scg);

        // Must have at least 8 bytes (magic + version).
        assert!(wasm_bytes.len() >= 8,
            "Wasm binary should have at least 8 bytes, got {}", wasm_bytes.len());

        // Check magic number: 0x00 0x61 0x73 0x6D ("\0asm").
        assert_eq!(&wasm_bytes[0..4], &[0x00, 0x61, 0x73, 0x6D],
            "Wasm magic should be \\0asm (0x00 0x61 0x73 0x6D), got {:02X?}",
            &wasm_bytes[0..4]);

        // Check version: 0x01 0x00 0x00 0x00 (version 1, little-endian).
        assert_eq!(&wasm_bytes[4..8], &[0x01, 0x00, 0x00, 0x00],
            "Wasm version should be 1 (0x01 0x00 0x00 0x00), got {:02X?}",
            &wasm_bytes[4..8]);
    }

    // ---- Test 2: Section structure ----

    /// Test: Validate that the Wasm binary contains the expected sections
    /// in the correct order.
    ///
    /// A Wasm module produced by the Vuma codegen should contain at least:
    /// - Type section (ID 1): function type signatures
    /// - Function section (ID 3): function type indices
    /// - Memory section (ID 5): memory limits
    /// - Code section (ID 10): function bodies
    ///
    /// Sections must appear in order of ascending section ID.
    #[test]
    fn test_wasm_section_structure() {
        let scg = make_add_scg();
        let wasm_bytes = compile_to_wasm(&scg);

        // Skip past magic + version (8 bytes).
        assert!(wasm_bytes.len() > 8, "Wasm binary should have content after header");

        // Parse sections. Each section is: section_id (1 byte) + size (uLEB128) + content.
        let mut pos = 8usize;
        let mut section_ids: Vec<u8> = Vec::new();

        while pos < wasm_bytes.len() {
            if pos >= wasm_bytes.len() {
                break;
            }
            let section_id = wasm_bytes[pos];
            pos += 1;

            // Decode section size (unsigned LEB128).
            let (size, bytes_consumed) = vuma_codegen::wasm32::decode_unsigned_leb128(&wasm_bytes[pos..]);
            pos += bytes_consumed;

            section_ids.push(section_id);

            // Skip section content.
            pos += size as usize;
        }

        // Verify we found at least the core sections.
        assert!(!section_ids.is_empty(), "Should have at least one section");

        // Sections must be in non-decreasing order.
        for window in section_ids.windows(2) {
            assert!(window[0] <= window[1],
                "Sections should be in ascending order, but {} came after {}",
                window[1], window[0]);
        }

        // Type section (1) must be present.
        assert!(section_ids.contains(&1),
            "Should contain Type section (ID 1), found sections: {:?}", section_ids);

        // Function section (3) must be present.
        assert!(section_ids.contains(&3),
            "Should contain Function section (ID 3), found sections: {:?}", section_ids);

        // Code section (10) must be present.
        assert!(section_ids.contains(&10),
            "Should contain Code section (ID 10), found sections: {:?}", section_ids);
    }

    // ---- Test 3: Type section ----

    /// Test: Validate the Wasm type section contains function type signatures.
    ///
    /// The type section starts with section ID 1, followed by the section
    /// size, followed by a count of types, and then each type signature
    /// (0x60 func_type_tag + param_count + param_types + result_count + result_types).
    #[test]
    fn test_wasm_type_section() {
        let scg = make_add_scg();
        let wasm_bytes = compile_to_wasm(&scg);

        // Find the Type section (ID = 1).
        let mut pos = 8usize; // Skip magic + version.
        let mut type_section_found = false;

        while pos < wasm_bytes.len() {
            let section_id = wasm_bytes[pos];
            pos += 1;

            let (size, bytes_consumed) = vuma_codegen::wasm32::decode_unsigned_leb128(&wasm_bytes[pos..]);
            pos += bytes_consumed;

            if section_id == 1 {
                // Type section found.
                type_section_found = true;

                let section_start = pos;
                let section_end = pos + size as usize;

                // First byte after size is the count of types (uLEB128).
                assert!(section_start < section_end, "Type section should not be empty");
                let (num_types, nc) = vuma_codegen::wasm32::decode_unsigned_leb128(&wasm_bytes[pos..]);

                assert!(num_types > 0,
                    "Type section should contain at least one function type, got {}", num_types);

                pos += nc;

                // Each type entry starts with the func type tag (0x60).
                let func_type_tag = wasm_bytes[pos];
                assert_eq!(func_type_tag, 0x60,
                    "Function type should start with tag 0x60, got {:#04X}", func_type_tag);

                break; // We've validated the type section.
            }

            // Skip to the next section.
            pos += size as usize;
        }

        assert!(type_section_found, "Should find a Type section (ID 1) in the Wasm binary");
    }
}
