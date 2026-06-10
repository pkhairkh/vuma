//! # LoongArch 64-bit Backend
//!
//! Implements the `Backend` trait for the LoongArch 64-bit target (LP64 ABI).
//! This module provides:
//!
//! - `Gpr` — General-purpose register enum (r0–r31)
//! - `Fpr` — Floating-point register enum (f0–f31)
//! - `Instruction` — LoongArch64 instruction enum with correct encoding
//! - Encoding helpers for all 9 instruction formats (2R, 3R, 4R, 2RI8, 2RI12,
//!   2RI14, 2RI16, 1RI21, I26)
//! - `LoongArch64Backend` — `Backend` implementation that lowers IR to LoongArch64
//!   machine code and emits ELF64 binaries
//!
//! ## LoongArch64 Register Convention (LP64 ABI)
//!
//! | Register(s) | ABI Name | Role                              |
//! |-------------|----------|-----------------------------------|
//! | r0          | zero     | Hardwired zero                    |
//! | r1          | ra       | Return address                    |
//! | r2          | tp       | Thread pointer                    |
//! | r3          | sp       | Stack pointer                     |
//! | r4–r11      | a0–a7    | Argument / return registers       |
//! | r12–r20     | t0–t8    | Caller-saved temporaries          |
//! | r21         | —        | Reserved                          |
//! | r22         | fp       | Frame pointer (callee-saved)      |
//! | r23–r31     | s0–s8    | Callee-saved                      |
//!
//! ## LoongArch64 FP Register Convention (LP64 ABI)
//!
//! | Register(s) | ABI Name | Role                              |
//! |-------------|----------|-----------------------------------|
//! | f0–f7       | fa0–fa7  | FP argument / return registers    |
//! | f8–f23      | ft0–ft15 | Caller-saved temporaries          |
//! | f24–f31     | fs0–fs7  | Callee-saved                      |
//!
//! ## Instruction Formats
//!
//! All instructions are 32 bits, little-endian, with no branch delay slots.
//! Nine formats: 2R, 3R, 4R, 2RI8, 2RI12, 2RI14, 2RI16, 1RI21, I26.
//!
//! ## References
//!
//! - LoongArch Reference Manual, Volume 1: Basic Architecture
//! - <https://loongson.github.io/LoongArch-Documentation/>

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, LoongArch64TargetInfo, PhysicalReg, RegClass, TargetInfo,
};
use crate::ir::{BinOpKind, CmpKind, IRFunction, IRInstr, IRValue, UnaryOpKind};
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// LoongArch64 general-purpose registers (r0–r31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Gpr {
    R0 = 0,
    Ra = 1,
    Tp = 2,
    Sp = 3,
    A0 = 4,
    A1 = 5,
    A2 = 6,
    A3 = 7,
    A4 = 8,
    A5 = 9,
    A6 = 10,
    A7 = 11,
    T0 = 12,
    T1 = 13,
    T2 = 14,
    T3 = 15,
    T4 = 16,
    T5 = 17,
    T6 = 18,
    T7 = 19,
    T8 = 20,
    R21 = 21,
    Fp = 22,
    S0 = 23,
    S1 = 24,
    S2 = 25,
    S3 = 26,
    S4 = 27,
    S5 = 28,
    S6 = 29,
    S7 = 30,
    S8 = 31,
}

impl Gpr {
    /// Returns the 5-bit encoding index for this register.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns `true` if this register is available for register allocation.
    ///
    /// R0 (zero), Ra, Tp, and Sp are reserved.
    pub fn is_allocatable(&self) -> bool {
        !matches!(self, Gpr::R0 | Gpr::Ra | Gpr::Tp | Gpr::Sp)
    }

    /// Returns `true` if this register is callee-saved (fp, s0–s8).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Gpr::Fp | Gpr::S0 | Gpr::S1 | Gpr::S2 | Gpr::S3 | Gpr::S4
                | Gpr::S5 | Gpr::S6 | Gpr::S7 | Gpr::S8
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
            Gpr::R0 => "$r0",
            Gpr::Ra => "$ra",
            Gpr::Tp => "$tp",
            Gpr::Sp => "$sp",
            Gpr::A0 => "$a0",
            Gpr::A1 => "$a1",
            Gpr::A2 => "$a2",
            Gpr::A3 => "$a3",
            Gpr::A4 => "$a4",
            Gpr::A5 => "$a5",
            Gpr::A6 => "$a6",
            Gpr::A7 => "$a7",
            Gpr::T0 => "$t0",
            Gpr::T1 => "$t1",
            Gpr::T2 => "$t2",
            Gpr::T3 => "$t3",
            Gpr::T4 => "$t4",
            Gpr::T5 => "$t5",
            Gpr::T6 => "$t6",
            Gpr::T7 => "$t7",
            Gpr::T8 => "$t8",
            Gpr::R21 => "$r21",
            Gpr::Fp => "$fp",
            Gpr::S0 => "$s0",
            Gpr::S1 => "$s1",
            Gpr::S2 => "$s2",
            Gpr::S3 => "$s3",
            Gpr::S4 => "$s4",
            Gpr::S5 => "$s5",
            Gpr::S6 => "$s6",
            Gpr::S7 => "$s7",
            Gpr::S8 => "$s8",
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

impl fmt::Display for Gpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Floating-Point Registers
// ===========================================================================

/// LoongArch64 floating-point registers (f0–f31).
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

    /// Returns `true` if this register is callee-saved (fs0–fs7, f24–f31).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Fpr::F24 | Fpr::F25 | Fpr::F26 | Fpr::F27 | Fpr::F28 | Fpr::F29 | Fpr::F30
                | Fpr::F31
        )
    }

    /// Returns `true` if this register is an FP argument register (fa0–fa7, f0–f7).
    pub fn is_arg_reg(&self) -> bool {
        matches!(
            self,
            Fpr::F0 | Fpr::F1 | Fpr::F2 | Fpr::F3 | Fpr::F4 | Fpr::F5 | Fpr::F6 | Fpr::F7
        )
    }

    /// Returns `true` if this register is available for register allocation.
    pub fn is_allocatable(&self) -> bool {
        true
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Fpr::F0 => "$fa0",
            Fpr::F1 => "$fa1",
            Fpr::F2 => "$fa2",
            Fpr::F3 => "$fa3",
            Fpr::F4 => "$fa4",
            Fpr::F5 => "$fa5",
            Fpr::F6 => "$fa6",
            Fpr::F7 => "$fa7",
            Fpr::F8 => "$ft0",
            Fpr::F9 => "$ft1",
            Fpr::F10 => "$ft2",
            Fpr::F11 => "$ft3",
            Fpr::F12 => "$ft4",
            Fpr::F13 => "$ft5",
            Fpr::F14 => "$ft6",
            Fpr::F15 => "$ft7",
            Fpr::F16 => "$ft8",
            Fpr::F17 => "$ft9",
            Fpr::F18 => "$ft10",
            Fpr::F19 => "$ft11",
            Fpr::F20 => "$ft12",
            Fpr::F21 => "$ft13",
            Fpr::F22 => "$ft14",
            Fpr::F23 => "$ft15",
            Fpr::F24 => "$fs0",
            Fpr::F25 => "$fs1",
            Fpr::F26 => "$fs2",
            Fpr::F27 => "$fs3",
            Fpr::F28 => "$fs4",
            Fpr::F29 => "$fs5",
            Fpr::F30 => "$fs6",
            Fpr::F31 => "$fs7",
        }
    }

    /// Returns the Fpr for a given FP argument index (0–7). Returns `None` for
    /// indices >= 8.
    pub fn arg_register(index: usize) -> Option<Fpr> {
        match index {
            0 => Some(Fpr::F0),
            1 => Some(Fpr::F1),
            2 => Some(Fpr::F2),
            3 => Some(Fpr::F3),
            4 => Some(Fpr::F4),
            5 => Some(Fpr::F5),
            6 => Some(Fpr::F6),
            7 => Some(Fpr::F7),
            _ => None,
        }
    }
}

impl fmt::Display for Fpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Instruction Encoding Helpers
// ===========================================================================

/// Encode a 2R format instruction.
///
/// Format: `opcode[31:10] | rj[9:5] | rd[4:0]`
fn encode_2r(opcode: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0x3FF_FFFF) << 10) | ((rj & 0x1F) << 5) | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 3R format instruction.
///
/// Format: `opcode[31:15] | rk[14:10] | rj[9:5] | rd[4:0]`
fn encode_3r(opcode: u32, rk: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0x1FFFF) << 15) | ((rk & 0x1F) << 10) | ((rj & 0x1F) << 5)
        | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 4R format instruction.
///
/// Format: `opcode[31:20] | ra[19:15] | rk[14:10] | rj[9:5] | rd[4:0]`
#[allow(dead_code)]
fn encode_4r(opcode: u32, ra: u32, rk: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0xFFF) << 20)
        | ((ra & 0x1F) << 15)
        | ((rk & 0x1F) << 10)
        | ((rj & 0x1F) << 5)
        | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 2RI8 format instruction.
///
/// Format: `opcode[31:22] | I8[21:14] | rj[9:5] | rd[4:0]`
fn encode_2ri8(opcode: u32, imm8: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0x3FF) << 22) | ((imm8 & 0xFF) << 14) | ((rj & 0x1F) << 5)
        | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 2RI12 format instruction.
///
/// Format: `opcode[31:22] | I12[21:10] | rj[9:5] | rd[4:0]`
fn encode_2ri12(opcode: u32, imm12: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0x3FF) << 22) | ((imm12 & 0xFFF) << 10) | ((rj & 0x1F) << 5)
        | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 2RI14 format instruction.
///
/// Format: `opcode[31:24] | I14[23:10] | rj[9:5] | rd[4:0]`
fn encode_2ri14(opcode: u32, imm14: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0xFF) << 24) | ((imm14 & 0x3FFF) << 10) | ((rj & 0x1F) << 5)
        | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 2RI16 format instruction.
///
/// Format: `opcode[31:26] | I16[25:10] | rj[9:5] | rd[4:0]`
fn encode_2ri16(opcode: u32, imm16: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26) | ((imm16 & 0xFFFF) << 10) | ((rj & 0x1F) << 5)
        | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 1RI21 format instruction.
///
/// Format: `opcode[31:26] | I21[25:5] | rj[9:5]`
///
/// The 21-bit immediate is split: bits[20:16] in bits[25:21], bits[15:0] in
/// bits[20:5].
fn encode_1ri21(opcode: u32, imm21: u32, rj: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((imm21 & 0x1F) << 21)
        | ((imm21 >> 5) & 0xFFFF) << 5
        | (rj & 0x1F);
    word.to_le_bytes()
}

/// Encode an I26 format instruction.
///
/// Format: `opcode[31:26] | I26[25:0]`
///
/// The 26-bit immediate is split: bits[25:16] in bits[25:16], bits[15:0] in
/// bits[9:0] with bits[15:10] in bits[25:20] and bits[9:0] in bits[9:0].
fn encode_i26(opcode: u32, imm26: u32) -> [u8; 4] {
    let hi10 = (imm26 >> 16) & 0x3FF;
    let lo16 = imm26 & 0xFFFF;
    let word = ((opcode & 0x3F) << 26) | (hi10 << 16) | lo16;
    word.to_le_bytes()
}

// ===========================================================================
// 3R-format Opcodes (bits[31:15])
// ===========================================================================

const OPC_ADD_W: u32 = 0x0020;
const OPC_ADD_D: u32 = 0x0021;
const OPC_SUB_W: u32 = 0x0030;
const OPC_SUB_D: u32 = 0x0031;
const OPC_SLT: u32 = 0x0040;
const OPC_SLTU: u32 = 0x0041;
const OPC_MUL_W: u32 = 0x0098;
const OPC_MUL_D: u32 = 0x0099;
const OPC_DIV_W: u32 = 0x009E;
const OPC_MOD_W: u32 = 0x009F;
const OPC_DIV_D: u32 = 0x00A0;
const OPC_MOD_D: u32 = 0x00A1;
const OPC_AND: u32 = 0x0080;
const OPC_OR: u32 = 0x0081;
const OPC_XOR: u32 = 0x0082;
const OPC_NOR: u32 = 0x0083;
const OPC_ANDN: u32 = 0x0084;
const OPC_ORN: u32 = 0x0085;
const OPC_SLL_W: u32 = 0x0089;
const OPC_SRL_W: u32 = 0x008A;
const OPC_SRA_W: u32 = 0x008B;
const OPC_SLL_D: u32 = 0x008C;
const OPC_SRL_D: u32 = 0x008D;
const OPC_SRA_D: u32 = 0x008E;
const OPC_ROTR_W: u32 = 0x008F;
const OPC_ROTR_D: u32 = 0x0090;

// ===========================================================================
// 2RI12-format Opcodes (bits[31:22])
// ===========================================================================

const OPC_ADDI_W: u32 = 0x00A;
const OPC_ADDI_D: u32 = 0x00B;
const OPC_SLTI: u32 = 0x008;
const OPC_SLTUI: u32 = 0x009;
const OPC_ANDI: u32 = 0x00D;
const OPC_ORI: u32 = 0x00E;
const OPC_XORI: u32 = 0x00F;
const OPC_LD_B: u32 = 0x0A0;
const OPC_LD_H: u32 = 0x0A1;
const OPC_LD_W: u32 = 0x0A2;
const OPC_LD_D: u32 = 0x0A3;
const OPC_LD_BU: u32 = 0x0A4;
const OPC_LD_HU: u32 = 0x0A5;
const OPC_LD_WU: u32 = 0x0A6;
const OPC_ST_B: u32 = 0x0A7;
const OPC_ST_H: u32 = 0x0A8;
const OPC_ST_W: u32 = 0x0A9;
const OPC_ST_D: u32 = 0x0AA;

// ===========================================================================
// 2RI16-format Opcodes (bits[31:26])
// ===========================================================================

const OPC_BEQ: u32 = 0x16;
const OPC_BNE: u32 = 0x17;
const OPC_BLT: u32 = 0x18;
const OPC_BGE: u32 = 0x19;
const OPC_BLTU: u32 = 0x1A;
const OPC_BGEU: u32 = 0x1B;
const OPC_JIRL: u32 = 0x13;
const OPC_LU12I_W: u32 = 0x05;
const OPC_LU32I_D: u32 = 0x06;
const OPC_LU52I_D: u32 = 0x03;

// ===========================================================================
// I26-format Opcodes (bits[31:26])
// ===========================================================================

const OPC_B: u32 = 0x14;
const OPC_BL: u32 = 0x15;

// ===========================================================================
// 2RI14-format Opcodes (bits[31:24])
// ===========================================================================

const OPC_LL_W: u32 = 0x20;
const OPC_SC_W: u32 = 0x21;
const OPC_LL_D: u32 = 0x22;
const OPC_SC_D: u32 = 0x23;

// ===========================================================================
// 1RI21-format Opcodes (bits[31:26])
// ===========================================================================

const OPC_BEQZ: u32 = 0x1C;
const OPC_BNEZ: u32 = 0x1D;
const OPC_PCADDU12I: u32 = 0x0E;
const OPC_PCADDU18I: u32 = 0x0F;

// ===========================================================================
// 2R-format Opcodes (bits[31:10])
// ===========================================================================

const OPC_EXT_W_H: u32 = 0x000005A;
const OPC_EXT_W_B: u32 = 0x000005B;

#[allow(dead_code)]
const OPC_REVB_2H: u32 = 0x0000060;
#[allow(dead_code)]
const OPC_REVB_4H: u32 = 0x0000061;
#[allow(dead_code)]
const OPC_REVB_2W: u32 = 0x0000062;
#[allow(dead_code)]
const OPC_BITREV_4B: u32 = 0x0000064;
#[allow(dead_code)]
const OPC_BITREV_8B: u32 = 0x0000065;
#[allow(dead_code)]
const OPC_CPBYTE: u32 = 0x0000057;

// 2R: special opcodes for move/clover
#[allow(dead_code)]
const OPC_MOVCF2GR: u32 = 0x0000055;
#[allow(dead_code)]
const OPC_MOVGR2CF: u32 = 0x0000054;

// ===========================================================================
// 4R-format Opcodes (bits[31:20])
// ===========================================================================

#[allow(dead_code)]
const OPC_BYTEREV_D: u32 = 0x009;
#[allow(dead_code)]
const OPC_BYTEREV_W: u32 = 0x00A;
#[allow(dead_code)]
const OPC_BYTEREV_H: u32 = 0x00B;

// ===========================================================================
// 2RI8-format Opcodes (bits[31:22])
// ===========================================================================

const OPC_SLLI_W: u32 = 0x008;
const OPC_SRLI_W: u32 = 0x009;
const OPC_SRAI_W: u32 = 0x00A;
const OPC_SLLI_D: u32 = 0x00B;
const OPC_SRLI_D: u32 = 0x00C;
const OPC_SRAI_D: u32 = 0x00D;
#[allow(dead_code)]
const OPC_ROTRI_W: u32 = 0x00E;
#[allow(dead_code)]
const OPC_ROTRI_D: u32 = 0x00F;

// ===========================================================================
// Instruction Enum
// ===========================================================================

/// LoongArch64 instruction representations for code generation.
///
/// Covers key arithmetic, logical, shift, load/store, branch, move, and FP
/// instructions. Each variant captures the operands needed for encoding and
/// disassembly. The `encode()` method produces a 4-byte little-endian machine
/// code word.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Instruction {
    // ── Arithmetic (3R) ──────────────────────────────────────────────
    /// Add Word: `add.w rd, rj, rk`
    AddW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Add Doubleword: `add.d rd, rj, rk`
    AddD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Subtract Word: `sub.w rd, rj, rk`
    SubW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Subtract Doubleword: `sub.d rd, rj, rk`
    SubD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Set Less Than (signed): `slt rd, rj, rk`
    Slt { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Set Less Than (unsigned): `sltu rd, rj, rk`
    Sltu { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Multiply Word: `mul.w rd, rj, rk`
    MulW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Multiply Doubleword: `mul.d rd, rj, rk`
    MulD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Divide Word (signed): `div.w rd, rj, rk`
    DivW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Modulo Word (signed): `mod.w rd, rj, rk`
    ModW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Divide Doubleword (signed): `div.d rd, rj, rk`
    DivD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Modulo Doubleword (signed): `mod.d rd, rj, rk`
    ModD { rd: Gpr, rj: Gpr, rk: Gpr },

    // ── Logical (3R) ────────────────────────────────────────────────
    /// AND: `and rd, rj, rk`
    And { rd: Gpr, rj: Gpr, rk: Gpr },
    /// OR: `or rd, rj, rk`
    Or { rd: Gpr, rj: Gpr, rk: Gpr },
    /// XOR: `xor rd, rj, rk`
    Xor { rd: Gpr, rj: Gpr, rk: Gpr },
    /// NOR: `nor rd, rj, rk`
    Nor { rd: Gpr, rj: Gpr, rk: Gpr },
    /// AND NOT: `andn rd, rj, rk`
    Andn { rd: Gpr, rj: Gpr, rk: Gpr },
    /// OR NOT: `orn rd, rj, rk`
    Orn { rd: Gpr, rj: Gpr, rk: Gpr },

    // ── Shift (3R) ──────────────────────────────────────────────────
    /// Shift Left Logical Word: `sll.w rd, rj, rk`
    SllW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Shift Right Logical Word: `srl.w rd, rj, rk`
    SrlW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Shift Right Arithmetic Word: `sra.w rd, rj, rk`
    SraW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Shift Left Logical Doubleword: `sll.d rd, rj, rk`
    SllD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Shift Right Logical Doubleword: `srl.d rd, rj, rk`
    SrlD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Shift Right Arithmetic Doubleword: `sra.d rd, rj, rk`
    SraD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Rotate Right Word: `rotr.w rd, rj, rk`
    RotrW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Rotate Right Doubleword: `rotr.d rd, rj, rk`
    RotrD { rd: Gpr, rj: Gpr, rk: Gpr },

    // ── Shift Immediate (2RI8) ──────────────────────────────────────
    /// Shift Left Logical Immediate Word: `slli.w rd, rj, ui8`
    SlliW { rd: Gpr, rj: Gpr, imm8: u32 },
    /// Shift Right Logical Immediate Word: `srli.w rd, rj, ui8`
    SrliW { rd: Gpr, rj: Gpr, imm8: u32 },
    /// Shift Right Arithmetic Immediate Word: `srai.w rd, rj, ui8`
    SraiW { rd: Gpr, rj: Gpr, imm8: u32 },
    /// Shift Left Logical Immediate Doubleword: `slli.d rd, rj, ui8`
    SlliD { rd: Gpr, rj: Gpr, imm8: u32 },
    /// Shift Right Logical Immediate Doubleword: `srli.d rd, rj, ui8`
    SrliD { rd: Gpr, rj: Gpr, imm8: u32 },
    /// Shift Right Arithmetic Immediate Doubleword: `srai.d rd, rj, ui8`
    SraiD { rd: Gpr, rj: Gpr, imm8: u32 },

    // ── Immediate Arithmetic (2RI12) ────────────────────────────────
    /// Add Immediate Word: `addi.w rd, rj, si12`
    AddiW { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Add Immediate Doubleword: `addi.d rd, rj, si12`
    AddiD { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Set Less Than Immediate (signed): `slti rd, rj, si12`
    Slti { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Set Less Than Immediate (unsigned): `sltui rd, rj, si12`
    Sltui { rd: Gpr, rj: Gpr, imm12: i32 },
    /// AND Immediate: `andi rd, rj, ui12`
    Andi { rd: Gpr, rj: Gpr, imm12: u32 },
    /// OR Immediate: `ori rd, rj, ui12`
    Ori { rd: Gpr, rj: Gpr, imm12: u32 },
    /// XOR Immediate: `xori rd, rj, ui12`
    Xori { rd: Gpr, rj: Gpr, imm12: u32 },

    // ── Load (2RI12) ────────────────────────────────────────────────
    /// Load Byte (sign-extended): `ld.b rd, rj, si12`
    LdB { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Load Halfword (sign-extended): `ld.h rd, rj, si12`
    LdH { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Load Word (sign-extended): `ld.w rd, rj, si12`
    LdW { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Load Doubleword: `ld.d rd, rj, si12`
    LdD { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Load Byte (zero-extended): `ld.bu rd, rj, si12`
    LdBu { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Load Halfword (zero-extended): `ld.hu rd, rj, si12`
    LdHu { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Load Word (zero-extended): `ld.wu rd, rj, si12`
    LdWu { rd: Gpr, rj: Gpr, imm12: i32 },

    // ── Store (2RI12) ───────────────────────────────────────────────
    /// Store Byte: `st.b rd, rj, si12`
    StB { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Store Halfword: `st.h rd, rj, si12`
    StH { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Store Word: `st.w rd, rj, si12`
    StW { rd: Gpr, rj: Gpr, imm12: i32 },
    /// Store Doubleword: `st.d rd, rj, si12`
    StD { rd: Gpr, rj: Gpr, imm12: i32 },

    // ── Branch (2RI16) ──────────────────────────────────────────────
    /// Branch if Equal: `beq rj, rd, offs16`
    Beq { rj: Gpr, rd: Gpr, offs16: i32 },
    /// Branch if Not Equal: `bne rj, rd, offs16`
    Bne { rj: Gpr, rd: Gpr, offs16: i32 },
    /// Branch if Less Than (signed): `blt rj, rd, offs16`
    Blt { rj: Gpr, rd: Gpr, offs16: i32 },
    /// Branch if Greater or Equal (signed): `bge rj, rd, offs16`
    Bge { rj: Gpr, rd: Gpr, offs16: i32 },
    /// Branch if Less Than (unsigned): `bltu rj, rd, offs16`
    Bltu { rj: Gpr, rd: Gpr, offs16: i32 },
    /// Branch if Greater or Equal (unsigned): `bgeu rj, rd, offs16`
    Bgeu { rj: Gpr, rd: Gpr, offs16: i32 },
    /// Jump Indirect and Return Link: `jirl rd, rj, offs16`
    Jirl { rd: Gpr, rj: Gpr, offs16: i32 },

    // ── Unconditional Branch (I26) ──────────────────────────────────
    /// Branch: `b offs26`
    B { offs26: i32 },
    /// Branch and Link: `bl offs26`
    Bl { offs26: i32 },

    // ── Branch on Zero/NonZero (1RI21) ──────────────────────────────
    /// Branch if Equal Zero: `beqz rj, offs21`
    Beqz { rj: Gpr, offs21: i32 },
    /// Branch if Not Equal Zero: `bnez rj, offs21`
    Bnez { rj: Gpr, offs21: i32 },

    // ── Upper Immediate (2RI16 / 1RI21) ─────────────────────────────
    /// Load Upper 12-bit Immediate Word: `lu12i.w rd, si20`
    Lu12iW { rd: Gpr, imm20: i32 },
    /// Load Upper 32-bit Immediate Doubleword (high): `lu32i.d rd, si20`
    Lu32iD { rd: Gpr, imm20: i32 },
    /// Load Upper 52-bit Immediate Doubleword: `lu52i.d rd, si12`
    Lu52iD { rd: Gpr, rj: Gpr, imm12: i32 },
    /// PC-add Upper: `pcaddu12i rd, si20`
    Pcaddu12i { rd: Gpr, imm20: i32 },
    /// PC-add Upper (18-bit): `pcaddu18i rd, si20`
    Pcaddu18i { rd: Gpr, imm20: i32 },

    // ── Atomic (2RI14) ──────────────────────────────────────────────
    /// Load-Linked Word: `ll.w rd, rj, si14`
    LlW { rd: Gpr, rj: Gpr, imm14: i32 },
    /// Store-Conditional Word: `sc.w rd, rj, si14`
    ScW { rd: Gpr, rj: Gpr, imm14: i32 },
    /// Load-Linked Doubleword: `ll.d rd, rj, si14`
    LlD { rd: Gpr, rj: Gpr, imm14: i32 },
    /// Store-Conditional Doubleword: `sc.d rd, rj, si14`
    ScD { rd: Gpr, rj: Gpr, imm14: i32 },

    // ── Move (2R) ───────────────────────────────────────────────────
    /// Sign-extend Halfword to Word: `ext.w.h rd, rj`
    ExtWH { rd: Gpr, rj: Gpr },
    /// Sign-extend Byte to Word: `ext.w.b rd, rj`
    ExtWB { rd: Gpr, rj: Gpr },

    // ── FP Load/Store (2RI12) ───────────────────────────────────────
    /// Load Float Word to FP: `fld.s fd, rj, si12`
    FldS { fd: Fpr, rj: Gpr, imm12: i32 },
    /// Load Double to FP: `fld.d fd, rj, si12`
    FldD { fd: Fpr, rj: Gpr, imm12: i32 },
    /// Store Float Word: `fst.s fd, rj, si12`
    FstS { fd: Fpr, rj: Gpr, imm12: i32 },
    /// Store Double: `fst.d fd, rj, si12`
    FstD { fd: Fpr, rj: Gpr, imm12: i32 },

    // ── FP Move (2R) ────────────────────────────────────────────────
    /// Move FP to GR Doubleword: `movfr2gr.d rd, fj`
    FmovGr2FprD { rd: Gpr, fj: Fpr },
    /// Move GR to FP Doubleword: `movgr2fr.d fd, rj`
    FmovFpr2GrD { fd: Fpr, rj: Gpr },

    // ── FP Arithmetic (3R) ──────────────────────────────────────────
    /// FP Add Double: `fadd.d fd, fj, fk`
    FaddD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Subtract Double: `fsub.d fd, fj, fk`
    FsubD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Multiply Double: `fmul.d fd, fj, fk`
    FmulD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Divide Double: `fdiv.d fd, fj, fk`
    FdivD { fd: Fpr, fj: Fpr, fk: Fpr },

    // ── No-op / Break ───────────────────────────────────────────────
    /// No-operation (pseudo: `and $r0, $r0, $r0`)
    Nop,
    /// System Call: `syscall 0x0`
    Syscall,
    /// Break: `break 0x0`
    Break,
}

impl Instruction {
    /// Encode this instruction into a 4-byte little-endian machine code word.
    ///
    /// Encoding follows the LoongArch ISA Specification.
    pub fn encode(&self) -> [u8; 4] {
        match self {
            // ── Arithmetic (3R) ────────────────────────────────────
            Instruction::AddW { rd, rj, rk } => {
                encode_3r(OPC_ADD_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::AddD { rd, rj, rk } => {
                encode_3r(OPC_ADD_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::SubW { rd, rj, rk } => {
                encode_3r(OPC_SUB_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::SubD { rd, rj, rk } => {
                encode_3r(OPC_SUB_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Slt { rd, rj, rk } => {
                encode_3r(OPC_SLT, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Sltu { rd, rj, rk } => {
                encode_3r(OPC_SLTU, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::MulW { rd, rj, rk } => {
                encode_3r(OPC_MUL_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::MulD { rd, rj, rk } => {
                encode_3r(OPC_MUL_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::DivW { rd, rj, rk } => {
                encode_3r(OPC_DIV_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::ModW { rd, rj, rk } => {
                encode_3r(OPC_MOD_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::DivD { rd, rj, rk } => {
                encode_3r(OPC_DIV_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::ModD { rd, rj, rk } => {
                encode_3r(OPC_MOD_D, rk.encoding(), rj.encoding(), rd.encoding())
            }

            // ── Logical (3R) ──────────────────────────────────────
            Instruction::And { rd, rj, rk } => {
                encode_3r(OPC_AND, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Or { rd, rj, rk } => {
                encode_3r(OPC_OR, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Xor { rd, rj, rk } => {
                encode_3r(OPC_XOR, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Nor { rd, rj, rk } => {
                encode_3r(OPC_NOR, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Andn { rd, rj, rk } => {
                encode_3r(OPC_ANDN, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Orn { rd, rj, rk } => {
                encode_3r(OPC_ORN, rk.encoding(), rj.encoding(), rd.encoding())
            }

            // ── Shift (3R) ────────────────────────────────────────
            Instruction::SllW { rd, rj, rk } => {
                encode_3r(OPC_SLL_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::SrlW { rd, rj, rk } => {
                encode_3r(OPC_SRL_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::SraW { rd, rj, rk } => {
                encode_3r(OPC_SRA_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::SllD { rd, rj, rk } => {
                encode_3r(OPC_SLL_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::SrlD { rd, rj, rk } => {
                encode_3r(OPC_SRL_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::SraD { rd, rj, rk } => {
                encode_3r(OPC_SRA_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::RotrW { rd, rj, rk } => {
                encode_3r(OPC_ROTR_W, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::RotrD { rd, rj, rk } => {
                encode_3r(OPC_ROTR_D, rk.encoding(), rj.encoding(), rd.encoding())
            }

            // ── Shift Immediate (2RI8) ────────────────────────────
            Instruction::SlliW { rd, rj, imm8 } => {
                encode_2ri8(OPC_SLLI_W, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SrliW { rd, rj, imm8 } => {
                encode_2ri8(OPC_SRLI_W, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SraiW { rd, rj, imm8 } => {
                encode_2ri8(OPC_SRAI_W, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SlliD { rd, rj, imm8 } => {
                encode_2ri8(OPC_SLLI_D, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SrliD { rd, rj, imm8 } => {
                encode_2ri8(OPC_SRLI_D, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SraiD { rd, rj, imm8 } => {
                encode_2ri8(OPC_SRAI_D, *imm8, rj.encoding(), rd.encoding())
            }

            // ── Immediate Arithmetic (2RI12) ──────────────────────
            Instruction::AddiW { rd, rj, imm12 } => {
                encode_2ri12(OPC_ADDI_W, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::AddiD { rd, rj, imm12 } => {
                encode_2ri12(OPC_ADDI_D, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Slti { rd, rj, imm12 } => {
                encode_2ri12(OPC_SLTI, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Sltui { rd, rj, imm12 } => {
                encode_2ri12(OPC_SLTUI, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Andi { rd, rj, imm12 } => {
                encode_2ri12(OPC_ANDI, *imm12 & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Ori { rd, rj, imm12 } => {
                encode_2ri12(OPC_ORI, *imm12 & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Xori { rd, rj, imm12 } => {
                encode_2ri12(OPC_XORI, *imm12 & 0xFFF, rj.encoding(), rd.encoding())
            }

            // ── Load (2RI12) ──────────────────────────────────────
            Instruction::LdB { rd, rj, imm12 } => {
                encode_2ri12(OPC_LD_B, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::LdH { rd, rj, imm12 } => {
                encode_2ri12(OPC_LD_H, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::LdW { rd, rj, imm12 } => {
                encode_2ri12(OPC_LD_W, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::LdD { rd, rj, imm12 } => {
                encode_2ri12(OPC_LD_D, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::LdBu { rd, rj, imm12 } => {
                encode_2ri12(OPC_LD_BU, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::LdHu { rd, rj, imm12 } => {
                encode_2ri12(OPC_LD_HU, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::LdWu { rd, rj, imm12 } => {
                encode_2ri12(OPC_LD_WU, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }

            // ── Store (2RI12) ─────────────────────────────────────
            Instruction::StB { rd, rj, imm12 } => {
                encode_2ri12(OPC_ST_B, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::StH { rd, rj, imm12 } => {
                encode_2ri12(OPC_ST_H, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::StW { rd, rj, imm12 } => {
                encode_2ri12(OPC_ST_W, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::StD { rd, rj, imm12 } => {
                encode_2ri12(OPC_ST_D, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }

            // ── Branch (2RI16) ────────────────────────────────────
            Instruction::Beq { rj, rd, offs16 } => {
                encode_2ri16(OPC_BEQ, (*offs16 as u32) & 0xFFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Bne { rj, rd, offs16 } => {
                encode_2ri16(OPC_BNE, (*offs16 as u32) & 0xFFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Blt { rj, rd, offs16 } => {
                encode_2ri16(OPC_BLT, (*offs16 as u32) & 0xFFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Bge { rj, rd, offs16 } => {
                encode_2ri16(OPC_BGE, (*offs16 as u32) & 0xFFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Bltu { rj, rd, offs16 } => {
                encode_2ri16(OPC_BLTU, (*offs16 as u32) & 0xFFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Bgeu { rj, rd, offs16 } => {
                encode_2ri16(OPC_BGEU, (*offs16 as u32) & 0xFFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Jirl { rd, rj, offs16 } => {
                encode_2ri16(OPC_JIRL, (*offs16 as u32) & 0xFFFF, rj.encoding(), rd.encoding())
            }

            // ── Unconditional Branch (I26) ────────────────────────
            Instruction::B { offs26 } => {
                encode_i26(OPC_B, (*offs26 as u32) & 0x3FFFFFF)
            }
            Instruction::Bl { offs26 } => {
                encode_i26(OPC_BL, (*offs26 as u32) & 0x3FFFFFF)
            }

            // ── Branch on Zero/NonZero (1RI21) ────────────────────
            Instruction::Beqz { rj, offs21 } => {
                encode_1ri21(OPC_BEQZ, (*offs21 as u32) & 0x1FFFFF, rj.encoding())
            }
            Instruction::Bnez { rj, offs21 } => {
                encode_1ri21(OPC_BNEZ, (*offs21 as u32) & 0x1FFFFF, rj.encoding())
            }

            // ── Upper Immediate ────────────────────────────────────
            Instruction::Lu12iW { rd, imm20 } => {
                // 2RI16 format with rd and si20
                encode_2ri16(OPC_LU12I_W, (*imm20 as u32) & 0xFFFFF, Gpr::R0.encoding(), rd.encoding())
            }
            Instruction::Lu32iD { rd, imm20 } => {
                // 1RI21 format: opcode[31:26] | si20[25:6] | rd[4:0]
                // Actually a special encoding: opcode + si20 + rd (no rj)
                let word = ((OPC_LU32I_D & 0x3F) << 26)
                    | (((*imm20 as u32) & 0xFFFFF) << 5)
                    | (rd.encoding() & 0x1F);
                word.to_le_bytes()
            }
            Instruction::Lu52iD { rd, rj, imm12 } => {
                // 2RI12 format
                encode_2ri12(OPC_LU52I_D, (*imm12 as u32) & 0xFFF, rj.encoding(), rd.encoding())
            }
            Instruction::Pcaddu12i { rd, imm20 } => {
                encode_1ri21(OPC_PCADDU12I, (*imm20 as u32) & 0x1FFFFF, rd.encoding())
            }
            Instruction::Pcaddu18i { rd, imm20 } => {
                encode_1ri21(OPC_PCADDU18I, (*imm20 as u32) & 0x1FFFFF, rd.encoding())
            }

            // ── Atomic (2RI14) ────────────────────────────────────
            Instruction::LlW { rd, rj, imm14 } => {
                encode_2ri14(OPC_LL_W, (*imm14 as u32) & 0x3FFF, rj.encoding(), rd.encoding())
            }
            Instruction::ScW { rd, rj, imm14 } => {
                encode_2ri14(OPC_SC_W, (*imm14 as u32) & 0x3FFF, rj.encoding(), rd.encoding())
            }
            Instruction::LlD { rd, rj, imm14 } => {
                encode_2ri14(OPC_LL_D, (*imm14 as u32) & 0x3FFF, rj.encoding(), rd.encoding())
            }
            Instruction::ScD { rd, rj, imm14 } => {
                encode_2ri14(OPC_SC_D, (*imm14 as u32) & 0x3FFF, rj.encoding(), rd.encoding())
            }

            // ── Move (2R) ─────────────────────────────────────────
            Instruction::ExtWH { rd, rj } => {
                encode_2r(OPC_EXT_W_H, rj.encoding(), rd.encoding())
            }
            Instruction::ExtWB { rd, rj } => {
                encode_2r(OPC_EXT_W_B, rj.encoding(), rd.encoding())
            }

            // ── FP Load/Store (2RI12) ─────────────────────────────
            Instruction::FldS { fd, rj, imm12 } => {
                encode_2ri12(0x0AB, (*imm12 as u32) & 0xFFF, rj.encoding(), fd.encoding())
            }
            Instruction::FldD { fd, rj, imm12 } => {
                encode_2ri12(0x0AC, (*imm12 as u32) & 0xFFF, rj.encoding(), fd.encoding())
            }
            Instruction::FstS { fd, rj, imm12 } => {
                encode_2ri12(0x0AD, (*imm12 as u32) & 0xFFF, rj.encoding(), fd.encoding())
            }
            Instruction::FstD { fd, rj, imm12 } => {
                encode_2ri12(0x0AE, (*imm12 as u32) & 0xFFF, rj.encoding(), fd.encoding())
            }

            // ── FP Move (2R) ──────────────────────────────────────
            Instruction::FmovGr2FprD { rd, fj } => {
                encode_2r(0x0000052, fj.encoding(), rd.encoding())
            }
            Instruction::FmovFpr2GrD { fd, rj } => {
                encode_2r(0x0000053, rj.encoding(), fd.encoding())
            }

            // ── FP Arithmetic (3R) ────────────────────────────────
            Instruction::FaddD { fd, fj, fk } => {
                encode_3r(0x0101, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FsubD { fd, fj, fk } => {
                encode_3r(0x0102, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FmulD { fd, fj, fk } => {
                encode_3r(0x0103, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FdivD { fd, fj, fk } => {
                encode_3r(0x0104, fk.encoding(), fj.encoding(), fd.encoding())
            }

            // ── No-op / Break ─────────────────────────────────────
            Instruction::Nop => {
                // NOP pseudo: and $r0, $r0, $r0
                encode_3r(OPC_AND, Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R0.encoding())
            }
            Instruction::Syscall => {
                // SYSCALL = 0x0000002B (opcode bits [31:15] = 0x0000, bits [14:0] = 0x002B)
                0x0000002Bu32.to_le_bytes()
            }
            Instruction::Break => {
                // BREAK = 0x0000002A (opcode bits [31:15] = 0x0000, bits [14:0] = 0x002A)
                0x0000002Au32.to_le_bytes()
            }
        }
    }

    /// Returns the mnemonic name of this instruction.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Instruction::AddW { .. } => "add.w",
            Instruction::AddD { .. } => "add.d",
            Instruction::SubW { .. } => "sub.w",
            Instruction::SubD { .. } => "sub.d",
            Instruction::Slt { .. } => "slt",
            Instruction::Sltu { .. } => "sltu",
            Instruction::MulW { .. } => "mul.w",
            Instruction::MulD { .. } => "mul.d",
            Instruction::DivW { .. } => "div.w",
            Instruction::ModW { .. } => "mod.w",
            Instruction::DivD { .. } => "div.d",
            Instruction::ModD { .. } => "mod.d",
            Instruction::And { .. } => "and",
            Instruction::Or { .. } => "or",
            Instruction::Xor { .. } => "xor",
            Instruction::Nor { .. } => "nor",
            Instruction::Andn { .. } => "andn",
            Instruction::Orn { .. } => "orn",
            Instruction::SllW { .. } => "sll.w",
            Instruction::SrlW { .. } => "srl.w",
            Instruction::SraW { .. } => "sra.w",
            Instruction::SllD { .. } => "sll.d",
            Instruction::SrlD { .. } => "srl.d",
            Instruction::SraD { .. } => "sra.d",
            Instruction::RotrW { .. } => "rotr.w",
            Instruction::RotrD { .. } => "rotr.d",
            Instruction::SlliW { .. } => "slli.w",
            Instruction::SrliW { .. } => "srli.w",
            Instruction::SraiW { .. } => "srai.w",
            Instruction::SlliD { .. } => "slli.d",
            Instruction::SrliD { .. } => "srli.d",
            Instruction::SraiD { .. } => "srai.d",
            Instruction::AddiW { .. } => "addi.w",
            Instruction::AddiD { .. } => "addi.d",
            Instruction::Slti { .. } => "slti",
            Instruction::Sltui { .. } => "sltui",
            Instruction::Andi { .. } => "andi",
            Instruction::Ori { .. } => "ori",
            Instruction::Xori { .. } => "xori",
            Instruction::LdB { .. } => "ld.b",
            Instruction::LdH { .. } => "ld.h",
            Instruction::LdW { .. } => "ld.w",
            Instruction::LdD { .. } => "ld.d",
            Instruction::LdBu { .. } => "ld.bu",
            Instruction::LdHu { .. } => "ld.hu",
            Instruction::LdWu { .. } => "ld.wu",
            Instruction::StB { .. } => "st.b",
            Instruction::StH { .. } => "st.h",
            Instruction::StW { .. } => "st.w",
            Instruction::StD { .. } => "st.d",
            Instruction::Beq { .. } => "beq",
            Instruction::Bne { .. } => "bne",
            Instruction::Blt { .. } => "blt",
            Instruction::Bge { .. } => "bge",
            Instruction::Bltu { .. } => "bltu",
            Instruction::Bgeu { .. } => "bgeu",
            Instruction::Jirl { .. } => "jirl",
            Instruction::B { .. } => "b",
            Instruction::Bl { .. } => "bl",
            Instruction::Beqz { .. } => "beqz",
            Instruction::Bnez { .. } => "bnez",
            Instruction::Lu12iW { .. } => "lu12i.w",
            Instruction::Lu32iD { .. } => "lu32i.d",
            Instruction::Lu52iD { .. } => "lu52i.d",
            Instruction::Pcaddu12i { .. } => "pcaddu12i",
            Instruction::Pcaddu18i { .. } => "pcaddu18i",
            Instruction::LlW { .. } => "ll.w",
            Instruction::ScW { .. } => "sc.w",
            Instruction::LlD { .. } => "ll.d",
            Instruction::ScD { .. } => "sc.d",
            Instruction::ExtWH { .. } => "ext.w.h",
            Instruction::ExtWB { .. } => "ext.w.b",
            Instruction::FldS { .. } => "fld.s",
            Instruction::FldD { .. } => "fld.d",
            Instruction::FstS { .. } => "fst.s",
            Instruction::FstD { .. } => "fst.d",
            Instruction::FmovGr2FprD { .. } => "movfr2gr.d",
            Instruction::FmovFpr2GrD { .. } => "movgr2fr.d",
            Instruction::FaddD { .. } => "fadd.d",
            Instruction::FsubD { .. } => "fsub.d",
            Instruction::FmulD { .. } => "fmul.d",
            Instruction::FdivD { .. } => "fdiv.d",
            Instruction::Nop => "nop",
            Instruction::Syscall => "syscall",
            Instruction::Break => "break",
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::AddW { rd, rj, rk } => write!(f, "add.w {}, {}, {}", rd, rj, rk),
            Instruction::AddD { rd, rj, rk } => write!(f, "add.d {}, {}, {}", rd, rj, rk),
            Instruction::SubW { rd, rj, rk } => write!(f, "sub.w {}, {}, {}", rd, rj, rk),
            Instruction::SubD { rd, rj, rk } => write!(f, "sub.d {}, {}, {}", rd, rj, rk),
            Instruction::Slt { rd, rj, rk } => write!(f, "slt {}, {}, {}", rd, rj, rk),
            Instruction::Sltu { rd, rj, rk } => write!(f, "sltu {}, {}, {}", rd, rj, rk),
            Instruction::MulW { rd, rj, rk } => write!(f, "mul.w {}, {}, {}", rd, rj, rk),
            Instruction::MulD { rd, rj, rk } => write!(f, "mul.d {}, {}, {}", rd, rj, rk),
            Instruction::DivW { rd, rj, rk } => write!(f, "div.w {}, {}, {}", rd, rj, rk),
            Instruction::ModW { rd, rj, rk } => write!(f, "mod.w {}, {}, {}", rd, rj, rk),
            Instruction::DivD { rd, rj, rk } => write!(f, "div.d {}, {}, {}", rd, rj, rk),
            Instruction::ModD { rd, rj, rk } => write!(f, "mod.d {}, {}, {}", rd, rj, rk),
            Instruction::And { rd, rj, rk } => write!(f, "and {}, {}, {}", rd, rj, rk),
            Instruction::Or { rd, rj, rk } => write!(f, "or {}, {}, {}", rd, rj, rk),
            Instruction::Xor { rd, rj, rk } => write!(f, "xor {}, {}, {}", rd, rj, rk),
            Instruction::Nor { rd, rj, rk } => write!(f, "nor {}, {}, {}", rd, rj, rk),
            Instruction::Andn { rd, rj, rk } => write!(f, "andn {}, {}, {}", rd, rj, rk),
            Instruction::Orn { rd, rj, rk } => write!(f, "orn {}, {}, {}", rd, rj, rk),
            Instruction::SllW { rd, rj, rk } => write!(f, "sll.w {}, {}, {}", rd, rj, rk),
            Instruction::SrlW { rd, rj, rk } => write!(f, "srl.w {}, {}, {}", rd, rj, rk),
            Instruction::SraW { rd, rj, rk } => write!(f, "sra.w {}, {}, {}", rd, rj, rk),
            Instruction::SllD { rd, rj, rk } => write!(f, "sll.d {}, {}, {}", rd, rj, rk),
            Instruction::SrlD { rd, rj, rk } => write!(f, "srl.d {}, {}, {}", rd, rj, rk),
            Instruction::SraD { rd, rj, rk } => write!(f, "sra.d {}, {}, {}", rd, rj, rk),
            Instruction::RotrW { rd, rj, rk } => write!(f, "rotr.w {}, {}, {}", rd, rj, rk),
            Instruction::RotrD { rd, rj, rk } => write!(f, "rotr.d {}, {}, {}", rd, rj, rk),
            Instruction::SlliW { rd, rj, imm8 } => write!(f, "slli.w {}, {}, {}", rd, rj, imm8),
            Instruction::SrliW { rd, rj, imm8 } => write!(f, "srli.w {}, {}, {}", rd, rj, imm8),
            Instruction::SraiW { rd, rj, imm8 } => write!(f, "srai.w {}, {}, {}", rd, rj, imm8),
            Instruction::SlliD { rd, rj, imm8 } => write!(f, "slli.d {}, {}, {}", rd, rj, imm8),
            Instruction::SrliD { rd, rj, imm8 } => write!(f, "srli.d {}, {}, {}", rd, rj, imm8),
            Instruction::SraiD { rd, rj, imm8 } => write!(f, "srai.d {}, {}, {}", rd, rj, imm8),
            Instruction::AddiW { rd, rj, imm12 } => write!(f, "addi.w {}, {}, {}", rd, rj, imm12),
            Instruction::AddiD { rd, rj, imm12 } => write!(f, "addi.d {}, {}, {}", rd, rj, imm12),
            Instruction::Slti { rd, rj, imm12 } => write!(f, "slti {}, {}, {}", rd, rj, imm12),
            Instruction::Sltui { rd, rj, imm12 } => write!(f, "sltui {}, {}, {}", rd, rj, imm12),
            Instruction::Andi { rd, rj, imm12 } => write!(f, "andi {}, {}, {:#x}", rd, rj, imm12),
            Instruction::Ori { rd, rj, imm12 } => write!(f, "ori {}, {}, {:#x}", rd, rj, imm12),
            Instruction::Xori { rd, rj, imm12 } => write!(f, "xori {}, {}, {:#x}", rd, rj, imm12),
            Instruction::LdB { rd, rj, imm12 } => write!(f, "ld.b {}, {}, {}", rd, rj, imm12),
            Instruction::LdH { rd, rj, imm12 } => write!(f, "ld.h {}, {}, {}", rd, rj, imm12),
            Instruction::LdW { rd, rj, imm12 } => write!(f, "ld.w {}, {}, {}", rd, rj, imm12),
            Instruction::LdD { rd, rj, imm12 } => write!(f, "ld.d {}, {}, {}", rd, rj, imm12),
            Instruction::LdBu { rd, rj, imm12 } => write!(f, "ld.bu {}, {}, {}", rd, rj, imm12),
            Instruction::LdHu { rd, rj, imm12 } => write!(f, "ld.hu {}, {}, {}", rd, rj, imm12),
            Instruction::LdWu { rd, rj, imm12 } => write!(f, "ld.wu {}, {}, {}", rd, rj, imm12),
            Instruction::StB { rd, rj, imm12 } => write!(f, "st.b {}, {}, {}", rd, rj, imm12),
            Instruction::StH { rd, rj, imm12 } => write!(f, "st.h {}, {}, {}", rd, rj, imm12),
            Instruction::StW { rd, rj, imm12 } => write!(f, "st.w {}, {}, {}", rd, rj, imm12),
            Instruction::StD { rd, rj, imm12 } => write!(f, "st.d {}, {}, {}", rd, rj, imm12),
            Instruction::Beq { rj, rd, offs16 } => write!(f, "beq {}, {}, {:+}", rj, rd, offs16),
            Instruction::Bne { rj, rd, offs16 } => write!(f, "bne {}, {}, {:+}", rj, rd, offs16),
            Instruction::Blt { rj, rd, offs16 } => write!(f, "blt {}, {}, {:+}", rj, rd, offs16),
            Instruction::Bge { rj, rd, offs16 } => write!(f, "bge {}, {}, {:+}", rj, rd, offs16),
            Instruction::Bltu { rj, rd, offs16 } => write!(f, "bltu {}, {}, {:+}", rj, rd, offs16),
            Instruction::Bgeu { rj, rd, offs16 } => write!(f, "bgeu {}, {}, {:+}", rj, rd, offs16),
            Instruction::Jirl { rd, rj, offs16 } => write!(f, "jirl {}, {}, {:+}", rd, rj, offs16),
            Instruction::B { offs26 } => write!(f, "b {:+}", offs26),
            Instruction::Bl { offs26 } => write!(f, "bl {:+}", offs26),
            Instruction::Beqz { rj, offs21 } => write!(f, "beqz {}, {:+}", rj, offs21),
            Instruction::Bnez { rj, offs21 } => write!(f, "bnez {}, {:+}", rj, offs21),
            Instruction::Lu12iW { rd, imm20 } => write!(f, "lu12i.w {}, {}", rd, imm20),
            Instruction::Lu32iD { rd, imm20 } => write!(f, "lu32i.d {}, {}", rd, imm20),
            Instruction::Lu52iD { rd, rj, imm12 } => write!(f, "lu52i.d {}, {}, {}", rd, rj, imm12),
            Instruction::Pcaddu12i { rd, imm20 } => write!(f, "pcaddu12i {}, {}", rd, imm20),
            Instruction::Pcaddu18i { rd, imm20 } => write!(f, "pcaddu18i {}, {}", rd, imm20),
            Instruction::LlW { rd, rj, imm14 } => write!(f, "ll.w {}, {}, {}", rd, rj, imm14),
            Instruction::ScW { rd, rj, imm14 } => write!(f, "sc.w {}, {}, {}", rd, rj, imm14),
            Instruction::LlD { rd, rj, imm14 } => write!(f, "ll.d {}, {}, {}", rd, rj, imm14),
            Instruction::ScD { rd, rj, imm14 } => write!(f, "sc.d {}, {}, {}", rd, rj, imm14),
            Instruction::ExtWH { rd, rj } => write!(f, "ext.w.h {}, {}", rd, rj),
            Instruction::ExtWB { rd, rj } => write!(f, "ext.w.b {}, {}", rd, rj),
            Instruction::FldS { fd, rj, imm12 } => write!(f, "fld.s {}, {}, {}", fd, rj, imm12),
            Instruction::FldD { fd, rj, imm12 } => write!(f, "fld.d {}, {}, {}", fd, rj, imm12),
            Instruction::FstS { fd, rj, imm12 } => write!(f, "fst.s {}, {}, {}", fd, rj, imm12),
            Instruction::FstD { fd, rj, imm12 } => write!(f, "fst.d {}, {}, {}", fd, rj, imm12),
            Instruction::FmovGr2FprD { rd, fj } => write!(f, "movfr2gr.d {}, {}", rd, fj),
            Instruction::FmovFpr2GrD { fd, rj } => write!(f, "movgr2fr.d {}, {}", fd, rj),
            Instruction::FaddD { fd, fj, fk } => write!(f, "fadd.d {}, {}, {}", fd, fj, fk),
            Instruction::FsubD { fd, fj, fk } => write!(f, "fsub.d {}, {}, {}", fd, fj, fk),
            Instruction::FmulD { fd, fj, fk } => write!(f, "fmul.d {}, {}, {}", fd, fj, fk),
            Instruction::FdivD { fd, fj, fk } => write!(f, "fdiv.d {}, {}, {}", fd, fj, fk),
            Instruction::Nop => write!(f, "nop"),
            Instruction::Syscall => write!(f, "syscall"),
            Instruction::Break => write!(f, "break"),
        }
    }
}

// ===========================================================================
// ELF64 Emission
// ===========================================================================

/// Build a minimal ELF64 binary for LoongArch64 from raw code bytes.
///
/// Produces a static executable with a single LOAD segment containing the
/// `.text` section. Entry point is at `base_addr` + header offset.
fn build_minimal_loongarch64_elf(code: &[u8], base_addr: u64) -> Vec<u8> {
    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let text_offset = elf_header_size + phdr_size;
    let text_size = code.len() as u64;
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity(text_offset as usize + code.len());

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(2);   // ELFCLASS64
    elf.push(1);   // ELFDATA2LSB
    elf.push(1);   // EV_CURRENT
    elf.push(3);   // ELFOSABI_LINUX
    elf.push(0);   // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields ---
    elf.extend_from_slice(&2u16.to_le_bytes());       // e_type = ET_EXEC
    elf.extend_from_slice(&258u16.to_le_bytes());     // e_machine = EM_LOONGARCH
    elf.extend_from_slice(&1u32.to_le_bytes());       // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes());       // e_shoff (no section headers)
    elf.extend_from_slice(&0u32.to_le_bytes());       // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes());      // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes());      // e_phentsize
    elf.extend_from_slice(&1u16.to_le_bytes());       // e_phnum
    elf.extend_from_slice(&64u16.to_le_bytes());      // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes());       // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes());       // e_shstrndx

    // --- Program Header (single LOAD segment: PF_R | PF_X) ---
    elf.extend_from_slice(&1u32.to_le_bytes());       // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes());       // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_le_bytes());  // p_filesz
    elf.extend_from_slice(&text_size.to_le_bytes());  // p_memsz
    elf.extend_from_slice(&16u64.to_le_bytes());      // p_align

    // --- Code section ---
    elf.extend_from_slice(code);

    elf
}

// ===========================================================================
// LoongArch64Backend
// ===========================================================================

/// Decode a single LoongArch64 32-bit instruction word into a mnemonic string.
fn decode_loongarch64_instruction(word: u32) -> String {
    let opcode = word & 0x7f;
    match opcode {
        0x00 => {
            let rd = (word >> 7) & 0x1f;
            let rj = (word >> 15) & 0x1f;
            let rk = (word >> 10) & 0x1f;
            format!("add.w $r{}, $r{}, $r{}", rd, rj, rk)
        }
        0x01 => {
            let rd = (word >> 7) & 0x1f;
            let rj = (word >> 15) & 0x1f;
            let rk = (word >> 10) & 0x1f;
            format!("sub.w $r{}, $r{}, $r{}", rd, rj, rk)
        }
        0x02 => {
            let rd = (word >> 7) & 0x1f;
            let rj = (word >> 15) & 0x1f;
            let rk = (word >> 10) & 0x1f;
            format!("mul.w $r{}, $r{}, $r{}", rd, rj, rk)
        }
        0x05 => {
            let rd = (word >> 7) & 0x1f;
            let _rj = (word >> 15) & 0x1f; // lu12i.w does not use rj in the disassembly
            let si12 = ((word >> 10) as i32) << 20 >> 20;
            format!("lu12i.w $r{}, {}({})", rd, si12, si12)
        }
        0x08 => {
            let rd = (word >> 7) & 0x1f;
            let rj = (word >> 15) & 0x1f;
            let si12 = ((word >> 10) as i32) << 20 >> 20;
            format!("ld.w $r{}, $r{}, {}({})", rd, rj, si12, si12)
        }
        0x0a => {
            let rd = (word >> 7) & 0x1f;
            let rj = (word >> 15) & 0x1f;
            let si12 = ((word >> 10) as i32) << 20 >> 20;
            format!("st.w $r{}, $r{}, {}({})", rd, rj, si12, si12)
        }
        0x0c => {
            let rd = (word >> 7) & 0x1f;
            let rj = (word >> 15) & 0x1f;
            let si12 = ((word >> 10) as i32) << 20 >> 20;
            format!("ld.d $r{}, $r{}, {}({})", rd, rj, si12, si12)
        }
        0x0e => {
            let rd = (word >> 7) & 0x1f;
            let rj = (word >> 15) & 0x1f;
            let si12 = ((word >> 10) as i32) << 20 >> 20;
            format!("st.d $r{}, $r{}, {}({})", rd, rj, si12, si12)
        }
        0x10 => {
            let rj = (word >> 15) & 0x1f;
            let offs = ((word >> 10) as i32) << 12 >> 10;
            format!("beq $r{}, {}({})", rj, offs, offs)
        }
        0x11 => {
            let rj = (word >> 15) & 0x1f;
            let offs = ((word >> 10) as i32) << 12 >> 10;
            format!("bne $r{}, {}({})", rj, offs, offs)
        }
        0x14 => format!("bl {}", ((word >> 10) as i32) << 12 >> 10),
        0x15 => {
            let rd = (word >> 7) & 0x1f;
            format!("jirl $r{}, {}", rd, ((word >> 10) as i32) << 12 >> 10)
        }
        _ => format!(".word {:08x}", word),
    }
}

/// LoongArch64 code generation backend (LP64 ABI).
pub struct LoongArch64Backend {
    target_info: LoongArch64TargetInfo,
}

impl LoongArch64Backend {
    /// Create a new LoongArch64 backend.
    pub fn new() -> Self {
        Self {
            target_info: LoongArch64TargetInfo,
        }
    }
}

impl Default for LoongArch64Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the stack frame size for an IR function on LoongArch64.
///
/// Sums `Alloc` instruction sizes, adds 16 bytes for the ra/fp save pair,
/// and rounds up to 16-byte alignment.
fn loongarch64_compute_frame_size(func: &IRFunction) -> usize {
    let mut total: u32 = 16; // ra + fp save pair
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Alloc { size, .. } = instr {
                let aligned = (*size).div_ceil(16) * 16;
                total += aligned;
            }
        }
    }
    // Round up to 16-byte alignment
    total = (total + 15) & !15;
    total as usize
}

/// Allocatable GPR registers for LoongArch64, in priority order.
///
/// Order: temporaries first, then argument registers, then callee-saved.
const ALLOCATABLE_GPRS: &[Gpr] = &[
    // Caller-saved temporaries (highest priority — no save/restore needed)
    Gpr::T0, Gpr::T1, Gpr::T2, Gpr::T3, Gpr::T4, Gpr::T5, Gpr::T6, Gpr::T7, Gpr::T8,
    // Argument registers (also caller-saved)
    Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7,
    // Callee-saved (require save/restore)
    Gpr::S0, Gpr::S1, Gpr::S2, Gpr::S3, Gpr::S4, Gpr::S5, Gpr::S6, Gpr::S7, Gpr::S8,
    // Frame pointer is last — we prefer not to use it for general allocation
    Gpr::Fp,
];

/// Map from virtual register ID to a physical GPR using a simple linear scan.
fn map_vreg_to_gpr(
    vreg_id: u32,
    arg_index: Option<usize>,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
) -> Gpr {
    if let Some(gpr) = vreg_map.get(&vreg_id) {
        return *gpr;
    }

    // If this is an argument, map it to the corresponding argument register.
    if let Some(idx) = arg_index {
        if let Some(reg) = Gpr::arg_register(idx) {
            vreg_map.insert(vreg_id, reg);
            return reg;
        }
    }

    // Otherwise, pick the first available allocatable register.
    let used: std::collections::HashSet<Gpr> = vreg_map.values().copied().collect();
    for &reg in ALLOCATABLE_GPRS {
        if !used.contains(&reg) {
            vreg_map.insert(vreg_id, reg);
            return reg;
        }
    }

    // Fallback: use t8 as a scratch register.
    Gpr::T8
}

/// Helper: extract the virtual register ID from an IRValue, if it is a register.
fn vreg_id(val: &IRValue) -> u32 {
    match val {
        IRValue::Register(id) => *id,
        _ => 0,
    }
}

/// Emit a single AllocatedInstruction from a LoongArch64 Instruction.
fn emit_alloc_instr(inst: Instruction, reads: Vec<PhysicalReg>, writes: Vec<PhysicalReg>) -> AllocatedInstruction {
    AllocatedInstruction {
        opcode: inst.mnemonic().to_string(),
        reads,
        writes,
        encoded: inst.encode().to_vec(),
    }
}

/// Lower a comparison kind to LoongArch64 instructions, producing 0 or 1 in `dst`.
fn lower_cmp_la64(kind: &CmpKind, dst: Gpr, lhs: Gpr, rhs: Gpr) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();
    match kind {
        CmpKind::Eq => {
            // xor dst, lhs, rhs; sltui dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Xor { rd: dst, rj: lhs, rk: rhs },
                vec![PhysicalReg::new(RegClass::Gpr, lhs.encoding()), PhysicalReg::new(RegClass::Gpr, rhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Sltui { rd: dst, rj: dst, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::Ne => {
            // xor dst, lhs, rhs; sltu dst, $r0, dst
            result.push(emit_alloc_instr(
                Instruction::Xor { rd: dst, rj: lhs, rk: rhs },
                vec![PhysicalReg::new(RegClass::Gpr, lhs.encoding()), PhysicalReg::new(RegClass::Gpr, rhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Sltu { rd: dst, rj: Gpr::R0, rk: dst },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SLt => {
            result.push(emit_alloc_instr(
                Instruction::Slt { rd: dst, rj: lhs, rk: rhs },
                vec![PhysicalReg::new(RegClass::Gpr, lhs.encoding()), PhysicalReg::new(RegClass::Gpr, rhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SLe => {
            // slt dst, rhs, lhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Slt { rd: dst, rj: rhs, rk: lhs },
                vec![PhysicalReg::new(RegClass::Gpr, rhs.encoding()), PhysicalReg::new(RegClass::Gpr, lhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori { rd: dst, rj: dst, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SGt => {
            // slt dst, rhs, lhs
            result.push(emit_alloc_instr(
                Instruction::Slt { rd: dst, rj: rhs, rk: lhs },
                vec![PhysicalReg::new(RegClass::Gpr, rhs.encoding()), PhysicalReg::new(RegClass::Gpr, lhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SGe => {
            // slt dst, lhs, rhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Slt { rd: dst, rj: lhs, rk: rhs },
                vec![PhysicalReg::new(RegClass::Gpr, lhs.encoding()), PhysicalReg::new(RegClass::Gpr, rhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori { rd: dst, rj: dst, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::ULt => {
            result.push(emit_alloc_instr(
                Instruction::Sltu { rd: dst, rj: lhs, rk: rhs },
                vec![PhysicalReg::new(RegClass::Gpr, lhs.encoding()), PhysicalReg::new(RegClass::Gpr, rhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::ULe => {
            // sltu dst, rhs, lhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Sltu { rd: dst, rj: rhs, rk: lhs },
                vec![PhysicalReg::new(RegClass::Gpr, rhs.encoding()), PhysicalReg::new(RegClass::Gpr, lhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori { rd: dst, rj: dst, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::UGt => {
            result.push(emit_alloc_instr(
                Instruction::Sltu { rd: dst, rj: rhs, rk: lhs },
                vec![PhysicalReg::new(RegClass::Gpr, rhs.encoding()), PhysicalReg::new(RegClass::Gpr, lhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::UGe => {
            // sltu dst, lhs, rhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Sltu { rd: dst, rj: lhs, rk: rhs },
                vec![PhysicalReg::new(RegClass::Gpr, lhs.encoding()), PhysicalReg::new(RegClass::Gpr, rhs.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori { rd: dst, rj: dst, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
    }
    result
}

/// Lower a BinOp comparison kind to LoongArch64 instructions, producing 0 or 1 in `dst`.
fn lower_binop_cmp_la64(op: &BinOpKind, dst: Gpr, lhs: Gpr, rhs: Gpr) -> Vec<AllocatedInstruction> {
    let kind = match op {
        BinOpKind::SLt => CmpKind::SLt,
        BinOpKind::SLe => CmpKind::SLe,
        BinOpKind::SGt => CmpKind::SGt,
        BinOpKind::SGe => CmpKind::SGe,
        BinOpKind::ULt => CmpKind::ULt,
        BinOpKind::ULe => CmpKind::ULe,
        BinOpKind::UGt => CmpKind::UGt,
        BinOpKind::UGe => CmpKind::UGe,
        BinOpKind::Eq => CmpKind::Eq,
        BinOpKind::Ne => CmpKind::Ne,
        _ => CmpKind::Eq, // fallback, shouldn't happen
    };
    lower_cmp_la64(&kind, dst, lhs, rhs)
}

/// Lower a BinOp to LoongArch64 instructions.
fn lower_binop_la64(op: &BinOpKind, dst: &IRValue, lhs: &IRValue, rhs: &IRValue, vreg_map: &mut std::collections::HashMap<u32, Gpr>) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();
    let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
    let lhs_reg = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
    let rhs_reg = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);

    match op {
        BinOpKind::Add => {
            result.push(emit_alloc_instr(
                Instruction::AddD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::Sub => {
            result.push(emit_alloc_instr(
                Instruction::SubD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::Mul => {
            result.push(emit_alloc_instr(
                Instruction::MulD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::SDiv => {
            result.push(emit_alloc_instr(
                Instruction::DivD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::UDiv => {
            // LoongArch64 doesn't have unsigned div.d; use div.d as approximation
            result.push(emit_alloc_instr(
                Instruction::DivD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::SRem => {
            result.push(emit_alloc_instr(
                Instruction::ModD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::URem => {
            result.push(emit_alloc_instr(
                Instruction::ModD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::And => {
            result.push(emit_alloc_instr(
                Instruction::And { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::Or => {
            result.push(emit_alloc_instr(
                Instruction::Or { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::Xor => {
            result.push(emit_alloc_instr(
                Instruction::Xor { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::Shl => {
            result.push(emit_alloc_instr(
                Instruction::SllD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::ShrL => {
            result.push(emit_alloc_instr(
                Instruction::SrlD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::ShrA => {
            result.push(emit_alloc_instr(
                Instruction::SraD { rd: dst_reg, rj: lhs_reg, rk: rhs_reg },
                vec![PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()), PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        // Comparison BinOps
        BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
        | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
        | BinOpKind::Eq | BinOpKind::Ne => {
            result.extend(lower_binop_cmp_la64(op, dst_reg, lhs_reg, rhs_reg));
        }
    }
    result
}

/// Lower an IR instruction to a sequence of LoongArch64 AllocatedInstructions.
fn lower_ir_instr_la64(
    instr: &IRInstr,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();

    match instr {
        IRInstr::BinOp { op, dst, lhs, rhs } => {
            result.extend(lower_binop_la64(op, dst, lhs, rhs, vreg_map));
        }

        IRInstr::Add { dst, lhs, rhs } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let r = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::AddD { rd: d, rj: l, rk: r },
                vec![PhysicalReg::new(RegClass::Gpr, l.encoding()), PhysicalReg::new(RegClass::Gpr, r.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Sub { dst, lhs, rhs } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let r = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::SubD { rd: d, rj: l, rk: r },
                vec![PhysicalReg::new(RegClass::Gpr, l.encoding()), PhysicalReg::new(RegClass::Gpr, r.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Mul { dst, lhs, rhs } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let r = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::MulD { rd: d, rj: l, rk: r },
                vec![PhysicalReg::new(RegClass::Gpr, l.encoding()), PhysicalReg::new(RegClass::Gpr, r.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Div { dst, lhs, rhs } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let r = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::DivD { rd: d, rj: l, rk: r },
                vec![PhysicalReg::new(RegClass::Gpr, l.encoding()), PhysicalReg::new(RegClass::Gpr, r.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::UnaryOp { op, dst, operand } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let s = map_vreg_to_gpr(vreg_id(operand), None, vreg_map);
            match op {
                UnaryOpKind::Neg => {
                    // sub.d d, $r0, s
                    result.push(emit_alloc_instr(
                        Instruction::SubD { rd: d, rj: Gpr::R0, rk: s },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Not => {
                    // nor d, $r0, s
                    result.push(emit_alloc_instr(
                        Instruction::Nor { rd: d, rj: Gpr::R0, rk: s },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    // Placeholder: move operand to dst
                    result.push(emit_alloc_instr(
                        Instruction::AddD { rd: d, rj: s, rk: Gpr::R0 },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
            }
        }

        IRInstr::Cmp { kind, dst, lhs, rhs } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let r = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            result.extend(lower_cmp_la64(kind, d, l, r));
        }

        IRInstr::Load { dst, addr } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let a = map_vreg_to_gpr(vreg_id(addr), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::LdD { rd: d, rj: a, imm12: 0 },
                vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Store { value, addr } => {
            let v = map_vreg_to_gpr(vreg_id(value), None, vreg_map);
            let a = map_vreg_to_gpr(vreg_id(addr), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::StD { rd: v, rj: a, imm12: 0 },
                vec![PhysicalReg::new(RegClass::Gpr, a.encoding()), PhysicalReg::new(RegClass::Gpr, v.encoding())],
                vec![],
            ));
        }

        IRInstr::Alloc { dst, size: _ } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            // Placeholder: point to frame pointer area
            result.push(emit_alloc_instr(
                Instruction::AddiD { rd: d, rj: Gpr::Fp, imm12: 0 },
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::Fp.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Ret { values } => {
            // Move return value to $a0 if present
            if let Some(val) = values.first() {
                let v = map_vreg_to_gpr(vreg_id(val), None, vreg_map);
                if v != Gpr::A0 {
                    result.push(emit_alloc_instr(
                        Instruction::AddD { rd: Gpr::A0, rj: v, rk: Gpr::R0 },
                        vec![PhysicalReg::new(RegClass::Gpr, v.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::A0.encoding())],
                    ));
                }
            }
            // Epilogue: ld.d $fp, $sp, fs-16; ld.d $ra, $sp, fs-8; addi.d $sp, $sp, fs; jirl $r0, $ra, 0
            // Note: the frame size is not available here; the epilogue is already in the main function.
            // For Ret, we just do the return jump.
            result.push(emit_alloc_instr(
                Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 },
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
                vec![],
            ));
        }

        IRInstr::Call { dst, func: _, args } => {
            // Move args to argument registers, then bl
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    let a = map_vreg_to_gpr(vreg_id(arg), None, vreg_map);
                    if a != arg_reg {
                        result.push(emit_alloc_instr(
                            Instruction::AddD { rd: arg_reg, rj: a, rk: Gpr::R0 },
                            vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                            vec![PhysicalReg::new(RegClass::Gpr, arg_reg.encoding())],
                        ));
                    }
                }
            }
            result.push(emit_alloc_instr(
                Instruction::Bl { offs26: 0 },
                vec![],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
            ));
            // Move return value from $a0 to dst
            if let Some(d) = dst {
                let d_reg = map_vreg_to_gpr(vreg_id(d), None, vreg_map);
                if d_reg != Gpr::A0 {
                    result.push(emit_alloc_instr(
                        Instruction::AddD { rd: d_reg, rj: Gpr::A0, rk: Gpr::R0 },
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::A0.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d_reg.encoding())],
                    ));
                }
            }
        }

        IRInstr::Branch { target: _ } => {
            // b offs26 — placeholder offset
            result.push(emit_alloc_instr(
                Instruction::B { offs26: 0 },
                vec![],
                vec![],
            ));
        }

        IRInstr::CondBranch { cond, true_target: _, false_target: _ } => {
            let c = map_vreg_to_gpr(vreg_id(cond), None, vreg_map);
            // bnez c, true_target; b false_target
            result.push(emit_alloc_instr(
                Instruction::Bnez { rj: c, offs21: 8 },
                vec![PhysicalReg::new(RegClass::Gpr, c.encoding())],
                vec![],
            ));
            result.push(emit_alloc_instr(
                Instruction::B { offs26: 0 },
                vec![],
                vec![],
            ));
        }

        IRInstr::Cast { dst, src, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let s = map_vreg_to_gpr(vreg_id(src), None, vreg_map);
            if d != s {
                result.push(emit_alloc_instr(
                    Instruction::AddD { rd: d, rj: s, rk: Gpr::R0 },
                    vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                    vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                ));
            }
        }

        IRInstr::Select { dst, cond, true_val, false_val } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let c = map_vreg_to_gpr(vreg_id(cond), None, vreg_map);
            let tv = map_vreg_to_gpr(vreg_id(true_val), None, vreg_map);
            let fv = map_vreg_to_gpr(vreg_id(false_val), None, vreg_map);
            // Move false_val to dst; beqz cond, +8; move true_val to dst
            if fv != d {
                result.push(emit_alloc_instr(
                    Instruction::AddD { rd: d, rj: fv, rk: Gpr::R0 },
                    vec![PhysicalReg::new(RegClass::Gpr, fv.encoding())],
                    vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                ));
            }
            result.push(emit_alloc_instr(
                Instruction::Beqz { rj: c, offs21: 8 },
                vec![PhysicalReg::new(RegClass::Gpr, c.encoding())],
                vec![],
            ));
            result.push(emit_alloc_instr(
                Instruction::AddD { rd: d, rj: tv, rk: Gpr::R0 },
                vec![PhysicalReg::new(RegClass::Gpr, tv.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Offset { dst, base, offset } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let b = map_vreg_to_gpr(vreg_id(base), None, vreg_map);
            let o = map_vreg_to_gpr(vreg_id(offset), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::AddD { rd: d, rj: b, rk: o },
                vec![PhysicalReg::new(RegClass::Gpr, b.encoding()), PhysicalReg::new(RegClass::Gpr, o.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::GetAddress { dst, name: _ } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            result.push(emit_alloc_instr(
                Instruction::Lu12iW { rd: d, imm20: 0 },
                vec![],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Free { ptr: _ } | IRInstr::Phi { .. } => {
            // Placeholder: NOP
            result.push(emit_alloc_instr(
                Instruction::Nop,
                vec![],
                vec![],
            ));
        }
    }

    result
}

impl Backend for LoongArch64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {

        let func_name = func.name.clone();
        let frame_size = loongarch64_compute_frame_size(func);

        // Build a mapping from virtual register IDs to physical GPRs.
        let mut vreg_map: std::collections::HashMap<u32, Gpr> = std::collections::HashMap::new();

        // Scan instructions and build the register map.
        let param_count = func.params.len();
        for block in &func.blocks {
            for instr in &block.instructions {
                let arg_index = |vreg_id: u32| -> Option<usize> {
                    if (vreg_id as usize) < param_count {
                        Some(vreg_id as usize)
                    } else {
                        None
                    }
                };

                let values: Vec<&IRValue> = match instr {
                    IRInstr::BinOp { dst, lhs, rhs, .. } => vec![dst, lhs, rhs],
                    IRInstr::Add { dst, lhs, rhs } => vec![dst, lhs, rhs],
                    IRInstr::Sub { dst, lhs, rhs } => vec![dst, lhs, rhs],
                    IRInstr::Mul { dst, lhs, rhs } => vec![dst, lhs, rhs],
                    IRInstr::Div { dst, lhs, rhs } => vec![dst, lhs, rhs],
                    IRInstr::UnaryOp { dst, operand, .. } => vec![dst, operand],
                    IRInstr::Load { dst, addr } => vec![dst, addr],
                    IRInstr::Store { value, addr } => vec![value, addr],
                    IRInstr::Alloc { dst, .. } => vec![dst],
                    IRInstr::Cast { dst, src, .. } => vec![dst, src],
                    IRInstr::Select { dst, cond, true_val, false_val } => {
                        vec![dst, cond, true_val, false_val]
                    }
                    IRInstr::Offset { dst, base, offset } => vec![dst, base, offset],
                    IRInstr::GetAddress { dst, .. } => vec![dst],
                    _ => vec![],
                };
                for val in values {
                    if let IRValue::Register(id) = val {
                        map_vreg_to_gpr(*id, arg_index(*id), &mut vreg_map);
                    }
                }
            }
        }

        // Determine which callee-saved registers are used.
        let callee_saved: Vec<PhysicalReg> = vreg_map
            .values()
            .filter(|r| r.is_callee_saved())
            .map(|r| PhysicalReg::new(RegClass::Gpr, r.encoding()))
            .collect();

        // Generate prologue + body + epilogue as allocated instructions.
        let mut instructions: Vec<AllocatedInstruction> = Vec::new();

        // Prologue: addi.d $sp, $sp, -frame_size
        //           st.d $ra, $sp, frame_size-8
        //           st.d $fp, $sp, frame_size-16
        //           addi.d $fp, $sp, 0
        let fs = frame_size as i32;
        instructions.push(AllocatedInstruction {
            opcode: "addi.d".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            encoded: Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -fs }
                .encode()
                .to_vec(),
        });
        instructions.push(AllocatedInstruction {
            opcode: "st.d".to_string(),
            reads: vec![
                PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding()),
                PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()),
            ],
            writes: vec![],
            encoded: Instruction::StD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: fs - 8 }
                .encode()
                .to_vec(),
        });
        instructions.push(AllocatedInstruction {
            opcode: "st.d".to_string(),
            reads: vec![
                PhysicalReg::new(RegClass::Gpr, Gpr::Fp.encoding()),
                PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()),
            ],
            writes: vec![],
            encoded: Instruction::StD { rd: Gpr::Fp, rj: Gpr::Sp, imm12: fs - 16 }
                .encode()
                .to_vec(),
        });
        instructions.push(AllocatedInstruction {
            opcode: "addi.d".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Fp.encoding())],
            encoded: Instruction::AddiD { rd: Gpr::Fp, rj: Gpr::Sp, imm12: 0 }
                .encode()
                .to_vec(),
        });

        // Body: real instruction selection — translate each IR instruction
        // into one or more LoongArch64 machine-code instructions.
        for block in &func.blocks {
            for instr in &block.instructions {
                instructions.extend(lower_ir_instr_la64(instr, &mut vreg_map));
            }
        }

        // Epilogue: ld.d $fp, $sp, frame_size-16
        //           ld.d $ra, $sp, frame_size-8
        //           addi.d $sp, $sp, frame_size
        //           jirl $r0, $ra, 0
        instructions.push(AllocatedInstruction {
            opcode: "ld.d".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Fp.encoding())],
            encoded: Instruction::LdD { rd: Gpr::Fp, rj: Gpr::Sp, imm12: fs - 16 }
                .encode()
                .to_vec(),
        });
        instructions.push(AllocatedInstruction {
            opcode: "ld.d".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
            encoded: Instruction::LdD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: fs - 8 }
                .encode()
                .to_vec(),
        });
        instructions.push(AllocatedInstruction {
            opcode: "addi.d".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            encoded: Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: fs }
                .encode()
                .to_vec(),
        });
        instructions.push(AllocatedInstruction {
            opcode: "jirl".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
            writes: vec![],
            encoded: Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }
                .encode()
                .to_vec(),
        });

        let code_size = instructions.len() * 4;

        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions,
                code_offset: 0,
            }],
            frame_size,
            callee_saved,
            spill_slots: 0,
            code_size,
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
        // Collect all encoded bytes from every function and block.
        let mut all_code = Vec::new();
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }

        // Wrap in a minimal ELF64 binary for LoongArch64.
        Ok(build_minimal_loongarch64_elf(&all_code, 0x120000000))
    }

    fn return_stub(&self) -> Vec<u8> {
        // jirl $r0, $ra, 0 (return to caller)
        Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }
            .encode()
            .to_vec()
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // lu12i.w $t0, %hi20(entry_addr)
        // ori $t0, $t0, %lo12(entry_addr)
        // lu32i.d $t0, %hi32(entry_addr)
        // lu52i.d $t0, $t0, %hi52(entry_addr)
        // jr $t0
        let mut code = Vec::with_capacity(20);

        // lu12i.w $t0, bits[31:12] of entry_addr
        let hi20 = ((entry_addr >> 12) & 0xFFFFF) as i32;
        code.extend_from_slice(&Instruction::Lu12iW { rd: Gpr::T0, imm20: hi20 }.encode());

        // ori $t0, $t0, bits[11:0] of entry_addr
        let lo12 = (entry_addr & 0xFFF) as u32;
        code.extend_from_slice(&Instruction::Ori { rd: Gpr::T0, rj: Gpr::T0, imm12: lo12 }.encode());

        // lu32i.d $t0, bits[51:32] of entry_addr
        let hi32 = ((entry_addr >> 32) & 0xFFFFF) as i32;
        code.extend_from_slice(&Instruction::Lu32iD { rd: Gpr::T0, imm20: hi32 }.encode());

        // lu52i.d $t0, $t0, bits[63:52] of entry_addr
        let hi52 = ((entry_addr >> 52) & 0xFFF) as i32;
        code.extend_from_slice(
            &Instruction::Lu52iD { rd: Gpr::T0, rj: Gpr::T0, imm12: hi52 }.encode(),
        );

        // jr $t0 = jirl $r0, $t0, 0
        code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::T0, offs16: 0 }.encode());

        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        // LoongArch64 disassembler decoding 10+ instruction types.
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset + 4 <= bytes.len() {
            let word = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            let decoded = decode_loongarch64_instruction(word);
            lines.push(format!("{:#010x}:  {:08x}  {}", pc, word, decoded));
            offset += 4;
            pc += 4;
        }
        if offset < bytes.len() {
            let remaining = &bytes[offset..];
            lines.push(format!("{:#010x}:  {:02x?}", pc, remaining));
        }
        lines
    }

    fn name(&self) -> &'static str {
        "loongarch64"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── Gpr tests ──────────────────────────────────────────────────────

    #[test]
    fn test_gpr_encoding() {
        assert_eq!(Gpr::R0.encoding(), 0);
        assert_eq!(Gpr::Ra.encoding(), 1);
        assert_eq!(Gpr::Sp.encoding(), 3);
        assert_eq!(Gpr::A0.encoding(), 4);
        assert_eq!(Gpr::A7.encoding(), 11);
        assert_eq!(Gpr::T0.encoding(), 12);
        assert_eq!(Gpr::Fp.encoding(), 22);
        assert_eq!(Gpr::S0.encoding(), 23);
        assert_eq!(Gpr::S8.encoding(), 31);
    }

    #[test]
    fn test_gpr_allocatable() {
        assert!(!Gpr::R0.is_allocatable()); // zero
        assert!(!Gpr::Ra.is_allocatable()); // return address
        assert!(!Gpr::Tp.is_allocatable()); // thread pointer
        assert!(!Gpr::Sp.is_allocatable()); // stack pointer
        assert!(Gpr::A0.is_allocatable());
        assert!(Gpr::T0.is_allocatable());
        assert!(Gpr::S0.is_allocatable());
        assert!(Gpr::Fp.is_allocatable());
    }

    #[test]
    fn test_gpr_callee_saved() {
        assert!(Gpr::Fp.is_callee_saved());
        assert!(Gpr::S0.is_callee_saved());
        assert!(Gpr::S8.is_callee_saved());
        assert!(!Gpr::A0.is_callee_saved());
        assert!(!Gpr::T0.is_callee_saved());
        assert!(!Gpr::Ra.is_callee_saved());
    }

    #[test]
    fn test_gpr_arg_reg() {
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

    #[test]
    fn test_gpr_asm_name() {
        assert_eq!(Gpr::R0.asm_name(), "$r0");
        assert_eq!(Gpr::Ra.asm_name(), "$ra");
        assert_eq!(Gpr::Sp.asm_name(), "$sp");
        assert_eq!(Gpr::A0.asm_name(), "$a0");
        assert_eq!(Gpr::Fp.asm_name(), "$fp");
    }

    // ── Fpr tests ──────────────────────────────────────────────────────

    #[test]
    fn test_fpr_encoding() {
        assert_eq!(Fpr::F0.encoding(), 0);
        assert_eq!(Fpr::F7.encoding(), 7);
        assert_eq!(Fpr::F24.encoding(), 24);
        assert_eq!(Fpr::F31.encoding(), 31);
    }

    #[test]
    fn test_fpr_callee_saved() {
        assert!(Fpr::F24.is_callee_saved());
        assert!(Fpr::F31.is_callee_saved());
        assert!(!Fpr::F0.is_callee_saved());
        assert!(!Fpr::F23.is_callee_saved());
    }

    #[test]
    fn test_fpr_arg_reg() {
        assert!(Fpr::F0.is_arg_reg());
        assert!(Fpr::F7.is_arg_reg());
        assert!(!Fpr::F8.is_arg_reg());
        assert!(!Fpr::F24.is_arg_reg());
    }

    #[test]
    fn test_fpr_asm_name() {
        assert_eq!(Fpr::F0.asm_name(), "$fa0");
        assert_eq!(Fpr::F8.asm_name(), "$ft0");
        assert_eq!(Fpr::F24.asm_name(), "$fs0");
    }

    // ── Instruction encoding tests ─────────────────────────────────────

    #[test]
    fn test_encode_add_w() {
        // ADD.W $a0, $a1, $a2 => opcode=0x0020, rk=a2(6), rj=a1(5), rd=a0(4)
        let bytes = Instruction::AddW { rd: Gpr::A0, rj: Gpr::A1, rk: Gpr::A2 }.encode();
        let word = u32::from_le_bytes(bytes);
        // 3R: opcode[31:15] | rk[14:10] | rj[9:5] | rd[4:0]
        let expected = (0x0020u32 << 15) | (6u32 << 10) | (5u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_add_d() {
        let bytes = Instruction::AddD { rd: Gpr::A0, rj: Gpr::A1, rk: Gpr::A2 }.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0021u32 << 15) | (6u32 << 10) | (5u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_sub_d() {
        let bytes = Instruction::SubD { rd: Gpr::T0, rj: Gpr::T1, rk: Gpr::T2 }.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0031u32 << 15) | (14u32 << 10) | (13u32 << 5) | 12u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_addi_d() {
        // ADDI.D $sp, $sp, -16 => opcode=0x00B, imm12=0xFF0(-16), rj=sp(3), rd=sp(3)
        let bytes = Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -16 }.encode();
        let word = u32::from_le_bytes(bytes);
        let imm12 = ((-16i32) as u32) & 0xFFF;
        let expected = (0x00Bu32 << 22) | (imm12 << 10) | (3u32 << 5) | 3u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_ld_d() {
        // LD.D $a0, $sp, 8
        let bytes = Instruction::LdD { rd: Gpr::A0, rj: Gpr::Sp, imm12: 8 }.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A3u32 << 22) | (8u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_st_d() {
        // ST.D $ra, $sp, -8
        let bytes = Instruction::StD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: -8 }.encode();
        let word = u32::from_le_bytes(bytes);
        let imm12 = ((-8i32) as u32) & 0xFFF;
        let expected = (0x0AAu32 << 22) | (imm12 << 10) | (3u32 << 5) | 1u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_beq() {
        // BEQ $a0, $a1, 16
        let bytes = Instruction::Beq { rj: Gpr::A0, rd: Gpr::A1, offs16: 16 }.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x16u32 << 26) | (16u32 << 10) | (4u32 << 5) | 5u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_jirl() {
        // JIRL $r0, $ra, 0 (return instruction)
        let bytes = Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x13u32 << 26) | (0u32 << 10) | (1u32 << 5) | 0u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_bl() {
        // BL 0x100
        let bytes = Instruction::Bl { offs26: 0x100 }.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x15u32 << 26) | 0x100u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_and_or_xor() {
        // AND $a0, $a1, $a2
        let and_bytes = Instruction::And { rd: Gpr::A0, rj: Gpr::A1, rk: Gpr::A2 }.encode();
        let and_word = u32::from_le_bytes(and_bytes);
        assert_eq!(and_word >> 15, 0x0080);

        // OR $a0, $a1, $a2
        let or_bytes = Instruction::Or { rd: Gpr::A0, rj: Gpr::A1, rk: Gpr::A2 }.encode();
        let or_word = u32::from_le_bytes(or_bytes);
        assert_eq!(or_word >> 15, 0x0081);

        // XOR $a0, $a1, $a2
        let xor_bytes = Instruction::Xor { rd: Gpr::A0, rj: Gpr::A1, rk: Gpr::A2 }.encode();
        let xor_word = u32::from_le_bytes(xor_bytes);
        assert_eq!(xor_word >> 15, 0x0082);
    }

    #[test]
    fn test_encode_slt() {
        let bytes = Instruction::Slt { rd: Gpr::A0, rj: Gpr::A1, rk: Gpr::A2 }.encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word >> 15, 0x0040);
    }

    #[test]
    fn test_encode_beqz() {
        // BEQZ $a0, 0x10
        let bytes = Instruction::Beqz { rj: Gpr::A0, offs21: 0x10 }.encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 26) & 0x3F, 0x1C); // opcode check
    }

    // ── Format encoding tests ──────────────────────────────────────────

    #[test]
    fn test_encode_2r_format() {
        // ext.w.h $a0, $a1
        let bytes = encode_2r(OPC_EXT_W_H, Gpr::A1.encoding(), Gpr::A0.encoding());
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 10) & 0x3FF_FFFF, OPC_EXT_W_H);
        assert_eq!((word >> 5) & 0x1F, 5u32); // rj = a1
        assert_eq!(word & 0x1F, 4u32); // rd = a0
    }

    #[test]
    fn test_encode_3r_format() {
        let bytes = encode_3r(OPC_ADD_D, 6, 5, 4);
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_ADD_D);
        assert_eq!((word >> 10) & 0x1F, 6u32); // rk
        assert_eq!((word >> 5) & 0x1F, 5u32); // rj
        assert_eq!(word & 0x1F, 4u32); // rd
    }

    #[test]
    fn test_encode_2ri12_format() {
        let bytes = encode_2ri12(OPC_ADDI_D, 0x123, 3, 4);
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 22) & 0x3FF, OPC_ADDI_D);
        assert_eq!((word >> 10) & 0xFFF, 0x123u32); // imm12
        assert_eq!((word >> 5) & 0x1F, 3u32); // rj
        assert_eq!(word & 0x1F, 4u32); // rd
    }

    #[test]
    fn test_encode_i26_format() {
        let bytes = encode_i26(OPC_B, 0x12345);
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 26) & 0x3F, OPC_B);
        // hi10 = 0x12345 >> 16 = 0x1
        // lo16 = 0x2345
        assert_eq!((word >> 16) & 0x3FF, 0x1u32);
        assert_eq!(word & 0xFFFF, 0x2345u32);
    }

    // ── Backend tests ──────────────────────────────────────────────────

    #[test]
    fn test_backend_name() {
        let backend = LoongArch64Backend::new();
        assert_eq!(backend.name(), "loongarch64");
    }

    #[test]
    fn test_return_stub() {
        let backend = LoongArch64Backend::new();
        let stub = backend.return_stub();
        assert_eq!(stub.len(), 4);
        // JIRL $r0, $ra, 0
        let word = u32::from_le_bytes([stub[0], stub[1], stub[2], stub[3]]);
        let expected = (0x13u32 << 26) | (0u32 << 10) | (1u32 << 5) | 0u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_trampoline_length() {
        let backend = LoongArch64Backend::new();
        let tramp = backend.trampoline(0x120000000);
        assert_eq!(tramp.len(), 20); // 5 instructions × 4 bytes
    }

    #[test]
    fn test_disassemble() {
        let backend = LoongArch64Backend::new();
        let code = Instruction::AddD { rd: Gpr::A0, rj: Gpr::A1, rk: Gpr::A2 }.encode();
        let lines = backend.disassemble(&code, 0x120000000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("120000000"));
    }

    #[test]
    fn test_elf_header_machine_type() {
        let backend = LoongArch64Backend::new();
        let prog = AllocatedProgram {
            functions: vec![AllocatedFunction {
                name: "test".to_string(),
                blocks: vec![AllocatedBlock {
                    label: "entry".to_string(),
                    instructions: vec![AllocatedInstruction {
                        opcode: "nop".to_string(),
                        reads: vec![],
                        writes: vec![],
                        encoded: Instruction::Nop.encode().to_vec(),
                    }],
                    code_offset: 0,
                }],
                frame_size: 16,
                callee_saved: vec![],
                spill_slots: 0,
                code_size: 4,
            }],
            total_code_size: 4,
            total_data_size: 0,
        };
        let elf = backend.encode_program(&prog).unwrap();
        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // Check e_machine at offset 18 (2 bytes)
        let e_machine = u16::from_le_bytes([elf[18], elf[19]]);
        assert_eq!(e_machine, 258); // EM_LOONGARCH
    }
}
