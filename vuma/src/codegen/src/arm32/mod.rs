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
//! in bits [31:28]. The `AL` (always) condition is used for unconditional
//! instructions.
//!
//! ## References
//!
//! - ARM Architecture Reference Manual (ARMv7-A and ARMv7-R edition)
//! - Procedure Call Standard for the ARM Architecture (AAPCS)

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Arm32TargetInfo,
    Backend, BackendError, PhysicalReg, RegClass, TargetInfo,
};
use crate::ir::{BinOpKind, CmpKind, IRFunction, UnaryOpKind};
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
            Gpr::R4
                | Gpr::R5
                | Gpr::R6
                | Gpr::R7
                | Gpr::R8
                | Gpr::R9
                | Gpr::R10
                | Gpr::R11
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
            Dpr::D8
                | Dpr::D9
                | Dpr::D10
                | Dpr::D11
                | Dpr::D12
                | Dpr::D13
                | Dpr::D14
                | Dpr::D15
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

/// ARM condition codes (4-bit encoding in bits [31:28] of every ARM instruction).
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
const _DP_RSB: u32 = 0b0011;
const DP_ADD: u32 = 0b0100;
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
fn encode_dp_reg(
    cond: Condition,
    opcode: u32,
    s: bool,
    rn: u32,
    rd: u32,
    rm: u32,
) -> [u8; 4] {
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
fn encode_umull(
    cond: Condition,
    s: bool,
    rd_hi: u32,
    rd_lo: u32,
    rs: u32,
    rm: u32,
) -> [u8; 4] {
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
fn encode_smull(
    cond: Condition,
    s: bool,
    rd_hi: u32,
    rd_lo: u32,
    rs: u32,
    rm: u32,
) -> [u8; 4] {
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
/// Format: `cond[31:28] | 00010000[27:20] | Rd[15:12] | 111100000000[11:0]`
/// For CPSR: R=0. For SPSR: R=1 (bit 22).
fn encode_mrs(cond: Condition, rd: u32, spsr: bool) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_0000 << 20)
        | ((spsr as u32) << 22)
        | ((rd & 0xF) << 12)
        | 0x0F00; // bits [19:16] = 1111 (SBZ), bits [11:0] = 0
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
    Add { rd: Gpr, rn: Gpr, rm: Gpr, cond: Condition },
    /// SUB Rd, Rn, Rm
    Sub { rd: Gpr, rn: Gpr, rm: Gpr, cond: Condition },
    /// AND Rd, Rn, Rm
    And { rd: Gpr, rn: Gpr, rm: Gpr, cond: Condition },
    /// ORR Rd, Rn, Rm
    Orr { rd: Gpr, rn: Gpr, rm: Gpr, cond: Condition },
    /// EOR Rd, Rn, Rm
    Eor { rd: Gpr, rn: Gpr, rm: Gpr, cond: Condition },
    /// BIC Rd, Rn, Rm
    Bic { rd: Gpr, rn: Gpr, rm: Gpr, cond: Condition },
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
    AddImm { rd: Gpr, rn: Gpr, rotate: u32, imm8: u32, cond: Condition },
    /// SUB Rd, Rn, #imm8 (rotated)
    SubImm { rd: Gpr, rn: Gpr, rotate: u32, imm8: u32, cond: Condition },
    /// MOV Rd, #imm8 (rotated)
    MovImm { rd: Gpr, rotate: u32, imm8: u32, cond: Condition },
    /// CMP Rn, #imm8 (rotated)
    CmpImm { rn: Gpr, rotate: u32, imm8: u32, cond: Condition },

    // ── Shift by Immediate ───────────────────────────────────────────
    /// LSL Rd, Rm, #shift_imm (encoded as MOV Rd, Rm, LSL #imm)
    LslImm { rd: Gpr, rm: Gpr, shift_imm: u32, cond: Condition },
    /// LSR Rd, Rm, #shift_imm
    LsrImm { rd: Gpr, rm: Gpr, shift_imm: u32, cond: Condition },
    /// ASR Rd, Rm, #shift_imm
    AsrImm { rd: Gpr, rm: Gpr, shift_imm: u32, cond: Condition },
    /// ROR Rd, Rm, #shift_imm
    RorImm { rd: Gpr, rm: Gpr, shift_imm: u32, cond: Condition },

    // ── Shift by Register ────────────────────────────────────────────
    /// LSL Rd, Rn, Rs (encoded as MOV Rd, Rn, LSL Rs)
    LslReg { rd: Gpr, rn: Gpr, rs: Gpr, cond: Condition },
    /// LSR Rd, Rn, Rs
    LsrReg { rd: Gpr, rn: Gpr, rs: Gpr, cond: Condition },
    /// ASR Rd, Rn, Rs
    AsrReg { rd: Gpr, rn: Gpr, rs: Gpr, cond: Condition },
    /// ROR Rd, Rn, Rs
    RorReg { rd: Gpr, rn: Gpr, rs: Gpr, cond: Condition },

    // ── Multiply ─────────────────────────────────────────────────────
    /// MUL Rd, Rm, Rs
    Mul { rd: Gpr, rn: Gpr, rs: Gpr, rm: Gpr, cond: Condition },
    /// MLA Rd, Rn, Rm, Rs (Rd = Rn + Rm * Rs)
    Mla { rd: Gpr, rn: Gpr, rs: Gpr, rm: Gpr, cond: Condition },
    /// UMULL RdLo, RdHi, Rm, Rs
    Umull { rd_hi: Gpr, rd_lo: Gpr, rs: Gpr, rm: Gpr, cond: Condition },
    /// SMULL RdLo, RdHi, Rm, Rs
    Smull { rd_hi: Gpr, rd_lo: Gpr, rs: Gpr, rm: Gpr, cond: Condition },

    // ── Load/Store Word ──────────────────────────────────────────────
    /// LDR Rd, [Rn, #offset]
    Ldr { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },
    /// STR Rd, [Rn, #offset]
    Str { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },

    // ── Load/Store Byte ──────────────────────────────────────────────
    /// LDRB Rd, [Rn, #offset]
    Ldrb { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },
    /// STRB Rd, [Rn, #offset]
    Strb { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },

    // ── Load/Store Halfword ──────────────────────────────────────────
    /// LDRH Rd, [Rn, #offset]
    Ldrh { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },
    /// STRH Rd, [Rn, #offset]
    Strh { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },

    // ── Load/Store Doubleword ────────────────────────────────────────
    /// LDRD Rd, [Rn, #offset]
    Ldrd { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },
    /// STRD Rd, [Rn, #offset]
    Strd { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },

    // ── Load Signed Byte/Halfword ────────────────────────────────────
    /// LDRSB Rd, [Rn, #offset]
    Ldrsb { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },
    /// LDRSH Rd, [Rn, #offset]
    Ldrsh { rd: Gpr, rn: Gpr, offset: i32, cond: Condition },

    // ── Load/Store Multiple ──────────────────────────────────────────
    /// LDM Rn!, {register_list}
    Ldm { rn: Gpr, register_list: u16, writeback: bool, cond: Condition },
    /// STM Rn!, {register_list}
    Stm { rn: Gpr, register_list: u16, writeback: bool, cond: Condition },

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
    Mrs { rd: Gpr, spsr: bool, cond: Condition },
    /// MSR CPSR_f, Rm
    Msr { mask: u32, rm: Gpr, cond: Condition },
}

impl Instruction {
    /// Encode this instruction into a 4-byte little-endian machine code word.
    ///
    /// Encoding follows the ARM Architecture Reference Manual.
    pub fn encode(&self) -> [u8; 4] {
        match self {
            // ── Data Processing: Register-Register ──────────────────
            Instruction::Add { rd, rn, rm, cond } => {
                encode_dp_reg(*cond, DP_ADD, false, rn.encoding(), rd.encoding(), rm.encoding())
            }
            Instruction::Sub { rd, rn, rm, cond } => {
                encode_dp_reg(*cond, DP_SUB, false, rn.encoding(), rd.encoding(), rm.encoding())
            }
            Instruction::And { rd, rn, rm, cond } => {
                encode_dp_reg(*cond, DP_AND, false, rn.encoding(), rd.encoding(), rm.encoding())
            }
            Instruction::Orr { rd, rn, rm, cond } => {
                encode_dp_reg(*cond, DP_ORR, false, rn.encoding(), rd.encoding(), rm.encoding())
            }
            Instruction::Eor { rd, rn, rm, cond } => {
                encode_dp_reg(*cond, DP_EOR, false, rn.encoding(), rd.encoding(), rm.encoding())
            }
            Instruction::Bic { rd, rn, rm, cond } => {
                encode_dp_reg(*cond, DP_BIC, false, rn.encoding(), rd.encoding(), rm.encoding())
            }
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
            Instruction::AddImm { rd, rn, rotate, imm8, cond } => {
                encode_dp_imm(
                    *cond,
                    DP_ADD,
                    false,
                    rn.encoding(),
                    rd.encoding(),
                    *rotate,
                    *imm8,
                )
            }
            Instruction::SubImm { rd, rn, rotate, imm8, cond } => {
                encode_dp_imm(
                    *cond,
                    DP_SUB,
                    false,
                    rn.encoding(),
                    rd.encoding(),
                    *rotate,
                    *imm8,
                )
            }
            Instruction::MovImm { rd, rotate, imm8, cond } => {
                encode_dp_imm(*cond, DP_MOV, false, 0, rd.encoding(), *rotate, *imm8)
            }
            Instruction::CmpImm { rn, rotate, imm8, cond } => {
                encode_dp_imm(*cond, DP_CMP, true, rn.encoding(), 0, *rotate, *imm8)
            }

            // ── Shift by Immediate ──────────────────────────────────
            Instruction::LslImm { rd, rm, shift_imm, cond } => {
                // LSL = shift_type 0, encoded as MOV Rd, Rm, LSL #imm
                encode_dp_shift_imm(*cond, DP_MOV, false, 0, rd.encoding(), 0, *shift_imm, rm.encoding())
            }
            Instruction::LsrImm { rd, rm, shift_imm, cond } => {
                // LSR = shift_type 1
                encode_dp_shift_imm(*cond, DP_MOV, false, 0, rd.encoding(), 1, *shift_imm, rm.encoding())
            }
            Instruction::AsrImm { rd, rm, shift_imm, cond } => {
                // ASR = shift_type 2
                encode_dp_shift_imm(*cond, DP_MOV, false, 0, rd.encoding(), 2, *shift_imm, rm.encoding())
            }
            Instruction::RorImm { rd, rm, shift_imm, cond } => {
                // ROR = shift_type 3
                encode_dp_shift_imm(*cond, DP_MOV, false, 0, rd.encoding(), 3, *shift_imm, rm.encoding())
            }

            // ── Shift by Register ───────────────────────────────────
            Instruction::LslReg { rd, rn, rs, cond } => {
                encode_dp_shift_reg(*cond, DP_MOV, false, 0, rd.encoding(), 0, rs.encoding(), rn.encoding())
            }
            Instruction::LsrReg { rd, rn, rs, cond } => {
                encode_dp_shift_reg(*cond, DP_MOV, false, 0, rd.encoding(), 1, rs.encoding(), rn.encoding())
            }
            Instruction::AsrReg { rd, rn, rs, cond } => {
                encode_dp_shift_reg(*cond, DP_MOV, false, 0, rd.encoding(), 2, rs.encoding(), rn.encoding())
            }
            Instruction::RorReg { rd, rn, rs, cond } => {
                encode_dp_shift_reg(*cond, DP_MOV, false, 0, rd.encoding(), 3, rs.encoding(), rn.encoding())
            }

            // ── Multiply ────────────────────────────────────────────
            Instruction::Mul { rd, rn, rs, rm, cond } => {
                encode_mul(*cond, false, rd.encoding(), rn.encoding(), rs.encoding(), rm.encoding())
            }
            Instruction::Mla { rd, rn, rs, rm, cond } => {
                encode_mla(*cond, false, rd.encoding(), rn.encoding(), rs.encoding(), rm.encoding())
            }
            Instruction::Umull { rd_hi, rd_lo, rs, rm, cond } => {
                encode_umull(*cond, false, rd_hi.encoding(), rd_lo.encoding(), rs.encoding(), rm.encoding())
            }
            Instruction::Smull { rd_hi, rd_lo, rs, rm, cond } => {
                encode_smull(*cond, false, rd_hi.encoding(), rd_lo.encoding(), rs.encoding(), rm.encoding())
            }

            // ── Load/Store Word ─────────────────────────────────────
            Instruction::Ldr { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(*cond, true, u, false, false, true, rn.encoding(), rd.encoding(), off)
            }
            Instruction::Str { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(*cond, true, u, false, false, false, rn.encoding(), rd.encoding(), off)
            }

            // ── Load/Store Byte ─────────────────────────────────────
            Instruction::Ldrb { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(*cond, true, u, true, false, true, rn.encoding(), rd.encoding(), off)
            }
            Instruction::Strb { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_imm(*cond, true, u, true, false, false, rn.encoding(), rd.encoding(), off)
            }

            // ── Load/Store Halfword ─────────────────────────────────
            Instruction::Ldrh { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_half_imm(*cond, true, u, false, true, rn.encoding(), rd.encoding(), off)
            }
            Instruction::Strh { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_half_imm(*cond, true, u, false, false, rn.encoding(), rd.encoding(), off)
            }

            // ── Load/Store Doubleword ───────────────────────────────
            Instruction::Ldrd { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_double_imm(*cond, true, u, false, true, rn.encoding(), rd.encoding(), off)
            }
            Instruction::Strd { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ls_double_imm(*cond, true, u, false, false, rn.encoding(), rd.encoding(), off)
            }

            // ── Load Signed Byte/Halfword ───────────────────────────
            Instruction::Ldrsb { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ldrsb_imm(*cond, true, u, false, rn.encoding(), rd.encoding(), off)
            }
            Instruction::Ldrsh { rd, rn, offset, cond } => {
                let (u, off) = if *offset >= 0 {
                    (true, *offset as u32)
                } else {
                    (false, (-*offset) as u32)
                };
                encode_ldrsh_imm(*cond, true, u, false, rn.encoding(), rd.encoding(), off)
            }

            // ── Load/Store Multiple ─────────────────────────────────
            Instruction::Ldm { rn, register_list, writeback, cond } => {
                // LDM = Increment After (P=0, U=1) — typical IA variant
                encode_ldm(*cond, false, true, false, *writeback, rn.encoding(), *register_list)
            }
            Instruction::Stm { rn, register_list, writeback, cond } => {
                // STM = Decrement Before (P=1, U=0) — typical DB (push) variant
                encode_stm(*cond, true, false, false, *writeback, rn.encoding(), *register_list)
            }

            // ── Branch ──────────────────────────────────────────────
            Instruction::B { offset, cond } => {
                encode_branch(*cond, false, *offset)
            }
            Instruction::Bl { offset, cond } => {
                encode_branch(*cond, true, *offset)
            }
            Instruction::Bx { rm, cond } => {
                encode_bx(*cond, rm.encoding())
            }
            Instruction::BlxReg { rm, cond } => {
                encode_blx_reg(*cond, rm.encoding())
            }

            // ── System ──────────────────────────────────────────────
            Instruction::Svc { imm24, cond } => {
                encode_svc(*cond, *imm24)
            }
            Instruction::Nop => {
                // NOP = MOV R0, R0 = 0xE1A00000
                0xE1A0_0000u32.to_le_bytes()
            }
            Instruction::Mrs { rd, spsr, cond } => {
                encode_mrs(*cond, rd.encoding(), *spsr)
            }
            Instruction::Msr { mask, rm, cond } => {
                encode_msr(*cond, *mask, rm.encoding())
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
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Add { rd, rn, rm, cond } => write!(f, "add{} {}, {}, {}", cond, rd, rn, rm),
            Instruction::Sub { rd, rn, rm, cond } => write!(f, "sub{} {}, {}, {}", cond, rd, rn, rm),
            Instruction::And { rd, rn, rm, cond } => write!(f, "and{} {}, {}, {}", cond, rd, rn, rm),
            Instruction::Orr { rd, rn, rm, cond } => write!(f, "orr{} {}, {}, {}", cond, rd, rn, rm),
            Instruction::Eor { rd, rn, rm, cond } => write!(f, "eor{} {}, {}, {}", cond, rd, rn, rm),
            Instruction::Bic { rd, rn, rm, cond } => write!(f, "bic{} {}, {}, {}", cond, rd, rn, rm),
            Instruction::Mov { rd, rm, cond } => write!(f, "mov{} {}, {}", cond, rd, rm),
            Instruction::Mvn { rd, rm, cond } => write!(f, "mvn{} {}, {}", cond, rd, rm),
            Instruction::Cmp { rn, rm, cond } => write!(f, "cmp{} {}, {}", cond, rn, rm),
            Instruction::Cmn { rn, rm, cond } => write!(f, "cmn{} {}, {}", cond, rn, rm),
            Instruction::Tst { rn, rm, cond } => write!(f, "tst{} {}, {}", cond, rn, rm),
            Instruction::Teq { rn, rm, cond } => write!(f, "teq{} {}, {}", cond, rn, rm),
            Instruction::AddImm { rd, rn, rotate: _, imm8, cond } => {
                write!(f, "add{} {}, {}, #{}", cond, rd, rn, imm8)
            }
            Instruction::SubImm { rd, rn, rotate: _, imm8, cond } => {
                write!(f, "sub{} {}, {}, #{}", cond, rd, rn, imm8)
            }
            Instruction::MovImm { rd, rotate: _, imm8, cond } => {
                write!(f, "mov{} {}, #{}", cond, rd, imm8)
            }
            Instruction::CmpImm { rn, rotate: _, imm8, cond } => {
                write!(f, "cmp{} {}, #{}", cond, rn, imm8)
            }
            Instruction::LslImm { rd, rm, shift_imm, cond } => {
                write!(f, "lsl{} {}, {}, #{}", cond, rd, rm, shift_imm)
            }
            Instruction::LsrImm { rd, rm, shift_imm, cond } => {
                write!(f, "lsr{} {}, {}, #{}", cond, rd, rm, shift_imm)
            }
            Instruction::AsrImm { rd, rm, shift_imm, cond } => {
                write!(f, "asr{} {}, {}, #{}", cond, rd, rm, shift_imm)
            }
            Instruction::RorImm { rd, rm, shift_imm, cond } => {
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
            Instruction::Mul { rd, rn, rs, rm: _, cond } => {
                write!(f, "mul{} {}, {}, {}", cond, rd, rn, rs)
            }
            Instruction::Mla { rd, rn, rs, rm, cond } => {
                write!(f, "mla{} {}, {}, {}, {}", cond, rd, rn, rm, rs)
            }
            Instruction::Umull { rd_hi, rd_lo, rs, rm, cond } => {
                write!(f, "umull{} {}, {}, {}, {}", cond, rd_lo, rd_hi, rm, rs)
            }
            Instruction::Smull { rd_hi, rd_lo, rs, rm, cond } => {
                write!(f, "smull{} {}, {}, {}, {}", cond, rd_lo, rd_hi, rm, rs)
            }
            Instruction::Ldr { rd, rn, offset, cond } => {
                write!(f, "ldr{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Str { rd, rn, offset, cond } => {
                write!(f, "str{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrb { rd, rn, offset, cond } => {
                write!(f, "ldrb{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Strb { rd, rn, offset, cond } => {
                write!(f, "strb{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrh { rd, rn, offset, cond } => {
                write!(f, "ldrh{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Strh { rd, rn, offset, cond } => {
                write!(f, "strh{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrd { rd, rn, offset, cond } => {
                write!(f, "ldrd{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Strd { rd, rn, offset, cond } => {
                write!(f, "strd{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrsb { rd, rn, offset, cond } => {
                write!(f, "ldrsb{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldrsh { rd, rn, offset, cond } => {
                write!(f, "ldrsh{} {}, [{}, #{}]", cond, rd, rn, offset)
            }
            Instruction::Ldm { rn, register_list, writeback, cond } => {
                write!(
                    f,
                    "ldm{} {}{}, {{{:#06x}}}",
                    cond,
                    rn,
                    if *writeback { "!" } else { "" },
                    register_list
                )
            }
            Instruction::Stm { rn, register_list, writeback, cond } => {
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
        }
    }
}

// ===========================================================================
// ELF32 Emission
// ===========================================================================

/// Build a minimal ELF32 binary for ARM from raw code bytes.
///
/// Produces a static executable with a single LOAD segment containing the
/// `.text` section.  Entry point is at `base_addr` + header offset.
fn build_minimal_arm32_elf(code: &[u8], base_addr: u64) -> Vec<u8> {
    // Layout: ELF header (52) | 1 program header (32) | code
    let elf_header_size: u64 = 52;
    let phdr_size: u64 = 32;
    let text_offset = elf_header_size + phdr_size;
    let text_size = code.len() as u64;
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity(text_offset as usize + code.len());

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
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&52u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&32u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&1u16.to_le_bytes()); // e_phnum
    elf.extend_from_slice(&40u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header (32-bit, single LOAD segment: PF_R | PF_X) ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&(text_offset as u32).to_le_bytes()); // p_offset
    elf.extend_from_slice(&((base_addr + text_offset) as u32).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&((base_addr + text_offset) as u32).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&(text_size as u32).to_le_bytes()); // p_filesz
    elf.extend_from_slice(&(text_size as u32).to_le_bytes()); // p_memsz
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&4u32.to_le_bytes()); // p_align

    // --- Code section ---
    elf.extend_from_slice(code);

    elf
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
        code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_MOV, false, 0, rd.encoding(), rotate, imm8));
        return code;
    }

    // Try MVN: if ~val can be encoded as a rotated immediate, use MVN Rd, #~val
    let inv = !val;
    if let Some((rotate, imm8)) = try_encode_arm_imm(inv) {
        code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_MVN, false, 0, rd.encoding(), rotate, imm8));
        return code;
    }

    // Split into two 16-bit halves and use MOV + ORR
    let lo = val & 0xFFFF;
    let hi = (val >> 16) & 0xFFFF;

    // Load the low half
    if lo == 0 {
        // MOV Rd, #0
        code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_MOV, false, 0, rd.encoding(), 0, 0));
    } else if let Some((rot, imm8)) = try_encode_arm_imm(lo) {
        code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_MOV, false, 0, rd.encoding(), rot, imm8));
    } else {
        // Further split lo into two bytes and use ORR
        let lo_lo = lo & 0xFF;
        let lo_hi = (lo >> 8) & 0xFF;
        code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_MOV, false, 0, rd.encoding(), 0, lo_lo));
        if lo_hi != 0 {
            // lo_hi << 8 = lo_hi rotated right by 24 → rotate=12
            code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_ORR, false, rd.encoding(), rd.encoding(), 12, lo_hi));
        }
    }

    // ORR in the high half
    if hi != 0 {
        if let Some((rot, imm8)) = try_encode_arm_imm(hi << 16) {
            code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_ORR, false, rd.encoding(), rd.encoding(), rot, imm8));
        } else {
            // Split hi into two bytes
            let hi_lo = hi & 0xFF;
            let hi_hi = (hi >> 8) & 0xFF;
            if hi_lo != 0 {
                // hi_lo << 16 = hi_lo rotated right by 16 → rotate=8
                code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_ORR, false, rd.encoding(), rd.encoding(), 8, hi_lo));
            }
            if hi_hi != 0 {
                // hi_hi << 24 = hi_hi rotated right by 8 → rotate=4
                code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_ORR, false, rd.encoding(), rd.encoding(), 4, hi_hi));
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
    scratch: Gpr,
) -> (Gpr, Vec<u8>) {
    match val {
        crate::ir::IRValue::Register(id) => (
            reg_map.get(id).copied().unwrap_or(Gpr::R0),
            Vec::new(),
        ),
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
        0b0000 => "eq", 0b0001 => "ne", 0b0010 => "cs", 0b0011 => "cc",
        0b0100 => "mi", 0b0101 => "pl", 0b0110 => "vs", 0b0111 => "vc",
        0b1000 => "hi", 0b1001 => "ls", 0b1010 => "ge", 0b1011 => "lt",
        0b1100 => "gt", 0b1101 => "le", 0b1110 => "", 0b1111 => "nv",
        _ => "??",
    };
    let cond_suffix = if cond_str.is_empty() { String::new() } else { format!(".{}", cond_str) };

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
        // Data processing
        0b00 => {
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
                        0 => "lsl", 1 => "lsr", 2 => "asr", 3 => "ror", _ => "???",
                    };
                    format!(", {} #{}", st, shift_imm)
                };
                match opcode {
                    0b0000 => format!("and{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                    0b0001 => format!("eor{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                    0b0010 => format!("sub{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                    0b0011 => format!("rsb{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                    0b0100 => format!("add{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                    0b1000 if s_bit == 1 && rd == 0 => format!("tst{} r{}, r{}{}", cond_suffix, rn, rm, shift_str),
                    0b1001 if s_bit == 1 && rd == 0 => format!("teq{} r{}, r{}{}", cond_suffix, rn, rm, shift_str),
                    0b1010 if s_bit == 1 && rd == 0 => format!("cmp{} r{}, r{}{}", cond_suffix, rn, rm, shift_str),
                    0b1011 if s_bit == 1 && rd == 0 => format!("cmn{} r{}, r{}{}", cond_suffix, rn, rm, shift_str),
                    0b1100 => format!("orr{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                    0b1101 if rn == 0 => format!("mov{} r{}, r{}{}", cond_suffix, rd, rm, shift_str),
                    0b1110 => format!("bic{} r{}, r{}, r{}{}", cond_suffix, rd, rn, rm, shift_str),
                    0b1111 if rn == 0 => format!("mvn{} r{}, r{}{}", cond_suffix, rd, rm, shift_str),
                    _ => format!(".word {:08x}", word),
                }
            }
        }
        // Load/Store word/byte
        0b01 => {
            let l_bit = (word >> 20) & 1;
            let b_bit = (word >> 22) & 1;
            let u_bit = (word >> 23) & 1;
            let offset_val = imm12;
            let off_str = if u_bit == 1 { format!("#{}", offset_val) } else { format!("#-{}", offset_val) };
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
        // Simple round-robin register allocation over allocatable GPRs.
        let allocatable: Vec<Gpr> = [
            Gpr::R0, Gpr::R1, Gpr::R2, Gpr::R3,
            Gpr::R4, Gpr::R5, Gpr::R6, Gpr::R7,
            Gpr::R8, Gpr::R9, Gpr::R10, Gpr::R11,
            Gpr::R12,
        ]
        .to_vec();

        let func_name = func.name.clone();
        let frame_size = arm32_compute_frame_size(func);

        // Collect all virtual register IDs
        let mut vreg_ids: Vec<u32> = Vec::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                for id in instr.defined_regs() {
                    if !vreg_ids.contains(&id) {
                        vreg_ids.push(id);
                    }
                }
            }
        }

        // Map virtual registers to physical registers
        let mut reg_map: std::collections::HashMap<u32, Gpr> = std::collections::HashMap::new();
        for (i, &id) in vreg_ids.iter().enumerate() {
            reg_map.insert(id, allocatable[i % allocatable.len()]);
        }

        // Determine callee-saved registers used
        let callee_saved: Vec<PhysicalReg> = reg_map
            .values()
            .filter(|r| r.is_callee_saved())
            .map(|r| PhysicalReg::new(RegClass::Gpr, r.encoding()))
            .collect();

        // Generate prologue
        let mut encoded_instrs: Vec<AllocatedInstruction> = Vec::new();

        // PUSH {R11, LR} — STM DB SP!, {R11, LR}
        // register_list: R11=bit11, LR=bit14 → 0x4800
        let push_bytes = encode_stm(Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x4800);
        encoded_instrs.push(AllocatedInstruction {
            opcode: "push".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
            encoded: push_bytes.to_vec(),
        });

        // MOV R11, SP
        let mov_bytes = encode_dp_reg(Condition::Al, DP_MOV, false, 0, Gpr::R11.encoding(), Gpr::R13.encoding());
        encoded_instrs.push(AllocatedInstruction {
            opcode: "mov".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R11.encoding())],
            encoded: mov_bytes.to_vec(),
        });

        // SUB SP, SP, #frame_size
        if frame_size > 0 {
            let sub_bytes = encode_dp_imm(Condition::Al, DP_SUB, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, frame_size as u32 & 0xFF);
            encoded_instrs.push(AllocatedInstruction {
                opcode: "sub".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::R13.encoding())],
                encoded: sub_bytes.to_vec(),
            });
        }

        // Encode each IR instruction
        for block in &func.blocks {
            for instr in &block.instructions {
                let encoded = match instr {
                    crate::ir::IRInstr::Add { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (l, mut code) = resolve_gpr_arm32(lhs, &reg_map, Gpr::R12);
                        // Optimise: if rhs is an immediate that fits in ARM rotated form, use ADD imm
                        if let crate::ir::IRValue::Immediate(imm) = rhs {
                            if let Some((rotate, imm8)) = try_encode_arm_imm(*imm as u32) {
                                if l != d {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding()));
                                }
                                code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_ADD, false, l.encoding(), d.encoding(), rotate, imm8));
                                code
                            } else {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding()));
                                }
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_ADD, false, l.encoding(), d.encoding(), r.encoding()));
                                code
                            }
                        } else {
                            let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                            code.extend_from_slice(&pre);
                            if l != d {
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding()));
                            }
                            code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_ADD, false, l.encoding(), d.encoding(), r.encoding()));
                            code
                        }
                    }
                    crate::ir::IRInstr::Sub { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (l, mut code) = resolve_gpr_arm32(lhs, &reg_map, Gpr::R12);
                        if let crate::ir::IRValue::Immediate(imm) = rhs {
                            if let Some((rotate, imm8)) = try_encode_arm_imm(*imm as u32) {
                                if l != d {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding()));
                                }
                                code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_SUB, false, l.encoding(), d.encoding(), rotate, imm8));
                                code
                            } else {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding()));
                                }
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, l.encoding(), d.encoding(), r.encoding()));
                                code
                            }
                        } else {
                            let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                            code.extend_from_slice(&pre);
                            if l != d {
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding()));
                            }
                            code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_SUB, false, l.encoding(), d.encoding(), r.encoding()));
                            code
                        }
                    }
                    crate::ir::IRInstr::Mul { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (l, mut code) = resolve_gpr_arm32(lhs, &reg_map, Gpr::R12);
                        let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                        code.extend_from_slice(&pre);
                        // MUL: we need l in d, then MUL d, d, r (Rd=Rn in MUL encoding)
                        if l != d {
                            code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding()));
                        }
                        code.extend_from_slice(&encode_mul(Condition::Al, false, d.encoding(), d.encoding(), r.encoding(), d.encoding()));
                        code
                    }
                    crate::ir::IRInstr::Div { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (l, mut code) = resolve_gpr_arm32(lhs, &reg_map, Gpr::R0);
                        let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R1);
                        code.extend_from_slice(&pre);
                        // ARM32 baseline doesn't have hardware divide; set up r0/r1 and use SVC
                        if l != Gpr::R0 {
                            code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), l.encoding()));
                        }
                        if r != Gpr::R1 {
                            code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), r.encoding()));
                        }
                        code.extend_from_slice(&encode_svc(Condition::Al, 0));
                        if d != Gpr::R0 {
                            code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), Gpr::R0.encoding()));
                        }
                        code
                    }
                    crate::ir::IRInstr::BinOp { op, dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (l, mut code) = resolve_gpr_arm32(lhs, &reg_map, Gpr::R12);
                        match op {
                            BinOpKind::And => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_AND, false, l.encoding(), d.encoding(), r.encoding()));
                            }
                            BinOpKind::Or => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_ORR, false, l.encoding(), d.encoding(), r.encoding()));
                            }
                            BinOpKind::Xor => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_EOR, false, l.encoding(), d.encoding(), r.encoding()));
                            }
                            BinOpKind::Shl => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                // LSL Rd, Rn, Rs: shift_type=0, by register
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), 0, r.encoding(), l.encoding()));
                            }
                            BinOpKind::ShrL => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                // LSR Rd, Rn, Rs: shift_type=1, by register
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), 1, r.encoding(), l.encoding()));
                            }
                            BinOpKind::ShrA => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                // ASR Rd, Rn, Rs: shift_type=2, by register
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), 2, r.encoding(), l.encoding()));
                            }
                            BinOpKind::Add | BinOpKind::Sub => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                let arm_op = match op {
                                    BinOpKind::Add => DP_ADD,
                                    BinOpKind::Sub => DP_SUB,
                                    _ => DP_ADD,
                                };
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, arm_op, false, l.encoding(), d.encoding(), r.encoding()));
                            }
                            BinOpKind::Mul => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if l != d { code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), l.encoding())); }
                                code.extend_from_slice(&encode_mul(Condition::Al, false, d.encoding(), d.encoding(), r.encoding(), d.encoding()));
                            }
                            BinOpKind::SDiv | BinOpKind::UDiv | BinOpKind::SRem | BinOpKind::URem => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                // Set up r0=l, r1=r for software div/rem via SVC
                                if l != Gpr::R0 {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), l.encoding()));
                                }
                                if r != Gpr::R1 {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), r.encoding()));
                                }
                                code.extend_from_slice(&encode_svc(Condition::Al, 0));
                                if d != Gpr::R0 {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), Gpr::R0.encoding()));
                                }
                            }
                            // Comparison BinOps: produce 0 or 1
                            BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
                            | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
                            | BinOpKind::Eq | BinOpKind::Ne => {
                                let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                // CMP l, r; MOV d, #0; MOVcond d, #1
                                let cmp_cond = match op {
                                    BinOpKind::SLt | BinOpKind::ULt => Condition::Lt,
                                    BinOpKind::SLe | BinOpKind::ULe => Condition::Le,
                                    BinOpKind::SGt | BinOpKind::UGt => Condition::Gt,
                                    BinOpKind::SGe | BinOpKind::UGe => Condition::Ge,
                                    BinOpKind::Eq => Condition::Eq,
                                    BinOpKind::Ne => Condition::Ne,
                                    _ => Condition::Eq,
                                };
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_CMP, true, l.encoding(), 0, r.encoding()));
                                code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_MOV, false, 0, d.encoding(), 0, 0));
                                code.extend_from_slice(&encode_dp_imm(cmp_cond, DP_MOV, false, 0, d.encoding(), 0, 1));
                            }
                        }
                        code
                    }
                    crate::ir::IRInstr::UnaryOp { op, dst, operand } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (s, mut code) = resolve_gpr_arm32(operand, &reg_map, Gpr::R12);
                        match op {
                            UnaryOpKind::Neg => {
                                // RSB d, s, #0 (reverse subtract)
                                code.extend_from_slice(&encode_dp_imm(Condition::Al, 0b0011, false, s.encoding(), d.encoding(), 0, 0));
                            }
                            UnaryOpKind::Not => {
                                // MVN d, s
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MVN, false, 0, d.encoding(), s.encoding()));
                            }
                            UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                                // Placeholder: MOV d, s
                                if s != d {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d.encoding(), s.encoding()));
                                }
                            }
                        }
                        code
                    }
                    crate::ir::IRInstr::Cmp { kind, dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (l, mut code) = resolve_gpr_arm32(lhs, &reg_map, Gpr::R12);
                        let (r, pre) = resolve_gpr_arm32(rhs, &reg_map, Gpr::R3);
                        code.extend_from_slice(&pre);
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
                        code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_CMP, true, l.encoding(), 0, r.encoding()));
                        code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_MOV, false, 0, d.encoding(), 0, 0));
                        code.extend_from_slice(&encode_dp_imm(cmp_cond, DP_MOV, false, 0, d.encoding(), 0, 1));
                        code
                    }
                    crate::ir::IRInstr::Load { dst, addr } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (a, mut code) = resolve_gpr_arm32(addr, &reg_map, Gpr::R12);
                        code.extend_from_slice(&encode_ls_imm(Condition::Al, true, true, false, false, true, a.encoding(), d.encoding(), 0));
                        code
                    }
                    crate::ir::IRInstr::Store { value, addr } => {
                        let (v, mut code) = resolve_gpr_arm32(value, &reg_map, Gpr::R12);
                        let (a, pre) = resolve_gpr_arm32(addr, &reg_map, Gpr::R3);
                        code.extend_from_slice(&pre);
                        code.extend_from_slice(&encode_ls_imm(Condition::Al, true, true, false, false, false, a.encoding(), v.encoding(), 0));
                        code
                    }
                    crate::ir::IRInstr::Alloc { dst, size: _ } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        // Point to frame area: ADD d, R11, #0
                        encode_dp_imm(Condition::Al, DP_ADD, false, Gpr::R11.encoding(), d.encoding(), 0, 0).to_vec()
                    }
                    crate::ir::IRInstr::Call { dst, func: _, args } => {
                        let mut code = Vec::new();
                        // Move args to argument registers using resolve_gpr_arm32
                        for (i, arg) in args.iter().enumerate() {
                            if let Some(arg_reg) = Gpr::arg_register(i) {
                                let (a, pre) = resolve_gpr_arm32(arg, &reg_map, arg_reg);
                                code.extend_from_slice(&pre);
                                if a != arg_reg {
                                    code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, arg_reg.encoding(), a.encoding()));
                                }
                            }
                        }
                        // BL offset (placeholder)
                        code.extend_from_slice(&encode_branch(Condition::Al, true, 0));
                        // Move return value from R0 to dst
                        if let Some(d) = dst {
                            let d_reg = reg_map.get(&d.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                            if d_reg != Gpr::R0 {
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, d_reg.encoding(), Gpr::R0.encoding()));
                            }
                        }
                        code
                    }
                    crate::ir::IRInstr::Branch { target: _ } => {
                        // B offset (placeholder)
                        encode_branch(Condition::Al, false, 0).to_vec()
                    }
                    crate::ir::IRInstr::CondBranch { cond, true_target: _, false_target: _ } => {
                        let (c, mut code) = resolve_gpr_arm32(cond, &reg_map, Gpr::R12);
                        // CMP c, #0; BNE true_target; B false_target
                        code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_CMP, true, c.encoding(), 0, 0, 0));
                        code.extend_from_slice(&encode_branch(Condition::Ne, false, 0));
                        code.extend_from_slice(&encode_branch(Condition::Al, false, 0));
                        code
                    }
                    crate::ir::IRInstr::Cast { dst, src, .. } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (s, mut code) = resolve_gpr_arm32(src, &reg_map, Gpr::R12);
                        // Bitwise reinterpret: just move the register
                        if s != d {
                            code.extend_from_slice(&Instruction::Mov { rd: d, rm: s, cond: Condition::Al }.encode());
                        }
                        code
                    }
                    crate::ir::IRInstr::Select { dst, cond, true_val, false_val } => {
                        // dst = if cond != 0 { true_val } else { false_val }
                        // Lowered as: MOV dst, false_val; CMP cond, #0; MOVNE dst, true_val
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (c, mut code) = resolve_gpr_arm32(cond, &reg_map, Gpr::R12);
                        let (tv, pre_tv) = resolve_gpr_arm32(true_val, &reg_map, Gpr::R3);
                        let (fv, pre_fv) = resolve_gpr_arm32(false_val, &reg_map, Gpr::R2);
                        // MOV dst, fv
                        code.extend_from_slice(&pre_fv);
                        if fv != d {
                            code.extend_from_slice(&Instruction::Mov { rd: d, rm: fv, cond: Condition::Al }.encode());
                        } else if fv == d {
                            // fv is already in d, nothing needed
                        }
                        // CMP c, #0
                        code.extend_from_slice(&Instruction::CmpImm { rn: c, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
                        // MOVNE dst, tv
                        code.extend_from_slice(&pre_tv);
                        code.extend_from_slice(&Instruction::Mov { rd: d, rm: tv, cond: Condition::Ne }.encode());
                        code
                    }
                    crate::ir::IRInstr::Offset { dst, base, offset } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        let (b, mut code) = resolve_gpr_arm32(base, &reg_map, Gpr::R12);
                        match offset {
                            crate::ir::IRValue::Immediate(imm) => {
                                let off = *imm as u32;
                                // Try ADD imm form first
                                if let Some((rotate, imm8)) = try_encode_arm_imm(off) {
                                    if b != d {
                                        code.extend_from_slice(&Instruction::Mov { rd: d, rm: b, cond: Condition::Al }.encode());
                                    }
                                    code.extend_from_slice(&Instruction::AddImm { rd: d, rn: d, rotate, imm8, cond: Condition::Al }.encode());
                                } else {
                                    // Load offset to scratch, then ADD reg
                                    code.extend_from_slice(&load_immediate_arm32(Gpr::R3, off));
                                    if b != d {
                                        code.extend_from_slice(&Instruction::Mov { rd: d, rm: b, cond: Condition::Al }.encode());
                                    }
                                    code.extend_from_slice(&Instruction::Add { rd: d, rn: d, rm: Gpr::R3, cond: Condition::Al }.encode());
                                }
                            }
                            _ => {
                                let (o, pre) = resolve_gpr_arm32(offset, &reg_map, Gpr::R3);
                                code.extend_from_slice(&pre);
                                if b != d {
                                    code.extend_from_slice(&Instruction::Mov { rd: d, rm: b, cond: Condition::Al }.encode());
                                }
                                code.extend_from_slice(&Instruction::Add { rd: d, rn: d, rm: o, cond: Condition::Al }.encode());
                            }
                        }
                        code
                    }
                    crate::ir::IRInstr::GetAddress { dst, name: _ } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::R0);
                        // Placeholder: load 0 as the address (relocation needed at link time)
                        load_immediate_arm32(d, 0)
                    }
                    crate::ir::IRInstr::Free { ptr: _ } => {
                        // Free is lowered to a runtime call; emit a NOP as placeholder
                        Instruction::Nop.encode().to_vec()
                    }
                    crate::ir::IRInstr::Phi { .. } => {
                        // Phi nodes are eliminated during register allocation; no code to emit
                        Vec::new()
                    }
                    crate::ir::IRInstr::Ret { values } => {
                        let mut code = Vec::new();
                        // Move return value to R0 if needed
                        if let Some(val) = values.first() {
                            let (src, pre) = resolve_gpr_arm32(val, &reg_map, Gpr::R0);
                            code.extend_from_slice(&pre);
                            if src != Gpr::R0 {
                                code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), src.encoding()));
                            }
                        }
                        // Epilogue: MOV SP, R11; POP {R11, PC}
                        code.extend_from_slice(&encode_dp_reg(Condition::Al, DP_MOV, false, 0, Gpr::R13.encoding(), Gpr::R11.encoding()));
                        // POP {R11, PC} — LDM IA SP!, {R11, PC}
                        // register_list: R11=bit11, PC=bit15 → 0x8800
                        code.extend_from_slice(&encode_ldm(Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x8800));
                        code
                    }

                };

                encoded_instrs.push(AllocatedInstruction {
                    opcode: "arm32".to_string(),
                    reads: vec![],
                    writes: vec![],
                    encoded,
                });
            }
        }

        let code_size = encoded_instrs.len() * 4;

        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions: encoded_instrs,
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

        // Wrap in a minimal ELF32 binary for ARM.
        Ok(build_minimal_arm32_elf(&all_code, 0x10000))
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
        let ldr_bytes = encode_ls_imm(Condition::Al, true, true, false, false, true, Gpr::R15.encoding(), Gpr::R12.encoding(), 4);
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

#[cfg(test)]
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
        let instr = Instruction::Add { rd: Gpr::R0, rn: Gpr::R1, rm: Gpr::R2, cond: Condition::Al };
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
        let instr = Instruction::Sub { rd: Gpr::R3, rn: Gpr::R4, rm: Gpr::R5, cond: Condition::Al };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00, I=0, opcode=0010(SUB), S=0, Rn=0100, Rd=0011, 00000000, Rm=0101
        let expected = 0xE0443005u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_mov_reg_encoding() {
        // MOV R0, R1 (AL) — Rn should be 0 (SBZ)
        let instr = Instruction::Mov { rd: Gpr::R0, rm: Gpr::R1, cond: Condition::Al };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00, I=0, opcode=1101(MOV), S=0, Rn=0000, Rd=0000, 00000000, Rm=0001
        let expected = 0xE1A00001u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_cmp_reg_encoding() {
        // CMP R0, R1 (AL) — Rd=0 (SBZ), S=1
        let instr = Instruction::Cmp { rn: Gpr::R0, rm: Gpr::R1, cond: Condition::Al };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 00, I=0, opcode=1010(CMP), S=1, Rn=0000, Rd=0000, 00000000, Rm=0001
        let expected = 0xE1500001u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_conditional_add() {
        // ADD R0, R1, R2 (EQ)
        let instr = Instruction::Add { rd: Gpr::R0, rn: Gpr::R1, rm: Gpr::R2, cond: Condition::Eq };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=0000 instead of 1110
        let expected = 0x00810002u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_ldr_encoding() {
        // LDR R0, [R1, #8] (AL)
        let instr = Instruction::Ldr { rd: Gpr::R0, rn: Gpr::R1, offset: 8, cond: Condition::Al };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 01, I=0, P=1, U=1, B=0, W=0, L=1, Rn=0001, Rd=0000, offset=000000001000
        let expected = 0xE5910008u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_str_encoding() {
        // STR R0, [R1, #-4] (AL)
        let instr = Instruction::Str { rd: Gpr::R0, rn: Gpr::R1, offset: -4, cond: Condition::Al };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 01, I=0, P=1, U=0, B=0, W=0, L=0, Rn=0001, Rd=0000, offset=000000000100
        let expected = 0xE5010004u32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_ldrb_encoding() {
        // LDRB R0, [R1, #0] (AL)
        let instr = Instruction::Ldrb { rd: Gpr::R0, rn: Gpr::R1, offset: 0, cond: Condition::Al };
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
        let instr = Instruction::Bx { rm: Gpr::R14, cond: Condition::Al };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        let expected = 0xE12FFF1Eu32;
        assert_eq!(word, expected);
    }

    #[test]
    fn test_mul_encoding() {
        // MUL R0, R1, R2 (AL) — Rd=R0, Rn=R1, Rs=R2, Rm=R1
        let instr = Instruction::Mul { rd: Gpr::R0, rn: Gpr::R1, rs: Gpr::R2, rm: Gpr::R1, cond: Condition::Al };
        let bytes = instr.encode();
        let word = u32::from_le_bytes(bytes);
        // cond=1110, 000000, S=0, Rd[19:16]=0000, Rn[15:12]=0001, Rs[11:8]=0010, 1001, Rm[3:0]=0001
        let expected = 0xE0001291u32;
        assert_eq!(word, expected);
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
            }],
        };
        let result = backend.allocate_registers(&func).unwrap();
        // Should have: prologue (PUSH, MOV FP, SUB SP) + ADD imm + MOV R0 + epilogue
        // The ADD imm should use the immediate form, not load into scratch first
        let all_code: Vec<u8> = result.blocks.iter()
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
        assert!(found_add_imm, "Expected ADD immediate instruction in generated code");
    }

    #[test]
    fn test_isel_resolve_gpr_immediate() {
        // resolve_gpr_arm32 with an immediate should load it into the scratch register
        let mut reg_map = std::collections::HashMap::new();
        reg_map.insert(0, Gpr::R0);
        reg_map.insert(1, Gpr::R1);

        // Register value: should return the mapped register with no pre-code
        let (gpr, pre_code) = resolve_gpr_arm32(
            &crate::ir::IRValue::Register(0),
            &reg_map,
            Gpr::R12,
        );
        assert_eq!(gpr, Gpr::R0);
        assert!(pre_code.is_empty());

        // Immediate value 0: should load into scratch with a single MOV Rd, #0
        let (gpr, pre_code) = resolve_gpr_arm32(
            &crate::ir::IRValue::Immediate(0),
            &reg_map,
            Gpr::R12,
        );
        assert_eq!(gpr, Gpr::R12);
        assert_eq!(pre_code.len(), 4); // single MOV instruction
        let word = u32::from_le_bytes([pre_code[0], pre_code[1], pre_code[2], pre_code[3]]);
        // MOV R12, #0 = cond=1110, 001, opcode=1101, S=0, Rn=0, Rd=12, rotate=0, imm8=0
        assert_eq!(word, 0xE3A0C000); // MOV R12, #0

        // Immediate value 42: should load into scratch with MOV Rd, #42
        let (gpr, pre_code) = resolve_gpr_arm32(
            &crate::ir::IRValue::Immediate(42),
            &reg_map,
            Gpr::R3,
        );
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
        assert!(code.len() >= 8, "Expected at least 2 instructions for large immediate, got {} bytes", code.len());

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
            }],
        };
        let result = backend.allocate_registers(&func).unwrap();
        let all_code: Vec<u8> = result.blocks.iter()
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
        assert!(found_sub_imm, "Expected SUB immediate instruction in generated code");
    }
}
pub mod disasm;
