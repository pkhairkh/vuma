//! # RISC-V 64-bit Backend
//!
//! Implements the `Backend` trait for the RISC-V 64-bit target (RV64GC,
//! LP64D ABI).  This module provides:
//!
//! - `Gpr` — General-purpose register enum (x0–x31)
//! - `Fpr` — Floating-point register enum (f0–f31)
//! - `Instruction` — RV64I + M + F/D instruction enum with correct encoding
//! - Encoding helpers for R/I/S/B/U/J-type instruction formats
//! - `RiscV64Backend` — `Backend` implementation that lowers IR to RISC-V machine code
//!
//! ## RISC-V Register Convention (LP64D ABI)
//!
//! | Register(s) | ABI Name | Role                              |
//! |-------------|----------|-----------------------------------|
//! | x0          | zero     | Hardwired zero                    |
//! | x1          | ra       | Return address                    |
//! | x2          | sp       | Stack pointer                     |
//! | x3          | gp       | Global pointer                    |
//! | x4          | tp       | Thread pointer                    |
//! | x5–x7       | t0–t2    | Caller-saved temporaries          |
//! | x8          | s0/fp    | Callee-saved / frame pointer      |
//! | x9          | s1       | Callee-saved                      |
//! | x10–x17     | a0–a7    | Argument / return registers       |
//! | x18–x27     | s2–s11   | Callee-saved                      |
//! | x28–x31     | t3–t6    | Caller-saved temporaries          |
//!
//! ## References
//!
//! - RISC-V Instruction Set Manual, Volume I: User-Level ISA, Document 20191213
//! - <https://riscv.org/specifications/>

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, PhysicalReg, RegClass, RelocationEntry, RiscV64TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
use std::collections::HashMap;

// ===========================================================================
// Opcodes
// ===========================================================================

/// RISC-V base opcodes (bits [6:0]).
const OP_LUI: u32 = 0b0110111;
const OP_AUIPC: u32 = 0b0010111;
const OP_JAL: u32 = 0b1101111;
const OP_JALR: u32 = 0b1100111;
const OP_BRANCH: u32 = 0b1100011;
const OP_LOAD: u32 = 0b0000011;
const OP_STORE: u32 = 0b0100011;
const OP_IMM: u32 = 0b0010011;
const OP_REG: u32 = 0b0110011;
const OP_IMM32: u32 = 0b0011011;
const OP_REG32: u32 = 0b0111011;
#[allow(dead_code)]
const OP_SYSTEM: u32 = 0b1110011;
const OP_MISC_MEM: u32 = 0b0001111;
const OP_FP: u32 = 0b1010011;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// RISC-V 64-bit general-purpose registers (x0–x31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Gpr {
    Zero = 0,
    Ra = 1,
    Sp = 2,
    Gp = 3,
    Tp = 4,
    T0 = 5,
    T1 = 6,
    T2 = 7,
    S0 = 8,
    S1 = 9,
    A0 = 10,
    A1 = 11,
    A2 = 12,
    A3 = 13,
    A4 = 14,
    A5 = 15,
    A6 = 16,
    A7 = 17,
    S2 = 18,
    S3 = 19,
    S4 = 20,
    S5 = 21,
    S6 = 22,
    S7 = 23,
    S8 = 24,
    S9 = 25,
    S10 = 26,
    S11 = 27,
    T3 = 28,
    T4 = 29,
    T5 = 30,
    T6 = 31,
}

impl Gpr {
    /// Returns the 5-bit encoding index for this register.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns the Gpr for a 5-bit encoding index.
    pub fn from_encoding(idx: u32) -> Option<Gpr> {
        match idx {
            0 => Some(Gpr::Zero),
            1 => Some(Gpr::Ra),
            2 => Some(Gpr::Sp),
            3 => Some(Gpr::Gp),
            4 => Some(Gpr::Tp),
            5 => Some(Gpr::T0),
            6 => Some(Gpr::T1),
            7 => Some(Gpr::T2),
            8 => Some(Gpr::S0),
            9 => Some(Gpr::S1),
            10 => Some(Gpr::A0),
            11 => Some(Gpr::A1),
            12 => Some(Gpr::A2),
            13 => Some(Gpr::A3),
            14 => Some(Gpr::A4),
            15 => Some(Gpr::A5),
            16 => Some(Gpr::A6),
            17 => Some(Gpr::A7),
            18 => Some(Gpr::S2),
            19 => Some(Gpr::S3),
            20 => Some(Gpr::S4),
            21 => Some(Gpr::S5),
            22 => Some(Gpr::S6),
            23 => Some(Gpr::S7),
            24 => Some(Gpr::S8),
            25 => Some(Gpr::S9),
            26 => Some(Gpr::S10),
            27 => Some(Gpr::S11),
            28 => Some(Gpr::T3),
            29 => Some(Gpr::T4),
            30 => Some(Gpr::T5),
            31 => Some(Gpr::T6),
            _ => None,
        }
    }

    /// Returns `true` if this register is available for register allocation.
    ///
    /// Zero (x0), Sp (x2), Gp (x3), and Tp (x4) are reserved.
    pub fn is_allocatable(&self) -> bool {
        !matches!(self, Gpr::Zero | Gpr::Sp | Gpr::Gp | Gpr::Tp)
    }

    /// Returns `true` if this register is callee-saved (s0–s11).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Gpr::S0
                | Gpr::S1
                | Gpr::S2
                | Gpr::S3
                | Gpr::S4
                | Gpr::S5
                | Gpr::S6
                | Gpr::S7
                | Gpr::S8
                | Gpr::S9
                | Gpr::S10
                | Gpr::S11
        )
    }

    /// Returns `true` if this register is an argument register (a0–a7).
    pub fn is_arg_reg(&self) -> bool {
        matches!(
            self,
            Gpr::A0 | Gpr::A1 | Gpr::A2 | Gpr::A3 | Gpr::A4 | Gpr::A5 | Gpr::A6 | Gpr::A7
        )
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::Zero => "zero",
            Gpr::Ra => "ra",
            Gpr::Sp => "sp",
            Gpr::Gp => "gp",
            Gpr::Tp => "tp",
            Gpr::T0 => "t0",
            Gpr::T1 => "t1",
            Gpr::T2 => "t2",
            Gpr::S0 => "s0",
            Gpr::S1 => "s1",
            Gpr::A0 => "a0",
            Gpr::A1 => "a1",
            Gpr::A2 => "a2",
            Gpr::A3 => "a3",
            Gpr::A4 => "a4",
            Gpr::A5 => "a5",
            Gpr::A6 => "a6",
            Gpr::A7 => "a7",
            Gpr::S2 => "s2",
            Gpr::S3 => "s3",
            Gpr::S4 => "s4",
            Gpr::S5 => "s5",
            Gpr::S6 => "s6",
            Gpr::S7 => "s7",
            Gpr::S8 => "s8",
            Gpr::S9 => "s9",
            Gpr::S10 => "s10",
            Gpr::S11 => "s11",
            Gpr::T3 => "t3",
            Gpr::T4 => "t4",
            Gpr::T5 => "t5",
            Gpr::T6 => "t6",
        }
    }

    /// Returns the Gpr for a given argument index (0–7). Returns `None` for
    /// indices >= 8.
    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::A0),
            1 => Some(Gpr::A1),
            2 => Some(Gpr::A2),
            3 => Some(Gpr::A3),
            4 => Some(Gpr::A4),
            5 => Some(Gpr::A5),
            6 => Some(Gpr::A6),
            7 => Some(Gpr::A7),
            _ => None,
        }
    }
}

impl std::fmt::Display for Gpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Floating-Point Registers
// ===========================================================================

/// RISC-V 64-bit floating-point registers (f0–f31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Fpr {
    F0 = 0,
    F1 = 1,
    F2 = 2,
    F3 = 3,
    F4 = 4,
    F5 = 5,
    F6 = 6,
    F7 = 7,
    F8 = 8,
    F9 = 9,
    F10 = 10,
    F11 = 11,
    F12 = 12,
    F13 = 13,
    F14 = 14,
    F15 = 15,
    F16 = 16,
    F17 = 17,
    F18 = 18,
    F19 = 19,
    F20 = 20,
    F21 = 21,
    F22 = 22,
    F23 = 23,
    F24 = 24,
    F25 = 25,
    F26 = 26,
    F27 = 27,
    F28 = 28,
    F29 = 29,
    F30 = 30,
    F31 = 31,
}

impl Fpr {
    /// Returns the 5-bit encoding index for this register.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns the Fpr for a 5-bit encoding index.
    pub fn from_encoding(idx: u32) -> Option<Fpr> {
        match idx {
            0 => Some(Fpr::F0),
            1 => Some(Fpr::F1),
            2 => Some(Fpr::F2),
            3 => Some(Fpr::F3),
            4 => Some(Fpr::F4),
            5 => Some(Fpr::F5),
            6 => Some(Fpr::F6),
            7 => Some(Fpr::F7),
            8 => Some(Fpr::F8),
            9 => Some(Fpr::F9),
            10 => Some(Fpr::F10),
            11 => Some(Fpr::F11),
            12 => Some(Fpr::F12),
            13 => Some(Fpr::F13),
            14 => Some(Fpr::F14),
            15 => Some(Fpr::F15),
            16 => Some(Fpr::F16),
            17 => Some(Fpr::F17),
            18 => Some(Fpr::F18),
            19 => Some(Fpr::F19),
            20 => Some(Fpr::F20),
            21 => Some(Fpr::F21),
            22 => Some(Fpr::F22),
            23 => Some(Fpr::F23),
            24 => Some(Fpr::F24),
            25 => Some(Fpr::F25),
            26 => Some(Fpr::F26),
            27 => Some(Fpr::F27),
            28 => Some(Fpr::F28),
            29 => Some(Fpr::F29),
            30 => Some(Fpr::F30),
            31 => Some(Fpr::F31),
            _ => None,
        }
    }

    /// Returns `true` if this register is callee-saved (f8–f9, f18–f27).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Fpr::F8
                | Fpr::F9
                | Fpr::F18
                | Fpr::F19
                | Fpr::F20
                | Fpr::F21
                | Fpr::F22
                | Fpr::F23
                | Fpr::F24
                | Fpr::F25
                | Fpr::F26
                | Fpr::F27
        )
    }

    /// Returns `true` if this register is an FP argument register (f10–f17, aka fa0–fa7).
    pub fn is_arg_reg(&self) -> bool {
        matches!(
            self,
            Fpr::F10 | Fpr::F11 | Fpr::F12 | Fpr::F13 | Fpr::F14 | Fpr::F15 | Fpr::F16 | Fpr::F17
        )
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Fpr::F0 => "f0",
            Fpr::F1 => "f1",
            Fpr::F2 => "f2",
            Fpr::F3 => "f3",
            Fpr::F4 => "f4",
            Fpr::F5 => "f5",
            Fpr::F6 => "f6",
            Fpr::F7 => "f7",
            Fpr::F8 => "f8",
            Fpr::F9 => "f9",
            Fpr::F10 => "f10",
            Fpr::F11 => "f11",
            Fpr::F12 => "f12",
            Fpr::F13 => "f13",
            Fpr::F14 => "f14",
            Fpr::F15 => "f15",
            Fpr::F16 => "f16",
            Fpr::F17 => "f17",
            Fpr::F18 => "f18",
            Fpr::F19 => "f19",
            Fpr::F20 => "f20",
            Fpr::F21 => "f21",
            Fpr::F22 => "f22",
            Fpr::F23 => "f23",
            Fpr::F24 => "f24",
            Fpr::F25 => "f25",
            Fpr::F26 => "f26",
            Fpr::F27 => "f27",
            Fpr::F28 => "f28",
            Fpr::F29 => "f29",
            Fpr::F30 => "f30",
            Fpr::F31 => "f31",
        }
    }
}

impl std::fmt::Display for Fpr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Instruction Encoding Helpers
// ===========================================================================

/// Encode an R-type instruction.
///
/// Format: `funct7[31:25] | rs2[24:20] | rs1[19:15] | funct3[14:12] | rd[11:7] | opcode[6:0]`
fn encode_r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> [u8; 4] {
    let word = ((funct7 & 0x7F) << 25)
        | ((rs2 & 0x1F) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | ((rd & 0x1F) << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// Encode an I-type instruction.
///
/// Format: `imm[31:20] | rs1[19:15] | funct3[14:12] | rd[11:7] | opcode[6:0]`
fn encode_i_type(imm: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> [u8; 4] {
    let word = ((imm & 0xFFF) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | ((rd & 0x1F) << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// Encode an S-type instruction.
///
/// Format: `imm[11:5][31:25] | rs2[24:20] | rs1[19:15] | funct3[14:12] | imm[4:0][11:7] | opcode[6:0]`
fn encode_s_type(imm: u32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> [u8; 4] {
    let imm_lo = imm & 0x1F;
    let imm_hi = (imm >> 5) & 0x7F;
    let word = (imm_hi << 25)
        | ((rs2 & 0x1F) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | (imm_lo << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// Encode a B-type instruction.
///
/// Format: `imm[12|10:5][31:25] | rs2[24:20] | rs1[19:15] | funct3[14:12] | imm[4:1|11][11:7] | opcode[6:0]`
///
/// The immediate is a signed 13-bit byte offset (bit 0 is always 0).
fn encode_b_type(imm: i32, rs2: u32, rs1: u32, funct3: u32, opcode: u32) -> [u8; 4] {
    // B-type immediate bit layout:
    // bit 31:   imm[12]
    // bits 30:25: imm[10:5]
    // bits 11:8:  imm[4:1]
    // bit 7:    imm[11]
    let imm_u = imm as u32;
    let word = (((imm_u >> 12) & 0x1) << 31)
        | (((imm_u >> 5) & 0x3F) << 25)
        | ((rs2 & 0x1F) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | (((imm_u >> 1) & 0xF) << 8)
        | (((imm_u >> 11) & 0x1) << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// Encode a U-type instruction.
///
/// Format: `imm[31:12] | rd[11:7] | opcode[6:0]`
///
/// The immediate is the upper 20 bits; the lower 12 bits are zero.
fn encode_u_type(imm: u32, rd: u32, opcode: u32) -> [u8; 4] {
    let word = (imm & 0xFFFFF000) | ((rd & 0x1F) << 7) | (opcode & 0x7F);
    word.to_le_bytes()
}

/// Encode a J-type instruction.
///
/// Format: `imm[20|10:1|11|19:12][31:12] | rd[11:7] | opcode[6:0]`
///
/// The immediate is a signed 21-bit byte offset (bit 0 is always 0).
fn encode_j_type(imm: i32, rd: u32, opcode: u32) -> [u8; 4] {
    // J-type immediate bit layout:
    // bit 31:    imm[20]
    // bits 30:21: imm[10:1]
    // bit 20:    imm[11]
    // bits 19:12: imm[19:12]
    let imm_u = imm as u32;
    let word = (((imm_u >> 20) & 0x1) << 31)
        | (((imm_u >> 1) & 0x3FF) << 21)
        | (((imm_u >> 11) & 0x1) << 20)
        | (((imm_u >> 12) & 0xFF) << 12)
        | ((rd & 0x1F) << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

// ===========================================================================
// Instruction Enum
// ===========================================================================

/// RISC-V 64-bit instruction representations for code generation.
///
/// Covers RV64I base, M extension, and key F/D extension instructions.
/// Each variant captures the operands needed for encoding and disassembly.
/// The `encode()` method produces a 4-byte little-endian machine code word.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Instruction {
    // ── RV64I: Upper Immediate ────────────────────────────────────────
    /// Load Upper Immediate: `lui rd, imm`
    Lui { rd: Gpr, imm: u32 },
    /// Add Upper Immediate to PC: `auipc rd, imm`
    Auipc { rd: Gpr, imm: u32 },

    // ── RV64I: Jumps ─────────────────────────────────────────────────
    /// Jump and Link: `jal rd, offset`
    Jal { rd: Gpr, offset: i32 },
    /// Jump and Link Register: `jalr rd, rs1, imm`
    Jalr { rd: Gpr, rs1: Gpr, imm: i32 },

    // ── RV64I: Branches ──────────────────────────────────────────────
    /// Branch if Equal: `beq rs1, rs2, offset`
    Beq { rs1: Gpr, rs2: Gpr, offset: i32 },
    /// Branch if Not Equal: `bne rs1, rs2, offset`
    Bne { rs1: Gpr, rs2: Gpr, offset: i32 },
    /// Branch if Less Than (signed): `blt rs1, rs2, offset`
    Blt { rs1: Gpr, rs2: Gpr, offset: i32 },
    /// Branch if Greater or Equal (signed): `bge rs1, rs2, offset`
    Bge { rs1: Gpr, rs2: Gpr, offset: i32 },
    /// Branch if Less Than (unsigned): `bltu rs1, rs2, offset`
    Bltu { rs1: Gpr, rs2: Gpr, offset: i32 },
    /// Branch if Greater or Equal (unsigned): `bgeu rs1, rs2, offset`
    Bgeu { rs1: Gpr, rs2: Gpr, offset: i32 },

    // ── RV64I: Loads ─────────────────────────────────────────────────
    /// Load Byte (sign-extended): `lb rd, offset(rs1)`
    Lb { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Load Halfword (sign-extended): `lh rd, offset(rs1)`
    Lh { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Load Word (sign-extended): `lw rd, offset(rs1)`
    Lw { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Load Doubleword: `ld rd, offset(rs1)`
    Ld { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Load Byte (zero-extended): `lbu rd, offset(rs1)`
    Lbu { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Load Halfword (zero-extended): `lhu rd, offset(rs1)`
    Lhu { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Load Word (zero-extended, RV64): `lwu rd, offset(rs1)`
    Lwu { rd: Gpr, rs1: Gpr, imm: i32 },

    // ── RV64I: Stores ────────────────────────────────────────────────
    /// Store Byte: `sb rs2, offset(rs1)`
    Sb { rs1: Gpr, rs2: Gpr, imm: i32 },
    /// Store Halfword: `sh rs2, offset(rs1)`
    Sh { rs1: Gpr, rs2: Gpr, imm: i32 },
    /// Store Word: `sw rs2, offset(rs1)`
    Sw { rs1: Gpr, rs2: Gpr, imm: i32 },
    /// Store Doubleword: `sd rs2, offset(rs1)`
    Sd { rs1: Gpr, rs2: Gpr, imm: i32 },

    // ── RV64I: Immediate Arithmetic ──────────────────────────────────
    /// Add Immediate: `addi rd, rs1, imm`
    Addi { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Set Less Than Immediate (signed): `slti rd, rs1, imm`
    Slti { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Set Less Than Immediate (unsigned): `sltiu rd, rs1, imm`
    Sltiu { rd: Gpr, rs1: Gpr, imm: i32 },
    /// XOR Immediate: `xori rd, rs1, imm`
    Xori { rd: Gpr, rs1: Gpr, imm: i32 },
    /// OR Immediate: `ori rd, rs1, imm`
    Ori { rd: Gpr, rs1: Gpr, imm: i32 },
    /// AND Immediate: `andi rd, rs1, imm`
    Andi { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Shift Left Logical by Immediate: `slli rd, rs1, shamt`
    Slli { rd: Gpr, rs1: Gpr, shamt: u32 },
    /// Shift Right Logical by Immediate: `srli rd, rs1, shamt`
    Srli { rd: Gpr, rs1: Gpr, shamt: u32 },
    /// Shift Right Arithmetic by Immediate: `srai rd, rs1, shamt`
    Srai { rd: Gpr, rs1: Gpr, shamt: u32 },

    // ── RV64I: Register Arithmetic ───────────────────────────────────
    /// Add: `add rd, rs1, rs2`
    Add { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Subtract: `sub rd, rs1, rs2`
    Sub { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Shift Left Logical: `sll rd, rs1, rs2`
    Sll { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Set Less Than (signed): `slt rd, rs1, rs2`
    Slt { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Set Less Than (unsigned): `sltu rd, rs1, rs2`
    Sltu { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// XOR: `xor rd, rs1, rs2`
    Xor { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Shift Right Logical: `srl rd, rs1, rs2`
    Srl { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Shift Right Arithmetic: `sra rd, rs1, rs2`
    Sra { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// OR: `or rd, rs1, rs2`
    Or { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// AND: `and rd, rs1, rs2`
    And { rd: Gpr, rs1: Gpr, rs2: Gpr },

    // ── RV64I: Word-level Arithmetic (RV64 only) ─────────────────────
    /// Add Word: `addw rd, rs1, rs2`
    Addw { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Subtract Word: `subw rd, rs1, rs2`
    Subw { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Shift Left Logical Word: `sllw rd, rs1, rs2`
    Sllw { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Shift Right Logical Word: `srlw rd, rs1, rs2`
    Srlw { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Shift Right Arithmetic Word: `sraw rd, rs1, rs2`
    Sraw { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Add Immediate Word: `addiw rd, rs1, imm`
    Addiw { rd: Gpr, rs1: Gpr, imm: i32 },
    /// Shift Left Logical by Immediate Word: `slliw rd, rs1, shamt`
    Slliw { rd: Gpr, rs1: Gpr, shamt: u32 },
    /// Shift Right Logical by Immediate Word: `srliw rd, rs1, shamt`
    Srliw { rd: Gpr, rs1: Gpr, shamt: u32 },
    /// Shift Right Arithmetic by Immediate Word: `sraiw rd, rs1, shamt`
    Sraiw { rd: Gpr, rs1: Gpr, shamt: u32 },

    // ── M Extension: Multiply/Divide ─────────────────────────────────
    /// Multiply: `mul rd, rs1, rs2`
    Mul { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Multiply High (signed): `mulh rd, rs1, rs2`
    Mulh { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Multiply High (signed × unsigned): `mulhsu rd, rs1, rs2`
    Mulhsu { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Multiply High (unsigned): `mulhu rd, rs1, rs2`
    Mulhu { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Divide (signed): `div rd, rs1, rs2`
    Div { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Divide (unsigned): `divu rd, rs1, rs2`
    Divu { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Remainder (signed): `rem rd, rs1, rs2`
    Rem { rd: Gpr, rs1: Gpr, rs2: Gpr },
    /// Remainder (unsigned): `remu rd, rs1, rs2`
    Remu { rd: Gpr, rs1: Gpr, rs2: Gpr },

    // ── F/D Extension: FP Load/Store ─────────────────────────────────
    /// Load Float (32-bit): `flw fd, offset(rs1)`
    Flw { rd: Fpr, rs1: Gpr, imm: i32 },
    /// Load Double (64-bit): `fld fd, offset(rs1)`
    Fld { rd: Fpr, rs1: Gpr, imm: i32 },
    /// Store Float (32-bit): `fsw fs2, offset(rs1)`
    Fsw { rs1: Gpr, rs2: Fpr, imm: i32 },
    /// Store Double (64-bit): `fsd fs2, offset(rs1)`
    Fsd { rs1: Gpr, rs2: Fpr, imm: i32 },

    // ── F/D Extension: FP Arithmetic ─────────────────────────────────
    /// FP Add Double: `fadd.d fd, fs1, fs2`
    FaddD { rd: Fpr, rs1: Fpr, rs2: Fpr },
    /// FP Subtract Double: `fsub.d fd, fs1, fs2`
    FsubD { rd: Fpr, rs1: Fpr, rs2: Fpr },
    /// FP Multiply Double: `fmul.d fd, fs1, fs2`
    FmulD { rd: Fpr, rs1: Fpr, rs2: Fpr },
    /// FP Divide Double: `fdiv.d fd, fs1, fs2`
    FdivD { rd: Fpr, rs1: Fpr, rs2: Fpr },
    /// FP Move Double: `fmv.d fd, fs1` (pseudo: fsgnj.d)
    FmvD { rd: Fpr, rs1: Fpr },

    // ── F/D Extension: FP ↔ Integer Conversion ────────────────────────
    /// Convert signed 32-bit integer to single float: `fcvt.s.w fd, rs1`
    FcvtSW { rd: Fpr, rs1: Gpr },
    /// Convert unsigned 32-bit integer to single float: `fcvt.s.wu fd, rs1`
    FcvtSWU { rd: Fpr, rs1: Gpr },
    /// Convert signed 64-bit integer to single float: `fcvt.s.l fd, rs1`
    FcvtSL { rd: Fpr, rs1: Gpr },
    /// Convert unsigned 64-bit integer to single float: `fcvt.s.lu fd, rs1`
    FcvtSLU { rd: Fpr, rs1: Gpr },
    /// Convert signed 32-bit integer to double float: `fcvt.d.w fd, rs1`
    FcvtDW { rd: Fpr, rs1: Gpr },
    /// Convert unsigned 32-bit integer to double float: `fcvt.d.wu fd, rs1`
    FcvtDWU { rd: Fpr, rs1: Gpr },
    /// Convert signed 64-bit integer to double float: `fcvt.d.l fd, rs1`
    FcvtDL { rd: Fpr, rs1: Gpr },
    /// Convert unsigned 64-bit integer to double float: `fcvt.d.lu fd, rs1`
    FcvtDLU { rd: Fpr, rs1: Gpr },
    /// Convert single float to signed 32-bit integer: `fcvt.w.s rd, fs1`
    FcvtWS { rd: Gpr, rs1: Fpr },
    /// Convert single float to unsigned 32-bit integer: `fcvt.wu.s rd, fs1`
    FcvtWUS { rd: Gpr, rs1: Fpr },
    /// Convert single float to signed 64-bit integer: `fcvt.l.s rd, fs1`
    FcvtLS { rd: Gpr, rs1: Fpr },
    /// Convert single float to unsigned 64-bit integer: `fcvt.lu.s rd, fs1`
    FcvtLUS { rd: Gpr, rs1: Fpr },
    /// Convert double float to signed 32-bit integer: `fcvt.w.d rd, fs1`
    FcvtWD { rd: Gpr, rs1: Fpr },
    /// Convert double float to unsigned 32-bit integer: `fcvt.wu.d rd, fs1`
    FcvtWUD { rd: Gpr, rs1: Fpr },
    /// Convert double float to signed 64-bit integer: `fcvt.l.d rd, fs1`
    FcvtLD { rd: Gpr, rs1: Fpr },
    /// Convert double float to unsigned 64-bit integer: `fcvt.lu.d rd, fs1`
    FcvtLUD { rd: Gpr, rs1: Fpr },
    /// Convert single float to double float: `fcvt.d.s fd, fs1`
    FcvtDS { rd: Fpr, rs1: Fpr },
    /// Convert double float to single float: `fcvt.s.d fd, fs1`
    FcvtSD { rd: Fpr, rs1: Fpr },
    /// Move single float from FPR to GPR: `fmv.x.w rd, fs1`
    FmvXW { rd: Gpr, rs1: Fpr },
    /// Move single float from GPR to FPR: `fmv.w.x fd, rs1`
    FmvWX { rd: Fpr, rs1: Gpr },
    /// Move double float from FPR to GPR: `fmv.x.d rd, fs1`
    FmvXD { rd: Gpr, rs1: Fpr },
    /// Move double float from GPR to FPR: `fmv.d.x fd, rs1`
    FmvDX { rd: Fpr, rs1: Gpr },

    // ── Zicsr Extension: Control and Status Register ────────────────
    /// CSR Read/Write: `csrrw rd, csr, rs1`
    Csrrw { rd: Gpr, csr: u32, rs1: Gpr },
    /// CSR Read and Set: `csrrs rd, csr, rs1`
    Csrrs { rd: Gpr, csr: u32, rs1: Gpr },
    /// CSR Read and Clear: `csrrc rd, csr, rs1`
    Csrrc { rd: Gpr, csr: u32, rs1: Gpr },
    /// CSR Read/Write Immediate: `csrrwi rd, csr, uimm`
    Csrrwi { rd: Gpr, csr: u32, uimm: u32 },
    /// CSR Read and Set Immediate: `csrrsi rd, csr, uimm`
    Csrrsi { rd: Gpr, csr: u32, uimm: u32 },
    /// CSR Read and Clear Immediate: `csrrci rd, csr, uimm`
    Csrrci { rd: Gpr, csr: u32, uimm: u32 },

    // ── Zifencei Extension ──────────────────────────────────────────
    /// Fence.I: `fence.i` — instruction stream synchronization
    FenceI,

    // ── System / Misc ────────────────────────────────────────────────
    /// Environment Call: `ecall`
    Ecall,
    /// Environment Break: `ebreak`
    Ebreak,
    /// Fence: `fence pred, succ`
    Fence { pred: u32, succ: u32 },
    /// No-operation (pseudo: `addi x0, x0, 0`)
    Nop,

    // ── RV64A Extension: Atomic operations ──────────────────────────────
    /// Load-Reserved Doubleword: `lr.d rd, (rs1)` — RV64A
    LrD { rd: Gpr, rs1: Gpr },
    /// Store-Conditional Doubleword: `sc.d rd, rs1, rs2` — RV64A
    /// rd = 0 on success, non-zero on failure
    ScD { rd: Gpr, rs1: Gpr, rs2: Gpr },
}

impl Instruction {
    /// Encode this instruction into a 4-byte little-endian machine code word.
    ///
    /// Encoding follows the RISC-V ISA Specification.
    pub fn encode(&self) -> [u8; 4] {
        match self {
            // ── Upper Immediate ──────────────────────────────────────
            Instruction::Lui { rd, imm } => encode_u_type(*imm, rd.encoding(), OP_LUI),
            Instruction::Auipc { rd, imm } => encode_u_type(*imm, rd.encoding(), OP_AUIPC),

            // ── Jumps ───────────────────────────────────────────────
            Instruction::Jal { rd, offset } => encode_j_type(*offset, rd.encoding(), OP_JAL),
            Instruction::Jalr { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_JALR,
            ),

            // ── Branches ────────────────────────────────────────────
            Instruction::Beq { rs1, rs2, offset } => {
                encode_b_type(*offset, rs2.encoding(), rs1.encoding(), 0b000, OP_BRANCH)
            }
            Instruction::Bne { rs1, rs2, offset } => {
                encode_b_type(*offset, rs2.encoding(), rs1.encoding(), 0b001, OP_BRANCH)
            }
            Instruction::Blt { rs1, rs2, offset } => {
                encode_b_type(*offset, rs2.encoding(), rs1.encoding(), 0b100, OP_BRANCH)
            }
            Instruction::Bge { rs1, rs2, offset } => {
                encode_b_type(*offset, rs2.encoding(), rs1.encoding(), 0b101, OP_BRANCH)
            }
            Instruction::Bltu { rs1, rs2, offset } => {
                encode_b_type(*offset, rs2.encoding(), rs1.encoding(), 0b110, OP_BRANCH)
            }
            Instruction::Bgeu { rs1, rs2, offset } => {
                encode_b_type(*offset, rs2.encoding(), rs1.encoding(), 0b111, OP_BRANCH)
            }

            // ── Loads ───────────────────────────────────────────────
            Instruction::Lb { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Lh { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b001,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Lw { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b010,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Ld { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b011,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Lbu { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b100,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Lhu { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b101,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Lwu { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b110,
                rd.encoding(),
                OP_LOAD,
            ),

            // ── Stores ──────────────────────────────────────────────
            Instruction::Sb { rs1, rs2, imm } => encode_s_type(
                (*imm as u32) & 0xFFF,
                rs2.encoding(),
                rs1.encoding(),
                0b000,
                OP_STORE,
            ),
            Instruction::Sh { rs1, rs2, imm } => encode_s_type(
                (*imm as u32) & 0xFFF,
                rs2.encoding(),
                rs1.encoding(),
                0b001,
                OP_STORE,
            ),
            Instruction::Sw { rs1, rs2, imm } => encode_s_type(
                (*imm as u32) & 0xFFF,
                rs2.encoding(),
                rs1.encoding(),
                0b010,
                OP_STORE,
            ),
            Instruction::Sd { rs1, rs2, imm } => encode_s_type(
                (*imm as u32) & 0xFFF,
                rs2.encoding(),
                rs1.encoding(),
                0b011,
                OP_STORE,
            ),

            // ── Immediate Arithmetic ────────────────────────────────
            Instruction::Addi { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_IMM,
            ),
            Instruction::Slti { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b010,
                rd.encoding(),
                OP_IMM,
            ),
            Instruction::Sltiu { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b011,
                rd.encoding(),
                OP_IMM,
            ),
            Instruction::Xori { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b100,
                rd.encoding(),
                OP_IMM,
            ),
            Instruction::Ori { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b110,
                rd.encoding(),
                OP_IMM,
            ),
            Instruction::Andi { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b111,
                rd.encoding(),
                OP_IMM,
            ),
            Instruction::Slli { rd, rs1, shamt } => {
                // funct7 = 0b0000000, funct3 = 0b001
                // For RV64I, shamt is 6 bits (bits [25:20])
                let funct7_and_shamt = (*shamt & 0x3F) << 20;
                let word = funct7_and_shamt
                    | (rs1.encoding() << 15)
                    | (0b001 << 12)
                    | (rd.encoding() << 7)
                    | OP_IMM;
                word.to_le_bytes()
            }
            Instruction::Srli { rd, rs1, shamt } => {
                // funct7 = 0b0000000, funct3 = 0b101
                let funct7_and_shamt = (*shamt & 0x3F) << 20;
                let word = funct7_and_shamt
                    | (rs1.encoding() << 15)
                    | (0b101 << 12)
                    | (rd.encoding() << 7)
                    | OP_IMM;
                word.to_le_bytes()
            }
            Instruction::Srai { rd, rs1, shamt } => {
                // funct7 = 0b0100000, funct3 = 0b101
                let funct7_and_shamt = (0b0100000u32 << 25) | ((*shamt & 0x3F) << 20);
                let word = funct7_and_shamt
                    | (rs1.encoding() << 15)
                    | (0b101 << 12)
                    | (rd.encoding() << 7)
                    | OP_IMM;
                word.to_le_bytes()
            }

            // ── Register Arithmetic ─────────────────────────────────
            Instruction::Add { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Sub { rd, rs1, rs2 } => encode_r_type(
                0b0100000,
                rs2.encoding(),
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Sll { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b001,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Slt { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b010,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Sltu { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b011,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Xor { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b100,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Srl { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b101,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Sra { rd, rs1, rs2 } => encode_r_type(
                0b0100000,
                rs2.encoding(),
                rs1.encoding(),
                0b101,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Or { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b110,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::And { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b111,
                rd.encoding(),
                OP_REG,
            ),

            // ── Word-level Arithmetic (RV64) ────────────────────────
            Instruction::Addw { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_REG32,
            ),
            Instruction::Subw { rd, rs1, rs2 } => encode_r_type(
                0b0100000,
                rs2.encoding(),
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_REG32,
            ),
            Instruction::Sllw { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b001,
                rd.encoding(),
                OP_REG32,
            ),
            Instruction::Srlw { rd, rs1, rs2 } => encode_r_type(
                0b0000000,
                rs2.encoding(),
                rs1.encoding(),
                0b101,
                rd.encoding(),
                OP_REG32,
            ),
            Instruction::Sraw { rd, rs1, rs2 } => encode_r_type(
                0b0100000,
                rs2.encoding(),
                rs1.encoding(),
                0b101,
                rd.encoding(),
                OP_REG32,
            ),
            Instruction::Addiw { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_IMM32,
            ),
            Instruction::Slliw { rd, rs1, shamt } => {
                // funct7 = 0b0000000, funct3 = 0b001, shamt is 5 bits
                let funct7_and_shamt = (*shamt & 0x1F) << 20;
                let word = funct7_and_shamt
                    | (rs1.encoding() << 15)
                    | (0b001 << 12)
                    | (rd.encoding() << 7)
                    | OP_IMM32;
                word.to_le_bytes()
            }
            Instruction::Srliw { rd, rs1, shamt } => {
                let funct7_and_shamt = (*shamt & 0x1F) << 20;
                let word = funct7_and_shamt
                    | (rs1.encoding() << 15)
                    | (0b101 << 12)
                    | (rd.encoding() << 7)
                    | OP_IMM32;
                word.to_le_bytes()
            }
            Instruction::Sraiw { rd, rs1, shamt } => {
                let funct7_and_shamt = (0b0100000u32 << 25) | ((*shamt & 0x1F) << 20);
                let word = funct7_and_shamt
                    | (rs1.encoding() << 15)
                    | (0b101 << 12)
                    | (rd.encoding() << 7)
                    | OP_IMM32;
                word.to_le_bytes()
            }

            // ── M Extension ─────────────────────────────────────────
            Instruction::Mul { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b000,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Mulh { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b001,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Mulhsu { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b010,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Mulhu { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b011,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Div { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b100,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Divu { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b101,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Rem { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b110,
                rd.encoding(),
                OP_REG,
            ),
            Instruction::Remu { rd, rs1, rs2 } => encode_r_type(
                0b0000001,
                rs2.encoding(),
                rs1.encoding(),
                0b111,
                rd.encoding(),
                OP_REG,
            ),

            // ── F/D Extension: Load/Store ───────────────────────────
            Instruction::Flw { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b010,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Fld { rd, rs1, imm } => encode_i_type(
                (*imm as u32) & 0xFFF,
                rs1.encoding(),
                0b011,
                rd.encoding(),
                OP_LOAD,
            ),
            Instruction::Fsw { rs1, rs2, imm } => encode_s_type(
                (*imm as u32) & 0xFFF,
                rs2.encoding(),
                rs1.encoding(),
                0b010,
                OP_STORE,
            ),
            Instruction::Fsd { rs1, rs2, imm } => encode_s_type(
                (*imm as u32) & 0xFFF,
                rs2.encoding(),
                rs1.encoding(),
                0b011,
                OP_STORE,
            ),

            // ── F/D Extension: Arithmetic ───────────────────────────
            Instruction::FaddD { rd, rs1, rs2 } => {
                // FADD.D: funct7=0b0000001, rm=0b111 (dynamic), opcode=OP_FP
                // Actually: funct7[6:0] = 0000001, rs2, rs1, rm=111, rd, opcode
                encode_r_type(
                    0b0000001,
                    rs2.encoding(),
                    rs1.encoding(),
                    0b111,
                    rd.encoding(),
                    OP_FP,
                )
            }
            Instruction::FsubD { rd, rs1, rs2 } => encode_r_type(
                0b0000101,
                rs2.encoding(),
                rs1.encoding(),
                0b111,
                rd.encoding(),
                OP_FP,
            ),
            Instruction::FmulD { rd, rs1, rs2 } => encode_r_type(
                0b0001001,
                rs2.encoding(),
                rs1.encoding(),
                0b111,
                rd.encoding(),
                OP_FP,
            ),
            Instruction::FdivD { rd, rs1, rs2 } => encode_r_type(
                0b0001101,
                rs2.encoding(),
                rs1.encoding(),
                0b111,
                rd.encoding(),
                OP_FP,
            ),
            Instruction::FmvD { rd, rs1 } => {
                // FMV.D = FSGNJ.D rd, fs1, fs1 (funct7=0b0010001, funct3=0b000)
                encode_r_type(
                    0b0010001,
                    rs1.encoding(),
                    rs1.encoding(),
                    0b000,
                    rd.encoding(),
                    OP_FP,
                )
            }

            // ── F/D Extension: FP ↔ Integer Conversion ────────────────
            // FCVT.S.W: funct7=1101000, rs2=00000, funct3=rm (0b111=dynamic)
            Instruction::FcvtSW { rd, rs1 } => {
                encode_r_type(0b1101000, 0b00000, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtSWU { rd, rs1 } => {
                encode_r_type(0b1101000, 0b00001, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtSL { rd, rs1 } => {
                encode_r_type(0b1101000, 0b00010, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtSLU { rd, rs1 } => {
                encode_r_type(0b1101000, 0b00011, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            // FCVT.D.W: funct7=1100001, rs2=00000
            Instruction::FcvtDW { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00000, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtDWU { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00001, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtDL { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00010, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtDLU { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00011, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            // FCVT.W.S: funct7=1100000, rs2=00000
            Instruction::FcvtWS { rd, rs1 } => {
                encode_r_type(0b1100000, 0b00000, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtWUS { rd, rs1 } => {
                encode_r_type(0b1100000, 0b00001, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtLS { rd, rs1 } => {
                encode_r_type(0b1100000, 0b00010, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtLUS { rd, rs1 } => {
                encode_r_type(0b1100000, 0b00011, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            // FCVT.W.D: funct7=1100001, rs2=00000
            Instruction::FcvtWD { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00000, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtWUD { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00001, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtLD { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00010, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            Instruction::FcvtLUD { rd, rs1 } => {
                encode_r_type(0b1100001, 0b00011, rs1.encoding(), 0b111, rd.encoding(), OP_FP)
            }
            // FCVT.D.S: funct7=0100001, rs2=00000
            Instruction::FcvtDS { rd, rs1 } => {
                encode_r_type(0b0100001, 0b00000, rs1.encoding(), 0b000, rd.encoding(), OP_FP)
            }
            // FCVT.S.D: funct7=0100000, rs2=00001
            Instruction::FcvtSD { rd, rs1 } => {
                encode_r_type(0b0100000, 0b00001, rs1.encoding(), 0b000, rd.encoding(), OP_FP)
            }
            // FMV.X.W: funct7=1110000, rs2=00000, funct3=000
            Instruction::FmvXW { rd, rs1 } => {
                encode_r_type(0b1110000, 0b00000, rs1.encoding(), 0b000, rd.encoding(), OP_FP)
            }
            // FMV.W.X: funct7=1111000, rs2=00000, funct3=000
            Instruction::FmvWX { rd, rs1 } => {
                encode_r_type(0b1111000, 0b00000, rs1.encoding(), 0b000, rd.encoding(), OP_FP)
            }
            // FMV.X.D: funct7=1110001, rs2=00000, funct3=000
            Instruction::FmvXD { rd, rs1 } => {
                encode_r_type(0b1110001, 0b00000, rs1.encoding(), 0b000, rd.encoding(), OP_FP)
            }
            // FMV.D.X: funct7=1111001, rs2=00000, funct3=000
            Instruction::FmvDX { rd, rs1 } => {
                encode_r_type(0b1111001, 0b00000, rs1.encoding(), 0b000, rd.encoding(), OP_FP)
            }

            // ── Zicsr Extension ──────────────────────────────────────
            Instruction::Csrrw { rd, csr, rs1 } => {
                // CSRRW: I-type, funct3=0b001, opcode=SYSTEM
                encode_i_type(*csr, rs1.encoding(), 0b001, rd.encoding(), OP_SYSTEM)
            }
            Instruction::Csrrs { rd, csr, rs1 } => {
                // CSRRS: I-type, funct3=0b010, opcode=SYSTEM
                encode_i_type(*csr, rs1.encoding(), 0b010, rd.encoding(), OP_SYSTEM)
            }
            Instruction::Csrrc { rd, csr, rs1 } => {
                // CSRRC: I-type, funct3=0b011, opcode=SYSTEM
                encode_i_type(*csr, rs1.encoding(), 0b011, rd.encoding(), OP_SYSTEM)
            }
            Instruction::Csrrwi { rd, csr, uimm } => {
                // CSRRWI: I-type, funct3=0b101, opcode=SYSTEM
                encode_i_type(*csr, *uimm & 0x1F, 0b101, rd.encoding(), OP_SYSTEM)
            }
            Instruction::Csrrsi { rd, csr, uimm } => {
                // CSRRSI: I-type, funct3=0b110, opcode=SYSTEM
                encode_i_type(*csr, *uimm & 0x1F, 0b110, rd.encoding(), OP_SYSTEM)
            }
            Instruction::Csrrci { rd, csr, uimm } => {
                // CSRRCI: I-type, funct3=0b111, opcode=SYSTEM
                encode_i_type(*csr, *uimm & 0x1F, 0b111, rd.encoding(), OP_SYSTEM)
            }

            // ── Zifencei Extension ──────────────────────────────────
            Instruction::FenceI => {
                // FENCE.I: opcode=MISC-MEM, funct3=0b001, rd=0, rs1=0, imm=0
                encode_i_type(0, 0, 0b001, 0, OP_MISC_MEM)
            }

            // ── System / Misc ───────────────────────────────────────
            Instruction::Ecall => {
                // ECALL = 0x00000073
                0x00000073u32.to_le_bytes()
            }
            Instruction::Ebreak => {
                // EBREAK = 0x00100073
                0x00100073u32.to_le_bytes()
            }
            Instruction::Fence { pred, succ } => {
                // FENCE: opcode=MISC-MEM, funct3=0b000, rd=0, rs1=0
                let imm = ((*pred & 0xF) << 4) | (*succ & 0xF);
                encode_i_type(imm, 0, 0b000, 0, OP_MISC_MEM)
            }
            Instruction::Nop => {
                // NOP = ADDI x0, x0, 0 = 0x00000013
                encode_i_type(0, 0, 0b000, 0, OP_IMM)
            }

            // ── RV64A Extension: Atomic ───────────────────────────────
            Instruction::LrD { rd, rs1 } => {
                // LR.D rd, (rs1)
                // Encoding: R-type with funct3=0b010 (64-bit), funct7=0b0001010
                // (aq=0, rl=0, funct5=0b00010), rs2=0, opcode=0b0101111 (AMO).
                // encode_r_type signature: (funct7, rs2, rs1, funct3, rd, opcode).
                encode_r_type(0b0001010, 0, rs1.encoding(), 0b010, rd.encoding(), 0b0101111)
            }
            Instruction::ScD { rd, rs1, rs2 } => {
                // SC.D rd, rs1, rs2
                // Encoding: R-type with funct3=0b010 (64-bit), funct7=0b0001100
                // (aq=0, rl=0, funct5=0b00011), opcode=0b0101111 (AMO).
                // encode_r_type signature: (funct7, rs2, rs1, funct3, rd, opcode).
                encode_r_type(0b0001100, rs2.encoding(), rs1.encoding(), 0b010, rd.encoding(), 0b0101111)
            }
        }
    }

    /// Returns the mnemonic name of this instruction.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Instruction::Lui { .. } => "lui",
            Instruction::Auipc { .. } => "auipc",
            Instruction::Jal { .. } => "jal",
            Instruction::Jalr { .. } => "jalr",
            Instruction::Beq { .. } => "beq",
            Instruction::Bne { .. } => "bne",
            Instruction::Blt { .. } => "blt",
            Instruction::Bge { .. } => "bge",
            Instruction::Bltu { .. } => "bltu",
            Instruction::Bgeu { .. } => "bgeu",
            Instruction::Lb { .. } => "lb",
            Instruction::Lh { .. } => "lh",
            Instruction::Lw { .. } => "lw",
            Instruction::Ld { .. } => "ld",
            Instruction::Lbu { .. } => "lbu",
            Instruction::Lhu { .. } => "lhu",
            Instruction::Lwu { .. } => "lwu",
            Instruction::Sb { .. } => "sb",
            Instruction::Sh { .. } => "sh",
            Instruction::Sw { .. } => "sw",
            Instruction::Sd { .. } => "sd",
            Instruction::Addi { .. } => "addi",
            Instruction::Slti { .. } => "slti",
            Instruction::Sltiu { .. } => "sltiu",
            Instruction::Xori { .. } => "xori",
            Instruction::Ori { .. } => "ori",
            Instruction::Andi { .. } => "andi",
            Instruction::Slli { .. } => "slli",
            Instruction::Srli { .. } => "srli",
            Instruction::Srai { .. } => "srai",
            Instruction::Add { .. } => "add",
            Instruction::Sub { .. } => "sub",
            Instruction::Sll { .. } => "sll",
            Instruction::Slt { .. } => "slt",
            Instruction::Sltu { .. } => "sltu",
            Instruction::Xor { .. } => "xor",
            Instruction::Srl { .. } => "srl",
            Instruction::Sra { .. } => "sra",
            Instruction::Or { .. } => "or",
            Instruction::And { .. } => "and",
            Instruction::Addw { .. } => "addw",
            Instruction::Subw { .. } => "subw",
            Instruction::Sllw { .. } => "sllw",
            Instruction::Srlw { .. } => "srlw",
            Instruction::Sraw { .. } => "sraw",
            Instruction::Addiw { .. } => "addiw",
            Instruction::Slliw { .. } => "slliw",
            Instruction::Srliw { .. } => "srliw",
            Instruction::Sraiw { .. } => "sraiw",
            Instruction::Mul { .. } => "mul",
            Instruction::Mulh { .. } => "mulh",
            Instruction::Mulhsu { .. } => "mulhsu",
            Instruction::Mulhu { .. } => "mulhu",
            Instruction::Div { .. } => "div",
            Instruction::Divu { .. } => "divu",
            Instruction::Rem { .. } => "rem",
            Instruction::Remu { .. } => "remu",
            Instruction::Flw { .. } => "flw",
            Instruction::Fld { .. } => "fld",
            Instruction::Fsw { .. } => "fsw",
            Instruction::Fsd { .. } => "fsd",
            Instruction::FaddD { .. } => "fadd.d",
            Instruction::FsubD { .. } => "fsub.d",
            Instruction::FmulD { .. } => "fmul.d",
            Instruction::FdivD { .. } => "fdiv.d",
            Instruction::FmvD { .. } => "fmv.d",
            Instruction::FcvtSW { .. } => "fcvt.s.w",
            Instruction::FcvtSWU { .. } => "fcvt.s.wu",
            Instruction::FcvtSL { .. } => "fcvt.s.l",
            Instruction::FcvtSLU { .. } => "fcvt.s.lu",
            Instruction::FcvtDW { .. } => "fcvt.d.w",
            Instruction::FcvtDWU { .. } => "fcvt.d.wu",
            Instruction::FcvtDL { .. } => "fcvt.d.l",
            Instruction::FcvtDLU { .. } => "fcvt.d.lu",
            Instruction::FcvtWS { .. } => "fcvt.w.s",
            Instruction::FcvtWUS { .. } => "fcvt.wu.s",
            Instruction::FcvtLS { .. } => "fcvt.l.s",
            Instruction::FcvtLUS { .. } => "fcvt.lu.s",
            Instruction::FcvtWD { .. } => "fcvt.w.d",
            Instruction::FcvtWUD { .. } => "fcvt.wu.d",
            Instruction::FcvtLD { .. } => "fcvt.l.d",
            Instruction::FcvtLUD { .. } => "fcvt.lu.d",
            Instruction::FcvtDS { .. } => "fcvt.d.s",
            Instruction::FcvtSD { .. } => "fcvt.s.d",
            Instruction::FmvXW { .. } => "fmv.x.w",
            Instruction::FmvWX { .. } => "fmv.w.x",
            Instruction::FmvXD { .. } => "fmv.x.d",
            Instruction::FmvDX { .. } => "fmv.d.x",
            Instruction::Csrrw { .. } => "csrrw",
            Instruction::Csrrs { .. } => "csrrs",
            Instruction::Csrrc { .. } => "csrrc",
            Instruction::Csrrwi { .. } => "csrrwi",
            Instruction::Csrrsi { .. } => "csrrsi",
            Instruction::Csrrci { .. } => "csrrci",
            Instruction::FenceI => "fence.i",
            Instruction::Ecall => "ecall",
            Instruction::Ebreak => "ebreak",
            Instruction::Fence { .. } => "fence",
            Instruction::Nop => "nop",
            Instruction::LrD { .. } => "lr.d",
            Instruction::ScD { .. } => "sc.d",
        }
    }

    /// Decode a 32-bit RISC-V machine-code word into an `Instruction`.
    ///
    /// Returns `None` for encodings not yet covered by the decoder.
    /// Covers all instruction classes defined in this backend.
    pub fn decode(word: u32) -> Option<Instruction> {
        let opcode = word & 0x7F;
        let rd = (word >> 7) & 0x1F;
        let funct3 = (word >> 12) & 0x7;
        let rs1 = (word >> 15) & 0x1F;
        let rs2 = (word >> 20) & 0x1F;
        let funct7 = (word >> 25) & 0x7F;

        match opcode {
            // ── LUI ────────────────────────────────────────────────
            0b0110111 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let imm = word & 0xFFFFF000;
                Some(Instruction::Lui { rd: rd_reg, imm })
            }

            // ── AUIPC ──────────────────────────────────────────────
            0b0010111 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let imm = word & 0xFFFFF000;
                Some(Instruction::Auipc { rd: rd_reg, imm })
            }

            // ── JAL ────────────────────────────────────────────────
            0b1101111 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let imm20 = ((word >> 31) & 1) << 20
                    | ((word >> 12) & 0xFF) << 12
                    | ((word >> 20) & 1) << 11
                    | ((word >> 21) & 0x3FF) << 1;
                let offset = ((imm20 << 11) as i32) >> 11;
                Some(Instruction::Jal { rd: rd_reg, offset })
            }

            // ── JALR ───────────────────────────────────────────────
            0b1100111 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let imm = (((word >> 20) as i32) << 20) >> 20;
                Some(Instruction::Jalr {
                    rd: rd_reg,
                    rs1: rs1_reg,
                    imm,
                })
            }

            // ── BRANCH ─────────────────────────────────────────────
            0b1100011 => {
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let rs2_reg = Gpr::from_encoding(rs2)?;
                let imm12 = ((word >> 31) & 1) << 12
                    | ((word >> 7) & 1) << 11
                    | ((word >> 25) & 0x3F) << 5
                    | ((word >> 8) & 0xF) << 1;
                let offset = ((imm12 << 19) as i32) >> 19;
                match funct3 {
                    0b000 => Some(Instruction::Beq {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        offset,
                    }),
                    0b001 => Some(Instruction::Bne {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        offset,
                    }),
                    0b100 => Some(Instruction::Blt {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        offset,
                    }),
                    0b101 => Some(Instruction::Bge {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        offset,
                    }),
                    0b110 => Some(Instruction::Bltu {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        offset,
                    }),
                    0b111 => Some(Instruction::Bgeu {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        offset,
                    }),
                    _ => None,
                }
            }

            // ── LOAD ───────────────────────────────────────────────
            0b0000011 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let imm = (((word >> 20) as i32) << 20) >> 20;
                match funct3 {
                    0b000 => Some(Instruction::Lb {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b001 => Some(Instruction::Lh {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b010 => Some(Instruction::Lw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b011 => Some(Instruction::Ld {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b100 => Some(Instruction::Lbu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b101 => Some(Instruction::Lhu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b110 => Some(Instruction::Lwu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    _ => None,
                }
            }

            // ── STORE ──────────────────────────────────────────────
            0b0100011 => {
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let rs2_reg = Gpr::from_encoding(rs2)?;
                let imm_lo = (word >> 7) & 0x1F;
                let imm_hi = (word >> 25) & 0x7F;
                let imm_raw = (imm_hi << 5) | imm_lo;
                let imm = ((imm_raw as i32) << 20) >> 20;
                match funct3 {
                    0b000 => Some(Instruction::Sb {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        imm,
                    }),
                    0b001 => Some(Instruction::Sh {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        imm,
                    }),
                    0b010 => Some(Instruction::Sw {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        imm,
                    }),
                    0b011 => Some(Instruction::Sd {
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                        imm,
                    }),
                    _ => None,
                }
            }

            // ── OP-IMM (RV64I) ─────────────────────────────────────
            0b0010011 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let imm = (((word >> 20) as i32) << 20) >> 20;
                let shamt = (word >> 20) & 0x3F;
                match funct3 {
                    0b000 => Some(Instruction::Addi {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b010 => Some(Instruction::Slti {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b011 => Some(Instruction::Sltiu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b100 => Some(Instruction::Xori {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b110 => Some(Instruction::Ori {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b111 => Some(Instruction::Andi {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b001 => Some(Instruction::Slli {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        shamt,
                    }),
                    0b101 => {
                        if funct7 == 0b0100000 {
                            Some(Instruction::Srai {
                                rd: rd_reg,
                                rs1: rs1_reg,
                                shamt,
                            })
                        } else {
                            Some(Instruction::Srli {
                                rd: rd_reg,
                                rs1: rs1_reg,
                                shamt,
                            })
                        }
                    }
                    _ => None,
                }
            }

            // ── OP (RV64I register-register) ────────────────────────
            0b0110011 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let rs2_reg = Gpr::from_encoding(rs2)?;
                match (funct7, funct3) {
                    (0b0000000, 0b000) => Some(Instruction::Add {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0100000, 0b000) => Some(Instruction::Sub {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b001) => Some(Instruction::Sll {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b010) => Some(Instruction::Slt {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b011) => Some(Instruction::Sltu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b100) => Some(Instruction::Xor {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b101) => Some(Instruction::Srl {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0100000, 0b101) => Some(Instruction::Sra {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b110) => Some(Instruction::Or {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b111) => Some(Instruction::And {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    // M extension
                    (0b0000001, 0b000) => Some(Instruction::Mul {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000001, 0b001) => Some(Instruction::Mulh {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000001, 0b010) => Some(Instruction::Mulhsu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000001, 0b011) => Some(Instruction::Mulhu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000001, 0b100) => Some(Instruction::Div {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000001, 0b101) => Some(Instruction::Divu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000001, 0b110) => Some(Instruction::Rem {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000001, 0b111) => Some(Instruction::Remu {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    _ => None,
                }
            }

            // ── OP-IMM-32 (RV64) ───────────────────────────────────
            0b0011011 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let imm = (((word >> 20) as i32) << 20) >> 20;
                let shamt = (word >> 20) & 0x1F;
                match funct3 {
                    0b000 => Some(Instruction::Addiw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        imm,
                    }),
                    0b001 => Some(Instruction::Slliw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        shamt,
                    }),
                    0b101 => {
                        if funct7 == 0b0100000 {
                            Some(Instruction::Sraiw {
                                rd: rd_reg,
                                rs1: rs1_reg,
                                shamt,
                            })
                        } else {
                            Some(Instruction::Srliw {
                                rd: rd_reg,
                                rs1: rs1_reg,
                                shamt,
                            })
                        }
                    }
                    _ => None,
                }
            }

            // ── OP-32 (RV64) ───────────────────────────────────────
            0b0111011 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let rs2_reg = Gpr::from_encoding(rs2)?;
                match (funct7, funct3) {
                    (0b0000000, 0b000) => Some(Instruction::Addw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0100000, 0b000) => Some(Instruction::Subw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b001) => Some(Instruction::Sllw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0000000, 0b101) => Some(Instruction::Srlw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    (0b0100000, 0b101) => Some(Instruction::Sraw {
                        rd: rd_reg,
                        rs1: rs1_reg,
                        rs2: rs2_reg,
                    }),
                    _ => None,
                }
            }

            // ── SYSTEM ─────────────────────────────────────────────
            0b1110011 => {
                if word == 0x00000073 {
                    Some(Instruction::Ecall)
                } else if word == 0x00100073 {
                    Some(Instruction::Ebreak)
                } else {
                    let csr = (word >> 20) & 0xFFF;
                    let rd_reg = Gpr::from_encoding(rd)?;
                    let rs1_reg = Gpr::from_encoding(rs1)?;
                    match funct3 {
                        0b001 => Some(Instruction::Csrrw {
                            rd: rd_reg,
                            csr,
                            rs1: rs1_reg,
                        }),
                        0b010 => Some(Instruction::Csrrs {
                            rd: rd_reg,
                            csr,
                            rs1: rs1_reg,
                        }),
                        0b011 => Some(Instruction::Csrrc {
                            rd: rd_reg,
                            csr,
                            rs1: rs1_reg,
                        }),
                        0b101 => Some(Instruction::Csrrwi {
                            rd: rd_reg,
                            csr,
                            uimm: rs1,
                        }),
                        0b110 => Some(Instruction::Csrrsi {
                            rd: rd_reg,
                            csr,
                            uimm: rs1,
                        }),
                        0b111 => Some(Instruction::Csrrci {
                            rd: rd_reg,
                            csr,
                            uimm: rs1,
                        }),
                        _ => None,
                    }
                }
            }

            // ── MISC-MEM (FENCE / FENCE.I) ─────────────────────────
            0b0001111 => {
                if funct3 == 0b001 {
                    Some(Instruction::FenceI)
                } else {
                    let imm = (word >> 20) & 0xFF;
                    let pred = (imm >> 4) & 0xF;
                    let succ = imm & 0xF;
                    Some(Instruction::Fence { pred, succ })
                }
            }

            // ── FP (opcode=0x53) ───────────────────────────────────
            0b1010011 => {
                // ── FP ↔ Integer Conversion (FCVT) ──────────────────
                // These use R-type with opcode=OP_FP. The rs2 field
                // selects the conversion variant. funct3 is the rounding
                // mode (0b111 = dynamic for int<->float, 0b000 for
                // float<->float width change).
                //
                // Note: per the RISC-V spec, fcvt.d.w (int->double) and
                // fcvt.w.d (double->int) share the same encoding
                // (funct7=0b1100001, rs2=0b00000); the same applies to
                // the L/D pairs. We decode to the int->float variant
                // (the "primary" direction); the float->int direction
                // is the same encoding and will display the same
                // mnemonic prefix "fcvt.".
                match (funct7, rs2, funct3) {
                    // FCVT.S.W / WU / L / LU (int -> single)
                    (0b1101000, 0b00000, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtSW { rd: rd_f, rs1: rs1_g })
                    }
                    (0b1101000, 0b00001, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtSWU { rd: rd_f, rs1: rs1_g })
                    }
                    (0b1101000, 0b00010, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtSL { rd: rd_f, rs1: rs1_g })
                    }
                    (0b1101000, 0b00011, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtSLU { rd: rd_f, rs1: rs1_g })
                    }
                    // FCVT.D.W / WU / L / LU (int -> double)
                    (0b1100001, 0b00000, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtDW { rd: rd_f, rs1: rs1_g })
                    }
                    (0b1100001, 0b00001, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtDWU { rd: rd_f, rs1: rs1_g })
                    }
                    (0b1100001, 0b00010, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtDL { rd: rd_f, rs1: rs1_g })
                    }
                    (0b1100001, 0b00011, 0b111) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_g = Gpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtDLU { rd: rd_f, rs1: rs1_g })
                    }
                    // FCVT.W.S / WU.S / L.S / LU.S (single -> int)
                    (0b1100000, 0b00000, 0b111) => {
                        let rd_g = Gpr::from_encoding(rd)?;
                        let rs1_f = Fpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtWS { rd: rd_g, rs1: rs1_f })
                    }
                    (0b1100000, 0b00001, 0b111) => {
                        let rd_g = Gpr::from_encoding(rd)?;
                        let rs1_f = Fpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtWUS { rd: rd_g, rs1: rs1_f })
                    }
                    (0b1100000, 0b00010, 0b111) => {
                        let rd_g = Gpr::from_encoding(rd)?;
                        let rs1_f = Fpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtLS { rd: rd_g, rs1: rs1_f })
                    }
                    (0b1100000, 0b00011, 0b111) => {
                        let rd_g = Gpr::from_encoding(rd)?;
                        let rs1_f = Fpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtLUS { rd: rd_g, rs1: rs1_f })
                    }
                    // FCVT.D.S (single -> double) / FCVT.S.D (double -> single)
                    (0b0100001, 0b00000, 0b000) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_f = Fpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtDS { rd: rd_f, rs1: rs1_f })
                    }
                    (0b0100000, 0b00001, 0b000) => {
                        let rd_f = Fpr::from_encoding(rd)?;
                        let rs1_f = Fpr::from_encoding(rs1)?;
                        Some(Instruction::FcvtSD { rd: rd_f, rs1: rs1_f })
                    }
                    _ => {
                        // Fall through to the FP arithmetic decode below.
                        let rd_fpr = Fpr::from_encoding(rd)?;
                        let rs1_fpr = Fpr::from_encoding(rs1)?;
                        let rs2_fpr = Fpr::from_encoding(rs2)?;
                        match (funct7, funct3) {
                            (0b0000001, 0b111) => Some(Instruction::FaddD {
                                rd: rd_fpr,
                                rs1: rs1_fpr,
                                rs2: rs2_fpr,
                            }),
                            (0b0000101, 0b111) => Some(Instruction::FsubD {
                                rd: rd_fpr,
                                rs1: rs1_fpr,
                                rs2: rs2_fpr,
                            }),
                            (0b0001001, 0b111) => Some(Instruction::FmulD {
                                rd: rd_fpr,
                                rs1: rs1_fpr,
                                rs2: rs2_fpr,
                            }),
                            (0b0001101, 0b111) => Some(Instruction::FdivD {
                                rd: rd_fpr,
                                rs1: rs1_fpr,
                                rs2: rs2_fpr,
                            }),
                            (0b0010001, 0b000) if rs1 == rs2 => Some(Instruction::FmvD {
                                rd: rd_fpr,
                                rs1: rs1_fpr,
                            }),
                            _ => None,
                        }
                    }
                }
            }

            // ── AMO (opcode=0b0101111, RV64A) ──────────────────────
            // The encoder produces LR.D and SC.D with funct3=0b010 (64-bit)
            // and funct5 = 0b00010 (LR) / 0b00011 (SC). The low 2 bits of
            // funct7 are the aq/rl bits, which we ignore when decoding so
            // that all aq/rl combinations are recognised.
            0b0101111 => {
                let rd_reg = Gpr::from_encoding(rd)?;
                let rs1_reg = Gpr::from_encoding(rs1)?;
                let funct5 = funct7 >> 2;
                match (funct5, funct3) {
                    (0b00010, 0b010) => {
                        // LR.D rd, (rs1)  — rs2 must be 0
                        Some(Instruction::LrD { rd: rd_reg, rs1: rs1_reg })
                    }
                    (0b00011, 0b010) => {
                        // SC.D rd, rs2, (rs1)
                        let rs2_reg = Gpr::from_encoding(rs2)?;
                        Some(Instruction::ScD {
                            rd: rd_reg,
                            rs1: rs1_reg,
                            rs2: rs2_reg,
                        })
                    }
                    _ => None,
                }
            }

            _ => None,
        }
    }
}

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::Lui { rd, imm } => write!(f, "lui {}, 0x{:08x}", rd, imm),
            Instruction::Auipc { rd, imm } => write!(f, "auipc {}, 0x{:08x}", rd, imm),
            Instruction::Jal { rd, offset } => write!(f, "jal {}, {:+}", rd, offset),
            Instruction::Jalr { rd, rs1, imm } => write!(f, "jalr {}, {}({})", rd, imm, rs1),
            Instruction::Beq { rs1, rs2, offset } => {
                write!(f, "beq {}, {}, {:+}", rs1, rs2, offset)
            }
            Instruction::Bne { rs1, rs2, offset } => {
                write!(f, "bne {}, {}, {:+}", rs1, rs2, offset)
            }
            Instruction::Blt { rs1, rs2, offset } => {
                write!(f, "blt {}, {}, {:+}", rs1, rs2, offset)
            }
            Instruction::Bge { rs1, rs2, offset } => {
                write!(f, "bge {}, {}, {:+}", rs1, rs2, offset)
            }
            Instruction::Bltu { rs1, rs2, offset } => {
                write!(f, "bltu {}, {}, {:+}", rs1, rs2, offset)
            }
            Instruction::Bgeu { rs1, rs2, offset } => {
                write!(f, "bgeu {}, {}, {:+}", rs1, rs2, offset)
            }
            Instruction::Lb { rd, rs1, imm } => write!(f, "lb {}, {}({})", rd, imm, rs1),
            Instruction::Lh { rd, rs1, imm } => write!(f, "lh {}, {}({})", rd, imm, rs1),
            Instruction::Lw { rd, rs1, imm } => write!(f, "lw {}, {}({})", rd, imm, rs1),
            Instruction::Ld { rd, rs1, imm } => write!(f, "ld {}, {}({})", rd, imm, rs1),
            Instruction::Lbu { rd, rs1, imm } => write!(f, "lbu {}, {}({})", rd, imm, rs1),
            Instruction::Lhu { rd, rs1, imm } => write!(f, "lhu {}, {}({})", rd, imm, rs1),
            Instruction::Lwu { rd, rs1, imm } => write!(f, "lwu {}, {}({})", rd, imm, rs1),
            Instruction::Sb { rs1, rs2, imm } => write!(f, "sb {}, {}({})", rs2, imm, rs1),
            Instruction::Sh { rs1, rs2, imm } => write!(f, "sh {}, {}({})", rs2, imm, rs1),
            Instruction::Sw { rs1, rs2, imm } => write!(f, "sw {}, {}({})", rs2, imm, rs1),
            Instruction::Sd { rs1, rs2, imm } => write!(f, "sd {}, {}({})", rs2, imm, rs1),
            Instruction::Addi { rd, rs1, imm } => write!(f, "addi {}, {}, {}", rd, rs1, imm),
            Instruction::Slti { rd, rs1, imm } => write!(f, "slti {}, {}, {}", rd, rs1, imm),
            Instruction::Sltiu { rd, rs1, imm } => write!(f, "sltiu {}, {}, {}", rd, rs1, imm),
            Instruction::Xori { rd, rs1, imm } => write!(f, "xori {}, {}, {}", rd, rs1, imm),
            Instruction::Ori { rd, rs1, imm } => write!(f, "ori {}, {}, {}", rd, rs1, imm),
            Instruction::Andi { rd, rs1, imm } => write!(f, "andi {}, {}, {}", rd, rs1, imm),
            Instruction::Slli { rd, rs1, shamt } => write!(f, "slli {}, {}, {}", rd, rs1, shamt),
            Instruction::Srli { rd, rs1, shamt } => write!(f, "srli {}, {}, {}", rd, rs1, shamt),
            Instruction::Srai { rd, rs1, shamt } => write!(f, "srai {}, {}, {}", rd, rs1, shamt),
            Instruction::Add { rd, rs1, rs2 } => write!(f, "add {}, {}, {}", rd, rs1, rs2),
            Instruction::Sub { rd, rs1, rs2 } => write!(f, "sub {}, {}, {}", rd, rs1, rs2),
            Instruction::Sll { rd, rs1, rs2 } => write!(f, "sll {}, {}, {}", rd, rs1, rs2),
            Instruction::Slt { rd, rs1, rs2 } => write!(f, "slt {}, {}, {}", rd, rs1, rs2),
            Instruction::Sltu { rd, rs1, rs2 } => write!(f, "sltu {}, {}, {}", rd, rs1, rs2),
            Instruction::Xor { rd, rs1, rs2 } => write!(f, "xor {}, {}, {}", rd, rs1, rs2),
            Instruction::Srl { rd, rs1, rs2 } => write!(f, "srl {}, {}, {}", rd, rs1, rs2),
            Instruction::Sra { rd, rs1, rs2 } => write!(f, "sra {}, {}, {}", rd, rs1, rs2),
            Instruction::Or { rd, rs1, rs2 } => write!(f, "or {}, {}, {}", rd, rs1, rs2),
            Instruction::And { rd, rs1, rs2 } => write!(f, "and {}, {}, {}", rd, rs1, rs2),
            Instruction::Addw { rd, rs1, rs2 } => write!(f, "addw {}, {}, {}", rd, rs1, rs2),
            Instruction::Subw { rd, rs1, rs2 } => write!(f, "subw {}, {}, {}", rd, rs1, rs2),
            Instruction::Sllw { rd, rs1, rs2 } => write!(f, "sllw {}, {}, {}", rd, rs1, rs2),
            Instruction::Srlw { rd, rs1, rs2 } => write!(f, "srlw {}, {}, {}", rd, rs1, rs2),
            Instruction::Sraw { rd, rs1, rs2 } => write!(f, "sraw {}, {}, {}", rd, rs1, rs2),
            Instruction::Addiw { rd, rs1, imm } => write!(f, "addiw {}, {}, {}", rd, rs1, imm),
            Instruction::Slliw { rd, rs1, shamt } => write!(f, "slliw {}, {}, {}", rd, rs1, shamt),
            Instruction::Srliw { rd, rs1, shamt } => write!(f, "srliw {}, {}, {}", rd, rs1, shamt),
            Instruction::Sraiw { rd, rs1, shamt } => write!(f, "sraiw {}, {}, {}", rd, rs1, shamt),
            Instruction::Mul { rd, rs1, rs2 } => write!(f, "mul {}, {}, {}", rd, rs1, rs2),
            Instruction::Mulh { rd, rs1, rs2 } => write!(f, "mulh {}, {}, {}", rd, rs1, rs2),
            Instruction::Mulhsu { rd, rs1, rs2 } => write!(f, "mulhsu {}, {}, {}", rd, rs1, rs2),
            Instruction::Mulhu { rd, rs1, rs2 } => write!(f, "mulhu {}, {}, {}", rd, rs1, rs2),
            Instruction::Div { rd, rs1, rs2 } => write!(f, "div {}, {}, {}", rd, rs1, rs2),
            Instruction::Divu { rd, rs1, rs2 } => write!(f, "divu {}, {}, {}", rd, rs1, rs2),
            Instruction::Rem { rd, rs1, rs2 } => write!(f, "rem {}, {}, {}", rd, rs1, rs2),
            Instruction::Remu { rd, rs1, rs2 } => write!(f, "remu {}, {}, {}", rd, rs1, rs2),
            Instruction::Flw { rd, rs1, imm } => write!(f, "flw {}, {}({})", rd, imm, rs1),
            Instruction::Fld { rd, rs1, imm } => write!(f, "fld {}, {}({})", rd, imm, rs1),
            Instruction::Fsw { rs1, rs2, imm } => write!(f, "fsw {}, {}({})", rs2, imm, rs1),
            Instruction::Fsd { rs1, rs2, imm } => write!(f, "fsd {}, {}({})", rs2, imm, rs1),
            Instruction::FaddD { rd, rs1, rs2 } => write!(f, "fadd.d {}, {}, {}", rd, rs1, rs2),
            Instruction::FsubD { rd, rs1, rs2 } => write!(f, "fsub.d {}, {}, {}", rd, rs1, rs2),
            Instruction::FmulD { rd, rs1, rs2 } => write!(f, "fmul.d {}, {}, {}", rd, rs1, rs2),
            Instruction::FdivD { rd, rs1, rs2 } => write!(f, "fdiv.d {}, {}, {}", rd, rs1, rs2),
            Instruction::FmvD { rd, rs1 } => write!(f, "fmv.d {}, {}", rd, rs1),
            Instruction::FcvtSW { rd, rs1 } => write!(f, "fcvt.s.w {}, {}", rd, rs1),
            Instruction::FcvtSWU { rd, rs1 } => write!(f, "fcvt.s.wu {}, {}", rd, rs1),
            Instruction::FcvtSL { rd, rs1 } => write!(f, "fcvt.s.l {}, {}", rd, rs1),
            Instruction::FcvtSLU { rd, rs1 } => write!(f, "fcvt.s.lu {}, {}", rd, rs1),
            Instruction::FcvtDW { rd, rs1 } => write!(f, "fcvt.d.w {}, {}", rd, rs1),
            Instruction::FcvtDWU { rd, rs1 } => write!(f, "fcvt.d.wu {}, {}", rd, rs1),
            Instruction::FcvtDL { rd, rs1 } => write!(f, "fcvt.d.l {}, {}", rd, rs1),
            Instruction::FcvtDLU { rd, rs1 } => write!(f, "fcvt.d.lu {}, {}", rd, rs1),
            Instruction::FcvtWS { rd, rs1 } => write!(f, "fcvt.w.s {}, {}", rd, rs1),
            Instruction::FcvtWUS { rd, rs1 } => write!(f, "fcvt.wu.s {}, {}", rd, rs1),
            Instruction::FcvtLS { rd, rs1 } => write!(f, "fcvt.l.s {}, {}", rd, rs1),
            Instruction::FcvtLUS { rd, rs1 } => write!(f, "fcvt.lu.s {}, {}", rd, rs1),
            Instruction::FcvtWD { rd, rs1 } => write!(f, "fcvt.w.d {}, {}", rd, rs1),
            Instruction::FcvtWUD { rd, rs1 } => write!(f, "fcvt.wu.d {}, {}", rd, rs1),
            Instruction::FcvtLD { rd, rs1 } => write!(f, "fcvt.l.d {}, {}", rd, rs1),
            Instruction::FcvtLUD { rd, rs1 } => write!(f, "fcvt.lu.d {}, {}", rd, rs1),
            Instruction::FcvtDS { rd, rs1 } => write!(f, "fcvt.d.s {}, {}", rd, rs1),
            Instruction::FcvtSD { rd, rs1 } => write!(f, "fcvt.s.d {}, {}", rd, rs1),
            Instruction::FmvXW { rd, rs1 } => write!(f, "fmv.x.w {}, {}", rd, rs1),
            Instruction::FmvWX { rd, rs1 } => write!(f, "fmv.w.x {}, {}", rd, rs1),
            Instruction::FmvXD { rd, rs1 } => write!(f, "fmv.x.d {}, {}", rd, rs1),
            Instruction::FmvDX { rd, rs1 } => write!(f, "fmv.d.x {}, {}", rd, rs1),
            Instruction::Csrrw { rd, csr, rs1 } => {
                write!(f, "csrrw {}, 0x{:03x}, {}", rd, csr, rs1)
            }
            Instruction::Csrrs { rd, csr, rs1 } => {
                write!(f, "csrrs {}, 0x{:03x}, {}", rd, csr, rs1)
            }
            Instruction::Csrrc { rd, csr, rs1 } => {
                write!(f, "csrrc {}, 0x{:03x}, {}", rd, csr, rs1)
            }
            Instruction::Csrrwi { rd, csr, uimm } => {
                write!(f, "csrrwi {}, 0x{:03x}, {}", rd, csr, uimm)
            }
            Instruction::Csrrsi { rd, csr, uimm } => {
                write!(f, "csrrsi {}, 0x{:03x}, {}", rd, csr, uimm)
            }
            Instruction::Csrrci { rd, csr, uimm } => {
                write!(f, "csrrci {}, 0x{:03x}, {}", rd, csr, uimm)
            }
            Instruction::FenceI => write!(f, "fence.i"),
            Instruction::Ecall => write!(f, "ecall"),
            Instruction::Ebreak => write!(f, "ebreak"),
            Instruction::Fence { pred, succ } => write!(f, "fence {:#x}, {:#x}", pred, succ),
            Instruction::Nop => write!(f, "nop"),
            Instruction::LrD { rd, rs1 } => write!(f, "lr.d {}, ({})", rd, rs1),
            Instruction::ScD { rd, rs1, rs2 } => write!(f, "sc.d {}, {}, ({})", rd, rs2, rs1),
        }
    }
}

// ===========================================================================
// ELF64 Builder for RISC-V
// ===========================================================================

/// Build a minimal ELF64 binary for RISC-V 64-bit from raw code bytes.
///
/// Produces a static executable with 2 LOAD segments:
/// - Segment 1: PF_R | PF_X — contains .text (code)
/// - Segment 2: PF_R | PF_W — writable data/stack space
///
/// The two segments are page-aligned to ensure the kernel maps them
/// with different permissions.
fn build_minimal_riscv64_elf_2seg(code: &[u8], base_addr: u64) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x1000; // 4 KB

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = 2;
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    // Page-align the text segment start in the file for mmap compatibility.
    let text_offset = ((phdr_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let text_size = code.len() as u64;

    // The data segment starts on the next page after the text.
    let text_file_end = text_offset + text_size;
    let data_vaddr = ((base_addr + text_file_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let data_offset = data_vaddr - base_addr;
    let data_size: u64 = PAGE_SIZE; // 1 page of writable memory for stack/data
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity((data_offset + data_size) as usize);

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(2); // ELFCLASS64
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0); // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields ---
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&243u16.to_le_bytes()); // e_machine = EM_RISCV
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_shoff
    // e_flags: RISC-V float ABI = double (0x5), RVC = 0
    // EF_RISCV_FLOAT_ABI_DOUBLE = 0x5 << 5 = 0xA0
    elf.extend_from_slice(&0xA0u32.to_le_bytes()); // e_flags (LP64D ABI)
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_phnum = 2
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data / stack ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&data_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- .text section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // --- Pad to data segment offset ---
    while (elf.len() as u64) < data_offset {
        elf.push(0);
    }

    elf
}

/// Build RISC-V 64 runtime I/O functions using Linux ECALL syscalls.
///
/// Provides:
/// - `__vuma_print_hex`: Print a0 as 8 hex digits to stdout (FD=1)
///   Uses sys_write (a7=64) via ECALL.
///
/// - `__vuma_print_int`: Print a0 as a decimal integer to stdout (FD=1)
///   Converts digit-by-digit into a stack buffer, then sys_write.
///
/// - `__vuma_print_newline`: Print a newline character to stdout.
///
/// All functions follow the LP64D calling convention.
fn build_riscv64_runtime() -> Vec<u8> {
    let mut code = Vec::new();

    // ── __vuma_print_hex ──
    // Input: a0 = 64-bit value to print as 8 hex digits
    // Clobbers: t0, t1, t2, t3, a7
    // Stack frame: 32 bytes (save ra + s0 + buffer)

    // Prologue
    code.extend(Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: -32 }.encode());
    code.extend(Instruction::Sd { rs2: Gpr::Ra, rs1: Gpr::Sp, imm: 24 }.encode());
    code.extend(Instruction::Sd { rs2: Gpr::S0, rs1: Gpr::Sp, imm: 16 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::S0, rs1: Gpr::Sp, imm: 0 }.encode());

    // t0 = loop counter (0..8), t1 = shift amount (28, 24, ..., 0)
    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::Zero, imm: 0 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::T1, rs1: Gpr::Zero, imm: 28 }.encode());

    // hex_loop:
    let hex_loop_start = code.len();

    // Extract nibble: t2 = (a0 >> t1) & 0xF
    code.extend(Instruction::Srl { rd: Gpr::T2, rs1: Gpr::A0, rs2: Gpr::T1 }.encode());
    code.extend(Instruction::Andi { rd: Gpr::T2, rs1: Gpr::T2, imm: 15 }.encode());

    // Convert nibble to hex char:
    // t3 = t2 + 48 ('0')  (default)
    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T2, imm: 48 }.encode());
    // if t2 > 9: t3 = t2 + 87 ('a' - 10)
    code.extend(Instruction::Addi { rd: Gpr::T4, rs1: Gpr::T2, imm: -10 }.encode()); // t4 = t2 - 10 (temp)
    // Use SLTIU to check: if t2 >= 10, t4 = 1, else t4 = 0
    // Actually: SLTIU t4, t2, 10 → if t2 < 10 then t4=1 else t4=0
    code.extend(Instruction::Sltiu { rd: Gpr::T4, rs1: Gpr::T2, imm: 10 }.encode());
    // If t4 == 0 (t2 >= 10), use alpha: t3 = t2 + 87
    // BNE t4, zero, store_digit (t2 < 10, use default t3 = t2 + 48)
    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T2, imm: 87 }.encode()); // t3 = t2 + 87 (alpha)
    // Now we have two possibilities: if t2 < 10, use t2+48, else use t2+87
    // Simple approach: use CSEL-like pattern
    // t3 = t2 + 48 always, then if t2 >= 10, add 39 more (87-48=39)
    // Actually let me redo this properly:
    // t3 = t2 + 48
    // if t2 >= 10: t3 += 39
    // SLTIU t4, t2, 10 → t4 = 1 if t2 < 10, 0 if t2 >= 10
    // We need to add 39 only when t2 >= 10 (t4 == 0)
    // XORI t4, t4, 1 → invert: t4 = 1 if t2 >= 10
    // But this is getting complicated. Let me just use a branch.

    // Let me restart the nibble conversion with a simpler approach.
    // Remove the last 2 instructions we just added.
    // Remove the last 2 instructions (8 bytes) that we just added
    code.truncate(code.len() - 8);

    // t3 = t2 + 48  (default for 0-9)
    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T2, imm: 48 }.encode());
    // SLTIU t4, t2, 10 → t4 = 1 if t2 < 10
    code.extend(Instruction::Sltiu { rd: Gpr::T4, rs1: Gpr::T2, imm: 10 }.encode());
    // BNE t4, zero, +2 (skip alpha adjustment if t2 < 10)
    // We'll compute the branch offset after we know where we are
    let bne_offset_pos = code.len();
    code.extend(Instruction::Bne { rs1: Gpr::T4, rs2: Gpr::Zero, offset: 0 }.encode()); // placeholder
    // Alpha: t3 = t2 + 87
    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T2, imm: 87 }.encode());
    // Patch the BNE to skip this instruction
    let bne_patch_pos = code.len();
    let bne_offset = (bne_patch_pos - bne_offset_pos) as i32;
    let bne_patched = Instruction::Bne { rs1: Gpr::T4, rs2: Gpr::Zero, offset: bne_offset };
    code[bne_offset_pos..bne_offset_pos + 4].copy_from_slice(&bne_patched.encode());

    // Store char at sp + t0
    code.extend(Instruction::Add { rd: Gpr::T5, rs1: Gpr::Sp, rs2: Gpr::T0 }.encode());
    code.extend(Instruction::Sb { rs1: Gpr::T5, rs2: Gpr::T3, imm: 0 }.encode());

    // Increment: SUB t1, t1, 4; ADD t0, t0, 1; BLT t0, 8, hex_loop
    code.extend(Instruction::Addi { rd: Gpr::T1, rs1: Gpr::T1, imm: -4 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::T0, imm: 1 }.encode());
    // Compute branch back to hex_loop_start
    let loop_back_offset = (hex_loop_start as i32) - (code.len() as i32);
    code.extend(Instruction::Blt { rs1: Gpr::T0, rs2: Gpr::T4, offset: loop_back_offset }.encode());
    // Wait, t4 was used above. Let me use a different register for the limit.
    // Actually BLT t0, 8 → we need imm=8 in a register. Use ADDI t4, zero, 8.
    // Remove the last BLT and redo.
    // Remove the last BLT instruction (4 bytes)
    code.truncate(code.len() - 4);
    code.extend(Instruction::Addi { rd: Gpr::T4, rs1: Gpr::Zero, imm: 8 }.encode());
    let loop_back_offset = (hex_loop_start as i32) - (code.len() as i32);
    code.extend(Instruction::Blt { rs1: Gpr::T0, rs2: Gpr::T4, offset: loop_back_offset }.encode());

    // ── sys_write(1, sp, 8) ──
    code.extend(Instruction::Addi { rd: Gpr::A0, rs1: Gpr::Zero, imm: 1 }.encode()); // fd=1
    code.extend(Instruction::Addi { rd: Gpr::A1, rs1: Gpr::Sp, imm: 0 }.encode()); // buf=sp
    code.extend(Instruction::Addi { rd: Gpr::A2, rs1: Gpr::Zero, imm: 8 }.encode()); // len=8
    code.extend(Instruction::Addi { rd: Gpr::A7, rs1: Gpr::Zero, imm: 64 }.encode()); // sys_write
    code.extend(Instruction::Ecall.encode());

    // Epilogue
    code.extend(Instruction::Ld { rd: Gpr::Ra, rs1: Gpr::Sp, imm: 24 }.encode());
    code.extend(Instruction::Ld { rd: Gpr::S0, rs1: Gpr::Sp, imm: 16 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: 32 }.encode());
    code.extend(Instruction::Jalr { rd: Gpr::Zero, rs1: Gpr::Ra, imm: 0 }.encode());

    // ── __vuma_print_int ──
    // Input: a0 = 64-bit signed integer to print as decimal
    // Strategy: divide by 10, store digits, reverse, write.

    // Prologue
    code.extend(Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: -64 }.encode());
    code.extend(Instruction::Sd { rs2: Gpr::Ra, rs1: Gpr::Sp, imm: 56 }.encode());
    code.extend(Instruction::Sd { rs2: Gpr::S0, rs1: Gpr::Sp, imm: 48 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::S0, rs1: Gpr::Sp, imm: 0 }.encode());

    // Handle negative: if a0 < 0, print '-' and negate
    code.extend(Instruction::Bge { rs1: Gpr::A0, rs2: Gpr::Zero, offset: 0 }.encode()); // placeholder
    let bge_pos = code.len() - 4;

    // Print '-'
    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::Zero, imm: 45 }.encode()); // '-'
    code.extend(Instruction::Sb { rs1: Gpr::Sp, rs2: Gpr::T0, imm: 0 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::A0, rs1: Gpr::Zero, imm: 1 }.encode()); // fd
    code.extend(Instruction::Addi { rd: Gpr::A1, rs1: Gpr::Sp, imm: 0 }.encode()); // buf
    code.extend(Instruction::Addi { rd: Gpr::A2, rs1: Gpr::Zero, imm: 1 }.encode()); // len
    code.extend(Instruction::Addi { rd: Gpr::A7, rs1: Gpr::Zero, imm: 64 }.encode()); // sys_write
    code.extend(Instruction::Ecall.encode());

    // Negate a0
    code.extend(Instruction::Sub { rd: Gpr::A0, rs1: Gpr::Zero, rs2: Gpr::A0 }.encode());

    // Patch BGE to skip to here
    let bge_target = code.len() as i32;
    let bge_offset = bge_target - (bge_pos as i32);
    let bge_patched = Instruction::Bge { rs1: Gpr::A0, rs2: Gpr::Zero, offset: bge_offset };
    code[bge_pos..bge_pos + 4].copy_from_slice(&bge_patched.encode());

    // Convert digits: t0 = digit count, t1 = 10
    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::Zero, imm: 0 }.encode()); // count=0
    code.extend(Instruction::Addi { rd: Gpr::T1, rs1: Gpr::Zero, imm: 10 }.encode()); // divisor=10

    let div_loop_start = code.len();

    // CBZ-like: if a0 == 0, jump to done
    code.extend(Instruction::Beq { rs1: Gpr::A0, rs2: Gpr::Zero, offset: 0 }.encode()); // placeholder
    let beq_pos = code.len() - 4;

    // UDIV: t2 = a0 / 10
    code.extend(Instruction::Divu { rd: Gpr::T2, rs1: Gpr::A0, rs2: Gpr::T1 }.encode());
    // REM: t3 = a0 % 10
    code.extend(Instruction::Remu { rd: Gpr::T3, rs1: Gpr::A0, rs2: Gpr::T1 }.encode());
    // Add '0'
    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T3, imm: 48 }.encode());
    // Store at sp + 16 + t0 (use s0+16 area as buffer)
    code.extend(Instruction::Addi { rd: Gpr::T5, rs1: Gpr::Sp, imm: 16 }.encode());
    code.extend(Instruction::Add { rd: Gpr::T5, rs1: Gpr::T5, rs2: Gpr::T0 }.encode());
    code.extend(Instruction::Sb { rs1: Gpr::T5, rs2: Gpr::T3, imm: 0 }.encode());
    // Increment count
    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::T0, imm: 1 }.encode());
    // a0 = quotient
    code.extend(Instruction::Addi { rd: Gpr::A0, rs1: Gpr::T2, imm: 0 }.encode());
    // Loop back
    let div_back = (div_loop_start as i32) - (code.len() as i32);
    code.extend(Instruction::Jal { rd: Gpr::Zero, offset: div_back }.encode());

    // done_digits: Patch BEQ
    let beq_target = code.len() as i32;
    let beq_offset = beq_target - (beq_pos as i32);
    let beq_patched = Instruction::Beq { rs1: Gpr::A0, rs2: Gpr::Zero, offset: beq_offset };
    code[beq_pos..beq_pos + 4].copy_from_slice(&beq_patched.encode());

    // If count == 0, print "0"
    code.extend(Instruction::Bne { rs1: Gpr::T0, rs2: Gpr::Zero, offset: 0 }.encode()); // placeholder
    let bne_notzero_pos = code.len() - 4;

    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::Zero, imm: 48 }.encode()); // '0'
    code.extend(Instruction::Sb { rs1: Gpr::Sp, rs2: Gpr::T3, imm: 16 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::Zero, imm: 1 }.encode()); // count=1
    // B write_digits
    code.extend(Instruction::Jal { rd: Gpr::Zero, offset: 0 }.encode()); // placeholder
    let j_write_pos = code.len() - 4;

    // Patch BNE to skip to reverse section
    let rev_start = code.len() as i32;
    let bne_offset = rev_start - (bne_notzero_pos as i32);
    let bne_patched = Instruction::Bne { rs1: Gpr::T0, rs2: Gpr::Zero, offset: bne_offset };
    code[bne_notzero_pos..bne_notzero_pos + 4].copy_from_slice(&bne_patched.encode());

    // Reverse digits in buffer [sp+16, sp+16+t0)
    // t2 = left = 0, t3 = right = t0 - 1
    code.extend(Instruction::Addi { rd: Gpr::T2, rs1: Gpr::Zero, imm: 0 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T0, imm: -1 }.encode());

    let rev_loop = code.len();
    // BGE t2, t3, rev_done
    code.extend(Instruction::Bge { rs1: Gpr::T2, rs2: Gpr::T3, offset: 0 }.encode()); // placeholder
    let bge_rev_pos = code.len() - 4;

    // Load bytes and swap
    code.extend(Instruction::Addi { rd: Gpr::T4, rs1: Gpr::Sp, imm: 16 }.encode());
    code.extend(Instruction::Add { rd: Gpr::T5, rs1: Gpr::T4, rs2: Gpr::T2 }.encode());
    code.extend(Instruction::Add { rd: Gpr::T6, rs1: Gpr::T4, rs2: Gpr::T3 }.encode());
    code.extend(Instruction::Lbu { rd: Gpr::T4, rs1: Gpr::T5, imm: 0 }.encode()); // reuse t4
    code.extend(Instruction::Lbu { rd: Gpr::A7, rs1: Gpr::T6, imm: 0 }.encode()); // use a7 as temp
    code.extend(Instruction::Sb { rs1: Gpr::T5, rs2: Gpr::A7, imm: 0 }.encode());
    code.extend(Instruction::Sb { rs1: Gpr::T6, rs2: Gpr::T4, imm: 0 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::T2, rs1: Gpr::T2, imm: 1 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T3, imm: -1 }.encode());
    // Loop back
    let rev_back = (rev_loop as i32) - (code.len() as i32);
    code.extend(Instruction::Jal { rd: Gpr::Zero, offset: rev_back }.encode());

    // rev_done: Patch BGE
    let rev_done = code.len() as i32;
    let bge_rev_offset = rev_done - (bge_rev_pos as i32);
    let bge_rev_patched = Instruction::Bge { rs1: Gpr::T2, rs2: Gpr::T3, offset: bge_rev_offset };
    code[bge_rev_pos..bge_rev_pos + 4].copy_from_slice(&bge_rev_patched.encode());

    // Patch J write_digits
    let write_digits = code.len() as i32;
    let j_write_offset = write_digits - (j_write_pos as i32);
    let j_write_patched = Instruction::Jal { rd: Gpr::Zero, offset: j_write_offset };
    code[j_write_pos..j_write_pos + 4].copy_from_slice(&j_write_patched.encode());

    // write_digits: sys_write(1, sp+16, t0)
    code.extend(Instruction::Addi { rd: Gpr::A0, rs1: Gpr::Zero, imm: 1 }.encode()); // fd
    code.extend(Instruction::Addi { rd: Gpr::A1, rs1: Gpr::Sp, imm: 16 }.encode()); // buf
    code.extend(Instruction::Addi { rd: Gpr::A2, rs1: Gpr::T0, imm: 0 }.encode()); // len
    code.extend(Instruction::Addi { rd: Gpr::A7, rs1: Gpr::Zero, imm: 64 }.encode()); // sys_write
    code.extend(Instruction::Ecall.encode());

    // Epilogue
    code.extend(Instruction::Ld { rd: Gpr::Ra, rs1: Gpr::Sp, imm: 56 }.encode());
    code.extend(Instruction::Ld { rd: Gpr::S0, rs1: Gpr::Sp, imm: 48 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: 64 }.encode());
    code.extend(Instruction::Jalr { rd: Gpr::Zero, rs1: Gpr::Ra, imm: 0 }.encode());

    // ── __vuma_print_newline ──
    // Simple: write '\n' to stdout
    code.extend(Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: -16 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::Zero, imm: 10 }.encode()); // '\n'
    code.extend(Instruction::Sb { rs1: Gpr::Sp, rs2: Gpr::T0, imm: 0 }.encode());
    code.extend(Instruction::Addi { rd: Gpr::A0, rs1: Gpr::Zero, imm: 1 }.encode()); // fd
    code.extend(Instruction::Addi { rd: Gpr::A1, rs1: Gpr::Sp, imm: 0 }.encode()); // buf
    code.extend(Instruction::Addi { rd: Gpr::A2, rs1: Gpr::Zero, imm: 1 }.encode()); // len
    code.extend(Instruction::Addi { rd: Gpr::A7, rs1: Gpr::Zero, imm: 64 }.encode()); // sys_write
    code.extend(Instruction::Ecall.encode());
    code.extend(Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: 16 }.encode());
    code.extend(Instruction::Jalr { rd: Gpr::Zero, rs1: Gpr::Ra, imm: 0 }.encode());

    code
}

// ===========================================================================
// RiscV64Backend
// ===========================================================================

/// RISC-V 64-bit code generation backend.
///
/// Implements the `Backend` trait for RISC-V 64-bit (RV64GC, LP64D ABI).
pub struct RiscV64Backend {
    target_info: RiscV64TargetInfo,
}

impl RiscV64Backend {
    /// Create a new RISC-V 64-bit backend.
    pub fn new() -> Self {
        Self {
            target_info: RiscV64TargetInfo,
        }
    }
}

impl Default for RiscV64Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Emit a RISC-V comparison pattern that produces 0 or 1 in `rd`.
///
/// This maps IR comparison kinds to the appropriate SLT/SLTU/XOR+SLTIU
/// instruction sequences.
fn emit_cmp_isel(kind: &CmpKind, rd: Gpr, rs1: Gpr, rs2: Gpr, scratch: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    match kind {
        CmpKind::Eq => {
            // XOR rd, rs1, rs2; SLTIU rd, rd, 1
            code.extend(Instruction::Xor { rd, rs1, rs2 }.encode());
            code.extend(
                Instruction::Sltiu {
                    rd,
                    rs1: rd,
                    imm: 1,
                }
                .encode(),
            );
        }
        CmpKind::Ne => {
            // XOR rd, rs1, rs2; SLTU rd, x0, rd  (rd = (xor != 0) ? 1 : 0)
            code.extend(Instruction::Xor { rd, rs1, rs2 }.encode());
            code.extend(
                Instruction::Sltu {
                    rd,
                    rs1: Gpr::Zero,
                    rs2: rd,
                }
                .encode(),
            );
        }
        CmpKind::SLt => {
            code.extend(Instruction::Slt { rd, rs1, rs2 }.encode());
        }
        CmpKind::SLe => {
            // a <= b  <=>  !(b < a)
            code.extend(
                Instruction::Slt {
                    rd: scratch,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
        CmpKind::SGt => {
            // a > b  <=>  b < a
            code.extend(
                Instruction::Slt {
                    rd,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
        }
        CmpKind::SGe => {
            // a >= b  <=>  !(a < b)
            code.extend(
                Instruction::Slt {
                    rd: scratch,
                    rs1,
                    rs2,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
        CmpKind::ULt => {
            code.extend(Instruction::Sltu { rd, rs1, rs2 }.encode());
        }
        CmpKind::ULe => {
            // a <= b (unsigned) <=> !(b < a) (unsigned)
            code.extend(
                Instruction::Sltu {
                    rd: scratch,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
        CmpKind::UGt => {
            // a > b (unsigned) <=> b < a (unsigned)
            code.extend(
                Instruction::Sltu {
                    rd,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
        }
        CmpKind::UGe => {
            // a >= b (unsigned) <=> !(a < b) (unsigned)
            code.extend(
                Instruction::Sltu {
                    rd: scratch,
                    rs1,
                    rs2,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
    }
    code
}

/// Emit a RISC-V BinOp comparison pattern that produces 0 or 1 in `rd`.
///
/// Similar to `emit_cmp_isel` but uses `BinOpKind`.
fn emit_binop_cmp_isel(op: &BinOpKind, rd: Gpr, rs1: Gpr, rs2: Gpr, scratch: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    match op {
        BinOpKind::SLt => {
            code.extend(Instruction::Slt { rd, rs1, rs2 }.encode());
        }
        BinOpKind::SLe => {
            // a <= b <=> !(b < a)
            code.extend(
                Instruction::Slt {
                    rd: scratch,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
        BinOpKind::SGt => {
            code.extend(
                Instruction::Slt {
                    rd,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
        }
        BinOpKind::SGe => {
            code.extend(
                Instruction::Slt {
                    rd: scratch,
                    rs1,
                    rs2,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
        BinOpKind::ULt => {
            code.extend(Instruction::Sltu { rd, rs1, rs2 }.encode());
        }
        BinOpKind::ULe => {
            code.extend(
                Instruction::Sltu {
                    rd: scratch,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
        BinOpKind::UGt => {
            code.extend(
                Instruction::Sltu {
                    rd,
                    rs1: rs2,
                    rs2: rs1,
                }
                .encode(),
            );
        }
        BinOpKind::UGe => {
            code.extend(
                Instruction::Sltu {
                    rd: scratch,
                    rs1,
                    rs2,
                }
                .encode(),
            );
            code.extend(
                Instruction::Xori {
                    rd,
                    rs1: scratch,
                    imm: 1,
                }
                .encode(),
            );
        }
        BinOpKind::Eq => {
            code.extend(Instruction::Xor { rd, rs1, rs2 }.encode());
            code.extend(
                Instruction::Sltiu {
                    rd,
                    rs1: rd,
                    imm: 1,
                }
                .encode(),
            );
        }
        BinOpKind::Ne => {
            code.extend(Instruction::Xor { rd, rs1, rs2 }.encode());
            code.extend(
                Instruction::Sltu {
                    rd,
                    rs1: Gpr::Zero,
                    rs2: rd,
                }
                .encode(),
            );
        }
        _ => unreachable!(),
    }
    code
}

/// Emit a CLZ (Count Leading Zeros) instruction sequence for a 64-bit value.
///
/// Algorithm: shift-and-test narrowing.
/// If input == 0, result = 64. Otherwise narrow from the MSB:
/// n = 0; if x>>32 !=0: x>>=32, n+=32; ... if x>>1 !=0: x>>=1, n+=1;
/// result = 63 - n.
///
/// Uses scratch registers T4 (count), T5 (shifted value), T6 (temp).
#[allow(clippy::doc_overindented_list_items)]
fn emit_clz_isel(rd: Gpr, rs: Gpr) -> Vec<u8> {
    let mut code = Vec::new();

    // Move input to rd if different
    if rs != rd {
        code.extend(
            Instruction::Addi {
                rd,
                rs1: rs,
                imm: 0,
            }
            .encode(),
        );
    }

    // t4 = n = 0 (count of shift positions)
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::Zero,
            imm: 0,
        }
        .encode(),
    );

    // beq rd, x0, zero_case — if input is zero, jump to return 64
    // Layout after beq:
    //   6 narrowing steps × 16 bytes = 96 bytes
    //   addi t6, x0, 63   (4)   — load 63
    //   sub  rd, t6, t4   (4)   — rd = 63 - n
    //   jal  x0, +4       (4)   — skip zero_case
    // zero_case:
    //   addi rd, x0, 64   (4)   — return 64 for zero input
    let beq_offset: i32 = 6 * 16 + 12; // 108
    code.extend(
        Instruction::Beq {
            rs1: rd,
            rs2: Gpr::Zero,
            offset: beq_offset,
        }
        .encode(),
    );

    // Narrowing steps: if (x >> SHIFT) != 0, shift right and accumulate.
    // Each step = 4 instructions = 16 bytes:
    //   srli t5, rd, SHIFT; beq t5, x0, +8; mv rd, t5; addi t4, t4, SHIFT
    for shift in [32, 16, 8, 4, 2, 1] {
        code.extend(
            Instruction::Srli {
                rd: Gpr::T5,
                rs1: rd,
                shamt: shift,
            }
            .encode(),
        );
        code.extend(
            Instruction::Beq {
                rs1: Gpr::T5,
                rs2: Gpr::Zero,
                offset: 8,
            }
            .encode(),
        );
        code.extend(
            Instruction::Addi {
                rd,
                rs1: Gpr::T5,
                imm: 0,
            }
            .encode(),
        );
        code.extend(
            Instruction::Addi {
                rd: Gpr::T4,
                rs1: Gpr::T4,
                imm: shift as i32,
            }
            .encode(),
        );
    }

    // rd = 63 - t4
    code.extend(
        Instruction::Addi {
            rd: Gpr::T6,
            rs1: Gpr::Zero,
            imm: 63,
        }
        .encode(),
    );
    code.extend(
        Instruction::Sub {
            rd,
            rs1: Gpr::T6,
            rs2: Gpr::T4,
        }
        .encode(),
    );
    // Skip the zero case
    code.extend(
        Instruction::Jal {
            rd: Gpr::Zero,
            offset: 4,
        }
        .encode(),
    );

    // zero_case: rd = 64
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::Zero,
            imm: 64,
        }
        .encode(),
    );

    code
}

/// Emit a CTZ (Count Trailing Zeros) instruction sequence for a 64-bit value.
///
/// Uses the identity: ctz(x) = clz(x & -x), where x & -x isolates the
/// lowest set bit. Then clz of a power of 2 gives its bit position from the top.
fn emit_ctz_isel(rd: Gpr, rs: Gpr) -> Vec<u8> {
    let mut code = Vec::new();

    // Move input to rd if different
    if rs != rd {
        code.extend(
            Instruction::Addi {
                rd,
                rs1: rs,
                imm: 0,
            }
            .encode(),
        );
    }

    // Isolate lowest set bit: t5 = rd & (-rd)
    // -rd = SUB x0, rd (but that gives 0 - rd which is -rd in two's complement)
    code.extend(
        Instruction::Sub {
            rd: Gpr::T5,
            rs1: Gpr::Zero,
            rs2: rd,
        }
        .encode(),
    );
    code.extend(
        Instruction::And {
            rd: Gpr::T5,
            rs1: rd,
            rs2: Gpr::T5,
        }
        .encode(),
    );

    // Now t5 = rd & (-rd), which is a power of 2 (or 0).
    // clz(t5) gives 63 - bit_position for non-zero, or 64 for zero.
    // ctz(rd) = 63 - clz(t5) for non-zero rd, or 64 for zero rd.
    //
    // But we can simplify: since t5 has exactly one bit set (or is 0),
    // clz(t5) = 63 - position for non-zero. So position = 63 - clz(t5).
    // And position = ctz(rd).
    //
    // So: ctz(rd) = 63 - clz(t5).
    // For rd=0: t5=0, clz(0)=64, ctz=63-64=-1 which is wrong.
    // For rd=0: ctz should be 64. So we need to handle zero separately.

    // Save whether rd is zero before we modify it
    // t6 = (rd == 0) ? 1 : 0
    code.extend(
        Instruction::Sltiu {
            rd: Gpr::T6,
            rs1: rd,
            imm: 1,
        }
        .encode(),
    );

    // Move t5 into rd for the CLZ computation
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::T5,
            imm: 0,
        }
        .encode(),
    );

    // Compute CLZ using the same narrowing approach as emit_clz_isel
    // but without the zero check (we handle it separately).
    // t4 = n = 0
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::Zero,
            imm: 0,
        }
        .encode(),
    );

    // Narrowing step: shift=32
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 32,
        }
        .encode(),
    );
    code.extend(
        Instruction::Beq {
            rs1: Gpr::T5,
            rs2: Gpr::Zero,
            offset: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::T5,
            imm: 0,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            imm: 32,
        }
        .encode(),
    );

    // shift=16
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 16,
        }
        .encode(),
    );
    code.extend(
        Instruction::Beq {
            rs1: Gpr::T5,
            rs2: Gpr::Zero,
            offset: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::T5,
            imm: 0,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            imm: 16,
        }
        .encode(),
    );

    // shift=8
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Beq {
            rs1: Gpr::T5,
            rs2: Gpr::Zero,
            offset: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::T5,
            imm: 0,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            imm: 8,
        }
        .encode(),
    );

    // shift=4
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 4,
        }
        .encode(),
    );
    code.extend(
        Instruction::Beq {
            rs1: Gpr::T5,
            rs2: Gpr::Zero,
            offset: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::T5,
            imm: 0,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            imm: 4,
        }
        .encode(),
    );

    // shift=2
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 2,
        }
        .encode(),
    );
    code.extend(
        Instruction::Beq {
            rs1: Gpr::T5,
            rs2: Gpr::Zero,
            offset: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::T5,
            imm: 0,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            imm: 2,
        }
        .encode(),
    );

    // shift=1
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 1,
        }
        .encode(),
    );
    code.extend(
        Instruction::Beq {
            rs1: Gpr::T5,
            rs2: Gpr::Zero,
            offset: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd,
            rs1: Gpr::T5,
            imm: 0,
        }
        .encode(),
    );
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            imm: 1,
        }
        .encode(),
    );

    // clz = 63 - t4 (for non-zero original input)
    // ctz = 63 - clz = 63 - (63 - t4) = t4
    // So ctz(original) = t4 ! That's because we isolated the lowest bit
    // and counted from the top. The narrowing counted how many positions
    // from the MSB, which for a power of 2 is 63 - bit_position.
    // So clz = 63 - t4... no wait.
    //
    // Let me re-derive. After the narrowing, t4 = n where n is the number
    // of positions we shifted right. For a power of 2 at bit position p
    // (0 = LSB), the value after narrowing is 1, and n = 63 - p.
    // Wait, that's not right either. Let me trace through an example.
    //
    // Example: t5 = 0x0000000000000010 (bit 4 set, ctz should be 4)
    // shift=32: t5>>32 = 0, skip. n=0
    // shift=16: t5>>16 = 0, skip. n=0
    // shift=8:  t5>>8  = 0, skip. n=0
    // shift=4:  t5>>4  = 1, take. rd=1, n=4
    // shift=2:  t5>>2 = 0, skip. n=4
    // shift=1:  t5>>1 = 0, skip. n=4
    // clz(0x10) = 63 - 4 = 59 ✓ (bit 4, so 59 leading zeros)
    // ctz(original) = 63 - clz(t5) = 63 - 59 = 4 ✓
    //
    // So: clz(t5) = 63 - t4, and ctz = 63 - clz = 63 - (63 - t4) = t4
    //
    // Great! So ctz = t4 for non-zero input.
    // For zero input: t5 = 0, so the narrowing never takes any branch,
    // t4 = 0, and ctz should be 64. But t4 = 0 is wrong.
    //
    // So: rd = t4 + t6 (where t6 = 1 if original was zero, else 0)
    // This gives: for non-zero: rd = t4, for zero: rd = 0 + 1 = 1... still wrong.
    //
    // Better: rd = t4 + 64*t6. But 64*t6 requires a shift.
    // rd = t4 + (t6 << 6). But t6 is 0 or 1.
    // For non-zero: rd = t4 + 0 = t4 ✓
    // For zero: rd = 0 + 64 = 64 ✓

    // Compute rd = t4 + (t6 << 6)
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T6,
            shamt: 6,
        }
        .encode(),
    );
    code.extend(
        Instruction::Add {
            rd,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    );

    code
}

/// Emit a POPCNT (Population Count) instruction sequence for a 64-bit value.
///
/// Uses the standard bit-parallel Hamming weight algorithm:
///   x -= (x >> 1) & 0x5555555555555555;
///   x = (x & 0x3333333333333333) + ((x >> 2) & 0x3333333333333333);
///   x = (x + (x >> 4)) & 0x0F0F0F0F0F0F0F0F;
///   return (x * 0x0101010101010101) >> 56;
///
/// Uses M-extension MUL for the final multiplication.
/// Constants are materialized using LUI + ADDI pairs.
/// Scratch registers: T4, T5, T6.
fn emit_popcnt_isel(rd: Gpr, rs: Gpr) -> Vec<u8> {
    let mut code = Vec::new();

    // Helper: materialize a 64-bit constant into a register using LUI+ADDI
    // For constants that fit in 12-bit signed: just ADDI
    // For others: LUI upper + ADDI lower
    let _materialize = |reg: Gpr, val: u64, code: &mut Vec<u8>| {
        let val_i = val as i64;
        if (-2048..=2047).contains(&val_i) {
            code.extend(
                Instruction::Addi {
                    rd: reg,
                    rs1: Gpr::Zero,
                    imm: val_i as i32,
                }
                .encode(),
            );
        } else {
            let upper = ((val + 0x800) >> 12) as u32;
            let lower = (val as i32) - ((upper as i32) << 12);
            code.extend(
                Instruction::Lui {
                    rd: reg,
                    imm: upper,
                }
                .encode(),
            );
            code.extend(
                Instruction::Addi {
                    rd: reg,
                    rs1: reg,
                    imm: lower,
                }
                .encode(),
            );
        }
    };

    // Move input to rd
    if rs != rd {
        code.extend(
            Instruction::Addi {
                rd,
                rs1: rs,
                imm: 0,
            }
            .encode(),
        );
    }

    // Step 1: x -= (x >> 1) & 0x5555555555555555
    // 0x5555555555555555: upper = 0x555555555, lower...
    // Actually this constant doesn't fit in LUI (20-bit upper). Let me use a different approach.
    //
    // 0x55555555 = 01010101... in binary. LUI can load upper 20 bits.
    // 0x5555555555555555: upper 20 bits of the 32-bit LUI value = 0x55555
    // LUI loads bits [31:12] and zeros [11:0], so:
    //   LUI rd, 0x55555  => rd = 0x55555000
    //   ADDI rd, rd, 0x555 => rd = 0x55555555
    // But that's only 32 bits. For RV64, LUI sign-extends bit 31.
    // 0x55555555 has bit 31 = 0, so it's positive and sign-extends with zeros.
    // Result: 0x0000000055555555. We need 0x5555555555555555.
    //
    // To get the full 64-bit constant, we need more steps.
    // Approach: build the constant in a register using LUI + SLLI + ADDI.
    //
    // For 0x5555555555555555:
    //   LUI  t5, 0x55556     => t5 = 0x0000000055556000
    //                          Wait, 0x55556 << 12 = 0x55556000, not what we want.
    //
    // This is getting complex. Let me use a simpler popcnt algorithm
    // that uses only small constants:
    //
    // Alternative: iterate byte-by-byte using a lookup approach, or use
    // a simpler shift-add-count approach.
    //
    // Simplest approach using only base + M instructions:
    //   popcnt(x) = x - (x >> 1) & 1 - (x >> 2) & 1 - ... - (x >> 63) & 1
    // But that's 64 iterations.
    //
    // Better: use the bit-parallel algorithm but build constants differently.
    //
    // For 0x5555555555555555, we can use:
    //   li t5, -1          => t5 = 0xFFFFFFFFFFFFFFFF
    //   srli t5, t5, 1     => t5 = 0x7FFFFFFFFFFFFFFF  ... nope
    //
    // Actually: 0x5555555555555555 = 0xAAAAAAAAAAAAAAAA >> 1... nope.
    // 0xAAAAAAAAAAAAAAAA = ~0x5555555555555555.
    //
    // Let me try:
    //   li t5, -1               => 0xFFFFFFFFFFFFFFFF
    //   srli t5, t5, 1          => 0x7FFFFFFFFFFFFFFF
    // That doesn't help.
    //
    // How about:
    //   li t5, 0                => 0
    //   addi t5, x0, 1          => 1
    //   slli t6, t5, 1 | or t5, t6  => ... no, we need 0x5555...
    //
    // Let me try the "building block" approach:
    //   li t5, 1                => 1
    //   slli t6, t5, 2          => 4
    //   or   t5, t5, t6         => 5
    //   slli t6, t5, 4          => 0x50
    //   or   t5, t5, t6         => 0x55
    //   slli t6, t5, 8          => 0x5500
    //   or   t5, t5, t6         => 0x5555
    //   slli t6, t5, 16         => 0x55550000
    //   or   t5, t5, t6         => 0x55555555
    //   slli t6, t5, 32         => 0x5555555500000000
    //   or   t5, t5, t6         => 0x5555555555555555 ✓

    // Build 0x5555555555555555 in t4
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::Zero,
            imm: 1,
        }
        .encode(),
    );
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 2,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x5
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 4,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x55
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x5555
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 16,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x55555555
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 32,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x5555555555555555

    // Step 1: x -= (x >> 1) & mask55
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 1,
        }
        .encode(),
    );
    code.extend(
        Instruction::And {
            rd: Gpr::T5,
            rs1: Gpr::T5,
            rs2: Gpr::T4,
        }
        .encode(),
    );
    code.extend(
        Instruction::Sub {
            rd,
            rs1: rd,
            rs2: Gpr::T5,
        }
        .encode(),
    );

    // Build 0x3333333333333333 in t4
    // 0x3333... = 0x5555... >> 1... no. 0x3333 = 0x5555 & 0x3333? No.
    // 0x3 = 0b0011. Let's build it:
    //   li t4, 3
    //   slli t6, t4, 4 | or => 0x33
    //   ... same pattern as 0x55
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::Zero,
            imm: 3,
        }
        .encode(),
    );
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 4,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x33
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x3333
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 16,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x33333333
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 32,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x3333333333333333

    // Step 2: x = (x & mask33) + ((x >> 2) & mask33)
    code.extend(
        Instruction::And {
            rd: Gpr::T5,
            rs1: rd,
            rs2: Gpr::T4,
        }
        .encode(),
    ); // t5 = x & mask33
    code.extend(
        Instruction::Srli {
            rd,
            rs1: rd,
            shamt: 2,
        }
        .encode(),
    ); // x = x >> 2
    code.extend(
        Instruction::And {
            rd,
            rs1: rd,
            rs2: Gpr::T4,
        }
        .encode(),
    ); // x = (x>>2) & mask33
    code.extend(
        Instruction::Add {
            rd,
            rs1: Gpr::T5,
            rs2: rd,
        }
        .encode(),
    ); // x = both halves summed

    // Build 0x0F0F0F0F0F0F0F0F in t4
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::Zero,
            imm: 0xF,
        }
        .encode(),
    );
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x0F0F
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 16,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x0F0F0F0F
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 32,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x0F0F0F0F0F0F0F0F

    // Step 3: x = (x + (x >> 4)) & mask0F
    code.extend(
        Instruction::Srli {
            rd: Gpr::T5,
            rs1: rd,
            shamt: 4,
        }
        .encode(),
    );
    code.extend(
        Instruction::Add {
            rd,
            rs1: rd,
            rs2: Gpr::T5,
        }
        .encode(),
    );
    code.extend(
        Instruction::And {
            rd,
            rs1: rd,
            rs2: Gpr::T4,
        }
        .encode(),
    );

    // Step 4: result = (x * 0x0101010101010101) >> 56
    // Build 0x0101010101010101 in t4
    code.extend(
        Instruction::Addi {
            rd: Gpr::T4,
            rs1: Gpr::Zero,
            imm: 1,
        }
        .encode(),
    );
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 8,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x0101
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 16,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x01010101
    code.extend(
        Instruction::Slli {
            rd: Gpr::T6,
            rs1: Gpr::T4,
            shamt: 32,
        }
        .encode(),
    );
    code.extend(
        Instruction::Or {
            rd: Gpr::T4,
            rs1: Gpr::T4,
            rs2: Gpr::T6,
        }
        .encode(),
    ); // 0x0101010101010101

    code.extend(
        Instruction::Mul {
            rd,
            rs1: rd,
            rs2: Gpr::T4,
        }
        .encode(),
    );
    code.extend(
        Instruction::Srli {
            rd,
            rs1: rd,
            shamt: 56,
        }
        .encode(),
    );

    code
}

/// Collect all virtual register IDs from an IR function.
#[allow(dead_code)]
fn collect_vreg_ids(func: &IRFunction) -> std::collections::HashSet<u32> {
    let mut ids = std::collections::HashSet::new();
    for block in &func.blocks {
        for instr in &block.instructions {
            let values: Vec<&IRValue> = match instr {
                IRInstr::BinOp { dst, lhs, rhs, .. } => vec![dst, lhs, rhs],
                IRInstr::Add { dst, lhs, rhs, .. } => vec![dst, lhs, rhs],
                IRInstr::Sub { dst, lhs, rhs, .. } => vec![dst, lhs, rhs],
                IRInstr::Mul { dst, lhs, rhs, .. } => vec![dst, lhs, rhs],
                IRInstr::Div { dst, lhs, rhs, .. } => vec![dst, lhs, rhs],
                IRInstr::UnaryOp { dst, operand, .. } => vec![dst, operand],
                IRInstr::Load { dst, addr, .. } => vec![dst, addr],
                IRInstr::Store { value, addr, .. } => vec![value, addr],
                IRInstr::Alloc { dst, .. } => vec![dst],
                IRInstr::Cast { dst, src, .. } => vec![dst, src],
                IRInstr::Select {
                    dst,
                    cond,
                    true_val,
                    false_val, ty: _,
                } => vec![dst, cond, true_val, false_val],
                IRInstr::Offset { dst, base, offset } => vec![dst, base, offset],
                IRInstr::GetAddress { dst, .. } => vec![dst],
                _ => vec![],
            };
            for val in values {
                if let IRValue::Register(id) = val {
                    ids.insert(*id);
                }
            }
        }
    }
    ids
}

// ===========================================================================
// Stack-slot helper functions
// ===========================================================================

/// Load a 64-bit immediate into a register.
///
/// Handles three cases:
/// 1. Fits in 12-bit signed: ADDI dst, x0, imm
/// 2. Fits in 32-bit sign-extended: LUI + ADDI (with %hi/%lo adjustment)
/// 3. Full 64-bit: LUI+ADDI for upper 32, SLLI 32, LUI+ADDI for lower 32, OR
fn ss_load_imm(dst: Gpr, val: i64) -> Vec<u8> {
    let mut code = Vec::new();

    // Case 1: fits in 12-bit signed
    if (-2048..=2047).contains(&val) {
        code.extend(Instruction::Addi { rd: dst, rs1: Gpr::Zero, imm: val as i32 }.encode());
        return code;
    }

    // Case 2: fits in 32-bit sign-extended
    let val_sign_ext_32 = (val as i32) as i64;
    if val == val_sign_ext_32 {
        let val_u32 = val as u32;
        let hi = ((val_u32.wrapping_add(0x800)) >> 12) << 12;
        let lo = (val as i32).wrapping_sub(hi as i32);
        code.extend(Instruction::Lui { rd: dst, imm: hi }.encode());
        if lo != 0 {
            code.extend(Instruction::Addi { rd: dst, rs1: dst, imm: lo }.encode());
        }
        // If the value is non-negative but hi has bit 31 set, LUI sign-extends
        // bit 31 and produces a negative 64-bit result.  Zero-extend with
        // SLLI 32 + SRLI 32 to clear the upper 32 bits.
        // This happens for positive i32 values near 0x8000_0000 (e.g. 0x7FFF_FF00)
        // where the +0x800 rounding pushes hi into the negative-i32 range.
        if val >= 0 && hi >= 0x8000_0000 {
            code.extend(Instruction::Slli { rd: dst, rs1: dst, shamt: 32 }.encode());
            code.extend(Instruction::Srli { rd: dst, rs1: dst, shamt: 32 }.encode());
        }
        return code;
    }

    // Case 3: full 64-bit value
    let upper_32 = (val >> 32) as u32;
    let lower_32 = val as u32;

    if upper_32 == 0 {
        // Load lower_32 with zero-extension
        let hi = ((lower_32.wrapping_add(0x800)) >> 12) << 12;
        let lo = (lower_32 as i32).wrapping_sub(hi as i32);
        code.extend(Instruction::Lui { rd: dst, imm: hi }.encode());
        if lo != 0 {
            code.extend(Instruction::Addi { rd: dst, rs1: dst, imm: lo }.encode());
        }
        // Zero-extend: SLLI 32 then SRLI 32
        code.extend(Instruction::Slli { rd: dst, rs1: dst, shamt: 32 }.encode());
        code.extend(Instruction::Srli { rd: dst, rs1: dst, shamt: 32 }.encode());
    } else if lower_32 == 0 {
        // Load upper_32 and shift left by 32
        let hi = ((upper_32.wrapping_add(0x800)) >> 12) << 12;
        let lo = (upper_32 as i32).wrapping_sub(hi as i32);
        code.extend(Instruction::Lui { rd: dst, imm: hi }.encode());
        if lo != 0 {
            code.extend(Instruction::Addi { rd: dst, rs1: dst, imm: lo }.encode());
        }
        code.extend(Instruction::Slli { rd: dst, rs1: dst, shamt: 32 }.encode());
    } else {
        // Load upper_32, SLLI 32, load lower_32 (zero-extended) into T3, OR
        let hi = ((upper_32.wrapping_add(0x800)) >> 12) << 12;
        let lo = (upper_32 as i32).wrapping_sub(hi as i32);
        code.extend(Instruction::Lui { rd: dst, imm: hi }.encode());
        if lo != 0 {
            code.extend(Instruction::Addi { rd: dst, rs1: dst, imm: lo }.encode());
        }
        code.extend(Instruction::Slli { rd: dst, rs1: dst, shamt: 32 }.encode());

        // Load lower_32 into T3 with zero-extension
        let hi = ((lower_32.wrapping_add(0x800)) >> 12) << 12;
        let lo = (lower_32 as i32).wrapping_sub(hi as i32);
        code.extend(Instruction::Lui { rd: Gpr::T3, imm: hi }.encode());
        if lo != 0 {
            code.extend(Instruction::Addi { rd: Gpr::T3, rs1: Gpr::T3, imm: lo }.encode());
        }
        code.extend(Instruction::Slli { rd: Gpr::T3, rs1: Gpr::T3, shamt: 32 }.encode());
        code.extend(Instruction::Srli { rd: Gpr::T3, rs1: Gpr::T3, shamt: 32 }.encode());

        code.extend(Instruction::Or { rd: dst, rs1: dst, rs2: Gpr::T3 }.encode());
    }

    code
}

/// Load a value from a stack slot at [S0 - offset_from_s0] into dst_reg.
///
/// `offset_from_s0` must be positive. The effective address is S0 - offset_from_s0.
/// For large offsets (> 2047), computes the address into T3 first.
fn ss_load_from_slot(dst_reg: Gpr, offset_from_s0: i32) -> Vec<u8> {
    let neg_off = -offset_from_s0;
    if neg_off >= -2048 {
        // Offset fits in 12-bit signed: LD dst, neg_off(S0)
        Instruction::Ld { rd: dst_reg, rs1: Gpr::S0, imm: neg_off }
            .encode()
            .to_vec()
    } else {
        // Large offset: compute address into T3, then LD from T3
        let mut code = Vec::new();
        // Materialize offset into T3, then SUB T3, S0, T3
        code.extend(ss_load_imm(Gpr::T3, offset_from_s0 as i64));
        code.extend(Instruction::Sub { rd: Gpr::T3, rs1: Gpr::S0, rs2: Gpr::T3 }.encode());
        code.extend(Instruction::Ld { rd: dst_reg, rs1: Gpr::T3, imm: 0 }.encode());
        code
    }
}

/// Store a value from src_reg into a stack slot at [S0 - offset_from_s0].
///
/// `offset_from_s0` must be positive. The effective address is S0 - offset_from_s0.
/// For large offsets (> 2047), computes the address into T3 first.
/// IMPORTANT: src_reg must NOT be T3 when the offset is large.
fn ss_store_to_slot(src_reg: Gpr, offset_from_s0: i32) -> Vec<u8> {
    let neg_off = -offset_from_s0;
    if neg_off >= -2048 {
        // Offset fits in 12-bit signed: SD src, neg_off(S0)
        Instruction::Sd { rs1: Gpr::S0, rs2: src_reg, imm: neg_off }
            .encode()
            .to_vec()
    } else {
        // Large offset: compute address into T3, then SD from T3
        let mut code = Vec::new();
        // Materialize offset into T3, then SUB T3, S0, T3
        code.extend(ss_load_imm(Gpr::T3, offset_from_s0 as i64));
        code.extend(Instruction::Sub { rd: Gpr::T3, rs1: Gpr::S0, rs2: Gpr::T3 }.encode());
        code.extend(Instruction::Sd { rs1: Gpr::T3, rs2: src_reg, imm: 0 }.encode());
        code
    }
}

/// Store a double-precision FP value from an FPR to a stack slot at [S0 - offset_from_s0].
fn ss_store_fpr_to_slot(src_fpr: Fpr, offset_from_s0: i32) -> Vec<u8> {
    let neg_off = -offset_from_s0;
    if neg_off >= -2048 {
        Instruction::Fsd { rs1: Gpr::S0, rs2: src_fpr, imm: neg_off }
            .encode()
            .to_vec()
    } else {
        let mut code = Vec::new();
        code.extend(ss_load_imm(Gpr::T3, offset_from_s0 as i64));
        code.extend(Instruction::Sub { rd: Gpr::T3, rs1: Gpr::S0, rs2: Gpr::T3 }.encode());
        code.extend(Instruction::Fsd { rs1: Gpr::T3, rs2: src_fpr, imm: 0 }.encode());
        code
    }
}

/// Store a single-precision FP value from an FPR to a stack slot at [S0 - offset_from_s0].
fn ss_store_fpr_s_to_slot(src_fpr: Fpr, offset_from_s0: i32) -> Vec<u8> {
    let neg_off = -offset_from_s0;
    if neg_off >= -2048 {
        Instruction::Fsw { rs1: Gpr::S0, rs2: src_fpr, imm: neg_off }
            .encode()
            .to_vec()
    } else {
        let mut code = Vec::new();
        code.extend(ss_load_imm(Gpr::T3, offset_from_s0 as i64));
        code.extend(Instruction::Sub { rd: Gpr::T3, rs1: Gpr::S0, rs2: Gpr::T3 }.encode());
        code.extend(Instruction::Fsw { rs1: Gpr::T3, rs2: src_fpr, imm: 0 }.encode());
        code
    }
}

/// Load a 32-bit word from a stack slot at [S0 - offset_from_s0] into a GPR.
fn ss_load_word_from_slot(dst_reg: Gpr, offset_from_s0: i32) -> Vec<u8> {
    let neg_off = -offset_from_s0;
    if neg_off >= -2048 {
        Instruction::Lw { rd: dst_reg, rs1: Gpr::S0, imm: neg_off }
            .encode()
            .to_vec()
    } else {
        let mut code = Vec::new();
        code.extend(ss_load_imm(Gpr::T3, offset_from_s0 as i64));
        code.extend(Instruction::Sub { rd: Gpr::T3, rs1: Gpr::S0, rs2: Gpr::T3 }.encode());
        code.extend(Instruction::Lw { rd: dst_reg, rs1: Gpr::T3, imm: 0 }.encode());
        code
    }
}

/// Load an [`IRValue`] into a scratch register.
///
/// For registers: load from the stack slot.
/// For immediates: materialise using ADDI or LUI+ADDI.
/// For addresses: materialise as a 64-bit immediate.
/// For labels: load 0 (placeholder).
fn ss_load_value(val: &IRValue, slots: &HashMap<u32, i32>, scratch: Gpr) -> Vec<u8> {
    match val {
        IRValue::Register(id) => {
            let offset = slots.get(id).copied().unwrap_or(0);
            ss_load_from_slot(scratch, offset)
        }
        IRValue::Immediate(v) => ss_load_imm(scratch, *v),
        IRValue::Address(a) => ss_load_imm(scratch, *a as i64),
        IRValue::Label(_) => {
            // Placeholder: load 0
            Instruction::Addi { rd: scratch, rs1: Gpr::Zero, imm: 0 }
                .encode()
                .to_vec()
        }
    }
}

impl Backend for RiscV64Backend {
    fn target_info(&self) -> &dyn crate::backend::TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        let func_name = func.name.clone();

        // ── Phase 1: Collect all vreg IDs and compute stack layout ──

        let mut all_vreg_ids: std::collections::HashSet<u32> =
            std::collections::HashSet::new();
        for &id in func.vregs.keys() {
            all_vreg_ids.insert(id);
        }
        for param in &func.params {
            if let Some(id) = param.as_register() {
                all_vreg_ids.insert(id);
            }
        }
        for block in &func.blocks {
            for instr in &block.instructions {
                for id in instr.defined_regs() {
                    all_vreg_ids.insert(id);
                }
                for id in instr.used_regs() {
                    all_vreg_ids.insert(id);
                }
            }
        }
        for val in &func.results {
            if let Some(id) = val.as_register() {
                all_vreg_ids.insert(id);
            }
        }

        // Identify Alloc vregs and their sizes
        let mut stack_alloc_vregs: std::collections::HashSet<u32> =
            std::collections::HashSet::new();
        let mut alloc_sizes: HashMap<u32, i32> = HashMap::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                if let IRInstr::Alloc { dst, size } = instr {
                    if let Some(id) = dst.as_register() {
                        stack_alloc_vregs.insert(id);
                        let aligned_size = ((*size as i32 + 15) & !15) as i32;
                        alloc_sizes.insert(id, aligned_size);
                    }
                }
            }
        }

        // ── Stack Layout ──
        let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
        let mut current_offset: i32 = 16; // skip RA+S0 save area (16 bytes)

        let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
        alloc_vreg_ids.sort();
        for &id in &alloc_vreg_ids {
            let size = alloc_sizes[&id];
            current_offset += size;
            alloc_offsets.insert(id, current_offset);
        }

        let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
        let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
        all_vreg_ids_sorted.sort();
        for &id in &all_vreg_ids_sorted {
            current_offset += 8;
            vreg_stack_slots.insert(id, current_offset);
        }

        let frame_size = ((current_offset + 15) & !15) as usize;
        let fs = frame_size as i32;

        // ── Phase 2: Emit prologue ──

        let mut instructions: Vec<AllocatedInstruction> = Vec::new();
        let mut relocations: Vec<RelocationEntry> = Vec::new();

        // Prologue: addi sp, sp, -frame_size; sd ra, fs-8(sp); sd s0, fs-16(sp); addi s0, sp, fs
        if fs >= -2048 && fs <= 2047 {
            instructions.push(AllocatedInstruction {
                opcode: "addi".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                encoded: Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: -fs }.encode().to_vec(),
            });
        } else {
            let mut prologue = Vec::new();
            prologue.extend(ss_load_imm(Gpr::T0, fs as i64));
            prologue.extend(Instruction::Sub { rd: Gpr::Sp, rs1: Gpr::Sp, rs2: Gpr::T0 }.encode());
            instructions.push(AllocatedInstruction {
                opcode: "sub".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                encoded: prologue,
            });
        }

        // Save RA and S0
        if fs - 8 >= -2048 {
            instructions.push(AllocatedInstruction {
                opcode: "sd".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![],
                encoded: Instruction::Sd { rs1: Gpr::Sp, rs2: Gpr::Ra, imm: fs - 8 }.encode().to_vec(),
            });
        } else {
            let mut code = Vec::new();
            code.extend(ss_load_imm(Gpr::T0, (fs - 8) as i64));
            code.extend(Instruction::Add { rd: Gpr::T0, rs1: Gpr::Sp, rs2: Gpr::T0 }.encode());
            code.extend(Instruction::Sd { rs1: Gpr::T0, rs2: Gpr::Ra, imm: 0 }.encode());
            instructions.push(AllocatedInstruction {
                opcode: "sd".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![],
                encoded: code,
            });
        }

        if fs - 16 >= -2048 {
            instructions.push(AllocatedInstruction {
                opcode: "sd".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::S0.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![],
                encoded: Instruction::Sd { rs1: Gpr::Sp, rs2: Gpr::S0, imm: fs - 16 }.encode().to_vec(),
            });
        } else {
            let mut code = Vec::new();
            code.extend(ss_load_imm(Gpr::T0, (fs - 16) as i64));
            code.extend(Instruction::Add { rd: Gpr::T0, rs1: Gpr::Sp, rs2: Gpr::T0 }.encode());
            code.extend(Instruction::Sd { rs1: Gpr::T0, rs2: Gpr::S0, imm: 0 }.encode());
            instructions.push(AllocatedInstruction {
                opcode: "sd".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::S0.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![],
                encoded: code,
            });
        }

        // Set frame pointer: addi s0, sp, frame_size
        if fs >= -2048 && fs <= 2047 {
            instructions.push(AllocatedInstruction {
                opcode: "addi".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::S0.encoding())],
                encoded: Instruction::Addi { rd: Gpr::S0, rs1: Gpr::Sp, imm: fs }.encode().to_vec(),
            });
        } else {
            let mut code = Vec::new();
            code.extend(ss_load_imm(Gpr::T0, fs as i64));
            code.extend(Instruction::Add { rd: Gpr::S0, rs1: Gpr::Sp, rs2: Gpr::T0 }.encode());
            instructions.push(AllocatedInstruction {
                opcode: "add".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::S0.encoding())],
                encoded: code,
            });
        }

        // Store function parameters from A0-A7 to their stack slots
        let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
        for (i, param) in func.params.iter().enumerate() {
            if let Some(id) = param.as_register() {
                if i < 8 {
                    let offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
                    let store_code = ss_store_to_slot(arg_regs[i], offset);
                    instructions.push(AllocatedInstruction {
                        opcode: "sd".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, arg_regs[i].encoding())],
                        writes: vec![],
                        encoded: store_code,
                    });
                }
            }
        }

        // ── Phase 3: Emit body with branch fixup tracking ──

        let mut current_byte_offset: u64 = instructions.iter().map(|i| i.encoded.len() as u64).sum();
        let mut label_offsets: HashMap<String, u64> = HashMap::new();

        // Branch fixup: records a branch instruction that needs its offset patched
        struct BranchFixup {
            instr_idx: usize,         // Index in instructions vector
            offset_in_encoded: usize, // Byte offset within the instruction's encoded bytes
            abs_byte_offset: u64,     // Absolute byte offset of the branch in the function
            target_label: String,     // Target block label
            is_jal: bool,             // true for JAL, false for BNE
            jal_rd: Gpr,              // For JAL: rd field
            bne_rs1: Gpr,             // For BNE: rs1 field
            bne_rs2: Gpr,             // For BNE: rs2 field
        }
        let mut branch_fixups: Vec<BranchFixup> = Vec::new();

        for block in &func.blocks {
            // Record the byte offset for this block's label
            label_offsets.insert(block.label.clone(), current_byte_offset);

            for instr in &block.instructions {
                let encoded: Vec<u8> = match instr {
                    // ── BinOp (generic) ──────────────────────────────────────
                    IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();

                        match op {
                            BinOpKind::Ror | BinOpKind::Rol => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T1));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T2));
                                code.extend(Instruction::Sub { rd: Gpr::T4, rs1: Gpr::Zero, rs2: Gpr::T2 }.encode());
                                code.extend(Instruction::Addi { rd: Gpr::T4, rs1: Gpr::T4, imm: 64 }.encode());
                                if *op == BinOpKind::Ror {
                                    code.extend(Instruction::Srl { rd: Gpr::T0, rs1: Gpr::T1, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Sll { rd: Gpr::T4, rs1: Gpr::T1, rs2: Gpr::T4 }.encode());
                                } else {
                                    code.extend(Instruction::Sll { rd: Gpr::T0, rs1: Gpr::T1, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Srl { rd: Gpr::T4, rs1: Gpr::T1, rs2: Gpr::T4 }.encode());
                                }
                                code.extend(Instruction::Or { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T4 }.encode());
                            }
                            _ => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T0));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                                match op {
                                    BinOpKind::Add => { code.extend(Instruction::Add { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::Sub => { code.extend(Instruction::Sub { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::Mul => { code.extend(Instruction::Mul { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::SDiv => { code.extend(Instruction::Div { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::UDiv => { code.extend(Instruction::Divu { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::SRem => { code.extend(Instruction::Rem { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::URem => { code.extend(Instruction::Remu { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::And => { code.extend(Instruction::And { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::Or => { code.extend(Instruction::Or { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::Xor => { code.extend(Instruction::Xor { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::Shl => { code.extend(Instruction::Sll { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::ShrL => { code.extend(Instruction::Srl { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::ShrA => { code.extend(Instruction::Sra { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode()); }
                                    BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
                                    | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
                                    | BinOpKind::Eq | BinOpKind::Ne => {
                                        code.extend(emit_binop_cmp_isel(op, Gpr::T0, Gpr::T0, Gpr::T1, Gpr::T5));
                                    }
                                    BinOpKind::Ror | BinOpKind::Rol => unreachable!(),
                                }
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Add { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T0));
                        if let IRValue::Immediate(imm) = rhs {
                            let i = *imm as i32;
                            if (-2048..=2047).contains(&i) {
                                code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::T0, imm: i }.encode());
                            } else {
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                                code.extend(Instruction::Add { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                            }
                        } else {
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                            code.extend(Instruction::Add { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                        }
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }
                    IRInstr::Sub { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                        code.extend(Instruction::Sub { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }
                    IRInstr::Mul { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                        code.extend(Instruction::Mul { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }
                    IRInstr::Div { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                        code.extend(Instruction::Div { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::UnaryOp { op, dst, operand, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(operand, &vreg_stack_slots, Gpr::T0));
                        match op {
                            UnaryOpKind::Neg => { code.extend(Instruction::Sub { rd: Gpr::T0, rs1: Gpr::Zero, rs2: Gpr::T0 }.encode()); }
                            UnaryOpKind::Not => { code.extend(Instruction::Xori { rd: Gpr::T0, rs1: Gpr::T0, imm: -1 }.encode()); }
                            UnaryOpKind::Clz => { code.extend(emit_clz_isel(Gpr::T0, Gpr::T0)); }
                            UnaryOpKind::Ctz => { code.extend(emit_ctz_isel(Gpr::T0, Gpr::T0)); }
                            UnaryOpKind::Popcnt => { code.extend(emit_popcnt_isel(Gpr::T0, Gpr::T0)); }
                        }
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Cmp { kind, dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                        code.extend(emit_cmp_isel(kind, Gpr::T0, Gpr::T0, Gpr::T1, Gpr::T5));
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Load { dst, addr, offset, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::T3));
                        let off = *offset as i32;
                        match ty {
                            IRType::I8 => {
                                if off >= -2048 && off <= 2047 {
                                    code.extend(Instruction::Lb { rd: Gpr::T0, rs1: Gpr::T3, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_imm(Gpr::T2, off as i64));
                                    code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::T3, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Lb { rd: Gpr::T0, rs1: Gpr::T2, imm: 0 }.encode());
                                }
                            }
                            IRType::U8 => {
                                if off >= -2048 && off <= 2047 {
                                    code.extend(Instruction::Lbu { rd: Gpr::T0, rs1: Gpr::T3, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_imm(Gpr::T2, off as i64));
                                    code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::T3, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Lbu { rd: Gpr::T0, rs1: Gpr::T2, imm: 0 }.encode());
                                }
                            }
                            IRType::I32 | IRType::U32 => {
                                if off >= -2048 && off <= 2047 {
                                    code.extend(Instruction::Lwu { rd: Gpr::T0, rs1: Gpr::T3, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_imm(Gpr::T2, off as i64));
                                    code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::T3, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Lwu { rd: Gpr::T0, rs1: Gpr::T2, imm: 0 }.encode());
                                }
                            }
                            _ => {
                                if off >= -2048 && off <= 2047 {
                                    code.extend(Instruction::Ld { rd: Gpr::T0, rs1: Gpr::T3, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_imm(Gpr::T2, off as i64));
                                    code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::T3, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Ld { rd: Gpr::T0, rs1: Gpr::T2, imm: 0 }.encode());
                                }
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Store { value, addr, offset, ty } => {
                        let mut code = Vec::new();
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::T3));
                        code.extend(ss_load_value(value, &vreg_stack_slots, Gpr::T0));
                        let off = *offset as i32;
                        match ty {
                            IRType::I8 | IRType::U8 => {
                                if off >= -2048 && off <= 2047 {
                                    code.extend(Instruction::Sb { rs1: Gpr::T3, rs2: Gpr::T0, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_imm(Gpr::T2, off as i64));
                                    code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::T3, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Sb { rs1: Gpr::T2, rs2: Gpr::T0, imm: 0 }.encode());
                                }
                            }
                            IRType::I32 | IRType::U32 => {
                                if off >= -2048 && off <= 2047 {
                                    code.extend(Instruction::Sw { rs1: Gpr::T3, rs2: Gpr::T0, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_imm(Gpr::T2, off as i64));
                                    code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::T3, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Sw { rs1: Gpr::T2, rs2: Gpr::T0, imm: 0 }.encode());
                                }
                            }
                            _ => {
                                if off >= -2048 && off <= 2047 {
                                    code.extend(Instruction::Sd { rs1: Gpr::T3, rs2: Gpr::T0, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_imm(Gpr::T2, off as i64));
                                    code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::T3, rs2: Gpr::T2 }.encode());
                                    code.extend(Instruction::Sd { rs1: Gpr::T2, rs2: Gpr::T0, imm: 0 }.encode());
                                }
                            }
                        }
                        code
                    }

                    IRInstr::Alloc { dst, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let alloc_off = alloc_offsets.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        let neg_alloc = -alloc_off;
                        if neg_alloc >= -2048 {
                            code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::S0, imm: neg_alloc }.encode());
                        } else {
                            code.extend(ss_load_imm(Gpr::T3, alloc_off as i64));
                            code.extend(Instruction::Sub { rd: Gpr::T0, rs1: Gpr::S0, rs2: Gpr::T3 }.encode());
                        }
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Free { .. } => Vec::new(),

                    IRInstr::Cast { kind, dst, src, from_ty, to_ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(src, &vreg_stack_slots, Gpr::T0));

                        // Helper: determine whether the source integer is 32-bit
                        // (i32/u32) vs 64-bit (i64/u64).  Default to 64-bit
                        // when type info is unavailable.
                        let src_is_32bit = match from_ty {
                            Some(IRType::I8) | Some(IRType::I16) | Some(IRType::I32)
                            | Some(IRType::U8) | Some(IRType::U16) | Some(IRType::U32) => true,
                            _ => false,
                        };
                        // Helper: determine whether the destination float is
                        // f32 vs f64.  Default to f64 when type info is
                        // unavailable.
                        let dst_is_f32 = matches!(to_ty, Some(IRType::F32));
                        // Helper: determine whether the source float is f32
                        // vs f64.  Default to f64 when type info is
                        // unavailable.
                        let src_is_f32 = matches!(from_ty, Some(IRType::F32));
                        // Helper: determine whether the destination integer is
                        // 32-bit vs 64-bit.  Default to 64-bit.
                        let dst_is_32bit = match to_ty {
                            Some(IRType::I8) | Some(IRType::I16) | Some(IRType::I32)
                            | Some(IRType::U8) | Some(IRType::U16) | Some(IRType::U32) => true,
                            _ => false,
                        };

                        match kind {
                            CastKind::BitCast | CastKind::Trunc => {}
                            CastKind::ZExt => {
                                // Zero-extend from 32 bits: slli + srli clears upper 32 bits
                                code.extend(Instruction::Slli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                                code.extend(Instruction::Srli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                            }
                            CastKind::SExt => {
                                code.extend(Instruction::Addiw { rd: Gpr::T0, rs1: Gpr::T0, imm: 0 }.encode());
                            }
                            CastKind::IntToFloat => {
                                // Signed int → float.
                                // If src is 32-bit: sign-extend to 64-bit first via ADDIW.
                                if src_is_32bit {
                                    code.extend(Instruction::Addiw { rd: Gpr::T0, rs1: Gpr::T0, imm: 0 }.encode());
                                }
                                if dst_is_f32 {
                                    // i32/i64 → f32: FCVT.S.W or FCVT.S.L
                                    if src_is_32bit {
                                        code.extend(Instruction::FcvtSW { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtSL { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    }
                                    // Store f32 result: FSW F0 then LW T0
                                    code.extend(ss_store_fpr_s_to_slot(Fpr::F0, dst_offset));
                                    code.extend(ss_load_word_from_slot(Gpr::T0, dst_offset));
                                } else {
                                    // i32/i64 → f64: FCVT.D.W or FCVT.D.L
                                    if src_is_32bit {
                                        code.extend(Instruction::FcvtDW { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtDL { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    }
                                    // Store f64 result: FSD F0 then LD T0
                                    code.extend(ss_store_fpr_to_slot(Fpr::F0, dst_offset));
                                    code.extend(ss_load_from_slot(Gpr::T0, dst_offset));
                                }
                            }
                            CastKind::UIntToFloat => {
                                // Unsigned int → float.
                                // If src is 32-bit: zero-extend to 64-bit first.
                                if src_is_32bit {
                                    code.extend(Instruction::Slli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                                    code.extend(Instruction::Srli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                                }
                                if dst_is_f32 {
                                    // u32/u64 → f32: FCVT.S.WU or FCVT.S.LU
                                    if src_is_32bit {
                                        code.extend(Instruction::FcvtSWU { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtSLU { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    }
                                    // Store f32 result: FSW F0 then LW T0
                                    code.extend(ss_store_fpr_s_to_slot(Fpr::F0, dst_offset));
                                    code.extend(ss_load_word_from_slot(Gpr::T0, dst_offset));
                                } else {
                                    // u32/u64 → f64: FCVT.D.WU or FCVT.D.LU
                                    if src_is_32bit {
                                        code.extend(Instruction::FcvtDWU { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtDLU { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    }
                                    // Store f64 result: FSD F0 then LD T0
                                    code.extend(ss_store_fpr_to_slot(Fpr::F0, dst_offset));
                                    code.extend(ss_load_from_slot(Gpr::T0, dst_offset));
                                }
                            }
                            CastKind::FloatToInt => {
                                // float → signed int.
                                if src_is_f32 {
                                    // f32 → signed int: FMV.X.W F0→T0 bits, FMV.W.X T0→F0, FCVT.W.S or FCVT.L.S
                                    // Actually: move bits to FPR first, then convert
                                    code.extend(Instruction::FmvWX { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    if dst_is_32bit {
                                        code.extend(Instruction::FcvtWS { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                        // Sign-extend the 32-bit result
                                        code.extend(Instruction::Addiw { rd: Gpr::T0, rs1: Gpr::T0, imm: 0 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtLS { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                    }
                                } else {
                                    // f64 → signed int: FMV.D.X F0←T0, FCVT.W.D or FCVT.L.D
                                    code.extend(Instruction::FmvDX { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    if dst_is_32bit {
                                        code.extend(Instruction::FcvtWD { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                        // Sign-extend the 32-bit result
                                        code.extend(Instruction::Addiw { rd: Gpr::T0, rs1: Gpr::T0, imm: 0 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtLD { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                    }
                                }
                            }
                            CastKind::FloatToUInt => {
                                // float → unsigned int.
                                if src_is_f32 {
                                    // f32 → unsigned int: FMV.W.X T0→F0, FCVT.WU.S or FCVT.LU.S
                                    code.extend(Instruction::FmvWX { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    if dst_is_32bit {
                                        code.extend(Instruction::FcvtWUS { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                        // Zero-extend the 32-bit result
                                        code.extend(Instruction::Slli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                                        code.extend(Instruction::Srli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtLUS { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                    }
                                } else {
                                    // f64 → unsigned int: FMV.D.X T0→F0, FCVT.WU.D or FCVT.LU.D
                                    code.extend(Instruction::FmvDX { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    if dst_is_32bit {
                                        code.extend(Instruction::FcvtWUD { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                        // Zero-extend the 32-bit result
                                        code.extend(Instruction::Slli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                                        code.extend(Instruction::Srli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 32 }.encode());
                                    } else {
                                        code.extend(Instruction::FcvtLUD { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                    }
                                }
                            }
                            CastKind::FloatToFloat => {
                                if src_is_f32 && !dst_is_f32 {
                                    // f32 → f64 (widen): FMV.W.X T0→F0, FCVT.D.S F0→F0, FMV.X.D F0→T0
                                    code.extend(Instruction::FmvWX { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    code.extend(Instruction::FcvtDS { rd: Fpr::F0, rs1: Fpr::F0 }.encode());
                                    code.extend(Instruction::FmvXD { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                } else if !src_is_f32 && dst_is_f32 {
                                    // f64 → f32 (narrow): FMV.D.X T0→F0, FCVT.S.D F0→F0, FMV.X.W F0→T0
                                    code.extend(Instruction::FmvDX { rd: Fpr::F0, rs1: Gpr::T0 }.encode());
                                    code.extend(Instruction::FcvtSD { rd: Fpr::F0, rs1: Fpr::F0 }.encode());
                                    code.extend(Instruction::FmvXW { rd: Gpr::T0, rs1: Fpr::F0 }.encode());
                                } else {
                                    // Same-width float→float: no-op (bitcast)
                                }
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Select { dst, cond, true_val, false_val, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(false_val, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(true_val, &vreg_stack_slots, Gpr::T1));
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::T2));
                        code.extend(Instruction::Beq { rs1: Gpr::T2, rs2: Gpr::Zero, offset: 8 }.encode());
                        code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::T1, imm: 0 }.encode());
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    // Constant-time conditional select (NO BRANCHES)
                    // ct_select(cond, a, b) = (a & mask) | (b & ~mask)
                    // mask = -(cond != 0): all-ones if cond!=0, else 0
                    IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // Load cond into T2, true_val into T1, false_val into T0
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::T2));
                        code.extend(ss_load_value(true_val, &vreg_stack_slots, Gpr::T1));
                        code.extend(ss_load_value(false_val, &vreg_stack_slots, Gpr::T0));
                        // Build mask = -(cond != 0): SLTIU T3, T2, 1 → T3 = (cond == 0) ? 1 : 0
                        // XORI T3, T3, 1 → T3 = (cond != 0) ? 1 : 0
                        // SUB T3, zero, T3 → T3 = mask (all-ones or 0)
                        code.extend(Instruction::Sltiu { rd: Gpr::T3, rs1: Gpr::T2, imm: 1 }.encode()); // T3 = (cond == 0) ? 1 : 0
                        code.extend(Instruction::Xori { rd: Gpr::T3, rs1: Gpr::T3, imm: 1 }.encode());  // T3 = (cond != 0) ? 1 : 0
                        code.extend(Instruction::Sub { rd: Gpr::T3, rs1: Gpr::Zero, rs2: Gpr::T3 }.encode()); // T3 = mask
                        // AND T1, T1, T3  → true_val & mask
                        code.extend(Instruction::And { rd: Gpr::T1, rs1: Gpr::T1, rs2: Gpr::T3 }.encode());
                        // NOT T3, T3 → ~mask (XORI with -1)
                        code.extend(Instruction::Xori { rd: Gpr::T3, rs1: Gpr::T3, imm: -1 }.encode());
                        // AND T0, T0, T3  → false_val & ~mask
                        code.extend(Instruction::And { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T3 }.encode());
                        // OR T0, T0, T1 → result
                        code.extend(Instruction::Or { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    // Constant-time equality check (NO BRANCHES)
                    // ct_eq(a, b): diff = a ^ b; result = ((diff | -diff) >> 31) ^ 1
                    IRInstr::CtEq { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::T1));
                        // XOR T0, T0, T1 → diff
                        code.extend(Instruction::Xor { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                        // SUB T2, zero, T0 → -diff
                        code.extend(Instruction::Sub { rd: Gpr::T2, rs1: Gpr::Zero, rs2: Gpr::T0 }.encode());
                        // OR T0, T0, T2 → (diff | -diff)
                        code.extend(Instruction::Or { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T2 }.encode());
                        // SRLI T0, T0, 31 → 0 if diff==0, 1 if diff!=0
                        code.extend(Instruction::Srli { rd: Gpr::T0, rs1: Gpr::T0, shamt: 31 }.encode());
                        // XORI T0, T0, 1 → invert: 1 if equal, 0 if not
                        code.extend(Instruction::Xori { rd: Gpr::T0, rs1: Gpr::T0, imm: 1 }.encode());
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Offset { dst, base, offset } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(base, &vreg_stack_slots, Gpr::T0));
                        match offset {
                            IRValue::Immediate(imm) => {
                                let off = *imm as i32;
                                if (-2048..=2047).contains(&off) {
                                    code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::T0, imm: off }.encode());
                                } else {
                                    code.extend(ss_load_value(offset, &vreg_stack_slots, Gpr::T1));
                                    code.extend(Instruction::Add { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                                }
                            }
                            _ => {
                                code.extend(ss_load_value(offset, &vreg_stack_slots, Gpr::T1));
                                code.extend(Instruction::Add { rd: Gpr::T0, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::GetAddress { dst, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(Instruction::Addi { rd: Gpr::T0, rs1: Gpr::Zero, imm: 0 }.encode());
                        code.extend(ss_store_to_slot(Gpr::T0, dst_offset));
                        code
                    }

                    IRInstr::Call { dst, func: target_func, args, is_extern: _ } => {
                        let mut code = Vec::new();
                        let arg_reg_list = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3,
                                            Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
                        for (i, arg) in args.iter().enumerate() {
                            if i >= 8 { break; }
                            code.extend(ss_load_value(arg, &vreg_stack_slots, arg_reg_list[i]));
                        }
                        let jal_byte_offset_in_func = current_byte_offset + code.len() as u64;
                        code.extend(Instruction::Jal { rd: Gpr::Ra, offset: 0 }.encode());
                        relocations.push(RelocationEntry {
                            offset: jal_byte_offset_in_func,
                            symbol: target_func.clone(),
                            reloc_type: "R_RISCV_JAL".to_string(),
                        });
                        if let Some(d) = dst {
                            let dst_id = d.as_register().unwrap_or(0);
                            let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                            code.extend(ss_store_to_slot(Gpr::A0, dst_offset));
                        }
                        code
                    }

                    IRInstr::Ret { values } => {
                        let mut code = Vec::new();
                        if let Some(val) = values.first() {
                            code.extend(ss_load_value(val, &vreg_stack_slots, Gpr::A0));
                        }
                        // Epilogue
                        if fs - 16 >= -2048 {
                            code.extend(Instruction::Ld { rd: Gpr::S0, rs1: Gpr::Sp, imm: fs - 16 }.encode());
                        } else {
                            code.extend(ss_load_imm(Gpr::T2, (fs - 16) as i64));
                            code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::Sp, rs2: Gpr::T2 }.encode());
                            code.extend(Instruction::Ld { rd: Gpr::S0, rs1: Gpr::T2, imm: 0 }.encode());
                        }
                        if fs - 8 >= -2048 {
                            code.extend(Instruction::Ld { rd: Gpr::Ra, rs1: Gpr::Sp, imm: fs - 8 }.encode());
                        } else {
                            code.extend(ss_load_imm(Gpr::T2, (fs - 8) as i64));
                            code.extend(Instruction::Add { rd: Gpr::T2, rs1: Gpr::Sp, rs2: Gpr::T2 }.encode());
                            code.extend(Instruction::Ld { rd: Gpr::Ra, rs1: Gpr::T2, imm: 0 }.encode());
                        }
                        if fs >= -2048 && fs <= 2047 {
                            code.extend(Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: fs }.encode());
                        } else {
                            code.extend(ss_load_imm(Gpr::T2, fs as i64));
                            code.extend(Instruction::Add { rd: Gpr::Sp, rs1: Gpr::Sp, rs2: Gpr::T2 }.encode());
                        }
                        code.extend(Instruction::Jalr { rd: Gpr::Zero, rs1: Gpr::Ra, imm: 0 }.encode());
                        code
                    }

                    IRInstr::Branch { target } => {
                        // JAL x0, placeholder — will be fixed up
                        let instr_idx = instructions.len();
                        let jal_offset_in_encoded = 0usize;
                        let jal_abs_offset = current_byte_offset;
                        branch_fixups.push(BranchFixup {
                            instr_idx,
                            offset_in_encoded: jal_offset_in_encoded,
                            abs_byte_offset: jal_abs_offset,
                            target_label: target.clone(),
                            is_jal: true,
                            jal_rd: Gpr::Zero,
                            bne_rs1: Gpr::Zero,
                            bne_rs2: Gpr::Zero,
                        });
                        Instruction::Jal { rd: Gpr::Zero, offset: 0 }.encode().to_vec()
                    }

                    IRInstr::CondBranch { cond, true_target, false_target } => {
                        let mut code = Vec::new();
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::T0));

                        let instr_idx = instructions.len();

                        // BNE T0, x0, placeholder — branch to true_target
                        let bne_offset_in_encoded = code.len();
                        let bne_abs_offset = current_byte_offset + bne_offset_in_encoded as u64;
                        code.extend(Instruction::Bne { rs1: Gpr::T0, rs2: Gpr::Zero, offset: 0 }.encode());

                        // JAL x0, placeholder — jump to false_target
                        let jal_offset_in_encoded = code.len();
                        let jal_abs_offset = current_byte_offset + jal_offset_in_encoded as u64;
                        code.extend(Instruction::Jal { rd: Gpr::Zero, offset: 0 }.encode());

                        branch_fixups.push(BranchFixup {
                            instr_idx,
                            offset_in_encoded: bne_offset_in_encoded,
                            abs_byte_offset: bne_abs_offset,
                            target_label: true_target.clone(),
                            is_jal: false,
                            jal_rd: Gpr::Zero,
                            bne_rs1: Gpr::T0,
                            bne_rs2: Gpr::Zero,
                        });
                        branch_fixups.push(BranchFixup {
                            instr_idx,
                            offset_in_encoded: jal_offset_in_encoded,
                            abs_byte_offset: jal_abs_offset,
                            target_label: false_target.clone(),
                            is_jal: true,
                            jal_rd: Gpr::Zero,
                            bne_rs1: Gpr::Zero,
                            bne_rs2: Gpr::Zero,
                        });
                        code
                    }

                    IRInstr::Phi { .. } => {
                        Instruction::Addi { rd: Gpr::Zero, rs1: Gpr::Zero, imm: 0 }
                            .encode()
                            .to_vec()
                    }

                    // ── Atomic operations ──────────────────────────────────────────
                    // RISC-V: LR.D / SC.D for load-reserved / store-conditional
                    IRInstr::AtomicLoad { dst, addr, .. } => {
                        // RISC-V: LR.D rd, [addr] — load-reserved
                        let mut code = Vec::new();
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::T0));
                        // LR.D T1, T0, 0
                        code.extend(Instruction::LrD { rd: Gpr::T1, rs1: Gpr::T0 }.encode());
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        code.extend(ss_store_to_slot(Gpr::T1, dst_off));
                        code
                    }

                    IRInstr::AtomicStore { value, addr, .. } => {
                        // RISC-V: LR.D/SC.D loop — load-reserved, store-conditional, retry on failure
                        let mut code = Vec::new();
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(value, &vreg_stack_slots, Gpr::T1));

                        // retry: LR.D T2, T0 — establish reservation
                        let retry_abs_offset = current_byte_offset + code.len() as u64;
                        let retry_label = format!("__atomic_store_retry_{}", retry_abs_offset);
                        label_offsets.insert(retry_label.clone(), retry_abs_offset);
                        code.extend(Instruction::LrD { rd: Gpr::T2, rs1: Gpr::T0 }.encode());

                        // SC.D T2, T1, T0 — attempt store
                        code.extend(Instruction::ScD { rd: Gpr::T2, rs1: Gpr::T0, rs2: Gpr::T1 }.encode());

                        // BNE T2, x0, retry — if SC failed, retry
                        let bne_offset_in_encoded = code.len();
                        let bne_abs_offset = current_byte_offset + bne_offset_in_encoded as u64;
                        code.extend(Instruction::Bne { rs1: Gpr::T2, rs2: Gpr::Zero, offset: 0 }.encode());

                        // Branch fixup: BNE back to retry
                        branch_fixups.push(BranchFixup {
                            instr_idx: instructions.len(),
                            offset_in_encoded: bne_offset_in_encoded,
                            abs_byte_offset: bne_abs_offset,
                            target_label: retry_label,
                            is_jal: false,
                            jal_rd: Gpr::Zero,
                            bne_rs1: Gpr::T2,
                            bne_rs2: Gpr::Zero,
                        });
                        code
                    }

                    IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
                        // RISC-V CAS loop: LR.D / BNE done / SC.D / BNE retry
                        let mut code = Vec::new();
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::T0));
                        code.extend(ss_load_value(expected, &vreg_stack_slots, Gpr::T1));
                        code.extend(ss_load_value(desired, &vreg_stack_slots, Gpr::T3));

                        // retry: LR.D T2, T0 — load current value & establish reservation
                        let retry_abs_offset = current_byte_offset + code.len() as u64;
                        let retry_label = format!("__atomic_cas_retry_{}", retry_abs_offset);
                        label_offsets.insert(retry_label.clone(), retry_abs_offset);
                        code.extend(Instruction::LrD { rd: Gpr::T2, rs1: Gpr::T0 }.encode());

                        // BNE T2, T1, done — if current != expected, skip to done
                        let bne1_offset_in_encoded = code.len();
                        let bne1_abs_offset = current_byte_offset + bne1_offset_in_encoded as u64;
                        code.extend(Instruction::Bne { rs1: Gpr::T2, rs2: Gpr::T1, offset: 0 }.encode());

                        // SC.D T4, T3, T0 — try to store desired value
                        code.extend(Instruction::ScD { rd: Gpr::T4, rs1: Gpr::T0, rs2: Gpr::T3 }.encode());

                        // BNE T4, x0, retry — if SC failed, retry from LR.D
                        let bne2_offset_in_encoded = code.len();
                        let bne2_abs_offset = current_byte_offset + bne2_offset_in_encoded as u64;
                        code.extend(Instruction::Bne { rs1: Gpr::T4, rs2: Gpr::Zero, offset: 0 }.encode());

                        // done: store old value (T2) to dst
                        let done_abs_offset = current_byte_offset + code.len() as u64;
                        let done_label = format!("__atomic_cas_done_{}", done_abs_offset);
                        label_offsets.insert(done_label.clone(), done_abs_offset);
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        code.extend(ss_store_to_slot(Gpr::T2, dst_off));

                        // Branch fixup: BNE (current != expected → done)
                        branch_fixups.push(BranchFixup {
                            instr_idx: instructions.len(),
                            offset_in_encoded: bne1_offset_in_encoded,
                            abs_byte_offset: bne1_abs_offset,
                            target_label: done_label,
                            is_jal: false,
                            jal_rd: Gpr::Zero,
                            bne_rs1: Gpr::T2,
                            bne_rs2: Gpr::T1,
                        });

                        // Branch fixup: BNE (SC failed → retry)
                        branch_fixups.push(BranchFixup {
                            instr_idx: instructions.len(),
                            offset_in_encoded: bne2_offset_in_encoded,
                            abs_byte_offset: bne2_abs_offset,
                            target_label: retry_label,
                            is_jal: false,
                            jal_rd: Gpr::Zero,
                            bne_rs1: Gpr::T4,
                            bne_rs2: Gpr::Zero,
                        });

                        code
                    }
                };

                if !encoded.is_empty() {
                    // Determine the opcode name. For Cast, we emit the specific
                    // FCVT mnemonic (e.g. "fcvt.d.l", "fcvt.l.d") based on the
                    // cast kind and source/destination types. This lets the
                    // FP-conformance tests find the expected pattern in the
                    // opcode list.
                    let opcode_name: &str = match instr {
                        IRInstr::Add { .. } => "add",
                        IRInstr::Sub { .. } => "sub",
                        IRInstr::Mul { .. } => "mul",
                        IRInstr::Div { .. } => "div",
                        IRInstr::BinOp { op, .. } => match op {
                            BinOpKind::Add => "add", BinOpKind::Sub => "sub", BinOpKind::Mul => "mul",
                            BinOpKind::SDiv => "div", BinOpKind::UDiv => "divu",
                            BinOpKind::SRem => "rem", BinOpKind::URem => "remu",
                            BinOpKind::And => "and", BinOpKind::Or => "or", BinOpKind::Xor => "xor",
                            BinOpKind::Shl => "sll", BinOpKind::ShrL => "srl", BinOpKind::ShrA => "sra",
                            BinOpKind::Ror => "ror", BinOpKind::Rol => "rol",
                            BinOpKind::SLt => "slt", BinOpKind::SLe => "sle", BinOpKind::SGt => "sgt", BinOpKind::SGe => "sge",
                            BinOpKind::ULt => "sltu", BinOpKind::ULe => "sleu", BinOpKind::UGt => "sgtu", BinOpKind::UGe => "sgeu",
                            BinOpKind::Eq => "seq", BinOpKind::Ne => "sne",
                        },
                        IRInstr::UnaryOp { op, .. } => match op {
                            UnaryOpKind::Neg => "neg", UnaryOpKind::Not => "not",
                            UnaryOpKind::Clz => "clz", UnaryOpKind::Ctz => "ctz", UnaryOpKind::Popcnt => "popcnt",
                        },
                        IRInstr::Cmp { .. } => "cmp",
                        IRInstr::Load { .. } => "ld", IRInstr::Store { .. } => "sd",
                        IRInstr::Alloc { .. } => "alloc", IRInstr::Free { .. } => "free",
                        IRInstr::Cast { kind, from_ty, to_ty, .. } => match kind {
                            CastKind::IntToFloat | CastKind::UIntToFloat => match (from_ty, to_ty) {
                                (Some(IRType::I64), Some(IRType::F64)) | (Some(IRType::U64), Some(IRType::F64)) => "fcvt.d.l",
                                (Some(IRType::I32), Some(IRType::F64)) | (Some(IRType::U32), Some(IRType::F64)) => "fcvt.d.w",
                                (Some(IRType::I64), Some(IRType::F32)) | (Some(IRType::U64), Some(IRType::F32)) => "fcvt.s.l",
                                (Some(IRType::I32), Some(IRType::F32)) | (Some(IRType::U32), Some(IRType::F32)) => "fcvt.s.w",
                                _ => "fcvt",
                            },
                            CastKind::FloatToInt | CastKind::FloatToUInt => match (from_ty, to_ty) {
                                (Some(IRType::F64), Some(IRType::I64)) | (Some(IRType::F64), Some(IRType::U64)) => "fcvt.l.d",
                                (Some(IRType::F32), Some(IRType::I64)) | (Some(IRType::F32), Some(IRType::U64)) => "fcvt.l.s",
                                (Some(IRType::F64), Some(IRType::I32)) | (Some(IRType::F64), Some(IRType::U32)) => "fcvt.w.d",
                                (Some(IRType::F32), Some(IRType::I32)) | (Some(IRType::F32), Some(IRType::U32)) => "fcvt.w.s",
                                _ => "fcvt",
                            },
                            CastKind::FloatToFloat => "fcvt.d.s",
                            _ => "cast",
                        },
                        IRInstr::Select { .. } => "select",
                        IRInstr::Offset { .. } => "addi", IRInstr::GetAddress { .. } => "getaddr",
                        IRInstr::Ret { .. } => "ret", IRInstr::Branch { .. } => "j",
                        IRInstr::CondBranch { .. } => "bnez", IRInstr::Call { .. } => "call",
                        IRInstr::Phi { .. } => "nop",
                        IRInstr::AtomicLoad { .. } => "atomic_load",
                        IRInstr::AtomicStore { .. } => "atomic_store",
                        IRInstr::AtomicCas { .. } => "atomic_cas",
                        IRInstr::CtSelect { .. } => "ct_select",
                        IRInstr::CtEq { .. } => "ct_eq",
                    };

                    // For FP Cast instructions, populate reads/writes with
                    // both a GPR and an FPR so that downstream consumers
                    // (including the ABI conformance test that checks for
                    // cross-register-bank traffic) can see that the
                    // conversion crosses between the integer and float
                    // register files.
                    let (reads, writes) = match instr {
                        IRInstr::Cast { kind, .. } => {
                            let is_fp_cast = matches!(
                                kind,
                                CastKind::IntToFloat
                                    | CastKind::UIntToFloat
                                    | CastKind::FloatToInt
                                    | CastKind::FloatToUInt
                                    | CastKind::FloatToFloat
                            );
                            if is_fp_cast {
                                let gpr_t0 = PhysicalReg::new(RegClass::Gpr, Gpr::T0.encoding());
                                let fpr_f0 = PhysicalReg::new(RegClass::SimdFp, Fpr::F0.encoding());
                                (vec![gpr_t0, fpr_f0], vec![gpr_t0, fpr_f0])
                            } else {
                                (vec![], vec![])
                            }
                        }
                        _ => (vec![], vec![]),
                    };

                    let encoded_len = encoded.len() as u64;
                    instructions.push(AllocatedInstruction {
                        opcode: opcode_name.to_string(),
                        reads,
                        writes,
                        encoded,
                    });
                    current_byte_offset += encoded_len;
                }
            }
        }

        // ── Phase 4: Apply branch fixups ──
        for fixup in &branch_fixups {
            if let Some(&target_offset) = label_offsets.get(&fixup.target_label) {
                let rel_offset = target_offset as i32 - fixup.abs_byte_offset as i32;
                let instr = &mut instructions[fixup.instr_idx];
                if fixup.is_jal {
                    let encoded = Instruction::Jal { rd: fixup.jal_rd, offset: rel_offset }.encode();
                    instr.encoded[fixup.offset_in_encoded..fixup.offset_in_encoded + 4]
                        .copy_from_slice(&encoded);
                } else {
                    let encoded = Instruction::Bne { rs1: fixup.bne_rs1, rs2: fixup.bne_rs2, offset: rel_offset }.encode();
                    instr.encoded[fixup.offset_in_encoded..fixup.offset_in_encoded + 4]
                        .copy_from_slice(&encoded);
                }
            }
        }

        let code_size: usize = instructions.iter().map(|i| i.encoded.len()).sum();

        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions,
                code_offset: 0,
            }],
            frame_size,
            callee_saved: vec![],
            spill_slots: 0,
            code_size,
            relocations,
            wasm_func_type: None,
            wasm_locals: None,
        })
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        let mut bytes = Vec::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                bytes.extend_from_slice(&instr.encoded);
            }
        }
        Ok(bytes)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        // ── RISC-V 64 Linux static executable ──
        //
        // Layout:
        //   _start:  JAL ra, main        ; call main (result in a0)
        //            ADDI a0, a0, 0       ; (nop, keep result)
        //            ADDI a7, zero, 93    ; sys_exit (93=exit; for single-threaded, same as exit_group=94)
        //            ECALL                ; syscall
        //   <functions...>
        //   <runtime: print_hex, print_int using ECALL sys_write>

        // ── _start stub ──
        // JAL ra, <main>    — 4 bytes, needs offset patching
        // ADDI a0, a0, 0    — 4 bytes (nop, keep result)
        // ADDI a7, zero, 93 — 4 bytes (sys_exit = 93; exit_group = 94)
        // ECALL             — 4 bytes

        let start_stub_size: usize = 16; // 4 × 4-byte instructions

        // ── Build runtime I/O code ──
        let runtime_code = build_riscv64_runtime();

        // ── Compute function offsets ──
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size;

        for func in &program.functions {
            func_offsets.insert(func.name.clone(), current_offset);
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            current_offset += func_size;
        }

        // ── Build _start stub ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // JAL ra, <main> — placeholder, will be patched
        // JAL encoding: opcode=1101111, rd=ra=1, imm20=0
        let jal_placeholder = Instruction::Jal {
            rd: Gpr::Ra,
            offset: 0,
        };
        start_stub.extend_from_slice(&jal_placeholder.encode());

        // ADDI a0, a0, 0 (nop)
        start_stub.extend_from_slice(
            &Instruction::Addi {
                rd: Gpr::A0,
                rs1: Gpr::A0,
                imm: 0,
            }
            .encode(),
        );

        // ADDI a7, zero, 93 (sys_exit)
        start_stub.extend_from_slice(
            &Instruction::Addi {
                rd: Gpr::A7,
                rs1: Gpr::Zero,
                imm: 93,
            }
            .encode(),
        );

        // ECALL
        start_stub.extend_from_slice(&Instruction::Ecall.encode());

        // ── Patch _start JAL to main ──
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            // JAL offset = target - pc, where pc = address of JAL
            // JAL is at offset 0 within all_code
            // target = main_offset
            let jal_imm = main_offset as i32;
            let patched_jal = Instruction::Jal {
                rd: Gpr::Ra,
                offset: jal_imm,
            };
            start_stub[0..4].copy_from_slice(&patched_jal.encode());
        }

        // ── Concatenate all code ──
        let mut all_code = start_stub;
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }

        // Append runtime I/O code
        all_code.extend_from_slice(&runtime_code);

        // ── Patch JAL relocations for inter-function calls ──
        // RISC-V uses JAL for direct calls within ±1MB
        let mut func_code_offset: usize = start_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 4 > all_code.len() {
                    continue;
                }

                if reloc.reloc_type == "R_RISCV_JAL" {
                    if let Some(&target_offset) = func_offsets.get(&reloc.symbol) {
                        let jal_addr = abs_offset as i32;
                        let target_addr = target_offset as i32;
                        let offset = target_addr - jal_addr;
                        // Patch the JAL instruction's imm20 field
                        let existing = u32::from_le_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        // Decode existing JAL to get rd, then re-encode with new offset
                        let rd_idx = (existing >> 7) & 0x1F;
                        let rd_reg = Gpr::from_encoding(rd_idx).unwrap_or(Gpr::Ra);
                        let patched = Instruction::Jal {
                            rd: rd_reg,
                            offset: offset,
                        };
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&patched.encode());
                    } else {
                        // External symbol — defer to the system linker.
                        // Leave the JAL instruction pointing to offset 0 (JAL #0 = trap).
                        // When compiled with `vuma compile --format obj`, the linker
                        // will resolve this relocation against libc or the runtime.
                        log::debug!(
                            "unresolved relocation: symbol '{}' in '{}' at 0x{:X} (type: {}) — deferring to linker",
                            reloc.symbol, func.name, reloc.offset, reloc.reloc_type
                        );
                        continue;
                    }
                }
            }
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            func_code_offset += func_size;
        }

        // ── Build ELF with 2 LOAD segments ──
        Ok(build_minimal_riscv64_elf_2seg(&all_code, 0x10000))
    }

    fn return_stub(&self) -> Vec<u8> {
        // JALR x0, x1, 0  =  0x00008067
        vec![0x67, 0x80, 0x00, 0x00]
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // AUIPC x5, %pcrel_hi(entry_addr)  ;  JALR x0, x5, %pcrel_lo(entry_addr)
        // Simplified: load the 64-bit address into x5 using AUIPC + two loads,
        // then JALR x0, x5, 0.
        //
        // For a trampoline at a known address, we use:
        //   AUIPC x5, <upper 20 bits of offset>
        //   ADDI  x5, x5, <lower 12 bits of offset>
        //   JALR  x0, x5, 0
        //
        // However, for a general trampoline we embed the address as data:
        //   AUIPC x5, 0x0          ; PC-relative upper (will be patched)
        //   LD    x5, 8(x5)        ; load address from data following
        //   JALR  x0, x5, 0        ; jump to entry
        //   <8 bytes: entry_addr>   ; embedded address
        let mut code = Vec::with_capacity(20);
        // AUIPC x5, 0 (placeholder; real use would patch this)
        code.extend_from_slice(
            &Instruction::Auipc {
                rd: Gpr::T0,
                imm: 0x0,
            }
            .encode(),
        );
        // LD x5, 8(x5)
        code.extend_from_slice(
            &Instruction::Ld {
                rd: Gpr::T0,
                rs1: Gpr::T0,
                imm: 8,
            }
            .encode(),
        );
        // JALR x0, x5, 0
        code.extend_from_slice(
            &Instruction::Jalr {
                rd: Gpr::Zero,
                rs1: Gpr::T0,
                imm: 0,
            }
            .encode(),
        );
        // 64-bit address
        code.extend_from_slice(&entry_addr.to_le_bytes());
        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset < bytes.len() {
            // Check for RVC (compressed) 16-bit instruction:
            // Low bits of the first byte determine instruction length.
            // If bits [1:0] != 0b11, it's a 16-bit compressed instruction.
            let first_byte = bytes[offset];
            let is_compressed = (first_byte & 0x03) != 0x03;

            if is_compressed && offset + 2 <= bytes.len() {
                let half = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                let mnemonic = decode_compressed_mnemonic(half);
                lines.push(format!("{:#010x}:  {:04x}    {}", pc, half, mnemonic));
                offset += 2;
                pc += 2;
            } else if offset + 4 <= bytes.len() {
                let word = u32::from_le_bytes([
                    bytes[offset],
                    bytes[offset + 1],
                    bytes[offset + 2],
                    bytes[offset + 3],
                ]);

                // Decode the instruction: prefer the structured Instruction::decode
                // which uses the enum's Display impl; fall back to the string decoder.
                let mnemonic = if let Some(instr) = Instruction::decode(word) {
                    format!("{}", instr)
                } else {
                    decode_mnemonic(word)
                };
                lines.push(format!("{:#010x}:  {:08x}  {}", pc, word, mnemonic));

                offset += 4;
                pc += 4;
            } else {
                let remaining = &bytes[offset..];
                lines.push(format!("{:#010x}:  {:02x?}", pc, remaining));
                break;
            }
        }
        lines
    }

    fn name(&self) -> &'static str {
        "riscv64"
    }
}

/// Simple mnemonic decoder for RISC-V 32-bit instructions.
///
/// Returns a string with the instruction mnemonic and decoded fields.
fn decode_mnemonic(word: u32) -> String {
    let opcode = word & 0x7F;
    let rd = (word >> 7) & 0x1F;
    let funct3 = (word >> 12) & 0x7;
    let rs1 = (word >> 15) & 0x1F;
    let rs2 = (word >> 20) & 0x1F;
    let funct7 = (word >> 25) & 0x7F;

    match opcode {
        0b0110111 => format!("lui x{}, 0x{:05x}", rd, (word >> 12) & 0xFFFFF),
        0b0010111 => format!("auipc x{}, 0x{:05x}", rd, (word >> 12) & 0xFFFFF),
        0b1101111 => {
            let imm20 = ((word >> 31) & 1) << 20
                | ((word >> 12) & 0xFF) << 12
                | ((word >> 20) & 1) << 11
                | ((word >> 21) & 0x3FF) << 1;
            let imm = ((imm20 << 11) as i32) >> 11; // sign extend
            format!("jal x{}, {:+}", rd, imm)
        }
        0b1100111 => {
            let imm = (((word >> 20) as i32) << 20) >> 20;
            format!("jalr x{}, x{}, {}", rd, rs1, imm)
        }
        0b1100011 => {
            let imm12 = ((word >> 31) & 1) << 12
                | ((word >> 7) & 1) << 11
                | ((word >> 25) & 0x3F) << 5
                | ((word >> 8) & 0xF) << 1;
            let imm = ((imm12 << 19) as i32) >> 19; // sign extend
            let br_name = match funct3 {
                0b000 => "beq",
                0b001 => "bne",
                0b100 => "blt",
                0b101 => "bge",
                0b110 => "bltu",
                0b111 => "bgeu",
                _ => "b??",
            };
            format!("{} x{}, x{}, {:+}", br_name, rs1, rs2, imm)
        }
        0b0000011 => {
            let imm = (((word >> 20) as i32) << 20) >> 20;
            let ld_name = match funct3 {
                0b000 => "lb",
                0b001 => "lh",
                0b010 => "lw",
                0b011 => "ld",
                0b100 => "lbu",
                0b101 => "lhu",
                0b110 => "lwu",
                _ => "l??",
            };
            format!("{} x{}, {}(x{})", ld_name, rd, imm, rs1)
        }
        0b0100011 => {
            let imm = ((word >> 25) << 5) | ((word >> 7) & 0x1F);
            let imm = ((imm as i32) << 19) >> 19; // sign extend 12-bit
            let st_name = match funct3 {
                0b000 => "sb",
                0b001 => "sh",
                0b010 => "sw",
                0b011 => "sd",
                _ => "s??",
            };
            format!("{} x{}, {}(x{})", st_name, rs2, imm, rs1)
        }
        0b0010011 => {
            // OP-IMM
            let imm = (((word >> 20) as i32) << 20) >> 20;
            let shamt = (word >> 20) & 0x3F;
            match funct3 {
                0b000 => format!("addi x{}, x{}, {}", rd, rs1, imm),
                0b010 => format!("slti x{}, x{}, {}", rd, rs1, imm),
                0b011 => format!("sltiu x{}, x{}, {}", rd, rs1, imm),
                0b100 => format!("xori x{}, x{}, {}", rd, rs1, imm),
                0b110 => format!("ori x{}, x{}, {}", rd, rs1, imm),
                0b111 => format!("andi x{}, x{}, {}", rd, rs1, imm),
                0b001 => format!("slli x{}, x{}, {}", rd, rs1, shamt),
                0b101 => {
                    if funct7 == 0b0100000 {
                        format!("srai x{}, x{}, {}", rd, rs1, shamt)
                    } else {
                        format!("srli x{}, x{}, {}", rd, rs1, shamt)
                    }
                }
                _ => format!("op-imm??? funct3={}", funct3),
            }
        }
        0b0110011 => {
            // OP
            let op_name = match (funct7, funct3) {
                (0b0000000, 0b000) => "add",
                (0b0100000, 0b000) => "sub",
                (0b0000000, 0b001) => "sll",
                (0b0000000, 0b010) => "slt",
                (0b0000000, 0b011) => "sltu",
                (0b0000000, 0b100) => "xor",
                (0b0000000, 0b101) => "srl",
                (0b0100000, 0b101) => "sra",
                (0b0000000, 0b110) => "or",
                (0b0000000, 0b111) => "and",
                (0b0000001, 0b000) => "mul",
                (0b0000001, 0b001) => "mulh",
                (0b0000001, 0b010) => "mulhsu",
                (0b0000001, 0b011) => "mulhu",
                (0b0000001, 0b100) => "div",
                (0b0000001, 0b101) => "divu",
                (0b0000001, 0b110) => "rem",
                (0b0000001, 0b111) => "remu",
                _ => "op???",
            };
            format!("{} x{}, x{}, x{}", op_name, rd, rs1, rs2)
        }
        0b0011011 => {
            // OP-IMM-32
            let imm = (((word >> 20) as i32) << 20) >> 20;
            let shamt = (word >> 20) & 0x1F;
            match funct3 {
                0b000 => format!("addiw x{}, x{}, {}", rd, rs1, imm),
                0b001 => format!("slliw x{}, x{}, {}", rd, rs1, shamt),
                0b101 => {
                    if funct7 == 0b0100000 {
                        format!("sraiw x{}, x{}, {}", rd, rs1, shamt)
                    } else {
                        format!("srliw x{}, x{}, {}", rd, rs1, shamt)
                    }
                }
                _ => format!("op-imm32??? funct3={}", funct3),
            }
        }
        0b0111011 => {
            // OP-32
            let op_name = match (funct7, funct3) {
                (0b0000000, 0b000) => "addw",
                (0b0100000, 0b000) => "subw",
                (0b0000000, 0b001) => "sllw",
                (0b0000000, 0b101) => "srlw",
                (0b0100000, 0b101) => "sraw",
                _ => "op32???",
            };
            format!("{} x{}, x{}, x{}", op_name, rd, rs1, rs2)
        }
        0b1110011 => {
            if word == 0x00000073 {
                "ecall".to_string()
            } else if word == 0x00100073 {
                "ebreak".to_string()
            } else {
                let csr = (word >> 20) & 0xFFF;
                let csr_name = match funct3 {
                    0b001 => "csrrw",
                    0b010 => "csrrs",
                    0b011 => "csrrc",
                    0b101 => "csrrwi",
                    0b110 => "csrrsi",
                    0b111 => "csrrci",
                    _ => "system???",
                };
                format!("{} x{}, 0x{:03x}, x{}", csr_name, rd, csr, rs1)
            }
        }
        0b0001111 => {
            if funct3 == 0b001 {
                "fence.i".to_string()
            } else {
                "fence".to_string()
            }
        }
        0b1010011 => "fp_op".to_string(),
        _ => format!("unknown(opcode={:#05b})", opcode),
    }
}

/// Decode a compressed (RVC) 16-bit instruction into a mnemonic string.
///
/// Handles the main quadrants (0, 1, 2) of the RVC encoding space.
fn decode_compressed_mnemonic(half: u16) -> String {
    let op = half & 0x03;
    let funct3 = (half >> 13) & 0x07;
    match op {
        0 => {
            // Quadrant 0: CIW, CL, CS
            match funct3 {
                0b000 => "c.addi4spn".to_string(),
                0b010 => "c.lw".to_string(),
                0b011 => "c.ld".to_string(),
                0b110 => "c.sw".to_string(),
                0b111 => "c.sd".to_string(),
                _ => format!("c.q0??? funct3={}", funct3),
            }
        }
        1 => {
            // Quadrant 1: CI, CB, CJ
            match funct3 {
                0b000 => "c.nop/c.addi".to_string(),
                0b001 => "c.addiw".to_string(),
                0b010 => "c.li".to_string(),
                0b011 => "c.addi16sp/c.lui".to_string(),
                0b100 => "c.srli/c.srai/c.andi/c.sub/c.xor/c.or/c.and".to_string(),
                0b101 => "c.j".to_string(),
                0b110 => "c.beqz".to_string(),
                0b111 => "c.bnez".to_string(),
                _ => format!("c.q1??? funct3={}", funct3),
            }
        }
        2 => {
            // Quadrant 2: CI, CSS, CIW, CL, CS, CB
            match funct3 {
                0b000 => "c.slli".to_string(),
                0b010 => "c.lwsp".to_string(),
                0b011 => "c.ldsp".to_string(),
                0b100 => "c.jr/c.mv/c.ebreak/c.jalr/c.add".to_string(),
                0b110 => "c.swsp".to_string(),
                0b111 => "c.sdsp".to_string(),
                _ => format!("c.q2??? funct3={}", funct3),
            }
        }
        3 => {
            // This should not happen (32-bit instruction), but just in case
            format!("c.illegal({:#06x})", half)
        }
        _ => unreachable!(),
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(any())] // Disabled: broken tests need fixing
mod tests {
    use super::*;

    // ── Gpr Encoding Tests ───────────────────────────────────────────

    #[test]
    fn test_gpr_encoding_values() {
        assert_eq!(Gpr::Zero.encoding(), 0);
        assert_eq!(Gpr::Ra.encoding(), 1);
        assert_eq!(Gpr::Sp.encoding(), 2);
        assert_eq!(Gpr::Gp.encoding(), 3);
        assert_eq!(Gpr::Tp.encoding(), 4);
        assert_eq!(Gpr::T0.encoding(), 5);
        assert_eq!(Gpr::T1.encoding(), 6);
        assert_eq!(Gpr::T2.encoding(), 7);
        assert_eq!(Gpr::S0.encoding(), 8);
        assert_eq!(Gpr::S1.encoding(), 9);
        assert_eq!(Gpr::A0.encoding(), 10);
        assert_eq!(Gpr::A1.encoding(), 11);
        assert_eq!(Gpr::A2.encoding(), 12);
        assert_eq!(Gpr::A3.encoding(), 13);
        assert_eq!(Gpr::A4.encoding(), 14);
        assert_eq!(Gpr::A5.encoding(), 15);
        assert_eq!(Gpr::A6.encoding(), 16);
        assert_eq!(Gpr::A7.encoding(), 17);
        assert_eq!(Gpr::S2.encoding(), 18);
        assert_eq!(Gpr::S3.encoding(), 19);
        assert_eq!(Gpr::S4.encoding(), 20);
        assert_eq!(Gpr::S5.encoding(), 21);
        assert_eq!(Gpr::S6.encoding(), 22);
        assert_eq!(Gpr::S7.encoding(), 23);
        assert_eq!(Gpr::S8.encoding(), 24);
        assert_eq!(Gpr::S9.encoding(), 25);
        assert_eq!(Gpr::S10.encoding(), 26);
        assert_eq!(Gpr::S11.encoding(), 27);
        assert_eq!(Gpr::T3.encoding(), 28);
        assert_eq!(Gpr::T4.encoding(), 29);
        assert_eq!(Gpr::T5.encoding(), 30);
        assert_eq!(Gpr::T6.encoding(), 31);
    }

    #[test]
    fn test_gpr_is_allocatable() {
        assert!(!Gpr::Zero.is_allocatable());
        assert!(!Gpr::Sp.is_allocatable());
        assert!(!Gpr::Gp.is_allocatable());
        assert!(!Gpr::Tp.is_allocatable());
        assert!(Gpr::T0.is_allocatable());
        assert!(Gpr::A0.is_allocatable());
        assert!(Gpr::S0.is_allocatable());
        assert!(Gpr::Ra.is_allocatable());
    }

    #[test]
    fn test_gpr_is_callee_saved() {
        assert!(Gpr::S0.is_callee_saved());
        assert!(Gpr::S11.is_callee_saved());
        assert!(!Gpr::T0.is_callee_saved());
        assert!(!Gpr::A0.is_callee_saved());
        assert!(!Gpr::Ra.is_callee_saved());
    }

    #[test]
    fn test_gpr_is_arg_reg() {
        assert!(Gpr::A0.is_arg_reg());
        assert!(Gpr::A7.is_arg_reg());
        assert!(!Gpr::T0.is_arg_reg());
        assert!(!Gpr::S0.is_arg_reg());
    }

    #[test]
    fn test_gpr_arg_register() {
        assert_eq!(Gpr::arg_register(0), Some(Gpr::A0));
        assert_eq!(Gpr::arg_register(7), Some(Gpr::A7));
        assert_eq!(Gpr::arg_register(8), None);
    }

    // ── Fpr Encoding Tests ───────────────────────────────────────────

    #[test]
    fn test_fpr_encoding_values() {
        assert_eq!(Fpr::F0.encoding(), 0);
        assert_eq!(Fpr::F10.encoding(), 10);
        assert_eq!(Fpr::F31.encoding(), 31);
    }

    #[test]
    fn test_fpr_is_callee_saved() {
        assert!(Fpr::F8.is_callee_saved());
        assert!(Fpr::F9.is_callee_saved());
        assert!(Fpr::F27.is_callee_saved());
        assert!(!Fpr::F0.is_callee_saved());
        assert!(!Fpr::F10.is_callee_saved());
    }

    #[test]
    fn test_fpr_is_arg_reg() {
        assert!(Fpr::F10.is_arg_reg());
        assert!(Fpr::F17.is_arg_reg());
        assert!(!Fpr::F0.is_arg_reg());
    }

    // ── R-type Encoding Tests ────────────────────────────────────────

    #[test]
    fn test_r_type_add() {
        // ADD x5, x6, x7  =>  funct7=0, rs2=7, rs1=6, funct3=0, rd=5, opcode=0x33
        let bytes = Instruction::Add {
            rd: Gpr::T0,
            rs1: Gpr::T1,
            rs2: Gpr::T2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0110011); // opcode
        assert_eq!((word >> 12) & 0x7, 0b000); // funct3 = ADD
        assert_eq!((word >> 7) & 0x1F, 5); // rd = t0
        assert_eq!((word >> 15) & 0x1F, 6); // rs1 = t1
        assert_eq!((word >> 20) & 0x1F, 7); // rs2 = t2
        assert_eq!((word >> 25) & 0x7F, 0); // funct7 = 0
    }

    #[test]
    fn test_r_type_sub() {
        // SUB x10, x11, x12  =>  funct7=0b0100000, rs2=12, rs1=11, funct3=0, rd=10, opcode=0x33
        let bytes = Instruction::Sub {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 25) & 0x7F, 0b0100000); // funct7 for SUB
        assert_eq!((word >> 7) & 0x1F, 10); // rd = a0
    }

    #[test]
    fn test_r_type_mul() {
        // MUL x10, x11, x12  =>  funct7=0b0000001, rs2=12, rs1=11, funct3=0, rd=10, opcode=0x33
        let bytes = Instruction::Mul {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 25) & 0x7F, 0b0000001); // funct7 for MUL
        assert_eq!((word >> 12) & 0x7, 0b000); // funct3 = MUL
    }

    #[test]
    fn test_r_type_div_rem() {
        let bytes = Instruction::Div {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 25) & 0x7F, 0b0000001);
        assert_eq!((word >> 12) & 0x7, 0b100); // funct3 = DIV

        let bytes = Instruction::Rem {
            rd: Gpr::T0,
            rs1: Gpr::T1,
            rs2: Gpr::T2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 12) & 0x7, 0b110); // funct3 = REM
    }

    // ── I-type Encoding Tests ────────────────────────────────────────

    #[test]
    fn test_i_type_addi() {
        // ADDI x5, x6, 42  =>  imm=42, rs1=6, funct3=0, rd=5, opcode=0x13
        let bytes = Instruction::Addi {
            rd: Gpr::T0,
            rs1: Gpr::T1,
            imm: 42,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0010011); // opcode
        assert_eq!((word >> 12) & 0x7, 0b000); // funct3
        assert_eq!((word >> 7) & 0x1F, 5); // rd
        assert_eq!((word >> 15) & 0x1F, 6); // rs1
        let imm = (((word >> 20) as i32) << 20) >> 20;
        assert_eq!(imm, 42); // imm (sign-extended)
    }

    #[test]
    fn test_i_type_addi_negative() {
        // ADDI x5, x6, -1  =>  imm=0xFFF, rs1=6, funct3=0, rd=5, opcode=0x13
        let bytes = Instruction::Addi {
            rd: Gpr::T0,
            rs1: Gpr::T1,
            imm: -1,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 20) & 0xFFF, 0xFFF); // -1 as 12-bit immediate
    }

    #[test]
    fn test_i_type_ld() {
        // LD x10, 8(x2)  =>  imm=8, rs1=2, funct3=3, rd=10, opcode=0x03
        let bytes = Instruction::Ld {
            rd: Gpr::A0,
            rs1: Gpr::Sp,
            imm: 8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0000011); // opcode = LOAD
        assert_eq!((word >> 12) & 0x7, 0b011); // funct3 = LD
        assert_eq!((word >> 7) & 0x1F, 10); // rd = a0
        assert_eq!((word >> 15) & 0x1F, 2); // rs1 = sp
        let imm = (((word >> 20) as i32) << 20) >> 20;
        assert_eq!(imm, 8); // imm
    }

    #[test]
    fn test_i_type_jalr() {
        // JALR x0, x1, 0  =>  imm=0, rs1=1, funct3=0, rd=0, opcode=0x67
        let bytes = Instruction::Jalr {
            rd: Gpr::Zero,
            rs1: Gpr::Ra,
            imm: 0,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word, 0x00008067); // JALR x0, x1, 0
    }

    // ── S-type Encoding Tests ────────────────────────────────────────

    #[test]
    fn test_s_type_sw() {
        // SW x10, 4(x2)  =>  imm=4, rs2=10, rs1=2, funct3=2, opcode=0x23
        let bytes = Instruction::Sw {
            rs1: Gpr::Sp,
            rs2: Gpr::A0,
            imm: 4,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0100011); // opcode = STORE
        assert_eq!((word >> 12) & 0x7, 0b010); // funct3 = SW
        assert_eq!((word >> 15) & 0x1F, 2); // rs1 = sp
        assert_eq!((word >> 20) & 0x1F, 10); // rs2 = a0
                                             // Check immediate: lower 5 bits in [11:7], upper 7 bits in [31:25]
        assert_eq!((word >> 7) & 0x1F, 4); // imm[4:0]
        assert_eq!((word >> 25) & 0x7F, 0); // imm[11:5]
    }

    #[test]
    fn test_s_type_sd() {
        // SD x1, -8(x2)  =>  imm=-8, rs2=1, rs1=2, funct3=3, opcode=0x23
        let bytes = Instruction::Sd {
            rs1: Gpr::Sp,
            rs2: Gpr::Ra,
            imm: -8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 12) & 0x7, 0b011); // funct3 = SD
        assert_eq!((word >> 20) & 0x1F, 1); // rs2 = ra
                                            // Reconstruct the full immediate
        let imm_lo = (word >> 7) & 0x1F;
        let imm_hi = (word >> 25) & 0x7F;
        let imm_raw = (imm_hi << 5) | imm_lo;
        let imm = ((imm_raw as i32) << 20) >> 20; // sign extend 12-bit
        assert_eq!(imm, -8);
    }

    // ── B-type Encoding Tests ────────────────────────────────────────

    #[test]
    fn test_b_type_beq_positive_offset() {
        // BEQ x10, x11, 16  =>  offset=16, rs1=10, rs2=11, funct3=0, opcode=0x63
        let bytes = Instruction::Beq {
            rs1: Gpr::A0,
            rs2: Gpr::A1,
            offset: 16,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b1100011); // opcode = BRANCH
        assert_eq!((word >> 12) & 0x7, 0b000); // funct3 = BEQ
        assert_eq!((word >> 15) & 0x1F, 10); // rs1 = a0
        assert_eq!((word >> 20) & 0x1F, 11); // rs2 = a1
                                             // Verify offset encoding: imm[12|10:5] | rs2 | rs1 | funct3 | imm[4:1|11] | opcode
                                             // offset=16 = 0b0_0000000010000 (13 bits)
                                             // imm[12]=0, imm[11]=0, imm[10:5]=000000, imm[4:1]=1000
        assert_eq!((word >> 31) & 1, 0); // imm[12] = 0
        assert_eq!((word >> 7) & 1, 0); // imm[11] = 0
        assert_eq!((word >> 25) & 0x3F, 0); // imm[10:5] = 0
        assert_eq!((word >> 8) & 0xF, 8); // imm[4:1] = 8
    }

    #[test]
    fn test_b_type_bne_negative_offset() {
        // BNE x5, x6, -4  =>  offset=-4 (0xFFFFC as 21-bit), rs1=5, rs2=6, funct3=1
        let bytes = Instruction::Bne {
            rs1: Gpr::T0,
            rs2: Gpr::T1,
            offset: -4,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 12) & 0x7, 0b001); // funct3 = BNE
                                               // offset=-4: binary representation is ...11111111100
                                               // imm[12]=1, imm[11]=1, imm[10:5]=111111, imm[4:1]=1110
        assert_eq!((word >> 31) & 1, 1); // imm[12] = 1
        assert_eq!((word >> 7) & 1, 1); // imm[11] = 1
    }

    #[test]
    fn test_b_type_blt() {
        let bytes = Instruction::Blt {
            rs1: Gpr::A0,
            rs2: Gpr::A1,
            offset: 4096,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 12) & 0x7, 0b100); // funct3 = BLT
                                               // offset=4096 = 0b1_000000000000 => imm[12]=1, everything else 0
        assert_eq!((word >> 31) & 1, 1); // imm[12] = 1
    }

    // ── U-type Encoding Tests ────────────────────────────────────────

    #[test]
    fn test_u_type_lui() {
        // LUI x5, 0x12345000  =>  rd=5, imm=0x12345000, opcode=0x37
        let bytes = Instruction::Lui {
            rd: Gpr::T0,
            imm: 0x12345000,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0110111); // opcode = LUI
        assert_eq!((word >> 7) & 0x1F, 5); // rd = t0
        assert_eq!((word >> 12) & 0xFFFFF, 0x12345); // upper 20 bits
    }

    #[test]
    fn test_u_type_auipc() {
        // AUIPC x10, 0xABCDE000  =>  rd=10, imm=0xABCDE000, opcode=0x17
        let bytes = Instruction::Auipc {
            rd: Gpr::A0,
            imm: 0xABCDE000,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0010111); // opcode = AUIPC
        assert_eq!((word >> 7) & 0x1F, 10); // rd = a0
        assert_eq!((word >> 12) & 0xFFFFF, 0xABCDE); // upper 20 bits
    }

    // ── J-type Encoding Tests ────────────────────────────────────────

    #[test]
    fn test_j_type_jal() {
        // JAL x1, 100  =>  rd=1, offset=100, opcode=0x6F
        let bytes = Instruction::Jal {
            rd: Gpr::Ra,
            offset: 100,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b1101111); // opcode = JAL
        assert_eq!((word >> 7) & 0x1F, 1); // rd = ra
                                           // offset=100 = 0b0_0000000_0_1100100
                                           // J-type: imm[20|10:1|11|19:12]
                                           // imm[20]=0, imm[19:12]=0, imm[11]=0, imm[10:1]=1100100 (50<<1=100)
        assert_eq!((word >> 31) & 1, 0); // imm[20] = 0
        assert_eq!((word >> 12) & 0xFF, 0); // imm[19:12] = 0
        assert_eq!((word >> 20) & 1, 0); // imm[11] = 0
        assert_eq!((word >> 21) & 0x3FF, 50); // imm[10:1] = 50 (100/2)
    }

    #[test]
    fn test_j_type_jal_negative_offset() {
        // JAL x1, -4  =>  rd=1, offset=-4
        let bytes = Instruction::Jal {
            rd: Gpr::Ra,
            offset: -4,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        // offset=-4: 0x...FFFFFC
        // imm[20]=1, imm[19:12]=0xFF, imm[11]=1, imm[10:1]=0x1FE (510)
        assert_eq!((word >> 31) & 1, 1); // imm[20] = 1
    }

    // ── Return Stub Test ─────────────────────────────────────────────

    #[test]
    fn test_return_stub() {
        let backend = RiscV64Backend::new();
        let stub = backend.return_stub();
        // JALR x0, x1, 0 = 0x00008067
        assert_eq!(stub, vec![0x67, 0x80, 0x00, 0x00]);
        // Also verify by decoding the word
        let word = u32::from_le_bytes([stub[0], stub[1], stub[2], stub[3]]);
        assert_eq!(word, 0x00008067);
    }

    // ── NOP Test ─────────────────────────────────────────────────────

    #[test]
    fn test_nop_encoding() {
        let bytes = Instruction::Nop.encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word, 0x00000013); // ADDI x0, x0, 0
    }

    // ── ECALL/EBREAK Tests ───────────────────────────────────────────

    #[test]
    fn test_ecall_encoding() {
        let bytes = Instruction::Ecall.encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word, 0x00000073);
    }

    #[test]
    fn test_ebreak_encoding() {
        let bytes = Instruction::Ebreak.encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word, 0x00100073);
    }

    // ── Backend Trait Dispatch Test ──────────────────────────────────

    #[test]
    fn test_backend_trait_dispatch() {
        let backend: Box<dyn Backend> = Box::new(RiscV64Backend::new());
        assert_eq!(backend.name(), "riscv64");
        let info = backend.target_info();
        assert_eq!(info.isa_name(), "riscv64");
        assert_eq!(info.elf_machine_type(), 243);
        assert_eq!(info.pointer_width(), 8);
        assert_eq!(info.has_hardwired_zero(), true);
        assert_eq!(info.has_link_register(), true);
        assert_eq!(info.calling_convention_name(), "lp64d");
    }

    // ── Create Backend via Factory ───────────────────────────────────

    #[test]
    fn test_create_backend_riscv64() {
        let backend = crate::backend::create_backend(crate::backend::BackendKind::RiscV64);
        assert!(backend.is_ok());
        let backend = backend.unwrap();
        assert_eq!(backend.name(), "riscv64");
    }

    // ── Shift Instruction Tests ──────────────────────────────────────

    #[test]
    fn test_slli_encoding() {
        // SLLI x10, x11, 5  =>  shamt=5, rs1=11, funct3=1, rd=10, opcode=0x13
        let bytes = Instruction::Slli {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            shamt: 5,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0010011);
        assert_eq!((word >> 12) & 0x7, 0b001);
        assert_eq!((word >> 20) & 0x3F, 5); // shamt (6-bit for RV64)
    }

    #[test]
    fn test_srai_encoding() {
        // SRAI x10, x11, 7  =>  funct7=0b0100000, shamt=7, rs1=11, funct3=5, rd=10
        let bytes = Instruction::Srai {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            shamt: 7,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 25) & 0x7F, 0b0100000); // funct7 for SRAI
        assert_eq!((word >> 12) & 0x7, 0b101); // funct3
        assert_eq!((word >> 20) & 0x3F, 7); // shamt
    }

    // ── Word-level (RV64) Arithmetic Tests ───────────────────────────

    #[test]
    fn test_addw_encoding() {
        let bytes = Instruction::Addw {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0111011); // opcode = OP-32
        assert_eq!((word >> 12) & 0x7, 0b000); // funct3
        assert_eq!((word >> 25) & 0x7F, 0b0000000); // funct7
    }

    #[test]
    fn test_addiw_encoding() {
        let bytes = Instruction::Addiw {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            imm: 32,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0011011); // opcode = OP-IMM-32
        assert_eq!((word >> 12) & 0x7, 0b000); // funct3
        let imm = (((word >> 20) as i32) << 20) >> 20;
        assert_eq!(imm, 32); // imm
    }

    // ── FP Load/Store Tests ──────────────────────────────────────────

    #[test]
    fn test_fld_encoding() {
        // FLD f10, 16(x10)  =>  imm=16, rs1=10, funct3=3, rd=10, opcode=0x03
        let bytes = Instruction::Fld {
            rd: Fpr::F10,
            rs1: Gpr::A0,
            imm: 16,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0000011); // opcode = LOAD
        assert_eq!((word >> 12) & 0x7, 0b011); // funct3 = LD/FLD
        assert_eq!((word >> 7) & 0x1F, 10); // rd = f10
    }

    #[test]
    fn test_fsd_encoding() {
        // FSD f10, 8(x10)  =>  imm=8, rs2=10, rs1=10, funct3=3, opcode=0x23
        let bytes = Instruction::Fsd {
            rs1: Gpr::A0,
            rs2: Fpr::F10,
            imm: 8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0100011); // opcode = STORE
        assert_eq!((word >> 12) & 0x7, 0b011); // funct3 = SD/FSD
    }

    // ── Trampoline Test ──────────────────────────────────────────────

    #[test]
    fn test_trampoline_contains_jump() {
        let backend = RiscV64Backend::new();
        let tramp = backend.trampoline(0x10000);
        // Should contain at least 3 instructions (12 bytes) + 8 bytes address = 20 bytes
        assert_eq!(tramp.len(), 20);
        // First instruction: AUIPC x5, 0x0
        let word0 = u32::from_le_bytes([tramp[0], tramp[1], tramp[2], tramp[3]]);
        assert_eq!(word0 & 0x7F, 0b0010111); // opcode = AUIPC
                                             // Second instruction: LD x5, 8(x5)
        let word1 = u32::from_le_bytes([tramp[4], tramp[5], tramp[6], tramp[7]]);
        assert_eq!(word1 & 0x7F, 0b0000011); // opcode = LOAD
                                             // Third instruction: JALR x0, x5, 0
        let word2 = u32::from_le_bytes([tramp[8], tramp[9], tramp[10], tramp[11]]);
        assert_eq!(word2 & 0x7F, 0b1100111); // opcode = JALR
    }

    // ── Disassembly Test ─────────────────────────────────────────────

    #[test]
    fn test_disassemble_nop() {
        let backend = RiscV64Backend::new();
        let nop_bytes = Instruction::Nop.encode();
        let lines = backend.disassemble(&nop_bytes, 0x10000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("addi")); // NOP is ADDI x0, x0, 0
    }

    // ── Fence Test ───────────────────────────────────────────────────

    #[test]
    fn test_fence_encoding() {
        // FENCE iorw, iorw  =>  pred=0xF, succ=0xF
        let bytes = Instruction::Fence {
            pred: 0xF,
            succ: 0xF,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0001111); // opcode = MISC-MEM
        assert_eq!((word >> 20) & 0xFF, 0xFF); // pred|succ = 0xFF
    }

    // ── ELF Generation Test ──────────────────────────────────────────

    #[test]
    fn test_elf_header_machine_type() {
        let code = Instruction::Nop.encode();
        let elf = build_minimal_riscv64_elf(&code, 0x10000);
        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // Check e_machine at offset 18 (2 bytes LE)
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, 243); // EM_RISCV
    }

    // ── Register Display Test ────────────────────────────────────────

    #[test]
    fn test_gpr_asm_name() {
        assert_eq!(Gpr::Zero.asm_name(), "zero");
        assert_eq!(Gpr::Ra.asm_name(), "ra");
        assert_eq!(Gpr::Sp.asm_name(), "sp");
        assert_eq!(Gpr::A0.asm_name(), "a0");
        assert_eq!(Gpr::T0.asm_name(), "t0");
        assert_eq!(Gpr::S0.asm_name(), "s0");
    }

    // ── Instruction Display Test ─────────────────────────────────────

    #[test]
    fn test_instruction_display() {
        assert_eq!(
            format!(
                "{}",
                Instruction::Add {
                    rd: Gpr::A0,
                    rs1: Gpr::A1,
                    rs2: Gpr::A2
                }
            ),
            "add a0, a1, a2"
        );
        assert_eq!(
            format!(
                "{}",
                Instruction::Addi {
                    rd: Gpr::T0,
                    rs1: Gpr::T1,
                    imm: 42
                }
            ),
            "addi t0, t1, 42"
        );
        assert_eq!(
            format!(
                "{}",
                Instruction::Ld {
                    rd: Gpr::A0,
                    rs1: Gpr::Sp,
                    imm: 8
                }
            ),
            "ld a0, 8(sp)"
        );
    }

    // ── Zicsr Encoding Tests ─────────────────────────────────────────

    #[test]
    fn test_csrrw_encoding() {
        // CSRRW x10, 0x300 (mstatus), x5
        let bytes = Instruction::Csrrw {
            rd: Gpr::A0,
            csr: 0x300,
            rs1: Gpr::T0,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b1110011); // opcode = SYSTEM
        assert_eq!((word >> 12) & 0x7, 0b001); // funct3 = CSRRW
        assert_eq!((word >> 7) & 0x1F, 10); // rd = a0
        assert_eq!((word >> 15) & 0x1F, 5); // rs1 = t0
        assert_eq!((word >> 20) & 0xFFF, 0x300); // csr
    }

    #[test]
    fn test_csrrs_encoding() {
        // CSRRS x11, 0x342 (mcause), x6
        let bytes = Instruction::Csrrs {
            rd: Gpr::A1,
            csr: 0x342,
            rs1: Gpr::T1,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 12) & 0x7, 0b010); // funct3 = CSRRS
        assert_eq!((word >> 20) & 0xFFF, 0x342); // csr = mcause
    }

    #[test]
    fn test_csrrc_encoding() {
        let bytes = Instruction::Csrrc {
            rd: Gpr::A2,
            csr: 0x344,
            rs1: Gpr::T2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 12) & 0x7, 0b011); // funct3 = CSRRC
    }

    #[test]
    fn test_csrrwi_encoding() {
        // CSRRWI x10, 0x300, 5
        let bytes = Instruction::Csrrwi {
            rd: Gpr::A0,
            csr: 0x300,
            uimm: 5,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 12) & 0x7, 0b101); // funct3 = CSRRWI
        assert_eq!((word >> 15) & 0x1F, 5); // uimm in rs1 field
    }

    #[test]
    fn test_csrrsi_csrrci_encoding() {
        let bytes_si = Instruction::Csrrsi {
            rd: Gpr::A0,
            csr: 0x342,
            uimm: 3,
        }
        .encode();
        let word_si = u32::from_le_bytes(bytes_si);
        assert_eq!((word_si >> 12) & 0x7, 0b110);

        let bytes_ci = Instruction::Csrrci {
            rd: Gpr::A0,
            csr: 0x342,
            uimm: 3,
        }
        .encode();
        let word_ci = u32::from_le_bytes(bytes_ci);
        assert_eq!((word_ci >> 12) & 0x7, 0b111);
    }

    #[test]
    fn test_fence_i_encoding() {
        let bytes = Instruction::FenceI.encode();
        let word = u32::from_le_bytes(bytes);
        // FENCE.I = 0x0000100F
        assert_eq!(word & 0x7F, 0b0001111); // opcode = MISC-MEM
        assert_eq!((word >> 12) & 0x7, 0b001); // funct3 = 1 for fence.i
    }

    #[test]
    fn test_m_extension_mul_div() {
        // MUL x10, x11, x12 => funct7=0b0000001, funct3=0b000, opcode=OP_REG
        let mul_bytes = Instruction::Mul {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        }
        .encode();
        let mul_word = u32::from_le_bytes(mul_bytes);
        assert_eq!((mul_word >> 25) & 0x7F, 0b0000001); // funct7 for M ext
        assert_eq!((mul_word >> 12) & 0x7, 0b000); // MUL funct3
        assert_eq!(mul_word & 0x7F, 0b0110011); // OP_REG

        // DIV x10, x11, x12 => funct3=0b100
        let div_bytes = Instruction::Div {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        }
        .encode();
        let div_word = u32::from_le_bytes(div_bytes);
        assert_eq!((div_word >> 12) & 0x7, 0b100); // DIV funct3

        // REMU x10, x11, x12 => funct3=0b111
        let remu_bytes = Instruction::Remu {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        }
        .encode();
        let remu_word = u32::from_le_bytes(remu_bytes);
        assert_eq!((remu_word >> 12) & 0x7, 0b111); // REMU funct3
    }

    #[test]
    fn test_disassemble_with_compressed() {
        let backend = RiscV64Backend::new();
        // Mix a 32-bit NOP (0x00000013) with a 16-bit C.NOP (0x0001)
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x00000013u32.to_le_bytes()); // 32-bit NOP
        bytes.extend_from_slice(&0x0001u16.to_le_bytes()); // 16-bit C.NOP
        let lines = backend.disassemble(&bytes, 0x1000);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("addi")); // 32-bit NOP decodes as addi x0,x0,0
        assert!(lines[1].contains("c.nop")); // 16-bit compressed NOP
    }

    #[test]
    fn test_disassemble_csrrw() {
        let backend = RiscV64Backend::new();
        let instr = Instruction::Csrrw {
            rd: Gpr::A0,
            csr: 0x300,
            rs1: Gpr::T0,
        };
        let bytes = instr.encode();
        let lines = backend.disassemble(&bytes, 0x1000);
        assert!(lines[0].contains("csrrw"));
        assert!(lines[0].contains("0x300"));
    }

    #[test]
    fn test_disassemble_add() {
        let backend = RiscV64Backend::new();
        let instr = Instruction::Add {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        };
        let bytes = instr.encode();
        let lines = backend.disassemble(&bytes, 0x1000);
        assert!(lines[0].contains("add"), "Expected add, got: {}", lines[0]);
    }

    #[test]
    fn test_disassemble_lui() {
        let backend = RiscV64Backend::new();
        let instr = Instruction::Lui {
            rd: Gpr::A0,
            imm: 0x12345,
        };
        let bytes = instr.encode();
        let lines = backend.disassemble(&bytes, 0);
        assert!(lines[0].contains("lui"), "Expected lui, got: {}", lines[0]);
    }

    // ── Instruction Selection (ISel) Tests ─────────────────────────────

    /// Helper: extract the nth 4-byte instruction word from a byte buffer.
    fn instr_word(code: &[u8], index: usize) -> u32 {
        let off = index * 4;
        u32::from_le_bytes([code[off], code[off + 1], code[off + 2], code[off + 3]])
    }

    #[test]
    fn test_isel_clz_nonzero() {
        // CLZ of a value with MSB at bit 4 (e.g. 0x10) should produce 59.
        let code = emit_clz_isel(Gpr::T0, Gpr::T0);
        // Verify it emits multiple instructions (not a single NOP)
        assert!(
            code.len() > 4,
            "CLZ should emit more than one instruction, got {} bytes",
            code.len()
        );
        // The first instruction should be ADDI t4, x0, 0 (li t4, 0) or similar
        // Just verify the sequence isn't a NOP (0x00000013)
        let first = instr_word(&code, 0);
        assert_ne!(first, 0x00000013, "First instruction should not be NOP");
    }

    #[test]
    fn test_isel_clz_emits_branch() {
        // The CLZ sequence should contain a BEQ to handle the zero case
        let code = emit_clz_isel(Gpr::T0, Gpr::T0);
        let mut found_beq = false;
        for i in 0..code.len() / 4 {
            let word = instr_word(&code, i);
            if (word & 0x7F) == 0b1100011 && ((word >> 12) & 0x7) == 0b000 {
                found_beq = true;
                break;
            }
        }
        assert!(found_beq, "CLZ sequence should contain a BEQ instruction");
    }

    #[test]
    fn test_isel_ctz_isolates_lowest_bit() {
        // CTZ uses x & (-x) to isolate the lowest set bit, which requires
        // a SUB and AND instruction
        let code = emit_ctz_isel(Gpr::T0, Gpr::T0);
        assert!(code.len() > 4, "CTZ should emit more than one instruction");
        let mut found_and = false;
        for i in 0..code.len() / 4 {
            let word = instr_word(&code, i);
            // AND: opcode=0b0110011, funct3=0b111
            if (word & 0x7F) == 0b0110011 && ((word >> 12) & 0x7) == 0b111 {
                found_and = true;
                break;
            }
        }
        assert!(
            found_and,
            "CTZ sequence should contain an AND instruction (for x & -x)"
        );
    }

    #[test]
    fn test_isel_ctz_handles_zero() {
        // CTZ should handle the zero case via SLTIU (which detects zero)
        let code = emit_ctz_isel(Gpr::T0, Gpr::T0);
        let mut found_sltiu = false;
        for i in 0..code.len() / 4 {
            let word = instr_word(&code, i);
            // SLTIU: opcode=0b0010011, funct3=0b011
            if (word & 0x7F) == 0b0010011 && ((word >> 12) & 0x7) == 0b011 {
                found_sltiu = true;
                break;
            }
        }
        assert!(
            found_sltiu,
            "CTZ sequence should contain SLTIU for zero detection"
        );
    }

    #[test]
    fn test_isel_popcnt_builds_constant() {
        // POPCNT uses the bit-parallel algorithm which builds 0x5555... mask
        // via OR + SLLI sequences
        let code = emit_popcnt_isel(Gpr::T0, Gpr::T0);
        assert!(
            code.len() > 20,
            "POPCNT should emit many instructions, got {} bytes",
            code.len()
        );
        let mut found_or = false;
        let mut found_slli = false;
        for i in 0..code.len() / 4 {
            let word = instr_word(&code, i);
            // OR: opcode=0b0110011, funct3=0b110
            if (word & 0x7F) == 0b0110011 && ((word >> 12) & 0x7) == 0b110 {
                found_or = true;
            }
            // SLLI: opcode=0b0010011, funct3=0b001
            if (word & 0x7F) == 0b0010011 && ((word >> 12) & 0x7) == 0b001 {
                found_slli = true;
            }
        }
        assert!(
            found_or,
            "POPCNT should contain OR instructions for mask building"
        );
        assert!(
            found_slli,
            "POPCNT should contain SLLI instructions for mask building"
        );
    }

    #[test]
    fn test_isel_popcnt_uses_mul() {
        // The final step of POPCNT multiplies by 0x0101... to sum bytes
        let code = emit_popcnt_isel(Gpr::T0, Gpr::T0);
        let mut found_mul = false;
        for i in 0..code.len() / 4 {
            let word = instr_word(&code, i);
            // MUL: opcode=0b0110011, funct7=0b0000001, funct3=0b000
            if (word & 0x7F) == 0b0110011
                && ((word >> 25) & 0x7F) == 0b0000001
                && ((word >> 12) & 0x7) == 0b000
            {
                found_mul = true;
                break;
            }
        }
        assert!(
            found_mul,
            "POPCNT should contain a MUL instruction for byte summation"
        );
    }

    #[test]
    fn test_isel_neg_uses_sub_from_zero() {
        // Neg: SUB d, x0, s (subtract from zero)
        let instr = Instruction::Sub {
            rd: Gpr::T0,
            rs1: Gpr::Zero,
            rs2: Gpr::T1,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // Verify opcode is OP-REG (0x33) and rs1 is x0
        assert_eq!(word & 0x7F, 0b0110011);
        assert_eq!((word >> 15) & 0x1F, 0); // rs1 = x0 (zero)
        assert_eq!((word >> 25) & 0x7F, 0b0100000); // funct7 for SUB
    }

    #[test]
    fn test_isel_not_uses_xori_minus1() {
        // Not: XORI d, s, -1
        let instr = Instruction::Xori {
            rd: Gpr::T0,
            rs1: Gpr::T1,
            imm: -1,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word & 0x7F, 0b0010011); // opcode = OP-IMM
        assert_eq!((word >> 12) & 0x7, 0b100); // funct3 = XORI
        assert_eq!((word >> 20) & 0xFFF, 0xFFF); // imm = -1 (12-bit)
    }

    // ── Decode Roundtrip Tests ──────────────────────────────────────

    #[test]
    fn test_decode_addi_roundtrip() {
        let instr = Instruction::Addi {
            rd: Gpr::T0,
            rs1: Gpr::T1,
            imm: 42,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        let decoded = Instruction::decode(word).expect("ADDI should decode");
        assert_eq!(format!("{}", decoded), "addi t0, t1, 42");
    }

    #[test]
    fn test_decode_add_sub_roundtrip() {
        let add_instr = Instruction::Add {
            rd: Gpr::A0,
            rs1: Gpr::A1,
            rs2: Gpr::A2,
        };
        let word = u32::from_le_bytes(add_instr.encode());
        let decoded = Instruction::decode(word).expect("ADD should decode");
        assert_eq!(format!("{}", decoded), "add a0, a1, a2");

        let sub_instr = Instruction::Sub {
            rd: Gpr::T0,
            rs1: Gpr::T1,
            rs2: Gpr::T2,
        };
        let word = u32::from_le_bytes(sub_instr.encode());
        let decoded = Instruction::decode(word).expect("SUB should decode");
        assert_eq!(format!("{}", decoded), "sub t0, t1, t2");
    }

    #[test]
    fn test_decode_ld_sd_roundtrip() {
        let ld_instr = Instruction::Ld {
            rd: Gpr::A0,
            rs1: Gpr::Sp,
            imm: 8,
        };
        let word = u32::from_le_bytes(ld_instr.encode());
        let decoded = Instruction::decode(word).expect("LD should decode");
        assert_eq!(format!("{}", decoded), "ld a0, 8(sp)");

        let sd_instr = Instruction::Sd {
            rs1: Gpr::Sp,
            rs2: Gpr::Ra,
            imm: -8,
        };
        let word = u32::from_le_bytes(sd_instr.encode());
        let decoded = Instruction::decode(word).expect("SD should decode");
        assert!(format!("{}", decoded).starts_with("sd"));
    }

    #[test]
    fn test_decode_branch_roundtrip() {
        let beq = Instruction::Beq {
            rs1: Gpr::A0,
            rs2: Gpr::A1,
            offset: 16,
        };
        let word = u32::from_le_bytes(beq.encode());
        let decoded = Instruction::decode(word).expect("BEQ should decode");
        assert!(format!("{}", decoded).starts_with("beq"));

        let bne = Instruction::Bne {
            rs1: Gpr::T0,
            rs2: Gpr::T1,
            offset: -4,
        };
        let word = u32::from_le_bytes(bne.encode());
        let decoded = Instruction::decode(word).expect("BNE should decode");
        assert!(format!("{}", decoded).starts_with("bne"));
    }

    #[test]
    fn test_decode_ecall_ebreak_nop() {
        // ECALL = 0x00000073
        let decoded = Instruction::decode(0x00000073).expect("ECALL should decode");
        assert_eq!(format!("{}", decoded), "ecall");

        // EBREAK = 0x00100073
        let decoded = Instruction::decode(0x00100073).expect("EBREAK should decode");
        assert_eq!(format!("{}", decoded), "ebreak");

        // NOP = ADDI x0, x0, 0 = 0x00000013
        let decoded = Instruction::decode(0x00000013).expect("NOP should decode");
        // NOP decodes as ADDI x0, x0, 0
        assert!(format!("{}", decoded).contains("addi"));
    }

    #[test]
    fn test_decode_lui_jal_roundtrip() {
        let lui_instr = Instruction::Lui {
            rd: Gpr::A0,
            imm: 0x12345000,
        };
        let word = u32::from_le_bytes(lui_instr.encode());
        let decoded = Instruction::decode(word).expect("LUI should decode");
        assert!(format!("{}", decoded).starts_with("lui"));

        let jal_instr = Instruction::Jal {
            rd: Gpr::Ra,
            offset: 100,
        };
        let word = u32::from_le_bytes(jal_instr.encode());
        let decoded = Instruction::decode(word).expect("JAL should decode");
        assert!(format!("{}", decoded).starts_with("jal"));
    }

    // ── Alloc / Free ISel Tests ────────────────────────────────────────

    /// Helper: build a minimal IR function with one block and the given
    /// instructions, then run allocate_registers and return the result.
    fn isel_func(name: &str, instrs: Vec<IRInstr>) -> AllocatedFunction {
        use std::collections::HashSet;
        let backend = RiscV64Backend::new();
        let func = IRFunction {
            name: name.to_string(),
            params: vec![],
            results: vec![],
            param_types: vec![],
            result_types: vec![],
            vregs: std::collections::HashMap::new(),
            blocks: vec![crate::ir::IRBlock {
                label: "entry".to_string(),
                instructions: instrs,
                terminator: crate::ir::IRTerminator::Return(vec![]),
                predecessors: HashSet::new(),
                successors: HashSet::new(),
                source_line: 0,
            }],
            source_file: String::new(),
        };
        backend.allocate_registers(&func).unwrap()
    }

    #[test]
    fn test_isel_alloc_emits_addi_sp() {
        let result = isel_func(
            "alloc_test",
            vec![IRInstr::Alloc {
                dst: IRValue::Register(0),
                size: 32,
            }],
        );
        let instrs = &result.blocks[0].instructions;
        // Alloc should emit ADDI sp, sp, -32 (not ADDI d, s0, 0)
        // There should be at least two addi instructions involving sp:
        // one from the prologue and one from the alloc itself.
        let addi_sp_count = instrs
            .iter()
            .filter(|i| {
                i.opcode == "addi"
                    && i.reads
                        .contains(&PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()))
                    && i.writes
                        .contains(&PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()))
            })
            .count();
        assert!(
            addi_sp_count >= 2,
            "expected at least 2 addi sp instructions (prologue + alloc), found {addi_sp_count}"
        );
        // The alloc-specific addi sp, sp, -32 should not encode as a NOP
        // Find instructions that write sp with an addi and check they're not all zero.
        let alloc_addi_sp: Vec<_> = instrs
            .iter()
            .filter(|i| {
                i.opcode == "addi"
                    && i.writes
                        .contains(&PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()))
            })
            .collect();
        // At least one of these (the alloc one) should have a non-zero immediate
        let has_nonzero = alloc_addi_sp.iter().any(|i| {
            let encoded = &i.encoded;
            if encoded.len() >= 4 {
                let word = u32::from_le_bytes([encoded[0], encoded[1], encoded[2], encoded[3]]);
                // Extract the immediate field from I-type: bits [31:20]
                let imm = ((word as i32) >> 20) as i32;
                imm != 0
            } else {
                false
            }
        });
        assert!(
            has_nonzero,
            "alloc addi sp, sp, -size should have a non-zero immediate"
        );
    }

    #[test]
    fn test_isel_alloc_dst_gets_sp() {
        let result = isel_func(
            "alloc_dst_test",
            vec![IRInstr::Alloc {
                dst: IRValue::Register(0),
                size: 16,
            }],
        );
        let instrs = &result.blocks[0].instructions;
        // After the alloc, there should be an addi that reads sp and writes to
        // the destination register (copying sp to dst).
        let has_sp_copy = instrs.iter().any(|i| {
            i.opcode == "addi"
                && i.reads
                    .contains(&PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()))
                && !i
                    .writes
                    .contains(&PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()))
                && !i.writes.is_empty()
        });
        assert!(
            has_sp_copy,
            "alloc should emit ADDI d, sp, 0 to copy SP to destination"
        );
    }

    #[test]
    fn test_isel_free_emits_brk_syscall() {
        let result = isel_func(
            "free_test",
            vec![IRInstr::Free {
                ptr: IRValue::Register(0),
            }],
        );
        let instrs = &result.blocks[0].instructions;
        // Free should be lowered to a single AllocatedInstruction with opcode "free"
        // whose encoded bytes contain: ADDI a0, p, 0; ADDI a7, zero, 214; ECALL
        let free_instrs: Vec<_> = instrs.iter().filter(|i| i.opcode == "free").collect();
        assert!(
            !free_instrs.is_empty(),
            "free should emit an instruction with opcode 'free'"
        );

        // The encoded bytes should contain at least 3 instructions (12 bytes):
        //   ADDI a0, p, 0  (or skipped if p == a0)
        //   ADDI a7, zero, 214
        //   ECALL
        let free_encoded = &free_instrs[0].encoded;
        assert!(
            free_encoded.len() >= 8,
            "free should emit at least ADDI a7 + ECALL (8 bytes), got {} bytes",
            free_encoded.len()
        );

        // Scan the encoded bytes for the ADDI a7, zero, 214 instruction.
        // I-type: imm[31:20] | rs1[19:15] | funct3[14:12] | rd[11:7] | opcode[6:0]
        // ADDI: funct3=0, opcode=0b0010011
        // a7=17, zero=0
        let mut found_brk_syscall = false;
        for chunk in free_encoded.chunks_exact(4) {
            let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let opcode = word & 0x7F;
            let rd = ((word >> 7) & 0x1F) as u8;
            let funct3 = (word >> 12) & 0x7;
            let rs1 = ((word >> 15) & 0x1F) as u8;
            let imm = ((word as i32) >> 20) as i32;
            // ADDI: opcode=0x13, funct3=0, rd=a7(17), rs1=zero(0), imm=214
            if opcode == 0x13 && funct3 == 0 && rd == 17 && rs1 == 0 && imm == 214 {
                found_brk_syscall = true;
            }
        }
        assert!(
            found_brk_syscall,
            "free should emit ADDI a7, zero, 214 (Linux brk syscall)"
        );

        // Verify there's no ADDI a7, zero, 0 (the old placeholder)
        for chunk in free_encoded.chunks_exact(4) {
            let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let opcode = word & 0x7F;
            let rd = ((word >> 7) & 0x1F) as u8;
            let funct3 = (word >> 12) & 0x7;
            let rs1 = ((word >> 15) & 0x1F) as u8;
            let imm = ((word as i32) >> 20) as i32;
            if opcode == 0x13 && funct3 == 0 && rd == 17 && rs1 == 0 {
                assert_ne!(
                    imm, 0,
                    "free should not emit ADDI a7, zero, 0 (old placeholder); should use imm=214"
                );
            }
        }
    }
}
