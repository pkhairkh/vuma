//! # ELF Validation Tests
//!
//! Validates that the ELF binaries produced by every native backend are
//! well-formed and could be loaded by the Linux kernel.  Each backend
//! compiles a minimal `fn add(a, b) -> i64 { a + b }` program through
//! the full SCG → IR → register-allocation → encode_program pipeline,
//! and the resulting bytes are parsed as an ELF file.
//!
//! # Test Matrix
//!
//! | # | Backend       | ELF Class | Endianness | EM_* value | Tests                       |
//! |---|---------------|-----------|------------|------------|-----------------------------|
//! | 1 | x86_64        | ELFCLASS64 | LE        | 62         | header, phdr, section       |
//! | 2 | AArch64       | ELFCLASS64 | LE        | 183        | header, phdr, section       |
//! | 3 | RISC-V 64     | ELFCLASS64 | LE        | 243        | header, phdr, section       |
//! | 4 | ARM32         | ELFCLASS32 | LE        | 40         | header, phdr, section       |
//! | 5 | MIPS64        | ELFCLASS64 | BE        | 8          | header, phdr, section       |
//! | 6 | PPC64         | ELFCLASS64 | BE        | 21         | header, phdr, section       |
//! | 7 | LoongArch64   | ELFCLASS64 | LE        | 258        | header, phdr, section       |

use vuma_codegen::{
    backend::{AllocatedProgram, Backend, Endianness, OutputFormat},
    ir::BinOpKind,
    scg_to_ir::{
        ComputationNode, IRBuilder, Scg, ScgExpr, ScgFunction, ScgNode, ScgParam, ScgStatement,
        ScgType,
    },
    AArch64Backend, Arm32Backend, LoongArch64Backend, Mips64Backend, PPC64Backend, RiscV64Backend,
    X86_64Backend,
};

// ===========================================================================
// ELF Constants
// ===========================================================================

const ELFMAG: [u8; 4] = [0x7f, b'E', b'L', b'F'];
const ELFCLASS32: u8 = 1;
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1; // Little-endian
const ELFDATA2MSB: u8 = 2; // Big-endian
const ET_EXEC: u16 = 2;
const PT_LOAD: u32 = 1;

// EM_* machine types
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const EM_RISCV: u16 = 243;
const EM_ARM: u16 = 40;
const EM_MIPS: u16 = 8;
const EM_PPC64: u16 = 21;
const EM_LOONGARCH: u16 = 258;

// ===========================================================================
// Parsed ELF structures
// ===========================================================================

/// Parsed ELF header (supports both 32-bit and 64-bit).
struct ElfHeader {
    ei_class: u8,
    ei_data: u8,
    e_type: u16,
    e_machine: u16,
    e_entry: u64,
    e_phoff: u64,
    e_shoff: u64,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

/// Parsed ELF program header (supports both 32-bit and 64-bit).
struct ElfPhdr {
    p_type: u32,
    p_flags: u32,
    p_offset: u64,
    p_vaddr: u64,
    p_paddr: u64,
    p_filesz: u64,
    p_memsz: u64,
    p_align: u64,
}

/// Parsed ELF section header (supports both 32-bit and 64-bit).
struct ElfShdr {
    sh_name: u32,
    sh_type: u32,
    sh_flags: u64,
    sh_addr: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: u64,
    sh_entsize: u64,
}

/// Fully parsed ELF file.
struct ElfFile {
    header: ElfHeader,
    phdrs: Vec<ElfPhdr>,
    shdrs: Vec<ElfShdr>,
}

impl ElfFile {
    /// Parse an ELF file from raw bytes.
    fn parse(bytes: &[u8]) -> Self {
        assert!(bytes.len() >= 16, "ELF too short for e_ident");
        assert_eq!(&bytes[0..4], &ELFMAG, "ELF magic mismatch");

        let ei_class = bytes[4];
        let ei_data = bytes[5];

        let is_le = ei_data == ELFDATA2LSB;
        let is_64 = ei_class == ELFCLASS64;

        // Helper closures to read multi-byte values with correct endianness.
        let read_u16 = |b: &[u8]| -> u16 {
            if is_le {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        };

        let read_u32 = |b: &[u8]| -> u32 {
            if is_le {
                u32::from_le_bytes([b[0], b[1], b[2], b[3]])
            } else {
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            }
        };

        let read_u64 = |b: &[u8]| -> u64 {
            if is_le {
                u64::from_le_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ])
            } else {
                u64::from_be_bytes([
                    b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
                ])
            }
        };

        let header = if is_64 {
            // ELF64 header is 64 bytes
            assert!(
                bytes.len() >= 64,
                "ELF64 header requires at least 64 bytes, got {}",
                bytes.len()
            );
            ElfHeader {
                ei_class,
                ei_data,
                e_type: read_u16(&bytes[16..18]),
                e_machine: read_u16(&bytes[18..20]),
                e_entry: read_u64(&bytes[24..32]),
                e_phoff: read_u64(&bytes[32..40]),
                e_shoff: read_u64(&bytes[40..48]),
                e_flags: read_u32(&bytes[48..52]),
                e_ehsize: read_u16(&bytes[52..54]),
                e_phentsize: read_u16(&bytes[54..56]),
                e_phnum: read_u16(&bytes[56..58]),
                e_shentsize: read_u16(&bytes[58..60]),
                e_shnum: read_u16(&bytes[60..62]),
                e_shstrndx: read_u16(&bytes[62..64]),
            }
        } else {
            // ELF32 header is 52 bytes
            assert!(
                bytes.len() >= 52,
                "ELF32 header requires at least 52 bytes, got {}",
                bytes.len()
            );
            ElfHeader {
                ei_class,
                ei_data,
                e_type: read_u16(&bytes[16..18]),
                e_machine: read_u16(&bytes[18..20]),
                e_entry: read_u32(&bytes[24..28]) as u64,
                e_phoff: read_u32(&bytes[28..32]) as u64,
                e_shoff: read_u32(&bytes[32..36]) as u64,
                e_flags: read_u32(&bytes[36..40]),
                e_ehsize: read_u16(&bytes[40..42]),
                e_phentsize: read_u16(&bytes[42..44]),
                e_phnum: read_u16(&bytes[44..46]),
                e_shentsize: read_u16(&bytes[46..48]),
                e_shnum: read_u16(&bytes[48..50]),
                e_shstrndx: read_u16(&bytes[50..52]),
            }
        };

        // Parse program headers
        let mut phdrs = Vec::new();
        for i in 0..header.e_phnum as usize {
            let off = header.e_phoff as usize + i * header.e_phentsize as usize;
            if is_64 {
                // ELF64 Phdr: 56 bytes
                assert!(
                    off + 56 <= bytes.len(),
                    "ELF64 program header {} at offset {} extends past end",
                    i,
                    off
                );
                phdrs.push(ElfPhdr {
                    p_type: read_u32(&bytes[off..off + 4]),
                    p_flags: read_u32(&bytes[off + 4..off + 8]),
                    p_offset: read_u64(&bytes[off + 8..off + 16]),
                    p_vaddr: read_u64(&bytes[off + 16..off + 24]),
                    p_paddr: read_u64(&bytes[off + 24..off + 32]),
                    p_filesz: read_u64(&bytes[off + 32..off + 40]),
                    p_memsz: read_u64(&bytes[off + 40..off + 48]),
                    p_align: read_u64(&bytes[off + 48..off + 56]),
                });
            } else {
                // ELF32 Phdr: 32 bytes
                // Layout: p_type(4), p_offset(4), p_vaddr(4), p_paddr(4),
                //         p_filesz(4), p_memsz(4), p_flags(4), p_align(4)
                assert!(
                    off + 32 <= bytes.len(),
                    "ELF32 program header {} at offset {} extends past end",
                    i,
                    off
                );
                phdrs.push(ElfPhdr {
                    p_type: read_u32(&bytes[off..off + 4]),
                    p_offset: read_u32(&bytes[off + 4..off + 8]) as u64,
                    p_vaddr: read_u32(&bytes[off + 8..off + 12]) as u64,
                    p_paddr: read_u32(&bytes[off + 12..off + 16]) as u64,
                    p_filesz: read_u32(&bytes[off + 16..off + 20]) as u64,
                    p_memsz: read_u32(&bytes[off + 20..off + 24]) as u64,
                    p_flags: read_u32(&bytes[off + 24..off + 28]),
                    p_align: read_u32(&bytes[off + 28..off + 32]) as u64,
                });
            }
        }

        // Parse section headers (if any exist)
        let mut shdrs = Vec::new();
        if header.e_shoff != 0 && header.e_shnum > 0 {
            for i in 0..header.e_shnum as usize {
                let off = header.e_shoff as usize + i * header.e_shentsize as usize;
                if is_64 {
                    // ELF64 Shdr: 64 bytes
                    if off + 64 > bytes.len() {
                        break;
                    }
                    shdrs.push(ElfShdr {
                        sh_name: read_u32(&bytes[off..off + 4]),
                        sh_type: read_u32(&bytes[off + 4..off + 8]),
                        sh_flags: read_u64(&bytes[off + 8..off + 16]),
                        sh_addr: read_u64(&bytes[off + 16..off + 24]),
                        sh_offset: read_u64(&bytes[off + 24..off + 32]),
                        sh_size: read_u64(&bytes[off + 32..off + 40]),
                        sh_link: read_u32(&bytes[off + 40..off + 44]),
                        sh_info: read_u32(&bytes[off + 44..off + 48]),
                        sh_addralign: read_u64(&bytes[off + 48..off + 56]),
                        sh_entsize: read_u64(&bytes[off + 56..off + 64]),
                    });
                } else {
                    // ELF32 Shdr: 40 bytes
                    if off + 40 > bytes.len() {
                        break;
                    }
                    shdrs.push(ElfShdr {
                        sh_name: read_u32(&bytes[off..off + 4]),
                        sh_type: read_u32(&bytes[off + 4..off + 8]),
                        sh_flags: read_u32(&bytes[off + 8..off + 12]) as u64,
                        sh_addr: read_u32(&bytes[off + 12..off + 16]) as u64,
                        sh_offset: read_u32(&bytes[off + 16..off + 20]) as u64,
                        sh_size: read_u32(&bytes[off + 20..off + 24]) as u64,
                        sh_link: read_u32(&bytes[off + 24..off + 28]),
                        sh_info: read_u32(&bytes[off + 28..off + 32]),
                        sh_addralign: read_u32(&bytes[off + 32..off + 36]) as u64,
                        sh_entsize: read_u32(&bytes[off + 36..off + 40]) as u64,
                    });
                }
            }
        }

        ElfFile {
            header,
            phdrs,
            shdrs,
        }
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Build a minimal codegen SCG with a single `fn main(a, b) -> i64 { a + b }`.
fn make_add_scg() -> Scg {
    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![
                ScgParam {
                    name: "a".to_string(),
                    ty: ScgType::I64,
                },
                ScgParam {
                    name: "b".to_string(),
                    ty: ScgType::I64,
                },
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

/// Compile an SCG through a given backend and return the ELF bytes.
fn compile_to_elf(scg: &Scg, backend: &dyn Backend) -> Vec<u8> {
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(scg).expect("IRBuilder should succeed");

    let mut allocated_functions = Vec::new();
    for func in &ir_program.functions {
        let allocated = backend
            .allocate_registers(func)
            .expect("register allocation should succeed");
        allocated_functions.push(allocated);
    }

    let program = AllocatedProgram {
        functions: allocated_functions,
        total_code_size: 0,
        total_data_size: 0,
    };

    backend
        .encode_program(&program)
        .expect("encode_program should succeed")
}

// ===========================================================================
// Validation helpers
// ===========================================================================

/// Validate the ELF header for a given target configuration.
fn validate_elf_header(
    elf: &ElfFile,
    expected_class: u8,
    expected_data: u8,
    expected_machine: u16,
    backend_name: &str,
) {
    // ── e_ident[EI_CLASS] ──
    assert_eq!(
        elf.header.ei_class, expected_class,
        "[{}] e_ident[EI_CLASS]: expected {}, got {}",
        backend_name, expected_class, elf.header.ei_class
    );

    // ── e_ident[EI_DATA] ──
    assert_eq!(
        elf.header.ei_data, expected_data,
        "[{}] e_ident[EI_DATA]: expected {} ({}), got {}",
        backend_name,
        expected_data,
        if expected_data == ELFDATA2LSB {
            "LE"
        } else {
            "BE"
        },
        elf.header.ei_data
    );

    // ── e_type = ET_EXEC ──
    assert_eq!(
        elf.header.e_type, ET_EXEC,
        "[{}] e_type: expected ET_EXEC (2), got {}",
        backend_name, elf.header.e_type
    );

    // ── e_machine ──
    assert_eq!(
        elf.header.e_machine, expected_machine,
        "[{}] e_machine: expected {}, got {}",
        backend_name, expected_machine, elf.header.e_machine
    );

    // ── e_entry != 0 ──
    assert_ne!(
        elf.header.e_entry, 0,
        "[{}] e_entry should be non-zero",
        backend_name
    );

    // ── e_phoff != 0 ──
    assert_ne!(
        elf.header.e_phoff, 0,
        "[{}] e_phoff should be non-zero",
        backend_name
    );

    // ── e_ehsize ──
    let expected_ehsize: u16 = if expected_class == ELFCLASS64 { 64 } else { 52 };
    assert_eq!(
        elf.header.e_ehsize, expected_ehsize,
        "[{}] e_ehsize: expected {}, got {}",
        backend_name, expected_ehsize, elf.header.e_ehsize
    );

    // ── e_phentsize ──
    let expected_phentsize: u16 = if expected_class == ELFCLASS64 { 56 } else { 32 };
    assert_eq!(
        elf.header.e_phentsize, expected_phentsize,
        "[{}] e_phentsize: expected {}, got {}",
        backend_name, expected_phentsize, elf.header.e_phentsize
    );
}

/// Validate program headers: at least one PT_LOAD segment with valid fields.
fn validate_program_headers(elf: &ElfFile, total_bytes: usize, backend_name: &str) {
    assert!(
        !elf.phdrs.is_empty(),
        "[{}] should have at least one program header",
        backend_name
    );

    // Must have at least one PT_LOAD segment
    let load_segments: Vec<&ElfPhdr> = elf
        .phdrs
        .iter()
        .filter(|p| p.p_type == PT_LOAD)
        .collect();
    assert!(
        !load_segments.is_empty(),
        "[{}] should have at least one PT_LOAD segment",
        backend_name
    );

    for (i, phdr) in load_segments.iter().enumerate() {
        // p_offset and p_filesz should be within file bounds
        assert!(
            phdr.p_offset as usize <= total_bytes,
            "[{}] PT_LOAD[{}]: p_offset ({}) exceeds file size ({})",
            backend_name,
            i,
            phdr.p_offset,
            total_bytes
        );
        assert!(
            phdr.p_offset as usize + phdr.p_filesz as usize <= total_bytes,
            "[{}] PT_LOAD[{}]: p_offset ({}) + p_filesz ({}) exceeds file size ({})",
            backend_name,
            i,
            phdr.p_offset,
            phdr.p_filesz,
            total_bytes
        );

        // p_vaddr should be reasonable (not 0)
        assert_ne!(
            phdr.p_vaddr, 0,
            "[{}] PT_LOAD[{}]: p_vaddr should be non-zero",
            backend_name, i
        );

        // p_memsz >= p_filesz (segment in memory is at least as large as in file)
        assert!(
            phdr.p_memsz >= phdr.p_filesz,
            "[{}] PT_LOAD[{}]: p_memsz ({}) should be >= p_filesz ({})",
            backend_name,
            i,
            phdr.p_memsz,
            phdr.p_filesz
        );

        // p_align should be a power of 2 (or 0/1 for no alignment)
        if phdr.p_align > 1 {
            assert_eq!(
                phdr.p_align & (phdr.p_align - 1),
                0,
                "[{}] PT_LOAD[{}]: p_align ({}) should be a power of 2",
                backend_name,
                i,
                phdr.p_align
            );
        }

        // Executable PT_LOAD segments should contain the entry point
        if phdr.p_flags & 1 != 0 {
            // PF_X
            let entry = elf.header.e_entry;
            assert!(
                entry >= phdr.p_vaddr && entry < phdr.p_vaddr + phdr.p_memsz,
                "[{}] PT_LOAD[{}] (executable): entry point ({:#x}) should be within segment [{:#x}, {:#x})",
                backend_name,
                i,
                entry,
                phdr.p_vaddr,
                phdr.p_vaddr + phdr.p_memsz
            );
        }
    }
}

/// Validate section headers (if any exist) for consistency.
fn validate_section_headers(elf: &ElfFile, total_bytes: usize, backend_name: &str) {
    if elf.shdrs.is_empty() {
        // No section headers — this is valid for minimal ELF files (e_shnum = 0)
        return;
    }

    for (i, shdr) in elf.shdrs.iter().enumerate() {
        // sh_offset + sh_size should not exceed file size for non-BSS sections
        // SHT_NOBITS = 8 (BSS-like, no file content)
        if shdr.sh_type != 8 {
            assert!(
                shdr.sh_offset as usize + shdr.sh_size as usize <= total_bytes,
                "[{}] section[{}]: sh_offset ({}) + sh_size ({}) exceeds file size ({})",
                backend_name,
                i,
                shdr.sh_offset,
                shdr.sh_size,
                total_bytes
            );
        }

        // sh_addralign should be 0 or a power of 2
        if shdr.sh_addralign > 1 {
            assert_eq!(
                shdr.sh_addralign & (shdr.sh_addralign - 1),
                0,
                "[{}] section[{}]: sh_addralign ({}) should be a power of 2",
                backend_name,
                i,
                shdr.sh_addralign
            );
        }
    }
}

/// Run all ELF validation checks for a given backend.
fn validate_elf_for_backend(
    elf_bytes: &[u8],
    expected_class: u8,
    expected_data: u8,
    expected_machine: u16,
    backend_name: &str,
) {
    let elf = ElfFile::parse(elf_bytes);

    validate_elf_header(&elf, expected_class, expected_data, expected_machine, backend_name);
    validate_program_headers(&elf, elf_bytes.len(), backend_name);
    validate_section_headers(&elf, elf_bytes.len(), backend_name);
}

// ===========================================================================
// Per-backend tests
// ===========================================================================

// ---- x86_64 ----

#[test]
fn test_elf_validation_x86_64() {
    let scg = make_add_scg();
    let backend = X86_64Backend::new();
    let elf_bytes = compile_to_elf(&scg, &backend);

    validate_elf_for_backend(
        &elf_bytes,
        ELFCLASS64,
        ELFDATA2LSB,
        EM_X86_64,
        "x86_64",
    );
}

// ---- AArch64 ----

#[test]
fn test_elf_validation_aarch64() {
    let scg = make_add_scg();
    let backend = AArch64Backend::new();
    let elf_bytes = compile_to_elf(&scg, &backend);

    validate_elf_for_backend(
        &elf_bytes,
        ELFCLASS64,
        ELFDATA2LSB,
        EM_AARCH64,
        "aarch64",
    );
}

// ---- RISC-V 64 ----

#[test]
fn test_elf_validation_riscv64() {
    let scg = make_add_scg();
    let backend = RiscV64Backend::new();
    let elf_bytes = compile_to_elf(&scg, &backend);

    validate_elf_for_backend(
        &elf_bytes,
        ELFCLASS64,
        ELFDATA2LSB,
        EM_RISCV,
        "riscv64",
    );
}

// ---- ARM32 ----

#[test]
fn test_elf_validation_arm32() {
    let scg = make_add_scg();
    let backend = Arm32Backend::new();
    let elf_bytes = compile_to_elf(&scg, &backend);

    validate_elf_for_backend(
        &elf_bytes,
        ELFCLASS32,
        ELFDATA2LSB,
        EM_ARM,
        "arm32",
    );
}

// ---- MIPS64 ----

#[test]
fn test_elf_validation_mips64() {
    let scg = make_add_scg();
    let backend = Mips64Backend::new();
    let elf_bytes = compile_to_elf(&scg, &backend);

    validate_elf_for_backend(
        &elf_bytes,
        ELFCLASS64,
        ELFDATA2MSB,
        EM_MIPS,
        "mips64",
    );
}

// ---- PPC64 ----

#[test]
fn test_elf_validation_ppc64() {
    let scg = make_add_scg();
    let backend = PPC64Backend::new();
    let elf_bytes = compile_to_elf(&scg, &backend);

    validate_elf_for_backend(
        &elf_bytes,
        ELFCLASS64,
        ELFDATA2MSB,
        EM_PPC64,
        "ppc64",
    );
}

// ---- LoongArch64 ----

#[test]
fn test_elf_validation_loongarch64() {
    let scg = make_add_scg();
    let backend = LoongArch64Backend::new();
    let elf_bytes = compile_to_elf(&scg, &backend);

    validate_elf_for_backend(
        &elf_bytes,
        ELFCLASS64,
        ELFDATA2LSB,
        EM_LOONGARCH,
        "loongarch64",
    );
}

// ===========================================================================
// Cross-backend consistency checks
// ===========================================================================

/// Verify that every native backend produces ELF magic at offset 0.
#[test]
fn test_all_backends_produce_elf_magic() {
    let scg = make_add_scg();

    let backends: Vec<(&str, Box<dyn Backend>)> = vec![
        ("x86_64", Box::new(X86_64Backend::new())),
        ("aarch64", Box::new(AArch64Backend::new())),
        ("riscv64", Box::new(RiscV64Backend::new())),
        ("arm32", Box::new(Arm32Backend::new())),
        ("mips64", Box::new(Mips64Backend::new())),
        ("ppc64", Box::new(PPC64Backend::new())),
        ("loongarch64", Box::new(LoongArch64Backend::new())),
    ];

    for (name, backend) in backends {
        let elf_bytes = compile_to_elf(&scg, &*backend);
        assert!(
            elf_bytes.len() >= 16,
            "[{}] ELF output should be at least 16 bytes, got {}",
            name,
            elf_bytes.len()
        );
        assert_eq!(
            &elf_bytes[0..4],
            &ELFMAG,
            "[{}] ELF magic bytes incorrect",
            name
        );
    }
}

/// Verify that every native backend produces ET_EXEC as the ELF type.
#[test]
fn test_all_backends_produce_et_exec() {
    let scg = make_add_scg();

    let backends: Vec<(&str, Box<dyn Backend>)> = vec![
        ("x86_64", Box::new(X86_64Backend::new())),
        ("aarch64", Box::new(AArch64Backend::new())),
        ("riscv64", Box::new(RiscV64Backend::new())),
        ("arm32", Box::new(Arm32Backend::new())),
        ("mips64", Box::new(Mips64Backend::new())),
        ("ppc64", Box::new(PPC64Backend::new())),
        ("loongarch64", Box::new(LoongArch64Backend::new())),
    ];

    for (name, backend) in backends {
        let elf_bytes = compile_to_elf(&scg, &*backend);
        let elf = ElfFile::parse(&elf_bytes);
        assert_eq!(
            elf.header.e_type, ET_EXEC,
            "[{}] e_type should be ET_EXEC (2)",
            name
        );
    }
}

/// Verify that every native backend produces at least one PT_LOAD segment.
#[test]
fn test_all_backends_have_pt_load() {
    let scg = make_add_scg();

    let backends: Vec<(&str, Box<dyn Backend>)> = vec![
        ("x86_64", Box::new(X86_64Backend::new())),
        ("aarch64", Box::new(AArch64Backend::new())),
        ("riscv64", Box::new(RiscV64Backend::new())),
        ("arm32", Box::new(Arm32Backend::new())),
        ("mips64", Box::new(Mips64Backend::new())),
        ("ppc64", Box::new(PPC64Backend::new())),
        ("loongarch64", Box::new(LoongArch64Backend::new())),
    ];

    for (name, backend) in backends {
        let elf_bytes = compile_to_elf(&scg, &*backend);
        let elf = ElfFile::parse(&elf_bytes);
        let has_load = elf.phdrs.iter().any(|p| p.p_type == PT_LOAD);
        assert!(
            has_load,
            "[{}] should have at least one PT_LOAD segment",
            name
        );
    }
}

/// Verify that every native backend's TargetInfo matches the ELF it produces.
#[test]
fn test_all_backends_target_info_matches_elf() {
    let scg = make_add_scg();

    let backends: Vec<(&str, Box<dyn Backend>)> = vec![
        ("x86_64", Box::new(X86_64Backend::new())),
        ("aarch64", Box::new(AArch64Backend::new())),
        ("riscv64", Box::new(RiscV64Backend::new())),
        ("arm32", Box::new(Arm32Backend::new())),
        ("mips64", Box::new(Mips64Backend::new())),
        ("ppc64", Box::new(PPC64Backend::new())),
        ("loongarch64", Box::new(LoongArch64Backend::new())),
    ];

    for (name, backend) in backends {
        let info = backend.target_info();
        let elf_bytes = compile_to_elf(&scg, &*backend);
        let elf = ElfFile::parse(&elf_bytes);

        // e_machine should match TargetInfo::elf_machine_type()
        assert_eq!(
            elf.header.e_machine,
            info.elf_machine_type(),
            "[{}] e_machine ({}) should match TargetInfo::elf_machine_type ({})",
            name,
            elf.header.e_machine,
            info.elf_machine_type()
        );

        // EI_CLASS should match OutputFormat
        let expected_class = match info.output_format() {
            OutputFormat::Elf64 => ELFCLASS64,
            OutputFormat::Elf32 => ELFCLASS32,
            _ => panic!("[{}] unexpected output format", name),
        };
        assert_eq!(
            elf.header.ei_class, expected_class,
            "[{}] EI_CLASS should match output format",
            name
        );

        // EI_DATA should match endianness
        let expected_data = match info.endianness() {
            Endianness::Little => ELFDATA2LSB,
            Endianness::Big | Endianness::Bi => ELFDATA2MSB,
        };
        assert_eq!(
            elf.header.ei_data, expected_data,
            "[{}] EI_DATA should match endianness",
            name
        );
    }
}

/// Verify that every native backend's entry point is non-zero and within
/// a PT_LOAD segment.
#[test]
fn test_all_backends_entry_point_valid() {
    let scg = make_add_scg();

    let backends: Vec<(&str, Box<dyn Backend>)> = vec![
        ("x86_64", Box::new(X86_64Backend::new())),
        ("aarch64", Box::new(AArch64Backend::new())),
        ("riscv64", Box::new(RiscV64Backend::new())),
        ("arm32", Box::new(Arm32Backend::new())),
        ("mips64", Box::new(Mips64Backend::new())),
        ("ppc64", Box::new(PPC64Backend::new())),
        ("loongarch64", Box::new(LoongArch64Backend::new())),
    ];

    for (name, backend) in backends {
        let elf_bytes = compile_to_elf(&scg, &*backend);
        let elf = ElfFile::parse(&elf_bytes);

        assert_ne!(
            elf.header.e_entry, 0,
            "[{}] entry point should be non-zero",
            name
        );

        // Entry point must fall within some PT_LOAD segment
        let entry_in_segment = elf
            .phdrs
            .iter()
            .filter(|p| p.p_type == PT_LOAD)
            .any(|p| {
                elf.header.e_entry >= p.p_vaddr
                    && elf.header.e_entry < p.p_vaddr + p.p_memsz
            });
        assert!(
            entry_in_segment,
            "[{}] entry point ({:#x}) must be within a PT_LOAD segment",
            name,
            elf.header.e_entry
        );
    }
}
