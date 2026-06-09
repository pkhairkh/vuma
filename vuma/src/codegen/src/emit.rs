//! # ARM64 Code Emission
//!
//! Lowers IR to ARM64 machine code and produces a minimal ELF binary suitable
//! for Linux on ARM64 (the Raspberry Pi 5 target).
//!
//! ## Pipeline
//!
//! 1. **IR → ARM64 Instructions**: Each IR instruction is pattern-matched and
//!    lowered to one or more [`Instruction`](crate::arm64::Instruction)
//!    values, with virtual registers replaced by physical registers from the
//!    register allocator.
//! 2. **Emit function**: Each IR function is emitted as a sequence of 32-bit
//!    ARM64 code words.
//! 3. **Emit program**: All functions are collected, data sections are laid
//!    out, and a minimal ELF64 binary is produced.
//!
//! ## ELF Layout
//!
//! ```text
//! ┌─────────────────┐
//! │ ELF Header       │  64 bytes
//! ├─────────────────┤
//! │ Program Headers   │  2 × 56 bytes (LOAD segments)
//! ├─────────────────┤
//! │ .text            │  emitted code
//! ├─────────────────┤
//! │ .rodata          │  read-only data
//! ├─────────────────┤
//! │ .data            │  initialized data
//! ├─────────────────┤
//! │ .bss             │  zero-initialized (virtual only)
//! └─────────────────┘
//! ```
//!
//! ## Limitations (TODO)
//!
//! - No relocations / symbol resolution for external references.
//! - No DWARF debug info.
//! - Branch offsets are placeholders; a link / fixup pass is needed.
//! - Only the Linux ABI is targeted.

use std::collections::HashMap;

use crate::arm64::{Instruction, Operand, Register};
use crate::ir::*;
use crate::regalloc::RegAllocator;
use crate::CodegenError;
use crate::Result;

// ---------------------------------------------------------------------------
// ELF Constants (ARM64 / AArch64)
// ---------------------------------------------------------------------------

/// ELF magic bytes.
const ELF_MAGIC: [u8; 4] = [0x7f, b'E', b'L', b'F'];

/// 64-bit ELF.
const ELFCLASS64: u8 = 2;

/// Little-endian data encoding.
const ELFDATA2LSB: u8 = 1;

/// ELF version.
const EV_CURRENT: u8 = 1;

/// Linux OS/ABI.
const ELFOSABI_LINUX: u8 = 3;

/// Machine type: AArch64.
const EM_AARCH64: u16 = 183;

/// ELF type: executable.
const ET_EXEC: u16 = 2;

/// Program header type: LOAD.
const PT_LOAD: u32 = 1;

/// Program header flags.
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

/// Default base address for the LOAD segment.
const BASE_ADDR: u64 = 0x400000;

// ---------------------------------------------------------------------------
// Emitter
// ---------------------------------------------------------------------------

/// The ARM64 code emitter.
///
/// Holds state for the current emission context: the register allocator,
/// accumulated code, and fixup records for branch targets.
pub struct Emitter {
    /// Register allocator used during emission.
    reg_alloc: RegAllocator,
    /// Accumulated machine code for the current function (32-bit words).
    code: Vec<u32>,
    /// Fixup records: (word index, target label name).
    fixups: Vec<(usize, String)>,
    /// Map from label name to code offset (in words).
    label_offsets: HashMap<String, usize>,
}

impl Emitter {
    /// Create a new emitter with a fresh register allocator.
    pub fn new() -> Self {
        Self {
            reg_alloc: RegAllocator::new(),
            code: Vec::new(),
            fixups: Vec::new(),
            label_offsets: HashMap::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Function emission
    // -----------------------------------------------------------------------

    /// Emit a single IR function to ARM64 machine code.
    ///
    /// Returns a vector of 32-bit ARM64 instruction words.
    pub fn emit_function(&mut self, func: &IRFunction) -> Result<Vec<u32>> {
        self.code.clear();
        self.fixups.clear();
        self.label_offsets.clear();
        self.reg_alloc.reset();

        // Allocate registers for parameters (AAPCS64: X0–X7).
        for (i, param) in func.params.iter().enumerate() {
            if let IRValue::Register(vreg_id) = param {
                if i < 8 {
                    let _arg_reg = match i {
                        0 => Register::X0,
                        1 => Register::X1,
                        2 => Register::X2,
                        3 => Register::X3,
                        4 => Register::X4,
                        5 => Register::X5,
                        6 => Register::X6,
                        7 => Register::X7,
                        _ => unreachable!(),
                    };
                    // Force the allocator to map this vreg to the ABI register.
                    // TODO: The current RegAllocator doesn't support pinning,
                    // so we just allocate normally.  A proper implementation
                    // would pin argument registers.
                    let _ = self.reg_alloc.allocate(*vreg_id);
                }
            }
        }

        // Emit prologue: STP X29, X30, [SP, #-16]!
        self.emit_instruction(Instruction::STP {
            rt1: Register::X29,
            rt2: Register::X30,
            rn: Register::SP,
            offset: -16,
        })?;

        // MOV X29, SP (set frame pointer)
        self.emit_instruction(Instruction::MOV {
            rd: Register::X29,
            rm: Register::SP,
        })?;

        // Compute total stack space needed for spills.
        // We'll do a pre-pass to count, but for now use a fixed 64-byte
        // frame.  TODO: compute from register allocator spill count.
        let stack_size = 64;
        // Subtract stack_size from SP (must be 16-byte aligned).
        let aligned_stack = ((stack_size + 15) / 16) * 16;
        self.emit_instruction(Instruction::SUB {
            rd: Register::SP,
            rn: Register::SP,
            rm: Operand::Imm12(aligned_stack as u16),
        })?;

        // Save callee-saved registers used by this function.
        // TODO: emit push of callee-saved regs based on reg_alloc state.

        // Emit each basic block.
        for block in &func.blocks {
            self.label_offsets.insert(block.label.clone(), self.code.len());
            self.emit_block(block)?;
        }

        // Apply fixups — resolve branch targets.
        self.apply_fixups()?;

        Ok(self.code.clone())
    }

    /// Emit instructions for a single IR basic block.
    fn emit_block(&mut self, block: &IRBlock) -> Result<()> {
        for instr in &block.instructions {
            self.emit_ir_instr(instr)?;
        }
        self.emit_terminator(&block.terminator)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // IR → ARM64 instruction lowering
    // -----------------------------------------------------------------------

    /// Lower a single IR instruction to ARM64 instructions.
    fn emit_ir_instr(&mut self, instr: &IRInstr) -> Result<()> {
        match instr {
            IRInstr::Load { dst, addr } => {
                let rt = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(addr)?;
                self.emit_instruction(Instruction::LDR {
                    rt,
                    rn,
                    offset: 0,
                })?;
            }

            IRInstr::Store { value, addr } => {
                let rt = self.resolve_reg(value)?;
                let rn = self.resolve_reg(addr)?;
                self.emit_instruction(Instruction::STR {
                    rt,
                    rn,
                    offset: 0,
                })?;
            }

            IRInstr::BinOp { op, dst, lhs, rhs } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(lhs)?;
                // Try to use immediate if rhs is an immediate.
                let rm = match rhs {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            Operand::Imm12(*v as u16)
                        } else {
                            // Load the immediate into a temp register.
                            let temp = Register::X9; // TODO: allocate temp
                            self.emit_load_immediate(temp, *v)?;
                            Operand::Reg { reg: temp, shift: None }
                        }
                    }
                    _ => Operand::Reg {
                        reg: self.resolve_reg(rhs)?,
                        shift: None,
                    },
                };

                let arm_instr = match op {
                    BinOpKind::Add => Instruction::ADD { rd, rn, rm },
                    BinOpKind::Sub => Instruction::SUB { rd, rn, rm },
                    BinOpKind::Mul => {
                        let rm_reg = self.operand_to_reg(&rm)?;
                        Instruction::MUL { rd, rn, rm: rm_reg }
                    }
                    BinOpKind::SDiv => {
                        let rm_reg = self.operand_to_reg(&rm)?;
                        Instruction::SDIV { rd, rn, rm: rm_reg }
                    }
                    BinOpKind::UDiv => {
                        let rm_reg = self.operand_to_reg(&rm)?;
                        Instruction::UDIV { rd, rn, rm: rm_reg }
                    }
                    BinOpKind::And => {
                        let rm_reg = self.operand_to_reg(&rm)?;
                        Instruction::AND { rd, rn, rm: rm_reg }
                    }
                    BinOpKind::Or => {
                        let rm_reg = self.operand_to_reg(&rm)?;
                        Instruction::ORR { rd, rn, rm: rm_reg }
                    }
                    BinOpKind::Xor => {
                        let rm_reg = self.operand_to_reg(&rm)?;
                        Instruction::EOR { rd, rn, rm: rm_reg }
                    }
                    BinOpKind::Shl => Instruction::LSL { rd, rn, rm },
                    BinOpKind::ShrL => Instruction::LSR { rd, rn, rm },
                    BinOpKind::ShrA => Instruction::ASR { rd, rn, rm },
                    BinOpKind::SRem | BinOpKind::URem => {
                        // Remainder: compute quotient, then multiply and subtract.
                        // TODO: implement MSUB for remainder.
                        let rm_reg = self.operand_to_reg(&rm)?;
                        let div_instr = if *op == BinOpKind::SRem {
                            Instruction::SDIV { rd, rn, rm: rm_reg }
                        } else {
                            Instruction::UDIV { rd, rn, rm: rm_reg }
                        };
                        self.emit_instruction(div_instr)?;
                        // MSUB: rd = rm * rd - rn  → but we need a temp.
                        // For now, use a placeholder.
                        // TODO: proper MSUB encoding
                        return Ok(());
                    }
                };
                self.emit_instruction(arm_instr)?;
            }

            IRInstr::UnaryOp { op, dst, operand } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(operand)?;
                match op {
                    UnaryOpKind::Neg => {
                        // NEG Rd, Rn = SUB Rd, XZR, Rn
                        self.emit_instruction(Instruction::SUB {
                            rd,
                            rn: Register::XZR,
                            rm: Operand::Reg { reg: rn, shift: None },
                        })?;
                    }
                    UnaryOpKind::Not => {
                        // MVN Rd, Rn = ORN Rd, XZR, Rn
                        // TODO: ORN encoding — use EOR with -1 for now
                        self.emit_load_immediate(Register::X9, -1)?;
                        self.emit_instruction(Instruction::EOR {
                            rd,
                            rn,
                            rm: Register::X9,
                        })?;
                    }
                    UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                        // TODO: CLZ / CTZ / POPCNT using ARM64 system hints or
                        // bit-manipulation instructions.
                        log::warn!("unary op {:?} not yet implemented, emitting MOV XZR placeholder", op);
                        self.emit_instruction(Instruction::MOV {
                            rd,
                            rm: Register::XZR,
                        })?;
                    }
                }
            }

            IRInstr::Call { dst, func: _, args } => {
                // Move arguments into X0–X7.
                for (i, arg) in args.iter().enumerate() {
                    if i >= 8 {
                        break; // TODO: stack arguments
                    }
                    let src = self.resolve_reg(arg)?;
                    let dst_reg = match i {
                        0 => Register::X0,
                        1 => Register::X1,
                        2 => Register::X2,
                        3 => Register::X3,
                        4 => Register::X4,
                        5 => Register::X5,
                        6 => Register::X6,
                        7 => Register::X7,
                        _ => unreachable!(),
                    };
                    if src != dst_reg {
                        self.emit_instruction(Instruction::MOV {
                            rd: dst_reg,
                            rm: src,
                        })?;
                    }
                }

                // BL to the function name (placeholder offset).
                // TODO: resolve function address — for now use offset 0.
                self.emit_instruction(Instruction::BL { offset: 0 })?;

                // Move return value (X0) to the destination, if any.
                if let Some(dst_val) = dst {
                    let rd = self.resolve_reg(dst_val)?;
                    if rd != Register::X0 {
                        self.emit_instruction(Instruction::MOV {
                            rd,
                            rm: Register::X0,
                        })?;
                    }
                }
            }

            IRInstr::Alloc { dst, size } => {
                // Subtract `size` (aligned to 16) from SP, put result in dst.
                let rd = self.resolve_reg(dst)?;
                self.emit_instruction(Instruction::MOV {
                    rd,
                    rm: Register::SP,
                })?;
                let aligned = ((*size as u32 + 15) / 16) * 16;
                self.emit_instruction(Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(aligned as u16),
                })?;
            }

            IRInstr::Free { ptr: _ } => {
                // TODO: restore stack pointer or call free()
                log::warn!("IRInstr::Free not yet implemented");
            }

            IRInstr::Cast { kind, dst, src } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(src)?;
                match kind {
                    CastKind::BitCast => {
                        // No actual instruction needed — same bits, different type.
                        if rd != rn {
                            self.emit_instruction(Instruction::MOV { rd, rm: rn })?;
                        }
                    }
                    CastKind::ZExt => {
                        // TODO: UBFM for zero-extension
                        if rd != rn {
                            self.emit_instruction(Instruction::MOV { rd, rm: rn })?;
                        }
                    }
                    CastKind::SExt => {
                        // TODO: SBFM for sign-extension
                        if rd != rn {
                            self.emit_instruction(Instruction::MOV { rd, rm: rn })?;
                        }
                    }
                    CastKind::Trunc => {
                        // TODO: SBFM for truncation
                        if rd != rn {
                            self.emit_instruction(Instruction::MOV { rd, rm: rn })?;
                        }
                    }
                }
            }

            IRInstr::Phi { .. } => {
                // Phi nodes should have been resolved by the SSA deconstruction
                // pass (not yet implemented).  Skip for now.
                log::warn!("IRInstr::Phi encountered during emission — should be resolved by SSA pass");
            }

            IRInstr::GetAddress { dst, name: _ } => {
                // TODO: ADRP + ADD for PC-relative addressing.
                let rd = self.resolve_reg(dst)?;
                // Placeholder: load 0.
                self.emit_instruction(Instruction::MOVZ {
                    rd,
                    imm16: 0,
                    shift: 0,
                })?;
            }

            IRInstr::Offset { dst, base, offset } => {
                let rd = self.resolve_reg(dst)?;
                let rn = self.resolve_reg(base)?;
                let rm = match offset {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            Operand::Imm12(*v as u16)
                        } else {
                            let temp = Register::X9;
                            self.emit_load_immediate(temp, *v)?;
                            Operand::Reg { reg: temp, shift: None }
                        }
                    }
                    _ => Operand::Reg {
                        reg: self.resolve_reg(offset)?,
                        shift: None,
                    },
                };
                self.emit_instruction(Instruction::ADD { rd, rn, rm })?;
            }
        }
        Ok(())
    }

    /// Emit the block terminator.
    fn emit_terminator(&mut self, term: &IRTerminator) -> Result<()> {
        match term {
            IRTerminator::Jump(target) => {
                let offset = 0; // placeholder — will be fixed up
                let fixup_idx = self.code.len();
                self.fixups.push((fixup_idx, target.clone()));
                self.emit_instruction(Instruction::B { offset })?;
            }
            IRTerminator::Branch {
                cond,
                true_block,
                false_block,
            } => {
                // Compare cond against zero, then branch.
                let rt = self.resolve_reg(cond)?;
                // CBZ to false_block (inverted), or CBNZ to true_block.
                // For now, emit CBZ to false and B to true.
                let fixup_cbz = self.code.len();
                self.fixups.push((fixup_cbz, false_block.clone()));
                self.emit_instruction(Instruction::CBNZ {
                    rt,
                    offset: 0, // placeholder
                })?;
                let fixup_b = self.code.len();
                self.fixups.push((fixup_b, true_block.clone()));
                self.emit_instruction(Instruction::B { offset: 0 })?;
            }
            IRTerminator::Return(vals) => {
                // Move return values into X0–X7.
                for (i, val) in vals.iter().enumerate() {
                    if i >= 8 {
                        break;
                    }
                    let src = self.resolve_reg(val)?;
                    let dst_reg = match i {
                        0 => Register::X0,
                        1 => Register::X1,
                        2 => Register::X2,
                        3 => Register::X3,
                        4 => Register::X4,
                        5 => Register::X5,
                        6 => Register::X6,
                        7 => Register::X7,
                        _ => unreachable!(),
                    };
                    if src != dst_reg {
                        self.emit_instruction(Instruction::MOV {
                            rd: dst_reg,
                            rm: src,
                        })?;
                    }
                }

                // Restore callee-saved registers and stack pointer.
                // TODO: emit proper epilogue based on saved registers.

                // ADD SP, SP, #stack_size
                self.emit_instruction(Instruction::ADD {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(64), // TODO: use actual frame size
                })?;

                // LDP X29, X30, [SP], #16
                self.emit_instruction(Instruction::LDP {
                    rt1: Register::X29,
                    rt2: Register::X30,
                    rn: Register::SP,
                    offset: 16,
                })?;

                // RET
                self.emit_instruction(Instruction::RET { rn: None })?;
            }
            IRTerminator::Unreachable => {
                // Emit a trap instruction: BRK #0x1
                // TODO: BRK encoding
                // For now emit a NOP (hint #0).
                self.emit_instruction(Instruction::MOV {
                    rd: Register::XZR,
                    rm: Register::XZR,
                })?;
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Push a single ARM64 instruction (already encoded) into the code
    /// buffer.
    fn emit_instruction(&mut self, instr: Instruction) -> Result<()> {
        let word = instr.encode()?;
        self.code.push(word);
        Ok(())
    }

    /// Resolve an IR value to a physical register.
    ///
    /// For `Register(id)` values, allocates a physical register.  For
    /// immediates, loads the value into a temporary register.
    fn resolve_reg(&mut self, val: &IRValue) -> Result<Register> {
        match val {
            IRValue::Register(id) => self.reg_alloc.allocate(*id),
            IRValue::Immediate(v) => {
                let temp = Register::X9; // TODO: proper temp allocation
                self.emit_load_immediate(temp, *v)?;
                Ok(temp)
            }
            IRValue::Address(addr) => {
                let temp = Register::X10;
                self.emit_load_immediate(temp, *addr as i64)?;
                Ok(temp)
            }
            IRValue::Label(_) => {
                // Labels should not appear as register operands.
                Err(CodegenError::EncodingError(
                    "label value cannot be resolved to a register".into(),
                ))
            }
        }
    }

    /// Emit a MOVZ/MOVK sequence to load a 64-bit immediate into a register.
    fn emit_load_immediate(&mut self, rd: Register, value: i64) -> Result<()> {
        // Handle the common short-circuit cases.
        if value >= 0 && value <= 65535 {
            self.emit_instruction(Instruction::MOVZ {
                rd,
                imm16: value as u16,
                shift: 0,
            })?;
            return Ok(());
        }

        if value >= 0 && value <= 0xFFFF_FFFF {
            // MOVZ + MOVK
            let lo = (value & 0xFFFF) as u16;
            let hi = ((value >> 16) & 0xFFFF) as u16;
            self.emit_instruction(Instruction::MOVZ {
                rd,
                imm16: lo,
                shift: 0,
            })?;
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: hi,
                shift: 16,
            })?;
            return Ok(());
        }

        // Full 64-bit: MOVZ + 3 × MOVK.
        let w0 = (value & 0xFFFF) as u16;
        let w1 = ((value >> 16) & 0xFFFF) as u16;
        let w2 = ((value >> 32) & 0xFFFF) as u16;
        let w3 = ((value >> 48) & 0xFFFF) as u16;

        self.emit_instruction(Instruction::MOVZ {
            rd,
            imm16: w0,
            shift: 0,
        })?;
        if w1 != 0 {
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: w1,
                shift: 16,
            })?;
        }
        if w2 != 0 {
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: w2,
                shift: 32,
            })?;
        }
        if w3 != 0 {
            self.emit_instruction(Instruction::MOVK {
                rd,
                imm16: w3,
                shift: 48,
            })?;
        }

        Ok(())
    }

    /// Extract a register from an operand (for instructions that require
    /// register-only operands).
    fn operand_to_reg(&self, op: &Operand) -> Result<Register> {
        match op {
            Operand::Reg { reg, shift: _ } => Ok(*reg),
            Operand::Imm12(_) => Err(CodegenError::EncodingError(
                "expected register operand, got immediate".into(),
            )),
        }
    }

    /// Apply branch fixups — resolve label references to code offsets.
    fn apply_fixups(&mut self) -> Result<()> {
        let fixups = std::mem::take(&mut self.fixups);
        for (word_idx, label) in &fixups {
            let target_offset = self.label_offsets.get(label).copied().unwrap_or(0);
            let current_offset = *word_idx;
            // Branch offset is in units of 4 bytes (instructions).
            let offset = (target_offset as i32) - (current_offset as i32);
            // Re-encode the instruction with the correct offset.
            // TODO: this is a simplification; proper fixup should patch only
            // the immediate field of the already-encoded word.
            let old_word = self.code[*word_idx];
            // For B: bits [25:0] are imm26
            // For CBZ/CBNZ: bits [23:5] are imm19
            // This is a rough patch — refine per instruction encoding.
            let patched = (old_word & !0x03FFFFFF) | ((offset & 0x03FFFFFF) as u32);
            self.code[*word_idx] = patched;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Program emission → ELF
    // -----------------------------------------------------------------------

    /// Emit an entire IR program as a minimal ELF binary for Linux/ARM64.
    ///
    /// The binary has:
    /// - One LOAD segment for `.text` (read + execute).
    /// - One LOAD segment for `.data` + `.rodata` + `.bss` (read + write).
    pub fn emit_program(&mut self, program: &IRProgram) -> Result<Vec<u8>> {
        // ---- Emit all functions ----
        let mut text_section: Vec<u8> = Vec::new();

        // Find the entry function (default: "main").
        let entry_func = program
            .functions
            .iter()
            .find(|f| f.name == "main")
            .or_else(|| program.functions.first());

        let mut function_offsets: HashMap<String, u64> = HashMap::new();

        for func in &program.functions {
            function_offsets.insert(func.name.clone(), text_section.len() as u64);
            let code = self.emit_function(func)?;
            for word in code {
                text_section.extend_from_slice(&word.to_le_bytes());
            }
        }

        let entry_point = entry_func
            .and_then(|f| function_offsets.get(&f.name).copied())
            .unwrap_or(0);

        // ---- Collect data sections ----
        let mut rodata_section: Vec<u8> = Vec::new();
        let mut data_section: Vec<u8> = Vec::new();
        let mut bss_size: u64 = 0;

        for ds in &program.data_sections {
            match ds.kind {
                DataSectionKind::ReadOnly => {
                    rodata_section.extend_from_slice(&ds.data);
                }
                DataSectionKind::Data => {
                    data_section.extend_from_slice(&ds.data);
                }
                DataSectionKind::Bss => {
                    bss_size += ds.data.len() as u64;
                    // Ensure alignment.
                    if ds.align > 1 {
                        let padding = (ds.align as u64 - (bss_size % ds.align as u64)) % ds.align as u64;
                        bss_size += padding;
                    }
                }
            }
        }

        // ---- Build ELF ----
        let elf_header_size: u64 = 64;
        let phdr_size: u64 = 56;
        let num_phdrs: u64 = 2; // text + data

        let headers_total = elf_header_size + phdr_size * num_phdrs;

        // Align text to 16 bytes for performance.
        let text_offset = headers_total;
        let text_size = text_section.len() as u64;
        let text_aligned = align_up(text_size, 16);

        // Data starts after text.
        let data_offset = text_offset + text_aligned;
        let rodata_size = rodata_section.len() as u64;
        let rwdata_size = data_section.len() as u64;
        let data_total = rodata_size + rwdata_size;

        // Virtual addresses.
        let text_vaddr = BASE_ADDR + text_offset;
        let data_vaddr = BASE_ADDR + data_offset;

        // ---- ELF Header ----
        let mut elf = Vec::new();

        // e_ident
        elf.extend_from_slice(&ELF_MAGIC);           // EI_MAG0-3
        elf.push(ELFCLASS64);                         // EI_CLASS
        elf.push(ELFDATA2LSB);                        // EI_DATA
        elf.push(EV_CURRENT);                         // EI_VERSION
        elf.push(ELFOSABI_LINUX);                     // EI_OSABI
        elf.push(0);                                  // EI_ABIVERSION
        elf.extend_from_slice(&[0u8; 7]);             // EI_PAD

        // e_type
        elf.extend_from_slice(&ET_EXEC.to_le_bytes());
        // e_machine
        elf.extend_from_slice(&EM_AARCH64.to_le_bytes());
        // e_version
        elf.extend_from_slice(&(1u32).to_le_bytes());
        // e_entry
        elf.extend_from_slice(&(BASE_ADDR + entry_point).to_le_bytes());
        // e_phoff
        elf.extend_from_slice(&elf_header_size.to_le_bytes());
        // e_shoff (0 — no section headers for minimal ELF)
        elf.extend_from_slice(&(0u64).to_le_bytes());
        // e_flags
        elf.extend_from_slice(&(0u32).to_le_bytes());
        // e_ehsize
        elf.extend_from_slice(&(64u16).to_le_bytes());
        // e_phentsize
        elf.extend_from_slice(&(56u16).to_le_bytes());
        // e_phnum
        elf.extend_from_slice(&(num_phdrs as u16).to_le_bytes());
        // e_shentsize
        elf.extend_from_slice(&(0u16).to_le_bytes());
        // e_shnum
        elf.extend_from_slice(&(0u16).to_le_bytes());
        // e_shstrndx
        elf.extend_from_slice(&(0u16).to_le_bytes());

        assert_eq!(elf.len(), 64, "ELF header must be exactly 64 bytes");

        // ---- Program Headers ----

        // Segment 1: .text (read + execute)
        self.write_phdr(
            &mut elf,
            PT_LOAD,
            PF_R | PF_X,
            text_offset,
            text_vaddr,
            text_vaddr,
            text_size,
            text_size,
        );

        // Segment 2: .data + .rodata + .bss (read + write)
        self.write_phdr(
            &mut elf,
            PT_LOAD,
            PF_R | PF_W,
            data_offset,
            data_vaddr,
            data_vaddr,
            data_total,
            data_total + bss_size,
        );

        assert_eq!(elf.len() as u64, headers_total);

        // ---- .text section ----
        elf.extend_from_slice(&text_section);
        // Pad to alignment boundary.
        let padding = text_aligned - text_size;
        elf.extend_from_slice(&vec![0u8; padding as usize]);

        // ---- .rodata + .data ----
        elf.extend_from_slice(&rodata_section);
        elf.extend_from_slice(&data_section);

        Ok(elf)
    }

    /// Write a 64-bit ELF program header.
    fn write_phdr(
        &self,
        buf: &mut Vec<u8>,
        p_type: u32,
        p_flags: u32,
        p_offset: u64,
        p_vaddr: u64,
        p_paddr: u64,
        p_filesz: u64,
        p_memsz: u64,
    ) {
        buf.extend_from_slice(&p_type.to_le_bytes());
        buf.extend_from_slice(&p_flags.to_le_bytes());
        buf.extend_from_slice(&p_offset.to_le_bytes());
        buf.extend_from_slice(&p_vaddr.to_le_bytes());
        buf.extend_from_slice(&p_paddr.to_le_bytes());
        buf.extend_from_slice(&p_filesz.to_le_bytes());
        buf.extend_from_slice(&p_memsz.to_le_bytes());
        // p_align
        buf.extend_from_slice(&(0x1000u64).to_le_bytes());
    }
}

impl Default for Emitter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

/// Round `value` up to the nearest multiple of `alignment`.
fn align_up(value: u64, alignment: u64) -> u64 {
    ((value + alignment - 1) / alignment) * alignment
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_simple_function() {
        let mut func = IRFunction::new("main");
        func.current_block().terminator = IRTerminator::Return(vec![]);

        let mut emitter = Emitter::new();
        let code = emitter.emit_function(&func).unwrap();
        // Should have at least prologue + epilogue instructions.
        assert!(!code.is_empty());
    }

    #[test]
    fn emit_program_elf_header() {
        let mut func = IRFunction::new("main");
        func.current_block().terminator = IRTerminator::Return(vec![]);

        let program = IRProgram {
            functions: vec![func],
            data_sections: vec![],
        };

        let mut emitter = Emitter::new();
        let elf = emitter.emit_program(&program).unwrap();

        // Verify ELF magic.
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // Verify 64-bit.
        assert_eq!(elf[4], ELFCLASS64);
        // Verify little-endian.
        assert_eq!(elf[5], ELFDATA2LSB);
        // Verify AArch64.
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, EM_AARCH64);
    }

    #[test]
    fn load_immediate_short() {
        let mut emitter = Emitter::new();
        let mut func = IRFunction::new("_test");
        func.current_block().terminator = IRTerminator::Return(vec![]);

        // Use emit_function to set up context.
        let _ = emitter.emit_function(&func);
        emitter.code.clear();

        // Now test emit_load_immediate directly.
        emitter.emit_load_immediate(Register::X0, 42).unwrap();
        assert_eq!(emitter.code.len(), 1); // single MOVZ
    }
}
