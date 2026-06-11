//! # DWARF5 Debug Info Generation
//!
//! Produces DWARF version 5 debug information sections for the VUMA AArch64
//! backend.  The emitted sections can be appended to an ELF binary so that
//! tools such as `readelf`, `objdump`, and `gdb` can decode the program's
//! structure, function boundaries, local variables, and source-line mapping.
//!
//! ## Sections Generated
//!
//! | Section          | Contents                                          |
//! |------------------|---------------------------------------------------|
//! | `.debug_abbrev`  | Abbreviation tables (tag + attribute encodings)   |
//! | `.debug_info`    | Compilation unit DIEs (subprograms, variables)    |
//! | `.debug_line`    | Line-number program (DWARF5 standard opcodes)     |
//!
//! ## DWARF5 Encoding
//!
//! - Abbreviation codes: `DW_TAG_COMPILE_UNIT`, `DW_TAG_SUBPROGRAM`,
//!   `DW_TAG_VARIABLE`
//! - Attributes: `DW_AT_NAME`, `DW_AT_LOW_PC`, `DW_AT_HIGH_PC`,
//!   `DW_AT_TYPE`, `DW_AT_LOCATION`
//! - Forms: `DW_FORM_STRING`, `DW_FORM_ADDR`, `DW_FORM_DATA4`,
//!   `DW_FORM_EXPRLOC`
//! - Line-number opcodes: `DW_LNS_COPY`, `DW_LNS_ADVANCE_PC`,
//!   `DW_LNS_ADVANCE_LINE`, `DW_LNS_SET_FILE`, `DW_LNE_END_SEQUENCE`

// ---------------------------------------------------------------------------
// DWARF5 Constants
// ---------------------------------------------------------------------------

/// DWARF version number (5).
const DWARF_VERSION: u16 = 5;

/// DWARF5 unit type: compile unit.
const DW_UT_COMPILE: u8 = 0x01;

/// DWARF5 address size for AArch64 (64-bit).
const ADDRESS_SIZE: u8 = 8;

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

// ---------------------------------------------------------------------------
// DwarfBuilder
// ---------------------------------------------------------------------------

/// Accumulates debug information during codegen and emits DWARF5 sections.
///
/// # Example
///
/// ```
/// use vuma_codegen::dwarf::DwarfBuilder;
///
/// let mut db = DwarfBuilder::new();
/// db.add_compile_unit("test.vuma", "vuma-codegen 0.1");
/// db.add_subprogram("main", 0, 64);
/// db.add_variable("x", "i64", -8, 0);
/// db.add_line_entry(0, 1, 1, 0);
/// db.add_line_entry(16, 1, 2, 4);
/// let sections = db.emit_debug_sections();
/// assert!(!sections.debug_abbrev.is_empty());
/// assert!(!sections.debug_info.is_empty());
/// assert!(!sections.debug_line.is_empty());
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
}

/// The three DWARF5 debug sections emitted by [`DwarfBuilder::emit_debug_sections`].
#[derive(Debug, Clone)]
pub struct DebugSections {
    /// `.debug_abbrev` — abbreviation table.
    pub debug_abbrev: Vec<u8>,
    /// `.debug_info` — compilation unit DIEs.
    pub debug_info: Vec<u8>,
    /// `.debug_line` — line number program.
    pub debug_line: Vec<u8>,
}

impl DwarfBuilder {
    /// Create a new, empty `DwarfBuilder`.
    pub fn new() -> Self {
        Self {
            source_file: String::new(),
            producer: String::new(),
            subprograms: Vec::new(),
            variables: Vec::new(),
            line_entries: Vec::new(),
        }
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

    /// Emit all three DWARF5 debug sections.
    ///
    /// Returns a [`DebugSections`] containing `.debug_abbrev`, `.debug_info`,
    /// and `.debug_line`.
    pub fn emit_debug_sections(&self) -> DebugSections {
        let debug_abbrev = self.emit_debug_abbrev();
        let debug_line = self.emit_debug_line();
        let debug_info = self.emit_debug_info(&debug_abbrev, &debug_line);
        DebugSections {
            debug_abbrev,
            debug_info,
            debug_line,
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
        die_buf.extend_from_slice(&cu_low_pc.to_le_bytes());
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
            die_buf.extend_from_slice(&sub.start_offset.to_le_bytes());
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
        let mut buf = Vec::new();

        // Unit length (4-byte length, DWARF5 format) — we fill this in after.
        let unit_length = (die_buf.len() as u32)
            // header fields after the unit_length itself:
            + 2  // version
            + 1  // unit_type
            + 1  // address_size
            + 4; // debug_abbrev_offset
        buf.extend_from_slice(&unit_length.to_le_bytes());
        // Version
        buf.extend_from_slice(&DWARF_VERSION.to_le_bytes());
        // Unit type
        buf.push(DW_UT_COMPILE);
        // Address size
        buf.push(ADDRESS_SIZE);
        // Debug abbrev offset (always 0 — we have a single abbrev table)
        buf.extend_from_slice(&0u32.to_le_bytes());

        // DIE data
        buf.extend_from_slice(&die_buf);

        buf
    }

    // -----------------------------------------------------------------------
    // .debug_line
    // -----------------------------------------------------------------------

    /// Emit the `.debug_line` section containing a DWARF5 line-number program.
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
        // Opcode base: 13 (standard DWARF5 value)
        let opcode_base: u8 = 13;
        // Standard opcode lengths for opcodes 1..12
        let standard_opcode_lengths: [u8; 12] = [0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1];
        // Minimum instruction length: 4 (AArch64)
        let min_inst_length: u8 = 4;
        // Maximum operations per instruction: 1
        let max_ops_per_inst: u8 = 1;

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
                // Since min_inst_length = 4, operand = addr_advance / 4
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
        // Use the last entry's offset + some delta, or 0 if no entries
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

        // -- Build directory entry format and table --
        // DWARF5: directory entry format count
        let dir_entry_format_count: u8 = 1;
        // Directory entry format: (DW_LNCT_path, DW_FORM_STRING)
        let dir_entry_format: Vec<u8> = {
            let mut f = Vec::new();
            encode_uleb128(&mut f, 0x01); // DW_LNCT_path
            encode_uleb128(&mut f, DW_FORM_STRING as u64);
            f
        };

        // Directories: just one — the compilation directory (".")
        let directories_count: u8 = 1;
        let directory_name = b".\0";

        // -- Build file name entry format and table --
        // DWARF5: file name entry format count
        let file_entry_format_count: u8 = 2;
        // File entry format: (DW_LNCT_path, DW_FORM_STRING), (DW_LNCT_directory_index, DW_FORM_DATA4)
        let file_entry_format: Vec<u8> = {
            let mut f = Vec::new();
            encode_uleb128(&mut f, 0x01); // DW_LNCT_path
            encode_uleb128(&mut f, DW_FORM_STRING as u64);
            encode_uleb128(&mut f, 0x02); // DW_LNCT_directory_index
            encode_uleb128(&mut f, DW_FORM_DATA4 as u64);
            f
        };

        // File names: just one — the source file
        let file_names_count: u8 = 1;
        let file_name_entry = {
            let mut f = Vec::new();
            f.extend_from_slice(self.source_file.as_bytes());
            f.push(0); // null-terminated
            f.extend_from_slice(&0u32.to_le_bytes()); // directory index 0
            f
        };

        // -- Assemble the line number program header --
        let mut header = Vec::new();

        // Version
        header.extend_from_slice(&DWARF_VERSION.to_le_bytes());

        // Address size
        header.push(ADDRESS_SIZE);

        // Segment selector size (DWARF5: 0)
        header.push(0);

        // Header length — will be filled after we know the size
        let header_before_length_field = header.len();
        // Placeholder for 4-byte header length
        header.extend_from_slice(&0u32.to_le_bytes());

        // Minimum instruction length
        header.push(min_inst_length);
        // Maximum operations per instruction
        header.push(max_ops_per_inst);
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

        // Directory entry format count
        header.push(dir_entry_format_count);
        // Directory entry format
        header.extend_from_slice(&dir_entry_format);
        // Directories count
        encode_uleb128(&mut header, directories_count as u64);
        // Directory entries
        header.extend_from_slice(directory_name);

        // File name entry format count
        header.push(file_entry_format_count);
        // File name entry format
        header.extend_from_slice(&file_entry_format);
        // File names count
        encode_uleb128(&mut header, file_names_count as u64);
        // File name entries
        header.extend_from_slice(&file_name_entry);

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
}

impl Default for DwarfBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Encoding helpers
// ---------------------------------------------------------------------------

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

/// Append DWARF5 debug sections to an ELF binary.
///
/// Given an existing ELF binary and the debug sections, this function inserts
/// the three debug sections (`.debug_abbrev`, `.debug_info`, `.debug_line`)
/// into the ELF, updates the section header table, and patches the section
/// header string table and section count.
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

    // The new sections will be appended right before the section header table.
    // We need to compute their file offsets.
    let new_sections_start = e_shoff as usize;

    // Compute aligned sizes for debug sections.
    let debug_abbrev_aligned = align_up(debug.debug_abbrev.len() as u64, 8) as usize;
    let debug_info_aligned = align_up(debug.debug_info.len() as u64, 8) as usize;
    let debug_line_aligned = align_up(debug.debug_line.len() as u64, 8) as usize;
    let new_shstrtab_aligned = align_up(new_shstrtab.len() as u64, 8) as usize;

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

    // Replace the old shstrtab with the new one.
    // We'll place the new shstrtab at the end of the debug sections.
    let new_shstrtab_file_offset = new_sections_start + new_section_data.len();
    new_section_data.extend_from_slice(&new_shstrtab);
    let pad = new_shstrtab_aligned - new_shstrtab.len();
    new_section_data.extend_from_slice(&vec![0u8; pad]);

    // Now insert the new section data before the section headers.
    // First, remove the old section headers from the end.
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

    // Add section headers for the three debug sections.
    let new_num_shdrs = e_shnum + 3;

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
        assert_eq!(version, DWARF_VERSION, "DWARF version should be 5");
    }

    // -- Test 3: Debug info compilation unit is present --
    #[test]
    fn test_debug_info_compile_unit_present() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // After the unit header (4 + 2 + 1 + 1 + 4 = 12 bytes), the first
        // DIE should use abbreviation code 1 (DW_TAG_COMPILE_UNIT).
        let header_size = 12;
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
        assert_eq!(version, DWARF_VERSION, "line program version should be 5");
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

        // Verify section count increased.
        let e_shnum = u16::from_le_bytes([elf[60], elf[61]]);
        assert_eq!(
            e_shnum, 11,
            "expected 11 section headers (8 original + 3 debug)"
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

    // -- Test 12: Debug info unit type is DW_UT_COMPILE --
    #[test]
    fn test_debug_info_unit_type() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // Header: unit_length(4) + version(2) + unit_type(1)
        assert!(info.len() > 7, "debug_info must have at least 7 bytes");
        assert_eq!(info[6], DW_UT_COMPILE, "unit type should be DW_UT_COMPILE");
    }

    // -- Test 13: Debug info address size is 8 --
    #[test]
    fn test_debug_info_address_size() {
        let db = make_simple_builder();
        let sections = db.emit_debug_sections();
        let info = &sections.debug_info;

        // Header: unit_length(4) + version(2) + unit_type(1) + address_size(1)
        assert!(info.len() > 8, "debug_info must have at least 8 bytes");
        assert_eq!(
            info[7], ADDRESS_SIZE,
            "address size should be 8 for AArch64"
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
}
