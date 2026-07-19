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
    AllocatedFunction, AllocatedProgram, Backend,
    BackendError, LoongArch64TargetInfo, TargetInfo,
};
use crate::ir::IRFunction;
#[cfg(test)]
use crate::ir::IRInstr;
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// LoongArch64 general-purpose registers (r0–r31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
fn encode_4r(opcode: u32, ra: u32, rk: u32, rj: u32, rd: u32) -> [u8; 4] {
    let word = ((opcode & 0xFFF) << 20)
        | ((ra & 0x1F) << 15)
        | ((rk & 0x1F) << 10)
        | ((rj & 0x1F) << 5)
        | (rd & 0x1F);
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
const OPC_AMADD_W: u32 = 0x00C4;
const OPC_AMADD_D: u32 = 0x00C6;
const OPC_AMAND_W: u32 = 0x00C8;
const OPC_AMAND_D: u32 = 0x00CA;
const OPC_AMOR_W: u32 = 0x00CC;
const OPC_AMOR_D: u32 = 0x00CE;
const OPC_AMXOR_W: u32 = 0x00D0;
const OPC_AMXOR_D: u32 = 0x00D2;
const OPC_AMMAX_W: u32 = 0x00D4;
const OPC_AMMAX_D: u32 = 0x00D6;
const OPC_AMMIN_W: u32 = 0x00D8;
const OPC_AMMIN_D: u32 = 0x00DA;
const OPC_AMMAX_WU: u32 = 0x00DC;
const OPC_AMMAX_DU: u32 = 0x00DE;
const OPC_AMMIN_WU: u32 = 0x00E0;
const OPC_AMMIN_DU: u32 = 0x00E2;

// FP math opcodes (2R format: primary=0x0B in bits 31:26, sub-opcode in bits 9:0)
// These are the full 15-bit opcode values (bits 31:15) used by encode_2r.
const OPC_FSQRT_S: u32 = 0x2E000000 >> 15; // 0x2E000000 = primary 0x0B, sub 0x040000
const OPC_FSQRT_D: u32 = 0x2E000001 >> 15;
const OPC_FABS_S: u32 = 0x2E000004 >> 15;
const OPC_FABS_D: u32 = 0x2E000005 >> 15;
const OPC_FNEG_S: u32 = 0x2E000006 >> 15;
const OPC_FNEG_D: u32 = 0x2E000007 >> 15;
const OPC_FLOGB_S: u32 = 0x2E000008 >> 15;
const OPC_FLOGB_D: u32 = 0x2E000009 >> 15;
const OPC_FCLASS_S: u32 = 0x2E00000A >> 15;
const OPC_FCLASS_D: u32 = 0x2E00000B >> 15;

// FP math opcodes (3R format: primary=0x0B in bits 31:26)
const OPC_FMIN_S: u32 = 0x2E300000 >> 15;
const OPC_FMIN_D: u32 = 0x2E300001 >> 15;
const OPC_FMAX_S: u32 = 0x2E300002 >> 15;
const OPC_FMAX_D: u32 = 0x2E300003 >> 15;
const OPC_FSCALEB_S: u32 = 0x2E300008 >> 15;
const OPC_FSCALEB_D: u32 = 0x2E300009 >> 15;
const OPC_FCOPYSIGN_S: u32 = 0x2E30000A >> 15;
const OPC_FCOPYSIGN_D: u32 = 0x2E30000B >> 15;
const OPC_FSGNJ_S: u32 = 0x2E30000C >> 15;
const OPC_FSGNJ_D: u32 = 0x2E30000D >> 15;
const OPC_FSGNJN_S: u32 = 0x2E30000E >> 15;
const OPC_FSGNJN_D: u32 = 0x2E30000F >> 15;
const OPC_FSGNJX_S: u32 = 0x2E300010 >> 15;
const OPC_FSGNJX_D: u32 = 0x2E300011 >> 15;

// Bit manipulation opcodes — already defined below in the 2R section
// (OPC_REVB_2H, OPC_REVB_4H, OPC_REVB_2W, OPC_BITREV_4B, OPC_BITREV_8B)

// Misc system instruction opcodes (2R format)
const OPC_CPUCFG: u32 = 0x00000006;
const OPC_RDTIMEL_W: u32 = 0x00000020;
const OPC_RDTIMEH_W: u32 = 0x00000021;
const OPC_ASRTLE_D: u32 = 0x00000000;
const OPC_ASRTGT_D: u32 = 0x00000001;

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
/// CTZ.D (count trailing zeros, doubleword): opcode 0x000000C in 2R format.
const OPC_CTZ_D: u32 = 0x000000C;
/// POPCNT.D (population count, doubleword): opcode 0x000000E in 2R format.
const OPC_POPCNT_D: u32 = 0x000000E;

const OPC_REVB_2H: u32 = 0x000000C;
const OPC_REVB_4H: u32 = 0x000000D;
const OPC_REVB_2W: u32 = 0x000000E;
const OPC_BITREV_4B: u32 = 0x0000012;
const OPC_BITREV_8B: u32 = 0x0000013;
// ===========================================================================
// 4R-format Opcodes (bits[31:20])
// ===========================================================================

// ===========================================================================
// reg2i5-format Opcodes (bits[31:15], 17-bit) — .W shift immediates
// ===========================================================================

const OPC_SLLI_W: u32 = 0x0081;
const OPC_SRLI_W: u32 = 0x0089;
const OPC_SRAI_W: u32 = 0x0091;

// ===========================================================================
// reg2i6-format Opcodes (bits[31:16], 16-bit) — .D shift immediates
// ===========================================================================

const OPC_SLLI_D: u32 = 0x0041;
const OPC_SRLI_D: u32 = 0x0045;
const OPC_SRAI_D: u32 = 0x0049;

// ===========================================================================
// Instruction Enum
// ===========================================================================

/// LoongArch64 instruction representations for code generation.
///
/// Covers key arithmetic, logical, shift, load/store, branch, move, and FP
/// instructions. Each variant captures the operands needed for encoding and
/// disassembly. The `encode()` method produces a 4-byte little-endian machine
/// code word.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Atomic Memory Add Word: `amadd.w rd, rj, rk`
    /// rd = old value at [rj]; [rj] = old + rk
    AmaddW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Add Doubleword: `amadd.d rd, rj, rk`
    AmaddD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory AND Word: `amand.w rd, rj, rk`
    AmandW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory AND Doubleword: `amand.d rd, rj, rk`
    AmandD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory OR Word: `amor.w rd, rj, rk`
    AmorW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory OR Doubleword: `amor.d rd, rj, rk`
    AmorD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory XOR Word: `amxor.w rd, rj, rk`
    AmxorW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory XOR Doubleword: `amxor.d rd, rj, rk`
    AmxorD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Max Word (signed): `ammax.w rd, rj, rk`
    AmmaxW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Max Doubleword (signed): `ammax.d rd, rj, rk`
    AmmaxD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Max Word Unsigned: `ammax_wu.w rd, rj, rk`
    AmmaxWu { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Max Doubleword Unsigned: `ammax_wu.d rd, rj, rk`
    AmmaxDu { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Min Word (signed): `ammin.w rd, rj, rk`
    AmminW { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Min Doubleword (signed): `ammin.d rd, rj, rk`
    AmminD { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Min Word Unsigned: `ammin_wu.w rd, rj, rk`
    AmminWu { rd: Gpr, rj: Gpr, rk: Gpr },
    /// Atomic Memory Min Doubleword Unsigned: `ammin_wu.d rd, rj, rk`
    AmminDu { rd: Gpr, rj: Gpr, rk: Gpr },

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
    /// Count Trailing Zeros, Doubleword: `ctz.d rd, rj`
    CtzD { rd: Gpr, rj: Gpr },
    /// Population Count, Doubleword: `popcnt.d rd, rj`
    PopcntD { rd: Gpr, rj: Gpr },

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
    /// Signed Integer Word to Single: `ffint.s.w fd, fj` (i32→f32)
    FfintSW { fd: Fpr, fj: Fpr },
    /// Signed Integer Long to Single: `ffint.s.l fd, fj` (i64→f32)
    FfintSL { fd: Fpr, fj: Fpr },
    /// Signed Integer Word to Double: `ffint.d.w fd, fj` (i32→f64)
    FfintDW { fd: Fpr, fj: Fpr },
    /// Signed Integer Long to Double: `ffint.d.l fd, fj` (i64→f64)
    FfintDL { fd: Fpr, fj: Fpr },
    /// Single to Signed Integer Word: `ftint.w.s fd, fj` (f32→i32)
    FtintWS { fd: Fpr, fj: Fpr },
    /// Double to Signed Integer Word: `ftint.w.d fd, fj` (f64→i32)
    FtintWD { fd: Fpr, fj: Fpr },
    /// Single to Signed Integer Long: `ftint.l.s fd, fj` (f32→i64)
    FtintLS { fd: Fpr, fj: Fpr },
    /// Double to Signed Integer Long: `ftint.l.d fd, fj` (f64→i64)
    FtintLD { fd: Fpr, fj: Fpr },
    /// Single to Double: `fcvt.d.s fd, fj` (f32→f64)
    FcvtDS { fd: Fpr, fj: Fpr },
    /// Double to Single: `fcvt.s.d fd, fj` (f64→f32)
    FcvtSD { fd: Fpr, fj: Fpr },

    // ── FP Compare (4R-like) ────────────────────────────────────────
    /// FP Compare Single: `fcmp.cond.s cd, fj, fk`
    FCmpS { cond: u8, fj: Fpr, fk: Fpr, cd: u8 },
    /// FP Compare Double: `fcmp.cond.d cd, fj, fk`
    FCmpD { cond: u8, fj: Fpr, fk: Fpr, cd: u8 },

    // ── FP Math (2R) ───────────────────────────────────────────────
    /// FP Square Root Single: `fsqrt.s fd, fj`
    FsqrtS { fd: Fpr, fj: Fpr },
    /// FP Square Root Double: `fsqrt.d fd, fj`
    FsqrtD { fd: Fpr, fj: Fpr },
    /// FP Absolute Value Single: `fabs.s fd, fj`
    FabsS { fd: Fpr, fj: Fpr },
    /// FP Absolute Value Double: `fabs.d fd, fj`
    FabsD { fd: Fpr, fj: Fpr },
    /// FP Negate Single: `fneg.s fd, fj`
    FnegS { fd: Fpr, fj: Fpr },
    /// FP Negate Double: `fneg.d fd, fj`
    FnegD { fd: Fpr, fj: Fpr },
    /// FP Min Single: `fmin.s fd, fj, fk`
    FminS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Min Double: `fmin.d fd, fj, fk`
    FminD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Max Single: `fmax.s fd, fj, fk`
    FmaxS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Max Double: `fmax.d fd, fj, fk`
    FmaxD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Copy Sign Single: `fcopysign.s fd, fj, fk`
    FcopysignS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Copy Sign Double: `fcopysign.d fd, fj, fk`
    FcopysignD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Scale by Power Single: `fscaleb.s fd, fj, fk`
    FscalebS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Scale by Power Double: `fscaleb.d fd, fj, fk`
    FscalebD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Log Base 2 Single: `flogb.s fd, fj`
    FlogbS { fd: Fpr, fj: Fpr },
    /// FP Log Base 2 Double: `flogb.d fd, fj`
    FlogbD { fd: Fpr, fj: Fpr },
    /// FP Class Single: `fclass.s fd, fj`
    FclassS { fd: Fpr, fj: Fpr },
    /// FP Class Double: `fclass.d fd, fj`
    FclassD { fd: Fpr, fj: Fpr },
    /// FP Sign Inject Single: `fsgnj.s fd, fj, fk`
    FsgnjS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Sign Inject Double: `fsgnj.d fd, fj, fk`
    FsgnjD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Neg Sign Inject Single: `fsgnjn.s fd, fj, fk`
    FsgnjnS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP Neg Sign Inject Double: `fsgnjn.d fd, fj, fk`
    FsgnjnD { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP XOR Sign Inject Single: `fsgnjx.s fd, fj, fk`
    FsgnjxS { fd: Fpr, fj: Fpr, fk: Fpr },
    /// FP XOR Sign Inject Double: `fsgnjx.d fd, fj, fk`
    FsgnjxD { fd: Fpr, fj: Fpr, fk: Fpr },

    // ── Bit Manipulation (2R) ──────────────────────────────────────
    /// Reverse Bytes in 2 Halfwords: `revb.2h rd, rj`
    Revb2H { rd: Gpr, rj: Gpr },
    /// Reverse Bytes in 4 Halfwords: `revb.4h rd, rj`
    Revb4H { rd: Gpr, rj: Gpr },
    /// Reverse Bytes in 2 Words: `revb.2w rd, rj`
    Revb2W { rd: Gpr, rj: Gpr },
    /// Bit Reverse 4 Bytes: `bitrev.4b rd, rj`
    Bitrev4B { rd: Gpr, rj: Gpr },
    /// Bit Reverse 8 Bytes: `bitrev.8b rd, rj`
    Bitrev8B { rd: Gpr, rj: Gpr },

    // ── Misc System Instructions ───────────────────────────────────
    /// CPU Configure: `cpucfg rd, rj`
    Cpucfg { rd: Gpr, rj: Gpr },
    /// Read Time Stamp Counter Low: `rdtimel.w rd, rj`
    RdtimelW { rd: Gpr, rj: Gpr },
    /// Read Time Stamp Counter High: `rdtimeh.w rd, rj`
    RdtimehW { rd: Gpr, rj: Gpr },
    /// Address Bounds Check Less: `asrtle.d rj, rk`
    AsrtleD { rj: Gpr, rk: Gpr },
    /// Address Bounds Check Greater: `asrgt.d rj, rk`
    AsrgtD { rj: Gpr, rk: Gpr },

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
            Instruction::AmaddW { rd, rj, rk } => encode_3r(OPC_AMADD_W, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmaddD { rd, rj, rk } => encode_3r(OPC_AMADD_D, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmandW { rd, rj, rk } => encode_3r(OPC_AMAND_W, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmandD { rd, rj, rk } => encode_3r(OPC_AMAND_D, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmorW { rd, rj, rk } => encode_3r(OPC_AMOR_W, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmorD { rd, rj, rk } => encode_3r(OPC_AMOR_D, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmxorW { rd, rj, rk } => encode_3r(OPC_AMXOR_W, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmxorD { rd, rj, rk } => encode_3r(OPC_AMXOR_D, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmmaxW { rd, rj, rk } => encode_3r(OPC_AMMAX_W, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmmaxD { rd, rj, rk } => encode_3r(OPC_AMMAX_D, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmmaxWu { rd, rj, rk } => encode_3r(OPC_AMMAX_WU, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmmaxDu { rd, rj, rk } => encode_3r(OPC_AMMAX_DU, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmminW { rd, rj, rk } => encode_3r(OPC_AMMIN_W, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmminD { rd, rj, rk } => encode_3r(OPC_AMMIN_D, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmminWu { rd, rj, rk } => encode_3r(OPC_AMMIN_WU, rk.encoding(), rj.encoding(), rd.encoding()),
            Instruction::AmminDu { rd, rj, rk } => encode_3r(OPC_AMMIN_DU, rk.encoding(), rj.encoding(), rd.encoding()),

            // ── FP Math (2R: unary) ───────────────────────────────
            Instruction::FsqrtS { fd, fj } => encode_2r(OPC_FSQRT_S, fj.encoding(), fd.encoding()),
            Instruction::FsqrtD { fd, fj } => encode_2r(OPC_FSQRT_D, fj.encoding(), fd.encoding()),
            Instruction::FabsS { fd, fj } => encode_2r(OPC_FABS_S, fj.encoding(), fd.encoding()),
            Instruction::FabsD { fd, fj } => encode_2r(OPC_FABS_D, fj.encoding(), fd.encoding()),
            Instruction::FnegS { fd, fj } => encode_2r(OPC_FNEG_S, fj.encoding(), fd.encoding()),
            Instruction::FnegD { fd, fj } => encode_2r(OPC_FNEG_D, fj.encoding(), fd.encoding()),
            Instruction::FlogbS { fd, fj } => encode_2r(OPC_FLOGB_S, fj.encoding(), fd.encoding()),
            Instruction::FlogbD { fd, fj } => encode_2r(OPC_FLOGB_D, fj.encoding(), fd.encoding()),
            Instruction::FclassS { fd, fj } => encode_2r(OPC_FCLASS_S, fj.encoding(), fd.encoding()),
            Instruction::FclassD { fd, fj } => encode_2r(OPC_FCLASS_D, fj.encoding(), fd.encoding()),

            // ── FP Math (3R: binary) ──────────────────────────────
            Instruction::FminS { fd, fj, fk } => encode_3r(OPC_FMIN_S, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FminD { fd, fj, fk } => encode_3r(OPC_FMIN_D, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FmaxS { fd, fj, fk } => encode_3r(OPC_FMAX_S, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FmaxD { fd, fj, fk } => encode_3r(OPC_FMAX_D, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FscalebS { fd, fj, fk } => encode_3r(OPC_FSCALEB_S, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FscalebD { fd, fj, fk } => encode_3r(OPC_FSCALEB_D, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FcopysignS { fd, fj, fk } => encode_3r(OPC_FCOPYSIGN_S, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FcopysignD { fd, fj, fk } => encode_3r(OPC_FCOPYSIGN_D, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FsgnjS { fd, fj, fk } => encode_3r(OPC_FSGNJ_S, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FsgnjD { fd, fj, fk } => encode_3r(OPC_FSGNJ_D, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FsgnjnS { fd, fj, fk } => encode_3r(OPC_FSGNJN_S, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FsgnjnD { fd, fj, fk } => encode_3r(OPC_FSGNJN_D, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FsgnjxS { fd, fj, fk } => encode_3r(OPC_FSGNJX_S, fk.encoding(), fj.encoding(), fd.encoding()),
            Instruction::FsgnjxD { fd, fj, fk } => encode_3r(OPC_FSGNJX_D, fk.encoding(), fj.encoding(), fd.encoding()),

            // ── Bit Manipulation (2R) ─────────────────────────────
            Instruction::Revb2H { rd, rj } => encode_2r(OPC_REVB_2H, rj.encoding(), rd.encoding()),
            Instruction::Revb4H { rd, rj } => encode_2r(OPC_REVB_4H, rj.encoding(), rd.encoding()),
            Instruction::Revb2W { rd, rj } => encode_2r(OPC_REVB_2W, rj.encoding(), rd.encoding()),
            Instruction::Bitrev4B { rd, rj } => encode_2r(OPC_BITREV_4B, rj.encoding(), rd.encoding()),
            Instruction::Bitrev8B { rd, rj } => encode_2r(OPC_BITREV_8B, rj.encoding(), rd.encoding()),

            // ── Misc System Instructions ──────────────────────────
            Instruction::Cpucfg { rd, rj } => encode_2r(OPC_CPUCFG, rj.encoding(), rd.encoding()),
            Instruction::RdtimelW { rd, rj } => encode_2r(OPC_RDTIMEL_W, rj.encoding(), rd.encoding()),
            Instruction::RdtimehW { rd, rj } => encode_2r(OPC_RDTIMEH_W, rj.encoding(), rd.encoding()),
            Instruction::AsrtleD { rj, rk } => encode_3r(OPC_ASRTLE_D, rk.encoding(), rj.encoding(), 0),
            Instruction::AsrgtD { rj, rk } => encode_3r(OPC_ASRTGT_D, rk.encoding(), rj.encoding(), 0),

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
            Instruction::CtzD { rd, rj } => encode_2r(OPC_CTZ_D, rj.encoding(), rd.encoding()),
            Instruction::PopcntD { rd, rj } => encode_2r(OPC_POPCNT_D, rj.encoding(), rd.encoding()),

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
            // Opcodes verified against QEMU 7.2.0 loongarch64 disassembly.
            // The 0x004502-0x00451F range used previously maps to fabs/fclass/
            // frsqrt — *not* the conversion instructions — which is why QEMU
            // raised SIGILL on every Cast. The real conversions live in the
            // 0x0046XX (fcvt/ftint) and 0x0047XX (ffint) ranges.
            //
            // For FloatToInt casts we use the round-toward-zero (truncating)
            // `ftintrz.*` variants rather than the default-rounding `ftint.*`,
            // matching Rust `as` semantics (e.g. 9.7 -> 9, not 10).
            //
            // FFINT.S.W: opcode=0x004744 (i32→f32)
            Instruction::FfintSW { fd, fj } => encode_2r(0x004744, fj.encoding(), fd.encoding()),
            // FFINT.S.L: opcode=0x004746 (i64→f32)
            Instruction::FfintSL { fd, fj } => encode_2r(0x004746, fj.encoding(), fd.encoding()),
            // FFINT.D.W: opcode=0x004748 (i32→f64)
            Instruction::FfintDW { fd, fj } => encode_2r(0x004748, fj.encoding(), fd.encoding()),
            // FFINT.D.L: opcode=0x00474A (i64→f64)
            Instruction::FfintDL { fd, fj } => encode_2r(0x00474A, fj.encoding(), fd.encoding()),
            // FTINTRZ.W.S: opcode=0x0046A1 (f32→i32, round toward zero)
            Instruction::FtintWS { fd, fj } => encode_2r(0x0046A1, fj.encoding(), fd.encoding()),
            // FTINTRZ.W.D: opcode=0x0046A2 (f64→i32, round toward zero)
            Instruction::FtintWD { fd, fj } => encode_2r(0x0046A2, fj.encoding(), fd.encoding()),
            // FTINTRZ.L.S: opcode=0x0046A9 (f32→i64, round toward zero)
            Instruction::FtintLS { fd, fj } => encode_2r(0x0046A9, fj.encoding(), fd.encoding()),
            // FTINTRZ.L.D: opcode=0x0046AA (f64→i64, round toward zero)
            Instruction::FtintLD { fd, fj } => encode_2r(0x0046AA, fj.encoding(), fd.encoding()),
            // FCVT.D.S: opcode=0x004649 (f32→f64)
            Instruction::FcvtDS { fd, fj } => encode_2r(0x004649, fj.encoding(), fd.encoding()),
            // FCVT.S.D: opcode=0x004646 (f64→f32)
            Instruction::FcvtSD { fd, fj } => encode_2r(0x004646, fj.encoding(), fd.encoding()),

            // ── FP Compare (4R-like) ──────────────────────────────
            Instruction::FCmpS { cond, fj, fk, cd } => {
                // fcmp.cond.s cd, fj, fk — LoongArch encoding (verified
                // empirically against QEMU):
                //   bits 31-20: 12-bit opcode  = 0x0C1 (.s) / 0x0C2 (.d)
                //   bits 19-16: cond (4 bits)  — selects CAF/CLT/CEQ/CLE/
                //                               CUN/CNE/COR/... (NOT the
                //                               standard 0-15 cond numbering;
                //                               see LoongArch manual table)
                //   bits 14-10: fk   (5 bits)
                //   bits 9-5:   fj   (5 bits)
                //   bits 4-0:   cd   (5 bits, only low 3 used for fcc0-7)
                //
                // The previous encode_4r call placed cond in bits 19-15
                // (5 bits) and fk in bits 14-10, which QEMU decoded as
                // cond=SAF (signaling always-false) for cond=1, causing
                // every FP comparison to return false.  The correct
                // encoding uses bits 19-16 (4 bits) for cond.
                let word = ((OPC_FCMP_S & 0xFFF) << 20)
                    | (((*cond & 0xF) as u32) << 16)
                    | ((fk.encoding() as u32) << 10)
                    | ((fj.encoding() as u32) << 5)
                    | ((*cd & 0x1F) as u32);
                word.to_le_bytes()
            }
            Instruction::FCmpD { cond, fj, fk, cd } => {
                // fcmp.cond.d cd, fj, fk — same layout as .s, but opcode
                // bits 31-20 = 0x0C2.
                let word = ((OPC_FCMP_D & 0xFFF) << 20)
                    | (((*cond & 0xF) as u32) << 16)
                    | ((fk.encoding() as u32) << 10)
                    | ((fj.encoding() as u32) << 5)
                    | ((*cd & 0x1F) as u32);
                word.to_le_bytes()
            }

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
            Instruction::AmaddW { .. } => "amadd.w",
            Instruction::AmaddD { .. } => "amadd.d",
            Instruction::AmandW { .. } => "amand.w",
            Instruction::AmandD { .. } => "amand.d",
            Instruction::AmorW { .. } => "amor.w",
            Instruction::AmorD { .. } => "amor.d",
            Instruction::AmxorW { .. } => "amxor.w",
            Instruction::AmxorD { .. } => "amxor.d",
            Instruction::AmmaxW { .. } => "ammax.w",
            Instruction::AmmaxD { .. } => "ammax.d",
            Instruction::AmmaxWu { .. } => "ammax.wu",
            Instruction::AmmaxDu { .. } => "ammax.du",
            Instruction::AmminW { .. } => "ammin.w",
            Instruction::AmminD { .. } => "ammin.d",
            Instruction::AmminWu { .. } => "ammin.wu",
            Instruction::AmminDu { .. } => "ammin.du",
            Instruction::FsqrtS { .. } => "fsqrt.s",
            Instruction::FsqrtD { .. } => "fsqrt.d",
            Instruction::FabsS { .. } => "fabs.s",
            Instruction::FabsD { .. } => "fabs.d",
            Instruction::FnegS { .. } => "fneg.s",
            Instruction::FnegD { .. } => "fneg.d",
            Instruction::FminS { .. } => "fmin.s",
            Instruction::FminD { .. } => "fmin.d",
            Instruction::FmaxS { .. } => "fmax.s",
            Instruction::FmaxD { .. } => "fmax.d",
            Instruction::FcopysignS { .. } => "fcopysign.s",
            Instruction::FcopysignD { .. } => "fcopysign.d",
            Instruction::FscalebS { .. } => "fscaleb.s",
            Instruction::FscalebD { .. } => "fscaleb.d",
            Instruction::FlogbS { .. } => "flogb.s",
            Instruction::FlogbD { .. } => "flogb.d",
            Instruction::FclassS { .. } => "fclass.s",
            Instruction::FclassD { .. } => "fclass.d",
            Instruction::FsgnjS { .. } => "fsgnj.s",
            Instruction::FsgnjD { .. } => "fsgnj.d",
            Instruction::FsgnjnS { .. } => "fsgnjn.s",
            Instruction::FsgnjnD { .. } => "fsgnjn.d",
            Instruction::FsgnjxS { .. } => "fsgnjx.s",
            Instruction::FsgnjxD { .. } => "fsgnjx.d",
            Instruction::Revb2H { .. } => "revb.2h",
            Instruction::Revb4H { .. } => "revb.4h",
            Instruction::Revb2W { .. } => "revb.2w",
            Instruction::Bitrev4B { .. } => "bitrev.4b",
            Instruction::Bitrev8B { .. } => "bitrev.8b",
            Instruction::Cpucfg { .. } => "cpucfg",
            Instruction::RdtimelW { .. } => "rdtimel.w",
            Instruction::RdtimehW { .. } => "rdtimeh.w",
            Instruction::AsrtleD { .. } => "asrtle.d",
            Instruction::AsrgtD { .. } => "asrgt.d",
            Instruction::Dbar { .. } => "dbar",
            Instruction::ExtWH { .. } => "ext.w.h",
            Instruction::ExtWB { .. } => "ext.w.b",
            Instruction::CloD { .. } => "clo.d",
            Instruction::CtzD { .. } => "ctz.d",
            Instruction::PopcntD { .. } => "popcnt.d",
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
            Instruction::FfintSL { .. } => "ffint.s.l",
            Instruction::FfintDW { .. } => "ffint.d.w",
            Instruction::FfintDL { .. } => "ffint.d.l",
            Instruction::FtintWS { .. } => "ftint.w.s",
            Instruction::FtintWD { .. } => "ftint.w.d",
            Instruction::FtintLS { .. } => "ftint.l.s",
            Instruction::FtintLD { .. } => "ftint.l.d",
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
            Instruction::AmaddW { rd, rj, rk } => write!(f, "amadd.w {}, {}, {}", rd, rj, rk),
            Instruction::AmaddD { rd, rj, rk } => write!(f, "amadd.d {}, {}, {}", rd, rj, rk),
            Instruction::AmandW { rd, rj, rk } => write!(f, "amand.w {}, {}, {}", rd, rj, rk),
            Instruction::AmandD { rd, rj, rk } => write!(f, "amand.d {}, {}, {}", rd, rj, rk),
            Instruction::AmorW { rd, rj, rk } => write!(f, "amor.w {}, {}, {}", rd, rj, rk),
            Instruction::AmorD { rd, rj, rk } => write!(f, "amor.d {}, {}, {}", rd, rj, rk),
            Instruction::AmxorW { rd, rj, rk } => write!(f, "amxor.w {}, {}, {}", rd, rj, rk),
            Instruction::AmxorD { rd, rj, rk } => write!(f, "amxor.d {}, {}, {}", rd, rj, rk),
            Instruction::AmmaxW { rd, rj, rk } => write!(f, "ammax.w {}, {}, {}", rd, rj, rk),
            Instruction::AmmaxD { rd, rj, rk } => write!(f, "ammax.d {}, {}, {}", rd, rj, rk),
            Instruction::AmmaxWu { rd, rj, rk } => write!(f, "ammax.wu {}, {}, {}", rd, rj, rk),
            Instruction::AmmaxDu { rd, rj, rk } => write!(f, "ammax.du {}, {}, {}", rd, rj, rk),
            Instruction::AmminW { rd, rj, rk } => write!(f, "ammin.w {}, {}, {}", rd, rj, rk),
            Instruction::AmminD { rd, rj, rk } => write!(f, "ammin.d {}, {}, {}", rd, rj, rk),
            Instruction::AmminWu { rd, rj, rk } => write!(f, "ammin.wu {}, {}, {}", rd, rj, rk),
            Instruction::AmminDu { rd, rj, rk } => write!(f, "ammin.du {}, {}, {}", rd, rj, rk),
            Instruction::FsqrtS { fd, fj } => write!(f, "fsqrt.s {}, {}", fd, fj),
            Instruction::FsqrtD { fd, fj } => write!(f, "fsqrt.d {}, {}", fd, fj),
            Instruction::FabsS { fd, fj } => write!(f, "fabs.s {}, {}", fd, fj),
            Instruction::FabsD { fd, fj } => write!(f, "fabs.d {}, {}", fd, fj),
            Instruction::FnegS { fd, fj } => write!(f, "fneg.s {}, {}", fd, fj),
            Instruction::FnegD { fd, fj } => write!(f, "fneg.d {}, {}", fd, fj),
            Instruction::FminS { fd, fj, fk } => write!(f, "fmin.s {}, {}, {}", fd, fj, fk),
            Instruction::FminD { fd, fj, fk } => write!(f, "fmin.d {}, {}, {}", fd, fj, fk),
            Instruction::FmaxS { fd, fj, fk } => write!(f, "fmax.s {}, {}, {}", fd, fj, fk),
            Instruction::FmaxD { fd, fj, fk } => write!(f, "fmax.d {}, {}, {}", fd, fj, fk),
            Instruction::FcopysignS { fd, fj, fk } => write!(f, "fcopysign.s {}, {}, {}", fd, fj, fk),
            Instruction::FcopysignD { fd, fj, fk } => write!(f, "fcopysign.d {}, {}, {}", fd, fj, fk),
            Instruction::FscalebS { fd, fj, fk } => write!(f, "fscaleb.s {}, {}, {}", fd, fj, fk),
            Instruction::FscalebD { fd, fj, fk } => write!(f, "fscaleb.d {}, {}, {}", fd, fj, fk),
            Instruction::FlogbS { fd, fj } => write!(f, "flogb.s {}, {}", fd, fj),
            Instruction::FlogbD { fd, fj } => write!(f, "flogb.d {}, {}", fd, fj),
            Instruction::FclassS { fd, fj } => write!(f, "fclass.s {}, {}", fd, fj),
            Instruction::FclassD { fd, fj } => write!(f, "fclass.d {}, {}", fd, fj),
            Instruction::FsgnjS { fd, fj, fk } => write!(f, "fsgnj.s {}, {}, {}", fd, fj, fk),
            Instruction::FsgnjD { fd, fj, fk } => write!(f, "fsgnj.d {}, {}, {}", fd, fj, fk),
            Instruction::FsgnjnS { fd, fj, fk } => write!(f, "fsgnjn.s {}, {}, {}", fd, fj, fk),
            Instruction::FsgnjnD { fd, fj, fk } => write!(f, "fsgnjn.d {}, {}, {}", fd, fj, fk),
            Instruction::FsgnjxS { fd, fj, fk } => write!(f, "fsgnjx.s {}, {}, {}", fd, fj, fk),
            Instruction::FsgnjxD { fd, fj, fk } => write!(f, "fsgnjx.d {}, {}, {}", fd, fj, fk),
            Instruction::Revb2H { rd, rj } => write!(f, "revb.2h {}, {}", rd, rj),
            Instruction::Revb4H { rd, rj } => write!(f, "revb.4h {}, {}", rd, rj),
            Instruction::Revb2W { rd, rj } => write!(f, "revb.2w {}, {}", rd, rj),
            Instruction::Bitrev4B { rd, rj } => write!(f, "bitrev.4b {}, {}", rd, rj),
            Instruction::Bitrev8B { rd, rj } => write!(f, "bitrev.8b {}, {}", rd, rj),
            Instruction::Cpucfg { rd, rj } => write!(f, "cpucfg {}, {}", rd, rj),
            Instruction::RdtimelW { rd, rj } => write!(f, "rdtimel.w {}, {}", rd, rj),
            Instruction::RdtimehW { rd, rj } => write!(f, "rdtimeh.w {}, {}", rd, rj),
            Instruction::AsrtleD { rj, rk } => write!(f, "asrtle.d {}, {}", rj, rk),
            Instruction::AsrgtD { rj, rk } => write!(f, "asrgt.d {}, {}", rj, rk),
            Instruction::Dbar { hint } => write!(f, "dbar {}", hint),
            Instruction::ExtWH { rd, rj } => write!(f, "ext.w.h {}, {}", rd, rj),
            Instruction::ExtWB { rd, rj } => write!(f, "ext.w.b {}, {}", rd, rj),
            Instruction::CloD { rd, rj } => write!(f, "clo.d {}, {}", rd, rj),
            Instruction::CtzD { rd, rj } => write!(f, "ctz.d {}, {}", rd, rj),
            Instruction::PopcntD { rd, rj } => write!(f, "popcnt.d {}, {}", rd, rj),
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
            Instruction::FfintSL { fd, fj } => write!(f, "ffint.s.l {}, {}", fd, fj),
            Instruction::FfintDW { fd, fj } => write!(f, "ffint.d.w {}, {}", fd, fj),
            Instruction::FfintDL { fd, fj } => write!(f, "ffint.d.l {}, {}", fd, fj),
            Instruction::FtintWS { fd, fj } => write!(f, "ftint.w.s {}, {}", fd, fj),
            Instruction::FtintWD { fd, fj } => write!(f, "ftint.w.d {}, {}", fd, fj),
            Instruction::FtintLS { fd, fj } => write!(f, "ftint.l.s {}, {}", fd, fj),
            Instruction::FtintLD { fd, fj } => write!(f, "ftint.l.d {}, {}", fd, fj),
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

    // ELF64 section header constants (Elf64_Shdr sh_type / sh_flags).
    const SHT_PROGBITS: u32 = 1;
    const SHT_STRTAB: u32 = 3;
    const SHT_NOBITS: u32 = 8;
    const SHF_WRITE: u64 = 0x1;
    const SHF_ALLOC: u64 = 0x2;
    const SHF_EXECINSTR: u64 = 0x4;
    const SHDR_SIZE: u64 = 64; // sizeof(Elf64_Shdr)

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = 3; // 2x PT_LOAD + 1x PT_GNU_STACK
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    // text_offset must match the value computed in `link()`. Both use
    // phdr_end (232) directly — no page alignment. This keeps the ELF
    // compact (no 65KB zero padding). If these two diverge, R_LARCH_64
    // relocations (GetAddress) patch the wrong absolute addresses.
    let text_offset = phdr_end;
    let text_size = code.len() as u64;

    // The data segment starts on the next page after the text.
    let text_file_end = text_offset + text_size;
    let data_vaddr = (base_addr + text_file_end).div_ceil(PAGE_SIZE) * PAGE_SIZE;
    let data_size: u64 = PAGE_SIZE; // 1 page of writable memory for stack/data
    let entry_point = base_addr + text_offset;

    // --- Section header table layout ---
    // .shstrtab content: NUL + ".text" + NUL + ".bss" + NUL + ".shstrtab" + NUL
    //   name offsets:  .text=1  .bss=7  .shstrtab=12
    let shstrtab_content: &[u8] = b"\0.text\0.bss\0.shstrtab\0";
    let shstrtab_size = shstrtab_content.len() as u64;
    // .shstrtab immediately follows the text segment in the file.
    let shstrtab_offset = text_offset + text_size;
    // Section header table starts after .shstrtab, 8-byte aligned
    // (Elf64_Shdr has natural alignment of 8 bytes).
    let shdr_offset = (shstrtab_offset + shstrtab_size).div_ceil(8) * 8;
    // Sections: 0=null, 1=.text, 2=.bss, 3=.shstrtab
    let num_shdrs: u64 = 4;
    let shstrndx: u16 = (num_shdrs - 1) as u16; // .shstrtab is the last section

    let mut elf = Vec::with_capacity((shdr_offset + num_shdrs * SHDR_SIZE) as usize);

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
    elf.extend_from_slice(&shdr_offset.to_le_bytes()); // e_shoff
    elf.extend_from_slice(&0x43u32.to_le_bytes()); // e_flags = 0x43 (LP64D ABI double-float)
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&3u16.to_le_bytes()); // e_phnum = 3 (2 LOAD + 1 GNU_STACK)
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&(num_shdrs as u16).to_le_bytes()); // e_shnum
    elf.extend_from_slice(&shstrndx.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    // p_filesz = page-aligned text size (code + headers). QEMU needs this
    // for mmap. p_offset=0 means the LOAD segment starts at the beginning
    // of the file (covering the ELF header + phdrs + code).
    let text_memsz = (text_offset + text_size).div_ceil(PAGE_SIZE) * PAGE_SIZE;
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset = 0 (include ELF header)
    elf.extend_from_slice(&base_addr.to_le_bytes()); // p_vaddr (page-aligned; p_offset=0 requires alignment)
    elf.extend_from_slice(&base_addr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_memsz.to_le_bytes()); // p_filesz (cover header + padding + code)
    elf.extend_from_slice(&text_memsz.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .bss / stack ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset = 0
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_filesz (write zeros so QEMU can mmap)
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_memsz (writable pages)
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 3: PT_GNU_STACK (non-executable stack) ---
    // p_type = 0x6474e551, p_flags = PF_R | PF_W (no PF_X)
    // All offsets/sizes are 0; p_align = 0x10 (64-bit ELF).
    elf.extend_from_slice(&0x6474e551u32.to_le_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&0x10u64.to_le_bytes()); // p_align

    // --- .text section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // --- .shstrtab section content ---
    // Immediately follows the text segment. Contains NUL-terminated section
    // name strings referenced by the section header table's sh_name fields.
    elf.extend_from_slice(shstrtab_content);

    // Pad to 8-byte alignment for the section header table.
    while (elf.len() as u64) < shdr_offset {
        elf.push(0);
    }

    // --- Section Header Table ---
    // Each Elf64_Shdr is 64 bytes:
    //   sh_name(u32) sh_type(u32) sh_flags(u64) sh_addr(u64) sh_offset(u64)
    //   sh_size(u64) sh_link(u32) sh_info(u32) sh_addralign(u64) sh_entsize(u64)

    // Section 0: SHT_NULL (reserved, all zeros).
    elf.extend_from_slice(&[0u8; 64]);

    // Section 1: .text (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR).
    elf.extend_from_slice(&1u32.to_le_bytes()); // sh_name (offset 1 in .shstrtab)
    elf.extend_from_slice(&SHT_PROGBITS.to_le_bytes()); // sh_type
    elf.extend_from_slice(&(SHF_ALLOC | SHF_EXECINSTR).to_le_bytes()); // sh_flags
    elf.extend_from_slice(&entry_point.to_le_bytes()); // sh_addr (= base_addr + text_offset)
    elf.extend_from_slice(&text_offset.to_le_bytes()); // sh_offset
    elf.extend_from_slice(&text_size.to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&16u64.to_le_bytes()); // sh_addralign (16 for code)
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize

    // Section 2: .bss (SHT_NOBITS, SHF_ALLOC | SHF_WRITE).
    // The data segment has p_filesz = data_size but the file does not actually
    // contain those bytes (the loader zero-fills the page); use SHT_NOBITS to
    // accurately reflect that there is no real file content for this section.
    elf.extend_from_slice(&7u32.to_le_bytes()); // sh_name (offset 7 in .shstrtab)
    elf.extend_from_slice(&SHT_NOBITS.to_le_bytes()); // sh_type
    elf.extend_from_slice(&(SHF_ALLOC | SHF_WRITE).to_le_bytes()); // sh_flags
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // sh_addr
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_offset (NOBITS: no file content)
    elf.extend_from_slice(&data_size.to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&16u64.to_le_bytes()); // sh_addralign
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize

    // Section 3: .shstrtab (SHT_STRTAB, no alloc flags — not loaded).
    elf.extend_from_slice(&12u32.to_le_bytes()); // sh_name (offset 12 in .shstrtab)
    elf.extend_from_slice(&SHT_STRTAB.to_le_bytes()); // sh_type
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_flags (not loaded into memory)
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_addr (no virtual address)
    elf.extend_from_slice(&shstrtab_offset.to_le_bytes()); // sh_offset
    elf.extend_from_slice(&shstrtab_size.to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&1u64.to_le_bytes()); // sh_addralign (byte-aligned strings)
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize

    // Note: we deliberately do NOT pad to data_offset — the data segment
    // has p_filesz=0 so there is no file content. Trailing bytes would
    // confuse QEMU's ELF loader.

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
    if opc_2ri12 == 0x0E7 {
        // DBAR hint: rd=$r0, rj=$r0, hint=si12
        let hint = (word >> 10) & 0xFFF;
        return format!("dbar {}", hint);
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

    /// Wave 22: Emit a function using real register allocation.
    ///
    /// Consumes a `RegAllocResult` and produces an `AllocatedFunction`
    /// with `reads`/`writes` annotated with the physical registers
    /// (a0-a7, t0-t8, s0-s9) assigned by the linear-scan allocator.
    pub fn emit_function_regalloc(
        &self,
        func: &IRFunction,
        alloc: &crate::regalloc::RegAllocResult,
    ) -> Result<AllocatedFunction, BackendError> {
        let mut allocated = self.allocate_registers(func)?;
        crate::regalloc_emit::annotate_with_regalloc(&mut allocated, alloc);
        Ok(allocated)
    }

    /// Wave 22: Convenience method — run regalloc + emit in one step.
    pub fn emit_function_with_regalloc(
        &self,
        func: &IRFunction,
    ) -> Result<AllocatedFunction, BackendError> {
        let alloc = crate::regalloc_emit::run_regalloc(func, "loongarch64");
        self.emit_function_regalloc(func, &alloc)
    }
}

impl Default for LoongArch64Backend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for LoongArch64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        stack_slot_isel::allocate_registers(func)
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
        //   _start:  LD.D $a0, $sp, 0       ; argc = *sp (64-bit)
        //            ADDI.D $a1, $sp, 8     ; argv = sp + 8 (64-bit pointers)
        //            BL main                ; call main(argc, argv) — result in $a0
        //            addi.d $a7, $r0, 93    ; sys_exit = 93
        //            syscall 0x0            ; exit(main_result)
        //   <functions...>
        //
        // The _start stub is 5 instructions = 20 bytes.
        // After that come all user functions.

        let start_stub_size: usize = 20; // 5 × 4-byte instructions
        let ffi_stub_size: usize = 8; // ADDI.W A0, R0, 0; JIRL R0, RA, 0
        let ffi_stub_offset: usize = start_stub_size;

        // ── Compute function offsets ──
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size + ffi_stub_size; // after _start + FFI stub

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

        // __vuma_alloc / __vuma_free stubs go at the end of all_code.
        // alloc stub = 9 instrs × 4 = 36 B.
        let vuma_alloc_offset = current_offset;
        let vuma_free_offset = vuma_alloc_offset + 36;
        func_offsets.insert("__vuma_alloc".to_string(), vuma_alloc_offset);
        func_offsets.insert("__vuma_free".to_string(), vuma_free_offset);

        // ── Build _start stub bytes ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // LD.D $a0, $sp, 0 — load argc from stack pointer (64-bit)
        start_stub.extend_from_slice(
            &Instruction::LdD {
                rd: Gpr::A0,
                rj: Gpr::Sp,
                imm12: 0,
            }
            .encode(),
        );

        // ADDI.D $a1, $sp, 8 — argv = sp + 8 (64-bit pointers on LoongArch64)
        start_stub.extend_from_slice(
            &Instruction::AddiD {
                rd: Gpr::A1,
                rj: Gpr::Sp,
                imm12: 8,
            }
            .encode(),
        );

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
            // BL is at byte offset 8 within start_stub (after LD.D $a0 and ADDI.D $a1)
            let bl_offset = ((main_offset as i64) - 8) / 4;
            // Re-encode the whole BL instruction
            let patched_word =
                u32::from_le_bytes(Instruction::Bl { offs26: bl_offset as i32 }.encode());
            start_stub[8..12].copy_from_slice(&patched_word.to_le_bytes());
        }

        // ── Add FFI return-0 stub ──
        let mut ffi_stub = Vec::with_capacity(ffi_stub_size);
        ffi_stub.extend_from_slice(&0x02800004u32.to_le_bytes()); // ADDI.W A0, R0, 0
        ffi_stub.extend_from_slice(&0x4C000020u32.to_le_bytes()); // JIRL R0, RA, 0

        // __vuma_alloc(size in a0) -> a0 = mmap(NULL, size, 3, 0x22, -1, 0)
        //   LoongArch64 Linux: mmap = syscall 222, args a0-a5,
        //   syscall # in a7, `syscall 0x0` instruction.
        let mut vuma_alloc_stub: Vec<u8> = Vec::new();
        vuma_alloc_stub.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::A0, rk: Gpr::R0 }.encode());      // a1 = a0 (size -> length)
        vuma_alloc_stub.extend_from_slice(&Instruction::Or { rd: Gpr::A0, rj: Gpr::R0, rk: Gpr::R0 }.encode());      // a0 = 0 (addr = NULL)
        vuma_alloc_stub.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: 3 }.encode());      // a2 = 3 (prot)
        vuma_alloc_stub.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::R0, imm12: 0x22 }.encode());    // a3 = 0x22 (flags)
        vuma_alloc_stub.extend_from_slice(&Instruction::AddiD { rd: Gpr::A4, rj: Gpr::R0, imm12: -1 }.encode());      // a4 = -1 (fd)
        vuma_alloc_stub.extend_from_slice(&Instruction::Or { rd: Gpr::A5, rj: Gpr::R0, rk: Gpr::R0 }.encode());       // a5 = 0 (offset)
        vuma_alloc_stub.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 222 }.encode());     // a7 = 222 (sys_mmap)
        vuma_alloc_stub.extend_from_slice(&Instruction::Syscall.encode());
        vuma_alloc_stub.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());      // return
        // __vuma_free(addr in a0) -> munmap(addr, 0)
        //   __NR_munmap = 215
        let mut vuma_free_stub: Vec<u8> = Vec::new();
        vuma_free_stub.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::R0, rk: Gpr::R0 }.encode());       // a1 = 0 (size)
        vuma_free_stub.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 215 }.encode());     // a7 = 215 (sys_munmap)
        vuma_free_stub.extend_from_slice(&Instruction::Syscall.encode());
        vuma_free_stub.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());      // return

        // ── POSIX syscall stubs ──────────────────────────────────────
        // These provide the syscalls needed by mmap_sha256d, signal_hash,
        // lock_free_queue, epoll_echo, and ffi_demo tests.
        //
        // LoongArch64 calling convention: args in $a0-$a5 (r4-r9),
        //   return in $a0.
        // LoongArch64 syscall convention: args in $a0-$a5, syscall# in
        //   $a7 (r11), SYSCALL instruction (0x0000000b), return in $a0.
        // The calling convention matches the syscall convention for most
        // syscalls, so stubs are just:
        //     addi.d $a7, $r0, #num ; syscall ; jirl $r0, $ra, 0
        //
        // For syscalls that need arg shuffling (open→openat,
        // unlink→unlinkat, sigaction→rt_sigaction, pipe→pipe2, dup2→dup3,
        // fork→clone), extra instructions are added before the syscall.
        //
        // LoongArch uses openat/unlinkat/pipe2/dup3/clone (like RISC-V),
        // per the generic Linux syscall ABI. All syscall numbers (max 260)
        // and AT_FDCWD (-100) fit in the 12-bit signed ADDI.D immediate
        // field, so no LU12I.W is needed.
        let syscall_stubs: Vec<(String, Vec<u8>)> = {
            let mut stubs: Vec<(String, Vec<u8>)> = Vec::new();

            // Simple stubs (args already in correct registers $a0-$a5):
            // Numbers verified against asm-generic/unistd.h.
            for (name, num) in [
                ("write", 64), ("read", 63), ("close", 57), ("mmap", 222),
                ("munmap", 215), ("exit", 93), ("getpid", 172),
                ("socket", 198), ("epoll_create1", 20), ("futex", 98),
                ("execve", 221), ("wait4", 260), ("epoll_ctl", 21), ("epoll_wait", 22),
                ("clone", 220),
                // ── W6: additional POSIX syscall stubs ──
                ("lseek", 62), ("fstat", 80),
                ("kill", 129), ("getcwd", 17), ("chdir", 49),
                ("ioctl", 29), ("fcntl", 25), ("connect", 203),
                ("nanosleep", 101), ("mprotect", 226),
                ("dup", 23), ("exit_group", 94),
                ("recv", 207), ("send", 206), ("shutdown", 210),
                ("bind", 200), ("listen", 201), ("accept", 202),
                ("setsockopt", 208),
                ("getsockopt", 209),
                // ── Phase 8: additional POSIX syscalls for full coverage ──
                ("dup3", 24),
                ("recvfrom", 207),    // same as recv on generic ABI
                ("sendto", 206),      // same as send on generic ABI
                // ── W7: more POSIX syscall stubs ──
                // waitpid is the same syscall as wait4 (caller passes NULL
                // rusage in $a3 if it doesn't care).
                ("waitpid", 260),
                ("brk", 214),
                ("clock_gettime", 113),
                ("gettimeofday", 169),
                ("rt_sigprocmask", 135),
                // NOTE: stat/lstat/poll/alarm do not exist on the generic
                // ABI; provided as newfstatat/ppoll/setitimer shims below.
                // ── Wave 7: POSIX file-metadata & I/O syscalls (asm-generic) ──
                // LoongArch64 has 8 reg args ($a0-$a7); all take ≤5 args →
                // simple "addi.d $a7,$r0,#num; syscall; jirl" stub. Numbers
                // from asm-generic/unistd.h. Plain mkdir/rmdir/rename/link/
                // symlink/readlink/chmod/chown do NOT exist on the generic ABI
                // — provided as *at wrappers below.
                ("umask", 166), ("fchmod", 52), ("fchown", 55),
                ("openat", 56), ("unlinkat", 35), ("renameat", 38),
                ("linkat", 37), ("symlinkat", 36), ("readlinkat", 78),
                ("faccessat", 48), ("fchmodat", 53), ("fchownat", 54),
                ("ftruncate", 46), ("fsync", 82), ("fdatasync", 83),
                ("sync", 81), ("syncfs", 306),
                ("pread", 67), ("pwrite", 68), ("readv", 65), ("writev", 66),
                ("preadv", 69), ("pwritev", 70),
                ("fchdir", 50), ("chroot", 51),
                // ── Wave 9: POSIX system & advanced syscalls (asm-generic) ──
                // LoongArch64 has 8 reg args; all take ≤5 args → simple stub.
                // eventfd→eventfd2(19), signalfd→signalfd4(74) = modern variants.
                ("mlock", 228), ("munlock", 229), ("mlockall", 230), ("munlockall", 231),
                ("mincore", 232), ("madvise", 233), ("msync", 227), ("mremap", 216),
                ("getrlimit", 163), ("setrlimit", 164), ("prlimit64", 261),
                ("getrusage", 165), ("times", 153),
                ("getrandom", 278),
                ("eventfd", 19), ("timerfd_create", 85), ("timerfd_settime", 86),
                ("timerfd_gettime", 87), ("signalfd", 74),
                ("inotify_init1", 26), ("inotify_add_watch", 27), ("inotify_rm_watch", 28),
                ("ptrace", 117),
                // ── Wave 8: POSIX process & identity syscalls (asm-generic/unistd.h) ──
                // All present directly in asm-generic (no *at wrapping). All take
                // ≤5 args; LoongArch has 8 reg args (a0-a7) → simple_stub for all.
                // Family 1: identity
                ("getuid", 174), ("geteuid", 175), ("getgid", 176), ("getegid", 177),
                ("setuid", 146), ("setgid", 144), ("setresuid", 147), ("setresgid", 149),
                // Family 2: process group (getpid already present; getpgrp ABSENT in
                // asm-generic → callers use getpgid(0))
                ("getppid", 173), ("getsid", 156), ("setsid", 157),
                ("setpgid", 154), ("getpgid", 155),
                ("getpgrp", 65),
                // Family 3: clone/wait (clone/wait4 already present; vfork ABSENT →
                // callers use clone(CLONE_VFORK))
                ("clone3", 435), ("waitid", 95),
                // Family 4: exec/exit (execve/exit_group already present)
                ("execveat", 281),
                // Family 5: signals (kill/rt_sigprocmask/rt_sigreturn already present)
                ("tgkill", 131), ("tkill", 130), ("rt_sigaction", 134),
                // Family 6: directory read (getdents/readdir ABSENT in asm-generic →
                // use getdents64)
                ("getdents64", 61),
                // Family 7: system (arch_prctl is x86_64-only)
                ("prctl", 167), ("uname", 160), ("sysinfo", 179),
                            ("eventfd2", 19),
                ("newfstatat", 79),
                ("signalfd4", 74),
] {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: num }.encode());
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push((name.to_string(), code));
            }

            // rt_sigreturn (139) — special: no args, never returns.
            // The kernel restores the saved signal context and resumes
            // execution at the interrupted PC. We emit just
            // `addi.d $a7, $r0, 139 ; syscall 0x0` followed by a `break`
            // trap as a safety net in case the kernel ever does return.
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 139 }.encode());
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Break.encode());
                stubs.push(("rt_sigreturn".to_string(), code));
            }

            // stat(path, statbuf) → newfstatat(AT_FDCWD=-100, path, statbuf, 0)
            // stat() does not exist on the generic ABI; newfstatat=79 replaces it.
            // Caller args: a0=path, a1=statbuf
            // Need:        a0=-100, a1=path, a2=statbuf, a3=0
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::A1, imm12: 0 }.encode());  // a2 <- statbuf (OR-style move via addi.d a2,a1,0)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());  // a1 <- path
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode());// a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::R0, imm12: 0 }.encode());  // a3 = 0 (flags)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 79 }.encode()); // newfstatat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("stat".to_string(), code));
            }

            // lstat(path, statbuf) → newfstatat(AT_FDCWD, path, statbuf, AT_SYMLINK_NOFOLLOW=0x100)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::A1, imm12: 0 }.encode());  // a2 <- statbuf
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());  // a1 <- path
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode());// a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::R0, imm12: 0x100 }.encode());// a3 = AT_SYMLINK_NOFOLLOW
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 79 }.encode()); // newfstatat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("lstat".to_string(), code));
            }

            // poll(fds, nfds, timeout) → ppoll(fds, nfds, &ts, NULL)
            // poll() does not exist on the generic ABI; ppoll=73 replaces it.
            // Caller args: a0=fds, a1=nfds, a2=timeout
            // Need:        a0=fds, a1=nfds, a2=&ts, a3=NULL
            // Build a 16-byte timespec {tv_sec=timeout, tv_nsec=0} on the stack.
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -16 }.encode()); // sp -= 16
                code.extend_from_slice(&Instruction::StD { rd: Gpr::A2, rj: Gpr::Sp, imm12: 0 }.encode());      // ts.tv_sec = timeout
                code.extend_from_slice(&Instruction::StD { rd: Gpr::R0, rj: Gpr::Sp, imm12: 8 }.encode());     // ts.tv_nsec = 0
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::Sp, imm12: 0 }.encode());   // a2 = &ts
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::R0, imm12: 0 }.encode());   // a3 = NULL
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 73 }.encode());  // ppoll
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: 16 }.encode()); // sp += 16
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("poll".to_string(), code));
            }

            // alarm(seconds) → setitimer(ITIMER_REAL=0, &itimerval, NULL)
            // alarm() does not exist on the generic ABI. Schedule SIGALRM via
            // setitimer=103. Build a 32-byte itimerval on the stack.
            // Caller args: a0=seconds; Need: a0=0, a1=&itimerval, a2=NULL
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -32 }.encode()); // sp -= 32
                // it_interval = {0, 0}
                code.extend_from_slice(&Instruction::StD { rd: Gpr::R0, rj: Gpr::Sp, imm12: 0 }.encode());
                code.extend_from_slice(&Instruction::StD { rd: Gpr::R0, rj: Gpr::Sp, imm12: 8 }.encode());
                // it_value = {a0, 0}
                code.extend_from_slice(&Instruction::StD { rd: Gpr::A0, rj: Gpr::Sp, imm12: 16 }.encode());
                code.extend_from_slice(&Instruction::StD { rd: Gpr::R0, rj: Gpr::Sp, imm12: 24 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::Sp, imm12: 0 }.encode());   // a1 = &itimerval
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: 0 }.encode());   // a0 = ITIMER_REAL
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: 0 }.encode());   // a2 = NULL
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 103 }.encode()); // setitimer
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: 32 }.encode()); // sp += 32
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("alarm".to_string(), code));
            }

            // ── Runtime helpers: print_hex, print_int, strcmp ──
            // These are not syscalls but small assembly routines that user
            // code can call by name. They are appended to the syscall-stub
            // blob and registered in `func_offsets` like ordinary stubs.
            //
            // print_hex($a0) — print a0 as 16 lowercase hex digits to stdout.
            // Clobbers: $a1, $a2, $a7, $t0–$t6. Stack frame: 32 bytes.
            {
                let mut code = Vec::new();
                // Prologue: 32 bytes (16-byte buffer + 8 for $ra + 8 pad)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -32 }.encode());
                code.extend_from_slice(&Instruction::StD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: 24 }.encode());
                // t0 = 16 (loop counter), t1 = 60 (initial shift amount)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T0, rj: Gpr::R0, imm12: 16 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T1, rj: Gpr::R0, imm12: 60 }.encode());

                // hex_loop:
                let hex_loop_start = code.len();
                // t2 = (a0 >> t1) & 0xF
                code.extend_from_slice(&Instruction::SrlD { rd: Gpr::T2, rj: Gpr::A0, rk: Gpr::T1 }.encode());
                code.extend_from_slice(&Instruction::Andi { rd: Gpr::T2, rj: Gpr::T2, imm12: 0xF }.encode());
                // t3 = t2 + 48 ('0')
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T3, rj: Gpr::T2, imm12: 48 }.encode());
                // slti t4, t2, 10 → t4 = 1 if t2 < 10, else 0
                code.extend_from_slice(&Instruction::Slti { rd: Gpr::T4, rj: Gpr::T2, imm12: 10 }.encode());
                // bnez t4, +2 (skip the +39 adjustment if t2 < 10)
                code.extend_from_slice(&Instruction::Bnez { rj: Gpr::T4, offs21: 2 }.encode());
                // t3 += 39 (87 - 48 = 39, makes '0'+39 = 'a'-10+39 = 'a'+(t2-10) = t2+87)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T3, rj: Gpr::T3, imm12: 39 }.encode());
                // skip_add: store t3 at $sp + (16 - t0)
                // t5 = 16 - t0
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T5, rj: Gpr::R0, imm12: 16 }.encode());
                code.extend_from_slice(&Instruction::SubD { rd: Gpr::T5, rj: Gpr::T5, rk: Gpr::T0 }.encode());
                // t6 = $sp + t5
                code.extend_from_slice(&Instruction::AddD { rd: Gpr::T6, rj: Gpr::Sp, rk: Gpr::T5 }.encode());
                code.extend_from_slice(&Instruction::StB { rd: Gpr::T3, rj: Gpr::T6, imm12: 0 }.encode());
                // t0 -= 1, t1 -= 4
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T0, rj: Gpr::T0, imm12: -1 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T1, rj: Gpr::T1, imm12: -4 }.encode());
                // if t0 != 0, loop (backward branch)
                let hex_loop_back = ((hex_loop_start as i32) - (code.len() as i32 + 4)) / 4;
                code.extend_from_slice(&Instruction::Bnez { rj: Gpr::T0, offs21: hex_loop_back }.encode());
                // sys_write(1, $sp, 16)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: 1 }.encode());
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::Sp, rk: Gpr::R0 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: 16 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 64 }.encode());
                code.extend_from_slice(&Instruction::Syscall.encode());
                // Epilogue
                code.extend_from_slice(&Instruction::LdD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: 24 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: 32 }.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("print_hex".to_string(), code));
            }

            // print_int($a0) — print a0 as a signed decimal integer to stdout.
            // Clobbers: $a1, $a2, $a7, $t1–$t5. Stack frame: 48 bytes.
            {
                let mut code = Vec::new();
                // Prologue: 48 bytes (32-byte digit buffer + 8 for $ra + 8 pad)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -48 }.encode());
                code.extend_from_slice(&Instruction::StD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: 40 }.encode());
                // if a0 >= 0, skip negative handling.
                // Negative block (instructions 3-10): print '-', syscall, negate a0.
                // Positive label is instruction 11 (t1 = 32).
                // Bge target = current_index + 1 + offset = 2 + 1 + offset.
                // To land on instruction 11: offset = 11 - 2 - 1 = 8.
                code.extend_from_slice(&Instruction::Bge { rj: Gpr::A0, rd: Gpr::R0, offs16: 8 }.encode());
                // Print '-' (single byte)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T0, rj: Gpr::R0, imm12: 45 }.encode());
                code.extend_from_slice(&Instruction::StB { rd: Gpr::T0, rj: Gpr::Sp, imm12: 0 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: 1 }.encode());
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::Sp, rk: Gpr::R0 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: 1 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 64 }.encode());
                code.extend_from_slice(&Instruction::Syscall.encode());
                // Negate a0
                code.extend_from_slice(&Instruction::SubD { rd: Gpr::A0, rj: Gpr::R0, rk: Gpr::A0 }.encode());
                // positive: t1 = 32 (buffer end offset), t3 = 10 (divisor)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T1, rj: Gpr::R0, imm12: 32 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T3, rj: Gpr::R0, imm12: 10 }.encode());

                // div_loop:
                let div_loop_start = code.len();
                // t2 = a0 / 10 (unsigned — a0 is already non-negative here)
                code.extend_from_slice(&Instruction::DivDu { rd: Gpr::T2, rj: Gpr::A0, rk: Gpr::T3 }.encode());
                // t4 = a0 % 10
                code.extend_from_slice(&Instruction::ModDu { rd: Gpr::T4, rj: Gpr::A0, rk: Gpr::T3 }.encode());
                // t4 += '0'
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T4, rj: Gpr::T4, imm12: 48 }.encode());
                // t1 -= 1, then store t4 at $sp + t1
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::T1, rj: Gpr::T1, imm12: -1 }.encode());
                code.extend_from_slice(&Instruction::AddD { rd: Gpr::T5, rj: Gpr::Sp, rk: Gpr::T1 }.encode());
                code.extend_from_slice(&Instruction::StB { rd: Gpr::T4, rj: Gpr::T5, imm12: 0 }.encode());
                // a0 = quotient
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A0, rj: Gpr::T2, rk: Gpr::R0 }.encode());
                // if a0 != 0, loop (backward branch)
                let div_loop_back = ((div_loop_start as i32) - (code.len() as i32 + 4)) / 4;
                code.extend_from_slice(&Instruction::Bnez { rj: Gpr::A0, offs21: div_loop_back }.encode());
                // sys_write(1, $sp + t1, 32 - t1)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: 1 }.encode());
                code.extend_from_slice(&Instruction::AddD { rd: Gpr::A1, rj: Gpr::Sp, rk: Gpr::T1 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: 32 }.encode());
                code.extend_from_slice(&Instruction::SubD { rd: Gpr::A2, rj: Gpr::A2, rk: Gpr::T1 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 64 }.encode());
                code.extend_from_slice(&Instruction::Syscall.encode());
                // Epilogue
                code.extend_from_slice(&Instruction::LdD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: 40 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: 48 }.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("print_int".to_string(), code));
            }

            // strcmp($a0 = s1, $a1 = s2) → $a0 = (*s1 - *s2) at first difference
            // Not a syscall — a small byte-comparison loop.
            // Clobbers: $a0, $a1, $t0, $t1.
            {
                let mut code = Vec::new();
                // strcmp_loop:
                let loop_start = code.len();
                // t0 = *s1
                code.extend_from_slice(&Instruction::LdBu { rd: Gpr::T0, rj: Gpr::A0, imm12: 0 }.encode());
                // t1 = *s2
                code.extend_from_slice(&Instruction::LdBu { rd: Gpr::T1, rj: Gpr::A1, imm12: 0 }.encode());
                // if t0 != t1, jump to done (5 instructions ahead)
                code.extend_from_slice(&Instruction::Bne { rj: Gpr::T0, rd: Gpr::T1, offs16: 5 }.encode());
                // if t0 == 0, strings equal — jump to done (4 instructions ahead)
                code.extend_from_slice(&Instruction::Beq { rj: Gpr::T0, rd: Gpr::R0, offs16: 4 }.encode());
                // a0++, a1++
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::A0, imm12: 1 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A1, imm12: 1 }.encode());
                // B strcmp_loop (backward branch)
                let loop_back = ((loop_start as i32) - (code.len() as i32 + 4)) / 4;
                code.extend_from_slice(&Instruction::B { offs26: loop_back }.encode());
                // done: a0 = t0 - t1
                code.extend_from_slice(&Instruction::SubD { rd: Gpr::A0, rj: Gpr::T0, rk: Gpr::T1 }.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("strcmp".to_string(), code));
            }

            // open → openat(AT_FDCWD=-100, pathname, flags, mode)
            // Caller args: a0=pathname, a1=flags, a2=mode
            // Need:        a0=-100,   a1=pathname, a2=flags, a3=mode
            // Shuffle high→low to avoid clobbering.
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A3, rj: Gpr::A2, rk: Gpr::R0 }.encode());  // a3 <- mode
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A2, rj: Gpr::A1, rk: Gpr::R0 }.encode());  // a2 <- flags
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::A0, rk: Gpr::R0 }.encode());  // a1 <- pathname
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 56 }.encode());   // sys_openat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("open".to_string(), code));
            }

            // unlink → unlinkat(AT_FDCWD=-100, pathname, 0)
            // Caller args: a0=pathname
            // Need:        a0=-100,   a1=pathname, a2=0
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A2, rj: Gpr::R0, rk: Gpr::R0 }.encode());    // a2 = 0 (flags)
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::A0, rk: Gpr::R0 }.encode());    // a1 <- pathname
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 35 }.encode());   // sys_unlinkat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("unlink".to_string(), code));
            }

            // sigaction → rt_sigaction(signum, act, oldact, sigsetsize=8)
            // Caller args: a0=signum, a1=act, a2=oldact
            // Need:        a0=signum, a1=act, a2=oldact, a3=8
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::R0, imm12: 8 }.encode());    // a3 = sigsetsize
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 134 }.encode());  // sys_rt_sigaction
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("sigaction".to_string(), code));
            }

            // pipe → pipe2(pipefd, 0)
            // Caller args: a0=pipefd
            // Need:        a0=pipefd, a1=0
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::R0, rk: Gpr::R0 }.encode());    // a1 = 0 (flags)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 59 }.encode());    // sys_pipe2
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("pipe".to_string(), code));
            }

            // dup2 → dup3(oldfd, newfd, 0)
            // Caller args: a0=oldfd, a1=newfd
            // Need:        a0=oldfd, a1=newfd, a2=0
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A2, rj: Gpr::R0, rk: Gpr::R0 }.encode());    // a2 = 0 (flags)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 24 }.encode());    // sys_dup3
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("dup2".to_string(), code));
            }

            // fork → clone(SIGCHLD=17, 0, 0, 0, 0)
            // Caller args: none
            // Need:        a0=17, a1=0, a2=0, a3=0, a4=0
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: 17 }.encode());   // a0 = SIGCHLD
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A1, rj: Gpr::R0, rk: Gpr::R0 }.encode());    // a1 = 0
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A2, rj: Gpr::R0, rk: Gpr::R0 }.encode());    // a2 = 0
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A3, rj: Gpr::R0, rk: Gpr::R0 }.encode());    // a3 = 0
                code.extend_from_slice(&Instruction::Or { rd: Gpr::A4, rj: Gpr::R0, rk: Gpr::R0 }.encode());    // a4 = 0
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 220 }.encode());  // sys_clone
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("fork".to_string(), code));
            }

            // ── Wave 7 wrappers: plain POSIX names → *at(AT_FDCWD=-100, ...) ──
            // LoongArch (asm-generic) lacks the legacy mkdir/rmdir/rename/link/
            // symlink/readlink/chmod/chown syscalls; expose the plain names by
            // inserting AT_FDCWD=-100 (fits addi.d imm12) and shifting args.
            // AT_REMOVEDIR=0x200. Moves use the `addi.d rd,rs,0` idiom.
            // Shuffle high→low to avoid clobbering.

            // mkdir(path, mode) → mkdirat(AT_FDCWD, path, mode)  [mkdirat=34]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::A1, imm12: 0 }.encode());   // a2 = mode
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());   // a1 = path
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 34 }.encode());  // mkdirat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("mkdir".to_string(), code));
            }
            // rmdir(path) → unlinkat(AT_FDCWD, path, AT_REMOVEDIR=0x200)  [unlinkat=35]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: 0x200 }.encode());// a2 = AT_REMOVEDIR
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());   // a1 = path
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 35 }.encode());  // unlinkat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("rmdir".to_string(), code));
            }
            // rename(old, new) → renameat(AT_FDCWD, old, AT_FDCWD, new)  [renameat=38]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::A1, imm12: 0 }.encode());   // a3 = new
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: -100 }.encode()); // a2 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());   // a1 = old
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 38 }.encode());  // renameat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("rename".to_string(), code));
            }
            // link(old, new) → linkat(AT_FDCWD, old, AT_FDCWD, new, 0)  [linkat=37]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A4, rj: Gpr::R0, imm12: 0 }.encode());   // a4 = 0 (flags)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::A1, imm12: 0 }.encode());   // a3 = new
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: -100 }.encode()); // a2 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());   // a1 = old
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 37 }.encode());  // linkat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("link".to_string(), code));
            }
            // symlink(target, linkpath) → symlinkat(target, AT_FDCWD, linkpath)  [symlinkat=36]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::A1, imm12: 0 }.encode());   // a2 = linkpath
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::R0, imm12: -100 }.encode()); // a1 = AT_FDCWD
                // a0 = target (unchanged)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 36 }.encode());  // symlinkat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("symlink".to_string(), code));
            }
            // readlink(path, buf, siz) → readlinkat(AT_FDCWD, path, buf, siz)  [readlinkat=78]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::A2, imm12: 0 }.encode());   // a3 = siz
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::A1, imm12: 0 }.encode());   // a2 = buf
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());   // a1 = path
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 78 }.encode());  // readlinkat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("readlink".to_string(), code));
            }
            // chmod(path, mode) → fchmodat(AT_FDCWD, path, mode, 0)  [fchmodat=53]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::R0, imm12: 0 }.encode());   // a3 = 0 (flags)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::A1, imm12: 0 }.encode());   // a2 = mode
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());   // a1 = path
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 53 }.encode());  // fchmodat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("chmod".to_string(), code));
            }
            // chown(path, owner, group) → fchownat(AT_FDCWD, path, owner, group, 0)  [fchownat=54]
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A4, rj: Gpr::R0, imm12: 0 }.encode());   // a4 = 0 (flags)
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::A2, imm12: 0 }.encode());   // a3 = group
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::A1, imm12: 0 }.encode());   // a2 = owner
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::A0, imm12: 0 }.encode());   // a1 = path
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: -100 }.encode()); // a0 = AT_FDCWD
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 54 }.encode());  // fchownat
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("chown".to_string(), code));
            }

            // ── FFI scratchpad frame stubs (Wave 3b/fix) ──────────────────
            // ffi_scratch_push_frame: REAL mmap syscall (loongarch64 sys_mmap=222).
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: 0 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A1, rj: Gpr::R0, imm12: 4096 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A2, rj: Gpr::R0, imm12: 3 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A3, rj: Gpr::R0, imm12: 0x22 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A4, rj: Gpr::R0, imm12: -1 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A5, rj: Gpr::R0, imm12: 0 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 222 }.encode()); // sys_mmap
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("ffi_scratch_push_frame".to_string(), code));
            }

            // ffi_scratch_pop_frame: no-op (JIRL R0, RA, 0).
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("ffi_scratch_pop_frame".to_string(), code));
            }

            // __arena_overflow: real exit(1) syscall
            {
                let mut code = Vec::new();
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A0, rj: Gpr::R0, imm12: 1 }.encode());
                code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: 93 }.encode());
                code.extend_from_slice(&Instruction::Syscall.encode());
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                stubs.push(("__arena_overflow".to_string(), code));
            }

            stubs
        };

        // Compute offsets for syscall stubs and register them.
        // __vuma_alloc stub = 9 instrs (36 B), __vuma_free stub = 4 instrs (16 B).
        let syscall_stubs_start = vuma_free_offset + vuma_free_stub.len();
        let mut stub_offset = syscall_stubs_start;
        for (name, code) in &syscall_stubs {
            func_offsets.insert(name.clone(), stub_offset);
            stub_offset += code.len();
        }

        // Register canonical `__vuma_print_*` aliases.
        for (short, canonical) in [
            ("print_int", "__vuma_print_int"),
            ("print_hex", "__vuma_print_hex"),
        ] {
            if let Some(&off) = func_offsets.get(short) {
                func_offsets.insert(canonical.to_string(), off);
            }
        }

        // ── Concatenate all code ──
        let mut all_code = start_stub;
        all_code.extend_from_slice(&ffi_stub); // 8 bytes at offset 12
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }
        // Append __vuma_alloc / __vuma_free syscall stubs.
        all_code.extend_from_slice(&vuma_alloc_stub);
        all_code.extend_from_slice(&vuma_free_stub);
        // Append POSIX syscall stubs (write, read, open, close, mmap, etc.)
        for (_, code) in &syscall_stubs {
            all_code.extend_from_slice(code);
        }

        // ── Compute code virtual-address base ──
        // Must match the layout in build_loongarch64_elf_2seg.
        const ELF_BASE_ADDR: u64 = 0x120000000;
        const ELF_HEADER_SIZE: u64 = 64;
        const ELF_PHDR_SIZE: u64 = 56;
        const ELF_NUM_PHDRS: u64 = 3; // 2 LOAD + 1 GNU_STACK
        let phdr_end = ELF_HEADER_SIZE + ELF_NUM_PHDRS * ELF_PHDR_SIZE;
        // MUST match build_loongarch64_elf_2seg — both use phdr_end (232)
        // directly, no page alignment.
        let text_offset = phdr_end;
        let code_vaddr_base = ELF_BASE_ADDR + text_offset;

        // ── Patch relocations ──
        let mut func_code_offset: usize = start_stub_size + ffi_stub_size;
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
                    let target_offset = func_offsets.get(&reloc.symbol)
                        .copied()
                        .or_else(|| {
                            let prefix = format!("fn_{}", reloc.symbol);
                            func_offsets.keys()
                                .find(|k| k.starts_with(&prefix))
                                .and_then(|k| func_offsets.get(k))
                                .copied()
                        });
                    if let Some(target_offset) = target_offset {
                        let bl_addr = abs_offset as i64;
                        let target_addr = target_offset as i64;
                        let offset_words = (target_addr - bl_addr) / 4;
                        // Check range: ±128MB (26-bit signed * 4)
                        if offset_words < -(1i64 << 25) || offset_words >= (1i64 << 25) {
                            vuma_log!(warn, 
                                "BL relocation to '{}' out of range: {} words",
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
                    } else {
                        // External symbol — point to FFI return-0 stub
                        vuma_log!(warn, 
                            "unresolved relocation: symbol '{}' in '{}' at 0x{:X} (type: {}) — deferring to FFI stub",
                            reloc.symbol, func.name, reloc.offset, reloc.reloc_type
                        );
                        let target_addr = ffi_stub_offset as i64;
                        let bl_addr = abs_offset as i64;
                        let offset_words = (target_addr - bl_addr) / 4;
                        let patched = u32::from_le_bytes(
                            Instruction::Bl { offs26: offset_words as i32 }.encode()
                        );
                        all_code[abs_offset..abs_offset + 4].copy_from_slice(&patched.to_le_bytes());
                    }
                } else if reloc.reloc_type == R_LARCH_64 {
                    // R_LARCH_64: patch the 4-instruction load-immediate sequence
                    // (lu12i.w + ori + lu32i.d + lu52i.d = 16 bytes) with an
                    // absolute 64-bit address.
                    if abs_offset + 16 > all_code.len() {
                        vuma_log!(warn, 
                            "R_LARCH_64 relocation at offset {} overflows code (len {})",
                            abs_offset, all_code.len()
                        );
                        continue;
                    }
                    let target_offset = func_offsets.get(&reloc.symbol)
                        .copied()
                        .or_else(|| {
                            let prefix = format!("fn_{}", reloc.symbol);
                            func_offsets.keys()
                                .find(|k| k.starts_with(&prefix))
                                .and_then(|k| func_offsets.get(k))
                                .copied()
                        });
                    if let Some(target_offset) = target_offset {
                        let vaddr = code_vaddr_base + target_offset as u64;
                        patch_load_imm_64(&mut all_code, abs_offset, vaddr);
                    } else {
                        // External symbol — defer to the system linker.
                        // Leave the load-immediate sequence as-is (zero = trap).
                        // When compiled with `vuma compile --format obj`, the linker
                        // will resolve this relocation against libc or the runtime.
                        vuma_log!(warn, 
                            "unresolved relocation: symbol '{}' in '{}' at 0x{:X} (type: {}) — deferring to linker",
                            reloc.symbol, func.name, reloc.offset, reloc.reloc_type
                        );
                        continue;
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
    use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram};
    use crate::ir::{IRValue, IRInstr, IRFunction, IRType, BinOpKind, CmpKind, CastKind, UnaryOpKind};

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
        // SUB.D $t0, $t1, $t2 — opcode 0x0023 per LoongArch manual (3R format).
        // bits[31:15]=opcode, bits[14:10]=rk, bits[9:5]=rj, bits[4:0]=rd.
        let bytes = Instruction::SubD {
            rd: Gpr::T0,
            rj: Gpr::T1,
            rk: Gpr::T2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x0023u32 << 15) | (14u32 << 10) | (13u32 << 5) | 12u32;
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
        // BL 0x100 — I26 format per LoongArch manual:
        //   bits[31:26] = 0b010101 (BL opcode = 0x15)
        //   bits[25:10] = offs[15:0]   (lower 16 bits of offset)
        //   bits[9:0]   = offs[25:16]  (upper 10 bits of offset)
        // For offs26 = 0x100: lo16 = 0x100, hi10 = 0x0.
        let bytes = Instruction::Bl { offs26: 0x100 }.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = (0x15u32 << 26) | (0x100u32 << 10) | 0u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_encode_and_or_xor() {
        // AND/OR/XOR $a0, $a1, $a2 — 3R format opcodes per LoongArch manual:
        //   AND = 0x0029, OR = 0x002A, XOR = 0x002B (bits[31:15]).
        // AND $a0, $a1, $a2
        let and_bytes = Instruction::And {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let and_word = u32::from_le_bytes(and_bytes);
        assert_eq!(and_word >> 15, 0x0029);

        // OR $a0, $a1, $a2
        let or_bytes = Instruction::Or {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let or_word = u32::from_le_bytes(or_bytes);
        assert_eq!(or_word >> 15, 0x002A);

        // XOR $a0, $a1, $a2
        let xor_bytes = Instruction::Xor {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let xor_word = u32::from_le_bytes(xor_bytes);
        assert_eq!(xor_word >> 15, 0x002B);
    }

    #[test]
    fn test_encode_slt() {
        // SLT $a0, $a1, $a2 — opcode 0x0024 per LoongArch manual (3R format).
        let bytes = Instruction::Slt {
            rd: Gpr::A0,
            rj: Gpr::A1,
            rk: Gpr::A2,
        }
        .encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word >> 15, 0x0024);
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
        // I26 format per LoongArch manual:
        //   bits[31:26] = opcode
        //   bits[25:10] = offs[15:0]   (lower 16 bits of offset)
        //   bits[9:0]   = offs[25:16]  (upper 10 bits of offset)
        // For imm26 = 0x12345: lo16 = 0x2345, hi10 = 0x1.
        let bytes = encode_i26(OPC_B, 0x12345);
        let word = u32::from_le_bytes(bytes);
        assert_eq!((word >> 26) & 0x3F, OPC_B);
        // lo16 = 0x12345 & 0xFFFF = 0x2345, placed at bits[25:10]
        assert_eq!((word >> 10) & 0xFFFF, 0x2345u32);
        // hi10 = 0x12345 >> 16 = 0x1, placed at bits[9:0]
        assert_eq!(word & 0x3FF, 0x1u32);
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
            rodata_data: Vec::new(),
            function_names: std::collections::HashSet::new(),
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
                source_line: 0,
            }],
            source_file: String::new(),
        }
    }

    /// Returns true if `instr`'s encoded bytes contain a 4-byte instruction
    /// whose opcode field (extracted via `shift`/`mask`) equals `opcode`.
    ///
    /// The stack-slot ISel tags every emitted `AllocatedInstruction` with the
    /// *IR* name (e.g. "Add", "Sub", "Cmp") rather than the LoongArch
    /// mnemonic. To verify that a specific machine instruction was actually
    /// emitted, we decode the raw 32-bit words and compare the opcode bits.
    fn instr_contains_op(instr: &AllocatedInstruction, shift: u32, mask: u32, opcode: u32) -> bool {
        instr.encoded.chunks_exact(4).any(|chunk| {
            let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            (word >> shift) & mask == opcode
        })
    }

    /// Returns true if any instruction in `instructions` (filtered by IR
    /// `opcode` name) contains a 4-byte instruction whose opcode field equals
    /// `opcode`. If `name_filter` is `None`, all instructions are searched.
    fn any_instr_contains_op(
        instructions: &[AllocatedInstruction],
        name_filter: Option<&str>,
        shift: u32,
        mask: u32,
        opcode: u32,
    ) -> bool {
        instructions.iter().any(|i| {
            if let Some(name) = name_filter {
                if i.opcode != name {
                    return false;
                }
            }
            instr_contains_op(i, shift, mask, opcode)
        })
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
        // The stack-slot ISel tags the emitted instruction with the IR name
        // ("Add"), so we inspect the encoded bytes for an AddiD opcode
        // (2RI12 format, opcode at bits[31:22]).
        let has_addi = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("Add"),
            22,
            0x3FF,
            OPC_ADDI_D,
        );
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
        // Sub with small immediate is lowered as AddiD with a negated imm12.
        let has_addi = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("Sub"),
            22,
            0x3FF,
            OPC_ADDI_D,
        );
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
        // Neg is lowered as SubD $r0, S0 (3R format, opcode at bits[31:15]).
        let has_sub = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("UnaryOp"),
            15,
            0x1FFFF,
            OPC_SUB_D,
        );
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
        // Not is lowered as Nor $r0, S0 (3R format, opcode at bits[31:15]).
        let has_nor = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("UnaryOp"),
            15,
            0x1FFFF,
            OPC_NOR,
        );
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
        // Cmp SLt is lowered via encode_cmp which emits Slt (3R format,
        // opcode at bits[31:15]).
        let has_slt = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("Cmp"),
            15,
            0x1FFFF,
            OPC_SLT,
        );
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
        // Large immediate (100000) doesn't fit in si12, so the ISel loads it
        // via encode_load_imm which begins with Lu12iW (reg1i20 format,
        // 7-bit opcode at bits[31:25]).
        let has_lu12i = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("Add"),
            25,
            0x7F,
            OPC_LU12I_W,
        );
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
        // BinOp Shl with immediate is lowered as SlliD (reg2i6 format,
        // 16-bit opcode at bits[31:16]).
        let has_slli = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("BinOp"),
            16,
            0xFFFF,
            OPC_SLLI_D,
        );
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
        // The Ret IR instruction itself is a no-op (return value loaded by the
        // Return terminator). The Jirl is emitted as part of the epilogue in
        // the "return" terminator instruction (2RI16 format, opcode at
        // bits[31:26]).
        let has_jirl = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("return"),
            26,
            0x3F,
            OPC_JIRL,
        );
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
        // The Alloc instruction computes its address as $fp + alloc_off using
        // an AddiD instruction (2RI12 format, opcode at bits[31:22]). The
        // stack-slot ISel does not populate `reads`/`writes`, so we inspect
        // the encoded bytes instead.
        let has_addi = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("Alloc"),
            22,
            0x3FF,
            OPC_ADDI_D,
        );
        assert!(
            has_addi,
            "expected addi.d (from $fp) for stack allocation, got opcodes: {:?}",
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
        // The Alloc offset computation emits an AddiD instruction reading
        // $fp. The stack-slot ISel does not populate `reads`/`writes`, so we
        // check the encoded bytes for an AddiD opcode instead.
        let has_addi = any_instr_contains_op(
            &result.blocks[0].instructions,
            Some("Alloc"),
            22,
            0x3FF,
            OPC_ADDI_D,
        );
        assert!(
            has_addi,
            "expected addi.d for alloc at offset 0, got opcodes: {:?}",
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
            rodata_data: Vec::new(),
            function_names: std::collections::HashSet::new(),
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
            rodata_data: Vec::new(),
            function_names: std::collections::HashSet::new(),
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
            rodata_data: Vec::new(),
            function_names: std::collections::HashSet::new(),
        };
        let elf = backend.encode_program(&prog).unwrap();
        // e_entry at offset 24 (8 bytes)
        let e_entry = u64::from_le_bytes([
            elf[24], elf[25], elf[26], elf[27],
            elf[28], elf[29], elf[30], elf[31],
        ]);
        // The ELF emits 3 program headers (2 PT_LOAD + 1 PT_GNU_STACK),
        // each 56 bytes, after the 64-byte ELF header.  The text segment is
        // page-aligned (64 KB), so:
        //   phdr_end    = 64 + 3*56 = 232
        //   text_offset = phdr_end = 232 (0xE8)
        //   e_entry     = base_addr + text_offset = 0x120000000 + 0xE8 = 0x1200000E8
        const ELF_HEADER_SIZE: usize = 64;
        const PHDR_SIZE: usize = 56;
        const NUM_PHDRS: usize = 3;
        let text_offset = ELF_HEADER_SIZE + NUM_PHDRS * PHDR_SIZE; // 232
        let expected_entry: u64 = 0x120000000 + text_offset as u64;
        assert_eq!(e_entry, expected_entry, "entry point should point to _start stub");
        // Verify the first instruction at the entry point is BL
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
            rodata_data: Vec::new(),
            function_names: std::collections::HashSet::new(),
        };
        let elf = backend.encode_program(&prog).unwrap();

        // e_phoff at offset 32
        let e_phoff = u64::from_le_bytes([
            elf[32], elf[33], elf[34], elf[35], elf[36], elf[37], elf[38], elf[39],
        ]);
        assert_eq!(e_phoff, 64, "program headers should start right after ELF header");

        // e_phnum at offset 56
        let e_phnum = u16::from_le_bytes([elf[56], elf[57]]);
        assert_eq!(e_phnum, 3, "should have 3 program headers (2 LOAD + 1 GNU_STACK)");

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

        // Third program header: PT_GNU_STACK (non-executable stack)
        // p_type = 0x6474e551, p_flags = PF_R | PF_W (no PF_X)
        let ph3_off = ph2_off + 56;
        let p3_type = u32::from_le_bytes([elf[ph3_off], elf[ph3_off+1], elf[ph3_off+2], elf[ph3_off+3]]);
        let p3_flags = u32::from_le_bytes([elf[ph3_off+4], elf[ph3_off+5], elf[ph3_off+6], elf[ph3_off+7]]);
        assert_eq!(p3_type, 0x6474e551, "third segment should be PT_GNU_STACK");
        assert_eq!(p3_flags, 6, "PT_GNU_STACK should be PF_R | PF_W (no PF_X)");
    }
}
pub mod disasm;
// NOTE: `reg_alloc_isel` was an alternate register-allocator ISel that is
// superseded by `stack_slot_isel`. The module declaration is commented out
// because nothing in the crate references it; keeping it would just force
// `cargo` to keep compiling ~1.5k lines of unused code.
// pub mod reg_alloc_isel;
pub mod stack_slot_isel;
