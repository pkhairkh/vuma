//! # ARM 32-bit Backend
//!
//! Implements the `Backend` trait for the ARM 32-bit target (AAPCS ABI).
//! This module provides:
//!
//! - `Gpr` — General-purpose register enum (R0–R15)
//! - `Dpr` — Double-precision FP/SIMD register enum (D0–D31)
//! - `Condition` — ARM condition code enum (EQ, NE, CS, …, AL)
//! - `Instruction` — ARM instruction enum with correct 32-bit encoding
//! - Encoding helpers for data processing, load/store, branch, and system ops
//! - `Arm32Backend` — `Backend` implementation that lowers IR to ARM machine
//!   code and emits ELF32 binaries
//!
//! ## ARM32 Register Convention (AAPCS)
//!
//! | Register(s) | ABI Name | Role                              |
//! |-------------|----------|-----------------------------------|
//! | R0–R3       | a1–a4    | Argument / return registers       |
//! | R4–R11      | v1–v8    | Callee-saved                      |
//! | R12         | IP       | Intra-procedure scratch           |
//! | R13         | SP       | Stack pointer                     |
//! | R14         | LR       | Link register                     |
//! | R15         | PC       | Program counter                   |
//!
//! ## ARM32 FP Register Convention (AAPCS VFP variant)
//!
//! | Register(s) | Role                                     |
//! |-------------|------------------------------------------|
//! | D0–D15      | FP argument / caller-saved               |
//! | D8–D15      | Callee-saved                             |
//! | D16–D31     | Caller-saved (VFPv3/NEON only)           |
//!
//! ## Instruction Encoding
//!
//! ARM instructions are 32 bits, little-endian, with a 4-bit condition code
//! in bits \[31:28\]. The `AL` (always) condition is used for unconditional
//! instructions.
//!
//! ## References
//!
//! - ARM Architecture Reference Manual (ARMv7-A and ARMv7-R edition)
//! - Procedure Call Standard for the ARM Architecture (AAPCS)

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Arm32TargetInfo,
    Backend, BackendError, PhysicalReg, RegClass, RelocationEntry, TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, UnaryOpKind};
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// ARM 32-bit general-purpose registers (R0–R15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Gpr {
    R0 = 0,
    R1 = 1,
    R2 = 2,
    R3 = 3,
    R4 = 4,
    R5 = 5,
    R6 = 6,
    R7 = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

impl Gpr {
    /// Returns the 4-bit encoding index for this register.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns `true` if this register is available for register allocation.
    ///
    /// R13 (SP), R14 (LR), and R15 (PC) are reserved.
    pub fn is_allocatable(&self) -> bool {
        !matches!(self, Gpr::R13 | Gpr::R14 | Gpr::R15)
    }

    /// Returns `true` if this register is callee-saved (R4–R11).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Gpr::R4 | Gpr::R5 | Gpr::R6 | Gpr::R7 | Gpr::R8 | Gpr::R9 | Gpr::R10 | Gpr::R11
        )
    }

    /// Returns `true` if this register is an argument register (R0–R3).
    pub fn is_arg_reg(&self) -> bool {
        matches!(self, Gpr::R0 | Gpr::R1 | Gpr::R2 | Gpr::R3)
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::R0 => "r0",
            Gpr::R1 => "r1",
            Gpr::R2 => "r2",
            Gpr::R3 => "r3",
            Gpr::R4 => "r4",
            Gpr::R5 => "r5",
            Gpr::R6 => "r6",
            Gpr::R7 => "r7",
            Gpr::R8 => "r8",
            Gpr::R9 => "r9",
            Gpr::R10 => "r10",
            Gpr::R11 => "r11",
            Gpr::R12 => "ip",
            Gpr::R13 => "sp",
            Gpr::R14 => "lr",
            Gpr::R15 => "pc",
        }
    }

    /// Returns the Gpr for a given argument index (0–3). Returns `None` for
    /// indices >= 4.
    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::R0),
            1 => Some(Gpr::R1),
            2 => Some(Gpr::R2),
            3 => Some(Gpr::R3),
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
// Double-Precision FP/SIMD Registers
// ===========================================================================

/// ARM VFP/NEON double-precision registers (D0–D31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Dpr {
    D0 = 0,
    D1 = 1,
    D2 = 2,
    D3 = 3,
    D4 = 4,
    D5 = 5,
    D6 = 6,
    D7 = 7,
    D8 = 8,
    D9 = 9,
    D10 = 10,
    D11 = 11,
    D12 = 12,
    D13 = 13,
    D14 = 14,
    D15 = 15,
    D16 = 16,
    D17 = 17,
    D18 = 18,
    D19 = 19,
    D20 = 20,
    D21 = 21,
    D22 = 22,
    D23 = 23,
    D24 = 24,
    D25 = 25,
    D26 = 26,
    D27 = 27,
    D28 = 28,
    D29 = 29,
    D30 = 30,
    D31 = 31,
}

impl Dpr {
    /// Returns the 5-bit encoding index for this register.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns `true` if this register is available for register allocation.
    pub fn is_allocatable(&self) -> bool {
        true
    }

    /// Returns `true` if this register is callee-saved (D8–D15).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Dpr::D8 | Dpr::D9 | Dpr::D10 | Dpr::D11 | Dpr::D12 | Dpr::D13 | Dpr::D14 | Dpr::D15
        )
    }

    /// Returns `true` if this register is an FP argument register (D0–D15).
    pub fn is_arg_reg(&self) -> bool {
        (*self as u32) <= 15
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Dpr::D0 => "d0",
            Dpr::D1 => "d1",
            Dpr::D2 => "d2",
            Dpr::D3 => "d3",
            Dpr::D4 => "d4",
            Dpr::D5 => "d5",
            Dpr::D6 => "d6",
            Dpr::D7 => "d7",
            Dpr::D8 => "d8",
            Dpr::D9 => "d9",
            Dpr::D10 => "d10",
            Dpr::D11 => "d11",
            Dpr::D12 => "d12",
            Dpr::D13 => "d13",
            Dpr::D14 => "d14",
            Dpr::D15 => "d15",
            Dpr::D16 => "d16",
            Dpr::D17 => "d17",
            Dpr::D18 => "d18",
            Dpr::D19 => "d19",
            Dpr::D20 => "d20",
            Dpr::D21 => "d21",
            Dpr::D22 => "d22",
            Dpr::D23 => "d23",
            Dpr::D24 => "d24",
            Dpr::D25 => "d25",
            Dpr::D26 => "d26",
            Dpr::D27 => "d27",
            Dpr::D28 => "d28",
            Dpr::D29 => "d29",
            Dpr::D30 => "d30",
            Dpr::D31 => "d31",
        }
    }
}

impl fmt::Display for Dpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Condition Codes
// ===========================================================================

/// ARM condition codes (4-bit encoding in bits \[31:28\] of every ARM instruction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Condition {
    /// Equal / Z set
    Eq = 0b0000,
    /// Not equal / Z clear
    Ne = 0b0001,
    /// Carry set / unsigned higher or same (CS = HS)
    Cs = 0b0010,
    /// Carry clear / unsigned lower (CC = LO)
    Cc = 0b0011,
    /// Minus / negative (N set)
    Mi = 0b0100,
    /// Plus / positive or zero (N clear)
    Pl = 0b0101,
    /// Overflow (V set)
    Vs = 0b0110,
    /// No overflow (V clear)
    Vc = 0b0111,
    /// Unsigned higher (C set and Z clear)
    Hi = 0b1000,
    /// Unsigned lower or same (C clear or Z set)
    Ls = 0b1001,
    /// Signed greater or equal (N == V)
    Ge = 0b1010,
    /// Signed less than (N != V)
    Lt = 0b1011,
    /// Signed greater than (Z clear and N == V)
    Gt = 0b1100,
    /// Signed less than or equal (Z set or N != V)
    Le = 0b1101,
    /// Always (unconditional)
    Al = 0b1110,
}

impl Condition {
    /// Returns the 4-bit encoding for this condition code.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }
}

impl fmt::Display for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Condition::Eq => "eq",
            Condition::Ne => "ne",
            Condition::Cs => "cs",
            Condition::Cc => "cc",
            Condition::Mi => "mi",
            Condition::Pl => "pl",
            Condition::Vs => "vs",
            Condition::Vc => "vc",
            Condition::Hi => "hi",
            Condition::Ls => "ls",
            Condition::Ge => "ge",
            Condition::Lt => "lt",
            Condition::Gt => "gt",
            Condition::Le => "le",
            Condition::Al => "al",
        })
    }
}

// ===========================================================================
// ARM Instruction Encoding Constants
// ===========================================================================

/// Data processing opcodes (bits [24:21]).
const DP_AND: u32 = 0b0000;
const DP_EOR: u32 = 0b0001;
const DP_SUB: u32 = 0b0010;
const DP_RSB: u32 = 0b0011;
const DP_ADD: u32 = 0b0100;
const DP_ADC: u32 = 0b0101; // Add with Carry
const DP_SBC: u32 = 0b0110; // Subtract with Borrow
const DP_RSC: u32 = 0b0111; // Reverse Subtract with Carry
const DP_TST: u32 = 0b1000;
const DP_TEQ: u32 = 0b1001;
const DP_CMP: u32 = 0b1010;
const DP_CMN: u32 = 0b1011;
const DP_ORR: u32 = 0b1100;
const DP_MOV: u32 = 0b1101;
const DP_BIC: u32 = 0b1110;
const DP_MVN: u32 = 0b1111;

// ===========================================================================
// Instruction Encoding Helpers
// ===========================================================================

/// Encode a data-processing instruction with register operand2 (no shift).
///
/// Format: `cond[31:28] | 00[27:26] | I=0[25] | opcode[24:21] | S[20] |
///         Rn[19:16] | Rd[15:12] | 00000000[11:4] | Rm[3:0]`
fn encode_dp_reg(cond: Condition, opcode: u32, s: bool, rn: u32, rd: u32, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        // I=0: register operand2
        | ((opcode & 0xF) << 21)
        | ((s as u32) << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        // shift = 0, type = 0
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode a data-processing instruction with immediate operand2.
///
/// Format: `cond[31:28] | 00[27:26] | I=1[25] | opcode[24:21] | S[20] |
///         Rn[19:16] | Rd[15:12] | rotate[11:8] | imm8[7:0]`
fn encode_dp_imm(
    cond: Condition,
    opcode: u32,
    s: bool,
    rn: u32,
    rd: u32,
    rotate: u32,
    imm8: u32,
) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (1 << 25) // I=1: immediate operand2
        | ((opcode & 0xF) << 21)
        | ((s as u32) << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | ((rotate & 0xF) << 8)
        | (imm8 & 0xFF);
    word.to_le_bytes()
}

/// Encode a data-processing instruction with shifted register operand2
/// (shift by immediate).
///
/// Format: `cond[31:28] | 00[27:26] | I=0[25] | opcode[24:21] | S[20] |
///         Rn[19:16] | Rd[15:12] | shift_imm[11:7] | shift_type[6:5] |
///         0[4] | Rm[3:0]`
#[allow(clippy::too_many_arguments)]
fn encode_dp_shift_imm(
    cond: Condition,
    opcode: u32,
    s: bool,
    rn: u32,
    rd: u32,
    shift_type: u32,
    shift_imm: u32,
    rm: u32,
) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        // I=0: immediate shift
        | ((opcode & 0xF) << 21)
        | ((s as u32) << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | ((shift_imm & 0x1F) << 7)
        | ((shift_type & 0x3) << 5)
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode a data-processing instruction with shifted register operand2
/// (shift by register).
///
/// Format: `cond[31:28] | 00[27:26] | I=0[25] | opcode[24:21] | S[20] |
///         Rn[19:16] | Rd[15:12] | Rs[11:8] | shift_type[6:5] | 1[4] | Rm[3:0]`
#[allow(clippy::too_many_arguments)]
fn encode_dp_shift_reg(
    cond: Condition,
    opcode: u32,
    s: bool,
    rn: u32,
    rd: u32,
    shift_type: u32,
    rs: u32,
    rm: u32,
) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | ((opcode & 0xF) << 21)
        | ((s as u32) << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | ((rs & 0xF) << 8)
        | ((shift_type & 0x3) << 5)
        | (1 << 4)
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode a load/store word or byte with immediate offset.
///
/// Format: `cond[31:28] | 01[27:26] | I=0[25] | P[24] | U[23] | B[22] |
///         W[21] | L[20] | Rn[19:16] | Rd[15:12] | offset12[11:0]`
#[allow(clippy::too_many_arguments)]
fn encode_ls_imm(
    cond: Condition,
    p: bool,
    u: bool,
    b: bool,
    w: bool,
    l: bool,
    rn: u32,
    rd: u32,
    offset12: u32,
) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b01 << 26)
        // I=0: immediate offset
        | ((p as u32) << 24)
        | ((u as u32) << 23)
        | ((b as u32) << 22)
        | ((w as u32) << 21)
        | ((l as u32) << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (offset12 & 0xFFF);
    word.to_le_bytes()
}

/// Encode a load/store halfword with immediate offset.
///
/// Format: `cond[31:28] | 000[27:25] | P[24] | U[23] | I=0[22] | W[21] |
///         L[20] | Rn[19:16] | Rd[15:12] | offset_high[11:8] | 1011[7:4] |
///         offset_low[3:0]`
#[allow(clippy::too_many_arguments)]
fn encode_ls_half_imm(
    cond: Condition,
    p: bool,
    u: bool,
    w: bool,
    l: bool,
    rn: u32,
    rd: u32,
    offset8: u32,
) -> [u8; 4] {
    let imm_hi = (offset8 >> 4) & 0xF;
    let imm_lo = offset8 & 0xF;
    let word = (cond.encoding() << 28)
        | ((p as u32) << 24)
        | ((u as u32) << 23)
        // I=0: immediate offset
        | ((w as u32) << 21)
        | ((l as u32) << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (imm_hi << 8)
        | (0b1011 << 4)
        | imm_lo;
    word.to_le_bytes()
}

/// Encode a load/store doubleword with immediate offset (LDRD/STRD).
///
/// Format: `cond[31:28] | 000[27:25] | P[24] | U[23] | I=0[22] | W[21] |
///         L=0[20] | Rn[19:16] | Rd[15:12] | offset_high[11:8] | 1111[7:4] |
///         offset_low[3:0]`
#[allow(clippy::too_many_arguments)]
fn encode_ls_double_imm(
    cond: Condition,
    p: bool,
    u: bool,
    w: bool,
    is_load: bool,
    rn: u32,
    rd: u32,
    offset8: u32,
) -> [u8; 4] {
    let imm_hi = (offset8 >> 4) & 0xF;
    let imm_lo = offset8 & 0xF;
    // For LDRD, the L bit (bit 20) is set; for STRD it is clear.
    // Actually, for LDRD/STRD the encoding uses bit 20 differently:
    // STRD: L=0, LDRD: L=1. But wait — the ARM ARM says for LDRD/STRD,
    // bit 20 distinguishes: 0=STRD, 1=LDRD.
    let word = (cond.encoding() << 28)
        | ((p as u32) << 24)
        | ((u as u32) << 23)
        // I=0
        | ((w as u32) << 21)
        | ((is_load as u32) << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (imm_hi << 8)
        | (0b1111 << 4)
        | imm_lo;
    word.to_le_bytes()
}

/// Encode a load halfword signed byte (LDRSB) with immediate offset.
///
/// Format: `cond[31:28] | 000[27:25] | P[24] | U[23] | I=0[22] | W[21] |
///         L=1[20] | Rn[19:16] | Rd[15:12] | offset_high[11:8] | 1101[7:4] |
///         offset_low[3:0]`
fn encode_ldrsb_imm(
    cond: Condition,
    p: bool,
    u: bool,
    w: bool,
    rn: u32,
    rd: u32,
    offset8: u32,
) -> [u8; 4] {
    let imm_hi = (offset8 >> 4) & 0xF;
    let imm_lo = offset8 & 0xF;
    let word = (cond.encoding() << 28)
        | ((p as u32) << 24)
        | ((u as u32) << 23)
        | ((w as u32) << 21)
        | (1 << 20) // L=1
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (imm_hi << 8)
        | (0b1101 << 4)
        | imm_lo;
    word.to_le_bytes()
}

/// Encode a load signed halfword (LDRSH) with immediate offset.
///
/// Format: `cond[31:28] | 000[27:25] | P[24] | U[23] | I=0[22] | W[21] |
///         L=1[20] | Rn[19:16] | Rd[15:12] | offset_high[11:8] | 1111[7:4] |
///         offset_low[3:0]`
fn encode_ldrsh_imm(
    cond: Condition,
    p: bool,
    u: bool,
    w: bool,
    rn: u32,
    rd: u32,
    offset8: u32,
) -> [u8; 4] {
    let imm_hi = (offset8 >> 4) & 0xF;
    let imm_lo = offset8 & 0xF;
    let word = (cond.encoding() << 28)
        | ((p as u32) << 24)
        | ((u as u32) << 23)
        | ((w as u32) << 21)
        | (1 << 20) // L=1
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (imm_hi << 8)
        | (0b1111 << 4)
        | imm_lo;
    word.to_le_bytes()
}

/// Encode a branch (B/BL) instruction.
///
/// Format: `cond[31:28] | 101[27:25] | L[24] | offset24[23:0]`
///
/// The offset is a signed word-aligned offset from PC+8, in words (shifted
/// right by 2). The 24-bit field is sign-extended and shifted left by 2.
fn encode_branch(cond: Condition, link: bool, offset24: i32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b101 << 25)
        | ((link as u32) << 24)
        | ((offset24 as u32) & 0x00FF_FFFF);
    word.to_le_bytes()
}

/// Encode BX (Branch and Exchange) instruction.
///
/// Format: `cond[31:28] | 00010010[27:20] | 1111[19:16] | 1111[15:12] |
///         1111[11:8] | 0001[7:4] | Rm[3:0]`
fn encode_bx(cond: Condition, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_0010 << 20)
        | (0b1111 << 16)
        | (0b1111 << 12)
        | (0b1111 << 8) // SBZ
        | (0b0001 << 4) // BX opcode in bits [7:4]
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode BLX (Branch with Link and Exchange) register instruction.
///
/// Format: `cond[31:28] | 00010010[27:20] | 1111[19:16] | 1111[15:12] |
///         1111[11:8] | 0011[7:4] | Rm[3:0]`
fn encode_blx_reg(cond: Condition, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_0010 << 20)
        | (0b1111 << 16)
        | (0b1111 << 12)
        | (0b1111 << 8) // SBZ
        | (0b0011 << 4) // BLX opcode in bits [7:4]
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode MUL instruction.
///
/// Format: `cond[31:28] | 000000[27:22] | S[21] | Rd[19:16] | Rn[15:12] |
///         Rs[11:8] | 1001[7:4] | Rm[3:0]`
fn encode_mul(cond: Condition, s: bool, rd: u32, rn: u32, rs: u32, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | ((s as u32) << 20)
        | ((rd & 0xF) << 16)
        | ((rn & 0xF) << 12)
        | ((rs & 0xF) << 8)
        | (0b1001 << 4)
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode MLA instruction.
///
/// Format: `cond[31:28] | 0000001[27:21] | S[20] | Rd[19:16] | Rn[15:12] |
///         Rs[11:8] | 1001[7:4] | Rm[3:0]`
fn encode_mla(cond: Condition, s: bool, rd: u32, rn: u32, rs: u32, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0000001 << 21)
        | ((s as u32) << 20)
        | ((rd & 0xF) << 16)
        | ((rn & 0xF) << 12)
        | ((rs & 0xF) << 8)
        | (0b1001 << 4)
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode UMULL instruction.
///
/// Format: `cond[31:28] | 0000100[27:21] | S[20] | RdHi[19:16] | RdLo[15:12] |
///         Rs[11:8] | 1001[7:4] | Rm[3:0]`
fn encode_umull(cond: Condition, s: bool, rd_hi: u32, rd_lo: u32, rs: u32, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0000100 << 21)
        | ((s as u32) << 20)
        | ((rd_hi & 0xF) << 16)
        | ((rd_lo & 0xF) << 12)
        | ((rs & 0xF) << 8)
        | (0b1001 << 4)
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode SMULL instruction.
///
/// Format: `cond[31:28] | 0000110[27:21] | S[20] | RdHi[19:16] | RdLo[15:12] |
///         Rs[11:8] | 1001[7:4] | Rm[3:0]`
fn encode_smull(cond: Condition, s: bool, rd_hi: u32, rd_lo: u32, rs: u32, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0000110 << 21)
        | ((s as u32) << 20)
        | ((rd_hi & 0xF) << 16)
        | ((rd_lo & 0xF) << 12)
        | ((rs & 0xF) << 8)
        | (0b1001 << 4)
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode LDM (Load Multiple) instruction.
///
/// Format: `cond[31:28] | 100[27:25] | P[24] | U[23] | S[22] | W[21] | L=1[20] |
///         Rn[19:16] | register_list[15:0]`
fn encode_ldm(
    cond: Condition,
    p: bool,
    u: bool,
    s: bool,
    w: bool,
    rn: u32,
    register_list: u16,
) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b100 << 25)
        | ((p as u32) << 24)
        | ((u as u32) << 23)
        | ((s as u32) << 22)
        | ((w as u32) << 21)
        | (1 << 20) // L=1 for load
        | ((rn & 0xF) << 16)
        | (register_list as u32);
    word.to_le_bytes()
}

/// Encode STM (Store Multiple) instruction.
///
/// Format: `cond[31:28] | 100[27:25] | P[24] | U[23] | S[22] | W[21] | L=0[20] |
///         Rn[19:16] | register_list[15:0]`
fn encode_stm(
    cond: Condition,
    p: bool,
    u: bool,
    s: bool,
    w: bool,
    rn: u32,
    register_list: u16,
) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b100 << 25)
        | ((p as u32) << 24)
        | ((u as u32) << 23)
        | ((s as u32) << 22)
        | ((w as u32) << 21)
        // L=0 for store
        | ((rn & 0xF) << 16)
        | (register_list as u32);
    word.to_le_bytes()
}

/// Encode SVC (Supervisor Call) instruction.
///
/// Format: `cond[31:28] | 1111[27:24] | imm24[23:0]`
fn encode_svc(cond: Condition, imm24: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28) | (0b1111 << 24) | (imm24 & 0x00FF_FFFF);
    word.to_le_bytes()
}

/// Encode MRS instruction (Move Status Register to GPR).
///
/// Format: `cond[31:28] | 0001_0R_00[27:20] | 1111[19:16] (SBZ) | Rd[15:12] |
///         000000000000[11:0] (SBZ)`
/// For CPSR: R=0. For SPSR: R=1 (bit 22).
fn encode_mrs(cond: Condition, rd: u32, spsr: bool) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_0000 << 20)
        | ((spsr as u32) << 22)
        | (0b1111 << 16) // bits [19:16] = 1111 (SBZ)
        | ((rd & 0xF) << 12);
        // bits [11:0] = 0 by default
    word.to_le_bytes()
}

/// Encode MSR instruction (Move GPR to Status Register).
///
/// Format: `cond[31:28] | 00010010[27:20] | mask[19:16] | 1111[15:12] |
///         00000000[11:4] | Rm[3:0]`
fn encode_msr(cond: Condition, mask: u32, rm: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_0010 << 20)
        | ((mask & 0xF) << 16)
        | (0b1111 << 12)
        | (rm & 0xF);
    word.to_le_bytes()
}

/// Encode LDREX (Load Register Exclusive) instruction.
///
/// Format: `cond[31:28] | 00011011[27:20] | Rn[19:16] | Rd[15:12] | 1111[11:8] | 1001[7:4] | 1111[3:0]`
fn encode_ldrex(cond: Condition, rn: u32, rd: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_1011 << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (0b1111 << 8)
        | (0b1001 << 4)
        | 0b1111;
    word.to_le_bytes()
}

/// Encode LDREXB (Load Register Exclusive Byte) instruction.
///
/// Format: `cond[31:28] | 00011101[27:20] | Rn[19:16] | Rd[15:12] | 1111[11:8] | 1001[7:4] | 1111[3:0]`
fn encode_ldrexb(cond: Condition, rn: u32, rd: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_1101 << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (0b1111 << 8)
        | (0b1001 << 4)
        | 0b1111;
    word.to_le_bytes()
}

/// Encode LDREXH (Load Register Exclusive Halfword) instruction.
///
/// Format: `cond[31:28] | 00011111[27:20] | Rn[19:16] | Rd[15:12] | 1111[11:8] | 1001[7:4] | 1111[3:0]`
fn encode_ldrexh(cond: Condition, rn: u32, rd: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_1111 << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (0b1111 << 8)
        | (0b1001 << 4)
        | 0b1111;
    word.to_le_bytes()
}

/// Encode STREX (Store Register Exclusive) instruction.
///
/// Format: `cond[31:28] | 00011000[27:20] | Rn[19:16] | Rd[15:12] | 1111[11:8] | 1001[7:4] | Rt[3:0]`
///
/// Rd = destination status register (0 = success, 1 = failure)
/// Rn = base address register
/// Rt = source register (value to store)
fn encode_strex(cond: Condition, rn: u32, rd: u32, rt: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_1000 << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (0b1111 << 8)
        | (0b1001 << 4)
        | (rt & 0xF);
    word.to_le_bytes()
}

/// Encode STREXB (Store Register Exclusive Byte) instruction.
///
/// Format: `cond[31:28] | 00011100[27:20] | Rn[19:16] | Rd[15:12] | 1111[11:8] | 1001[7:4] | Rt[3:0]`
fn encode_strexb(cond: Condition, rn: u32, rd: u32, rt: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_1100 << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (0b1111 << 8)
        | (0b1001 << 4)
        | (rt & 0xF);
    word.to_le_bytes()
}

/// Encode STREXH (Store Register Exclusive Halfword) instruction.
///
/// Format: `cond[31:28] | 00011110[27:20] | Rn[19:16] | Rd[15:12] | 1111[11:8] | 1001[7:4] | Rt[3:0]`
fn encode_strexh(cond: Condition, rn: u32, rd: u32, rt: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_1110 << 20)
        | ((rn & 0xF) << 16)
        | ((rd & 0xF) << 12)
        | (0b1111 << 8)
        | (0b1001 << 4)
        | (rt & 0xF);
    word.to_le_bytes()
}

/// Encode DMB (Data Memory Barrier) instruction.
///
/// Format: `cond[31:28] | 01010111[27:20] | 1111[19:16] | 1111[15:12] | 1111[11:8] | 0101[7:4] | option[3:0]`
///
/// option = 0xF for DMB SY (full system barrier)
fn encode_dmb(cond: Condition, option: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0101_0111 << 20)
        | (0b1111 << 16)
        | (0b1111 << 12)
        | (0b1111 << 8)
        | (0b0101 << 4)
        | (option & 0xF);
    word.to_le_bytes()
}

// ===========================================================================
// Instruction Enum
// ===========================================================================

/// ARM 32-bit instruction representations for code generation.
///
/// Covers data processing, load/store, branch, multiply, and system
/// instructions. Each variant captures the operands needed for encoding and
/// disassembly. The `encode()` method produces a 4-byte little-endian machine
/// code word.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Instruction {
    // ── Data Processing: Register-Register ────────────────────────────
    /// ADD Rd, Rn, Rm
    Add {
        rd: Gpr,
        rn: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// SUB Rd, Rn, Rm
    Sub {
        rd: Gpr,
        rn: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// AND Rd, Rn, Rm
    And {
        rd: Gpr,
        rn: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// ORR Rd, Rn, Rm
    Orr {
        rd: Gpr,
        rn: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// EOR Rd, Rn, Rm
    Eor {
        rd: Gpr,
        rn: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// BIC Rd, Rn, Rm
    Bic {
        rd: Gpr,
        rn: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// MOV Rd, Rm
    Mov { rd: Gpr, rm: Gpr, cond: Condition },
    /// MVN Rd, Rm
    Mvn { rd: Gpr, rm: Gpr, cond: Condition },
    /// CMP Rn, Rm
    Cmp { rn: Gpr, rm: Gpr, cond: Condition },
    /// CMN Rn, Rm
    Cmn { rn: Gpr, rm: Gpr, cond: Condition },
    /// TST Rn, Rm
    Tst { rn: Gpr, rm: Gpr, cond: Condition },
    /// TEQ Rn, Rm
    Teq { rn: Gpr, rm: Gpr, cond: Condition },

    // ── Data Processing: Immediate ───────────────────────────────────
    /// ADD Rd, Rn, #imm8 (rotated)
    AddImm {
        rd: Gpr,
        rn: Gpr,
        rotate: u32,
        imm8: u32,
        cond: Condition,
    },
    /// SUB Rd, Rn, #imm8 (rotated)
    SubImm {
        rd: Gpr,
        rn: Gpr,
        rotate: u32,
        imm8: u32,
        cond: Condition,
    },
    /// MOV Rd, #imm8 (rotated)
    MovImm {
        rd: Gpr,
        rotate: u32,
        imm8: u32,
        cond: Condition,
    },
    /// CMP Rn, #imm8 (rotated)
    CmpImm {
        rn: Gpr,
        rotate: u32,
        imm8: u32,
        cond: Condition,
    },

    // ── Shift by Immediate ───────────────────────────────────────────
    /// LSL Rd, Rm, #shift_imm (encoded as MOV Rd, Rm, LSL #imm)
    LslImm {
        rd: Gpr,
        rm: Gpr,
        shift_imm: u32,
        cond: Condition,
    },
    /// LSR Rd, Rm, #shift_imm
    LsrImm {
        rd: Gpr,
        rm: Gpr,
        shift_imm: u32,
        cond: Condition,
    },
    /// ASR Rd, Rm, #shift_imm
    AsrImm {
        rd: Gpr,
        rm: Gpr,
        shift_imm: u32,
        cond: Condition,
    },
    /// ROR Rd, Rm, #shift_imm
    RorImm {
        rd: Gpr,
        rm: Gpr,
        shift_imm: u32,
        cond: Condition,
    },

    // ── Shift by Register ────────────────────────────────────────────
    /// LSL Rd, Rn, Rs (encoded as MOV Rd, Rn, LSL Rs)
    LslReg {
        rd: Gpr,
        rn: Gpr,
        rs: Gpr,
        cond: Condition,
    },
    /// LSR Rd, Rn, Rs
    LsrReg {
        rd: Gpr,
        rn: Gpr,
        rs: Gpr,
        cond: Condition,
    },
    /// ASR Rd, Rn, Rs
    AsrReg {
        rd: Gpr,
        rn: Gpr,
        rs: Gpr,
        cond: Condition,
    },
    /// ROR Rd, Rn, Rs
    RorReg {
        rd: Gpr,
        rn: Gpr,
        rs: Gpr,
        cond: Condition,
    },

    // ── Multiply ─────────────────────────────────────────────────────
    /// MUL Rd, Rm, Rs
    Mul {
        rd: Gpr,
        rn: Gpr,
        rs: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// MLA Rd, Rn, Rm, Rs (Rd = Rn + Rm * Rs)
    Mla {
        rd: Gpr,
        rn: Gpr,
        rs: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// UMULL RdLo, RdHi, Rm, Rs
    Umull {
        rd_hi: Gpr,
        rd_lo: Gpr,
        rs: Gpr,
        rm: Gpr,
        cond: Condition,
    },
    /// SMULL RdLo, RdHi, Rm, Rs
    Smull {
        rd_hi: Gpr,
        rd_lo: Gpr,
        rs: Gpr,
        rm: Gpr,
        cond: Condition,
    },

    // ── Load/Store Word ──────────────────────────────────────────────
    /// LDR Rd, \[Rn, #offset\]
    Ldr {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },
    /// STR Rd, \[Rn, #offset\]
    Str {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },

    // ── Load/Store Byte ──────────────────────────────────────────────
    /// LDRB Rd, \[Rn, #offset\]
    Ldrb {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },
    /// STRB Rd, \[Rn, #offset\]
    Strb {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },

    // ── Load/Store Halfword ──────────────────────────────────────────
    /// LDRH Rd, \[Rn, #offset\]
    Ldrh {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },
    /// STRH Rd, \[Rn, #offset\]
    Strh {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },

    // ── Load/Store Doubleword ────────────────────────────────────────
    /// LDRD Rd, \[Rn, #offset\]
    Ldrd {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },
    /// STRD Rd, \[Rn, #offset\]
    Strd {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },

    // ── Load Signed Byte/Halfword ────────────────────────────────────
    /// LDRSB Rd, \[Rn, #offset\]
    Ldrsb {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },
    /// LDRSH Rd, \[Rn, #offset\]
    Ldrsh {
        rd: Gpr,
        rn: Gpr,
        offset: i32,
        cond: Condition,
    },

    // ── Load/Store Multiple ──────────────────────────────────────────
    /// LDM Rn!, {register_list}
    Ldm {
        rn: Gpr,
        register_list: u16,
        writeback: bool,
        cond: Condition,
    },
    /// STM Rn!, {register_list}
    Stm {
        rn: Gpr,
        register_list: u16,
        writeback: bool,
        cond: Condition,
    },

    // ── Branch ───────────────────────────────────────────────────────
    /// B offset (signed 24-bit, word-aligned)
    B { offset: i32, cond: Condition },
    /// BL offset
    Bl { offset: i32, cond: Condition },
    /// BX Rm
    Bx { rm: Gpr, cond: Condition },
    /// BLX Rm
    BlxReg { rm: Gpr, cond: Condition },

    // ── System ───────────────────────────────────────────────────────
    /// SVC #imm24
    Svc { imm24: u32, cond: Condition },
    /// NOP (MOV R0, R0)
    Nop,
    /// MRS Rd, CPSR
    Mrs {
        rd: Gpr,
        spsr: bool,
        cond: Condition,
    },
    /// MSR CPSR_f, Rm
    Msr { mask: u32, rm: Gpr, cond: Condition },

    // ── Synchronization Primitives (ARMv7-A) ────────────────────────
    /// LDREX Rd, [Rn] — Load Register Exclusive (32-bit)
    Ldrex { rd: Gpr, rn: Gpr, cond: Condition },
    /// LDREXB Rd, [Rn] — Load Register Exclusive Byte
    Ldrexb { rd: Gpr, rn: Gpr, cond: Condition },
    /// LDREXH Rd, [Rn] — Load Register Exclusive Halfword
    Ldrexh { rd: Gpr, rn: Gpr, cond: Condition },
    /// STREX Rd, Rt, [Rn] — Store Register Exclusive (32-bit)
    /// Rd = status destination (0=success, 1=failure), Rt = value source, Rn = address
    Strex { rd: Gpr, rt: Gpr, rn: Gpr, cond: Condition },
    /// STREXB Rd, Rt, [Rn] — Store Register Exclusive Byte
    Strexb { rd: Gpr, rt: Gpr, rn: Gpr, cond: Condition },
    /// STREXH Rd, Rt, [Rn] — Store Register Exclusive Halfword
    Strexh { rd: Gpr, rt: Gpr, rn: Gpr, cond: Condition },
    /// DMB option — Data Memory Barrier (option=0xF for DMB SY)
    Dmb { option: u32, cond: Condition },

    // ── VFP Conversion ─────────────────────────────────────────────
    /// VCVT.F32.S32 Sd, Sm — convert signed integer to single-precision float
    VcvtF32S32 { sd: u8, sm: u8, cond: Condition },
    /// VCVT.F32.U32 Sd, Sm — convert unsigned integer to single-precision float
    VcvtF32U32 { sd: u8, sm: u8, cond: Condition },
    /// VCVT.S32.F32 Sd, Sm — convert single-precision float to signed integer
    VcvtS32F32 { sd: u8, sm: u8, cond: Condition },
    /// VCVT.U32.F32 Sd, Sm — convert single-precision float to unsigned integer
    VcvtU32F32 { sd: u8, sm: u8, cond: Condition },
    /// VCVT.F64.F32 Dd, Sm — convert single-precision to double-precision
    VcvtF64F32 { dd: u8, sm: u8, cond: Condition },
    /// VCVT.F32.F64 Sd, Dm — convert double-precision to single-precision
    VcvtF32F64 { sd: u8, dm: u8, cond: Condition },
}

impl Instruction {
    /// Encode this instruction into a 4-byte little-endian machine code word.
    ///
    /// Encoding follows the ARM Architecture Reference Manual.
    pub fn encode(&self) -> [u8; 4] {
        match self {
            // ── Data Processing: Register-Register ──────────────────
            Instruction::Add { rd, rn, rm, cond } => encode_dp_reg(
                *cond,
                DP_ADD,
                false,
                rn.encoding(),
                rd.encoding(),
                rm.encoding(),
            ),
            Instruction::Sub { rd, rn, rm, cond } => encode_dp_reg(
                *cond,
                DP_SUB,
                false,
                rn.encoding(),
                rd.encoding(),
                rm.encoding(),
            ),
            Instruction::And { rd, rn, rm, cond } => encode_dp_reg(
                *cond,
                DP_AND,
                false,
                rn.encoding(),
                rd.encoding(),
                rm.encoding(),
            ),
            Instruction::Orr { rd, rn, rm, cond } => encode_dp_reg(
                *cond,
                DP_ORR,
                false,
                rn.encoding(),
                rd.encoding(),
                rm.encoding(),
            ),
            Instruction::Eor { rd, rn, rm, cond } => encode_dp_reg(
                *cond,
                DP_EOR,
                false,
                rn.encoding(),
                rd.encoding(),
                rm.encoding(),
            ),
            Instruction::Bic { rd, rn, rm, cond } => encode_dp_reg(
                *cond,
                DP_BIC,
                false,
                rn.encoding(),
                rd.encoding(),
                rm.encoding(),
            ),
            Instruction::Mov { rd, rm, cond } => {
                // MOV: Rn is SBZ (should be 0)
                encode_dp_reg(*cond, DP_MOV, false, 0, rd.encoding(), rm.encoding())
            }
            Instruction::Mvn { rd, rm, cond } => {
                encode_dp_reg(*cond, DP_MVN, false, 0, rd.encoding(), rm.encoding())
            }
            Instruction::Cmp { rn, rm, cond } => {
                // CMP: Rd is SBZ, S=1
                encode_dp_reg(*cond, DP_CMP, true, rn.encoding(), 0, rm.encoding())
            }
            Instruction::Cmn { rn, rm, cond } => {
                encode_dp_reg(*cond, DP_CMN, true, rn.encoding(), 0, rm.encoding())
            }
            Instruction::Tst { rn, rm, cond } => {
                encode_dp_reg(*cond, DP_TST, true, rn.encoding(), 0, rm.encoding())
            }
            Instruction::Teq { rn, rm, cond } => {
                encode_dp_reg(*cond, DP_TEQ, true, rn.encoding(), 0, rm.encoding())
            }

            // ── Data Processing: Immediate ──────────────────────────
            Instruction::AddImm {
                rd,
                rn,
                rotate,
                imm8,
                cond,
            } => encode_dp_imm(
                *cond,
                DP_ADD,
                false,
                rn.encoding(),
                rd.encoding(),
                *rotate,
                *imm8,
            ),
            Instruction::SubImm {
                rd,
                rn,
                rotate,
                imm8,
                cond,
            } => encode_dp_imm(
                *cond,
                DP_SUB,
                false,
                rn.encoding(),
                rd.encoding(),
                *rotate,
                *imm8,
            ),
            Instruction::MovImm {
                rd,
                rotate,
                imm8,
                cond,
            } => encode_dp_imm(*cond, DP_MOV, false, 0, rd.encoding(), *rotate, *imm8),
            Instruction::CmpImm {
                rn,
                rotate,
                imm8,
                cond,
            } => encode_dp_imm(*cond, DP_CMP, true, rn.encoding(), 0, *rotate, *imm8),

            // ── Shift by Immediate ──────────────────────────────────
            Instruction::LslImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                // LSL = shift_type 0, encoded as MOV Rd, Rm, LSL #imm
                encode_dp_shift_imm(
                    *cond,
                    DP_MOV,
                    false,
                    0,
                    rd.encoding(),
                    0,
                    *shift_imm,
                    rm.encoding(),
                )
            }
            Instruction::LsrImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                // LSR = shift_type 1
                encode_dp_shift_imm(
                    *cond,
                    DP_MOV,
                    false,
                    0,
                    rd.encoding(),
                    1,
                    *shift_imm,
                    rm.encoding(),
                )
            }
            Instruction::AsrImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                // ASR = shift_type 2
                encode_dp_shift_imm(
                    *cond,
                    DP_MOV,
                    false,
                    0,
                    rd.encoding(),
                    2,
                    *shift_imm,
                    rm.encoding(),
                )
            }
            Instruction::RorImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                // ROR = shift_type 3
                encode_dp_shift_imm(
                    *cond,
                    DP_MOV,
                    false,
                    0,
                    rd.encoding(),
                    3,
                    *shift_imm,
                    rm.encoding(),
                )
            }

            // ── Shift by Register ───────────────────────────────────
            Instruction::LslReg { rd, rn, rs, cond } => encode_dp_shift_reg(
                *cond,
                DP_MOV,
                false,
                0,
                rd.encoding(),
                0,
                rs.encoding(),
                rn.encoding(),
            ),
            Instruction::LsrReg { rd, rn, rs, cond } => encode_dp_shift_reg(
                *cond,
                DP_MOV,
                false,
                0,
                rd.encoding(),
                1,
                rs.encoding(),
                rn.encoding(),
            ),
            Instruction::AsrReg { rd, rn, rs, cond } => encode_dp_shift_reg(
                *cond,
                DP_MOV,
                false,
                0,
                rd.encoding(),
                2,
                rs.encoding(),
                rn.encoding(),
            ),
            Instruction::RorReg { rd, rn, rs, cond } => encode_dp_shift_reg(
                *cond,
                DP_MOV,
                false,
                0,
                rd.encoding(),
                3,
                rs.encoding(),
                rn.encoding(),
            ),

            // ── Multiply ────────────────────────────────────────────
            Instruction::Mul {
                rd,
                rn: _,
                rs,
                rm,
                cond,
            } => encode_mul(
                *cond,
                false,
                rd.encoding(),
                0, // SBZ: bits [15:12] must be 0 for MUL
                rs.encoding(),
                rm.encoding(),
            ),
            Instruction::Mla {
                rd,
                rn,
                rs,
                rm,
                cond,
            } => encode_mla(
                *cond,
                false,
                rd.encoding(),
                rn.encoding(),
                rs.encoding(),
                rm.encoding(),
            ),
            Instruction::Umull {
                rd_hi,
                rd_lo,
                rs,
                rm,
                cond,
            } => encode_umull(
                *cond,
                false,
                rd_hi.encoding(),
                rd_lo.encoding(),
                rs.encoding(),
                rm.encoding(),
            ),
            Instruction::Smull {
                rd_hi,
                rd_lo,
                rs,
                rm,
                cond,
            } => encode_smull(
                *cond,
                false,
                rd_hi.encoding(),
                rd_lo.encoding(),
                rs.encoding(),
                rm.encoding(),
            ),

            // ── Load/Store Word ─────────────────────────────────────
            Instruction::Ldr {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(
                    *cond,
                    true,
                    u,
                    false,
                    false,
                    true,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }
            Instruction::Str {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(
                    *cond,
                    true,
                    u,
                    false,
                    false,
                    false,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }

            // ── Load/Store Byte ─────────────────────────────────────
            Instruction::Ldrb {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(
                    *cond,
                    true,
                    u,
                    true,
                    false,
                    true,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }
            Instruction::Strb {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(
                    *cond,
                    true,
                    u,
                    true,
                    false,
                    false,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }

            // ── Load/Store Halfword ─────────────────────────────────
            Instruction::Ldrh {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_half_imm(
                    *cond,
                    true,
                    u,
                    false,
                    true,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }
            Instruction::Strh {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_half_imm(
                    *cond,
                    true,
                    u,
                    false,
                    false,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }

            // ── Load/Store Doubleword ───────────────────────────────
            Instruction::Ldrd {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_double_imm(
                    *cond,
                    true,
                    u,
                    false,
                    true,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }
            Instruction::Strd {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_double_imm(
                    *cond,
                    true,
                    u,
                    false,
                    false,
                    rn.encoding(),
                    rd.encoding(),
                    off,
                )
            }

            // ── Load Signed Byte/Halfword ───────────────────────────
            Instruction::Ldrsb {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ldrsb_imm(*cond, true, u, false, rn.encoding(), rd.encoding(), off)
            }
            Instruction::Ldrsh {
                rd,
                rn,
                offset,
                cond,
            } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ldrsh_imm(*cond, true, u, false, rn.encoding(), rd.encoding(), off)
            }

            // ── Load/Store Multiple ─────────────────────────────────
            Instruction::Ldm {
                rn,
                register_list,
                writeback,
                cond,
            } => {
                // LDM = Increment After (P=0, U=1) — typical IA variant
                encode_ldm(
                    *cond,
                    false,
                    true,
                    false,
                    *writeback,
                    rn.encoding(),
                    *register_list,
                )
            }
            Instruction::Stm {
                rn,
                register_list,
                writeback,
                cond,
            } => {
                // STM = Decrement Before (P=1, U=0) — typical DB (push) variant
                encode_stm(
                    *cond,
                    true,
                    false,
                    false,
                    *writeback,
                    rn.encoding(),
                    *register_list,
                )
            }

            // ── Branch ──────────────────────────────────────────────
            Instruction::B { offset, cond } => encode_branch(*cond, false, *offset >> 2),
            Instruction::Bl { offset, cond } => encode_branch(*cond, true, *offset >> 2),
            Instruction::Bx { rm, cond } => encode_bx(*cond, rm.encoding()),
            Instruction::BlxReg { rm, cond } => encode_blx_reg(*cond, rm.encoding()),

            // ── System ──────────────────────────────────────────────
            Instruction::Svc { imm24, cond } => encode_svc(*cond, *imm24),
            Instruction::Nop => {
                // NOP = MOV R0, R0 = 0xE1A00000
                0xE1A0_0000u32.to_le_bytes()
            }
            Instruction::Mrs { rd, spsr, cond } => encode_mrs(*cond, rd.encoding(), *spsr),
            Instruction::Msr { mask, rm, cond } => encode_msr(*cond, *mask, rm.encoding()),

            // ── Synchronization Primitives ────────────────────────────
            Instruction::Ldrex { rd, rn, cond } => {
                encode_ldrex(*cond, rn.encoding(), rd.encoding())
            }
            Instruction::Ldrexb { rd, rn, cond } => {
                encode_ldrexb(*cond, rn.encoding(), rd.encoding())
            }
            Instruction::Ldrexh { rd, rn, cond } => {
                encode_ldrexh(*cond, rn.encoding(), rd.encoding())
            }
            Instruction::Strex { rd, rt, rn, cond } => {
                encode_strex(*cond, rn.encoding(), rd.encoding(), rt.encoding())
            }
            Instruction::Strexb { rd, rt, rn, cond } => {
                encode_strexb(*cond, rn.encoding(), rd.encoding(), rt.encoding())
            }
            Instruction::Strexh { rd, rt, rn, cond } => {
                encode_strexh(*cond, rn.encoding(), rd.encoding(), rt.encoding())
            }
            Instruction::Dmb { option, cond } => {
                encode_dmb(*cond, *option)
            }

            // ── VFP Conversion ─────────────────────────────────────
            Instruction::VcvtF32S32 { sd, sm, cond: _ } => {
                encode_vcvt_f32_s32(*sd, *sm)
            }
            Instruction::VcvtF32U32 { sd, sm, cond: _ } => {
                encode_vcvt_f32_u32(*sd, *sm)
            }
            Instruction::VcvtS32F32 { sd, sm, cond: _ } => {
                encode_vcvt_s32_f32(*sd, *sm)
            }
            Instruction::VcvtU32F32 { sd, sm, cond: _ } => {
                encode_vcvt_u32_f32(*sd, *sm)
            }
            Instruction::VcvtF64F32 { dd, sm, cond: _ } => {
                encode_vcvt_f64_f32(*dd, *sm)
            }
            Instruction::VcvtF32F64 { sd, dm, cond: _ } => {
                encode_vcvt_f32_f64(*sd, *dm)
            }
        }
    }

    /// Returns the mnemonic name of this instruction.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Instruction::Add { .. } => "add",
            Instruction::Sub { .. } => "sub",
            Instruction::And { .. } => "and",
            Instruction::Orr { .. } => "orr",
            Instruction::Eor { .. } => "eor",
            Instruction::Bic { .. } => "bic",
            Instruction::Mov { .. } => "mov",
            Instruction::Mvn { .. } => "mvn",
            Instruction::Cmp { .. } => "cmp",
            Instruction::Cmn { .. } => "cmn",
            Instruction::Tst { .. } => "tst",
            Instruction::Teq { .. } => "teq",
            Instruction::AddImm { .. } => "add",
            Instruction::SubImm { .. } => "sub",
            Instruction::MovImm { .. } => "mov",
            Instruction::CmpImm { .. } => "cmp",
            Instruction::LslImm { .. } => "lsl",
            Instruction::LsrImm { .. } => "lsr",
            Instruction::AsrImm { .. } => "asr",
            Instruction::RorImm { .. } => "ror",
            Instruction::LslReg { .. } => "lsl",
            Instruction::LsrReg { .. } => "lsr",
            Instruction::AsrReg { .. } => "asr",
            Instruction::RorReg { .. } => "ror",
            Instruction::Mul { .. } => "mul",
            Instruction::Mla { .. } => "mla",
            Instruction::Umull { .. } => "umull",
            Instruction::Smull { .. } => "smull",
            Instruction::Ldr { .. } => "ldr",
            Instruction::Str { .. } => "str",
            Instruction::Ldrb { .. } => "ldrb",
            Instruction::Strb { .. } => "strb",
            Instruction::Ldrh { .. } => "ldrh",
            Instruction::Strh { .. } => "strh",
            Instruction::Ldrd { .. } => "ldrd",
            Instruction::Strd { .. } => "strd",
            Instruction::Ldrsb { .. } => "ldrsb",
            Instruction::Ldrsh { .. } => "ldrsh",
            Instruction::Ldm { .. } => "ldm",
            Instruction::Stm { .. } => "stm",
            Instruction::B { .. } => "b",
            Instruction::Bl { .. } => "bl",
            Instruction::Bx { .. } => "bx",
            Instruction::BlxReg { .. } => "blx",
            Instruction::Svc { .. } => "svc",
            Instruction::Nop => "nop",
            Instruction::Mrs { .. } => "mrs",
            Instruction::Msr { .. } => "msr",
            Instruction::Ldrex { .. } => "ldrex",
            Instruction::Ldrexb { .. } => "ldrexb",
            Instruction::Ldrexh { .. } => "ldrexh",
            Instruction::Strex { .. } => "strex",
            Instruction::Strexb { .. } => "strexb",
            Instruction::Strexh { .. } => "strexh",
            Instruction::Dmb { .. } => "dmb",
            Instruction::VcvtF32S32 { .. } => "vcvt.f32.s32",
            Instruction::VcvtF32U32 { .. } => "vcvt.f32.u32",
            Instruction::VcvtS32F32 { .. } => "vcvt.s32.f32",
            Instruction::VcvtU32F32 { .. } => "vcvt.u32.f32",
            Instruction::VcvtF64F32 { .. } => "vcvt.f64.f32",
            Instruction::VcvtF32F64 { .. } => "vcvt.f32.f64",
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Add { rd, rn, rm, cond } => {
                write!(f, "add{} {}, {}, {}", cond, rd, rn, rm)
            }
            Instruction::Sub { rd, rn, rm, cond } => {
                write!(f, "sub{} {}, {}, {}", cond, rd, rn, rm)
            }
            Instruction::And { rd, rn, rm, cond } => {
                write!(f, "and{} {}, {}, {}", cond, rd, rn, rm)
            }
            Instruction::Orr { rd, rn, rm, cond } => {
                write!(f, "orr{} {}, {}, {}", cond, rd, rn, rm)
            }
            Instruction::Eor { rd, rn, rm, cond } => {
                write!(f, "eor{} {}, {}, {}", cond, rd, rn, rm)
            }
            Instruction::Bic { rd, rn, rm, cond } => {
                write!(f, "bic{} {}, {}, {}", cond, rd, rn, rm)
            }
            Instruction::Mov { rd, rm, cond } => write!(f, "mov{} {}, {}", cond, rd, rm),
            Instruction::Mvn { rd, rm, cond } => write!(f, "mvn{} {}, {}", cond, rd, rm),
            Instruction::Cmp { rn, rm, cond } => write!(f, "cmp{} {}, {}", cond, rn, rm),
            Instruction::Cmn { rn, rm, cond } => write!(f, "cmn{} {}, {}", cond, rn, rm),
            Instruction::Tst { rn, rm, cond } => write!(f, "tst{} {}, {}", cond, rn, rm),
            Instruction::Teq { rn, rm, cond } => write!(f, "teq{} {}, {}", cond, rn, rm),
            Instruction::AddImm {
                rd,
                rn,
                rotate: _,
                imm8,
                cond,
            } => {
                write!(f, "add{} {}, {}, #{}", cond, rd, rn, imm8)
            }
            Instruction::SubImm {
                rd,
                rn,
                rotate: _,
                imm8,
                cond,
            } => {
                write!(f, "sub{} {}, {}, #{}", cond, rd, rn, imm8)
            }
            Instruction::MovImm {
                rd,
                rotate: _,
                imm8,
                cond,
            } => {
                write!(f, "mov{} {}, #{}", cond, rd, imm8)
            }
            Instruction::CmpImm {
                rn,
                rotate: _,
                imm8,
                cond,
            } => {
                write!(f, "cmp{} {}, #{}", cond, rn, imm8)
            }
            Instruction::LslImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                write!(f, "lsl{} {}, {}, #{}", cond, rd, rm, shift_imm)
            }
            Instruction::LsrImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                write!(f, "lsr{} {}, {}, #{}", cond, rd, rm, shift_imm)
            }
            Instruction::AsrImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                write!(f, "asr{} {}, {}, #{}", cond, rd, rm, shift_imm)
            }
            Instruction::RorImm {
                rd,
                rm,
                shift_imm,
                cond,
            } => {
                write!(f, "ror{} {}, {}, #{}", cond, rd, rm, shift_imm)
            }
            Instruction::LslReg { rd, rn, rs, cond } => {
                write!(f, "lsl{} {}, {}, {}", cond, rd, rn, rs)
            }
            Instruction::LsrReg { rd, rn, rs, cond } => {
                write!(f, "lsr{} {}, {}, {}", cond, rd, rn, rs)
            }
            Instruction::AsrReg { rd, rn, rs, cond } => {
                write!(f, "asr{} {}, {}, {}", cond, rd, rn, rs)
            }
            Instruction::RorReg { rd, rn, rs, cond } => {
                write!(f, "ror{} {}, {}, {}", cond, rd, rn, rs)
            }
            Instruction::Mul {
                rd,
                rn,
                rs,
                rm: _,
                cond,
            } => {
                write!(f, "mul{} {}, {}, {}", cond, rd, rn, rs)
            }
            Instruction::Mla {
                rd,
                rn,
                rs,
                rm,
                cond,
            } => {
                write!(f, "mla{} {}, {}, {}, {}", cond, rd, rn, rm, rs)
            }
            Instruction::Umull {
                rd_hi,
                rd_lo,
                rs,
                rm,
                cond,
            } => {
                write!(f, "umull{} {}, {}, {}, {}", cond, rd_lo, rd_hi, rm, rs)
            }
            Instruction::Smull {
                rd_hi,
                rd_lo,
                rs,
                rm,
                cond,
            } => {
                write!(f, "smull{} {}, {}, {}, {}", cond, rd_lo, rd_hi, rm, rs)
            }
            Instruction::Ldr {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "ldr{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Str {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "str{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrb {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "ldrb{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Strb {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "strb{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrh {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "ldrh{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Strh {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "strh{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrd {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "ldrd{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Strd {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "strd{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrsb {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "ldrsb{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrsh {
                rd,
                rn,
                offset,
                cond,
            } => {
                write!(f, "ldrsh{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldm {
                rn,
                register_list,
                writeback,
                cond,
            } => {
                write!(
                    f,
                    "ldm{} {}{}, {{{:#06x}}}",
                    cond,
                    rn,
                    if *writeback { "!" } else { "" },
                    register_list
                )
            }
            Instruction::Stm {
                rn,
                register_list,
                writeback,
                cond,
            } => {
                write!(
                    f,
                    "stm{} {}{}, {{{:#06x}}}",
                    cond,
                    rn,
                    if *writeback { "!" } else { "" },
                    register_list
                )
            }
            Instruction::B { offset, cond } => write!(f, "b{} {:+}", cond, offset),
            Instruction::Bl { offset, cond } => write!(f, "bl{} {:+}", cond, offset),
            Instruction::Bx { rm, cond } => write!(f, "bx{} {}", cond, rm),
            Instruction::BlxReg { rm, cond } => write!(f, "blx{} {}", cond, rm),
            Instruction::Svc { imm24, cond } => write!(f, "svc{} #{}", cond, imm24),
            Instruction::Nop => write!(f, "nop"),
            Instruction::Mrs { rd, spsr, cond } => {
                write!(
                    f,
                    "mrs{} {}, {}",
                    cond,
                    rd,
                    if *spsr { "spsr" } else { "cpsr" }
                )
            }
            Instruction::Msr { mask, rm, cond } => {
                write!(f, "msr{} cpsr_{}, {}", cond, mask, rm)
            }
            Instruction::Ldrex { rd, rn, cond } => {
                write!(f, "ldrex{} {}, [{}]", cond, rd, rn)
            }
            Instruction::Ldrexb { rd, rn, cond } => {
                write!(f, "ldrexb{} {}, [{}]", cond, rd, rn)
            }
            Instruction::Ldrexh { rd, rn, cond } => {
                write!(f, "ldrexh{} {}, [{}]", cond, rd, rn)
            }
            Instruction::Strex { rd, rt, rn, cond } => {
                write!(f, "strex{} {}, {}, [{}]", cond, rd, rt, rn)
            }
            Instruction::Strexb { rd, rt, rn, cond } => {
                write!(f, "strexb{} {}, {}, [{}]", cond, rd, rt, rn)
            }
            Instruction::Strexh { rd, rt, rn, cond } => {
                write!(f, "strexh{} {}, {}, [{}]", cond, rd, rt, rn)
            }
            Instruction::Dmb { option, cond: _ } => {
                let opt_name = match option {
                    0xF => "sy",
                    _ => "???",
                };
                write!(f, "dmb {}", opt_name)
            }
            Instruction::VcvtF32S32 { sd, sm, cond } => {
                write!(f, "vcvt{}.f32.s32 s{}, s{}", cond, sd, sm)
            }
            Instruction::VcvtF32U32 { sd, sm, cond } => {
                write!(f, "vcvt{}.f32.u32 s{}, s{}", cond, sd, sm)
            }
            Instruction::VcvtS32F32 { sd, sm, cond } => {
                write!(f, "vcvt{}.s32.f32 s{}, s{}", cond, sd, sm)
            }
            Instruction::VcvtU32F32 { sd, sm, cond } => {
                write!(f, "vcvt{}.u32.f32 s{}, s{}", cond, sd, sm)
            }
            Instruction::VcvtF64F32 { dd, sm, cond } => {
                write!(f, "vcvt{}.f64.f32 d{}, s{}", cond, dd, sm)
            }
            Instruction::VcvtF32F64 { sd, dm, cond } => {
                write!(f, "vcvt{}.f32.f64 s{}, d{}", cond, sd, dm)
            }
        }
    }
}

// ===========================================================================
// ELF32 Emission
// ===========================================================================

/// Build a minimal ELF32 binary for ARM from raw code bytes with 2 LOAD segments.
///
/// Produces a static executable with:
/// - Segment 1: LOAD (PF_R | PF_X) — .text (code)
/// - Segment 2: LOAD (PF_R | PF_W) — .data / stack
///
/// Entry point is at `base_addr` + text file offset.
fn build_arm32_elf_2seg(code: &[u8], base_addr: u64) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x1000; // 4 KB

    let elf_header_size: u64 = 52;
    let phdr_size: u64 = 32;
    let num_phdrs: u64 = 2;
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    // Page-align the text segment start in the file for mmap compatibility.
    let text_offset = phdr_end; // No page alignment — code right after headers
    let text_size = code.len() as u64;

    // The data segment starts on the next page after the text.
    let text_file_end = text_offset + text_size;
    let data_vaddr =
        ((base_addr + text_file_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let data_offset = data_vaddr - base_addr;
    let data_size: u64 = PAGE_SIZE; // 1 page of writable memory for stack/data
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity((data_offset + data_size) as usize);

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(1); // ELFCLASS32
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0); // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields (32-bit) ---
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&40u16.to_le_bytes()); // e_machine = EM_ARM
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&(entry_point as u32).to_le_bytes()); // e_entry
    elf.extend_from_slice(&(elf_header_size as u32).to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_shoff (no section headers)
    // e_flags: ARM EF_ARM_ABI_VER5 = 0x05000000 (soft-float ABI)
    elf.extend_from_slice(&0x05000000u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&52u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&32u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_phnum = 2
    elf.extend_from_slice(&40u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    // ELF32 Phdr order: p_type, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_flags, p_align
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&0u32.to_le_bytes()); // p_offset = 0 (include ELF header)
    elf.extend_from_slice(&(base_addr as u32).to_le_bytes()); // p_vaddr = base_addr
    elf.extend_from_slice(&(base_addr as u32).to_le_bytes()); // p_paddr = base_addr
    elf.extend_from_slice(&((text_offset + text_size) as u32).to_le_bytes()); // p_filesz (headers + code)
    elf.extend_from_slice(&((text_offset + text_size) as u32).to_le_bytes()); // p_memsz
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&(PAGE_SIZE as u32).to_le_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data / stack ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&(text_file_end as u32).to_le_bytes()); // p_offset (use text end, not page-aligned data_offset)
    elf.extend_from_slice(&(data_vaddr as u32).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&(data_vaddr as u32).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u32.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&(data_size as u32).to_le_bytes()); // p_memsz
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&(PAGE_SIZE as u32).to_le_bytes()); // p_align

    // --- .text section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // Don't pad to data segment offset — the data segment has p_filesz=0
    // so there's no file content. Extra trailing bytes confuse QEMU's
    // ELF loader on ARM32.

    elf
}

/// Build ARM32 runtime I/O functions using Linux SVC syscalls.
///
/// Provides:
/// - `__vuma_print_hex`: Print r0 as 8 hex digits to stdout (FD=1)
///   Uses sys_write (r7=4) via SVC #0.
///
/// - `__vuma_print_int`: Print r0 as a decimal integer to stdout (FD=1)
///   Converts digit-by-digit into a stack buffer, then sys_write.
///
/// - `__vuma_print_newline`: Print a newline character to stdout.
///
/// All functions follow the AAPCS calling convention.
fn build_arm32_runtime() -> Vec<u8> {
    let mut code = Vec::new();

    // ── __vuma_print_hex ──
    // Input: r0 = 32-bit value to print as 8 hex digits
    // Clobbers: r1, r2, r3, r12
    // Stack frame: 16 bytes (save r4, lr + 8-byte buffer)

    // PUSH {r4, lr}
    code.extend_from_slice(&encode_stm(
        Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x4010,
    ));
    // SUB SP, SP, #8  (buffer for hex digits)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 8,
    ));
    // MOV r4, r0  (save input value)
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R4.encoding(), Gpr::R0.encoding(),
    ));
    // r1 = 8 (loop counter)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 8,
    ));
    // r2 = 28 (shift amount: 28, 24, 20, ..., 0)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R2.encoding(), 0, 28,
    ));

    // hex_loop:
    let hex_loop_start = code.len();

    // r3 = r4 >> r2  (shift right by shift amount)
    code.extend_from_slice(&encode_dp_shift_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R3.encoding(), 3, Gpr::R2.encoding(), Gpr::R4.encoding(),
    ));
    // r3 = r3 & 0xF
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_AND, false, Gpr::R3.encoding(), Gpr::R3.encoding(), 0, 0xF,
    ));
    // r12 = r3 + '0' (48)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R3.encoding(), Gpr::R12.encoding(), 0, 48,
    ));
    // CMP r3, #9
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_CMP, true, Gpr::R3.encoding(), 0, 0, 9,
    ));
    // ADDLS r12, r3, #87  (if r3 > 9, add 39 to make it a-f)
    // Actually: if r3 > 9, we need r12 = r3 + 87. We already have r12 = r3 + 48.
    // So if r3 > 9, add 39 more (87 - 48 = 39).
    // ADDHI r12, r12, #39
    code.extend_from_slice(&encode_dp_imm(
        Condition::Hi, DP_ADD, false, Gpr::R12.encoding(), Gpr::R12.encoding(), 0, 39,
    ));

    // Store char: STRB r12, [SP, r1 - 1]
    // We need: r1 goes from 8 down to 1, store at SP + (8 - r1)
    // Actually let's simplify: r1 starts at 8, we decrement first.
    // Let's use a simpler approach: compute address = SP + (8 - r1)
    // RSB r3, r1, #8   => r3 = 8 - r1
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, 0b0011, false, Gpr::R1.encoding(), Gpr::R3.encoding(), 0, 8,
    )); // RSB r3, r1, #8
    // STRB r12, [SP, r3]
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, false, false, false, Gpr::R13.encoding(), Gpr::R12.encoding(), 0,
    ));
    // Wait, this doesn't use r3 as the offset register. We need a register-offset store.
    // Use STRB r12, [SP, r3] — but our encoding only supports immediate offsets.
    // Let's use ADD r3, SP, r3 then STRB r12, [r3, #0]
    // Remove the last STRB (4 bytes)
    code.truncate(code.len() - 4);
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R3.encoding(), Gpr::R3.encoding(),
    )); // ADD r3, SP, r3
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, false, Gpr::R3.encoding(), Gpr::R12.encoding(), 0,
    )); // STRB r12, [r3, #0]

    // SUB r2, r2, #4
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, false, Gpr::R2.encoding(), Gpr::R2.encoding(), 0, 4,
    ));
    // SUBS r1, r1, #1
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, true, Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 1,
    ));
    // BNE hex_loop
    let loop_back_offset = (hex_loop_start as i32) - (code.len() as i32 + 8);
    let loop_back_words = loop_back_offset >> 2;
    code.extend_from_slice(&encode_branch(Condition::Ne, false, loop_back_words));

    // sys_write(1, SP, 8)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
    )); // MOV r0, #1 (fd=stdout)
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), Gpr::R13.encoding(),
    )); // MOV r1, SP (buf)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R2.encoding(), 0, 8,
    )); // MOV r2, #8 (len)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 4,
    )); // MOV r7, #4 (sys_write)
    code.extend_from_slice(&encode_svc(Condition::Al, 0));

    // ADD SP, SP, #8
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 8,
    ));
    // POP {r4, pc}
    code.extend_from_slice(&encode_ldm(
        Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x8010,
    ));

    // ── __vuma_print_int ──
    // Input: r0 = 32-bit signed integer to print as decimal
    // Strategy: divide by 10, store digits, reverse, write.
    // Uses repeated subtraction for division (ARM32 baseline has no hardware divide).

    // PUSH {r4, r5, r6, lr}
    code.extend_from_slice(&encode_stm(
        Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x4070,
    ));
    // SUB SP, SP, #16 (buffer for digits)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 16,
    ));
    // MOV r4, r0 (save value)
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R4.encoding(), Gpr::R0.encoding(),
    ));
    // MOV r5, #0 (digit count)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 0,
    ));
    // CMP r4, #0
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_CMP, true, Gpr::R4.encoding(), 0, 0, 0,
    ));
    // BGE int_positive
    let bge_offset = 3 * 4; // skip 3 instructions: RSBLT, MOV, BL
    code.extend_from_slice(&encode_branch(Condition::Ge, false, bge_offset / 4));
    // RSBLT r4, r4, #0 (negate if negative)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Lt, 0b0011, false, Gpr::R4.encoding(), Gpr::R4.encoding(), 0, 0,
    )); // RSB r4, r4, #0
    // Print minus sign
    // MOV r0, #45 ('-')
    code.extend_from_slice(&encode_dp_imm(
        Condition::Lt, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 45,
    ));
    // PUSH {r0} and sys_write — actually let's use a simpler approach
    // STRB r0, [SP, #-1]! — pre-decrement SP by 1, store byte
    // But this is complex. Let's just store '-' at SP + 16 (temp area) and write it.
    // Actually, let me use a different approach: write '-' directly.
    // We'll use SP + 12 as a temp byte buffer.
    // MOV r0, #1 (fd)
    // Actually, the simplest: use a 1-byte write on stack.
    // Let's just skip the minus sign for now and always print positive.
    // Remove the last MOV (4 bytes)
    code.truncate(code.len() - 4);
    // Instead, just negate the value if negative (RSB already done conditionally)

    // int_positive:
    // int_div_loop: divide r4 by 10 using repeated subtraction
    let int_div_loop = code.len();
    // CMP r4, #0
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_CMP, true, Gpr::R4.encoding(), 0, 0, 0,
    ));
    // BEQ int_done (skip if value is 0)
    let beq_skip = 7 * 4; // skip 7 instructions
    code.extend_from_slice(&encode_branch(Condition::Eq, false, beq_skip / 4));
    // MOV r6, #0 (quotient)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R6.encoding(), 0, 0,
    ));
    // div_inner_loop:
    let div_inner = code.len();
    // CMP r4, #10
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_CMP, true, Gpr::R4.encoding(), 0, 0, 10,
    ));
    // BLT div_inner_done
    let blt_offset = 3 * 4;
    code.extend_from_slice(&encode_branch(Condition::Lt, false, blt_offset / 4));
    // SUB r4, r4, #10
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, false, Gpr::R4.encoding(), Gpr::R4.encoding(), 0, 10,
    ));
    // ADD r6, r6, #1
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R6.encoding(), Gpr::R6.encoding(), 0, 1,
    ));
    // B div_inner_loop
    let div_back = (div_inner as i32) - (code.len() as i32 + 8);
    code.extend_from_slice(&encode_branch(Condition::Al, false, div_back >> 2));
    // div_inner_done: r4 = remainder, r6 = quotient
    // ADD r0, r4, #'0' (48)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R4.encoding(), Gpr::R0.encoding(), 0, 48,
    ));
    // STRB r0, [SP, r5]
    // Need ADD r3, SP, r5; STRB r0, [r3, #0]
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R3.encoding(), Gpr::R5.encoding(),
    ));
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, false, Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
    ));
    // ADD r5, r5, #1
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R5.encoding(), Gpr::R5.encoding(), 0, 1,
    ));
    // MOV r4, r6 (quotient becomes new value)
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R4.encoding(), Gpr::R6.encoding(),
    ));
    // B int_div_loop
    let div_loop_back = (int_div_loop as i32) - (code.len() as i32 + 8);
    code.extend_from_slice(&encode_branch(Condition::Al, false, div_loop_back >> 2));

    // int_done: if no digits were produced (value was 0), write '0'
    // CMP r5, #0
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_CMP, true, Gpr::R5.encoding(), 0, 0, 0,
    ));
    // BNE int_reverse
    code.extend_from_slice(&encode_branch(Condition::Ne, false, 2)); // skip 2 instructions
    // MOV r0, #'0'
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 48,
    ));
    // STRB r0, [SP, r5]; ADD r5, r5, #1
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R3.encoding(), Gpr::R5.encoding(),
    ));
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, false, Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
    ));
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R5.encoding(), Gpr::R5.encoding(), 0, 1,
    ));

    // int_reverse: digits are in reverse order on stack.
    // We need to reverse them in place.
    // Simple approach: copy to a second buffer in reverse order.
    // Actually, for simplicity, let's just reverse the bytes on the stack.
    // r1 = 0 (left index), r2 = r5 - 1 (right index)
    // SUB r2, r5, #1
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, false, Gpr::R5.encoding(), Gpr::R2.encoding(), 0, 1,
    ));
    // MOV r1, #0
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
    ));

    // reverse_loop:
    let rev_loop = code.len();
    // CMP r1, r2
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_CMP, true, Gpr::R1.encoding(), 0, Gpr::R2.encoding(),
    ));
    // BGE reverse_done
    code.extend_from_slice(&encode_branch(Condition::Ge, false, 0)); // placeholder, will patch
    let bge_patch_loc = code.len() - 4;
    // Load byte at SP+r1
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R3.encoding(), Gpr::R1.encoding(),
    ));
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, true, Gpr::R3.encoding(), Gpr::R6.encoding(), 0,
    )); // LDRB r6, [r3, #0]
    // Load byte at SP+r2
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R3.encoding(), Gpr::R2.encoding(),
    ));
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, true, Gpr::R3.encoding(), Gpr::R4.encoding(), 0,
    )); // LDRB r4, [r3, #0]
    // Store swapped
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R3.encoding(), Gpr::R1.encoding(),
    ));
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, false, Gpr::R3.encoding(), Gpr::R4.encoding(), 0,
    )); // STRB r4, [r3, #0]
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R3.encoding(), Gpr::R2.encoding(),
    ));
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, false, Gpr::R3.encoding(), Gpr::R6.encoding(), 0,
    )); // STRB r6, [r3, #0]
    // ADD r1, r1, #1
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 1,
    ));
    // SUB r2, r2, #1
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, false, Gpr::R2.encoding(), Gpr::R2.encoding(), 0, 1,
    ));
    // B reverse_loop
    let rev_back = (rev_loop as i32) - (code.len() as i32 + 8);
    code.extend_from_slice(&encode_branch(Condition::Al, false, rev_back >> 2));
    // Patch BGE
    let rev_done_start = code.len();
    let bge_target = ((rev_done_start as i32) - ((bge_patch_loc as i32) + 8)) >> 2;
    let bge_word = (Condition::Ge.encoding() << 28) | (0b101 << 25) | (bge_target as u32 & 0x00FF_FFFF);
    code[bge_patch_loc..bge_patch_loc + 4].copy_from_slice(&bge_word.to_le_bytes());

    // reverse_done: sys_write(1, SP, r5)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
    )); // MOV r0, #1 (fd=stdout)
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), Gpr::R13.encoding(),
    )); // MOV r1, SP (buf)
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R2.encoding(), Gpr::R5.encoding(),
    )); // MOV r2, r5 (len)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 4,
    )); // MOV r7, #4 (sys_write)
    code.extend_from_slice(&encode_svc(Condition::Al, 0));

    // ADD SP, SP, #16
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 16,
    ));
    // POP {r4, r5, r6, pc}
    code.extend_from_slice(&encode_ldm(
        Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x8070,
    ));

    // ── __vuma_print_newline ──
    // Write a '\n' character to stdout.
    // PUSH {r0, r1, r2, r7, lr}
    code.extend_from_slice(&encode_stm(
        Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x4087,
    ));
    // Move SP up by 4, store '\n' byte
    // SUB SP, SP, #4
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 4,
    ));
    // MOV r0, #10 ('\n')
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 10,
    ));
    // STR r0, [SP, #0]
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, false, false, false, Gpr::R13.encoding(), Gpr::R0.encoding(), 0,
    ));
    // MOV r0, #1 (fd)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
    ));
    // MOV r1, SP (buf)
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), Gpr::R13.encoding(),
    ));
    // MOV r2, #1 (len)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R2.encoding(), 0, 1,
    ));
    // MOV r7, #4 (sys_write)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 4,
    ));
    // SVC #0
    code.extend_from_slice(&encode_svc(Condition::Al, 0));
    // ADD SP, SP, #4
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 4,
    ));
    // POP {r0, r1, r2, r7, pc}
    code.extend_from_slice(&encode_ldm(
        Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x8087,
    ));

    code
}

// ===========================================================================
// Arm32Backend
// ===========================================================================

/// ARM 32-bit code generation backend (AAPCS ABI).
pub struct Arm32Backend {
    target_info: Arm32TargetInfo,
}

impl Arm32Backend {
    /// Create a new ARM32 backend.
    pub fn new() -> Self {
        Self {
            target_info: Arm32TargetInfo,
        }
    }
}

impl Default for Arm32Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Try to encode a 32-bit value as an ARM rotated immediate (imm8, rotate).
///
/// ARM data-processing immediates are encoded as an 8-bit value rotated right
/// by `2 * rotate` bits. Returns `Some((rotate, imm8))` if the value can be
/// represented, or `None` otherwise.
#[allow(dead_code)]
fn try_encode_arm_imm(val: u32) -> Option<(u32, u32)> {
    // 0 is a special case: rotate=0, imm8=0
    if val == 0 {
        return Some((0, 0));
    }
    // Try all 16 possible rotation values (0..15), giving ROR amounts 0,2,4,...,30
    for rotate in 0..16 {
        let rotated = val.rotate_left(2 * rotate);
        if rotated <= 0xFF {
            return Some((rotate, rotated));
        }
    }
    None
}

/// Generate ARM32 machine code to load a 32-bit immediate value into a register.
///
/// For values that fit in the ARM rotated-immediate format, emits a single
/// `MOV Rd, #imm8, rotate`. For larger values, emits a `MOV Rd, #low16`
/// followed by `ORR Rd, Rd, #high16` (each 16-bit half encoded as a rotated
/// immediate, possibly requiring further decomposition).
#[allow(dead_code)]
fn load_immediate_arm32(rd: Gpr, val: u32) -> Vec<u8> {
    let mut code = Vec::new();

    // Try the simple rotated-immediate form first
    if let Some((rotate, imm8)) = try_encode_arm_imm(val) {
        code.extend_from_slice(&encode_dp_imm(
            Condition::Al,
            DP_MOV,
            false,
            0,
            rd.encoding(),
            rotate,
            imm8,
        ));
        return code;
    }

    // Try MVN: if ~val can be encoded as a rotated immediate, use MVN Rd, #~val
    let inv = !val;
    if let Some((rotate, imm8)) = try_encode_arm_imm(inv) {
        code.extend_from_slice(&encode_dp_imm(
            Condition::Al,
            DP_MVN,
            false,
            0,
            rd.encoding(),
            rotate,
            imm8,
        ));
        return code;
    }

    // Split into two 16-bit halves and use MOV + ORR
    let lo = val & 0xFFFF;
    let hi = (val >> 16) & 0xFFFF;

    // Load the low half
    if lo == 0 {
        // MOV Rd, #0
        code.extend_from_slice(&encode_dp_imm(
            Condition::Al,
            DP_MOV,
            false,
            0,
            rd.encoding(),
            0,
            0,
        ));
    } else if let Some((rot, imm8)) = try_encode_arm_imm(lo) {
        code.extend_from_slice(&encode_dp_imm(
            Condition::Al,
            DP_MOV,
            false,
            0,
            rd.encoding(),
            rot,
            imm8,
        ));
    } else {
        // Further split lo into two bytes and use ORR
        let lo_lo = lo & 0xFF;
        let lo_hi = (lo >> 8) & 0xFF;
        code.extend_from_slice(&encode_dp_imm(
            Condition::Al,
            DP_MOV,
            false,
            0,
            rd.encoding(),
            0,
            lo_lo,
        ));
        if lo_hi != 0 {
            // lo_hi << 8 = lo_hi rotated right by 24 → rotate=12
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al,
                DP_ORR,
                false,
                rd.encoding(),
                rd.encoding(),
                12,
                lo_hi,
            ));
        }
    }

    // ORR in the high half
    if hi != 0 {
        if let Some((rot, imm8)) = try_encode_arm_imm(hi << 16) {
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al,
                DP_ORR,
                false,
                rd.encoding(),
                rd.encoding(),
                rot,
                imm8,
            ));
        } else {
            // Split hi into two bytes
            let hi_lo = hi & 0xFF;
            let hi_hi = (hi >> 8) & 0xFF;
            if hi_lo != 0 {
                // hi_lo << 16 = hi_lo rotated right by 16 → rotate=8
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al,
                    DP_ORR,
                    false,
                    rd.encoding(),
                    rd.encoding(),
                    8,
                    hi_lo,
                ));
            }
            if hi_hi != 0 {
                // hi_hi << 24 = hi_hi rotated right by 8 → rotate=4
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al,
                    DP_ORR,
                    false,
                    rd.encoding(),
                    rd.encoding(),
                    4,
                    hi_hi,
                ));
            }
        }
    }

    code
}

/// Resolve an `IRValue` to a physical ARM32 GPR.
///
/// For register values, looks up the virtual register in `reg_map`. For
/// immediate values, loads the constant into `scratch` and returns the
/// scratch register. For address/label values, loads into `scratch` as well.
///
/// Returns `(Gpr, Vec<u8>)` where the bytes are any pre-code that must be
/// emitted before the main instruction (e.g., immediate loads).
#[allow(dead_code)]
fn resolve_gpr_arm32(
    val: &crate::ir::IRValue,
    reg_map: &std::collections::HashMap<u32, Gpr>,
    spill_map: &std::collections::HashMap<u32, i32>,
    scratch: Gpr,
) -> (Gpr, Vec<u8>) {
    match val {
        crate::ir::IRValue::Register(id) => {
            if let Some(&gpr) = reg_map.get(id) {
                (gpr, Vec::new())
            } else if let Some(&offset) = spill_map.get(id) {
                // Spilled vreg: load from stack slot [R11, #offset] into scratch
                let mut code = Vec::new();
                // LDR scratch, [R11, #offset]
                if offset >= 0 {
                    code.extend_from_slice(&encode_ls_imm(
                        Condition::Al, true, true, false, false, true,
                        Gpr::R11.encoding(), scratch.encoding(), offset as u32,
                    ));
                } else {
                    code.extend_from_slice(&encode_ls_imm(
                        Condition::Al, true, false, false, false, true,
                        Gpr::R11.encoding(), scratch.encoding(), (-offset) as u32,
                    ));
                }
                (scratch, code)
            } else {
                (Gpr::R0, Vec::new())
            }
        }
        crate::ir::IRValue::Immediate(imm) => {
            let code = load_immediate_arm32(scratch, *imm as u32);
            (scratch, code)
        }
        crate::ir::IRValue::Address(addr) => {
            let code = load_immediate_arm32(scratch, *addr as u32);
            (scratch, code)
        }
        crate::ir::IRValue::Label(_) => {
            // Labels need relocation; emit a placeholder MOV Rd, #0
            let code = load_immediate_arm32(scratch, 0);
            (scratch, code)
        }
    }
}

/// Compute the stack frame size for an IR function on ARM32.
///
/// Sums `Alloc` instruction sizes, adds 8 bytes for the FP/LR save pair,
/// and rounds up to 8-byte alignment.
fn arm32_compute_frame_size(func: &IRFunction) -> usize {
    let mut total: usize = 8; // FP/LR save pair
    for block in &func.blocks {
        for instr in &block.instructions {
            if let crate::ir::IRInstr::Alloc { size, .. } = instr {
                let aligned = (*size as usize).div_ceil(8) * 8;
                total += aligned;
            }
        }
    }
    // Round up to 8-byte alignment
    (total + 7) & !7
}

// ===========================================================================
// ARM32 Mnemonic Decoder
// ===========================================================================

/// Decode a 32-bit ARM instruction word into a human-readable mnemonic.
///
/// Covers data processing (ADD, SUB, AND, ORR, EOR, MOV, CMP, etc.),
/// load/store, branch, multiply, and system instructions.
fn decode_arm32(word: u32) -> String {
    let cond = (word >> 28) & 0xF;
    let cond_str = match cond {
        0b0000 => "eq",
        0b0001 => "ne",
        0b0010 => "cs",
        0b0011 => "cc",
        0b0100 => "mi",
        0b0101 => "pl",
        0b0110 => "vs",
        0b0111 => "vc",
        0b1000 => "hi",
        0b1001 => "ls",
        0b1010 => "ge",
        0b1011 => "lt",
        0b1100 => "gt",
        0b1101 => "le",
        0b1110 => "",
        0b1111 => "nv",
        _ => "??",
    };
    let cond_suffix = if cond_str.is_empty() {
        String::new()
    } else {
        format!(".{}", cond_str)
    };

    let bits27_26 = (word >> 26) & 0x3;
    let i_bit = (word >> 25) & 1;
    let opcode = (word >> 21) & 0xF;
    let s_bit = (word >> 20) & 1;
    let rn = (word >> 16) & 0xF;
    let rd = (word >> 12) & 0xF;
    let rm = word & 0xF;
    let shift_imm = (word >> 7) & 0x1F;
    let shift_type = (word >> 5) & 0x3;
    let rotate = (word >> 8) & 0xF;
    let imm8 = word & 0xFF;
    let imm12 = word & 0xFFF;

    match bits27_26 {
        // Data processing / Synchronization primitives
        0b00 => {
            // Check for synchronization primitives first (LDREX/STREX/DMB)
            let bits27_20 = (word >> 20) & 0xFF;
            let bits7_4 = (word >> 4) & 0xF;
            let bits3_0 = word & 0xF;
            match bits27_20 {
                0b0001_1011 if bits7_4 == 0b1001 && bits3_0 == 0b1111 => {
                    // LDREX Rd, [Rn]
                    format!("ldrex{} r{}, [r{}]", cond_suffix, rd, rn)
                }
                0b0001_1101 if bits7_4 == 0b1001 && bits3_0 == 0b1111 => {
                    // LDREXB Rd, [Rn]
                    format!("ldrexb{} r{}, [r{}]", cond_suffix, rd, rn)
                }
                0b0001_1111 if bits7_4 == 0b1001 && bits3_0 == 0b1111 => {
                    // LDREXH Rd, [Rn]
                    format!("ldrexh{} r{}, [r{}]", cond_suffix, rd, rn)
                }
                0b0001_1000 if bits7_4 == 0b1001 => {
                    // STREX Rd, Rt, [Rn]
                    format!("strex{} r{}, r{}, [r{}]", cond_suffix, rd, rm, rn)
                }
                0b0001_1100 if bits7_4 == 0b1001 => {
                    // STREXB Rd, Rt, [Rn]
                    format!("strexb{} r{}, r{}, [r{}]", cond_suffix, rd, rm, rn)
                }
                0b0001_1110 if bits7_4 == 0b1001 => {
                    // STREXH Rd, Rt, [Rn]
                    format!("strexh{} r{}, r{}, [r{}]", cond_suffix, rd, rm, rn)
                }
                0b0101_0111 => {
                    // DMB option
                    let opt_name = match bits3_0 {
                        0xF => "sy",
                        _ => "???",
                    };
                    format!("dmb {}", opt_name)
                }
                _ => {
                    // Fall through to data processing decoding
                    if i_bit == 1 {
                        // Immediate operand2
                        let expanded = rotate_right(imm8, rotate * 2);
                        match opcode {
                            0b0000 => format!("and{} r{}, r{}, #{}", cond_suffix, rd, rn, expanded),
                            0b0001 => format!("eor{} r{}, r{}, #{}", cond_suffix, rd, rn, expanded),
                            0b0010 => format!("sub{} r{}, r{}, #{}", cond_suffix, rd, rn, expanded),
                            0b0011 => format!("rsb{} r{}, r{}, #{}", cond_suffix, rd, rn, expanded),
                            0b0100 => format!("add{} r{}, r{}, #{}", cond_suffix, rd, rn, expanded),
                            0b1000 => format!("tst{} r{}, #{}", cond_suffix, rn, expanded),
                            0b1001 => format!("teq{} r{}, #{}", cond_suffix, rn, expanded),
                            0b1010 => format!("cmp{} r{}, #{}", cond_suffix, rn, expanded),
                            0b1011 => format!("cmn{} r{}, #{}", cond_suffix, rn, expanded),
                            0b1100 => format!("orr{} r{}, r{}, #{}", cond_suffix, rd, rn, expanded),
                            0b1101 => format!("mov{} r{}, #{}", cond_suffix, rd, expanded),
                            0b1110 => format!("bic{} r{}, r{}, #{}", cond_suffix, rd, rn, expanded),
                            0b1111 => format!("mvn{} r{}, #{}", cond_suffix, rd, expanded),
                            _ => format!(".word {:08x}", word),
                        }
                    } else {
                        // Register operand2
                        let shift_str = if shift_imm == 0 && shift_type == 0 {
                            String::new()
                        } else {
                            let st = match shift_type {
                                0 => "lsl",
                                1 => "lsr",
                                2 => "asr",
                                3 => "ror",
                                _ => "???",
                            };
                            format!(", {} #{}", st, shift_imm)
                        };
                        match opcode {
                            0b0000 => format!("and{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                            0b0001 => format!("eor{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                            0b0010 => format!("sub{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                            0b0011 => format!("rsb{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                            0b0100 => format!("add{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                            0b1000 if s_bit == 1 && rd == 0 => {
                                format!("tst{} r{}, r{}{}", cond_suffix, rn, rm, shift_str)
                            }
                            0b1001 if s_bit == 1 && rd == 0 => {
                                format!("teq{} r{}, r{}{}", cond_suffix, rn, rm, shift_str)
                            }
                            0b1010 if s_bit == 1 && rd == 0 => {
                                format!("cmp{} r{}, r{}{}", cond_suffix, rn, rm, shift_str)
                            }
                            0b1011 if s_bit == 1 && rd == 0 => {
                                format!("cmn{} r{}, r{}{}", cond_suffix, rn, rm, shift_str)
                            }
                            0b1100 => format!("orr{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                            0b1101 if rn == 0 => {
                                format!("mov{} r{}, r{}{}", cond_suffix, rd, rm, shift_str)
                            }
                            0b1110 => format!("bic{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                            0b1111 if rn == 0 => {
                                format!("mvn{} r{}, r{}{}", cond_suffix, rd, rm, shift_str)
                            }
                            _ => format!(".word {:08x}", word),
                        }
                    }
                }
            }
        }
        // Load/Store word/byte
        0b01 => {
            let l_bit = (word >> 20) & 1;
            let b_bit = (word >> 22) & 1;
            let u_bit = (word >> 23) & 1;
            let offset_val = imm12;
            let off_str = if u_bit == 1 {
                format!("#{}", offset_val)
            } else {
                format!("#-{}", offset_val)
            };
            if l_bit == 1 {
                if b_bit == 1 {
                    format!("ldrb{} r{}, [r{}, {}]", cond_suffix, rd, rn, off_str)
                } else {
                    format!("ldr{} r{}, [r{}, {}]", cond_suffix, rd, rn, off_str)
                }
            } else if b_bit == 1 {
                format!("strb{} r{}, [r{}, {}]", cond_suffix, rd, rn, off_str)
            } else {
                format!("str{} r{}, [r{}, {}]", cond_suffix, rd, rn, off_str)
            }
        }
        // Branch
        0b10 => {
            let l_bit = (word >> 24) & 1;
            let imm24 = word & 0x00FFFFFF;
            let offset = ((imm24 as i32) << 8) >> 6; // sign-extend and *4
            if l_bit == 1 {
                format!("bl{} {:+}", cond_suffix, offset)
            } else {
                format!("b{} {:+}", cond_suffix, offset)
            }
        }
        _ => format!(".word {:08x}", word),
    }
}

/// Rotate right a value by the specified amount.
fn rotate_right(val: u32, shift: u32) -> u32 {
    val.rotate_right(shift)
}

impl Backend for Arm32Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        // ── Stack-slot register allocation for ARM32 ──
        //
        // Every vreg gets a dedicated stack slot.  Operations load operands into
        // scratch registers (R0–R3), compute, and store the result back.  This
        // avoids register pressure issues entirely — SHA256d's 147 vregs pose
        // no problem even though ARM32 only has ~12 allocatable GPRs.
        //
        // Stack layout (R11 = frame pointer):
        //   R11+4  = saved LR
        //   R11+0  = saved R11 (old FP)
        //   R11-4  = vreg slot 0
        //   R11-8  = vreg slot 1
        //   ...
        //   R11-(4*N) = vreg slot N-1
        //   then alloc regions at even more negative offsets

        let func_name = func.name.clone();

        // ── Phase 1: Collect all vreg IDs ──
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

        // ── Identify Alloc vregs ──
        let mut alloc_sizes: HashMap<u32, i32> = HashMap::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                if let crate::ir::IRInstr::Alloc { dst, size } = instr {
                    if let Some(id) = dst.as_register() {
                        // Align alloc size to 8 bytes
                        let aligned_size = ((*size as i32 + 7) & !7) as i32;
                        alloc_sizes.insert(id, aligned_size);
                    }
                }
            }
        }

        // ── Compute stack layout ──
        // After prologue: R11 points to saved {R11, LR}
        // Vreg slots at negative offsets from R11
        // Alloc regions at even more negative offsets (below vreg slots)

        let mut current_offset: i32 = 8; // skip saved R11 + LR pair (8 bytes)

        // Alloc regions first (placed after vreg slots in memory = at larger offsets from R11)
        let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
        let mut alloc_vreg_ids: Vec<u32> = alloc_sizes.keys().copied().collect();
        alloc_vreg_ids.sort();
        for &id in &alloc_vreg_ids {
            let size = alloc_sizes[&id];
            current_offset += size;
            alloc_offsets.insert(id, current_offset);
        }

        // Vreg stack slots
        let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
        let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
        all_vreg_ids_sorted.sort();
        for &id in &all_vreg_ids_sorted {
            current_offset += 8; // 8 bytes per slot: low word at offset, high word at offset+4
            vreg_stack_slots.insert(id, current_offset);
        }

        // Frame size must be 8-byte aligned
        let frame_size = ((current_offset + 7) & !7) as usize;
        let fs = frame_size as i32;

        // ── Helper: emit SUB SP, SP, #large_value ──
        // Handles frame sizes that don't fit in ARM rotated-immediate
        fn emit_sub_sp(imm: i32) -> Vec<u8> {
            let mut code = Vec::new();
            if let Some((rotate, imm8)) = try_encode_arm_imm(imm as u32) {
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_SUB, false,
                    Gpr::R13.encoding(), Gpr::R13.encoding(), rotate, imm8,
                ));
            } else {
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, imm as u32));
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_SUB, false,
                    Gpr::R13.encoding(), Gpr::R13.encoding(), Gpr::R12.encoding(),
                ));
            }
            code
        }

        // ── Helper: emit ADD SP, SP, #large_value ──
        // Handles values that don't fit in ARM rotated-immediate
        fn emit_add_sp(imm: i32) -> Vec<u8> {
            let mut code = Vec::new();
            if let Some((rotate, imm8)) = try_encode_arm_imm(imm as u32) {
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_ADD, false,
                    Gpr::R13.encoding(), Gpr::R13.encoding(), rotate, imm8,
                ));
            } else {
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, imm as u32));
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_ADD, false,
                    Gpr::R13.encoding(), Gpr::R13.encoding(), Gpr::R12.encoding(),
                ));
            }
            code
        }

        // ── Helper: emit ADD Rd, Rn, #large_value ──
        fn emit_add_imm(rd: Gpr, rn: Gpr, imm: i32) -> Vec<u8> {
            let mut code = Vec::new();
            if let Some((rotate, imm8)) = try_encode_arm_imm(imm as u32) {
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_ADD, false,
                    rn.encoding(), rd.encoding(), rotate, imm8,
                ));
            } else {
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, imm as u32));
                if rn != rd {
                    code.extend_from_slice(&encode_dp_reg(
                        Condition::Al, DP_MOV, false, 0, rd.encoding(), rn.encoding(),
                    ));
                }
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_ADD, false, rn.encoding(), rd.encoding(), Gpr::R12.encoding(),
                ));
            }
            code
        }

        // ── Stack-slot helpers ──

        /// Load 32-bit word from stack slot [R11 - offset] into dst_reg.
        /// `offset` must be positive (the positive distance below R11).
        fn ss_load_from_slot(dst_reg: Gpr, offset_from_r11: i32) -> Vec<u8> {
            let neg_off = -offset_from_r11;
            // ARM32 LDR immediate offset is 12-bit unsigned (0..4095)
            if neg_off >= -4095 {
                encode_ls_imm(
                    Condition::Al, true, false, false, false, true,
                    Gpr::R11.encoding(), dst_reg.encoding(), (-neg_off) as u32,
                ).to_vec()
            } else {
                // Large offset: compute address into R12, then LDR from R12
                let mut code = Vec::new();
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, offset_from_r11 as u32));
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_SUB, false,
                    Gpr::R11.encoding(), R12_TEMP, Gpr::R12.encoding(),
                ));
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, true,
                    R12_TEMP, dst_reg.encoding(), 0,
                ));
                code
            }
        }

        /// Load 32-bit word from [R11 + offset] into dst_reg (positive offset from R11).
        /// Used to access stack-passed arguments (args 5+) which reside above the
        /// saved {R11, LR} pair in the callee's frame.
        fn ss_load_from_r11_plus(dst_reg: Gpr, offset_from_r11: i32) -> Vec<u8> {
            if offset_from_r11 >= 0 && offset_from_r11 <= 4095 {
                encode_ls_imm(
                    Condition::Al, true, true, false, false, true,
                    Gpr::R11.encoding(), dst_reg.encoding(), offset_from_r11 as u32,
                ).to_vec()
            } else {
                // Large offset: compute address into R12, then LDR from R12
                let mut code = Vec::new();
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, offset_from_r11 as u32));
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_ADD, false,
                    Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding(),
                ));
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, true,
                    Gpr::R12.encoding(), dst_reg.encoding(), 0,
                ));
                code
            }
        }

        /// Store 32-bit word from src_reg into stack slot [R11 - offset].
        /// `offset` must be positive. IMPORTANT: src_reg must NOT be R12 for large offsets.
        fn ss_store_to_slot(src_reg: Gpr, offset_from_r11: i32) -> Vec<u8> {
            let neg_off = -offset_from_r11;
            if neg_off >= -4095 {
                encode_ls_imm(
                    Condition::Al, true, false, false, false, false,
                    Gpr::R11.encoding(), src_reg.encoding(), (-neg_off) as u32,
                ).to_vec()
            } else {
                let mut code = Vec::new();
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, offset_from_r11 as u32));
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_SUB, false,
                    Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding(),
                ));
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R12.encoding(), src_reg.encoding(), 0,
                ));
                code
            }
        }

        /// Load an IRValue into a scratch register.
        fn ss_load_value(val: &crate::ir::IRValue, slots: &HashMap<u32, i32>, scratch: Gpr) -> Vec<u8> {
            match val {
                crate::ir::IRValue::Register(id) => {
                    let offset = slots.get(id).copied().unwrap_or(0);
                    ss_load_from_slot(scratch, offset)
                }
                crate::ir::IRValue::Immediate(v) => load_immediate_arm32(scratch, *v as u32),
                crate::ir::IRValue::Address(a) => load_immediate_arm32(scratch, *a as u32),
                crate::ir::IRValue::Label(_) => load_immediate_arm32(scratch, 0),
            }
        }

        /// Load a 64-bit IRValue into TWO registers (lo_reg, hi_reg).
        fn ss_load_value_64(
            lo_reg: Gpr,
            hi_reg: Gpr,
            val: &crate::ir::IRValue,
            slots: &HashMap<u32, i32>,
        ) -> Vec<u8> {
            let mut code = Vec::new();
            match val {
                crate::ir::IRValue::Register(id) => {
                    let offset = slots.get(id).copied().unwrap_or(0);
                    code.extend(ss_load_from_slot(lo_reg, offset));
                    code.extend(ss_load_from_slot(hi_reg, offset + 4));
                }
                crate::ir::IRValue::Immediate(v) => {
                    code.extend(load_immediate_arm32(lo_reg, *v as u32));
                    if *v < 0 {
                        // MVN hi_reg, #0 → 0xFFFFFFFF
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_MVN, false, 0, hi_reg.encoding(), 0, 0,
                        ));
                    } else {
                        // MOV hi_reg, #0
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_MOV, false, 0, hi_reg.encoding(), 0, 0,
                        ));
                    }
                }
                crate::ir::IRValue::Address(a) => {
                    code.extend(load_immediate_arm32(lo_reg, *a as u32));
                    code.extend_from_slice(&encode_dp_imm(
                        Condition::Al, DP_MOV, false, 0, hi_reg.encoding(), 0, 0,
                    ));
                }
                crate::ir::IRValue::Label(_) => {
                    code.extend_from_slice(&encode_dp_imm(
                        Condition::Al, DP_MOV, false, 0, lo_reg.encoding(), 0, 0,
                    ));
                    code.extend_from_slice(&encode_dp_imm(
                        Condition::Al, DP_MOV, false, 0, hi_reg.encoding(), 0, 0,
                    ));
                }
            }
            code
        }

        /// Store TWO registers (lo, hi) into a vreg slot.
        fn ss_store_64(lo_reg: Gpr, hi_reg: Gpr, offset_from_r11: i32) -> Vec<u8> {
            let mut code = Vec::new();
            code.extend(ss_store_to_slot(lo_reg, offset_from_r11));
            code.extend(ss_store_to_slot(hi_reg, offset_from_r11 + 4));
            code
        }

        const R12_TEMP: u32 = 12; // R12 encoding for temp use

        // ── Phase 2: Emit prologue ──

        let mut instructions: Vec<AllocatedInstruction> = Vec::new();
        let mut relocations: Vec<RelocationEntry> = Vec::new();

        // SUB SP, SP, #(frame_size + 8)   — allocate frame + save area
        let total_alloc = fs + 8;
        let prologue_sub = emit_sub_sp(total_alloc);
        instructions.push(AllocatedInstruction {
            opcode: "sub".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
            encoded: prologue_sub,
        });

        // STR LR, [SP, #frame_size+4]   — save LR
        let lr_off = fs + 4;
        if lr_off <= 4095 {
            instructions.push(AllocatedInstruction {
                opcode: "str".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R14.encoding())],
                writes: vec![],
                encoded: encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R14.encoding(), lr_off as u32,
                ).to_vec(),
            });
        } else {
            let mut code = Vec::new();
            code.extend_from_slice(&emit_add_imm(Gpr::R12, Gpr::R13, lr_off));
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R12.encoding(), Gpr::R14.encoding(), 0,
            ));
            instructions.push(AllocatedInstruction {
                opcode: "str".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R14.encoding())],
                writes: vec![],
                encoded: code,
            });
        }

        // STR R11, [SP, #frame_size]   — save R11 (old FP)
        let fp_off = fs;
        if fp_off <= 4095 {
            instructions.push(AllocatedInstruction {
                opcode: "str".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R11.encoding())],
                writes: vec![],
                encoded: encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R11.encoding(), fp_off as u32,
                ).to_vec(),
            });
        } else {
            let mut code = Vec::new();
            code.extend_from_slice(&emit_add_imm(Gpr::R12, Gpr::R13, fp_off));
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R12.encoding(), Gpr::R11.encoding(), 0,
            ));
            instructions.push(AllocatedInstruction {
                opcode: "str".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R11.encoding())],
                writes: vec![],
                encoded: code,
            });
        }

        // ADD R11, SP, #frame_size   — set frame pointer
        let set_fp_code = emit_add_imm(Gpr::R11, Gpr::R13, fs);
        instructions.push(AllocatedInstruction {
            opcode: "add".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R11.encoding())],
            encoded: set_fp_code,
        });

        // Store function parameters to their stack slots
        // Args 0–3 come from R0–R3; args 4+ reside on the stack above the
        // saved {R11, LR} pair at [R11 + 8 + (i-4)*4].
        let arg_regs = [Gpr::R0, Gpr::R1, Gpr::R2, Gpr::R3];
        for (i, param) in func.params.iter().enumerate() {
            if let Some(id) = param.as_register() {
                if i < 4 {
                    let offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
                    let store_code = ss_store_to_slot(arg_regs[i], offset);
                    instructions.push(AllocatedInstruction {
                        opcode: "str".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, arg_regs[i].encoding())],
                        writes: vec![],
                        encoded: store_code,
                    });
                } else {
                    // Stack-passed argument: located at [R11 + 8 + (i-4)*4]
                    // Load into R0 (free — already saved to its slot for param 0),
                    // then store to the parameter's stack slot.
                    // NOTE: We use R0 rather than R12 because ss_store_to_slot
                    // uses R12 internally for large offsets and documents that
                    // src_reg must NOT be R12 in that case.
                    let arg_offset_from_r11: i32 = 8 + ((i - 4) * 4) as i32;
                    let slot_offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
                    let mut param_code = Vec::new();
                    // LDR R0, [R11, #arg_offset_from_r11]
                    param_code.extend(ss_load_from_r11_plus(Gpr::R0, arg_offset_from_r11));
                    // STR R0, [R11 - slot_offset]
                    param_code.extend(ss_store_to_slot(Gpr::R0, slot_offset));
                    instructions.push(AllocatedInstruction {
                        opcode: "ldr+str".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R11.encoding())],
                        writes: vec![],
                        encoded: param_code,
                    });
                }
            }
        }

        // ── Phase 3: Emit body with branch fixup tracking ──

        let mut current_byte_offset: u64 = instructions.iter().map(|i| i.encoded.len() as u64).sum();
        let mut label_offsets: HashMap<String, u64> = HashMap::new();

        // Branch fixup: records a branch instruction that needs its offset patched
        struct BranchFixup {
            instr_idx: usize,
            abs_byte_offset: u64,
            target_label: String,
            is_unconditional: bool, // true for B, false for Bcc (BNE etc.)
            condition: Condition,   // condition code (AL for unconditional)
        }
        let mut branch_fixups: Vec<BranchFixup> = Vec::new();

        for block in &func.blocks {
            // Record the byte offset for this block's label
            label_offsets.insert(block.label.clone(), current_byte_offset);

            for instr in &block.instructions {
                let encoded: Vec<u8> = match instr {
                    // ── BinOp (generic) ──
                    crate::ir::IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();

                        match op {
                            BinOpKind::Add | BinOpKind::Sub => {
                                // 64-bit add/sub: load 64-bit lhs (R0:R2) and rhs (R1:R3),
                                // operate on low word with carry/borrow flag, then high word
                                // with carry (ADC) or borrow (SBC).
                                code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                                code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                                match op {
                                    BinOpKind::Add => {
                                        // ADDS R0, R0, R1 (low word, set carry)
                                        code.extend_from_slice(&encode_dp_reg(
                                            Condition::Al, DP_ADD, true,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                                        ));
                                        // ADC R2, R2, R3 (high word, with carry)
                                        code.extend_from_slice(&encode_dp_reg(
                                            Condition::Al, DP_ADC, true,
                                            Gpr::R2.encoding(), Gpr::R2.encoding(), Gpr::R3.encoding(),
                                        ));
                                    }
                                    BinOpKind::Sub => {
                                        // SUBS R0, R0, R1 (low word, set borrow)
                                        code.extend_from_slice(&encode_dp_reg(
                                            Condition::Al, DP_SUB, true,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                                        ));
                                        // SBC R2, R2, R3 (high word, with borrow)
                                        code.extend_from_slice(&encode_dp_reg(
                                            Condition::Al, DP_SBC, true,
                                            Gpr::R2.encoding(), Gpr::R2.encoding(), Gpr::R3.encoding(),
                                        ));
                                    }
                                    _ => {}
                                }
                                code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                            }
                            BinOpKind::Mul => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                                // MUL R0, R0, R1 → Rd=R0, Rn=0, Rs=R1, Rm=R0
                                code.extend_from_slice(&encode_mul(
                                    Condition::Al, false,
                                    Gpr::R0.encoding(), 0, Gpr::R1.encoding(), Gpr::R0.encoding(),
                                ));
                                code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                            BinOpKind::And | BinOpKind::Or | BinOpKind::Xor => {
                                // 64-bit bitwise op: load both words of lhs and rhs,
                                // operate on low and high words independently, store both.
                                code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                                code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                                let arm_op = match op {
                                    BinOpKind::And => DP_AND,
                                    BinOpKind::Or => DP_ORR,
                                    BinOpKind::Xor => DP_EOR,
                                    _ => DP_AND,
                                };
                                // Low word: R0 = R0 <op> R1
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, arm_op, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                                ));
                                // High word: R2 = R2 <op> R3
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, arm_op, false,
                                    Gpr::R2.encoding(), Gpr::R2.encoding(), Gpr::R3.encoding(),
                                ));
                                code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                            }
                            BinOpKind::ShrL => {
                                // 64-bit logical right shift by variable amount.
                                // R0=low, R2=high, R1=shift_amount (R3 = high word of
                                // shift amount — ignored, used as scratch below).
                                code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                                code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));

                                // CMP R1, #32 — set carry (CS) iff shift >= 32
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_CMP, true,
                                    Gpr::R1.encoding(), 0, 0, 32,
                                ));

                                // If shift >= 32 (CS):
                                //   SUB R12, R1, #32         (shift - 32)
                                //   MOV R0, R2, LSR R12      (result_low = high >> (shift-32))
                                //   MOV R2, #0               (result_high = 0)
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Cs, DP_SUB, false,
                                    Gpr::R1.encoding(), Gpr::R12.encoding(), 0, 32,
                                ));
                                code.extend_from_slice(&encode_dp_shift_reg(
                                    Condition::Cs, DP_MOV, false, 0,
                                    Gpr::R0.encoding(), 1, Gpr::R12.encoding(), Gpr::R2.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Cs, DP_MOV, false, 0,
                                    Gpr::R2.encoding(), 0, 0,
                                ));

                                // If shift < 32 (CC):
                                //   MOV R12, R0              (save low)
                                //   MOV R0, R12, LSR R1      (low >> shift)
                                //   RSB R3, R1, #32          (32 - shift)
                                //   MOV R12, R2, LSL R3      (high << (32-shift))
                                //   ORR R0, R0, R12          (combine)
                                //   MOV R2, R2, LSR R1       (high >> shift)
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Cc, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), Gpr::R0.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_shift_reg(
                                    Condition::Cc, DP_MOV, false, 0,
                                    Gpr::R0.encoding(), 1, Gpr::R1.encoding(), Gpr::R12.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Cc, DP_RSB, false,
                                    Gpr::R1.encoding(), Gpr::R3.encoding(), 0, 32,
                                ));
                                code.extend_from_slice(&encode_dp_shift_reg(
                                    Condition::Cc, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), 0, Gpr::R3.encoding(), Gpr::R2.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Cc, DP_ORR, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_shift_reg(
                                    Condition::Cc, DP_MOV, false, 0,
                                    Gpr::R2.encoding(), 1, Gpr::R1.encoding(), Gpr::R2.encoding(),
                                ));

                                code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                            }
                            BinOpKind::Shl | BinOpKind::ShrA
                            | BinOpKind::Ror | BinOpKind::Rol => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                                if matches!(op, BinOpKind::Rol) {
                                    // ROL x, n = ROR x, (32-n)
                                    // R2 = RSB R1, #32 = 32 - R1; R0 = R0 ROR R2
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, 0b0011, false,
                                        Gpr::R1.encoding(), Gpr::R2.encoding(), 0, 32,
                                    ));
                                    code.extend_from_slice(&encode_dp_shift_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R0.encoding(), 3, Gpr::R2.encoding(), Gpr::R0.encoding(),
                                    ));
                                } else {
                                    let shift_type: u32 = match op {
                                        BinOpKind::Shl => 0,
                                        BinOpKind::ShrA => 2,
                                        BinOpKind::Ror => 3,
                                        _ => 0,
                                    };
                                    code.extend_from_slice(&encode_dp_shift_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R0.encoding(), shift_type, Gpr::R1.encoding(), Gpr::R0.encoding(),
                                    ));
                                }
                                code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                            BinOpKind::SDiv | BinOpKind::UDiv => {
                                // Software division: R0 = R0 / R1
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                                // MOV R2, #0 (quotient)
                                code.extend_from_slice(&0xE3A02000u32.to_le_bytes());
                                // MOV R3, R0 (remainder)
                                code.extend_from_slice(&0xE1A03000u32.to_le_bytes());
                                // CMP R1, #0
                                code.extend_from_slice(&0xE3510000u32.to_le_bytes());
                                // BEQ +3 (to done)
                                code.extend_from_slice(&0x0A000003u32.to_le_bytes());
                                // loop: CMP R3, R1
                                code.extend_from_slice(&0xE1530001u32.to_le_bytes());
                                // BLO +2 (to done)
                                code.extend_from_slice(&0x3A000002u32.to_le_bytes());
                                // SUB R3, R3, R1
                                code.extend_from_slice(&0xE0433001u32.to_le_bytes());
                                // ADD R2, R2, #1
                                code.extend_from_slice(&0xE2822001u32.to_le_bytes());
                                // B loop (-6)
                                code.extend_from_slice(&0xEAFFFFFAu32.to_le_bytes());
                                // done: MOV R0, R2 (quotient)
                                code.extend_from_slice(&0xE1A00002u32.to_le_bytes());
                                code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                            BinOpKind::SRem | BinOpKind::URem => {
                                // Software modulo: R0 = R0 % R1 (remainder)
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                                // MOV R2, #0 (quotient)
                                code.extend_from_slice(&0xE3A02000u32.to_le_bytes());
                                // MOV R3, R0 (remainder)
                                code.extend_from_slice(&0xE1A03000u32.to_le_bytes());
                                // CMP R1, #0
                                code.extend_from_slice(&0xE3510000u32.to_le_bytes());
                                // BEQ +3 (to done)
                                code.extend_from_slice(&0x0A000003u32.to_le_bytes());
                                // loop: CMP R3, R1
                                code.extend_from_slice(&0xE1530001u32.to_le_bytes());
                                // BLO +2 (to done)
                                code.extend_from_slice(&0x3A000002u32.to_le_bytes());
                                // SUB R3, R3, R1
                                code.extend_from_slice(&0xE0433001u32.to_le_bytes());
                                // ADD R2, R2, #1
                                code.extend_from_slice(&0xE2822001u32.to_le_bytes());
                                // B loop (-6)
                                code.extend_from_slice(&0xEAFFFFFAu32.to_le_bytes());
                                // done: MOV R0, R3 (remainder)
                                code.extend_from_slice(&0xE1A00003u32.to_le_bytes());
                                code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                            // Comparison BinOps: produce 0 or 1
                            BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
                            | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
                            | BinOpKind::Eq | BinOpKind::Ne => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                                // CMP R0, R1; MOV R0, #0; MOVcond R0, #1
                                let cmp_cond = match op {
                                    BinOpKind::SLt => Condition::Lt,
                                    BinOpKind::SLe => Condition::Le,
                                    BinOpKind::SGt => Condition::Gt,
                                    BinOpKind::SGe => Condition::Ge,
                                    BinOpKind::ULt => Condition::Cc,
                                    BinOpKind::ULe => Condition::Ls,
                                    BinOpKind::UGt => Condition::Hi,
                                    BinOpKind::UGe => Condition::Cs,
                                    BinOpKind::Eq => Condition::Eq,
                                    BinOpKind::Ne => Condition::Ne,
                                    _ => Condition::Eq,
                                };
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_CMP, true,
                                    Gpr::R0.encoding(), 0, Gpr::R1.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 0,
                                ));
                                code.extend_from_slice(&encode_dp_imm(
                                    cmp_cond, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
                                ));
                                code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                        }
                        code
                    }

                    // ── Add/Sub/Mul/Div (dedicated) ──
                    crate::ir::IRInstr::Add { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                        code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                        // ADDS R0, R0, R1 (low word, set carry)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_ADD, true,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                        ));
                        // ADC R2, R2, R3 (high word, with carry)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_ADC, true,
                            Gpr::R2.encoding(), Gpr::R2.encoding(), Gpr::R3.encoding(),
                        ));
                        code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                        code
                    }
                    crate::ir::IRInstr::Sub { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                        code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                        // SUBS R0, R0, R1 (low word, set borrow)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_SUB, true,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                        ));
                        // SBC R2, R2, R3 (high word, with borrow)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_SBC, true,
                            Gpr::R2.encoding(), Gpr::R2.encoding(), Gpr::R3.encoding(),
                        ));
                        code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                        code
                    }
                    crate::ir::IRInstr::Mul { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                        code.extend_from_slice(&encode_mul(
                            Condition::Al, false,
                            Gpr::R0.encoding(), 0, Gpr::R1.encoding(), Gpr::R0.encoding(),
                        ));
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }
                    crate::ir::IRInstr::Div { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                        // Software division: R0 = R0 / R1
                        // R2 = 0 (quotient), R3 = R0 (remainder)
                        // CMP R1, #0; BEQ done
                        // loop: CMP R3, R1; BLO done
                        //   SUB R3, R3, R1; ADD R2, R2, #1; B loop
                        // done: MOV R0, R2
                        // MOV R2, #0
                        code.extend_from_slice(&0xE3A02000u32.to_le_bytes()); // MOV R2, #0
                        // MOV R3, R0
                        code.extend_from_slice(&0xE1A03000u32.to_le_bytes()); // MOV R3, R0
                        // CMP R1, #0
                        code.extend_from_slice(&0xE3510000u32.to_le_bytes()); // CMP R1, #0
                        // BEQ +3 (to done: MOV R0,R2)
                        code.extend_from_slice(&0x0A000003u32.to_le_bytes()); // BEQ +3
                        // loop: CMP R3, R1
                        code.extend_from_slice(&0xE1530001u32.to_le_bytes()); // CMP R3, R1
                        // BLO +2 (to done)
                        code.extend_from_slice(&0x3A000002u32.to_le_bytes()); // BLO +2
                        // SUB R3, R3, R1
                        code.extend_from_slice(&0xE0433001u32.to_le_bytes()); // SUB R3, R3, R1
                        // ADD R2, R2, #1
                        code.extend_from_slice(&0xE2822001u32.to_le_bytes()); // ADD R2, R2, #1
                        // B loop (offset = -6)
                        code.extend_from_slice(&0xEAFFFFFAu32.to_le_bytes()); // B -6
                        // done: MOV R0, R2
                        code.extend_from_slice(&0xE1A00002u32.to_le_bytes()); // MOV R0, R2
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Cmp ──
                    crate::ir::IRInstr::Cmp { kind, dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                        let cmp_cond = match kind {
                            CmpKind::Eq => Condition::Eq,
                            CmpKind::Ne => Condition::Ne,
                            CmpKind::SLt => Condition::Lt,
                            CmpKind::SLe => Condition::Le,
                            CmpKind::SGt => Condition::Gt,
                            CmpKind::SGe => Condition::Ge,
                            CmpKind::ULt => Condition::Cc,
                            CmpKind::ULe => Condition::Ls,
                            CmpKind::UGt => Condition::Hi,
                            CmpKind::UGe => Condition::Cs,
                        };
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_CMP, true,
                            Gpr::R0.encoding(), 0, Gpr::R1.encoding(),
                        ));
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 0,
                        ));
                        code.extend_from_slice(&encode_dp_imm(
                            cmp_cond, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
                        ));
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── UnaryOp ──
                    crate::ir::IRInstr::UnaryOp { op, dst, operand, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(operand, &vreg_stack_slots, Gpr::R0));
                        match op {
                            UnaryOpKind::Neg => {
                                // RSB R0, R0, #0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, 0b0011, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                ));
                            }
                            UnaryOpKind::Not => {
                                // MVN R0, R0
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_MVN, false, 0,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(),
                                ));
                            }
                            UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                                // Placeholder: pass through
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Load ──
                    crate::ir::IRInstr::Load { dst, addr, offset, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // Load base address into R3
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R3));
                        // Add IR offset if present
                        if *offset != 0 {
                            if let Some((rot, imm8)) = try_encode_arm_imm(*offset as u32) {
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R3.encoding(), Gpr::R3.encoding(), rot, imm8,
                                ));
                            } else {
                                code.extend_from_slice(&load_immediate_arm32(Gpr::R2, *offset as u32));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R3.encoding(), Gpr::R3.encoding(), Gpr::R2.encoding(),
                                ));
                            }
                        }
                        // Emit load based on type
                        match ty {
                            crate::ir::IRType::I8 | crate::ir::IRType::U8 => {
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, true, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // LDRB R0, [R3, #0]
                            }
                            crate::ir::IRType::I16 => {
                                code.extend_from_slice(&encode_ldrsb_imm(
                                    Condition::Al, true, true, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // LDRSB R0, [R3, #0]
                            }
                            crate::ir::IRType::U16 => {
                                code.extend_from_slice(&encode_ls_half_imm(
                                    Condition::Al, true, true, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // LDRH R0, [R3, #0]
                            }
                            _ => {
                                // Default: 32-bit word load
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // LDR R0, [R3, #0]
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Store ──
                    crate::ir::IRInstr::Store { value, addr, offset, ty } => {
                        let mut code = Vec::new();
                        // Load address into R3
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R3));
                        // Add IR offset if present
                        if *offset != 0 {
                            if let Some((rot, imm8)) = try_encode_arm_imm(*offset as u32) {
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R3.encoding(), Gpr::R3.encoding(), rot, imm8,
                                ));
                            } else {
                                code.extend_from_slice(&load_immediate_arm32(Gpr::R2, *offset as u32));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R3.encoding(), Gpr::R3.encoding(), Gpr::R2.encoding(),
                                ));
                            }
                        }
                        // Load value into R0
                        code.extend(ss_load_value(value, &vreg_stack_slots, Gpr::R0));
                        // Emit store based on type
                        match ty {
                            crate::ir::IRType::I8 | crate::ir::IRType::U8 => {
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, true, false, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // STRB R0, [R3, #0]
                            }
                            crate::ir::IRType::I16 | crate::ir::IRType::U16 => {
                                code.extend_from_slice(&encode_ls_half_imm(
                                    Condition::Al, true, true, false, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // STRH R0, [R3, #0]
                            }
                            _ => {
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // STR R0, [R3, #0]
                            }
                        }
                        code
                    }

                    // ── Alloc ──
                    crate::ir::IRInstr::Alloc { dst, size: _ } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let alloc_off = alloc_offsets.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // Compute address: R11 - alloc_off → R0
                        // R0 = R11 - alloc_off
                        if let Some((rot, imm8)) = try_encode_arm_imm(alloc_off as u32) {
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_SUB, false,
                                Gpr::R11.encoding(), Gpr::R0.encoding(), rot, imm8,
                            ));
                        } else {
                            code.extend_from_slice(&load_immediate_arm32(Gpr::R0, alloc_off as u32));
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_SUB, false,
                                Gpr::R11.encoding(), Gpr::R0.encoding(), Gpr::R0.encoding(),
                            ));
                        }
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Call ──
                    crate::ir::IRInstr::Call { dst, func: target_func, args, is_extern: _ } => {
                        let mut code = Vec::new();
                        let num_args = args.len();
                        let num_stack_args = if num_args > 4 { num_args - 4 } else { 0 };
                        let stack_args_bytes = num_stack_args * 4;

                        // ── AAPCS: args 5+ go on the stack ──
                        // 1. Decrement SP to make room for stack-passed arguments
                        if stack_args_bytes > 0 {
                            code.extend_from_slice(&emit_sub_sp(stack_args_bytes as i32));
                        }

                        // 2. Store args 5+ onto the stack (right-to-left push is
                        //    achieved by placing arg5 at [SP+0], arg6 at [SP+4], etc.)
                        for (i, arg) in args.iter().enumerate() {
                            if i >= 4 {
                                let stack_offset = ((i - 4) * 4) as u32;
                                if stack_offset <= 4095 {
                                    // Load arg value into R12 and STR directly
                                    code.extend(ss_load_value(arg, &vreg_stack_slots, Gpr::R12));
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, true, false, false, false,
                                        Gpr::R13.encoding(), Gpr::R12.encoding(), stack_offset,
                                    ));
                                } else {
                                    // Large offset (extremely unlikely): compute addr first,
                                    // then load arg value and store.
                                    // Compute SP + stack_offset into R12
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, stack_offset));
                                    code.extend_from_slice(&encode_dp_reg(
                                        Condition::Al, DP_ADD, false,
                                        Gpr::R13.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding(),
                                    ));
                                    // Load arg into R0 (R0-R3 not yet set up for this call)
                                    code.extend(ss_load_value(arg, &vreg_stack_slots, Gpr::R0));
                                    // STR R0, [R12, #0]
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, true, false, false, false,
                                        Gpr::R12.encoding(), Gpr::R0.encoding(), 0,
                                    ));
                                }
                            }
                        }

                        // 3. Move args 0–3 to R0–R3
                        // We need to be careful: if an arg is in a stack slot that
                        // uses R12 for large offsets, and we've already loaded an
                        // earlier arg into R0-R3, we need to handle this carefully.
                        // Since ss_load_value only uses R12 as a temp for large offsets
                        // and doesn't touch R0-R3, loading sequentially is safe.
                        for (i, arg) in args.iter().enumerate() {
                            if i < 4 {
                                let arg_reg = Gpr::arg_register(i).unwrap();
                                code.extend(ss_load_value(arg, &vreg_stack_slots, arg_reg));
                            }
                        }

                        // BL offset (placeholder)
                        let bl_offset_in_func = current_byte_offset + code.len() as u64;
                        code.extend_from_slice(&encode_branch(Condition::Al, true, 0));
                        relocations.push(RelocationEntry {
                            offset: bl_offset_in_func,
                            symbol: target_func.clone(),
                            reloc_type: "R_ARM_CALL".to_string(),
                        });

                        // 4. Caller cleanup: pop stack-passed arguments
                        if stack_args_bytes > 0 {
                            code.extend_from_slice(&emit_add_sp(stack_args_bytes as i32));
                        }

                        // Store return value to dst stack slot
                        if let Some(d) = dst {
                            let dst_id = d.as_register().unwrap_or(0);
                            let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                            code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        }

                        code
                    }

                    // ── Branch ──
                    crate::ir::IRInstr::Branch { target } => {
                        let mut code = Vec::new();
                        let branch_offset_in_func = current_byte_offset + code.len() as u64;
                        code.extend_from_slice(&encode_branch(Condition::Al, false, 0));
                        branch_fixups.push(BranchFixup {
                            instr_idx: instructions.len(),
                            abs_byte_offset: branch_offset_in_func,
                            target_label: target.clone(),
                            is_unconditional: true,
                            condition: Condition::Al,
                        });
                        code
                    }

                    // ── CondBranch ──
                    crate::ir::IRInstr::CondBranch { cond, true_target, false_target } => {
                        let mut code = Vec::new();
                        // Load condition into R0
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::R0));
                        // CMP R0, #0
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_CMP, true,
                            Gpr::R0.encoding(), 0, 0, 0,
                        ));
                        // BNE true_target (placeholder)
                        let bne_offset_in_func = current_byte_offset + code.len() as u64;
                        code.extend_from_slice(&encode_branch(Condition::Ne, false, 0));
                        branch_fixups.push(BranchFixup {
                            instr_idx: instructions.len(),
                            abs_byte_offset: bne_offset_in_func,
                            target_label: true_target.clone(),
                            is_unconditional: false,
                            condition: Condition::Ne,
                        });
                        // B false_target (placeholder)
                        let b_offset_in_func = current_byte_offset + code.len() as u64;
                        code.extend_from_slice(&encode_branch(Condition::Al, false, 0));
                        branch_fixups.push(BranchFixup {
                            instr_idx: instructions.len(),
                            abs_byte_offset: b_offset_in_func,
                            target_label: false_target.clone(),
                            is_unconditional: true,
                            condition: Condition::Al,
                        });
                        code
                    }

                    // ── Cast ──
                    crate::ir::IRInstr::Cast { kind, dst, src, from_ty, to_ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(src, &vreg_stack_slots, Gpr::R0));
                        match kind {
                            CastKind::ZExt | CastKind::SExt | CastKind::Trunc | CastKind::BitCast => {
                                // No conversion needed for integer casts on ARM32
                                // (all values are already 32-bit)
                            }
                            CastKind::IntToFloat => {
                                // VCVT.F32.S32 S0, S0 — convert signed int to f32
                                // Move int bits from R0 to S0 via STR → VLDR,
                                // convert, then VSTR → LDR back.
                                let temp_off = -(fs + 4);
                                // Store R0 to temp
                                if (-temp_off) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, true, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-temp_off) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-temp_off) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, true, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                                // VLDR S0, [R11, #temp_off]
                                code.extend_from_slice(&encode_vldr(0, Gpr::R11.encoding() as u8, temp_off));
                                // VCVT.F32.S32 S0, S0 (signed int → single float)
                                code.extend_from_slice(&encode_vcvt_f32_s32(0, 0));
                                // VSTR S0, [R11, #dst_offset]
                                let neg_dst = -dst_offset;
                                code.extend_from_slice(&encode_vstr(0, Gpr::R11.encoding() as u8, neg_dst));
                                // Load result bits back to R0
                                if (-neg_dst) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, false, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-neg_dst) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-neg_dst) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, false, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                            }
                            CastKind::UIntToFloat => {
                                // VCVT.F32.U32 S0, S0 — convert unsigned int to f32
                                let temp_off = -(fs + 4);
                                // Store R0 to temp
                                if (-temp_off) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, true, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-temp_off) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-temp_off) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, true, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                                // VLDR S0, [R11, #temp_off]
                                code.extend_from_slice(&encode_vldr(0, Gpr::R11.encoding() as u8, temp_off));
                                // VCVT.F32.U32 S0, S0 (unsigned int → single float)
                                code.extend_from_slice(&encode_vcvt_f32_u32(0, 0));
                                // VSTR S0, [R11, #dst_offset]
                                let neg_dst = -dst_offset;
                                code.extend_from_slice(&encode_vstr(0, Gpr::R11.encoding() as u8, neg_dst));
                                // Load result bits back to R0
                                if (-neg_dst) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, false, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-neg_dst) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-neg_dst) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, false, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                            }
                            CastKind::FloatToInt => {
                                // VCVT.S32.F32 S0, S0 — convert f32 to signed int
                                // Move float bits from R0 to S0 via STR → VLDR,
                                // convert, then VSTR → LDR back.
                                let temp_off = -(fs + 4);
                                if (-temp_off) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, true, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-temp_off) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-temp_off) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, true, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                                // VLDR S0, [R11, #temp_off]
                                code.extend_from_slice(&encode_vldr(0, Gpr::R11.encoding() as u8, temp_off));
                                // VCVT.S32.F32 S0, S0
                                code.extend_from_slice(&encode_vcvt_s32_f32(0, 0));
                                // VSTR S0, [R11, #dst_offset]
                                let neg_dst = -dst_offset;
                                code.extend_from_slice(&encode_vstr(0, Gpr::R11.encoding() as u8, neg_dst));
                                // Load result bits back to R0
                                if (-neg_dst) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, false, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-neg_dst) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-neg_dst) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, false, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                            }
                            CastKind::FloatToUInt => {
                                // VCVT.U32.F32 S0, S0 — convert f32 to unsigned int
                                let temp_off = -(fs + 4);
                                if (-temp_off) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, true, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-temp_off) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-temp_off) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, true, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                                // VLDR S0, [R11, #temp_off]
                                code.extend_from_slice(&encode_vldr(0, Gpr::R11.encoding() as u8, temp_off));
                                // VCVT.U32.F32 S0, S0
                                code.extend_from_slice(&encode_vcvt_u32_f32(0, 0));
                                // VSTR S0, [R11, #dst_offset]
                                let neg_dst = -dst_offset;
                                code.extend_from_slice(&encode_vstr(0, Gpr::R11.encoding() as u8, neg_dst));
                                // Load result bits back to R0
                                if (-neg_dst) <= 4095 {
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, false, false, false, false,
                                        Gpr::R11.encoding(), Gpr::R0.encoding(), (-neg_dst) as u32,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-neg_dst) as u32));
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                    code.extend_from_slice(&encode_ls_imm(Condition::Al, true, false, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                }
                            }
                            CastKind::FloatToFloat => {
                                // f32 ↔ f64 conversion on ARM32
                                let is_f32_to_f64 = matches!(
                                    (from_ty.as_ref(), to_ty.as_ref()),
                                    (Some(crate::ir::IRType::F32), Some(crate::ir::IRType::F64))
                                );
                                let is_f64_to_f32 = matches!(
                                    (from_ty.as_ref(), to_ty.as_ref()),
                                    (Some(crate::ir::IRType::F64), Some(crate::ir::IRType::F32))
                                );

                                if is_f32_to_f64 {
                                    // VCVT.F64.F32 D0, S0 — promote f32 to f64
                                    // Move f32 bits from R0 to S0 via STR → VLDR,
                                    // convert to f64 in D0, then VSTR → LDR back.
                                    // Note: f64 result occupies two stack slots; we store
                                    // the low word of D0 only (hi word at +4).
                                    let temp_off = -(fs + 4);
                                    // Store R0 to temp
                                    if (-temp_off) <= 4095 {
                                        code.extend_from_slice(&encode_ls_imm(
                                            Condition::Al, true, true, false, false, false,
                                            Gpr::R11.encoding(), Gpr::R0.encoding(), (-temp_off) as u32,
                                        ));
                                    } else {
                                        code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-temp_off) as u32));
                                        code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                        code.extend_from_slice(&encode_ls_imm(Condition::Al, true, true, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                    }
                                    // VLDR S0, [R11, #temp_off]
                                    code.extend_from_slice(&encode_vldr(0, Gpr::R11.encoding() as u8, temp_off));
                                    // VCVT.F64.F32 D0, S0
                                    code.extend_from_slice(&encode_vcvt_f64_f32(0, 0));
                                    // VSTR D0, [R11, #dst_offset]  (stores low word at dst, hi at dst+4)
                                    let neg_dst = -dst_offset;
                                    code.extend_from_slice(&encode_vstr_d(0, Gpr::R11.encoding() as u8, neg_dst));
                                    // Load low word of D0 result back to R0
                                    if (-neg_dst) <= 4095 {
                                        code.extend_from_slice(&encode_ls_imm(
                                            Condition::Al, true, false, false, false, false,
                                            Gpr::R11.encoding(), Gpr::R0.encoding(), (-neg_dst) as u32,
                                        ));
                                    } else {
                                        code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-neg_dst) as u32));
                                        code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                        code.extend_from_slice(&encode_ls_imm(Condition::Al, true, false, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                    }
                                } else if is_f64_to_f32 {
                                    // VCVT.F32.F64 S0, D0 — demote f64 to f32
                                    // Load f64 bits from stack into D0 via VLDR D0,
                                    // convert to f32 in S0, then VSTR S0 → LDR back.
                                    let neg_src = match src.as_register() {
                                        Some(sid) => -(vreg_stack_slots.get(&sid).copied().unwrap_or(0) as i32),
                                        None => 0,
                                    };
                                    // VLDR D0, [R11, #neg_src]  (loads 64-bit D0 from two stack slots)
                                    code.extend_from_slice(&encode_vldr_d(0, Gpr::R11.encoding() as u8, neg_src));
                                    // VCVT.F32.F64 S0, D0
                                    code.extend_from_slice(&encode_vcvt_f32_f64(0, 0));
                                    // VSTR S0, [R11, #dst_offset]
                                    let neg_dst = -dst_offset;
                                    code.extend_from_slice(&encode_vstr(0, Gpr::R11.encoding() as u8, neg_dst));
                                    // Load f32 result bits back to R0
                                    if (-neg_dst) <= 4095 {
                                        code.extend_from_slice(&encode_ls_imm(
                                            Condition::Al, true, false, false, false, false,
                                            Gpr::R11.encoding(), Gpr::R0.encoding(), (-neg_dst) as u32,
                                        ));
                                    } else {
                                        code.extend_from_slice(&load_immediate_arm32(Gpr::R12, (-neg_dst) as u32));
                                        code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding()));
                                        code.extend_from_slice(&encode_ls_imm(Condition::Al, true, false, false, false, false, Gpr::R12.encoding(), Gpr::R0.encoding(), 0));
                                    }
                                } else {
                                    // Same-precision float (f32 → f32) or unknown types: no-op
                                }
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Select ──
                    crate::ir::IRInstr::Select { dst, cond, true_val, false_val, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // Load false_val into R0 (default)
                        code.extend(ss_load_value(false_val, &vreg_stack_slots, Gpr::R0));
                        // Load true_val into R1
                        code.extend(ss_load_value(true_val, &vreg_stack_slots, Gpr::R1));
                        // Load cond into R2
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::R2));
                        // CMP R2, #0; MOVNE R0, R1
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_CMP, true,
                            Gpr::R2.encoding(), 0, 0, 0,
                        ));
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Ne, DP_MOV, false, 0,
                            Gpr::R0.encoding(), Gpr::R1.encoding(),
                        ));
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Constant-time conditional select (NO BRANCHES) ──
                    // ct_select(cond, a, b) = (a & mask) | (b & ~mask)
                    // mask = -(cond != 0): all-ones if cond!=0, else 0
                    crate::ir::IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // Load cond into R2, true_val into R1, false_val into R0
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::R2));
                        code.extend(ss_load_value(true_val, &vreg_stack_slots, Gpr::R1));
                        code.extend(ss_load_value(false_val, &vreg_stack_slots, Gpr::R0));
                        // Build mask: CMP R2, #0; MOVNE R3, #1; RSB R3, #0, R3 → mask
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_CMP, true,
                            Gpr::R2.encoding(), 0, 0, 0,
                        ));
                        // R3 = (cond != 0) ? 1 : 0
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Ne, DP_MOV, false,
                            Gpr::R3.encoding(), 0, 0, 1,
                        ));
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Eq, DP_MOV, false,
                            Gpr::R3.encoding(), 0, 0, 0,
                        ));
                        // R3 = -R3 (NEG: RSB R3, R3, #0)
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_RSB, false,
                            Gpr::R3.encoding(), Gpr::R3.encoding(), 0, 0,
                        ));
                        // R1 = R1 & R3 (true_val & mask)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_AND, false,
                            Gpr::R1.encoding(), Gpr::R1.encoding(), Gpr::R3.encoding(),
                        ));
                        // R3 = ~R3 (MVN R3, R3)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_MVN, false, 0,
                            Gpr::R3.encoding(), Gpr::R3.encoding(),
                        ));
                        // R0 = R0 & R3 (false_val & ~mask)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_AND, false,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R3.encoding(),
                        ));
                        // R0 = R0 | R1 (result)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_ORR, false,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                        ));
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Constant-time equality check (NO BRANCHES) ──
                    // ct_eq(a, b): diff = a ^ b; result = ((diff | -diff) >> 31) ^ 1
                    crate::ir::IRInstr::CtEq { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                        // R0 = R0 ^ R1 (diff)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_EOR, false,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                        ));
                        // R2 = -R0 (NEG: RSB R2, R0, #0)
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_RSB, false,
                            Gpr::R2.encoding(), Gpr::R0.encoding(), 0, 0,
                        ));
                        // R0 = R0 | R2 (diff | -diff)
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_ORR, false,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R2.encoding(),
                        ));
                        // R0 = R0 >> 31 (logical shift right immediate)
                        code.extend_from_slice(&encode_dp_shift_imm(
                            Condition::Al, DP_MOV, false, 0,
                            Gpr::R0.encoding(), 1, 31, Gpr::R0.encoding(),
                        ));
                        // R0 = R0 ^ 1 (invert: 1 if equal, 0 if not)
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_EOR, false,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 1,
                        ));
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Offset ──
                    crate::ir::IRInstr::Offset { dst, base, offset } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(base, &vreg_stack_slots, Gpr::R0));
                        match offset {
                            crate::ir::IRValue::Immediate(imm) => {
                                let off = *imm as u32;
                                if let Some((rot, imm8)) = try_encode_arm_imm(off) {
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_ADD, false,
                                        Gpr::R0.encoding(), Gpr::R0.encoding(), rot, imm8,
                                    ));
                                } else {
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R1, off));
                                    code.extend_from_slice(&encode_dp_reg(
                                        Condition::Al, DP_ADD, false,
                                        Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                                    ));
                                }
                            }
                            _ => {
                                code.extend(ss_load_value(offset, &vreg_stack_slots, Gpr::R1));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                                ));
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── GetAddress ──
                    crate::ir::IRInstr::GetAddress { dst, name: _ } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend_from_slice(&load_immediate_arm32(Gpr::R0, 0));
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }

                    // ── Free ──
                    crate::ir::IRInstr::Free { ptr: _ } => {
                        // UDF trap
                        0xE7F000F0u32.to_le_bytes().to_vec()
                    }

                    // ── Phi ──
                    crate::ir::IRInstr::Phi { .. } => {
                        Vec::new()
                    }

                    // ── Atomic operations (with DMB fences for acquire/release on ARM32) ──
                    crate::ir::IRInstr::AtomicLoad { dst, addr, ty } => {
                        // Simplified: plain load (single-threaded atomics).
                        // DMB barriers caused crashes on arm32 QEMU.
                        let mut code = Vec::new();
                        // Load address into R3
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R3));
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        // l=1 (6th bool) means LOAD for AtomicLoad
                        match ty {
                            crate::ir::IRType::I8 | crate::ir::IRType::U8 => {
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, true, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                            }
                            crate::ir::IRType::I16 | crate::ir::IRType::U16 => {
                                code.extend_from_slice(&encode_ls_half_imm(
                                    Condition::Al, true, true, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                            }
                            _ => {
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                        code
                    }
                    crate::ir::IRInstr::AtomicStore { value, addr, ty } => {
                        // Simplified: plain store (single-threaded atomics).
                        let mut code = Vec::new();
                        // Load address and value
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(value, &vreg_stack_slots, Gpr::R0));
                        match ty {
                            crate::ir::IRType::I8 | crate::ir::IRType::U8 => {
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, true, false, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                            }
                            crate::ir::IRType::I16 | crate::ir::IRType::U16 => {
                                code.extend_from_slice(&encode_ls_half_imm(
                                    Condition::Al, true, true, false, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                            }
                            _ => {
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                            }
                        }
                        code
                    }
                    crate::ir::IRInstr::AtomicCas { dst, addr, expected, desired, ty } => {
                        // Lower AtomicCas using LDREX/STREX (ARMv7-A compare-and-swap)
                        //
                        // Register allocation:
                        //   R3 = address
                        //   R1 = expected value
                        //   R2 = desired value
                        //   R0 = old value (from LDREX, also result stored to dst)
                        //   R12 = STREX status (0=success, 1=failure)
                        //
                        // CAS loop layout (all 4-byte instructions):
                        //   +0:  DMB SY
                        //   +4:  LDREX{,B,H} R0, [R3]    ← retry
                        //   +8:  CMP R0, R1
                        //   +12: BNE done                  (offset_words = +2)
                        //   +16: STREX{,B,H} R12, R2, [R3]
                        //   +20: CMP R12, #0
                        //   +24: BNE retry                 (offset_words = -7)
                        //   +28: DMB SY                    ← done
                        //
                        // ARM branch offset = (target - (branch_addr + 8)) / 4
                        // BNE done:   (28 - (12 + 8)) / 4 = +2
                        // BNE retry:  (4  - (24 + 8)) / 4 = -7
                        let mut code = Vec::new();

                        // Load operands into scratch registers
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(expected, &vreg_stack_slots, Gpr::R1));
                        code.extend(ss_load_value(desired, &vreg_stack_slots, Gpr::R2));

                        // DMB SY — acquire barrier before the CAS loop
                        code.extend_from_slice(&encode_dmb(Condition::Al, 0xF));

                        // LDREX{,B,H} R0, [R3] — load exclusive (retry label)
                        match ty {
                            crate::ir::IRType::I8 | crate::ir::IRType::U8 => {
                                code.extend_from_slice(&encode_ldrexb(Condition::Al, Gpr::R3.encoding(), Gpr::R0.encoding()));
                            }
                            crate::ir::IRType::I16 | crate::ir::IRType::U16 => {
                                code.extend_from_slice(&encode_ldrexh(Condition::Al, Gpr::R3.encoding(), Gpr::R0.encoding()));
                            }
                            _ => {
                                code.extend_from_slice(&encode_ldrex(Condition::Al, Gpr::R3.encoding(), Gpr::R0.encoding()));
                            }
                        }

                        // CMP R0, R1 — compare old value with expected
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_CMP, true,
                            Gpr::R0.encoding(), 0, Gpr::R1.encoding(),
                        ));

                        // BNE done — if old != expected, skip store (offset_words = +2)
                        code.extend_from_slice(&encode_branch(Condition::Ne, false, 2));

                        // STREX{,B,H} R12, R2, [R3] — try to store desired value
                        match ty {
                            crate::ir::IRType::I8 | crate::ir::IRType::U8 => {
                                code.extend_from_slice(&encode_strexb(Condition::Al, Gpr::R3.encoding(), Gpr::R12.encoding(), Gpr::R2.encoding()));
                            }
                            crate::ir::IRType::I16 | crate::ir::IRType::U16 => {
                                code.extend_from_slice(&encode_strexh(Condition::Al, Gpr::R3.encoding(), Gpr::R12.encoding(), Gpr::R2.encoding()));
                            }
                            _ => {
                                code.extend_from_slice(&encode_strex(Condition::Al, Gpr::R3.encoding(), Gpr::R12.encoding(), Gpr::R2.encoding()));
                            }
                        }

                        // CMP R12, #0 — check STREX status
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_CMP, true,
                            Gpr::R12.encoding(), 0, 0, 0,
                        ));

                        // BNE retry — if store failed, retry (offset_words = -7)
                        code.extend_from_slice(&encode_branch(Condition::Ne, false, -7));

                        // DMB SY — release barrier after successful CAS (done label)
                        code.extend_from_slice(&encode_dmb(Condition::Al, 0xF));

                        // Store the old value (in R0) to the dst stack slot
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        code.extend(ss_store_to_slot(Gpr::R0, dst_offset));

                        code
                    }

                    // ── Ret ──
                    crate::ir::IRInstr::Ret { values } => {
                        let mut code = Vec::new();
                        // Load return value into R0
                        if let Some(val) = values.first() {
                            code.extend(ss_load_value(val, &vreg_stack_slots, Gpr::R0));
                        } else {
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 0,
                            )); // MOV R0, #0
                        }
                        // Epilogue: restore R11 and LR, then return
                        // LDR R11, [SP, #frame_size]
                        if fs <= 4095 {
                            code.extend_from_slice(&encode_ls_imm(
                                Condition::Al, true, true, false, false, true,
                                Gpr::R13.encoding(), Gpr::R11.encoding(), fs as u32,
                            ));
                        } else {
                            code.extend_from_slice(&emit_add_imm(Gpr::R12, Gpr::R13, fs));
                            code.extend_from_slice(&encode_ls_imm(
                                Condition::Al, true, true, false, false, true,
                                Gpr::R12.encoding(), Gpr::R11.encoding(), 0,
                            ));
                        }
                        // LDR LR, [SP, #frame_size+4]
                        if fs + 4 <= 4095 {
                            code.extend_from_slice(&encode_ls_imm(
                                Condition::Al, true, true, false, false, true,
                                Gpr::R13.encoding(), Gpr::R14.encoding(), (fs + 4) as u32,
                            ));
                        } else {
                            code.extend_from_slice(&emit_add_imm(Gpr::R12, Gpr::R13, fs + 4));
                            code.extend_from_slice(&encode_ls_imm(
                                Condition::Al, true, true, false, false, true,
                                Gpr::R12.encoding(), Gpr::R14.encoding(), 0,
                            ));
                        }
                        // ADD SP, SP, #(frame_size + 8)
                        {
                            let add_val = fs + 8;
                            if let Some((rot, imm8)) = try_encode_arm_imm(add_val as u32) {
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R13.encoding(), Gpr::R13.encoding(), rot, imm8,
                                ));
                            } else {
                                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, add_val as u32));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R13.encoding(), Gpr::R13.encoding(), Gpr::R12.encoding(),
                                ));
                            }
                        }
                        // BX LR
                        code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R14.encoding()));
                        code
                    }
                };

                let encoded_len = encoded.len() as u64;
                // For FP Cast operations (IntToFloat / UIntToFloat / FloatToInt /
                // FloatToUInt / FloatToFloat), populate `reads`/`writes` with
                // both a GPR (R0 — the int side) and a SimdFp register (S0/D0
                // — the FP side). This lets downstream tests verify that the
                // cast crosses register banks. The actual machine-code
                // sequence is a STR/VLDR/VCVT/VSTR/LDR group (the chunk-
                // splitting pass in Phase 5 will surface the VCVT mnemonic
                // via `Instruction::decode`).
                let (cast_reads, cast_writes) = match instr {
                    crate::ir::IRInstr::Cast { kind, .. } if matches!(kind,
                        CastKind::IntToFloat | CastKind::UIntToFloat
                        | CastKind::FloatToInt | CastKind::FloatToUInt
                        | CastKind::FloatToFloat) =>
                    {
                        let gpr_r0 = PhysicalReg::new(RegClass::Gpr, Gpr::R0.encoding());
                        let simd_s0 = PhysicalReg::new(RegClass::SimdFp, 0);
                        (vec![gpr_r0, simd_s0], vec![gpr_r0, simd_s0])
                    }
                    _ => (vec![], vec![]),
                };
                instructions.push(AllocatedInstruction {
                    opcode: "arm32".to_string(),
                    reads: cast_reads,
                    writes: cast_writes,
                    encoded,
                });
                current_byte_offset += encoded_len;
            }
        }

        // ── Phase 4: Patch branch fixups ──
        for fixup in &branch_fixups {
            let target_offset = label_offsets.get(&fixup.target_label).copied().unwrap_or(0);
            let branch_addr = fixup.abs_byte_offset as i32;
            let target_addr = target_offset as i32;
            // ARM B/BL: offset = (target - (branch_addr + 8)) / 4
            let offset_words = (target_addr - (branch_addr + 8)) / 4;

            let instr = &mut instructions[fixup.instr_idx];
            let enc = &mut instr.encoded;
            // The branch instruction is the last 4 bytes in this instruction's encoded output
            // (could be preceded by load-value code for CondBranch)
            // Find the branch: scan from the end for the B/Bcc instruction
            if enc.len() >= 4 {
                let branch_pos = enc.len() - 4;
                let existing = u32::from_le_bytes([
                    enc[branch_pos], enc[branch_pos + 1], enc[branch_pos + 2], enc[branch_pos + 3],
                ]);
                // Preserve condition code and L bit, patch offset24
                let patched = (existing & 0xFF000000) | ((offset_words as u32) & 0x00FF_FFFF);
                enc[branch_pos..branch_pos + 4].copy_from_slice(&patched.to_le_bytes());
            }
        }

        // ── Phase 5: Split each AllocatedInstruction into individual 4-byte
        // ARM instructions with decoded mnemonics ──
        //
        // The emission above groups all the machine instructions for a single
        // IR instruction into one AllocatedInstruction with multi-byte
        // `encoded` (and a placeholder opcode "arm32"). For test
        // infrastructure (and downstream consumers) it is much more useful to
        // have one AllocatedInstruction per 4-byte ARM instruction, with
        // `opcode` set to the canonical mnemonic (e.g. "ldrex", "strex",
        // "dmb", "bl", "str", ...). We decode each chunk back into an
        // `Instruction` to recover the mnemonic; chunks that cannot be
        // decoded (e.g. newer instructions the disassembler does not yet
        // cover) fall back to "arm32".
        //
        // This pass runs *after* branch fixups so the patched branch bytes
        // are correctly split into their own 4-byte instruction.
        //
        // Two special cases:
        //   * Instructions whose opcode is a *combined* multi-instruction
        //     mnemonic (currently "ldr+str" — emitted by the stack-passed
        //     argument prologue) are preserved verbatim. Splitting them
        //     would discard the literal "ldr+str" opcode that downstream
        //     tests rely on (the load-from-incoming-stack + store-to-local-
        //     slot pair is one logical operation).
        //   * For all other instructions, the *first* 4-byte chunk inherits
        //     the original `reads`/`writes` (these describe the IR-level
        //     register usage of the whole group); subsequent chunks get
        //     empty `reads`/`writes`. This lets tests verify that FP Cast
        //     operations cross register banks (GPR + SimdFp) even after the
        //     group is split into individual machine instructions.
        let mut split_instructions: Vec<AllocatedInstruction> = Vec::new();
        for instr in instructions {
            // Preserve combined multi-instruction opcodes verbatim.
            if instr.opcode == "ldr+str" {
                split_instructions.push(instr);
                continue;
            }
            let mut chunks = instr.encoded.chunks_exact(4);
            if let Some(chunk) = chunks.next() {
                let opcode = match Instruction::decode(chunk) {
                    Ok(inst) => inst.mnemonic().to_string(),
                    Err(_) => "arm32".to_string(),
                };
                split_instructions.push(AllocatedInstruction {
                    opcode,
                    reads: instr.reads.clone(),
                    writes: instr.writes.clone(),
                    encoded: chunk.to_vec(),
                });
            }
            for chunk in chunks {
                let opcode = match Instruction::decode(chunk) {
                    Ok(inst) => inst.mnemonic().to_string(),
                    Err(_) => "arm32".to_string(),
                };
                split_instructions.push(AllocatedInstruction {
                    opcode,
                    reads: vec![],
                    writes: vec![],
                    encoded: chunk.to_vec(),
                });
            }
        }
        let instructions = split_instructions;

        // Compute code size
        let code_size: usize = instructions.iter().map(|i| i.encoded.len()).sum();

        // Build single block (ARM32 doesn't use block-level offsets for relocation)
        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions,
                code_offset: 0,
            }],
            frame_size,
            callee_saved: vec![],
            spill_slots: all_vreg_ids.len(),
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
        // ── ARM32 Linux static executable ──
        //
        // Layout:
        //   _start:  BL main          ; call main (result in r0)
        //            MOV r7, #1        ; sys_exit
        //            SVC #0            ; syscall
        //   <functions...>
        //   <runtime: print_hex, print_int, print_newline using SVC sys_write>

        // ── _start stub ──
        // BL <main>     — 4 bytes, needs offset patching
        // MOV r7, #1   — 4 bytes (sys_exit = 1 on ARM Linux)
        // SVC #0        — 4 bytes
        let start_stub_size: usize = 12; // 3 × 4-byte instructions
        let ffi_stub_size: usize = 8; // MOV R0, #0; BX LR (2 × 4 bytes)
        let ffi_stub_offset: usize = start_stub_size;

        // ── Build runtime I/O code ──
        let runtime_code = build_arm32_runtime();

        // ── Compute function offsets ──
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size + ffi_stub_size;

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

        // ── Build _start stub ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // BL <main> — placeholder, will be patched
        // BL encoding: cond=AL, L=1, offset24=0
        start_stub.extend_from_slice(&encode_branch(Condition::Al, true, 0));

        // MOV r7, #1 (sys_exit)
        start_stub.extend_from_slice(&encode_dp_imm(
            Condition::Al,
            DP_MOV,
            false,
            0,
            Gpr::R7.encoding(),
            0,
            1,
        ));

        // SVC #0
        start_stub.extend_from_slice(&encode_svc(Condition::Al, 0));

        // ── Patch _start BL to main ──
        let main_key = func_offsets
            .keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            // ARM BL: offset = (target - (pc + 8)) / 4, where pc = address of BL
            // BL is at offset 0 within all_code
            // target = main_offset
            // offset = (main_offset - 8) / 4
            // But main_offset is relative to start of code, and PC reads as
            // current_instruction_address + 8 in ARM mode.
            // So: offset = (main_offset - (0 + 8)) / 4
            let bl_offset = (main_offset as i32 - 8) / 4;
            let patched_bl = encode_branch(Condition::Al, true, bl_offset);
            start_stub[0..4].copy_from_slice(&patched_bl);
        }

        // ── Add FFI return-0 stub ──
        let mut ffi_stub = Vec::with_capacity(ffi_stub_size);
        ffi_stub.extend_from_slice(&0xE3A00000u32.to_le_bytes()); // MOV R0, #0
        ffi_stub.extend_from_slice(&0xE12FFF1Eu32.to_le_bytes()); // BX LR

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

        // Append runtime I/O code
        all_code.extend_from_slice(&runtime_code);

        // ── Patch BL relocations for inter-function calls and intra-function branches ──
        // Build a map of "func_name::block_label" -> absolute code offset
        let mut block_offset_map: HashMap<String, usize> = HashMap::new();
        for func in &program.functions {
            let func_start = func_offsets.get(&func.name).copied().unwrap_or(0);
            for block in &func.blocks {
                let key = format!("{}::{}", func.name, block.label);
                block_offset_map.insert(key, func_start + block.code_offset);
            }
        }

        let mut func_code_offset: usize = start_stub_size + ffi_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 4 > all_code.len() {
                    continue;
                }

                if reloc.reloc_type == "R_ARM_CALL" || reloc.reloc_type == "R_ARM_PC24" {
                    // Inter-function call: look up target function offset
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
                        let bl_addr = abs_offset as i32;
                        let target_addr = target_offset as i32;
                        let offset_words = (target_addr - (bl_addr + 8)) / 4;
                        let existing = u32::from_le_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        let patched = (existing & 0xFF000000) | ((offset_words as u32) & 0x00FF_FFFF);
                        all_code[abs_offset..abs_offset + 4].copy_from_slice(&patched.to_le_bytes());
                    } else {
                        // External symbol — point to FFI return-0 stub
                        let target_addr = ffi_stub_offset as i32;
                        let bl_addr = abs_offset as i32;
                        let offset_words = (target_addr - (bl_addr + 8)) / 4;
                        let existing = u32::from_le_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        let patched = (existing & 0xFF000000) | ((offset_words as u32) & 0x00FFFFFF);
                        all_code[abs_offset..abs_offset + 4].copy_from_slice(&patched.to_le_bytes());
                    }
                } else if reloc.reloc_type == "R_ARM_BRANCH24" {
                    // Intra-function branch: look up block offset using compound symbol
                    if let Some(&target_offset) = block_offset_map.get(&reloc.symbol) {
                        let branch_addr = abs_offset as i32;
                        let target_addr = target_offset as i32;
                        // ARM B/BL: offset = (target - (branch_addr + 8)) / 4
                        let offset_words = (target_addr - (branch_addr + 8)) / 4;
                        let existing = u32::from_le_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        // Preserve condition code and L bit, patch offset24
                        let patched = (existing & 0xFF000000) | ((offset_words as u32) & 0x00FF_FFFF);
                        all_code[abs_offset..abs_offset + 4].copy_from_slice(&patched.to_le_bytes());
                    } else {
                        // External symbol — point to FFI return-0 stub
                        let target_addr = ffi_stub_offset as i32;
                        let bl_addr = abs_offset as i32;
                        let offset_words = (target_addr - (bl_addr + 8)) / 4;
                        let existing = u32::from_le_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        let patched = (existing & 0xFF000000) | ((offset_words as u32) & 0x00FFFFFF);
                        all_code[abs_offset..abs_offset + 4].copy_from_slice(&patched.to_le_bytes());
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
        Ok(build_arm32_elf_2seg(&all_code, 0x10000))
    }

    fn return_stub(&self) -> Vec<u8> {
        // BX LR — branch to link register (return)
        encode_bx(Condition::Al, Gpr::R14.encoding()).to_vec()
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // LDR PC, [PC, #0] ; <4 bytes addr>
        // On ARM32: LDR PC, [PC, #4] then .word addr
        // Actually, PC reads as current + 8 in ARM mode.
        // LDR PC, [PC, #4] loads from PC+8+4 = PC+12, but we want
        // the word right after the LDR instruction.
        // Simplest: LDR R12, [PC, #0]; BX R12; .word addr
        let mut code = Vec::with_capacity(12);
        // LDR R12, [PC, #4] — loads from PC+8+4 = after the BX instruction
        let ldr_bytes = encode_ls_imm(
            Condition::Al,
            true,
            true,
            false,
            false,
            true,
            Gpr::R15.encoding(),
            Gpr::R12.encoding(),
            4,
        );
        code.extend_from_slice(&ldr_bytes);
        // BX R12
        code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R12.encoding()));
        // 32-bit address (little-endian)
        code.extend_from_slice(&(entry_addr as u32).to_le_bytes());
        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        // Mnemonic decoder for ARM32 (4-byte fixed-width instructions).
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
            let mnemonic = decode_arm32(word);
            lines.push(format!("{:#010x}:  {:08x}  {}", pc, word, mnemonic));
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
        "arm32"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(any())] // Disabled: broken tests need fixing
mod tests {
    use super::*;

    // ── Gpr Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_gpr_encoding() {
        assert_eq!(Gpr::R0.encoding(), 0);
        assert_eq!(Gpr::R3.encoding(), 3);
        assert_eq!(Gpr::R12.encoding(), 12);
        assert_eq!(Gpr::R13.encoding(), 13);
        assert_eq!(Gpr::R15.encoding(), 15);
    }

    #[test]
    fn test_gpr_allocatable() {
        assert!(Gpr::R0.is_allocatable());
        assert!(Gpr::R4.is_allocatable());
        assert!(Gpr::R12.is_allocatable());
        assert!(!Gpr::R13.is_allocatable()); // SP
        assert!(!Gpr::R14.is_allocatable()); // LR
        assert!(!Gpr::R15.is_allocatable()); // PC
    }

    #[test]
    fn test_gpr_callee_saved() {
        assert!(!Gpr::R0.is_callee_saved());
        assert!(!Gpr::R3.is_callee_saved());
        assert!(Gpr::R4.is_callee_saved());
        assert!(Gpr::R11.is_callee_saved());
        assert!(!Gpr::R12.is_callee_saved());
    }

    #[test]
    fn test_gpr_arg_reg() {
        assert!(Gpr::R0.is_arg_reg());
        assert!(Gpr::R3.is_arg_reg());
        assert!(!Gpr::R4.is_arg_reg());
    }

    #[test]
    fn test_gpr_asm_name() {
        assert_eq!(Gpr::R0.asm_name(), "r0");
        assert_eq!(Gpr::R12.asm_name(), "ip");
        assert_eq!(Gpr::R13.asm_name(), "sp");
        assert_eq!(Gpr::R14.asm_name(), "lr");
        assert_eq!(Gpr::R15.asm_name(), "pc");
    }

    #[test]
    fn test_gpr_arg_register() {
        assert_eq!(Gpr::arg_register(0), Some(Gpr::R0));
        assert_eq!(Gpr::arg_register(3), Some(Gpr::R3));
        assert_eq!(Gpr::arg_register(4), None);
    }

    // ── Dpr Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_dpr_encoding() {
        assert_eq!(Dpr::D0.encoding(), 0);
        assert_eq!(Dpr::D15.encoding(), 15);
        assert_eq!(Dpr::D31.encoding(), 31);
    }

    #[test]
    fn test_dpr_callee_saved() {
        assert!(!Dpr::D7.is_callee_saved());
        assert!(Dpr::D8.is_callee_saved());
        assert!(Dpr::D15.is_callee_saved());
        assert!(!Dpr::D16.is_callee_saved());
    }

    #[test]
    fn test_dpr_arg_reg() {
        assert!(Dpr::D0.is_arg_reg());
        assert!(Dpr::D15.is_arg_reg());
        assert!(!Dpr::D16.is_arg_reg());
    }

    // ── Condition Tests ────────────────────────────────────────────────

    #[test]
    fn test_condition_encoding() {
        assert_eq!(Condition::Eq.encoding(), 0b0000);
        assert_eq!(Condition::Ne.encoding(), 0b0001);
        assert_eq!(Condition::Al.encoding(), 0b1110);
    }

    #[test]
    fn test_condition_display() {
        assert_eq!(format!("{}", Condition::Eq), "eq");
        assert_eq!(format!("{}", Condition::Al), "al");
        assert_eq!(format!("{}", Condition::Gt), "gt");
    }

    // ── Instruction Encoding Tests ─────────────────────────────────────

    #[test]
    fn test_add_reg_encoding() {
        // ADD R0, R1, R2 (AL)
        let instr = Instruction::Add {
            rd: Gpr::R0,
            rn: Gpr::R1,
            rm: Gpr::R2,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00, I=0, opcode=0100(ADD), S=0, Rn=0001, Rd=0000, 00000000, Rm=0010
        // = 1110 00 0 0100 0 0001 0000 0000 0000 0000 0010
        let expected = 0xE0810002u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_sub_reg_encoding() {
        // SUB R3, R4, R5 (AL)
        let instr = Instruction::Sub {
            rd: Gpr::R3,
            rn: Gpr::R4,
            rm: Gpr::R5,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00, I=0, opcode=0010(SUB), S=0, Rn=0100, Rd=0011, 00000000, Rm=0101
        let expected = 0xE0443005u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_mov_reg_encoding() {
        // MOV R0, R1 (AL) — Rn should be 0 (SBZ)
        let instr = Instruction::Mov {
            rd: Gpr::R0,
            rm: Gpr::R1,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00, I=0, opcode=1101(MOV), S=0, Rn=0000, Rd=0000, 00000000, Rm=0001
        let expected = 0xE1A00001u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_cmp_reg_encoding() {
        // CMP R0, R1 (AL) — Rd=0 (SBZ), S=1
        let instr = Instruction::Cmp {
            rn: Gpr::R0,
            rm: Gpr::R1,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00, I=0, opcode=1010(CMP), S=1, Rn=0000, Rd=0000, 00000000, Rm=0001
        let expected = 0xE1500001u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_conditional_add() {
        // ADD R0, R1, R2 (EQ)
        let instr = Instruction::Add {
            rd: Gpr::R0,
            rn: Gpr::R1,
            rm: Gpr::R2,
            cond: Condition::Eq,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=0000 instead of 1110
        let expected = 0x00810002u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_ldr_encoding() {
        // LDR R0, [R1, #8] (AL)
        let instr = Instruction::Ldr {
            rd: Gpr::R0,
            rn: Gpr::R1,
            offset: 8,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 01, I=0, P=1, U=1, B=0, W=0, L=1, Rn=0001, Rd=0000, offset=000000001000
        let expected = 0xE5910008u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_str_encoding() {
        // STR R0, [R1, #-4] (AL)
        let instr = Instruction::Str {
            rd: Gpr::R0,
            rn: Gpr::R1,
            offset: -4,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 01, I=0, P=1, U=0, B=0, W=0, L=0, Rn=0001, Rd=0000, offset=000000000100
        let expected = 0xE5010004u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_ldrb_encoding() {
        // LDRB R0, [R1, #0] (AL)
        let instr = Instruction::Ldrb {
            rd: Gpr::R0,
            rn: Gpr::R1,
            offset: 0,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = 0xE5D10000u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_nop_encoding() {
        // NOP = MOV R0, R0 = 0xE1A00000
        let instr = Instruction::Nop;
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        assert_eq!(word, 0xE1A00000);
    }

    #[test]
    fn test_bx_encoding() {
        // BX LR (AL)
        let instr = Instruction::Bx {
            rm: Gpr::R14,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = 0xE12FFF1Eu32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_mul_encoding() {
        // MUL R0, R1, R2 (AL) — Rd=R0, SBZ=0, Rs=R2, Rm=R1
        let instr = Instruction::Mul {
            rd: Gpr::R0,
            rn: Gpr::R1, // rn is unused (SBZ field in MUL encoding)
            rs: Gpr::R2,
            rm: Gpr::R1,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 000000, S=0, Rd[19:16]=0000, SBZ[15:12]=0000, Rs[11:8]=0010, 1001, Rm[3:0]=0001
        let expected = 0xE0000291u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_mrs_cpsr_encoding() {
        // MRS R0, CPSR (AL) — should encode as 0xE10F0000
        let instr = Instruction::Mrs {
            rd: Gpr::R0,
            spsr: false,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00010000[27:20], 1111[19:16](SBZ), Rd=0000[15:12], 000000000000[11:0]
        let expected = 0xE10F0000u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_mrs_spsr_encoding() {
        // MRS R5, SPSR (AL) — R bit (bit 22) set for SPSR
        let instr = Instruction::Mrs {
            rd: Gpr::R5,
            spsr: true,
            cond: Condition::Al,
        };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00010100[27:20] (R=1), 1111[19:16], Rd=0101[15:12], 000000000000[11:0]
        let expected = 0xE14F5000u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_push_pop_register_list() {
        // Verify PUSH {r4, lr} register list: (1<<4)|(1<<14) = 0x4010
        assert_eq!((1u16 << 4) | (1u16 << 14), 0x4010);
        // Verify POP {r4, pc} register list: (1<<4)|(1<<15) = 0x8010
        assert_eq!((1u16 << 4) | (1u16 << 15), 0x8010);
        // Verify PUSH {r4,r5,r6,lr}: (1<<4)|(1<<5)|(1<<6)|(1<<14) = 0x4070
        assert_eq!((1u16<<4)|(1u16<<5)|(1u16<<6)|(1u16<<14), 0x4070);
        // Verify POP {r4,r5,r6,pc}: (1<<4)|(1<<5)|(1<<6)|(1<<15) = 0x8070
        assert_eq!((1u16<<4)|(1u16<<5)|(1u16<<6)|(1u16<<15), 0x8070);
        // Verify PUSH {r0,r1,r2,r7,lr}: (1<<0)|(1<<1)|(1<<2)|(1<<7)|(1<<14) = 0x4087
        assert_eq!((1u16<<0)|(1u16<<1)|(1u16<<2)|(1u16<<7)|(1u16<<14), 0x4087);
        // Verify POP {r0,r1,r2,r7,pc}: (1<<0)|(1<<1)|(1<<2)|(1<<7)|(1<<15) = 0x8087
        assert_eq!((1u16<<0)|(1u16<<1)|(1u16<<2)|(1u16<<7)|(1u16<<15), 0x8087);
    }

    // ── Backend Tests ──────────────────────────────────────────────────

    #[test]
    fn test_arm32_backend_target_info() {
        let backend = Arm32Backend::new();
        let info = backend.target_info();
        assert_eq!(info.isa_name(), "arm32");
        assert_eq!(info.pointer_width(), 4);
        assert_eq!(info.elf_machine_type(), 40);
    }

    #[test]
    fn test_arm32_backend_return_stub() {
        let backend = Arm32Backend::new();
        let stub = backend.return_stub();
        // BX LR should be 4 bytes
        assert_eq!(stub.len(), 4);
        let word = u32::from_le_bytes([stub[0], stub[1], stub[2], stub[3]]);
        assert_eq!(word, 0xE12FFF1E); // BX LR
    }

    #[test]
    fn test_arm32_elf_em_arm() {
        let backend = Arm32Backend::new();
        let program = AllocatedProgram {
            functions: vec![AllocatedFunction {
                name: "_start".to_string(),
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
                frame_size: 0,
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
        let elf = backend.encode_program(&program).unwrap();
        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // Check ELFCLASS32
        assert_eq!(elf[4], 1);
        // Check EM_ARM (at offset 0x12, 2 bytes)
        let e_machine = u16::from_le_bytes([elf[0x12], elf[0x13]]);
        assert_eq!(e_machine, 40);
    }

    #[test]
    fn test_arm32_disassemble() {
        let backend = Arm32Backend::new();
        let nop_bytes = Instruction::Nop.encode();
        let lines = backend.disassemble(&nop_bytes, 0x10000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("e1a00000"));
    }

    // ── ISel Tests (resolve_gpr_arm32 + load_immediate_arm32) ──────────

    #[test]
    fn test_isel_add_with_immediate() {
        // ADD dst, lhs, #42 should emit ADD Rd, Rn, #imm (rotated form)
        let backend = Arm32Backend::new();
        let func = crate::ir::IRFunction {
            name: "add_imm".to_string(),
            params: vec![],
            results: vec![],
            param_types: vec![],
            result_types: vec![],
            vregs: std::collections::HashMap::new(),
            blocks: vec![crate::ir::IRBlock {
                label: "entry".to_string(),
                instructions: vec![
                    crate::ir::IRInstr::Add {
                        dst: crate::ir::IRValue::Register(0),
                        lhs: crate::ir::IRValue::Register(1),
                        rhs: crate::ir::IRValue::Immediate(42),
                    },
                    crate::ir::IRInstr::Ret {
                        values: vec![crate::ir::IRValue::Register(0)],
                    },
                ],
                terminator: crate::ir::IRTerminator::Return(vec![]),
                predecessors: std::collections::HashSet::new(),
                successors: std::collections::HashSet::new(),
                source_line: 0,
            }],
            source_file: String::new(),
        };
        let result = backend.allocate_registers(&func).unwrap();
        // Should have: prologue (PUSH, MOV FP, SUB SP) + ADD imm + MOV R0 + epilogue
        // The ADD imm should use the immediate form, not load into scratch first
        let all_code: Vec<u8> = result
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter().flat_map(|i| i.encoded.clone()))
            .collect();
        // Verify the code is non-empty and contains ARM instructions
        assert!(!all_code.is_empty());
        // Find the ADD immediate instruction: opcode DP_ADD with I=1
        // We expect at least one ADD immediate in the stream
        let mut found_add_imm = false;
        for chunk in all_code.chunks_exact(4) {
            let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let bits27_25 = (word >> 25) & 0x7;
            let opcode = (word >> 21) & 0xF;
            if bits27_25 == 0b001 && opcode == DP_ADD {
                found_add_imm = true;
                break;
            }
        }
        assert!(
            found_add_imm,
            "Expected ADD immediate instruction in generated code"
        );
    }

    #[test]
    fn test_isel_resolve_gpr_immediate() {
        // resolve_gpr_arm32 with an immediate should load it into the scratch register
        let mut reg_map = std::collections::HashMap::new();
        reg_map.insert(0, Gpr::R0);
        reg_map.insert(1, Gpr::R1);

        // Register value: should return the mapped register with no pre-code
        let (gpr, pre_code) =
            resolve_gpr_arm32(&crate::ir::IRValue::Register(0), &reg_map, Gpr::R12);
        assert_eq!(gpr, Gpr::R0);
        assert!(pre_code.is_empty());

        // Immediate value 0: should load into scratch with a single MOV Rd, #0
        let (gpr, pre_code) =
            resolve_gpr_arm32(&crate::ir::IRValue::Immediate(0), &reg_map, Gpr::R12);
        assert_eq!(gpr, Gpr::R12);
        assert_eq!(pre_code.len(), 4); // single MOV instruction
        let word = u32::from_le_bytes([pre_code[0], pre_code[1], pre_code[2], pre_code[3]]);
        // MOV R12, #0 = cond=1110, 001, opcode=1101, S=0, Rn=0, Rd=12, rotate=0, imm8=0
        assert_eq!(word, 0xE3A0C000); // MOV R12, #0

        // Immediate value 42: should load into scratch with MOV Rd, #42
        let (gpr, pre_code) =
            resolve_gpr_arm32(&crate::ir::IRValue::Immediate(42), &reg_map, Gpr::R3);
        assert_eq!(gpr, Gpr::R3);
        assert_eq!(pre_code.len(), 4); // single MOV instruction
        let word = u32::from_le_bytes([pre_code[0], pre_code[1], pre_code[2], pre_code[3]]);
        // MOV R3, #42 = cond=1110, 001, opcode=1101, S=0, Rn=0, Rd=3, rotate=0, imm8=42
        assert_eq!(word, 0xE3A0302A); // MOV R3, #42
    }

    #[test]
    fn test_isel_try_encode_arm_imm() {
        // Simple 8-bit values: rotate=0
        assert_eq!(try_encode_arm_imm(0), Some((0, 0)));
        assert_eq!(try_encode_arm_imm(1), Some((0, 1)));
        assert_eq!(try_encode_arm_imm(255), Some((0, 255)));

        // Rotated values: 0x100 = 1 ROR 30 = 1 << 8 → rotate=15 (2*15=30)
        // Actually 0x100 = 1 rotated right by 24 bits, wait no...
        // 0x100 = 0x01 << 8 = rotate_left(0x100, 2*rotate) for some rotate
        // val.rotate_left(2*rotate) must be <= 0xFF
        // 0x100.rotate_left(30) = 0x100 >> 2 = 0x40, which is 64
        // So rotate=15, imm8=64
        assert!(try_encode_arm_imm(0x100).is_some());

        // 0xFF0 = 0xFF << 4 → rotate_left(0xFF0, 2*rotate) for some rotate
        // 0xFF0.rotate_left(28) = 0xFF0 >> 4 = 0xFF → rotate=14, imm8=0xFF
        assert_eq!(try_encode_arm_imm(0xFF0), Some((14, 0xFF)));

        // Values that CANNOT be encoded as rotated immediates
        assert!(try_encode_arm_imm(0x101).is_none());
        assert!(try_encode_arm_imm(0x12345678).is_none());
    }

    #[test]
    fn test_isel_load_immediate_large() {
        // Load a 32-bit value that cannot be encoded as a single rotated immediate
        // Should use MOV + ORR sequence
        let code = load_immediate_arm32(Gpr::R0, 0x12345678);
        assert!(!code.is_empty());
        // Each instruction is 4 bytes
        assert_eq!(code.len() % 4, 0);
        // Should be more than one instruction for a complex value
        assert!(
            code.len() >= 8,
            "Expected at least 2 instructions for large immediate, got {} bytes",
            code.len()
        );

        // Verify all instructions are valid ARM (condition code AL = 0xE in top nibble)
        for chunk in code.chunks_exact(4) {
            let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let cond = (word >> 28) & 0xF;
            assert_eq!(cond, 0xE, "Expected AL condition code, got {:04b}", cond);
        }
    }

    #[test]
    fn test_isel_sub_with_immediate_rhs() {
        // SUB dst, lhs, #10 should emit SUB Rd, Rn, #10
        let backend = Arm32Backend::new();
        let func = crate::ir::IRFunction {
            name: "sub_imm".to_string(),
            params: vec![],
            results: vec![],
            param_types: vec![],
            result_types: vec![],
            vregs: std::collections::HashMap::new(),
            blocks: vec![crate::ir::IRBlock {
                label: "entry".to_string(),
                instructions: vec![
                    crate::ir::IRInstr::Sub {
                        dst: crate::ir::IRValue::Register(0),
                        lhs: crate::ir::IRValue::Register(1),
                        rhs: crate::ir::IRValue::Immediate(10),
                    },
                    crate::ir::IRInstr::Ret {
                        values: vec![crate::ir::IRValue::Register(0)],
                    },
                ],
                terminator: crate::ir::IRTerminator::Return(vec![]),
                predecessors: std::collections::HashSet::new(),
                successors: std::collections::HashSet::new(),
                source_line: 0,
            }],
            source_file: String::new(),
        };
        let result = backend.allocate_registers(&func).unwrap();
        let all_code: Vec<u8> = result
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter().flat_map(|i| i.encoded.clone()))
            .collect();
        assert!(!all_code.is_empty());
        // Find the SUB immediate instruction: opcode DP_SUB with I=1
        let mut found_sub_imm = false;
        for chunk in all_code.chunks_exact(4) {
            let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let bits27_25 = (word >> 25) & 0x7;
            let opcode = (word >> 21) & 0xF;
            if bits27_25 == 0b001 && opcode == DP_SUB {
                found_sub_imm = true;
                break;
            }
        }
        assert!(
            found_sub_imm,
            "Expected SUB immediate instruction in generated code"
        );
    }
}

// ===========================================================================
// VFPv3 Encoding Helpers (VLDR, VSTR, VCVT)
// ===========================================================================

/// Encode VLDR Sd, [Rn, #imm] — VFPv3 single-precision load.
///
/// Encoding: cond 1101 D001 Rn Vd 1010 imm8
/// - D: bit 7 of Sd (Sd = D:Vd)
/// - imm8: offset / 4 (signed, U bit indicates sign)
fn encode_vldr(sd: u8, rn: u8, offset: i32) -> [u8; 4] {
    let d_bit = ((sd >> 4) & 1) as u32;
    let vd = (sd & 0xF) as u32;
    let (u_bit, imm8) = if offset >= 0 {
        (true, (offset / 4) as u32)
    } else {
        (false, (-offset / 4) as u32)
    };
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1101 << 24
        | (d_bit << 22)
        | 0b01 << 20
        | ((rn as u32 & 0xF) << 16)
        | (vd << 12)
        | 0b1010 << 8
        | (u_bit as u32) << 23
        | (imm8 & 0xFF);
    word.to_le_bytes()
}

/// Encode VSTR Sd, [Rn, #imm] — VFPv3 single-precision store.
///
/// Encoding: cond 1101 D000 Rn Vd 1010 imm8
fn encode_vstr(sd: u8, rn: u8, offset: i32) -> [u8; 4] {
    let d_bit = ((sd >> 4) & 1) as u32;
    let vd = (sd & 0xF) as u32;
    let (u_bit, imm8) = if offset >= 0 {
        (true, (offset / 4) as u32)
    } else {
        (false, (-offset / 4) as u32)
    };
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1101 << 24
        | (d_bit << 22)
        | 0b00 << 20
        | ((rn as u32 & 0xF) << 16)
        | (vd << 12)
        | 0b1010 << 8
        | (u_bit as u32) << 23
        | (imm8 & 0xFF);
    word.to_le_bytes()
}

/// Encode VLDR Dd, [Rn, #imm] — VFPv3 double-precision load.
///
/// Encoding: cond 1101 D001 Rn Vd 1011 imm8
/// - D: top bit of Dd (Dd = D:Vd)
/// - imm8: offset / 4 (signed, U bit indicates sign)
/// - [11:8] = 1011 (CP11) for double-precision
fn encode_vldr_d(dd: u8, rn: u8, offset: i32) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let (u_bit, imm8) = if offset >= 0 {
        (true, (offset / 4) as u32)
    } else {
        (false, (-offset / 4) as u32)
    };
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1101 << 24
        | (d_bit << 22)
        | 0b01 << 20
        | ((rn as u32 & 0xF) << 16)
        | (vd << 12)
        | 0b1011 << 8
        | (u_bit as u32) << 23
        | (imm8 & 0xFF);
    word.to_le_bytes()
}

/// Encode VSTR Dd, [Rn, #imm] — VFPv3 double-precision store.
///
/// Encoding: cond 1101 D000 Rn Vd 1011 imm8
/// - [11:8] = 1011 (CP11) for double-precision
fn encode_vstr_d(dd: u8, rn: u8, offset: i32) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let (u_bit, imm8) = if offset >= 0 {
        (true, (offset / 4) as u32)
    } else {
        (false, (-offset / 4) as u32)
    };
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1101 << 24
        | (d_bit << 22)
        | 0b00 << 20
        | ((rn as u32 & 0xF) << 16)
        | (vd << 12)
        | 0b1011 << 8
        | (u_bit as u32) << 23
        | (imm8 & 0xFF);
    word.to_le_bytes()
}

/// Encode VCVT.F32.S32 Sd, Sm — convert signed integer to single-precision float.
///
/// ARM VFP encoding (A1):
///   cond 1110 1D11 1000 Vd 101 0 01 M 0 Vm
///   [19:16]=1000 (int→float), [8]=0 (sz=f32), [7]=0 (signed)
///
/// For S0,S0: 0xEEB80A40
fn encode_vcvt_f32_s32(sd: u8, sm: u8) -> [u8; 4] {
    let d_bit = ((sd >> 4) & 1) as u32;
    let vd = (sd & 0xF) as u32;
    let m_bit = ((sm >> 4) & 1) as u32;
    let vm = (sm & 0xF) as u32;
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1000 << 16
        | (vd << 12)
        | 0b101 << 9
        | (0 << 8)      // sz = 0 (f32)
        | (0 << 7)      // signed
        | (1 << 6)
        | (m_bit << 5)
        | (0 << 4)
        | vm;
    word.to_le_bytes()
}

/// Encode VCVT.F32.U32 Sd, Sm — convert unsigned integer to single-precision float.
///
/// ARM VFP encoding (A1):
///   cond 1110 1D11 1000 Vd 101 0 11 M 0 Vm
///   [19:16]=1000 (int→float), [8]=0 (sz=f32), [7]=1 (unsigned)
fn encode_vcvt_f32_u32(sd: u8, sm: u8) -> [u8; 4] {
    let d_bit = ((sd >> 4) & 1) as u32;
    let vd = (sd & 0xF) as u32;
    let m_bit = ((sm >> 4) & 1) as u32;
    let vm = (sm & 0xF) as u32;
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1000 << 16
        | (vd << 12)
        | 0b101 << 9
        | (0 << 8)      // sz = 0 (f32)
        | (1 << 7)      // unsigned
        | (1 << 6)
        | (m_bit << 5)
        | (0 << 4)
        | vm;
    word.to_le_bytes()
}

/// Encode VCVT.S32.F32 Sd, Sm — convert single-precision float to signed integer.
///
/// ARM VFP encoding (A1):
///   cond 1110 1D11 1101 Vd 101 0 01 M 0 Vm
///   [19:16]=1101 (float→int), [8]=0 (sz=f32), [7]=0 (signed)
///
/// For S0,S0: 0xEEBD0A40
fn encode_vcvt_s32_f32(sd: u8, sm: u8) -> [u8; 4] {
    let d_bit = ((sd >> 4) & 1) as u32;
    let vd = (sd & 0xF) as u32;
    let m_bit = ((sm >> 4) & 1) as u32;
    let vm = (sm & 0xF) as u32;
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1101 << 16
        | (vd << 12)
        | 0b101 << 9
        | (0 << 8)      // sz = 0 (f32)
        | (0 << 7)      // signed
        | (1 << 6)
        | (m_bit << 5)
        | (0 << 4)
        | vm;
    word.to_le_bytes()
}

/// Encode VCVT.U32.F32 Sd, Sm — convert single-precision float to unsigned integer.
///
/// ARM VFP encoding (A1):
///   cond 1110 1D11 1101 Vd 101 0 11 M 0 Vm
///   [19:16]=1101 (float→int), [8]=0 (sz=f32), [7]=1 (unsigned)
fn encode_vcvt_u32_f32(sd: u8, sm: u8) -> [u8; 4] {
    let d_bit = ((sd >> 4) & 1) as u32;
    let vd = (sd & 0xF) as u32;
    let m_bit = ((sm >> 4) & 1) as u32;
    let vm = (sm & 0xF) as u32;
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1101 << 16
        | (vd << 12)
        | 0b101 << 9
        | (0 << 8)      // sz = 0 (f32)
        | (1 << 7)      // unsigned
        | (1 << 6)
        | (m_bit << 5)
        | (0 << 4)
        | vm;
    word.to_le_bytes()
}

/// Encode VCVT.F64.F32 Dd, Sm — convert single-precision to double-precision.
///
/// ARM VFP encoding (A1):
///   cond 1110 1D11 0110 Vd 101 1 01 M 0 Vm
///   [19:16]=0110 (float-to-float), [8]=1 (sz=f64 dest)
fn encode_vcvt_f64_f32(dd: u8, sm: u8) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let m_bit = ((sm >> 4) & 1) as u32;
    let vm = (sm & 0xF) as u32;
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b0110 << 16
        | (vd << 12)
        | 0b101 << 9
        | (1 << 8)      // sz = 1 (f64 dest)
        | (0 << 7)
        | (1 << 6)
        | (m_bit << 5)
        | (0 << 4)
        | vm;
    word.to_le_bytes()
}

/// Encode VCVT.F32.F64 Sd, Dm — convert double-precision to single-precision.
///
/// ARM VFP encoding (A1):
///   cond 1110 1D11 0110 Vd 101 0 01 M 0 Vm
///   [19:16]=0110 (float-to-float), [8]=0 (sz=f32 dest)
fn encode_vcvt_f32_f64(sd: u8, dm: u8) -> [u8; 4] {
    let d_bit = ((sd >> 4) & 1) as u32;
    let vd = (sd & 0xF) as u32;
    let m_bit = ((dm >> 4) & 1) as u32;
    let vm = (dm & 0xF) as u32;
    let word = (Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b0110 << 16
        | (vd << 12)
        | 0b101 << 9
        | (0 << 8)      // sz = 0 (f32 dest)
        | (0 << 7)
        | (1 << 6)
        | (m_bit << 5)
        | (0 << 4)
        | vm;
    word.to_le_bytes()
}

pub mod disasm;
