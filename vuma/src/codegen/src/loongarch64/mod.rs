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
    BackendError, LoongArch64TargetInfo, PhysicalReg, RegClass, RelocationEntry, TargetInfo,
};
use crate::ir::{BinOpKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
use std::collections::HashMap;
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

    /// Convert a 5-bit encoding index back to a Gpr variant.
    pub fn from_encoding(enc: u32) -> Gpr {
        match enc {
            0 => Gpr::R0,
            1 => Gpr::Ra,
            2 => Gpr::Tp,
            3 => Gpr::Sp,
            4 => Gpr::A0,
            5 => Gpr::A1,
            6 => Gpr::A2,
            7 => Gpr::A3,
            8 => Gpr::A4,
            9 => Gpr::A5,
            10 => Gpr::A6,
            11 => Gpr::A7,
            12 => Gpr::T0,
            13 => Gpr::T1,
            14 => Gpr::T2,
            15 => Gpr::T3,
            16 => Gpr::T4,
            17 => Gpr::T5,
            18 => Gpr::T6,
            19 => Gpr::T7,
            20 => Gpr::T8,
            21 => Gpr::R21,
            22 => Gpr::Fp,
            23 => Gpr::S0,
            24 => Gpr::S1,
            25 => Gpr::S2,
            26 => Gpr::S3,
            27 => Gpr::S4,
            28 => Gpr::S5,
            29 => Gpr::S6,
            30 => Gpr::S7,
            31 => Gpr::S8,
            _ => Gpr::R0, // fallback: zero register
        }
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
            Gpr::Fp
                | Gpr::S0
                | Gpr::S1
                | Gpr::S2
                | Gpr::S3
                | Gpr::S4
                | Gpr::S5
                | Gpr::S6
                | Gpr::S7
                | Gpr::S8
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
            Fpr::F24 | Fpr::F25 | Fpr::F26 | Fpr::F27 | Fpr::F28 | Fpr::F29 | Fpr::F30 | Fpr::F31
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
    let word = ((opcode & 0x1FFFF) << 15) | ((rk & 0x1F) << 10) | ((rj & 0x1F) << 5) | (rd & 0x1F);
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
#[allow(dead_code)]
fn encode_2ri8(opcode: u32, imm8: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0x3FF) << 22) | ((imm8 & 0xFF) << 14) | ((rj & 0x1F) << 5) | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a reg2i5 format instruction (17-bit opcode).
///
/// Format: `opcode[31:15] | I5[14:10] | rj[9:5] | rd[4:0]`
fn encode_reg2i5(opcode: u32, imm5: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0x1FFFF) << 15) | ((imm5 & 0x1F) << 10) | ((rj & 0x1F) << 5) | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a reg2i6 format instruction (16-bit opcode).
///
/// Format: `opcode[31:16] | I6[15:10] | rj[9:5] | rd[4:0]`
fn encode_reg2i6(opcode: u32, imm6: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0xFFFF) << 16) | ((imm6 & 0x3F) << 10) | ((rj & 0x1F) << 5) | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 2RI12 format instruction.
///
/// Format: `opcode[31:22] | I12[21:10] | rj[9:5] | rd[4:0]`
fn encode_2ri12(opcode: u32, imm12: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word =
        ((opcode & 0x3FF) << 22) | ((imm12 & 0xFFF) << 10) | ((rj & 0x1F) << 5) | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 2RI14 format instruction.
///
/// Format: `opcode[31:24] | I14[23:10] | rj[9:5] | rd[4:0]`
fn encode_2ri14(opcode: u32, imm14: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word =
        ((opcode & 0xFF) << 24) | ((imm14 & 0x3FFF) << 10) | ((rj & 0x1F) << 5) | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 2RI16 format instruction.
///
/// Format: `opcode[31:26] | I16[25:10] | rj[9:5] | rd[4:0]`
fn encode_2ri16(opcode: u32, imm16: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word =
        ((opcode & 0x3F) << 26) | ((imm16 & 0xFFFF) << 10) | ((rj & 0x1F) << 5) | (rd & 0x1F);
    word.to_le_bytes()
}

/// Encode a 1RI21 format instruction (used for BEQZ, BNEZ).
///
/// Format: `opcode[31:26] | offs21[15:0] in bits[25:10] | rj[9:5] | offs21[20:16] in bits[4:0]`
///
/// Note: the offset bits are split non-linearly — the lower 16 bits go in the
/// higher position (bits[25:10]), and the upper 5 bits go in the lower position
/// (bits[4:0]). The register field `rj` sits between them at bits[9:5].
fn encode_1ri21(opcode: u32, imm21: u32, rj: u32) -> [u8; 4] {
    // 1RI21 format (BEQZ/BNEZ): opcode[31:26] | offs[15:0] at [25:10] | rj[9:5] | offs[20:16] at [4:0]
    let word = ((opcode & 0x3F) << 26)
        | ((imm21 & 0xFFFF) << 10)       // offs[15:0] at bits 25:10
        | ((rj & 0x1F) << 5)             // rj at bits 9:5
        | ((imm21 >> 16) & 0x1F);        // offs[20:16] at bits 4:0
    word.to_le_bytes()
}

/// Encode an I26 format instruction (used for B, BL).
///
/// Format: `opcode[31:26] | offs26[15:0] in bits[25:10] | offs26[25:16] in bits[9:0]`
///
/// Note: the offset bits are SWAPPED compared to a linear layout.
/// The lower 16 bits of the offset go in the higher position (bits[25:10]),
/// and the upper 10 bits go in the lower position (bits[9:0]).
fn encode_i26(opcode: u32, imm26: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((imm26 & 0xFFFF) << 10)
        | ((imm26 >> 16) & 0x3FF);
    word.to_le_bytes()
}

// ===========================================================================
// 3R-format Opcodes (bits[31:15])
// ===========================================================================

const OPC_ADD_W: u32 = 0x0020;
const OPC_ADD_D: u32 = 0x0021;
const OPC_SUB_W: u32 = 0x0022;
const OPC_SUB_D: u32 = 0x0023;
const OPC_SLT: u32 = 0x0024;
const OPC_SLTU: u32 = 0x0025;
const OPC_MASKEQZ: u32 = 0x0026;
const OPC_MASKNEZ: u32 = 0x0027;
const OPC_NOR: u32 = 0x0028;
const OPC_AND: u32 = 0x0029;
const OPC_OR: u32 = 0x002A;
const OPC_XOR: u32 = 0x002B;
const OPC_ORN: u32 = 0x002C;
const OPC_ANDN: u32 = 0x002D;
const OPC_SLL_W: u32 = 0x002E;
const OPC_SRL_W: u32 = 0x002F;
const OPC_SRA_W: u32 = 0x0030;
const OPC_SLL_D: u32 = 0x0031;
const OPC_SRL_D: u32 = 0x0032;
const OPC_SRA_D: u32 = 0x0033;
const OPC_ROTR_W: u32 = 0x0036;
const OPC_ROTR_D: u32 = 0x0037;
const OPC_MUL_W: u32 = 0x0038;
const OPC_MUL_D: u32 = 0x003B;
const OPC_DIV_W: u32 = 0x0040;
const OPC_MOD_W: u32 = 0x0041;
const OPC_DIV_WU: u32 = 0x0042;
const OPC_MOD_WU: u32 = 0x0043;
const OPC_DIV_D: u32 = 0x0044;
const OPC_MOD_D: u32 = 0x0045;
const OPC_DIV_DU: u32 = 0x0046;
const OPC_MOD_DU: u32 = 0x0047;

// ===========================================================================
// 3R-format FP Arithmetic Opcodes (bits[31:15])
// ===========================================================================

const OPC_FADD_S: u32 = 0x0201;
const OPC_FADD_D: u32 = 0x0202;
const OPC_FSUB_S: u32 = 0x0205;
const OPC_FSUB_D: u32 = 0x0206;
const OPC_FMUL_S: u32 = 0x0209;
const OPC_FMUL_D: u32 = 0x020A;
const OPC_FDIV_S: u32 = 0x020D;
const OPC_FDIV_D: u32 = 0x020E;

// ===========================================================================
// 2R-format FP Move Opcodes (bits[31:10])
// ===========================================================================

const OPC_FMOV_S: u32 = 0x004525;
const OPC_FMOV_D: u32 = 0x004526;

// ===========================================================================
// 4R-format FP Compare Opcodes (bits[31:20])
// ===========================================================================

const OPC_FCMP_S: u32 = 0x0C1;
const OPC_FCMP_D: u32 = 0x0C2;

// ===========================================================================
// 2RI12-format FP Load/Store Opcodes (bits[31:22])
// ===========================================================================

const OPC_FLD_S: u32 = 0x0AC;
const OPC_FLD_D: u32 = 0x0AE;
const OPC_FST_S: u32 = 0x0AD;
const OPC_FST_D: u32 = 0x0AF;

// ===========================================================================
// 2R-format FP GPR<->FPR Move Opcodes (bits[31:10])
// ===========================================================================

const OPC_MOVFR2GR_D: u32 = 0x00452E;
const OPC_MOVGR2FR_D: u32 = 0x00452A;

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
const OPC_ST_B: u32 = 0x0A4;
const OPC_ST_H: u32 = 0x0A5;
const OPC_ST_W: u32 = 0x0A6;
const OPC_ST_D: u32 = 0x0A7;
const OPC_LD_BU: u32 = 0x0A8;
const OPC_LD_HU: u32 = 0x0A9;
const OPC_LD_WU: u32 = 0x0AA;
const OPC_DBAR: u32 = 0x0E7; // DBAR: 2RI12 format with rd=$r0, rj=$r0

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
const OPC_LU12I_W: u32 = 0x0A; // reg1i20 format, 7-bit opcode at bits 31:25
const OPC_LU32I_D: u32 = 0x0B; // reg1i20 format, 7-bit opcode at bits 31:25
const OPC_LU52I_D: u32 = 0x0C; // 2RI12 format, 10-bit opcode at bits 31:22

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
// 3R-format Atomic Memory Operation Opcodes (bits[31:15])
// ===========================================================================

const OPC_AMSWAP_W: u32 = 0x00C0;
const OPC_AMSWAP_D: u32 = 0x00C2;

// ===========================================================================
// 1RI21-format Opcodes (bits[31:26])
// ===========================================================================

const OPC_BEQZ: u32 = 0x10;
const OPC_BNEZ: u32 = 0x11;
const OPC_PCADDU12I: u32 = 0x0E;
const OPC_PCADDU18I: u32 = 0x0F;

// ===========================================================================
// 2R-format Opcodes (bits[31:10])
// ===========================================================================

const OPC_EXT_W_H: u32 = 0x0000016;
const OPC_EXT_W_B: u32 = 0x0000017;
/// CLO.D (count leading ones, doubleword): opcode 0x0000008 in 2R format.
const OPC_CLO_D: u32 = 0x0000008;

#[allow(dead_code)]
const OPC_REVB_2H: u32 = 0x000000C;
#[allow(dead_code)]
const OPC_REVB_4H: u32 = 0x000000D;
#[allow(dead_code)]
const OPC_REVB_2W: u32 = 0x000000E;
#[allow(dead_code)]
const OPC_BITREV_4B: u32 = 0x0000012;
#[allow(dead_code)]
const OPC_BITREV_8B: u32 = 0x0000013;
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
// reg2i5-format Opcodes (bits[31:15], 17-bit) — .W shift immediates
// ===========================================================================

const OPC_SLLI_W: u32 = 0x0081;
const OPC_SRLI_W: u32 = 0x0089;
const OPC_SRAI_W: u32 = 0x0091;
#[allow(dead_code)]
const OPC_ROTRI_W: u32 = 0x0099;

// ===========================================================================
// reg2i6-format Opcodes (bits[31:16], 16-bit) — .D shift immediates
// ===========================================================================

const OPC_SLLI_D: u32 = 0x0041;
const OPC_SRLI_D: u32 = 0x0045;
const OPC_SRAI_D: u32 = 0x0049;
#[allow(dead_code)]
const OPC_ROTRI_D: u32 = 0x004D;

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
    /// Divide Word (unsigned): `div.wu rd, rj, rk`
    DivWu { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Modulo Word (unsigned): `mod.wu rd, rj, rk`
    ModWu { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Divide Doubleword (signed): `div.d rd, rj, rk`
    DivD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Modulo Doubleword (signed): `mod.d rd, rj, rk`
    ModD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Divide Doubleword (unsigned): `div.du rd, rj, rk`
    DivDu { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Modulo Doubleword (unsigned): `mod.du rd, rj, rk`
    ModDu { rd: Gpr, rj: Gpr, rk: Gpr },

    // ── Conditional Mask (3R) ───────────────────────────────────────
    /// Mask Equal Zero: `rd = (rk == 0) ? rj : 0`
    Maskeqz { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Mask Not Equal Zero: `rd = (rk != 0) ? rj : 0`
    Masknez { rd: Gpr, rj: Gpr, rk: Gpr },

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

    // ── Atomic Memory Operations (3R) ──────────────────────────────
    /// Atomic Memory Swap Word: `amswap.w rd, rj, rk`
    /// rd = old value at [rj]; [rj] = rk
    AmswapW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Swap Doubleword: `amswap.d rd, rj, rk`
    /// rd = old value at [rj]; [rj] = rk
    AmswapD { rd: Gpr, rj: Gpr, rk: Gpr },

    // ── Memory Barrier (2RI12) ─────────────────────────────────────
    /// Data Barrier: `dbar hint`
    /// Ensures memory ordering. hint=0 is a full barrier.
    Dbar { hint: u32 },

    // ── Move (2R) ───────────────────────────────────────────────────
    /// Sign-extend Halfword to Word: `ext.w.h rd, rj`
    ExtWH { rd: Gpr, rj: Gpr },
    /// Sign-extend Byte to Word: `ext.w.b rd, rj`
    ExtWB { rd: Gpr, rj: Gpr },
    /// Count Leading Ones, Doubleword: `clo.d rd, rj`
    CloD { rd: Gpr, rj: Gpr },

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
    /// FP Add Single: `fadd.s fd, fj, fk`
    FaddS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Add Double: `fadd.d fd, fj, fk`
    FaddD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Subtract Single: `fsub.s fd, fj, fk`
    FsubS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Subtract Double: `fsub.d fd, fj, fk`
    FsubD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Multiply Single: `fmul.s fd, fj, fk`
    FmulS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Multiply Double: `fmul.d fd, fj, fk`
    FmulD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Divide Single: `fdiv.s fd, fj, fk`
    FdivS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Divide Double: `fdiv.d fd, fj, fk`
    FdivD { fd: Fpr, fj: Fpr, fk: Fpr },

    // ── FP Move (2R) ───────────────────────────────────────────────
    /// FP Move Single: `fmov.s fd, fj`
    FmovS { fd: Fpr, fj: Fpr },
    /// FP Move Double: `fmov.d fd, fj`
    FmovD { fd: Fpr, fj: Fpr },

    // ── FP Conversion (2R) ──────────────────────────────────────────
    /// Float Convert From Signed Integer Word: `ffint.s.w fd, fj`
    FfintSW { fd: Fpr, fj: Fpr },
    /// Float Convert From Signed Integer Doubleword: `ffint.d.w fd, fj`
    FfintDW { fd: Fpr, fj: Fpr },
    /// Float Convert To Signed Integer Word: `ftint.w.s fd, fj`
    FtintWS { fd: Fpr, fj: Fpr },
    /// Float Convert To Signed Integer Doubleword: `ftint.w.d fd, fj`
    FtintWD { fd: Fpr, fj: Fpr },
    /// Float Convert Single to Double: `fcvt.d.s fd, fj`
    FcvtDS { fd: Fpr, fj: Fpr },
    /// Float Convert Double to Single: `fcvt.s.d fd, fj`
    FcvtSD { fd: Fpr, fj: Fpr },

    // ── FP Compare (4R-like) ────────────────────────────────────────
    /// FP Compare Single: `fcmp.cond.s cd, fj, fk`
    FCmpS { cond: u8, fj: Fpr, fk: Fpr, cd: u8 },
    /// FP Compare Double: `fcmp.cond.d cd, fj, fk`
    FCmpD { cond: u8, fj: Fpr, fk: Fpr, cd: u8 },

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
            Instruction::DivWu { rd, rj, rk } => {
                encode_3r(OPC_DIV_WU, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::ModWu { rd, rj, rk } => {
                encode_3r(OPC_MOD_WU, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::DivD { rd, rj, rk } => {
                encode_3r(OPC_DIV_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::ModD { rd, rj, rk } => {
                encode_3r(OPC_MOD_D, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::DivDu { rd, rj, rk } => {
                encode_3r(OPC_DIV_DU, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::ModDu { rd, rj, rk } => {
                encode_3r(OPC_MOD_DU, rk.encoding(), rj.encoding(), rd.encoding())
            }

            // ── Conditional Mask (3R) ─────────────────────────────
            Instruction::Maskeqz { rd, rj, rk } => {
                encode_3r(OPC_MASKEQZ, rk.encoding(), rj.encoding(), rd.encoding())
            }
            Instruction::Masknez { rd, rj, rk } => {
                encode_3r(OPC_MASKNEZ, rk.encoding(), rj.encoding(), rd.encoding())
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

            // ── Shift Immediate (reg2i5 / reg2i6) ───────────────────────
            Instruction::SlliW { rd, rj, imm8 } => {
                encode_reg2i5(OPC_SLLI_W, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SrliW { rd, rj, imm8 } => {
                encode_reg2i5(OPC_SRLI_W, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SraiW { rd, rj, imm8 } => {
                encode_reg2i5(OPC_SRAI_W, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SlliD { rd, rj, imm8 } => {
                encode_reg2i6(OPC_SLLI_D, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SrliD { rd, rj, imm8 } => {
                encode_reg2i6(OPC_SRLI_D, *imm8, rj.encoding(), rd.encoding())
            }
            Instruction::SraiD { rd, rj, imm8 } => {
                encode_reg2i6(OPC_SRAI_D, *imm8, rj.encoding(), rd.encoding())
            }

            // ── Immediate Arithmetic (2RI12) ──────────────────────
            Instruction::AddiW { rd, rj, imm12 } => encode_2ri12(
                OPC_ADDI_W,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::AddiD { rd, rj, imm12 } => encode_2ri12(
                OPC_ADDI_D,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Slti { rd, rj, imm12 } => encode_2ri12(
                OPC_SLTI,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Sltui { rd, rj, imm12 } => encode_2ri12(
                OPC_SLTUI,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
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
            Instruction::LdB { rd, rj, imm12 } => encode_2ri12(
                OPC_LD_B,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::LdH { rd, rj, imm12 } => encode_2ri12(
                OPC_LD_H,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::LdW { rd, rj, imm12 } => encode_2ri12(
                OPC_LD_W,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::LdD { rd, rj, imm12 } => encode_2ri12(
                OPC_LD_D,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::LdBu { rd, rj, imm12 } => encode_2ri12(
                OPC_LD_BU,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::LdHu { rd, rj, imm12 } => encode_2ri12(
                OPC_LD_HU,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::LdWu { rd, rj, imm12 } => encode_2ri12(
                OPC_LD_WU,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),

            // ── Store (2RI12) ─────────────────────────────────────
            Instruction::StB { rd, rj, imm12 } => encode_2ri12(
                OPC_ST_B,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::StH { rd, rj, imm12 } => encode_2ri12(
                OPC_ST_H,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::StW { rd, rj, imm12 } => encode_2ri12(
                OPC_ST_W,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::StD { rd, rj, imm12 } => encode_2ri12(
                OPC_ST_D,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                rd.encoding(),
            ),

            // ── Branch (2RI16) ────────────────────────────────────
            Instruction::Beq { rj, rd, offs16 } => encode_2ri16(
                OPC_BEQ,
                (*offs16 as u32) & 0xFFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Bne { rj, rd, offs16 } => encode_2ri16(
                OPC_BNE,
                (*offs16 as u32) & 0xFFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Blt { rj, rd, offs16 } => encode_2ri16(
                OPC_BLT,
                (*offs16 as u32) & 0xFFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Bge { rj, rd, offs16 } => encode_2ri16(
                OPC_BGE,
                (*offs16 as u32) & 0xFFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Bltu { rj, rd, offs16 } => encode_2ri16(
                OPC_BLTU,
                (*offs16 as u32) & 0xFFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Bgeu { rj, rd, offs16 } => encode_2ri16(
                OPC_BGEU,
                (*offs16 as u32) & 0xFFFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::Jirl { rd, rj, offs16 } => encode_2ri16(
                OPC_JIRL,
                (*offs16 as u32) & 0xFFFF,
                rj.encoding(),
                rd.encoding(),
            ),

            // ── Unconditional Branch (I26) ────────────────────────
            Instruction::B { offs26 } => encode_i26(OPC_B, (*offs26 as u32) & 0x3FFFFFF),
            Instruction::Bl { offs26 } => encode_i26(OPC_BL, (*offs26 as u32) & 0x3FFFFFF),

            // ── Branch on Zero/NonZero (1RI21) ────────────────────
            Instruction::Beqz { rj, offs21 } => {
                encode_1ri21(OPC_BEQZ, (*offs21 as u32) & 0x1FFFFF, rj.encoding())
            }
            Instruction::Bnez { rj, offs21 } => {
                encode_1ri21(OPC_BNEZ, (*offs21 as u32) & 0x1FFFFF, rj.encoding())
            }

            // ── Upper Immediate ────────────────────────────────────
            Instruction::Lu12iW { rd, imm20 } => {
                // lu12i.w is reg1i20 format: opcode[31:25] | si20[24:5] | rd[4:0]
                let word = ((OPC_LU12I_W & 0x7F) << 25)
                    | (((*imm20 as u32) & 0xFFFFF) << 5)
                    | (rd.encoding() & 0x1F);
                word.to_le_bytes()
            }
            Instruction::Lu32iD { rd, imm20 } => {
                // lu32i.d is reg1i20 format: opcode[31:25] | si20[24:5] | rd[4:0]
                let word = ((OPC_LU32I_D & 0x7F) << 25)
                    | (((*imm20 as u32) & 0xFFFFF) << 5)
                    | (rd.encoding() & 0x1F);
                word.to_le_bytes()
            }
            Instruction::Lu52iD { rd, rj, imm12 } => {
                // 2RI12 format
                encode_2ri12(
                    OPC_LU52I_D,
                    (*imm12 as u32) & 0xFFF,
                    rj.encoding(),
                    rd.encoding(),
                )
            }
            Instruction::Pcaddu12i { rd, imm20 } => {
                // PCADDU12I format: opcode[31:26] | imm20[19:0] in bits[25:6] | 0 in bit[5] | rd[4:0]
                let word = ((OPC_PCADDU12I & 0x3F) << 26)
                    | (((*imm20 as u32) & 0xFFFFF) << 6)
                    | (rd.encoding() & 0x1F);
                word.to_le_bytes()
            }
            Instruction::Pcaddu18i { rd, imm20 } => {
                // PCADDU18I format: opcode[31:26] | imm20[19:0] in bits[25:6] | 0 in bit[5] | rd[4:0]
                let word = ((OPC_PCADDU18I & 0x3F) << 26)
                    | (((*imm20 as u32) & 0xFFFFF) << 6)
                    | (rd.encoding() & 0x1F);
                word.to_le_bytes()
            }

            // ── Atomic (2RI14) ────────────────────────────────────
            Instruction::LlW { rd, rj, imm14 } => encode_2ri14(
                OPC_LL_W,
                (*imm14 as u32) & 0x3FFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::ScW { rd, rj, imm14 } => encode_2ri14(
                OPC_SC_W,
                (*imm14 as u32) & 0x3FFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::LlD { rd, rj, imm14 } => encode_2ri14(
                OPC_LL_D,
                (*imm14 as u32) & 0x3FFF,
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::ScD { rd, rj, imm14 } => encode_2ri14(
                OPC_SC_D,
                (*imm14 as u32) & 0x3FFF,
                rj.encoding(),
                rd.encoding(),
            ),

            // ── Atomic Memory Operations (3R) ─────────────────────
            Instruction::AmswapW { rd, rj, rk } => encode_3r(
                OPC_AMSWAP_W,
                rk.encoding(),
                rj.encoding(),
                rd.encoding(),
            ),
            Instruction::AmswapD { rd, rj, rk } => encode_3r(
                OPC_AMSWAP_D,
                rk.encoding(),
                rj.encoding(),
                rd.encoding(),
            ),

            // ── Memory Barrier (2RI12) ────────────────────────────
            Instruction::Dbar { hint } => encode_2ri12(
                OPC_DBAR,
                (*hint) & 0xFFF,
                0, // rj = $r0
                0, // rd = $r0
            ),

            // ── Move (2R) ─────────────────────────────────────────
            Instruction::ExtWH { rd, rj } => encode_2r(OPC_EXT_W_H, rj.encoding(), rd.encoding()),
            Instruction::ExtWB { rd, rj } => encode_2r(OPC_EXT_W_B, rj.encoding(), rd.encoding()),
            Instruction::CloD { rd, rj } => encode_2r(OPC_CLO_D, rj.encoding(), rd.encoding()),

            // ── FP Load/Store (2RI12) ─────────────────────────────
            Instruction::FldS { fd, rj, imm12 } => encode_2ri12(
                OPC_FLD_S,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                fd.encoding(),
            ),
            Instruction::FldD { fd, rj, imm12 } => encode_2ri12(
                OPC_FLD_D,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                fd.encoding(),
            ),
            Instruction::FstS { fd, rj, imm12 } => encode_2ri12(
                OPC_FST_S,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                fd.encoding(),
            ),
            Instruction::FstD { fd, rj, imm12 } => encode_2ri12(
                OPC_FST_D,
                (*imm12 as u32) & 0xFFF,
                rj.encoding(),
                fd.encoding(),
            ),

            // ── FP Move GPR<->FPR (2R) ────────────────────────────
            Instruction::FmovGr2FprD { rd, fj } => {
                encode_2r(OPC_MOVFR2GR_D, fj.encoding(), rd.encoding())
            }
            Instruction::FmovFpr2GrD { fd, rj } => {
                encode_2r(OPC_MOVGR2FR_D, rj.encoding(), fd.encoding())
            }

            // ── FP Arithmetic (3R) ────────────────────────────────
            Instruction::FaddS { fd, fj, fk } => {
                encode_3r(OPC_FADD_S, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FaddD { fd, fj, fk } => {
                encode_3r(OPC_FADD_D, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FsubS { fd, fj, fk } => {
                encode_3r(OPC_FSUB_S, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FsubD { fd, fj, fk } => {
                encode_3r(OPC_FSUB_D, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FmulS { fd, fj, fk } => {
                encode_3r(OPC_FMUL_S, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FmulD { fd, fj, fk } => {
                encode_3r(OPC_FMUL_D, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FdivS { fd, fj, fk } => {
                encode_3r(OPC_FDIV_S, fk.encoding(), fj.encoding(), fd.encoding())
            }
            Instruction::FdivD { fd, fj, fk } => {
                encode_3r(OPC_FDIV_D, fk.encoding(), fj.encoding(), fd.encoding())
            }

            // ── FP Move (2R) ──────────────────────────────────────
            Instruction::FmovS { fd, fj } => encode_2r(OPC_FMOV_S, fj.encoding(), fd.encoding()),
            Instruction::FmovD { fd, fj } => encode_2r(OPC_FMOV_D, fj.encoding(), fd.encoding()),

            // ── FP Conversion (2R) ──────────────────────────────
            // FFINT.S.W: opcode=0x004519, FFINT.D.W: opcode=0x00451A
            Instruction::FfintSW { fd, fj } => encode_2r(0x004519, fj.encoding(), fd.encoding()),
            Instruction::FfintDW { fd, fj } => encode_2r(0x00451A, fj.encoding(), fd.encoding()),
            // FTINT.W.S: opcode=0x00450C, FTINT.W.D: opcode=0x00450D
            Instruction::FtintWS { fd, fj } => encode_2r(0x00450C, fj.encoding(), fd.encoding()),
            Instruction::FtintWD { fd, fj } => encode_2r(0x00450D, fj.encoding(), fd.encoding()),
            // FCVT.D.S: opcode=0x004502, FCVT.S.D: opcode=0x004503
            Instruction::FcvtDS { fd, fj } => encode_2r(0x004502, fj.encoding(), fd.encoding()),
            Instruction::FcvtSD { fd, fj } => encode_2r(0x004503, fj.encoding(), fd.encoding()),

            // ── FP Compare (4R-like) ──────────────────────────────
            Instruction::FCmpS { cond, fj, fk, cd } => encode_4r(
                OPC_FCMP_S,
                (*cond & 0x1F) as u32,
                fk.encoding(),
                fj.encoding(),
                (*cd & 0x1F) as u32,
            ),
            Instruction::FCmpD { cond, fj, fk, cd } => encode_4r(
                OPC_FCMP_D,
                (*cond & 0x1F) as u32,
                fk.encoding(),
                fj.encoding(),
                (*cd & 0x1F) as u32,
            ),

            // ── No-op / Break ─────────────────────────────────────
            Instruction::Nop => {
                // NOP pseudo: and $r0, $r0, $r0
                encode_3r(
                    OPC_AND,
                    Gpr::R0.encoding(),
                    Gpr::R0.encoding(),
                    Gpr::R0.encoding(),
                )
            }
            Instruction::Syscall => {
                // SYSCALL = 0x002B0000 (with code=0)
                0x002B0000u32.to_le_bytes()
            }
            Instruction::Break => {
                // BREAK = 0x002A0000 (with code=0)
                0x002A0000u32.to_le_bytes()
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
            Instruction::DivWu { .. } => "div.wu",
            Instruction::ModWu { .. } => "mod.wu",
            Instruction::DivD { .. } => "div.d",
            Instruction::ModD { .. } => "mod.d",
            Instruction::DivDu { .. } => "div.du",
            Instruction::ModDu { .. } => "mod.du",
            Instruction::Maskeqz { .. } => "maskeqz",
            Instruction::Masknez { .. } => "masknez",
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
            Instruction::AmswapW { .. } => "amswap.w",
            Instruction::AmswapD { .. } => "amswap.d",
            Instruction::Dbar { .. } => "dbar",
            Instruction::ExtWH { .. } => "ext.w.h",
            Instruction::ExtWB { .. } => "ext.w.b",
            Instruction::CloD { .. } => "clo.d",
            Instruction::FldS { .. } => "fld.s",
            Instruction::FldD { .. } => "fld.d",
            Instruction::FstS { .. } => "fst.s",
            Instruction::FstD { .. } => "fst.d",
            Instruction::FmovGr2FprD { .. } => "movfr2gr.d",
            Instruction::FmovFpr2GrD { .. } => "movgr2fr.d",
            Instruction::FaddS { .. } => "fadd.s",
            Instruction::FaddD { .. } => "fadd.d",
            Instruction::FsubS { .. } => "fsub.s",
            Instruction::FsubD { .. } => "fsub.d",
            Instruction::FmulS { .. } => "fmul.s",
            Instruction::FmulD { .. } => "fmul.d",
            Instruction::FdivS { .. } => "fdiv.s",
            Instruction::FdivD { .. } => "fdiv.d",
            Instruction::FmovS { .. } => "fmov.s",
            Instruction::FmovD { .. } => "fmov.d",
            Instruction::FfintSW { .. } => "ffint.s.w",
            Instruction::FfintDW { .. } => "ffint.d.w",
            Instruction::FtintWS { .. } => "ftint.w.s",
            Instruction::FtintWD { .. } => "ftint.w.d",
            Instruction::FcvtDS { .. } => "fcvt.d.s",
            Instruction::FcvtSD { .. } => "fcvt.s.d",
            Instruction::FCmpS { .. } => "fcmp.cond.s",
            Instruction::FCmpD { .. } => "fcmp.cond.d",
            Instruction::Nop => "nop",
            Instruction::Syscall => "syscall",
            Instruction::Break => "break",
        }
    }
}

/// Returns the mnemonic for an FCMP condition code.
fn fcmp_cond_mnemonic(cond: u8) -> &'static str {
    match cond {
        0x00 => "caf",
        0x01 => "clt",
        0x02 => "ceq",
        0x03 => "cle",
        0x04 => "cun",
        0x05 => "cult",
        0x06 => "cueq",
        0x07 => "cule",
        0x08 => "cne",
        0x09 => "clts", // signed less-than (alternative encoding)
        0x0A => "cnes",
        0x0B => "cles",
        0x0C => "cuns",
        0x0D => "cults",
        0x0E => "cunes",
        0x0F => "cules",
        0x10 => "cat",
        _ => "c??",
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
            Instruction::DivWu { rd, rj, rk } => write!(f, "div.wu {}, {}, {}", rd, rj, rk),
            Instruction::ModWu { rd, rj, rk } => write!(f, "mod.wu {}, {}, {}", rd, rj, rk),
            Instruction::DivD { rd, rj, rk } => write!(f, "div.d {}, {}, {}", rd, rj, rk),
            Instruction::ModD { rd, rj, rk } => write!(f, "mod.d {}, {}, {}", rd, rj, rk),
            Instruction::DivDu { rd, rj, rk } => write!(f, "div.du {}, {}, {}", rd, rj, rk),
            Instruction::ModDu { rd, rj, rk } => write!(f, "mod.du {}, {}, {}", rd, rj, rk),
            Instruction::Maskeqz { rd, rj, rk } => write!(f, "maskeqz {}, {}, {}", rd, rj, rk),
            Instruction::Masknez { rd, rj, rk } => write!(f, "masknez {}, {}, {}", rd, rj, rk),
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
            Instruction::AmswapW { rd, rj, rk } => write!(f, "amswap.w {}, {}, {}", rd, rj, rk),
            Instruction::AmswapD { rd, rj, rk } => write!(f, "amswap.d {}, {}, {}", rd, rj, rk),
            Instruction::Dbar { hint } => write!(f, "dbar {}", hint),
            Instruction::ExtWH { rd, rj } => write!(f, "ext.w.h {}, {}", rd, rj),
            Instruction::ExtWB { rd, rj } => write!(f, "ext.w.b {}, {}", rd, rj),
            Instruction::CloD { rd, rj } => write!(f, "clo.d {}, {}", rd, rj),
            Instruction::FldS { fd, rj, imm12 } => write!(f, "fld.s {}, {}, {}", fd, rj, imm12),
            Instruction::FldD { fd, rj, imm12 } => write!(f, "fld.d {}, {}, {}", fd, rj, imm12),
            Instruction::FstS { fd, rj, imm12 } => write!(f, "fst.s {}, {}, {}", fd, rj, imm12),
            Instruction::FstD { fd, rj, imm12 } => write!(f, "fst.d {}, {}, {}", fd, rj, imm12),
            Instruction::FmovGr2FprD { rd, fj } => write!(f, "movfr2gr.d {}, {}", rd, fj),
            Instruction::FmovFpr2GrD { fd, rj } => write!(f, "movgr2fr.d {}, {}", fd, rj),
            Instruction::FaddS { fd, fj, fk } => write!(f, "fadd.s {}, {}, {}", fd, fj, fk),
            Instruction::FaddD { fd, fj, fk } => write!(f, "fadd.d {}, {}, {}", fd, fj, fk),
            Instruction::FsubS { fd, fj, fk } => write!(f, "fsub.s {}, {}, {}", fd, fj, fk),
            Instruction::FsubD { fd, fj, fk } => write!(f, "fsub.d {}, {}, {}", fd, fj, fk),
            Instruction::FmulS { fd, fj, fk } => write!(f, "fmul.s {}, {}, {}", fd, fj, fk),
            Instruction::FmulD { fd, fj, fk } => write!(f, "fmul.d {}, {}, {}", fd, fj, fk),
            Instruction::FdivS { fd, fj, fk } => write!(f, "fdiv.s {}, {}, {}", fd, fj, fk),
            Instruction::FdivD { fd, fj, fk } => write!(f, "fdiv.d {}, {}, {}", fd, fj, fk),
            Instruction::FmovS { fd, fj } => write!(f, "fmov.s {}, {}", fd, fj),
            Instruction::FmovD { fd, fj } => write!(f, "fmov.d {}, {}", fd, fj),
            Instruction::FfintSW { fd, fj } => write!(f, "ffint.s.w {}, {}", fd, fj),
            Instruction::FfintDW { fd, fj } => write!(f, "ffint.d.w {}, {}", fd, fj),
            Instruction::FtintWS { fd, fj } => write!(f, "ftint.w.s {}, {}", fd, fj),
            Instruction::FtintWD { fd, fj } => write!(f, "ftint.w.d {}, {}", fd, fj),
            Instruction::FcvtDS { fd, fj } => write!(f, "fcvt.d.s {}, {}", fd, fj),
            Instruction::FcvtSD { fd, fj } => write!(f, "fcvt.s.d {}, {}", fd, fj),
            Instruction::FCmpS { cond, fj, fk, cd } => write!(
                f,
                "fcmp.{}.s $c{}, {}, {}",
                fcmp_cond_mnemonic(*cond),
                cd,
                fj,
                fk
            ),
            Instruction::FCmpD { cond, fj, fk, cd } => write!(
                f,
                "fcmp.{}.d $c{}, {}, {}",
                fcmp_cond_mnemonic(*cond),
                cd,
                fj,
                fk
            ),
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
/// Produces a static executable with two LOAD segments:
///   1. PF_R | PF_X — .text (code)
///   2. PF_R | PF_W — .data / stack (writable memory)
fn build_loongarch64_elf_2seg(code: &[u8], base_addr: u64) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x10000; // 64 KB (LoongArch64 typical page size)

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = 2;
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    // Page-align the text segment start in the file.
    let text_offset = ((phdr_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let text_size = code.len() as u64;

    // The data segment starts on the next page after the text.
    let text_file_end = text_offset + text_size;
    let data_vaddr = ((base_addr + text_file_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let data_offset = data_vaddr - base_addr; // file offset for data segment
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
    elf.extend_from_slice(&258u16.to_le_bytes()); // e_machine = EM_LOONGARCH
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_shoff (no section headers)
    elf.extend_from_slice(&0x43u32.to_le_bytes()); // e_flags = 0x43 (LP64D ABI double-float)
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_phnum = 2
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    // p_filesz = actual code size, p_memsz = page-aligned (QEMU needs this for mapping)
    let text_memsz = ((text_size + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_memsz.to_le_bytes()); // p_filesz (cover full page for QEMU)
    elf.extend_from_slice(&text_memsz.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data / stack ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&data_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_filesz (write zeros so QEMU can mmap)
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_memsz (writable pages)
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- .text section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // --- Pad to data segment offset and write data segment ---
    while (elf.len() as u64) < data_offset {
        elf.push(0);
    }
    // Write data segment (all zeros, but must exist in file for QEMU mmap)
    elf.extend_from_slice(&vec![0u8; data_size as usize]);

    elf
}

/// Patch a 4-instruction load-immediate sequence in `code` starting at `offset`
/// with the 64-bit value `val`.
///
/// The sequence is:
///   lu12i.w rd, bits[31:12]   — sets bits[31:12] and sign-extends to 64 bits
///   ori     rd, rd, bits[11:0] — sets bits[11:0]
///   lu32i.d rd, bits[51:32]   — sets bits[51:32]
///   lu52i.d rd, rd, bits[63:52] — sets bits[63:52]
///
/// The `rd` register is extracted from the existing first instruction.
fn patch_load_imm_64(code: &mut [u8], offset: usize, val: u64) {
    // Extract rd from the first instruction (lu12i.w): rd is at bits[4:0]
    let word0 = u32::from_le_bytes([code[offset], code[offset + 1], code[offset + 2], code[offset + 3]]);
    let rd_enc = (word0 & 0x1F) as u32;
    let rd = Gpr::from_encoding(rd_enc);

    // Re-encode all 4 instructions with the new value

    // Step 1: lu12i.w rd, bits[31:12]
    let hi20 = ((val >> 12) & 0xFFFFF) as i32;
    let new_word0 = u32::from_le_bytes(Instruction::Lu12iW { rd, imm20: hi20 }.encode());
    code[offset..offset + 4].copy_from_slice(&new_word0.to_le_bytes());

    // Step 2: ori rd, rd, bits[11:0]
    let lo12 = (val & 0xFFF) as u32;
    let new_word1 = u32::from_le_bytes(Instruction::Ori { rd, rj: rd, imm12: lo12 }.encode());
    code[offset + 4..offset + 8].copy_from_slice(&new_word1.to_le_bytes());

    // Step 3: lu32i.d rd, bits[51:32]
    let hi32 = ((val >> 32) & 0xFFFFF) as i32;
    let new_word2 = u32::from_le_bytes(Instruction::Lu32iD { rd, imm20: hi32 }.encode());
    code[offset + 8..offset + 12].copy_from_slice(&new_word2.to_le_bytes());

    // Step 4: lu52i.d rd, rd, bits[63:52]
    let hi52 = ((val >> 52) & 0xFFF) as i32;
    let new_word3 = u32::from_le_bytes(Instruction::Lu52iD { rd, rj: rd, imm12: hi52 }.encode());
    code[offset + 12..offset + 16].copy_from_slice(&new_word3.to_le_bytes());
}

// ===========================================================================
// LoongArch64Backend
// ===========================================================================

/// Decode a single LoongArch64 32-bit instruction word into a mnemonic string.
fn decode_loongarch64_instruction(word: u32) -> String {
    // Check higher-bit opcodes first (more specific patterns)

    // ── 2RI12 format: 10-bit opcode at bits[31:22] ──
    let opc_2ri12 = (word >> 22) & 0x3FF;
    match opc_2ri12 {
        0x0E7 => {
            // DBAR hint: rd=$r0, rj=$r0, hint=si12
            let hint = (word >> 10) & 0xFFF;
            return format!("dbar {}", hint);
        }
        _ => {}
    }

    // ── 3R format: 17-bit opcode at bits[31:15] for atomic memory ops ──
    let opc_3r_17 = (word >> 15) & 0x1FFFF;
    match opc_3r_17 {
        0x00C0 => {
            let rd = word & 0x1F;
            let rj = (word >> 5) & 0x1F;
            let rk = (word >> 10) & 0x1F;
            return format!("amswap.w $r{}, $r{}, $r{}", rd, rj, rk);
        }
        0x00C2 => {
            let rd = word & 0x1F;
            let rj = (word >> 5) & 0x1F;
            let rk = (word >> 10) & 0x1F;
            return format!("amswap.d $r{}, $r{}, $r{}", rd, rj, rk);
        }
        _ => {}
    }

    // ── 2RI14 format: 8-bit opcode at bits[31:24] for LL/SC ──
    let opc_2ri14 = (word >> 24) & 0xFF;
    match opc_2ri14 {
        0x20 => {
            let rd = word & 0x1F;
            let rj = (word >> 5) & 0x1F;
            let imm14 = ((word >> 10) as i32) << 18 >> 18;
            return format!("ll.w $r{}, $r{}, {}", rd, rj, imm14);
        }
        0x21 => {
            let rd = word & 0x1F;
            let rj = (word >> 5) & 0x1F;
            let imm14 = ((word >> 10) as i32) << 18 >> 18;
            return format!("sc.w $r{}, $r{}, {}", rd, rj, imm14);
        }
        0x22 => {
            let rd = word & 0x1F;
            let rj = (word >> 5) & 0x1F;
            let imm14 = ((word >> 10) as i32) << 18 >> 18;
            return format!("ll.d $r{}, $r{}, {}", rd, rj, imm14);
        }
        0x23 => {
            let rd = word & 0x1F;
            let rj = (word >> 5) & 0x1F;
            let imm14 = ((word >> 10) as i32) << 18 >> 18;
            return format!("sc.d $r{}, $r{}, {}", rd, rj, imm14);
        }
        _ => {}
    }

    // ── Fall back to simplified 7-bit opcode matching ──
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
    Gpr::T0,
    Gpr::T1,
    Gpr::T2,
    Gpr::T3,
    Gpr::T4,
    Gpr::T5,
    Gpr::T6,
    Gpr::T7,
    Gpr::T8,
    // Argument registers (also caller-saved)
    Gpr::A0,
    Gpr::A1,
    Gpr::A2,
    Gpr::A3,
    Gpr::A4,
    Gpr::A5,
    Gpr::A6,
    Gpr::A7,
    // Callee-saved (require save/restore)
    Gpr::S0,
    Gpr::S1,
    Gpr::S2,
    Gpr::S3,
    Gpr::S4,
    Gpr::S5,
    Gpr::S6,
    Gpr::S7,
    Gpr::S8,
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

/// Load a 64-bit immediate into a register using the lu12i.w/ori/lu32i.d/lu52i.d
/// instruction sequence.
///
/// Returns the list of AllocatedInstructions emitted to perform the load.
fn load_imm_la64(rd: Gpr, imm: i64) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();
    let val = imm as u64;

    // lu12i.w rd, bits[31:12]
    let hi20 = ((val >> 12) & 0xFFFFF) as i32;
    result.push(emit_alloc_instr(
        Instruction::Lu12iW { rd, imm20: hi20 },
        vec![],
        vec![PhysicalReg::new(RegClass::Gpr, rd.encoding())],
    ));

    // ori rd, rd, bits[11:0]
    let lo12 = (val & 0xFFF) as u32;
    result.push(emit_alloc_instr(
        Instruction::Ori {
            rd,
            rj: rd,
            imm12: lo12,
        },
        vec![PhysicalReg::new(RegClass::Gpr, rd.encoding())],
        vec![PhysicalReg::new(RegClass::Gpr, rd.encoding())],
    ));

    // After LU12I.W + ORI, rd = sign_extend(val[31:0]) to 64 bits.
    // This is correct only if the upper 32 bits of val match the sign extension
    // of the lower 32 bits. Otherwise we need LU32I.D and/or LU52I.D.

    let lower32 = val as u32;
    let sign_extended_upper: u32 = if lower32 & 0x80000000 != 0 {
        0xFFFFFFFF
    } else {
        0x00000000
    };

    if (val >> 32) as u32 != sign_extended_upper {
        // Upper bits don't match sign extension — need LU32I.D and/or LU52I.D.

        // Step 3: LU32I.D rd, bits[51:32]
        // Sets bits[51:32] of rd to the immediate; bits[63:52] and bits[31:0] unchanged.
        let bits_51_32 = ((val >> 32) & 0xFFFFF) as i32;
        let sign_ext_bits_51_32: u32 = if lower32 & 0x80000000 != 0 { 0xFFFFF } else { 0 };
        if (bits_51_32 as u32) != sign_ext_bits_51_32 {
            result.push(emit_alloc_instr(
                Instruction::Lu32iD { rd, imm20: bits_51_32 },
                vec![PhysicalReg::new(RegClass::Gpr, rd.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, rd.encoding())],
            ));
        }

        // Step 4: LU52I.D rd, rd, bits[63:52]
        // Sets bits[63:52] of rd to the immediate; other bits unchanged.
        // After LU12I.W + ORI (+ optional LU32I.D), bits[63:52] are still from the
        // sign extension of the lower 32 bits, so we need to check if they're correct.
        let bits_63_52 = ((val >> 52) & 0xFFF) as i32;
        let sign_ext_bits_63_52: i32 = if lower32 & 0x80000000 != 0 { 0xFFF } else { 0 };
        if bits_63_52 != sign_ext_bits_63_52 {
            result.push(emit_alloc_instr(
                Instruction::Lu52iD {
                    rd,
                    rj: rd,
                    imm12: bits_63_52,
                },
                vec![PhysicalReg::new(RegClass::Gpr, rd.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, rd.encoding())],
            ));
        }
    }

    result
}

/// Resolve an IRValue to a physical GPR.
///
/// - For `IRValue::Register`: looks up in `vreg_map`.
/// - For `IRValue::Immediate`: loads the value into `scratch` and returns
///   `(scratch, instructions)`.
/// - For `IRValue::Address`: loads the address into `scratch`.
/// - For `IRValue::Label`: loads a placeholder into `scratch`.
fn resolve_gpr_la64(
    val: &IRValue,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
    scratch: Gpr,
) -> (Gpr, Vec<AllocatedInstruction>) {
    match val {
        IRValue::Register(id) => {
            let reg = map_vreg_to_gpr(*id, None, vreg_map);
            (reg, Vec::new())
        }
        IRValue::Immediate(imm) => {
            let code = load_imm_la64(scratch, *imm);
            (scratch, code)
        }
        IRValue::Address(addr) => {
            let code = load_imm_la64(scratch, *addr as i64);
            (scratch, code)
        }
        IRValue::Label(_) => {
            // Labels need relocation; emit a placeholder load
            let code = load_imm_la64(scratch, 0);
            (scratch, code)
        }
    }
}

/// Check if an immediate value fits in a signed 12-bit range (for addi.d/slti/etc.).
fn fits_si12(val: i64) -> bool {
    (-2048..=2047).contains(&val)
}

/// Emit a single AllocatedInstruction from a LoongArch64 Instruction.
fn emit_alloc_instr(
    inst: Instruction,
    reads: Vec<PhysicalReg>,
    writes: Vec<PhysicalReg>,
) -> AllocatedInstruction {
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
                Instruction::Xor {
                    rd: dst,
                    rj: lhs,
                    rk: rhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Sltui {
                    rd: dst,
                    rj: dst,
                    imm12: 1,
                },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::Ne => {
            // xor dst, lhs, rhs; sltu dst, $r0, dst
            result.push(emit_alloc_instr(
                Instruction::Xor {
                    rd: dst,
                    rj: lhs,
                    rk: rhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Sltu {
                    rd: dst,
                    rj: Gpr::R0,
                    rk: dst,
                },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SLt => {
            result.push(emit_alloc_instr(
                Instruction::Slt {
                    rd: dst,
                    rj: lhs,
                    rk: rhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SLe => {
            // slt dst, rhs, lhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Slt {
                    rd: dst,
                    rj: rhs,
                    rk: lhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori {
                    rd: dst,
                    rj: dst,
                    imm12: 1,
                },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SGt => {
            // slt dst, rhs, lhs
            result.push(emit_alloc_instr(
                Instruction::Slt {
                    rd: dst,
                    rj: rhs,
                    rk: lhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::SGe => {
            // slt dst, lhs, rhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Slt {
                    rd: dst,
                    rj: lhs,
                    rk: rhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori {
                    rd: dst,
                    rj: dst,
                    imm12: 1,
                },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::ULt => {
            result.push(emit_alloc_instr(
                Instruction::Sltu {
                    rd: dst,
                    rj: lhs,
                    rk: rhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::ULe => {
            // sltu dst, rhs, lhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Sltu {
                    rd: dst,
                    rj: rhs,
                    rk: lhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori {
                    rd: dst,
                    rj: dst,
                    imm12: 1,
                },
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::UGt => {
            result.push(emit_alloc_instr(
                Instruction::Sltu {
                    rd: dst,
                    rj: rhs,
                    rk: lhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
        }
        CmpKind::UGe => {
            // sltu dst, lhs, rhs; xori dst, dst, 1
            result.push(emit_alloc_instr(
                Instruction::Sltu {
                    rd: dst,
                    rj: lhs,
                    rk: rhs,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::Xori {
                    rd: dst,
                    rj: dst,
                    imm12: 1,
                },
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
        other => unreachable!("BinOpKind::{:?} is not a comparison", other),
    };
    lower_cmp_la64(&kind, dst, lhs, rhs)
}

/// Lower a BinOp to LoongArch64 instructions with immediate-form optimisations.
fn lower_binop_la64(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();
    let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
    let (l, code) = resolve_gpr_la64(lhs, vreg_map, Gpr::T0);
    result.extend(code);

    match op {
        BinOpKind::Add => {
            if let IRValue::Immediate(imm) = rhs {
                if fits_si12(*imm) {
                    // addi.d dst, l, imm
                    result.push(emit_alloc_instr(
                        Instruction::AddiD {
                            rd: dst_reg,
                            rj: l,
                            imm12: *imm as i32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::AddD {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::AddD {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::Sub => {
            if let IRValue::Immediate(imm) = rhs {
                if fits_si12(*imm) {
                    // addi.d dst, l, -imm
                    result.push(emit_alloc_instr(
                        Instruction::AddiD {
                            rd: dst_reg,
                            rj: l,
                            imm12: -(*imm as i32),
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::SubD {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::SubD {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::Mul => {
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::MulD {
                    rd: dst_reg,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::SDiv => {
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::DivD {
                    rd: dst_reg,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::UDiv => {
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            // LoongArch64 div.du is not in our instruction enum; use div.d as approximation
            result.push(emit_alloc_instr(
                Instruction::DivD {
                    rd: dst_reg,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::SRem => {
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::ModD {
                    rd: dst_reg,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::URem => {
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::ModD {
                    rd: dst_reg,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::And => {
            if let IRValue::Immediate(imm) = rhs {
                if (*imm as u64) & 0xFFF == *imm as u64 && *imm >= 0 {
                    // andi dst, l, imm12 (unsigned 12-bit)
                    result.push(emit_alloc_instr(
                        Instruction::Andi {
                            rd: dst_reg,
                            rj: l,
                            imm12: *imm as u32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::And {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::And {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::Or => {
            if let IRValue::Immediate(imm) = rhs {
                if (*imm as u64) & 0xFFF == *imm as u64 && *imm >= 0 {
                    result.push(emit_alloc_instr(
                        Instruction::Ori {
                            rd: dst_reg,
                            rj: l,
                            imm12: *imm as u32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::Or {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::Or {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::Xor => {
            if let IRValue::Immediate(imm) = rhs {
                if *imm == -1 {
                    // xori dst, l, -1 => NOT via xori with 0xFFF
                    result.push(emit_alloc_instr(
                        Instruction::Xori {
                            rd: dst_reg,
                            rj: l,
                            imm12: 0xFFF,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else if (*imm as u64) & 0xFFF == *imm as u64 && *imm >= 0 {
                    result.push(emit_alloc_instr(
                        Instruction::Xori {
                            rd: dst_reg,
                            rj: l,
                            imm12: *imm as u32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::Xor {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::Xor {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::Shl => {
            if let IRValue::Immediate(imm) = rhs {
                if *imm >= 0 && *imm < 64 {
                    result.push(emit_alloc_instr(
                        Instruction::SlliD {
                            rd: dst_reg,
                            rj: l,
                            imm8: *imm as u32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::SllD {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::SllD {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::ShrL => {
            if let IRValue::Immediate(imm) = rhs {
                if *imm >= 0 && *imm < 64 {
                    result.push(emit_alloc_instr(
                        Instruction::SrliD {
                            rd: dst_reg,
                            rj: l,
                            imm8: *imm as u32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::SrlD {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::SrlD {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::ShrA => {
            if let IRValue::Immediate(imm) = rhs {
                if *imm >= 0 && *imm < 64 {
                    result.push(emit_alloc_instr(
                        Instruction::SraiD {
                            rd: dst_reg,
                            rj: l,
                            imm8: *imm as u32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::SraD {
                            rd: dst_reg,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::SraD {
                        rd: dst_reg,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                ));
            }
        }
        BinOpKind::Ror => {
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::RotrD {
                    rd: dst_reg,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        BinOpKind::Rol => {
            // ROL(x, n) = ROTR(x, 64-n)
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            // Compute 64-n in T2: ADDI.D T2, $r0, 64; SUB.D T2, T2, r
            result.push(emit_alloc_instr(
                Instruction::AddiD { rd: Gpr::T2, rj: Gpr::R0, imm12: 64 },
                vec![],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::SubD { rd: Gpr::T2, rj: Gpr::T2, rk: r },
                vec![
                    PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding())],
            ));
            result.push(emit_alloc_instr(
                Instruction::RotrD {
                    rd: dst_reg,
                    rj: l,
                    rk: Gpr::T2,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
            ));
        }
        // Comparison BinOps
        BinOpKind::SLt
        | BinOpKind::SLe
        | BinOpKind::SGt
        | BinOpKind::SGe
        | BinOpKind::ULt
        | BinOpKind::ULe
        | BinOpKind::UGt
        | BinOpKind::UGe
        | BinOpKind::Eq
        | BinOpKind::Ne => {
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.extend(lower_binop_cmp_la64(op, dst_reg, l, r));
        }
    }
    result
}

/// Helper: compute the current byte offset of generated code (each instruction = 4 bytes).
fn current_code_offset(instrs: &[AllocatedInstruction]) -> usize {
    instrs.len() * 4
}

/// Lower an IR instruction to a sequence of LoongArch64 AllocatedInstructions.
/// `relocations` is populated with relocation entries for Call instructions.
/// `alloc_offsets` maps Alloc virtual register IDs to their stack offset from $sp.
fn lower_ir_instr_la64(
    instr: &IRInstr,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
    relocations: &mut Vec<RelocationEntry>,
    alloc_offsets: &HashMap<u32, i32>,
) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();

    match instr {
        IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
            result.extend(lower_binop_la64(op, dst, lhs, rhs, vreg_map));
        }

        IRInstr::Add { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (l, code) = resolve_gpr_la64(lhs, vreg_map, Gpr::T0);
            result.extend(code);
            if let IRValue::Immediate(imm) = rhs {
                if fits_si12(*imm) {
                    result.push(emit_alloc_instr(
                        Instruction::AddiD {
                            rd: d,
                            rj: l,
                            imm12: *imm as i32,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::AddD {
                            rd: d,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::AddD {
                        rd: d,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                ));
            }
        }

        IRInstr::Sub { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (l, code) = resolve_gpr_la64(lhs, vreg_map, Gpr::T0);
            result.extend(code);
            if let IRValue::Immediate(imm) = rhs {
                if fits_si12(*imm) {
                    // sub dst, l, imm => addi.d dst, l, -imm
                    result.push(emit_alloc_instr(
                        Instruction::AddiD {
                            rd: d,
                            rj: l,
                            imm12: -(*imm as i32),
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, l.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                } else {
                    let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                    result.extend(pre);
                    result.push(emit_alloc_instr(
                        Instruction::SubD {
                            rd: d,
                            rj: l,
                            rk: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
            } else {
                let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
                result.extend(pre);
                result.push(emit_alloc_instr(
                    Instruction::SubD {
                        rd: d,
                        rj: l,
                        rk: r,
                    },
                    vec![
                        PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    ],
                    vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                ));
            }
        }

        IRInstr::Mul { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (l, code) = resolve_gpr_la64(lhs, vreg_map, Gpr::T0);
            result.extend(code);
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::MulD {
                    rd: d,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Div { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (l, code) = resolve_gpr_la64(lhs, vreg_map, Gpr::T0);
            result.extend(code);
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::DivD {
                    rd: d,
                    rj: l,
                    rk: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (s, code) = resolve_gpr_la64(operand, vreg_map, Gpr::T0);
            result.extend(code);
            match op {
                UnaryOpKind::Neg => {
                    // sub.d d, $r0, s
                    result.push(emit_alloc_instr(
                        Instruction::SubD {
                            rd: d,
                            rj: Gpr::R0,
                            rk: s,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Not => {
                    // nor d, $r0, s
                    result.push(emit_alloc_instr(
                        Instruction::Nor {
                            rd: d,
                            rj: Gpr::R0,
                            rk: s,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Clz => {
                    // Count leading zeros using clo.d + adjustment
                    // LoongArch has no direct clz; use clo.d (count leading ones) on
                    // the inverted value: clz(x) = clo(~x)
                    // Emit: nor d, $r0, s; then clo.d d, d
                    result.push(emit_alloc_instr(
                        Instruction::Nor {
                            rd: d,
                            rj: Gpr::R0,
                            rk: s,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                    result.push(emit_alloc_instr(
                        Instruction::CloD { rd: d, rj: d },
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    // Placeholder: move operand to dst
                    result.push(emit_alloc_instr(
                        Instruction::AddD {
                            rd: d,
                            rj: s,
                            rk: Gpr::R0,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
            }
        }

        IRInstr::Cmp {
            kind,
            dst,
            lhs,
            rhs, ty: _,
        } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (l, code) = resolve_gpr_la64(lhs, vreg_map, Gpr::T0);
            result.extend(code);
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            result.extend(lower_cmp_la64(kind, d, l, r));
        }

        IRInstr::Load { dst, addr, offset, ty } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (a, code) = resolve_gpr_la64(addr, vreg_map, Gpr::T0);
            result.extend(code);
            let off = *offset;
            let load_instr = match ty {
                IRType::I8 => Instruction::LdB { rd: d, rj: a, imm12: off },
                IRType::U8 => Instruction::LdBu { rd: d, rj: a, imm12: off },
                IRType::I16 => Instruction::LdH { rd: d, rj: a, imm12: off },
                IRType::U16 => Instruction::LdHu { rd: d, rj: a, imm12: off },
                IRType::I32 => Instruction::LdW { rd: d, rj: a, imm12: off },
                IRType::U32 => Instruction::LdWu { rd: d, rj: a, imm12: off },
                _ => Instruction::LdD { rd: d, rj: a, imm12: off },
            };
            result.push(emit_alloc_instr(
                load_instr,
                vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Store { value, addr, offset, ty } => {
            let (v, code) = resolve_gpr_la64(value, vreg_map, Gpr::T0);
            result.extend(code);
            let (a, pre) = resolve_gpr_la64(addr, vreg_map, Gpr::T1);
            result.extend(pre);
            let off = *offset;
            let store_instr = match ty {
                IRType::I8 | IRType::U8 => Instruction::StB { rd: v, rj: a, imm12: off },
                IRType::I16 | IRType::U16 => Instruction::StH { rd: v, rj: a, imm12: off },
                IRType::I32 | IRType::U32 => Instruction::StW { rd: v, rj: a, imm12: off },
                _ => Instruction::StD { rd: v, rj: a, imm12: off },
            };
            result.push(emit_alloc_instr(
                store_instr,
                vec![
                    PhysicalReg::new(RegClass::Gpr, a.encoding()),
                    PhysicalReg::new(RegClass::Gpr, v.encoding()),
                ],
                vec![],
            ));
        }

        IRInstr::Alloc { dst, size: _ } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            // The prologue already decremented SP by the full frame_size which
            // includes all Alloc sizes. Each Alloc has a pre-computed offset
            // from $sp stored in alloc_offsets. Just compute dst = $sp + offset.
            if let Some(&off) = alloc_offsets.get(&vreg_id(dst)) {
                if off == 0 {
                    // dst = $sp (addi.d d, $sp, 0 or add.d d, $sp, $r0)
                    if d != Gpr::Sp {
                        result.push(emit_alloc_instr(
                            Instruction::AddiD {
                                rd: d,
                                rj: Gpr::Sp,
                                imm12: 0,
                            },
                            vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                            vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                        ));
                    }
                } else if fits_si12(off as i64) {
                    result.push(emit_alloc_instr(
                        Instruction::AddiD {
                            rd: d,
                            rj: Gpr::Sp,
                            imm12: off,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                } else {
                    // Large offset: load into scratch and add
                    let offset_code = load_imm_la64(Gpr::T0, off as i64);
                    for instr in offset_code {
                        result.push(instr);
                    }
                    result.push(emit_alloc_instr(
                        Instruction::AddD {
                            rd: d,
                            rj: Gpr::Sp,
                            rk: Gpr::T0,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()),
                            PhysicalReg::new(RegClass::Gpr, Gpr::T0.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
            } else {
                // Fallback: just use $sp
                if d != Gpr::Sp {
                    result.push(emit_alloc_instr(
                        Instruction::AddiD {
                            rd: d,
                            rj: Gpr::Sp,
                            imm12: 0,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
            }
        }

        IRInstr::Ret { values } => {
            // Move return value to $a0 if present
            if let Some(val) = values.first() {
                let (v, code) = resolve_gpr_la64(val, vreg_map, Gpr::T0);
                result.extend(code);
                if v != Gpr::A0 {
                    result.push(emit_alloc_instr(
                        Instruction::AddD {
                            rd: Gpr::A0,
                            rj: v,
                            rk: Gpr::R0,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, v.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::A0.encoding())],
                    ));
                }
            }
            // jirl $r0, $ra, 0 (return)
            result.push(emit_alloc_instr(
                Instruction::Jirl {
                    rd: Gpr::R0,
                    rj: Gpr::Ra,
                    offs16: 0,
                },
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
                vec![],
            ));
        }

        IRInstr::Call { dst, func: target_name, args, is_extern: _ } => {
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    let (a, code) = resolve_gpr_la64(arg, vreg_map, Gpr::T0);
                    result.extend(code);
                    if a != arg_reg {
                        result.push(emit_alloc_instr(
                            Instruction::AddD {
                                rd: arg_reg,
                                rj: a,
                                rk: Gpr::R0,
                            },
                            vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                            vec![PhysicalReg::new(RegClass::Gpr, arg_reg.encoding())],
                        ));
                    }
                }
            }
            // BL — record a relocation for later patching
            let bl_byte_offset = current_code_offset(&result);
            result.push(emit_alloc_instr(
                Instruction::Bl { offs26: 0 },
                vec![],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
            ));
            relocations.push(RelocationEntry {
                offset: bl_byte_offset as u64,
                symbol: target_name.clone(),
                reloc_type: "R_LARCH_B26".to_string(),
            });
            // Move return value from $a0 to dst
            if let Some(d) = dst {
                let d_reg = map_vreg_to_gpr(vreg_id(d), None, vreg_map);
                if d_reg != Gpr::A0 {
                    result.push(emit_alloc_instr(
                        Instruction::AddD {
                            rd: d_reg,
                            rj: Gpr::A0,
                            rk: Gpr::R0,
                        },
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

        IRInstr::CondBranch {
            cond,
            true_target: _,
            false_target: _,
        } => {
            let (c, code) = resolve_gpr_la64(cond, vreg_map, Gpr::T0);
            result.extend(code);
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
            let (s, code) = resolve_gpr_la64(src, vreg_map, Gpr::T0);
            result.extend(code);
            if d != s {
                result.push(emit_alloc_instr(
                    Instruction::AddD {
                        rd: d,
                        rj: s,
                        rk: Gpr::R0,
                    },
                    vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                    vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                ));
            }
        }

        IRInstr::Select {
            dst,
            cond,
            true_val,
            false_val, ty: _,
        } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (c, code) = resolve_gpr_la64(cond, vreg_map, Gpr::T0);
            result.extend(code);
            let (fv, pre_fv) = resolve_gpr_la64(false_val, vreg_map, Gpr::T1);
            result.extend(pre_fv);
            let (tv, pre_tv) = resolve_gpr_la64(true_val, vreg_map, Gpr::T2);
            result.extend(pre_tv);
            // Move false_val to dst; beqz cond, +8; move true_val to dst
            if fv != d {
                result.push(emit_alloc_instr(
                    Instruction::AddD {
                        rd: d,
                        rj: fv,
                        rk: Gpr::R0,
                    },
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
                Instruction::AddD {
                    rd: d,
                    rj: tv,
                    rk: Gpr::R0,
                },
                vec![PhysicalReg::new(RegClass::Gpr, tv.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        // Constant-time conditional select (NO BRANCHES)
        // ct_select(cond, a, b) = (a & mask) | (b & ~mask)
        // mask = -(cond != 0): all-ones if cond!=0, else 0
        IRInstr::CtSelect {
            dst, cond, true_val, false_val, ..
        } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (c, code) = resolve_gpr_la64(cond, vreg_map, Gpr::T0);
            result.extend(code);
            let (fv, pre_fv) = resolve_gpr_la64(false_val, vreg_map, Gpr::T1);
            result.extend(pre_fv);
            let (tv, pre_tv) = resolve_gpr_la64(true_val, vreg_map, Gpr::T2);
            result.extend(pre_tv);
            // Build mask = -(cond != 0)
            // sltui T3, c, 1 → T3 = (c == 0) ? 1 : 0
            result.push(emit_alloc_instr(
                Instruction::Sltui { rd: Gpr::T3, rj: c, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, c.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
            ));
            // xori T3, T3, 1 → T3 = (c != 0) ? 1 : 0
            result.push(emit_alloc_instr(
                Instruction::Xori { rd: Gpr::T3, rj: Gpr::T3, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
            ));
            // sub.d T3, $r0, T3 → T3 = -T3 = mask (all-ones or 0)
            result.push(emit_alloc_instr(
                Instruction::SubD { rd: Gpr::T3, rj: Gpr::R0, rk: Gpr::T3 },
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
            ));
            // and T2, T2, T3 → true_val & mask
            result.push(emit_alloc_instr(
                Instruction::And { rd: Gpr::T2, rj: tv, rk: Gpr::T3 },
                vec![PhysicalReg::new(RegClass::Gpr, tv.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding())],
            ));
            // nor T3, $r0, T3 → ~mask
            result.push(emit_alloc_instr(
                Instruction::Nor { rd: Gpr::T3, rj: Gpr::R0, rk: Gpr::T3 },
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
            ));
            // and T1, T1, T3 → false_val & ~mask
            result.push(emit_alloc_instr(
                Instruction::And { rd: Gpr::T1, rj: fv, rk: Gpr::T3 },
                vec![PhysicalReg::new(RegClass::Gpr, fv.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::T3.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T1.encoding())],
            ));
            // or d, T1, T2 → result
            result.push(emit_alloc_instr(
                Instruction::Or { rd: d, rj: Gpr::T1, rk: Gpr::T2 },
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T1.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        // Constant-time equality check (NO BRANCHES)
        // ct_eq(a, b): diff = a ^ b; result = ((diff | -diff) >> 31) ^ 1
        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (l, code) = resolve_gpr_la64(lhs, vreg_map, Gpr::T0);
            result.extend(code);
            let (r, pre) = resolve_gpr_la64(rhs, vreg_map, Gpr::T1);
            result.extend(pre);
            // xor d, l, r → diff
            result.push(emit_alloc_instr(
                Instruction::Xor { rd: d, rj: l, rk: r },
                vec![PhysicalReg::new(RegClass::Gpr, l.encoding()), PhysicalReg::new(RegClass::Gpr, r.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
            // sub.d T2, $r0, d → -diff
            result.push(emit_alloc_instr(
                Instruction::SubD { rd: Gpr::T2, rj: Gpr::R0, rk: d },
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding())],
            ));
            // or d, d, T2 → (diff | -diff)
            result.push(emit_alloc_instr(
                Instruction::Or { rd: d, rj: d, rk: Gpr::T2 },
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding()), PhysicalReg::new(RegClass::Gpr, Gpr::T2.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
            // srli.d d, d, 31 → 0 if diff==0, 1 if diff!=0
            result.push(emit_alloc_instr(
                Instruction::SrliD { rd: d, rj: d, imm8: 31 },
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
            // xori d, d, 1 → invert: 1 if equal, 0 if not
            result.push(emit_alloc_instr(
                Instruction::Xori { rd: d, rj: d, imm12: 1 },
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Offset { dst, base, offset } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let (b, code) = resolve_gpr_la64(base, vreg_map, Gpr::T0);
            result.extend(code);
            let (o, pre) = resolve_gpr_la64(offset, vreg_map, Gpr::T1);
            result.extend(pre);
            result.push(emit_alloc_instr(
                Instruction::AddD {
                    rd: d,
                    rj: b,
                    rk: o,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, b.encoding()),
                    PhysicalReg::new(RegClass::Gpr, o.encoding()),
                ],
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

        IRInstr::Free { ptr: _ } => {
            // Free is not directly implementable as a single instruction;
            // emit a break to catch any accidental execution at runtime.
            result.push(emit_alloc_instr(Instruction::Break, vec![], vec![]));
        }

        IRInstr::Phi { .. } => {
            // Phi nodes are eliminated by SSA deconstruction; emit NOP.
            result.push(emit_alloc_instr(Instruction::Nop, vec![], vec![]));
        }

        // Atomic operations — lower as non-atomic (single-threaded)
        IRInstr::AtomicLoad { dst, addr, ty } => {
            let ir_load = IRInstr::Load { dst: dst.clone(), addr: addr.clone(), offset: 0, ty: ty.clone() };
            let sub_result = lower_ir_instr_la64(&ir_load, vreg_map, relocations, alloc_offsets);
            result.extend(sub_result);
        }
        IRInstr::AtomicStore { value, addr, ty } => {
            let ir_store = IRInstr::Store { value: value.clone(), addr: addr.clone(), offset: 0, ty: ty.clone() };
            let sub_result = lower_ir_instr_la64(&ir_store, vreg_map, relocations, alloc_offsets);
            result.extend(sub_result);
        }
        IRInstr::AtomicCas { dst, addr, expected, desired, ty } => {
            // Placeholder: lower as a simple load
            let ir_load = IRInstr::Load { dst: dst.clone(), addr: addr.clone(), offset: 0, ty: ty.clone() };
            let sub_result = lower_ir_instr_la64(&ir_load, vreg_map, relocations, alloc_offsets);
            result.extend(sub_result);
            let _ = (expected, desired);
        }
    }

    result
}

impl Backend for LoongArch64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        reg_alloc_isel::allocate_registers(func)
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
        const R_LARCH_B26: &str = "R_LARCH_B26";
        const R_LARCH_64: &str = "R_LARCH_64";

        // ── LoongArch64 Linux static executable ──
        //
        // Layout:
        //   _start:  BL main           ; call main (result in $a0)
        //            addi.d $a7, $r0, 93 ; sys_exit = 93
        //            syscall 0x0        ; exit(main_result)
        //   <functions...>
        //
        // The _start stub is 3 instructions = 12 bytes.
        // After that come all user functions.

        let start_stub_size: usize = 12; // 3 × 4-byte instructions

        // ── Compute function offsets ──
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size; // after _start

        for func in &program.functions {
            func_offsets.insert(func.name.clone(), current_offset);
            let func_size: usize = func
                .blocks
                .iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            current_offset += func_size;
        }

        // ── Build _start stub bytes ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // BL <main> — placeholder, will be patched
        start_stub.extend_from_slice(&Instruction::Bl { offs26: 0 }.encode());

        // addi.d $a7, $r0, 93 (sys_exit = 93)
        start_stub.extend_from_slice(
            &Instruction::AddiD {
                rd: Gpr::A7,
                rj: Gpr::R0,
                imm12: 93,
            }
            .encode(),
        );

        // syscall 0x0
        start_stub.extend_from_slice(&Instruction::Syscall.encode());

        // ── Patch _start BL to main ──
        let main_key = func_offsets
            .keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            let bl_offset = (main_offset as i64) / 4;
            // Re-encode the whole BL instruction
            let patched_word =
                u32::from_le_bytes(Instruction::Bl { offs26: bl_offset as i32 }.encode());
            start_stub[0..4].copy_from_slice(&patched_word.to_le_bytes());
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

        // ── Compute code virtual-address base ──
        // Must match the layout in build_loongarch64_elf_2seg.
        const ELF_BASE_ADDR: u64 = 0x120000000;
        const ELF_PAGE_SIZE: u64 = 0x10000;
        const ELF_HEADER_SIZE: u64 = 64;
        const ELF_PHDR_SIZE: u64 = 56;
        const ELF_NUM_PHDRS: u64 = 2;
        let phdr_end = ELF_HEADER_SIZE + ELF_NUM_PHDRS * ELF_PHDR_SIZE;
        let text_offset = ((phdr_end + ELF_PAGE_SIZE - 1) / ELF_PAGE_SIZE) * ELF_PAGE_SIZE;
        let code_vaddr_base = ELF_BASE_ADDR + text_offset;

        // ── Patch relocations ──
        let mut func_code_offset: usize = start_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;

                if reloc.reloc_type == R_LARCH_B26 {
                    // R_LARCH_B26: patch BL instruction's offs26 field.
                    // BL target = PC + SignExtend(offs26) * 4
                    // So: offs26 = (target_addr - bl_addr) / 4
                    if abs_offset + 4 > all_code.len() {
                        continue; // skip invalid relocations
                    }
                    if let Some(&target_offset) = func_offsets.get(&reloc.symbol) {
                        let bl_addr = abs_offset as i64;
                        let target_addr = target_offset as i64;
                        let offset_words = (target_addr - bl_addr) / 4;
                        // Check range: ±128MB (26-bit signed * 4)
                        if offset_words < -(1i64 << 25) || offset_words >= (1i64 << 25) {
                            eprintln!(
                                "warning: BL relocation to '{}' out of range: {} words",
                                reloc.symbol, offset_words
                            );
                            continue;
                        }
                        // Re-encode the BL with the correct offset
                        let patched =
                            u32::from_le_bytes(Instruction::Bl { offs26: offset_words as i32 }
                                .encode());
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&patched.to_le_bytes());
                    }
                } else if reloc.reloc_type == R_LARCH_64 {
                    // R_LARCH_64: patch the 4-instruction load-immediate sequence
                    // (lu12i.w + ori + lu32i.d + lu52i.d = 16 bytes) with an
                    // absolute 64-bit address.
                    if abs_offset + 16 > all_code.len() {
                        eprintln!(
                            "warning: R_LARCH_64 relocation at offset {} overflows code (len {})",
                            abs_offset, all_code.len()
                        );
                        continue;
                    }
                    if let Some(&target_offset) = func_offsets.get(&reloc.symbol) {
                        let vaddr = code_vaddr_base + target_offset as u64;
                        patch_load_imm_64(&mut all_code, abs_offset, vaddr);
                    }
                }
            }
            let func_size: usize = func
                .blocks
                .iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            func_code_offset += func_size;
        }

        // ── Build ELF with 2 LOAD segments ──
        Ok(build_loongarch64_elf_2seg(&all_code, ELF_BASE_ADDR))
    }

    fn return_stub(&self) -> Vec<u8> {
        // jirl $r0, $ra, 0 (return to caller)
        Instruction::Jirl {
            rd: Gpr::R0,
            rj: Gpr::Ra,
            offs16: 0,
        }
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
        code.extend_from_slice(
            &Instruction::Lu12iW {
                rd: Gpr::T0,
                imm20: hi20,
            }
            .encode(),
        );

        // ori $t0, $t0, bits[11:0] of entry_addr
        let lo12 = (entry_addr & 0xFFF) as u32;
        code.extend_from_slice(
            &Instruction::Ori {
                rd: Gpr::T0,
                rj: Gpr::T0,
                imm12: lo12,
            }
            .encode(),
        );

        // lu32i.d $t0, bits[51:32] of entry_addr
        let hi32 = ((entry_addr >> 32) & 0xFFFFF) as i32;
        code.extend_from_slice(
            &Instruction::Lu32iD {
                rd: Gpr::T0,
                imm20: hi32,
            }
            .encode(),
        );

        // lu52i.d $t0, $t0, bits[63:52] of entry_addr
        let hi52 = ((entry_addr >> 52) & 0xFFF) as i32;
        code.extend_from_slice(
            &Instruction::Lu52iD {
                rd: Gpr::T0,
                rj: Gpr::T0,
                imm12: hi52,
            }
            .encode(),
        );

        // jr $t0 = jirl $r0, $t0, 0
        code.extend_from_slice(
            &Instruction::Jirl {
                rd: Gpr::R0,
                rj: Gpr::T0,
                offs16: 0,
            }
            .encode(),
        );

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
        let bytes = Instruction::AddW {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        // 3R: opcode[31:15] | rk[14:10] | rj[9:5] | rd[4:0]
        let expected = (0x0020u32 << 15) | (6u32 << 10) | (5u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_add_d() {
        let bytes = Instruction::AddD {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0021u32 << 15) | (6u32 << 10) | (5u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_sub_d() {
        let bytes = Instruction::SubD {
            rd: Gpr::T0,
            rj: Gpr::T1,
            rk: Gpr::T2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0031u32 << 15) | (14u32 << 10) | (13u32 << 5) | 12u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_addi_d() {
        // ADDI.D $sp, $sp, -16 => opcode=0x00B, imm12=0xFF0(-16), rj=sp(3), rd=sp(3)
        let bytes = Instruction::AddiD {
            rd: Gpr::Sp,
            rj: Gpr::Sp,
            imm12: -16,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let imm12 = ((-16i32) as u32) & 0xFFF;
        let expected = (0x00Bu32 << 22) | (imm12 << 10) | (3u32 << 5) | 3u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_ld_d() {
        // LD.D $a0, $sp, 8
        let bytes = Instruction::LdD {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A3u32 << 22) | (8u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_st_d() {
        // ST.D $ra, $sp, -8
        let bytes = Instruction::StD {
            rd: Gpr::Ra,
            rj: Gpr::Sp,
            imm12: -8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let imm12 = ((-8i32) as u32) & 0xFFF;
        let expected = (0x0A7u32 << 22) | (imm12 << 10) | (3u32 << 5) | 1u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_beq() {
        // BEQ $a0, $a1, 16
        let bytes = Instruction::Beq {
            rj: Gpr::A0,
            rd: Gpr::A1,
            offs16: 16,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x16u32 << 26) | (16u32 << 10) | (4u32 << 5) | 5u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_jirl() {
        // JIRL $r0, $ra, 0 (return instruction)
        let bytes = Instruction::Jirl {
            rd: Gpr::R0,
            rj: Gpr::Ra,
            offs16: 0,
        }
        .encode();
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
        let and_bytes = Instruction::And {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let and_word = u32::from_le_bytes(and_bytes);
        assert_eq!(and_word >> 15, 0x0080);

        // OR $a0, $a1, $a2
        let or_bytes = Instruction::Or {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let or_word = u32::from_le_bytes(or_bytes);
        assert_eq!(or_word >> 15, 0x0081);

        // XOR $a0, $a1, $a2
        let xor_bytes = Instruction::Xor {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let xor_word = u32::from_le_bytes(xor_bytes);
        assert_eq!(xor_word >> 15, 0x0082);
    }

    #[test]
    fn test_encode_slt() {
        let bytes = Instruction::Slt {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word >> 15, 0x0040);
    }

    #[test]
    fn test_encode_beqz() {
        // BEQZ $a0, 0x10
        let bytes = Instruction::Beqz {
            rj: Gpr::A0,
            offs21: 0x10,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 26) & 0x3F, OPC_BEQZ); // opcode check
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
        let code = Instruction::AddD {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
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
                relocations: Vec::new(),
                wasm_func_type: None,
                wasm_locals: None,
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

    // ── ISel (instruction selection) tests ──────────────────────────────

    /// Helper: build a minimal IR function with a single instruction.
    fn make_ir_func(name: &str, instrs: Vec<IRInstr>) -> IRFunction {
        use crate::ir::IRBlock;
        use std::collections::HashSet;
        IRFunction {
            name: name.to_string(),
            params: vec![],
            results: vec![],
            param_types: vec![],
            result_types: vec![],
            vregs: std::collections::HashMap::new(),
            blocks: vec![IRBlock {
                label: "entry".to_string(),
                instructions: instrs,
                terminator: crate::ir::IRTerminator::Return(vec![]),
                predecessors: HashSet::new(),
                successors: HashSet::new(),
            }],
        }
    }

    #[test]
    fn test_isel_add_with_immediate_si12() {
        // dst = lhs + 10 should emit addi.d
        let func = make_ir_func(
            "add_imm",
            vec![IRInstr::Add {
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Immediate(10),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        // Should contain an "addi.d" instruction (not just "add.d")
        let has_addi = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "addi.d");
        assert!(
            has_addi,
            "expected addi.d for small immediate add, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_sub_with_immediate_si12() {
        // dst = lhs - 5 should emit addi.d with negated immediate
        let func = make_ir_func(
            "sub_imm",
            vec![IRInstr::Sub {
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Immediate(5),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_addi = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "addi.d");
        assert!(
            has_addi,
            "expected addi.d for small immediate sub, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_neg_is_sub_from_zero() {
        // Neg: dst = -operand => sub.d dst, $r0, src
        let func = make_ir_func(
            "neg",
            vec![IRInstr::UnaryOp {
                op: UnaryOpKind::Neg,
                dst: IRValue::Register(0),
                operand: IRValue::Register(1),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_sub = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "sub.d");
        assert!(
            has_sub,
            "expected sub.d for neg, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_not_is_nor_from_zero() {
        // Not: dst = ~operand => nor dst, $r0, src
        let func = make_ir_func(
            "not",
            vec![IRInstr::UnaryOp {
                op: UnaryOpKind::Not,
                dst: IRValue::Register(0),
                operand: IRValue::Register(1),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_nor = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "nor");
        assert!(
            has_nor,
            "expected nor for not, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_cmp_slt_emits_slt() {
        // Cmp SLt: dst = (lhs < rhs) => slt dst, lhs, rhs
        let func = make_ir_func(
            "cmp_slt",
            vec![IRInstr::Cmp {
                kind: CmpKind::SLt,
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Register(2),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_slt = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "slt");
        assert!(
            has_slt,
            "expected slt for signed less-than, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_load_immediate_emits_lu12i() {
        // Adding with a large immediate should emit lu12i.w to load the constant
        let func = make_ir_func(
            "add_big_imm",
            vec![IRInstr::Add {
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Immediate(100000),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_lu12i = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "lu12i.w");
        assert!(
            has_lu12i,
            "expected lu12i.w for large immediate load, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_shift_by_immediate_emits_slli() {
        // BinOp Shl with immediate should emit slli.d
        let func = make_ir_func(
            "shl_imm",
            vec![IRInstr::BinOp {
                op: BinOpKind::Shl,
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Immediate(3),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_slli = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "slli.d");
        assert!(
            has_slli,
            "expected slli.d for shift-by-immediate, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_ret_emits_jirl() {
        // Ret should emit jirl $r0, $ra, 0
        let func = make_ir_func("ret_test", vec![IRInstr::Ret { values: vec![] }]);
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_jirl = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "jirl");
        assert!(
            has_jirl,
            "expected jirl for ret, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    // ── ELF emission tests ────────────────────────────────────────────

    // ── Bump allocator tests ──────────────────────────────────────────

    #[test]
    fn test_alloc_emits_addi_d_from_sp() {
        // Alloc should compute dst = $sp + offset, emitting addi.d
        let func = make_ir_func(
            "alloc_test",
            vec![IRInstr::Alloc {
                dst: IRValue::Register(0),
                size: 32,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        // The Alloc should produce an instruction that reads $sp
        let has_sp_read = result.blocks[0].instructions.iter().any(|i| {
            i.reads.iter().any(|r| r.class == RegClass::Gpr && r.index == Gpr::Sp.encoding())
                && (i.opcode.contains("addi.d") || i.opcode.contains("add.d") || i.opcode == "Alloc")
        });
        assert!(
            has_sp_read,
            "expected addi.d/add.d from $sp or Alloc for stack allocation, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_alloc_increases_frame_size() {
        // An Alloc of 32 bytes should increase the frame_size beyond the baseline (16)
        let func = make_ir_func(
            "alloc_frame",
            vec![IRInstr::Alloc {
                dst: IRValue::Register(0),
                size: 32,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        // Baseline frame = 16 (ra+fp save), Alloc(32) rounds to 32 => total 48, aligned to 48
        assert!(
            result.frame_size >= 48,
            "expected frame_size >= 48 with 32-byte alloc, got {}",
            result.frame_size
        );
    }

    #[test]
    fn test_alloc_zero_offset_uses_sp_directly() {
        // First Alloc gets offset 0 from $sp; should emit addi.d dst, $sp, 0
        let func = make_ir_func(
            "alloc_zero_off",
            vec![IRInstr::Alloc {
                dst: IRValue::Register(0),
                size: 16,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        // There should be an instruction reading $sp (the alloc offset computation)
        let has_sp_read = result.blocks[0].instructions.iter().any(|i| {
            i.reads.iter().any(|r| r.class == RegClass::Gpr && r.index == Gpr::Sp.encoding())
        });
        assert!(
            has_sp_read,
            "expected instruction reading $sp for alloc at offset 0, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    // ── Function calling with 4+ arguments tests ─────────────────────

    #[test]
    fn test_call_with_four_args_emits_bl() {
        // Call with 4 args should emit bl and create a relocation
        let func = make_ir_func(
            "call_4args",
            vec![IRInstr::Call {
                dst: None,
                func: "target".to_string(),
                args: vec![
                    IRValue::Register(0),
                    IRValue::Register(1),
                    IRValue::Register(2),
                    IRValue::Register(3),
                ],
                is_extern: false,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        // Should contain a "Call" instruction in the stack_slot_isel output
        let has_call = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Call");
        assert!(
            has_call,
            "expected Call instruction for function call, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
        // Verify relocation was created for the call
        assert!(
            !result.relocations.is_empty(),
            "expected relocation for call instruction"
        );
        let reloc = &result.relocations[0];
        assert_eq!(reloc.symbol, "target");
        assert_eq!(reloc.reloc_type, "R_LARCH_B26");
    }

    #[test]
    fn test_call_with_six_args_all_arg_regs() {
        // Call with 6 args should use a0–a5 (6 of 8 arg registers)
        let func = make_ir_func(
            "call_6args",
            vec![IRInstr::Call {
                dst: None,
                func: "target".to_string(),
                args: vec![
                    IRValue::Register(0),
                    IRValue::Register(1),
                    IRValue::Register(2),
                    IRValue::Register(3),
                    IRValue::Register(4),
                    IRValue::Register(5),
                ],
                is_extern: false,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        // Should contain a Call instruction
        let has_call = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Call");
        assert!(
            has_call,
            "expected Call for function call with 6 args, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
        // Verify relocation was created
        assert!(
            !result.relocations.is_empty(),
            "expected relocation for call instruction"
        );
        assert_eq!(result.relocations[0].symbol, "target");
    }

    #[test]
    fn test_call_return_value() {
        // Call with dst should produce a Call instruction and relocation
        let func = make_ir_func(
            "call_ret",
            vec![IRInstr::Call {
                dst: Some(IRValue::Register(0)),
                func: "get_value".to_string(),
                args: vec![],
                is_extern: false,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_call = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Call");
        assert!(
            has_call,
            "expected Call for function call with return value, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    // ── Conditional branch encoding and offset calculation tests ──────

    #[test]
    fn test_encode_bne() {
        // BNE $a0, $a1, 8
        let bytes = Instruction::Bne {
            rj: Gpr::A0,
            rd: Gpr::A1,
            offs16: 8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x17u32 << 26) | (8u32 << 10) | (4u32 << 5) | 5u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_blt() {
        // BLT $t0, $t1, -4
        let bytes = Instruction::Blt {
            rj: Gpr::T0,
            rd: Gpr::T1,
            offs16: -4,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let imm16 = ((-4i32) as u32) & 0xFFFF;
        let expected = (0x18u32 << 26) | (imm16 << 10) | (12u32 << 5) | 13u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_bge() {
        // BGE $a0, $a1, 32
        let bytes = Instruction::Bge {
            rj: Gpr::A0,
            rd: Gpr::A1,
            offs16: 32,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x19u32 << 26) | (32u32 << 10) | (4u32 << 5) | 5u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_bltu() {
        // BLTU $a2, $a3, 16
        let bytes = Instruction::Bltu {
            rj: Gpr::A2,
            rd: Gpr::A3,
            offs16: 16,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x1Au32 << 26) | (16u32 << 10) | (6u32 << 5) | 7u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_bgeu() {
        // BGEU $a0, $a1, 0
        let bytes = Instruction::Bgeu {
            rj: Gpr::A0,
            rd: Gpr::A1,
            offs16: 0,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x1Bu32 << 26) | (0u32 << 10) | (4u32 << 5) | 5u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_bnez() {
        // BNEZ $a0, 0x20
        let bytes = Instruction::Bnez {
            rj: Gpr::A0,
            offs21: 0x20,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        // 1RI21: opcode[31:26] | offs[15:0] at [25:10] | rj[9:5] | offs[20:16] at [4:0]
        assert_eq!((word >> 26) & 0x3F, OPC_BNEZ);
        assert_eq!((word >> 5) & 0x1F, 4u32); // rj = a0
    }

    #[test]
    fn test_encode_branch_negative_offset() {
        // BEQ $a0, $a1, -8 (backward branch)
        let bytes = Instruction::Beq {
            rj: Gpr::A0,
            rd: Gpr::A1,
            offs16: -8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let imm16 = ((-8i32) as u32) & 0xFFFF;
        let expected = (0x16u32 << 26) | (imm16 << 10) | (4u32 << 5) | 5u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_cond_branch_emits_bnez_and_b() {
        // CondBranch should emit bnez + b pattern
        let func = make_ir_func(
            "cond_br",
            vec![IRInstr::CondBranch {
                cond: IRValue::Register(0),
                true_target: "then".to_string(),
                false_target: "else".to_string(),
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let opcodes: Vec<&str> = result.blocks[0]
            .instructions
            .iter()
            .map(|i| i.opcode.as_str())
            .collect();
        let has_cond_br = opcodes.contains(&"CondBranch");
        assert!(
            has_cond_br,
            "expected CondBranch in output, got opcodes: {:?}",
            opcodes
        );
    }

    // ── Load/store with various types and offsets tests ───────────────

    #[test]
    fn test_encode_ld_b() {
        // LD.B $a0, $sp, 4
        let bytes = Instruction::LdB {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 4,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A0u32 << 22) | (4u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_ld_h() {
        // LD.H $a0, $sp, 8
        let bytes = Instruction::LdH {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A1u32 << 22) | (8u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_ld_w() {
        // LD.W $a0, $sp, 12
        let bytes = Instruction::LdW {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 12,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A2u32 << 22) | (12u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_ld_bu() {
        // LD.BU $a0, $sp, 4
        let bytes = Instruction::LdBu {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 4,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A8u32 << 22) | (4u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_ld_hu() {
        // LD.HU $a0, $sp, 8
        let bytes = Instruction::LdHu {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A9u32 << 22) | (8u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_ld_wu() {
        // LD.WU $a0, $sp, 12
        let bytes = Instruction::LdWu {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 12,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0AAu32 << 22) | (12u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_st_b() {
        // ST.B $a0, $sp, 4
        let bytes = Instruction::StB {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 4,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A4u32 << 22) | (4u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_st_h() {
        // ST.H $a0, $sp, 8
        let bytes = Instruction::StH {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 8,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A5u32 << 22) | (8u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_st_w() {
        // ST.W $a0, $sp, 12
        let bytes = Instruction::StW {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: 12,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A6u32 << 22) | (12u32 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_load_store_negative_offset() {
        // LD.D $a0, $sp, -16
        let bytes = Instruction::LdD {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: -16,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let imm12 = ((-16i32) as u32) & 0xFFF;
        let expected = (0x0A3u32 << 22) | (imm12 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);

        // ST.D $a0, $sp, -16
        let bytes = Instruction::StD {
            rd: Gpr::A0,
            rj: Gpr::Sp,
            imm12: -16,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0A7u32 << 22) | (imm12 << 10) | (3u32 << 5) | 4u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_isel_load_i8_emits_load() {
        // Load with IRType::I8 should produce a Load instruction
        let func = make_ir_func(
            "load_i8",
            vec![IRInstr::Load {
                dst: IRValue::Register(0),
                addr: IRValue::Register(1),
                offset: 0,
                ty: IRType::I8,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_load = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Load");
        assert!(
            has_load,
            "expected Load for I8 load, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_load_u16_emits_load() {
        // Load with IRType::U16 should produce a Load instruction
        let func = make_ir_func(
            "load_u16",
            vec![IRInstr::Load {
                dst: IRValue::Register(0),
                addr: IRValue::Register(1),
                offset: 0,
                ty: IRType::U16,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_load = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Load");
        assert!(
            has_load,
            "expected Load for U16 load, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_store_i32_emits_store() {
        // Store with IRType::I32 should produce a Store instruction
        let func = make_ir_func(
            "store_i32",
            vec![IRInstr::Store {
                value: IRValue::Register(0),
                addr: IRValue::Register(1),
                offset: 0,
                ty: IRType::I32,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_store = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Store");
        assert!(
            has_store,
            "expected Store for I32 store, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_load_i64_emits_load() {
        // Load with IRType::I64 should produce a Load instruction
        let func = make_ir_func(
            "load_i64",
            vec![IRInstr::Load {
                dst: IRValue::Register(0),
                addr: IRValue::Register(1),
                offset: 8,
                ty: IRType::I64,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_load = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Load");
        assert!(
            has_load,
            "expected Load for I64 load, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    // ── 64-bit arithmetic operations tests ────────────────────────────

    #[test]
    fn test_encode_mul_d() {
        // MUL.D $a0, $a1, $a2
        let bytes = Instruction::MulD {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_MUL_D);
        assert_eq!((word >> 10) & 0x1F, 6u32); // rk = a2
        assert_eq!((word >> 5) & 0x1F, 5u32); // rj = a1
        assert_eq!(word & 0x1F, 4u32); // rd = a0
    }

    #[test]
    fn test_encode_div_d() {
        // DIV.D $t0, $t1, $t2
        let bytes = Instruction::DivD {
            rd: Gpr::T0,
            rj: Gpr::T1,
            rk: Gpr::T2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_DIV_D);
        assert_eq!((word >> 10) & 0x1F, 14u32); // rk = t2
        assert_eq!((word >> 5) & 0x1F, 13u32); // rj = t1
        assert_eq!(word & 0x1F, 12u32); // rd = t0
    }

    #[test]
    fn test_encode_mod_d() {
        // MOD.D $a0, $a1, $a2
        let bytes = Instruction::ModD {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_MOD_D);
    }

    #[test]
    fn test_encode_div_du() {
        // DIV.DU $a0, $a1, $a2
        let bytes = Instruction::DivDu {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_DIV_DU);
    }

    #[test]
    fn test_encode_mod_du() {
        // MOD.DU $a0, $a1, $a2
        let bytes = Instruction::ModDu {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_MOD_DU);
    }

    #[test]
    fn test_encode_sll_d() {
        // SLL.D $a0, $a1, $a2
        let bytes = Instruction::SllD {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_SLL_D);
    }

    #[test]
    fn test_encode_srl_d() {
        // SRL.D $a0, $a1, $a2
        let bytes = Instruction::SrlD {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_SRL_D);
    }

    #[test]
    fn test_encode_sra_d() {
        // SRA.D $a0, $a1, $a2
        let bytes = Instruction::SraD {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 15) & 0x1FFFF, OPC_SRA_D);
    }

    #[test]
    fn test_isel_mul_emits_mul() {
        // IRInstr::Mul should produce a Mul instruction
        let func = make_ir_func(
            "mul_test",
            vec![IRInstr::Mul {
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Register(2),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_mul = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Mul");
        assert!(
            has_mul,
            "expected Mul for IR Mul, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_div_emits_div() {
        // IRInstr::Div should produce a Div instruction
        let func = make_ir_func(
            "div_test",
            vec![IRInstr::Div {
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Register(2),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_div = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "Div");
        assert!(
            has_div,
            "expected Div for IR Div, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_add_emits_add() {
        // BinOp Add should produce an Add instruction
        let func = make_ir_func(
            "add_d_test",
            vec![IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Register(2),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_add = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "BinOp" || i.opcode == "Add");
        assert!(
            has_add,
            "expected Add/BinOp for BinOp Add, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_isel_sub_emits_sub() {
        // BinOp Sub should produce a Sub instruction
        let func = make_ir_func(
            "sub_d_test",
            vec![IRInstr::BinOp {
                op: BinOpKind::Sub,
                dst: IRValue::Register(0),
                lhs: IRValue::Register(1),
                rhs: IRValue::Register(2),
                ty: None,
            }],
        );
        let backend = LoongArch64Backend::new();
        let result = backend.allocate_registers(&func).unwrap();
        let has_sub = result.blocks[0]
            .instructions
            .iter()
            .any(|i| i.opcode == "BinOp" || i.opcode == "Sub");
        assert!(
            has_sub,
            "expected Sub/BinOp for BinOp Sub, got opcodes: {:?}",
            result.blocks[0]
                .instructions
                .iter()
                .map(|i| &i.opcode)
                .collect::<Vec<_>>()
        );
    }

    // ── ELF emission tests ────────────────────────────────────────────

    #[test]
    fn test_elf_header_endianness() {
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
                relocations: Vec::new(),
                wasm_func_type: None,
                wasm_locals: None,
            }],
            total_code_size: 4,
            total_data_size: 0,
        };
        let elf = backend.encode_program(&prog).unwrap();
        // Check ELFCLASS64
        assert_eq!(elf[4], 2, "expected ELFCLASS64");
        // Check ELFDATA2LSB (little-endian)
        assert_eq!(elf[5], 1, "expected ELFDATA2LSB for LoongArch64");
    }

    #[test]
    fn test_elf_header_flags_lp64d() {
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
                relocations: Vec::new(),
                wasm_func_type: None,
                wasm_locals: None,
            }],
            total_code_size: 4,
            total_data_size: 0,
        };
        let elf = backend.encode_program(&prog).unwrap();
        // Check e_flags at offset 48 (4 bytes)
        let e_flags = u32::from_le_bytes([elf[48], elf[49], elf[50], elf[51]]);
        assert_eq!(e_flags, 0x43, "expected EF_LARCH_ABI_LP64D (0x43)");
    }

    #[test]
    fn test_elf_entry_point_points_to_start_stub() {
        let backend = LoongArch64Backend::new();
        let prog = AllocatedProgram {
            functions: vec![AllocatedFunction {
                name: "main".to_string(),
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
                relocations: Vec::new(),
                wasm_func_type: None,
                wasm_locals: None,
            }],
            total_code_size: 4,
            total_data_size: 0,
        };
        let elf = backend.encode_program(&prog).unwrap();
        // e_entry at offset 24 (8 bytes)
        let e_entry = u64::from_le_bytes([
            elf[24], elf[25], elf[26], elf[27],
            elf[28], elf[29], elf[30], elf[31],
        ]);
        // Entry should be base_addr + text_offset (0x120000000 + 0x10000 = 0x120010000)
        assert_eq!(e_entry, 0x120010000, "entry point should point to _start stub");
        // Verify the first instruction at the entry point is BL
        let text_offset = 0x10000usize;
        let first_word = u32::from_le_bytes([
            elf[text_offset], elf[text_offset + 1], elf[text_offset + 2], elf[text_offset + 3],
        ]);
        let opcode = (first_word >> 26) & 0x3F;
        assert_eq!(opcode, 0x15, "first instruction at entry should be BL (opcode 0x15)");
    }

    #[test]
    fn test_patch_load_imm_64() {
        // Verify that re-encoding a 4-instruction load-immediate sequence
        // with a new 64-bit value produces the correct encoding.
        let rd = Gpr::A0;
        let target_addr: u64 = 0x120010ABC;

        // Emit the full sequence with the target value directly
        let mut code = Vec::new();
        let hi20 = ((target_addr >> 12) & 0xFFFFF) as i32;
        code.extend_from_slice(&Instruction::Lu12iW { rd, imm20: hi20 }.encode());
        let lo12 = (target_addr & 0xFFF) as u32;
        code.extend_from_slice(&Instruction::Ori { rd, rj: rd, imm12: lo12 }.encode());
        let hi32 = ((target_addr >> 32) & 0xFFFFF) as i32;
        code.extend_from_slice(&Instruction::Lu32iD { rd, imm20: hi32 }.encode());
        let hi52 = ((target_addr >> 52) & 0xFFF) as i32;
        code.extend_from_slice(&Instruction::Lu52iD { rd, rj: rd, imm12: hi52 }.encode());
        assert_eq!(code.len(), 16);

        // Verify by decoding the instructions
        // Step 1: lu12i.w rd, bits[31:12]
        let word0 = u32::from_le_bytes([code[0], code[1], code[2], code[3]]);
        let expected_word0 = u32::from_le_bytes(Instruction::Lu12iW { rd, imm20: hi20 }.encode());
        assert_eq!(word0, expected_word0, "lu12i.w encoding mismatch");

        // Step 2: ori rd, rd, bits[11:0]
        let word1 = u32::from_le_bytes([code[4], code[5], code[6], code[7]]);
        let expected_word1 = u32::from_le_bytes(Instruction::Ori { rd, rj: rd, imm12: lo12 }.encode());
        assert_eq!(word1, expected_word1, "ori encoding mismatch");

        // Step 3: lu32i.d rd, bits[51:32]
        let word2 = u32::from_le_bytes([code[8], code[9], code[10], code[11]]);
        let expected_word2 = u32::from_le_bytes(Instruction::Lu32iD { rd, imm20: hi32 }.encode());
        assert_eq!(word2, expected_word2, "lu32i.d encoding mismatch");

        // Step 4: lu52i.d rd, rd, bits[63:52]
        let word3 = u32::from_le_bytes([code[12], code[13], code[14], code[15]]);
        let expected_word3 = u32::from_le_bytes(Instruction::Lu52iD { rd, rj: rd, imm12: hi52 }.encode());
        assert_eq!(word3, expected_word3, "lu52i.d encoding mismatch");
    }

    #[test]
    fn test_elf_program_headers() {
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
                relocations: Vec::new(),
                wasm_func_type: None,
                wasm_locals: None,
            }],
            total_code_size: 4,
            total_data_size: 0,
        };
        let elf = backend.encode_program(&prog).unwrap();

        // e_phoff at offset 32
        let e_phoff = u64::from_le_bytes([
            elf[32], elf[33], elf[34], elf[35], elf[36], elf[37], elf[38], elf[39],
        ]);
        assert_eq!(e_phoff, 64, "program headers should start right after ELF header");

        // e_phnum at offset 56
        let e_phnum = u16::from_le_bytes([elf[56], elf[57]]);
        assert_eq!(e_phnum, 2, "should have 2 program headers");

        // First program header: LOAD RX (text)
        let ph1_off = e_phoff as usize;
        let p1_type = u32::from_le_bytes([elf[ph1_off], elf[ph1_off+1], elf[ph1_off+2], elf[ph1_off+3]]);
        let p1_flags = u32::from_le_bytes([elf[ph1_off+4], elf[ph1_off+5], elf[ph1_off+6], elf[ph1_off+7]]);
        assert_eq!(p1_type, 1, "first segment should be PT_LOAD");
        assert_eq!(p1_flags, 5, "first segment should be PF_R | PF_X");

        // Second program header: LOAD RW (data)
        let ph2_off = ph1_off + 56;
        let p2_type = u32::from_le_bytes([elf[ph2_off], elf[ph2_off+1], elf[ph2_off+2], elf[ph2_off+3]]);
        let p2_flags = u32::from_le_bytes([elf[ph2_off+4], elf[ph2_off+5], elf[ph2_off+6], elf[ph2_off+7]]);
        assert_eq!(p2_type, 1, "second segment should be PT_LOAD");
        assert_eq!(p2_flags, 6, "second segment should be PF_R | PF_W");
    }
}
pub mod disasm;
pub mod reg_alloc_isel;
pub mod stack_slot_isel;
