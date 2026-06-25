//! # DWARF Debug Info Generation
//!
//! Produces DWARF version 4 debug information sections for all VUMA
//! backends.  The emitted sections can be appended to an ELF binary so that
//! tools such as `readelf`, `objdump`, and `gdb` can decode the program's
//! structure, function boundaries, local variables, source-line mapping,
//! and call frame information for stack unwinding.
//!
//! ## Sections Generated
//!
//! | Section          | Contents                                          |
//! |------------------|---------------------------------------------------|
//! | `.debug_abbrev`  | Abbreviation tables (tag + attribute encodings)   |
//! | `.debug_info`    | Compilation unit DIEs (subprograms, variables)    |
//! | `.debug_line`    | Line-number program (DWARF standard opcodes)      |
//! | `.debug_frame`   | Call frame information (CIE + FDE entries)        |
//!
//! ## Multi-Backend Support
//!
//! The `DwarfBuilder` is parameterised by address size to support all
//! eight VUMA backends:
//!
//! | Backend      | Address Size | Min Inst Length |
//! |--------------|-------------|-----------------|
//! | x86_64       | 8 bytes     | 1               |
//! | AArch64      | 8 bytes     | 4               |
//! | RISC-V 64    | 8 bytes     | 2               |
//! | ARM32        | 4 bytes     | 2               |
//! | MIPS64       | 8 bytes     | 4               |
//! | PPC64        | 8 bytes     | 4               |
//! | LoongArch64  | 8 bytes     | 4               |
//! | Wasm32       | 4 bytes     | 1               |
//!
//! ## DWARF v4 Encoding
//!
//! - Abbreviation codes: `DW_TAG_COMPILE_UNIT`, `DW_TAG_SUBPROGRAM`,
//!   `DW_TAG_VARIABLE`
//! - Attributes: `DW_AT_NAME`, `DW_AT_LOW_PC`, `DW_AT_HIGH_PC`,
//!   `DW_AT_TYPE`, `DW_AT_LOCATION`
//! - Forms: `DW_FORM_STRING`, `DW_FORM_ADDR`, `DW_FORM_DATA4`,
//!   `DW_FORM_EXPRLOC`
//! - Line-number opcodes: `DW_LNS_COPY`, `DW_LNS_ADVANCE_PC`,
//!   `DW_LNS_ADVANCE_LINE`, `DW_LNS_SET_FILE`, `DW_LNE_END_SEQUENCE`
//! - Call frame: CIE with DW_CFA_def_cfa / DW_CFA_offset,
//!   FDE per function with initial_location and address_range

// ---------------------------------------------------------------------------
// DWARF5 Constants
// ---------------------------------------------------------------------------

/// DWARF version number (4 — widely supported by GDB, LLDB, readelf).
const DWARF_VERSION: u16 = 4;

/// Default address size for 64-bit targets.
const ADDRESS_SIZE_64: u8 = 8;
/// Address size for 32-bit targets.
const ADDRESS_SIZE_32: u8 = 4;

// -- Tags --

/// Tag for a compilation unit.
const DW_TAG_COMPILE_UNIT: u16 = 0x11;
/// Tag for a subprogram (function).
const DW_TAG_SUBPROGRAM: u16 = 0x2E;
/// Tag for a variable.
const DW_TAG_VARIABLE: u16 = 0x34;

// -- Attributes --

/// Name attribute.
const DW_AT_NAME: u16 = 0x03;
/// Low PC (start address) attribute.
const DW_AT_LOW_PC: u16 = 0x11;
/// High PC (end address) attribute.
const DW_AT_HIGH_PC: u16 = 0x12;
/// Language attribute.
const DW_AT_LANGUAGE: u16 = 0x13;
/// Producer attribute.
const DW_AT_PRODUCER: u16 = 0x25;
/// Type attribute.
const DW_AT_TYPE: u16 = 0x49;
/// Location attribute.
const DW_AT_LOCATION: u16 = 0x02;
/// Statement list attribute (offset into .debug_line).
const DW_AT_STMT_LIST: u16 = 0x10;

// -- Forms --

/// Null form (terminator).
const DW_FORM_NONE: u16 = 0x00;
/// Inline null-terminated string.
const DW_FORM_STRING: u16 = 0x08;
/// Address (8 bytes for 64-bit).
const DW_FORM_ADDR: u16 = 0x01;
/// 4-byte unsigned data.
const DW_FORM_DATA4: u16 = 0x06;
/// 2-byte unsigned data.
const DW_FORM_DATA2: u16 = 0x05;
/// Expression location (ULEB128 length + bytes).
const DW_FORM_EXPRLOC: u16 = 0x18;
/// 4-byte offset into .debug_line.
const DW_FORM_SEC_OFFSET: u16 = 0x17;

// -- Children --

/// DIE has children.
const DW_CHILDREN_YES: u8 = 0x01;
/// DIE has no children.
const DW_CHILDREN_NO: u8 = 0x00;

// -- Line Number Standard Opcodes --

/// Copy current row to matrix.
const DW_LNS_COPY: u8 = 0x01;
/// Advance PC by operation advance * minimum_instruction_length.
const DW_LNS_ADVANCE_PC: u8 = 0x02;
/// Advance line by signed LEB128 increment.
const DW_LNS_ADVANCE_LINE: u8 = 0x03;
/// Set the file register to the value in the unsigned LEB128 operand.
const DW_LNS_SET_FILE: u8 = 0x04;

// -- Line Number Extended Opcodes --

/// End sequence of line number entries.
const DW_LNE_END_SEQUENCE: u8 = 0x01;

// -- Language codes --

/// VUMA user language code (reserved range 0x8001–0xFFFF).
const DW_LANG_VUMA: u16 = 0x8001;

// -- Call Frame Information (CFI) constants --

/// CFI opcode: advance location by 1 * code_alignment_factor.
#[allow(dead_code)]
const DW_CFA_ADVANCE_LOC: u8 = 0x40;
/// CFI opcode: define CFA rule (register + offset).
const DW_CFA_DEF_CFA: u8 = 0x0C;
/// CFI opcode: offset — register saved at CFA + offset.
const DW_CFA_OFFSET: u8 = 0x80;
/// CFI opcode: restore register to initial state.
#[allow(dead_code)]
const DW_CFA_RESTORE: u8 = 0xC0;
/// CFI extended opcode: def_cfa_offset — change CFA offset only.
#[allow(dead_code)]
const DW_CFA_DEF_CFA_OFFSET: u8 = 0x0E;

/// CIE augmentation string (empty — no augmentation).
#[allow(dead_code)]
const CIE_AUGMENTATION: &[u8] = b"";

/// DWARF CIE identifier (0xFFFFFFFF for 64-bit DWARF, 0xFFFFFFFF for 32-bit).
const CIE_ID_32: u32 = 0xFFFFFFFF;
/// DWARF CIE identifier for 64-bit DWARF format.
#[allow(dead_code)]
const CIE_ID_64: u64 = 0xFFFFFFFFFFFFFFFF;

/// Frame version for DWARF v4.
const DW_CIE_VERSION: u8 = 1;

// ---------------------------------------------------------------------------
// Debug Info Data Structures
// ---------------------------------------------------------------------------

/// A recorded subprogram (function boundary).
#[derive(Debug, Clone)]
pub struct Subprogram {
    /// Function name.
    pub name: String,
    /// Byte offset (relative to text section start) where the function begins.
    pub start_offset: u64,
    /// Byte offset (relative to text section start) where the function ends.
    pub end_offset: u64,
}

/// A recorded local variable.
#[derive(Debug, Clone)]
pub struct Variable {
    /// Variable name.
    pub name: String,
    /// DWARF type name (e.g. "int", "i64").
    pub type_name: String,
    /// Stack offset for the variable (used in the DW_OP_fbreg expression).
    pub offset: i32,
    /// Register number the variable is in (AArch64 register encoding, 0–31).
    pub register: u8,
}

/// A line-number table entry.
#[derive(Debug, Clone)]
pub struct LineEntry {
    /// Byte offset within the text section.
    pub offset: u64,
    /// Source file index (1-based in the file table).
    pub file: u32,
    /// Line number (1-based).
    pub line: u32,
    /// Column number (0 = no column info).
    pub column: u32,
}

/// A register saved in the prologue (used for `.debug_frame` generation).
#[derive(Debug, Clone)]
pub struct SavedRegister {
    /// DWARF register number (architecture-specific encoding).
    pub reg: u8,
    /// Offset from the CFA where the register is saved.
    pub cfa_offset: i32,
}

/// A Frame Description Entry (FDE) for a single function.
///
/// Each FDE describes the call frame for one function, referencing the
/// Common Information Entry (CIE) that shares the prologue pattern.
#[derive(Debug, Clone)]
pub struct FrameDescriptorEntry {
    /// Byte offset of this function within the text section.
    pub initial_location: u64,
    /// Size of the function in bytes.
    pub address_range: u64,
}

/// A Common Information Entry (CIE) shared across all FDEs.
///
/// The CIE describes the prologue pattern that is common to all functions
/// in the compilation unit: which register is the frame pointer, which
/// registers are saved, and the stack pointer offset.
#[derive(Debug, Clone)]
pub struct CommonInformationEntry {
    /// DWARF register number for the stack pointer (CFA base register).
    /// e.g. 31 for AArch64 SP (x31), 7 for x86_64 RSP.
    pub cfa_reg: u8,
    /// Offset of the CFA from the stack pointer register.
    /// e.g. 0 means CFA = SP, 16 means CFA = SP + 16.
    pub cfa_offset: u32,
    /// Registers saved in the prologue and their offsets from CFA.
    /// e.g. SavedRegister { reg: 30, cfa_offset: -8 } for AArch64 LR (x30).
    pub saved_regs: Vec<SavedRegister>,
    /// Code alignment factor (minimum instruction length in bytes).
    pub code_alignment_factor: u8,
    /// Data alignment factor (byte size of a stack slot, typically -4 or -8).
    pub data_alignment_factor: i8,
    /// Return address register number (e.g. 30 for AArch64 LR).
    pub return_address_reg: u8,
}

// ---------------------------------------------------------------------------
// DwarfBuilder
// ---------------------------------------------------------------------------

/// Accumulates debug information during codegen and emits DWARF v4 sections.
///
/// The builder is parameterised by address size (4 for ARM32/Wasm32,
/// 8 for all 64-bit targets) and minimum instruction length to correctly
/// generate DWARF sections for each supported backend.
///
/// # Example
///
/// ```
/// use vuma_codegen::dwarf::DwarfBuilder;
///
/// let mut db = DwarfBuilder::new(); // defaults to 64-bit, min_inst_length=4
/// db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
/// db.add_subprogram("main", 0, 64);
/// db.add_variable("x", "i64", -8, 0);
/// db.add_line_entry(0, 1, 1, 0);
/// db.add_line_entry(16, 1, 2, 4);
/// db.set_cie_aarch64();
/// db.add_fde("main", 0, 64);
/// let sections = db.emit_debug_sections();
/// assert!(!sections.debug_abbrev.is_empty());
/// assert!(!sections.debug_info.is_empty());
/// assert!(!sections.debug_line.is_empty());
/// assert!(!sections.debug_frame.is_empty());
/// ```
///
/// For 32-bit targets:
///
/// ```
/// use vuma_codegen::dwarf::DwarfBuilder;
///
/// let mut db = DwarfBuilder::new_32bit(2); // ARM32: 4-byte addr, min_inst=2
/// db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
/// db.add_subprogram("main", 0, 32);
/// db.set_cie_arm32();
/// db.add_fde("main", 0, 32);
/// let sections = db.emit_debug_sections();
/// assert!(!sections.debug_info.is_empty());
/// assert!(!sections.debug_frame.is_empty());
/// ```
#[derive(Debug, Clone)]
pub struct DwarfBuilder {
    /// Source file path for the compilation unit.
    source_file: String,
    /// Producer string (compiler name + version).
    producer: String,
    /// Recorded subprograms.
    subprograms: Vec<Subprogram>,
    /// Recorded local variables.
    variables: Vec<Variable>,
    /// Recorded line-number entries.
    line_entries: Vec<LineEntry>,
    /// Common Information Entry for call frame info.
    cie: Option<CommonInformationEntry>,
    /// Frame Description Entries (one per function).
    fdes: Vec<FrameDescriptorEntry>,
    /// Address size in bytes: 4 for ARM32/Wasm32, 8 for all 64-bit targets.
    address_size: u8,
    /// Minimum instruction length in bytes for the target.
    min_inst_length: u8,
}

/// The four DWARF debug sections emitted by [`DwarfBuilder::emit_debug_sections`].
#[derive(Debug, Clone)]
pub struct DebugSections {
    /// `.debug_abbrev` — abbreviation table.
    pub debug_abbrev: Vec<u8>,
    /// `.debug_info` — compilation unit DIEs.
    pub debug_info: Vec<u8>,
    /// `.debug_line` — line number program.
    pub debug_line: Vec<u8>,
    /// `.debug_frame` — call frame information (CIE + FDEs).
    pub debug_frame: Vec<u8>,
}

impl DwarfBuilder {
    /// Create a new `DwarfBuilder` for 64-bit targets (default).
    ///
    /// Uses 8-byte address size and 4-byte minimum instruction length
    /// (suitable for AArch64, MIPS64, PPC64, LoongArch64).
    pub fn new() -> Self {
        Self {
            source_file: String::new(),
            producer: String::new(),
            subprograms: Vec::new(),
            variables: Vec::new(),
            line_entries: Vec::new(),
            cie: None,
            fdes: Vec::new(),
            address_size: ADDRESS_SIZE_64,
            min_inst_length: 4,
        }
    }

    /// Create a new `DwarfBuilder` for 32-bit targets.
    ///
    /// Uses 4-byte address size and the given minimum instruction length.
    ///
    /// # Arguments
    /// * `min_inst_length` - Minimum instruction length for the target
    ///   (2 for ARM32, 1 for Wasm32)
    pub fn new_32bit(min_inst_length: u8) -> Self {
        Self {
            source_file: String::new(),
            producer: String::new(),
            subprograms: Vec::new(),
            variables: Vec::new(),
            line_entries: Vec::new(),
            cie: None,
            fdes: Vec::new(),
            address_size: ADDRESS_SIZE_32,
            min_inst_length,
        }
    }

    /// Create a `DwarfBuilder` with explicit address size and min instruction length.
    ///
    /// This is the most flexible constructor, allowing any combination
    /// of address size and instruction length.
    pub fn with_config(address_size: u8, min_inst_length: u8) -> Self {
        Self {
            source_file: String::new(),
            producer: String::new(),
            subprograms: Vec::new(),
            variables: Vec::new(),
            line_entries: Vec::new(),
            cie: None,
            fdes: Vec::new(),
            address_size,
            min_inst_length,
        }
    }

    /// Create a `DwarfBuilder` configured for a specific backend.
    ///
    /// Selects the correct address size and minimum instruction length
    /// for each supported ISA:
    ///
    /// | Backend      | address_size | min_inst_length |
    /// |--------------|-------------|-----------------|
    /// | x86_64       | 8           | 1               |
    /// | AArch64      | 8           | 4               |
    /// | RISC-V 64    | 8           | 2               |
    /// | ARM32        | 4           | 2               |
    /// | MIPS64       | 8           | 4               |
    /// | PPC64        | 8           | 4               |
    /// | LoongArch64  | 8           | 4               |
    /// | Wasm32       | 4           | 1               |
    pub fn for_backend(backend: crate::backend::BackendKind) -> Self {
        use crate::backend::BackendKind;
        let (addr_size, min_inst) = match backend {
            BackendKind::X86_64       => (8, 1),
            BackendKind::AArch64      => (8, 4),
            BackendKind::RiscV64      => (8, 2),
            BackendKind::RiscV32      => (4, 2),
            BackendKind::Arm32        => (4, 2),
            BackendKind::Mips64       => (8, 4),
            BackendKind::PowerPC64    => (8, 4),
            BackendKind::LoongArch64  => (8, 4),
            BackendKind::Wasm32       => (4, 1),
        };
        Self::with_config(addr_size, min_inst)
    }

    /// Returns the address size configured for this builder.
    pub fn address_size(&self) -> u8 {
        self.address_size
    }

    /// Returns the minimum instruction length configured for this builder.
    pub fn min_inst_length(&self) -> u8 {
        self.min_inst_length
    }

    /// Returns a reference to the CIE (Common Information Entry), if set.
    ///
    /// Returns `None` if no CIE has been configured (via `set_cie_aarch64`,
    /// `set_cie_for_backend`, etc.).
    pub fn cie(&self) -> Option<&CommonInformationEntry> {
        self.cie.as_ref()
    }

    /// Record the top-level compilation unit.
    pub fn add_compile_unit(&mut self, source_file: &str, producer: &str) {
        self.source_file = source_file.to_string();
        self.producer = producer.to_string();
    }

    /// Record a subprogram (function boundary).
    ///
    /// `start_offset` and `end_offset` are byte offsets relative to the
    /// beginning of the text section.
    pub fn add_subprogram(&mut self, name: &str, start_offset: u64, end_offset: u64) {
        self.subprograms.push(Subprogram {
            name: name.to_string(),
            start_offset,
            end_offset,
        });
    }

    /// Record a local variable.
    ///
    /// `offset` is the stack offset (negative for below the frame pointer).
    /// `register` is the AArch64 register encoding number (0–31).
    pub fn add_variable(&mut self, name: &str, type_name: &str, offset: i32, register: u8) {
        self.variables.push(Variable {
            name: name.to_string(),
            type_name: type_name.to_string(),
            offset,
            register,
        });
    }

    /// Record a line-number table entry.
    ///
    /// `offset` is the byte offset within the text section.
    /// `file` is a 1-based file index.
    /// `line` is a 1-based line number.
    /// `column` is a 0-based column number (0 = whole line).
    pub fn add_line_entry(&mut self, offset: u64, file: u32, line: u32, column: u32) {
        self.line_entries.push(LineEntry {
            offset,
            file,
            line,
            column,
        });
    }

    /// Set the CIE for AArch64 (ARM64) targets.
    ///
    /// AArch64 calling convention: SP is register 31, LR is register 30,
    /// FP is register 29.  The prologue saves LR and FP on the stack.
    pub fn set_cie_aarch64(&mut self) {
        self.cie = Some(CommonInformationEntry {
            cfa_reg: 31,       // SP (x31)
            cfa_offset: 0,    // CFA = SP at function entry
            saved_regs: vec![
                SavedRegister { reg: 29, cfa_offset: -16 }, // FP (x29) at CFA-16
                SavedRegister { reg: 30, cfa_offset: -8 },  // LR (x30) at CFA-8
            ],
            code_alignment_factor: 4, // 4-byte instructions
            data_alignment_factor: -8, // 8-byte stack slots, negative for grows-down
            return_address_reg: 30,   // LR (x30)
        });
    }

    /// Set the CIE for x86_64 targets.
    ///
    /// x86_64 calling convention: RSP is register 7, RBP is register 6.
    /// The prologue pushes RBP and saves RSP-based CFA.
    pub fn set_cie_x86_64(&mut self) {
        self.cie = Some(CommonInformationEntry {
            cfa_reg: 7,        // RSP
            cfa_offset: 8,    // CFA = RSP + 8 (return address on stack)
            saved_regs: vec![
                SavedRegister { reg: 6, cfa_offset: -16 }, // RBP at CFA-16
            ],
            code_alignment_factor: 1, // variable-length instructions
            data_alignment_factor: -8,
            return_address_reg: 16,   // RIP (return address)
        });
    }

    /// Set the CIE for RISC-V 64 targets.
    ///
    /// RISC-V calling convention: SP is register 2, RA is register 1,
    /// FP (s0) is register 8.  The prologue saves RA and s0 on the stack.
    pub fn set_cie_riscv64(&mut self) {
        self.cie = Some(CommonInformationEntry {
            cfa_reg: 2,        // SP (x2)
            cfa_offset: 0,    // CFA = SP at function entry
            saved_regs: vec![
                SavedRegister { reg: 1, cfa_offset: -8 },  // RA (x1) at CFA-8
                SavedRegister { reg: 8, cfa_offset: -16 }, // s0/fp (x8) at CFA-16
            ],
            code_alignment_factor: 2, // 2-byte minimum instruction length
            data_alignment_factor: -8,
            return_address_reg: 1,    // RA (x1)
        });
    }

    /// Set the CIE for ARM32 targets.
    ///
    /// ARM32 calling convention: SP is register 13, LR is register 14,
    /// FP is register 11.  The prologue pushes LR and FP.
    pub fn set_cie_arm32(&mut self) {
        self.cie = Some(CommonInformationEntry {
            cfa_reg: 13,       // SP (r13)
            cfa_offset: 0,    // CFA = SP at function entry
            saved_regs: vec![
                SavedRegister { reg: 11, cfa_offset: -8 }, // FP (r11) at CFA-8
                SavedRegister { reg: 14, cfa_offset: -4 }, // LR (r14) at CFA-4
            ],
            code_alignment_factor: 2, // 2-byte instructions (Thumb) or 4 (ARM)
            data_alignment_factor: -4, // 4-byte stack slots
            return_address_reg: 14,   // LR (r14)
        });
    }

    /// Set the CIE for MIPS64 targets.
    pub fn set_cie_mips64(&mut self) {
        self.cie = Some(CommonInformationEntry {
            cfa_reg: 29,       // SP ($sp)
            cfa_offset: 0,
            saved_regs: vec![
                SavedRegister { reg: 31, cfa_offset: -8 }, // RA ($ra) at CFA-8
                SavedRegister { reg: 30, cfa_offset: -16 }, // FP ($fp) at CFA-16
            ],
            code_alignment_factor: 4,
            data_alignment_factor: -8,
            return_address_reg: 31, // RA ($ra)
        });
    }

    /// Set the CIE for PPC64 targets.
    pub fn set_cie_ppc64(&mut self) {
        self.cie = Some(CommonInformationEntry {
            cfa_reg: 1,        // R1 (SP)
            cfa_offset: 0,
            saved_regs: vec![
                SavedRegister { reg: 65, cfa_offset: -8 }, // LR (saved in LR save word)
            ],
            code_alignment_factor: 4,
            data_alignment_factor: -8,
            return_address_reg: 65, // LR
        });
    }

    /// Set the CIE for LoongArch64 targets.
    pub fn set_cie_loongarch64(&mut self) {
        self.cie = Some(CommonInformationEntry {
            cfa_reg: 3,        // $sp (r3)
            cfa_offset: 0,
            saved_regs: vec![
                SavedRegister { reg: 1, cfa_offset: -8 },  // $ra (r1) at CFA-8
                SavedRegister { reg: 22, cfa_offset: -16 }, // $fp (r22) at CFA-16
            ],
            code_alignment_factor: 4,
            data_alignment_factor: -8,
            return_address_reg: 1, // $ra (r1)
        });
    }

    /// Set the CIE for a specific backend using the `BackendKind` enum.
    ///
    /// Automatically selects the correct CIE configuration based on the
    /// target architecture.
    pub fn set_cie_for_backend(&mut self, backend: crate::backend::BackendKind) {
        use crate::backend::BackendKind;
        match backend {
            BackendKind::AArch64 => self.set_cie_aarch64(),
            BackendKind::X86_64 => self.set_cie_x86_64(),
            BackendKind::RiscV64 => self.set_cie_riscv64(),
            BackendKind::Arm32 => self.set_cie_arm32(),
            BackendKind::Mips64 => self.set_cie_mips64(),
            BackendKind::PowerPC64 => self.set_cie_ppc64(),
            BackendKind::RiscV32 => self.set_cie_riscv64(),
            BackendKind::RiscV32 => self.set_cie_riscv64(),
            BackendKind::LoongArch64 => self.set_cie_loongarch64(),
            BackendKind::Wasm32 => {
                // Wasm32 doesn't use .debug_frame — stack unwinding is
                // handled by the Wasm runtime. Set a minimal CIE.
                self.cie = Some(CommonInformationEntry {
                    cfa_reg: 0,
                    cfa_offset: 0,
                    saved_regs: vec![],
                    code_alignment_factor: 1,
                    data_alignment_factor: -4,
                    return_address_reg: 0,
                });
            }
        }
    }

    /// Set a custom CIE.
    pub fn set_cie(&mut self, cie: CommonInformationEntry) {
        self.cie = Some(cie);
    }

    /// Record a Frame Description Entry (FDE) for a function.
    ///
    /// `initial_location` is the byte offset of the function within the
    /// text section. `address_range` is the size of the function in bytes.
    pub fn add_fde(&mut self, _name: &str, initial_location: u64, address_range: u64) {
        self.fdes.push(FrameDescriptorEntry {
            initial_location,
            address_range,
        });
    }

    /// Emit all four DWARF debug sections.
    ///
    /// Returns a [`DebugSections`] containing `.debug_abbrev`, `.debug_info`,
    /// `.debug_line`, and `.debug_frame`.
    pub fn emit_debug_sections(&self) -> DebugSections {
        let debug_abbrev = self.emit_debug_abbrev();
        let debug_line = self.emit_debug_line();
        let debug_info = self.emit_debug_info(&debug_abbrev, &debug_line);
        let debug_frame = self.emit_debug_frame();
        DebugSections {
            debug_abbrev,
            debug_info,
            debug_line,
            debug_frame,
        }
    }

    // -----------------------------------------------------------------------
    // .debug_abbrev
    // -----------------------------------------------------------------------

    /// Emit the `.debug_abbrev` section.
    ///
    /// Contains three abbreviation entries:
    /// - Abbrev 1: `DW_TAG_COMPILE_UNIT` (has children)
    ///   - `DW_AT_NAME` / `DW_FORM_STRING`
    ///   - `DW_AT_LANGUAGE` / `DW_FORM_DATA2`
    ///   - `DW_AT_PRODUCER` / `DW_FORM_STRING`
    ///   - `DW_AT_LOW_PC` / `DW_FORM_ADDR`
    ///   - `DW_AT_HIGH_PC` / `DW_FORM_DATA4`
    ///   - `DW_AT_STMT_LIST` / `DW_FORM_SEC_OFFSET`
    ///
    /// - Abbrev 2: `DW_TAG_SUBPROGRAM` (has children)
    ///   - `DW_AT_NAME` / `DW_FORM_STRING`
    ///   - `DW_AT_LOW_PC` / `DW_FORM_ADDR`
    ///   - `DW_AT_HIGH_PC` / `DW_FORM_DATA4`
    ///
    /// - Abbrev 3: `DW_TAG_VARIABLE` (no children)
    ///   - `DW_AT_NAME` / `DW_FORM_STRING`
    ///   - `DW_AT_TYPE` / `DW_FORM_STRING`
    ///   - `DW_AT_LOCATION` / `DW_FORM_EXPRLOC`
    fn emit_debug_abbrev(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // Abbrev 1: DW_TAG_COMPILE_UNIT
        encode_uleb128(&mut buf, 1); // abbreviation code
        encode_uleb128(&mut buf, DW_TAG_COMPILE_UNIT as u64);
        buf.push(DW_CHILDREN_YES);
        // DW_AT_NAME, DW_FORM_STRING
        encode_uleb128(&mut buf, DW_AT_NAME as u64);
        encode_uleb128(&mut buf, DW_FORM_STRING as u64);
        // DW_AT_LANGUAGE, DW_FORM_DATA2
        encode_uleb128(&mut buf, DW_AT_LANGUAGE as u64);
        encode_uleb128(&mut buf, DW_FORM_DATA2 as u64);
        // DW_AT_PRODUCER, DW_FORM_STRING
        encode_uleb128(&mut buf, DW_AT_PRODUCER as u64);
        encode_uleb128(&mut buf, DW_FORM_STRING as u64);
        // DW_AT_LOW_PC, DW_FORM_ADDR
        encode_uleb128(&mut buf, DW_AT_LOW_PC as u64);
        encode_uleb128(&mut buf, DW_FORM_ADDR as u64);
        // DW_AT_HIGH_PC, DW_FORM_DATA4
        encode_uleb128(&mut buf, DW_AT_HIGH_PC as u64);
        encode_uleb128(&mut buf, DW_FORM_DATA4 as u64);
        // DW_AT_STMT_LIST, DW_FORM_SEC_OFFSET
        encode_uleb128(&mut buf, DW_AT_STMT_LIST as u64);
        encode_uleb128(&mut buf, DW_FORM_SEC_OFFSET as u64);
        // Terminator
        encode_uleb128(&mut buf, DW_FORM_NONE as u64);
        encode_uleb128(&mut buf, DW_FORM_NONE as u64);

        // Abbrev 2: DW_TAG_SUBPROGRAM
        encode_uleb128(&mut buf, 2);
        encode_uleb128(&mut buf, DW_TAG_SUBPROGRAM as u64);
        buf.push(DW_CHILDREN_YES);
        // DW_AT_NAME, DW_FORM_STRING
        encode_uleb128(&mut buf, DW_AT_NAME as u64);
        encode_uleb128(&mut buf, DW_FORM_STRING as u64);
        // DW_AT_LOW_PC, DW_FORM_ADDR
        encode_uleb128(&mut buf, DW_AT_LOW_PC as u64);
        encode_uleb128(&mut buf, DW_FORM_ADDR as u64);
        // DW_AT_HIGH_PC, DW_FORM_DATA4
        encode_uleb128(&mut buf, DW_AT_HIGH_PC as u64);
        encode_uleb128(&mut buf, DW_FORM_DATA4 as u64);
        // Terminator
        encode_uleb128(&mut buf, DW_FORM_NONE as u64);
        encode_uleb128(&mut buf, DW_FORM_NONE as u64);

        // Abbrev 3: DW_TAG_VARIABLE
        encode_uleb128(&mut buf, 3);
        encode_uleb128(&mut buf, DW_TAG_VARIABLE as u64);
        buf.push(DW_CHILDREN_NO);
        // DW_AT_NAME, DW_FORM_STRING
        encode_uleb128(&mut buf, DW_AT_NAME as u64);
        encode_uleb128(&mut buf, DW_FORM_STRING as u64);
        // DW_AT_TYPE, DW_FORM_STRING (simplified: type name as string)
        encode_uleb128(&mut buf, DW_AT_TYPE as u64);
        encode_uleb128(&mut buf, DW_FORM_STRING as u64);
        // DW_AT_LOCATION, DW_FORM_EXPRLOC
        encode_uleb128(&mut buf, DW_AT_LOCATION as u64);
        encode_uleb128(&mut buf, DW_FORM_EXPRLOC as u64);
        // Terminator
        encode_uleb128(&mut buf, DW_FORM_NONE as u64);
        encode_uleb128(&mut buf, DW_FORM_NONE as u64);

        // End of abbreviation table
        encode_uleb128(&mut buf, 0);

        buf
    }

    // -----------------------------------------------------------------------
    // .debug_info
    // -----------------------------------------------------------------------

    /// Emit the `.debug_info` section.
    ///
    /// Contains a single compilation unit DIE with nested subprogram and
    /// variable DIEs.
    fn emit_debug_info(&self, _debug_abbrev: &[u8], _debug_line: &[u8]) -> Vec<u8> {
        let mut die_buf = Vec::new();

        // -- Compile Unit DIE (abbrev 1) --
        encode_uleb128(&mut die_buf, 1); // abbrev code
                                         // DW_AT_NAME (DW_FORM_STRING)
        write_null_string(&mut die_buf, &self.source_file);
        // DW_AT_LANGUAGE (DW_FORM_DATA2)
        die_buf.extend_from_slice(&DW_LANG_VUMA.to_le_bytes());
        // DW_AT_PRODUCER (DW_FORM_STRING)
        write_null_string(&mut die_buf, &self.producer);
        // DW_AT_LOW_PC (DW_FORM_ADDR)
        let cu_low_pc = self
            .subprograms
            .iter()
            .map(|s| s.start_offset)
            .min()
            .unwrap_or(0);
        write_address(&mut die_buf, cu_low_pc, self.address_size);
        // DW_AT_HIGH_PC (DW_FORM_DATA4) — offset from low_pc
        let cu_high_pc: u32 = self
            .subprograms
            .iter()
            .map(|s| s.end_offset)
            .max()
            .unwrap_or(0) as u32
            - cu_low_pc as u32;
        die_buf.extend_from_slice(&cu_high_pc.to_le_bytes());
        // DW_AT_STMT_LIST (DW_FORM_SEC_OFFSET) — offset into .debug_line
        die_buf.extend_from_slice(&0u32.to_le_bytes());

        // -- Subprogram DIEs (abbrev 2) --
        for sub in &self.subprograms {
            encode_uleb128(&mut die_buf, 2); // abbrev code
                                             // DW_AT_NAME (DW_FORM_STRING)
            write_null_string(&mut die_buf, &sub.name);
            // DW_AT_LOW_PC (DW_FORM_ADDR)
            write_address(&mut die_buf, sub.start_offset, self.address_size);
            // DW_AT_HIGH_PC (DW_FORM_DATA4) — offset from low_pc
            let size = (sub.end_offset - sub.start_offset) as u32;
            die_buf.extend_from_slice(&size.to_le_bytes());

            // -- Variable DIEs (abbrev 3) — nested inside subprogram --
            for var in &self.variables {
                encode_uleb128(&mut die_buf, 3); // abbrev code
                                                 // DW_AT_NAME (DW_FORM_STRING)
                write_null_string(&mut die_buf, &var.name);
                // DW_AT_TYPE (DW_FORM_STRING)
                write_null_string(&mut die_buf, &var.type_name);
                // DW_AT_LOCATION (DW_FORM_EXPRLOC)
                // DW_OP_fbreg <offset>  =  0x91 + signed LEB128
                let expr = build_fbreg_expr(var.offset);
                encode_uleb128(&mut die_buf, expr.len() as u64);
                die_buf.extend_from_slice(&expr);
            }

            // Null terminator for subprogram children
            encode_uleb128(&mut die_buf, 0);
        }

        // Null terminator for compile_unit children
        encode_uleb128(&mut die_buf, 0);

        // Now build the full .debug_info section with the compilation unit
        // header.
        //
        // DWARF v4 .debug_info header:
        //   unit_length       : 4 bytes
        //   version           : 2 bytes (4)
        //   debug_abbrev_offset: 4 bytes
        //   address_size      : 1 byte
        let mut buf = Vec::new();

        let unit_length = (die_buf.len() as u32)
            // header fields after the unit_length itself:
            + 2  // version
            + 4  // debug_abbrev_offset
            + 1; // address_size
        buf.extend_from_slice(&unit_length.to_le_bytes());
        // Version
        buf.extend_from_slice(&DWARF_VERSION.to_le_bytes());
        // Debug abbrev offset (always 0 — we have a single abbrev table)
        buf.extend_from_slice(&0u32.to_le_bytes());
        // Address size
        buf.push(self.address_size);

        // DIE data
        buf.extend_from_slice(&die_buf);

        buf
    }

    // -----------------------------------------------------------------------
    // .debug_line
    // -----------------------------------------------------------------------

    /// Emit the `.debug_line` section containing a DWARF v4 line-number program.
    fn emit_debug_line(&self) -> Vec<u8> {
        let mut program = Vec::new();

        // Sort line entries by offset for a well-formed program.
        let mut entries = self.line_entries.clone();
        entries.sort_by_key(|e| e.offset);

        // Default_is_stmt: true (beginning of a statement)
        let default_is_stmt: u8 = 1;
        // Line base: -5 (standard DWARF range)
        let line_base: i8 = -5;
        // Line range: 14 (standard DWARF value)
        let line_range: u8 = 14;
        // Opcode base: 13 (standard DWARF value)
        let opcode_base: u8 = 13;
        // Standard opcode lengths for opcodes 1..12
        let standard_opcode_lengths: [u8; 12] = [0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1];
        // Minimum instruction length: target-dependent
        let min_inst_length = self.min_inst_length;

        // -- Build line program opcodes --
        let mut current_line: i32 = 1;
        let mut current_file: u32 = 1;
        let mut current_offset: u64 = 0;

        for entry in &entries {
            // Set file if different
            if entry.file != current_file {
                program.push(DW_LNS_SET_FILE);
                encode_uleb128(&mut program, entry.file as u64);
                current_file = entry.file;
            }

            // Advance line if different
            let line_diff = (entry.line as i32) - current_line;
            if line_diff != 0 {
                program.push(DW_LNS_ADVANCE_LINE);
                encode_sleb128(&mut program, i64::from(line_diff));
                current_line = entry.line as i32;
            }

            // Advance PC
            if entry.offset > current_offset {
                let addr_advance = entry.offset - current_offset;
                // DW_LNS_ADVANCE_PC advances by the operand * min_inst_length
                let op_advance = addr_advance / (min_inst_length as u64);
                if op_advance > 0 {
                    program.push(DW_LNS_ADVANCE_PC);
                    encode_uleb128(&mut program, op_advance);
                }
                current_offset = entry.offset;
            }

            // Copy current row
            program.push(DW_LNS_COPY);
        }

        // End sequence: advance PC to end and emit DW_LNE_END_SEQUENCE
        let end_addr = entries.last().map(|e| e.end_offset()).unwrap_or(0);
        if end_addr > current_offset {
            let op_advance = (end_addr - current_offset) / (min_inst_length as u64);
            if op_advance > 0 {
                program.push(DW_LNS_ADVANCE_PC);
                encode_uleb128(&mut program, op_advance);
            }
        }
        // DW_LNE_END_SEQUENCE: extended opcode
        let ext_opcode_bytes = [DW_LNE_END_SEQUENCE];
        encode_uleb128(&mut program, (ext_opcode_bytes.len() as u64) + 1); // length (includes sub-opcode)
        program.push(DW_LNE_END_SEQUENCE);

        // -- Build DWARF v4 directory and file tables --
        //
        // DWARF v4 uses a simple null-terminated list format:
        //   include_directories: sequence of null-terminated strings,
        //       terminated by a single null byte (empty string).
        //   file_names: sequence of entries, each containing:
        //       name (null-terminated string),
        //       directory index (ULEB128),
        //       time of last modification (ULEB128),
        //       length in bytes (ULEB128),
        //     terminated by a single null byte.

        let directory_table = b".\0"; // one directory: "."

        let mut file_table = Vec::new();
        // File entry: name, dir_index, time, size
        file_table.extend_from_slice(self.source_file.as_bytes());
        file_table.push(0); // null-terminated name
        encode_uleb128(&mut file_table, 0); // directory index 0
        encode_uleb128(&mut file_table, 0); // time (unknown)
        encode_uleb128(&mut file_table, 0); // size (unknown)

        // -- Assemble the DWARF v4 line number program header --
        let mut header = Vec::new();

        // Version (2 bytes)
        header.extend_from_slice(&DWARF_VERSION.to_le_bytes());

        // Header length — will be filled after we know the size
        let header_before_length_field = header.len();
        // Placeholder for 4-byte header length
        header.extend_from_slice(&0u32.to_le_bytes());

        // Minimum instruction length
        header.push(min_inst_length);
        // Default is_stmt
        header.push(default_is_stmt);
        // Line base
        header.push(line_base as u8);
        // Line range
        header.push(line_range);
        // Opcode base
        header.push(opcode_base);
        // Standard opcode lengths
        header.extend_from_slice(&standard_opcode_lengths);

        // Include directories (DWARF v4 format)
        header.extend_from_slice(directory_table);
        header.push(0); // terminator (empty string)

        // File names (DWARF v4 format)
        header.extend_from_slice(&file_table);
        header.push(0); // terminator (empty string = null byte)

        // Fix up header_length (bytes after the header_length field itself)
        let header_length = (header.len() - header_before_length_field - 4) as u32;
        let hl_offset = header_before_length_field;
        header[hl_offset..hl_offset + 4].copy_from_slice(&header_length.to_le_bytes());

        // -- Assemble full .debug_line section --
        let mut buf = Vec::new();

        // Unit length (4 bytes) — total length of the rest of the unit
        let unit_length = (header.len() + program.len()) as u32;
        buf.extend_from_slice(&unit_length.to_le_bytes());
        buf.extend_from_slice(&header);
        buf.extend_from_slice(&program);

        buf
    }

    // -----------------------------------------------------------------------
    // .debug_frame
    // -----------------------------------------------------------------------

    /// Emit the `.debug_frame` section containing a CIE and FDEs.
    ///
    /// The `.debug_frame` section describes how to unwind the stack at every
    /// point in the program.  It consists of:
    ///
    /// 1. A **Common Information Entry (CIE)** that describes the prologue
    ///    pattern shared by all functions: which register is the CFA base,
    ///    which registers are saved, and the alignment factors.
    ///
    /// 2. One **Frame Description Entry (FDE)** per function, referencing the
    ///    CIE and specifying the function's address range.
    ///
    /// If no CIE has been configured (via `set_cie_aarch64`, etc.), this
    /// method returns an empty section.
    fn emit_debug_frame(&self) -> Vec<u8> {
        let cie_ref = match &self.cie {
            Some(c) => c,
            None => return Vec::new(),
        };

        let mut buf = Vec::new();

        // ---- CIE (Common Information Entry) ----
        //
        // Format (DWARF v4, .debug_frame):
        //   length            : 4 bytes (32-bit DWARF) — length of remaining CIE data
        //   CIE_id            : 4 bytes (0xFFFFFFFF)
        //   version           : 1 byte  (1 for DWARF v4 CFI)
        //   augmentation      : null-terminated string ("")
        //   address_size      : 1 byte  (not in DWARF v2; present in v4)
        //   segment_size      : 1 byte  (not in DWARF v2; present in v4)
        //   code_alignment    : ULEB128
        //   data_alignment    : SLEB128
        //   return_address_reg: ULEB128
        //   initial_instructions: CFI instructions

        let mut cie_body = Vec::new();

        // Version
        cie_body.push(DW_CIE_VERSION);

        // Augmentation string (empty)
        cie_body.push(0);

        // Address size (DWARF v4 .debug_frame)
        cie_body.push(self.address_size);

        // Segment selector size (0)
        cie_body.push(0);

        // Code alignment factor
        encode_uleb128(&mut cie_body, cie_ref.code_alignment_factor as u64);

        // Data alignment factor
        encode_sleb128(&mut cie_body, cie_ref.data_alignment_factor as i64);

        // Return address register
        encode_uleb128(&mut cie_body, cie_ref.return_address_reg as u64);

        // Initial CFI instructions for the CIE:
        // DW_CFA_def_cfa: defines CFA = register + offset
        cie_body.push(DW_CFA_DEF_CFA);
        encode_uleb128(&mut cie_body, cie_ref.cfa_reg as u64);
        encode_uleb128(&mut cie_body, cie_ref.cfa_offset as u64);

        // DW_CFA_offset for each saved register
        for saved in &cie_ref.saved_regs {
            // DW_CFA_offset encodes: (opcode | reg) + ULEB128(offset / data_alignment)
            // The opcode is (DW_CFA_OFFSET | (reg & 0x3F))
            // The offset factored by data_alignment_factor.
            // Since data_alignment_factor is negative (e.g., -8), we need
            // offset / |data_alignment_factor| as a positive ULEB128.
            let abs_data_align = cie_ref.data_alignment_factor.unsigned_abs() as i32;
            let factored_offset = if abs_data_align != 0 {
                saved.cfa_offset.abs() / abs_data_align
            } else {
                saved.cfa_offset.abs()
            };
            let reg_low6 = saved.reg & 0x3F;
            cie_body.push(DW_CFA_OFFSET | reg_low6);
            encode_uleb128(&mut cie_body, factored_offset as u64);
        }

        // Compute CIE length: everything after the length field itself
        // length (4) + CIE_id (4) are not counted in the length field
        let cie_length = (cie_body.len() + 4) as u32; // +4 for CIE_id

        // Write CIE to output
        buf.extend_from_slice(&cie_length.to_le_bytes());
        // CIE_id = 0xFFFFFFFF (32-bit DWARF)
        buf.extend_from_slice(&CIE_ID_32.to_le_bytes());
        // CIE body
        buf.extend_from_slice(&cie_body);

        // ---- FDEs (Frame Description Entries) ----
        //
        // Format (DWARF v4, .debug_frame):
        //   length            : 4 bytes
        //   CIE_pointer       : 4 bytes (offset from start of .debug_frame to CIE)
        //   initial_location  : address_size bytes
        //   address_range     : address_size bytes
        //   instructions      : CFI instructions (if any per-function overrides)

        let cie_offset_from_start: u32 = 0; // CIE is the first entry

        for fde in &self.fdes {
            let mut fde_body = Vec::new();

            // CIE pointer (offset to the CIE from the beginning of .debug_frame)
            fde_body.extend_from_slice(&cie_offset_from_start.to_le_bytes());

            // Initial location (address-size dependent)
            write_address(&mut fde_body, fde.initial_location, self.address_size);

            // Address range (address-size dependent)
            write_address(&mut fde_body, fde.address_range, self.address_size);

            // Per-function CFI instructions (none for now — the CIE covers
            // the prologue pattern and GDB/LLDB can infer the rest from
            // the default CFA rule). Future work: emit DW_CFA_advance_loc
            // and DW_CFA_def_cfa_offset for dynamic frame size changes
            // within the function body.

            // Compute FDE length: everything after the length field itself
            // length (4) not counted
            let fde_length = fde_body.len() as u32;

            // Write FDE to output
            buf.extend_from_slice(&fde_length.to_le_bytes());
            buf.extend_from_slice(&fde_body);
        }

        buf
    }
}

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

impl Default for DwarfBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Encode an unsigned LEB128 value into `buf`.
fn encode_uleb128(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        buf.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// Encode a signed LEB128 value into `buf`.
fn encode_sleb128(buf: &mut Vec<u8>, mut value: i64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        let sign_bit_set = (byte & 0x40) != 0;
        let done = value == 0 || (value == -1 && sign_bit_set);
        if !done {
            byte |= 0x80;
        }
        buf.push(byte);
        if done {
            break;
        }
    }
}

/// Write a null-terminated string into `buf`.
fn write_null_string(buf: &mut Vec<u8>, s: &str) {
    buf.extend_from_slice(s.as_bytes());
    buf.push(0);
}

/// Write an address value into `buf` with the given address size (4 or 8 bytes).
fn write_address(buf: &mut Vec<u8>, addr: u64, address_size: u8) {
    match address_size {
        4 => buf.extend_from_slice(&(addr as u32).to_le_bytes()),
        _ => buf.extend_from_slice(&addr.to_le_bytes()),
    }
}

/// Build a `DW_OP_fbreg <offset>` expression.
///
/// `DW_OP_fbreg` (0x91) followed by a signed LEB128 offset.
fn build_fbreg_expr(offset: i32) -> Vec<u8> {
    let mut expr = vec![0x91]; // DW_OP_fbreg
    encode_sleb128(&mut expr, offset as i64);
    expr
}

// ---------------------------------------------------------------------------
// LineEntry helper
// ---------------------------------------------------------------------------

impl LineEntry {
    /// Compute a reasonable end-offset for this line entry.
    ///
    /// For the last entry, we use the offset + 4 (one instruction).
    fn end_offset(&self) -> u64 {
        self.offset + 4
    }
}

// ---------------------------------------------------------------------------
// ELF Integration
// ---------------------------------------------------------------------------

/// Section header type for DWARF debug info sections (progbits).
const SHT_PROGBITS: u32 = 1;

/// Append DWARF debug sections to an ELF binary.
///
/// Given an existing ELF binary and the debug sections, this function inserts
/// the four debug sections (`.debug_abbrev`, `.debug_info`, `.debug_line`,
/// `.debug_frame`) into the ELF, updates the section header table, and patches
/// the section header string table and section count.
///
/// If `.debug_frame` is empty (no CIE configured), it is simply omitted from
/// the ELF, keeping the binary compact.
///
/// This is a simplified integration that appends sections before the section
/// headers. It assumes the input ELF was produced by `emit_elf` with
/// section headers enabled.
pub fn append_debug_sections_to_elf(elf: &mut Vec<u8>, debug: &DebugSections) {
    // Parse the ELF header to find section header table location and count.
    if elf.len() < 64 {
        return;
    }

    let e_shoff = u64::from_le_bytes(elf[40..48].try_into().unwrap_or([0; 8]));
    let e_shentsize = u16::from_le_bytes(elf[58..60].try_into().unwrap_or([0; 2])) as u64;
    let e_shnum = u16::from_le_bytes(elf[60..62].try_into().unwrap_or([0; 2])) as usize;
    let e_shstrndx = u16::from_le_bytes(elf[62..64].try_into().unwrap_or([0; 2])) as usize;

    if e_shoff == 0 || e_shnum == 0 || e_shentsize == 0 {
        return;
    }

    // Read the .shstrtab section to find its data.
    let shstrtab_shdr_offset = e_shoff as usize + e_shstrndx * e_shentsize as usize;
    if shstrtab_shdr_offset + 64 > elf.len() {
        return;
    }
    let shstrtab_offset = u64::from_le_bytes(
        elf[shstrtab_shdr_offset + 24..shstrtab_shdr_offset + 32]
            .try_into()
            .unwrap_or([0; 8]),
    ) as usize;
    let shstrtab_size = u64::from_le_bytes(
        elf[shstrtab_shdr_offset + 32..shstrtab_shdr_offset + 40]
            .try_into()
            .unwrap_or([0; 8]),
    ) as usize;

    // Build new shstrtab with debug section names appended.
    let mut new_shstrtab = elf[shstrtab_offset..shstrtab_offset + shstrtab_size].to_vec();

    // Record name offsets
    let debug_abbrev_name_off = new_shstrtab.len() as u32;
    new_shstrtab.extend_from_slice(b".debug_abbrev\0");
    let debug_info_name_off = new_shstrtab.len() as u32;
    new_shstrtab.extend_from_slice(b".debug_info\0");
    let debug_line_name_off = new_shstrtab.len() as u32;
    new_shstrtab.extend_from_slice(b".debug_line\0");
    let debug_frame_name_off = new_shstrtab.len() as u32;
    new_shstrtab.extend_from_slice(b".debug_frame\0");

    // The new sections will be appended right before the section header table.
    let new_sections_start = e_shoff as usize;

    // Compute aligned sizes for debug sections.
    let debug_abbrev_aligned = align_up(debug.debug_abbrev.len() as u64, 8) as usize;
    let debug_info_aligned = align_up(debug.debug_info.len() as u64, 8) as usize;
    let debug_line_aligned = align_up(debug.debug_line.len() as u64, 8) as usize;
    let debug_frame_aligned = align_up(debug.debug_frame.len() as u64, 8) as usize;
    let new_shstrtab_aligned = align_up(new_shstrtab.len() as u64, 8) as usize;

    // Count how many debug sections we'll add (debug_frame may be empty).
    let include_debug_frame = !debug.debug_frame.is_empty();
    let num_new_sections = if include_debug_frame { 4 } else { 3 };

    // Build section data to insert.
    let mut new_section_data = Vec::new();

    let debug_abbrev_file_offset = new_sections_start + new_section_data.len();
    new_section_data.extend_from_slice(&debug.debug_abbrev);
    let pad = debug_abbrev_aligned - debug.debug_abbrev.len();
    new_section_data.extend_from_slice(&vec![0u8; pad]);

    let debug_info_file_offset = new_sections_start + new_section_data.len();
    new_section_data.extend_from_slice(&debug.debug_info);
    let pad = debug_info_aligned - debug.debug_info.len();
    new_section_data.extend_from_slice(&vec![0u8; pad]);

    let debug_line_file_offset = new_sections_start + new_section_data.len();
    new_section_data.extend_from_slice(&debug.debug_line);
    let pad = debug_line_aligned - debug.debug_line.len();
    new_section_data.extend_from_slice(&vec![0u8; pad]);

    let debug_frame_file_offset = if include_debug_frame {
        let off = new_sections_start + new_section_data.len();
        new_section_data.extend_from_slice(&debug.debug_frame);
        let pad = debug_frame_aligned - debug.debug_frame.len();
        new_section_data.extend_from_slice(&vec![0u8; pad]);
        Some(off)
    } else {
        None
    };

    // Replace the old shstrtab with the new one.
    let new_shstrtab_file_offset = new_sections_start + new_section_data.len();
    new_section_data.extend_from_slice(&new_shstrtab);
    let pad = new_shstrtab_aligned - new_shstrtab.len();
    new_section_data.extend_from_slice(&vec![0u8; pad]);

    // Now insert the new section data before the section headers.
    let old_shdrs: Vec<u8> = elf.drain(e_shoff as usize..).collect();

    // Insert debug section data.
    elf.extend_from_slice(&new_section_data);

    // Compute the new section header table offset.
    let new_e_shoff = elf.len() as u64;

    // Build new section headers: copy old ones, add debug section headers,
    // and update the .shstrtab header.
    let mut new_shdrs = old_shdrs;

    // Update the .shstrtab section header to point to the new data.
    let shstrtab_shdr_start = e_shstrndx * 64;
    if shstrtab_shdr_start + 64 <= new_shdrs.len() {
        // sh_offset (bytes 24-31)
        new_shdrs[shstrtab_shdr_start + 24..shstrtab_shdr_start + 32]
            .copy_from_slice(&new_shstrtab_file_offset.to_le_bytes());
        // sh_size (bytes 32-39)
        new_shdrs[shstrtab_shdr_start + 32..shstrtab_shdr_start + 40]
            .copy_from_slice(&(new_shstrtab.len() as u64).to_le_bytes());
    }

    // Add section headers for the debug sections.
    let new_num_shdrs = e_shnum + num_new_sections;

    // .debug_abbrev section header
    let mut sh = new_shdr(
        SHT_PROGBITS,
        0, // no flags
        0, // no virtual address
        debug_abbrev_file_offset as u64,
        debug.debug_abbrev.len() as u64,
        0,
        0,
        1,
        0,
    );
    sh.name = debug_abbrev_name_off;
    write_filled_shdr(&mut new_shdrs, &sh);

    // .debug_info section header
    let mut sh = new_shdr(
        SHT_PROGBITS,
        0,
        0,
        debug_info_file_offset as u64,
        debug.debug_info.len() as u64,
        0,
        0,
        1,
        0,
    );
    sh.name = debug_info_name_off;
    write_filled_shdr(&mut new_shdrs, &sh);

    // .debug_line section header
    let mut sh = new_shdr(
        SHT_PROGBITS,
        0,
        0,
        debug_line_file_offset as u64,
        debug.debug_line.len() as u64,
        0,
        0,
        1,
        0,
    );
    sh.name = debug_line_name_off;
    write_filled_shdr(&mut new_shdrs, &sh);

    // .debug_frame section header (only if we have frame data)
    if let Some(frame_offset) = debug_frame_file_offset {
        let mut sh = new_shdr(
            SHT_PROGBITS,
            0,
            0,
            frame_offset as u64,
            debug.debug_frame.len() as u64,
            0,
            0,
            4, // debug_frame typically has 4-byte alignment
            0,
        );
        sh.name = debug_frame_name_off;
        write_filled_shdr(&mut new_shdrs, &sh);
    }

    // Append section headers.
    elf.extend_from_slice(&new_shdrs);

    // Update the ELF header: e_shoff and e_shnum.
    elf[40..48].copy_from_slice(&new_e_shoff.to_le_bytes());
    elf[60..62].copy_from_slice(&(new_num_shdrs as u16).to_le_bytes());
    // e_shstrndx stays the same — the .shstrtab section index hasn't changed.
}

// ---------------------------------------------------------------------------
// ELF helper structs (duplicated from emit.rs to keep dwarf.rs self-contained)
// ---------------------------------------------------------------------------

/// A filled-in section header, ready to be serialized.
struct FilledShdr {
    name: u32,
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

#[allow(clippy::too_many_arguments)]
fn new_shdr(
    sh_type: u32,
    sh_flags: u64,
    sh_addr: u64,
    sh_offset: u64,
    sh_size: u64,
    sh_link: u32,
    sh_info: u32,
    sh_addralign: u64,
    sh_entsize: u64,
) -> FilledShdr {
    FilledShdr {
        name: 0,
        sh_type,
        sh_flags,
        sh_addr,
        sh_offset,
        sh_size,
        sh_link,
        sh_info,
        sh_addralign,
        sh_entsize,
    }
}

fn write_filled_shdr(buf: &mut Vec<u8>, sh: &FilledShdr) {
    buf.extend_from_slice(&sh.name.to_le_bytes());
    buf.extend_from_slice(&sh.sh_type.to_le_bytes());
    buf.extend_from_slice(&sh.sh_flags.to_le_bytes());
    buf.extend_from_slice(&sh.sh_addr.to_le_bytes());
    buf.extend_from_slice(&sh.sh_offset.to_le_bytes());
    buf.extend_from_slice(&sh.sh_size.to_le_bytes());
    buf.extend_from_slice(&sh.sh_link.to_le_bytes());
    buf.extend_from_slice(&sh.sh_info.to_le_bytes());
    buf.extend_from_slice(&sh.sh_addralign.to_le_bytes());
    buf.extend_from_slice(&sh.sh_entsize.to_le_bytes());
}

/// Round `value` up to the nearest multiple of `alignment`.
fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

// ---------------------------------------------------------------------------
// ULEB128 / SLEB128 decoding helpers (for tests)
// ---------------------------------------------------------------------------

/// Decode a ULEB128 value from `buf` starting at `offset`.
/// Returns (value, bytes_consumed).
#[cfg(test)]
fn decode_uleb128(buf: &[u8], offset: usize) -> (u64, usize) {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    let mut i = offset;
    loop {
        let byte = buf[i];
        value |= ((byte & 0x7F) as u64) << shift;
        shift += 7;
        i += 1;
        if (byte & 0x80) == 0 {
            break;
        }
    }
    (value, i - offset)
}

/// Decode a SLEB128 value from `buf` starting at `offset`.
/// Returns (value, bytes_consumed).
#[cfg(test)]
fn decode_sleb128(buf: &[u8], offset: usize) -> (i64, usize) {
    let mut value: i64 = 0;
    let mut shift: u32 = 0;
    let mut i = offset;
    let mut byte: u8;
    loop {
        byte = buf[i];
        value |= ((byte & 0x7F) as i64) << shift;
        shift += 7;
        i += 1;
        if (byte & 0x80) == 0 {
            break;
        }
    }
    // Sign extend
    if shift < 64 && (byte & 0x40) != 0 {
        value |= !0 << shift;
    }
    (value, i - offset)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a DwarfBuilder with a compile unit and one subprogram.
    fn make_simple_builder() -> DwarfBuilder {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
        db.add_subprogram("main", 0, 64);
        db
    }

    // -- Test 1: Debug abbrev has correct abbreviation codes --
    #[test]
    fn test_debug_abbrev_abbrev_codes() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let abbrev = &sections.debug_abbrev;

        // First abbreviation code should be 1
        let (code1, n1) = decode_uleb128(abbrev, 0);
        assert_eq!(code1, 1, "first abbreviation code should be 1");
        // It should be DW_TAG_COMPILE_UNIT
        let (tag1, _) = decode_uleb128(abbrev, n1);
        assert_eq!(
            tag1, DW_TAG_COMPILE_UNIT as u64,
            "first tag should be DW_TAG_COMPILE_UNIT"
        );
    }

    // -- Test 2: Debug info has correct DWARF version --
    #[test]
    fn test_debug_info_version() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // .debug_info starts with: unit_length (4 bytes) + version (2 bytes)
        assert!(info.len() > 6, "debug_info must have at least 6 bytes");
        let version = u16::from_le_bytes([info[4], info[5]]);
        assert_eq!(version, DWARF_VERSION, "DWARF version should be 4");
    }

    // -- Test 3: Debug info compilation unit is present --
    #[test]
    fn test_debug_info_compile_unit_present() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // DWARF v4 header: unit_length(4) + version(2) + debug_abbrev_offset(4) + address_size(1) = 11 bytes
        let header_size = 11;
        assert!(
            info.len() > header_size,
            "debug_info must have DIEs after header"
        );
        let (abbrev_code, _) = decode_uleb128(info, header_size);
        assert_eq!(
            abbrev_code, 1,
            "first DIE should be DW_TAG_COMPILE_UNIT (abbrev 1)"
        );
    }

    // -- Test 4: Subprogram entries are correct --
    #[test]
    fn test_subprogram_entries() {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen");
        db.add_subprogram("foo", 0, 32);
        db.add_subprogram("bar", 32, 96);
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // Check that both function names appear in the debug_info section.
        let info_str = String::from_utf8_lossy(info);
        assert!(info_str.contains("foo"), "debug_info should contain 'foo'");
        assert!(info_str.contains("bar"), "debug_info should contain 'bar'");
    }

    // -- Test 5: Debug line has correct DWARF version --
    #[test]
    fn test_debug_line_version() {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen");
        db.add_line_entry(0, 1, 1, 0);
        let sections = db.emit_debug_sections();
        let line = &sections.debug_line;

        // .debug_line starts with: unit_length (4 bytes) + version (2 bytes)
        assert!(line.len() > 6, "debug_line must have at least 6 bytes");
        let version = u16::from_le_bytes([line[4], line[5]]);
        assert_eq!(version, DWARF_VERSION, "line program version should be 4");
    }

    // -- Test 6: Line number program is well-formed (has DW_LNE_END_SEQUENCE) --
    #[test]
    fn test_line_program_end_sequence() {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen");
        db.add_line_entry(0, 1, 1, 0);
        db.add_line_entry(8, 1, 2, 0);
        let sections = db.emit_debug_sections();
        let line = &sections.debug_line;

        // The line program must end with DW_LNE_END_SEQUENCE.
        // Search for the extended opcode from the end.
        let mut found_end_seq = false;
        for i in (0..line.len()).rev() {
            if line[i] == DW_LNE_END_SEQUENCE {
                // Check that the byte before is the length of the extended opcode (1)
                if i > 0 {
                    let (len, _) = decode_uleb128(line, i - 1);
                    // The length could be encoded as a single byte
                    // The DW_LNE_END_SEQUENCE takes 1 byte, so length = 1
                    if len == 1 {
                        found_end_seq = true;
                        break;
                    }
                }
            }
        }
        assert!(
            found_end_seq,
            "line program must contain DW_LNE_END_SEQUENCE"
        );
    }

    // -- Test 7: Line program contains DW_LNS_COPY opcodes --
    #[test]
    fn test_line_program_has_copy_opcodes() {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen");
        db.add_line_entry(0, 1, 1, 0);
        db.add_line_entry(8, 1, 2, 0);
        let sections = db.emit_debug_sections();
        let line = &sections.debug_line;

        // Search for DW_LNS_COPY opcode in the line program.
        assert!(
            line.iter().any(|&b| b == DW_LNS_COPY),
            "line program must contain DW_LNS_COPY opcodes"
        );
    }

    // -- Test 8: Integration with ELF emission --
    #[test]
    fn test_elf_debug_section_integration() {
        use crate::emit::{emit_elf, EmitConfig};
        use crate::ir::{IRFunction, IRTerminator};

        // Create a minimal function.
        let mut func = IRFunction::new("main");
        func.current_block().terminator = IRTerminator::Return(vec![]);

        let config = EmitConfig::linux_elf();
        let mut elf = emit_elf(&[func], &[], &config).unwrap();

        // Build debug info.
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
        db.add_subprogram("main", 0, 32);
        db.add_line_entry(0, 1, 1, 0);
        db.set_cie_aarch64(); // Set CIE so .debug_frame is emitted
        db.add_fde("main", 0, 32);
        let sections = db.emit_debug_sections();

        // Append debug sections to ELF.
        append_debug_sections_to_elf(&mut elf, &sections);

        // Verify ELF is still valid.
        assert_eq!(
            &elf[0..4],
            &[0x7f, b'E', b'L', b'F'],
            "ELF magic must be intact"
        );
        assert!(elf.len() > 64, "ELF must be larger than header");

        // Verify section count increased (8 original + 4 debug = 12).
        let e_shnum = u16::from_le_bytes([elf[60], elf[61]]);
        assert_eq!(
            e_shnum, 12,
            "expected 12 section headers (8 original + 4 debug)"
        );

        // Verify debug section names are in the section header string table.
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
    }

    // -- Test 9: Variable location expression (DW_OP_fbreg) --
    #[test]
    fn test_variable_location_expr() {
        let expr = build_fbreg_expr(-8);
        assert_eq!(expr[0], 0x91, "first byte should be DW_OP_fbreg");
        let (offset, _) = decode_sleb128(&expr, 1);
        assert_eq!(offset, -8, "fbreg offset should be -8");
    }

    // -- Test 10: ULEB128 / SLEB128 round-trip --
    #[test]
    fn test_leb128_roundtrip() {
        // ULEB128
        let mut buf = Vec::new();
        encode_uleb128(&mut buf, 0);
        assert_eq!(buf, vec![0]);

        let mut buf = Vec::new();
        encode_uleb128(&mut buf, 127);
        assert_eq!(buf, vec![127]);

        let mut buf = Vec::new();
        encode_uleb128(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        let (val, n) = decode_uleb128(&[0x80, 0x01], 0);
        assert_eq!(val, 128);
        assert_eq!(n, 2);

        // SLEB128
        let mut buf = Vec::new();
        encode_sleb128(&mut buf, -1);
        let (val, _) = decode_sleb128(&buf, 0);
        assert_eq!(val, -1);

        let mut buf = Vec::new();
        encode_sleb128(&mut buf, -128);
        let (val, _) = decode_sleb128(&buf, 0);
        assert_eq!(val, -128);

        let mut buf = Vec::new();
        encode_sleb128(&mut buf, 63);
        let (val, _) = decode_sleb128(&buf, 0);
        assert_eq!(val, 63);
    }

    // -- Test 11: Empty builder still produces valid sections --
    #[test]
    fn test_empty_builder() {
        let db = DwarfBuilder::new();
        let sections = db.emit_debug_sections();
        // All sections should be non-empty even with no data recorded.
        assert!(
            !sections.debug_abbrev.is_empty(),
            "debug_abbrev should not be empty"
        );
        assert!(
            !sections.debug_info.is_empty(),
            "debug_info should not be empty"
        );
        assert!(
            !sections.debug_line.is_empty(),
            "debug_line should not be empty"
        );
    }

    // -- Test 12: DWARF v4 debug_info header has correct layout --
    #[test]
    fn test_debug_info_v4_header() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // DWARF v4 header: unit_length(4) + version(2) + debug_abbrev_offset(4) + address_size(1)
        assert!(info.len() > 11, "debug_info must have at least 11 bytes");
        // version at offset 4 should be 4
        let version = u16::from_le_bytes([info[4], info[5]]);
        assert_eq!(version, 4, "DWARF version should be 4");
        // debug_abbrev_offset at offset 6 should be 0
        let abbrev_off = u32::from_le_bytes([info[6], info[7], info[8], info[9]]);
        assert_eq!(abbrev_off, 0, "debug_abbrev_offset should be 0");
        // address_size at offset 10 should be 8 (64-bit)
        assert_eq!(info[10], 8, "address_size should be 8 for 64-bit builder");
    }

    // -- Test 13: Debug info address size matches builder config --
    #[test]
    fn test_debug_info_address_size() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // DWARF v4 header: unit_length(4) + version(2) + debug_abbrev_offset(4) + address_size(1)
        assert!(info.len() > 11, "debug_info must have at least 11 bytes");
        assert_eq!(
            info[10], 8,
            "address size should be 8 for 64-bit builder (default)"
        );

        // Test 32-bit builder
        let mut db32 = DwarfBuilder::new_32bit(2);
        db32.add_compile_unit("test.vuma", "vuma-codegen");
        db32.add_subprogram("main", 0, 32);
        let sections32 = db32.emit_debug_sections();
        let info32 = &sections32.debug_info;
        assert!(info32.len() > 11, "32-bit debug_info must have at least 11 bytes");
        assert_eq!(
            info32[10], 4,
            "address size should be 4 for 32-bit builder"
        );
    }

    // -- Test 14: Line program contains DW_LNS_ADVANCE_LINE --
    #[test]
    fn test_line_program_advance_line() {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen");
        db.add_line_entry(0, 1, 1, 0);
        db.add_line_entry(8, 1, 5, 0); // line jump from 1 to 5
        let sections = db.emit_debug_sections();
        let line = &sections.debug_line;

        assert!(
            line.iter().any(|&b| b == DW_LNS_ADVANCE_LINE),
            "line program must contain DW_LNS_ADVANCE_LINE for line jumps"
        );
    }

    // -- Test 15: Debug info contains source file name --
    #[test]
    fn test_debug_info_source_file() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info_str = String::from_utf8_lossy(&sections.debug_info);
        assert!(
            info_str.contains("test.vuma"),
            "debug_info should contain the source file name"
        );
    }

    // -- Test 16: for_backend creates correct address sizes --
    #[test]
    fn test_for_backend_address_sizes() {
        use crate::backend::BackendKind;

        // 64-bit backends should have address_size = 8
        for kind in [
            BackendKind::AArch64,
            BackendKind::X86_64,
            BackendKind::RiscV64,
            BackendKind::Mips64,
            BackendKind::PowerPC64,
            BackendKind::LoongArch64,
        ] {
            let db = DwarfBuilder::for_backend(kind);
            assert_eq!(
                db.address_size(), 8,
                "{:?}: 64-bit backend should have address_size=8", kind
            );
        }

        // 32-bit backends should have address_size = 4
        for kind in [BackendKind::Arm32, BackendKind::Wasm32] {
            let db = DwarfBuilder::for_backend(kind);
            assert_eq!(
                db.address_size(), 4,
                "{:?}: 32-bit backend should have address_size=4", kind
            );
        }
    }

    // -- Test 17: for_backend creates correct min_inst_length --
    #[test]
    fn test_for_backend_min_inst_length() {
        use crate::backend::BackendKind;

        assert_eq!(DwarfBuilder::for_backend(BackendKind::X86_64).min_inst_length(), 1);
        assert_eq!(DwarfBuilder::for_backend(BackendKind::AArch64).min_inst_length(), 4);
        assert_eq!(DwarfBuilder::for_backend(BackendKind::RiscV64).min_inst_length(), 2);
        assert_eq!(DwarfBuilder::for_backend(BackendKind::Arm32).min_inst_length(), 2);
        assert_eq!(DwarfBuilder::for_backend(BackendKind::Mips64).min_inst_length(), 4);
        assert_eq!(DwarfBuilder::for_backend(BackendKind::PowerPC64).min_inst_length(), 4);
        assert_eq!(DwarfBuilder::for_backend(BackendKind::LoongArch64).min_inst_length(), 4);
        assert_eq!(DwarfBuilder::for_backend(BackendKind::Wasm32).min_inst_length(), 1);
    }

    // -- Test 18: DWARF v4 debug_line section has correct version --
    #[test]
    fn test_debug_line_v4_version() {
        let mut db = DwarfBuilder::new_32bit(2);
        db.add_compile_unit("test.vuma", "vuma-codegen");
        db.add_line_entry(0, 1, 1, 0);
        let sections = db.emit_debug_sections();
        let line = &sections.debug_line;

        // .debug_line header: unit_length(4) + version(2)
        assert!(line.len() > 6, "debug_line must have at least 6 bytes");
        let version = u16::from_le_bytes([line[4], line[5]]);
        assert_eq!(version, 4, "debug_line version should be 4 (DWARF v4)");
    }

    // -- Test 19: 32-bit debug_info addresses are 4 bytes --
    #[test]
    fn test_32bit_debug_info_addresses() {
        let mut db32 = DwarfBuilder::new_32bit(2);
        db32.add_compile_unit("test.vuma", "vuma-codegen");
        db32.add_subprogram("main", 0x100, 0x200);
        let sections32 = db32.emit_debug_sections();
        let info32 = &sections32.debug_info;

        // DWARF v4 header is 11 bytes
        assert!(info32.len() > 11, "32-bit debug_info must have DIEs after header");
        let (abbrev_code, _) = decode_uleb128(info32, 11);
        assert_eq!(abbrev_code, 1, "first DIE should be compile unit");

        // Verify that 4-byte addresses produce smaller sections than 8-byte
        let mut db64 = DwarfBuilder::new();
        db64.add_compile_unit("test.vuma", "vuma-codegen");
        db64.add_subprogram("main", 0x100, 0x200);
        let sections64 = db64.emit_debug_sections();

        // 64-bit sections should be larger due to 8-byte addresses
        assert!(
            sections64.debug_info.len() > sections32.debug_info.len(),
            "64-bit debug_info should be larger than 32-bit (8-byte vs 4-byte addresses)"
        );
    }

    // -- Test 20: with_config allows arbitrary configuration --
    #[test]
    fn test_with_config() {
        let db = DwarfBuilder::with_config(4, 1);
        assert_eq!(db.address_size(), 4);
        assert_eq!(db.min_inst_length(), 1);

        let db2 = DwarfBuilder::with_config(8, 2);
        assert_eq!(db2.address_size(), 8);
        assert_eq!(db2.min_inst_length(), 2);
    }

    // -- Test 21: .debug_frame section with CIE and FDE --
    #[test]
    fn test_debug_frame_section() {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
        db.add_subprogram("main", 0, 64);
        db.set_cie_aarch64();
        db.add_fde("main", 0, 64);
        let sections = db.emit_debug_sections();
        let frame = &sections.debug_frame;

        assert!(!frame.is_empty(), "debug_frame should not be empty when CIE is set");

        // First 4 bytes: CIE length
        let cie_length = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        assert!(cie_length > 0, "CIE length should be positive");

        // Next 4 bytes: CIE_id = 0xFFFFFFFF
        let cie_id = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
        assert_eq!(cie_id, 0xFFFFFFFF, "CIE_id should be 0xFFFFFFFF");
    }

    // -- Test 22: debug_frame is empty without CIE --
    #[test]
    fn test_debug_frame_empty_without_cie() {
        let db = DwarfBuilder::new();
        let sections = db.emit_debug_sections();
        assert!(
            sections.debug_frame.is_empty(),
            "debug_frame should be empty when no CIE is set"
        );
    }

    // -- Test 23: set_cie_for_backend creates correct CIEs --
    #[test]
    fn test_set_cie_for_backend() {
        use crate::backend::BackendKind;

        let mut db = DwarfBuilder::for_backend(BackendKind::AArch64);
        db.set_cie_for_backend(BackendKind::AArch64);
        assert!(db.cie.is_some(), "AArch64 CIE should be set");
        let cie = db.cie.as_ref().unwrap();
        assert_eq!(cie.cfa_reg, 31, "AArch64 CFA register should be SP (31)");
        assert_eq!(cie.return_address_reg, 30, "AArch64 return address should be LR (30)");

        let mut db2 = DwarfBuilder::for_backend(BackendKind::X86_64);
        db2.set_cie_for_backend(BackendKind::X86_64);
        let cie2 = db2.cie.as_ref().unwrap();
        assert_eq!(cie2.cfa_reg, 7, "x86_64 CFA register should be RSP (7)");
    }

    // -- Test 24: FDE entries produce correct debug_frame layout --
    #[test]
    fn test_fde_entries_in_debug_frame() {
        let mut db = DwarfBuilder::new();
        db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
        db.add_subprogram("foo", 0, 32);
        db.add_subprogram("bar", 32, 64);
        db.set_cie_aarch64();
        db.add_fde("foo", 0, 32);
        db.add_fde("bar", 32, 64);
        let sections = db.emit_debug_sections();
        let frame = &sections.debug_frame;

        // CIE length + CIE_id + CIE body + FDE1 length + FDE1 body + FDE2 length + FDE2 body
        let cie_length = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
        let cie_total = 4 + cie_length; // 4 bytes for the length field itself
        assert!(frame.len() > cie_total, "debug_frame should have FDEs after CIE");

        // First FDE starts after CIE
        let fde_offset = cie_total;
        let fde_length = u32::from_le_bytes(
            [frame[fde_offset], frame[fde_offset + 1], frame[fde_offset + 2], frame[fde_offset + 3]]
        ) as usize;
        // FDE should contain: CIE_pointer(4) + initial_location(8) + address_range(8) = 20 bytes
        assert!(fde_length >= 20, "FDE should contain at least 20 bytes for 64-bit");
    }
}
