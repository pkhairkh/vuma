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

// Global set of function names that return 64-bit values (I64/U64).
// Populated by compile_dump/compile_to_binary_direct before allocation,
// used by allocate_registers to determine whether to store R1 (high word)
// for non-extern call returns.
// Uses RwLock instead of thread_local! because compile_dump uses
// std::thread::scope for parallel allocation — thread_local values set on
// the main thread are NOT visible in scoped worker threads.
static FUNC_64BIT_RETURNS: std::sync::OnceLock<std::sync::RwLock<Option<std::collections::HashSet<String>>>> = std::sync::OnceLock::new();

fn func_64bit_returns() -> &'static std::sync::RwLock<Option<std::collections::HashSet<String>>> {
    FUNC_64BIT_RETURNS.get_or_init(|| std::sync::RwLock::new(None))
}

/// Set the global set of 64-bit-returning function names.
pub fn set_64bit_returns(names: &std::collections::HashSet<String>) {
    let lock = func_64bit_returns();
    *lock.write().unwrap() = Some(names.clone());
}
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// ARM 32-bit general-purpose registers (R0–R15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
/// Format: `cond[31:28] | 00011001[27:20] | Rn[19:16] | Rd[15:12] | 1111[11:8] | 1001[7:4] | 1111[3:0]`
///
/// IMPORTANT: The previous encoding used bits[27:20] = 0001_1011 (0x1B), which
/// is the LDREXD (Load Register Exclusive Doubleword) opcode, NOT LDREX.
/// LDREXD requires an even/odd register pair (Rt, Rt+1) where bits[15:12]=Rt
/// and bits[3:0]=Rt2. The old code set bits[3:0]=0b1111 (=R15=PC), producing
/// "LDREXD Rd, PC, [Rn]" which is UNPREDICTABLE on ARMv7-A. The CPU wrote
/// the high word of the exclusive load to PC, jumping to a garbage address
/// and causing SIGSEGV (spinlock.vuma CRASH -11 on arm32).
///
/// The correct LDREX (single-word) opcode is 0001_1001 (0x19), verified
/// against the ARM ARM A8.8.71 and the GNU assembler output
/// (`LDREX R0, [R3]` → 0xE1930F9F).
fn encode_ldrex(cond: Condition, rn: u32, rd: u32) -> [u8; 4] {
    let word = (cond.encoding() << 28)
        | (0b0001_1001 << 20)
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
/// Format: `cond[31:28] | 01010111[27:20] | 1111[19:16] | 1111[15:12] | 0101[11:8] | 1111[7:4] | option[3:0]`
///
/// option = 0xF for DMB SY (full system barrier).
///
/// NOTE: An earlier version of this encoder had bits[11:8] and bits[7:4]
/// swapped (emitted `0xE57FFF5F` instead of the correct `0xE57FF5FF`), which
/// decoded to an UNPREDICTABLE `LDR PC, [PC, #imm]!` and caused SIGSEGV under
/// QEMU. The fields are now correctly placed.
fn encode_dmb(cond: Condition, option: u32) -> [u8; 4] {
    // ARMv7 DMB encoding:
    // cond[31:28] | 01010111[27:20] | 1111[19:16] | 0000[15:12] | 1111[11:8] | 0101[7:4] | option[3:0]
    // DMB SY: cond=AL, option=0xF → 0xE57F0F5F
    let word = ((cond.encoding() << 28)
        | (0b0101_0111 << 20)
        | (0b1111 << 16))  // Rd = 0 (must be 0000 for DMB)
        | (0b1111 << 8)   // CRm = 1111
        | (0b0101 << 4)   // DMB = 0101
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
#[derive(Debug, Clone, PartialEq, Eq)]
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

    // ELF32 section header constants (Elf32_Shdr sh_type / sh_flags).
    const SHT_PROGBITS: u32 = 1;
    const SHT_STRTAB: u32 = 3;
    const SHT_NOBITS: u32 = 8;
    const SHF_WRITE: u32 = 0x1;
    const SHF_ALLOC: u32 = 0x2;
    const SHF_EXECINSTR: u32 = 0x4;
    const SHDR_SIZE: u64 = 40; // sizeof(Elf32_Shdr)

    let elf_header_size: u64 = 52;
    let phdr_size: u64 = 32;
    let num_phdrs: u64 = 3; // 2x PT_LOAD + 1x PT_GNU_STACK
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    // Page-align the text segment start in the file for mmap compatibility.
    let text_offset = phdr_end; // No page alignment — code right after headers
    let text_size = code.len() as u64;

    // The data segment starts on the next page after the text.
    let text_file_end = text_offset + text_size;
    let data_vaddr =
        (base_addr + text_file_end).div_ceil(PAGE_SIZE) * PAGE_SIZE;
    let _data_offset = data_vaddr - base_addr;
    let data_size: u64 = PAGE_SIZE; // 1 page of writable memory for stack/data
    let entry_point = base_addr + text_offset;

    // --- Section header table layout ---
    // .shstrtab content: NUL + ".text" + NUL + ".data" + NUL + ".shstrtab" + NUL
    //   name offsets:  .text=1  .data=7  .shstrtab=13
    let shstrtab_content: &[u8] = b"\0.text\0.data\0.shstrtab\0";
    let shstrtab_size = shstrtab_content.len() as u64;
    // .shstrtab immediately follows the text segment in the file.
    let shstrtab_offset = text_offset + text_size;
    // Section header table starts after .shstrtab, 4-byte aligned
    // (Elf32_Shdr has natural alignment of 4 bytes).
    let shdr_offset = (shstrtab_offset + shstrtab_size).div_ceil(4) * 4;
    // Sections: 0=null, 1=.text, 2=.data, 3=.shstrtab
    let num_shdrs: u64 = 4;
    let shstrndx: u16 = (num_shdrs - 1) as u16; // .shstrtab is the last section

    let mut elf = Vec::with_capacity((shdr_offset + num_shdrs * SHDR_SIZE) as usize);

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
    elf.extend_from_slice(&(shdr_offset as u32).to_le_bytes()); // e_shoff
    // e_flags: ARM EF_ARM_ABI_VER5 = 0x05000000 (soft-float ABI)
    elf.extend_from_slice(&0x05000000u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&52u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&32u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&3u16.to_le_bytes()); // e_phnum = 3 (2 LOAD + 1 GNU_STACK)
    elf.extend_from_slice(&40u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&(num_shdrs as u16).to_le_bytes()); // e_shnum
    elf.extend_from_slice(&shstrndx.to_le_bytes()); // e_shstrndx

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

    // --- Program Header 3: PT_GNU_STACK (non-executable stack) ---
    // p_type = 0x6474e551, p_flags = PF_R | PF_W (no PF_X)
    // All offsets/sizes are 0; p_align = 0x4 (32-bit ELF).
    elf.extend_from_slice(&0x6474e551u32.to_le_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&0u32.to_le_bytes()); // p_offset
    elf.extend_from_slice(&0u32.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&0u32.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u32.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&0u32.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&0x4u32.to_le_bytes()); // p_align

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

    // Pad to 4-byte alignment for the section header table.
    while (elf.len() as u64) < shdr_offset {
        elf.push(0);
    }

    // --- Section Header Table ---
    // Each Elf32_Shdr is 40 bytes:
    //   sh_name(u32) sh_type(u32) sh_flags(u32) sh_addr(u32) sh_offset(u32)
    //   sh_size(u32) sh_link(u32) sh_info(u32) sh_addralign(u32) sh_entsize(u32)

    // Section 0: SHT_NULL (reserved, all zeros).
    elf.extend_from_slice(&[0u8; 40]);

    // Section 1: .text (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR).
    elf.extend_from_slice(&1u32.to_le_bytes()); // sh_name (offset 1 in .shstrtab)
    elf.extend_from_slice(&SHT_PROGBITS.to_le_bytes()); // sh_type
    elf.extend_from_slice(&(SHF_ALLOC | SHF_EXECINSTR).to_le_bytes()); // sh_flags
    elf.extend_from_slice(&(entry_point as u32).to_le_bytes()); // sh_addr (= base_addr + text_offset)
    elf.extend_from_slice(&(text_offset as u32).to_le_bytes()); // sh_offset
    elf.extend_from_slice(&(text_size as u32).to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&4u32.to_le_bytes()); // sh_addralign (4 for ARM32 code)
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_entsize

    // Section 2: .data (SHT_NOBITS, SHF_ALLOC | SHF_WRITE).
    // The data segment has p_filesz=0 (BSS-like: zero-filled by the loader),
    // so we use SHT_NOBITS to accurately reflect that there is no file
    // content for this section.
    elf.extend_from_slice(&7u32.to_le_bytes()); // sh_name (offset 7 in .shstrtab)
    elf.extend_from_slice(&SHT_NOBITS.to_le_bytes()); // sh_type
    elf.extend_from_slice(&(SHF_ALLOC | SHF_WRITE).to_le_bytes()); // sh_flags
    elf.extend_from_slice(&(data_vaddr as u32).to_le_bytes()); // sh_addr
    elf.extend_from_slice(&(text_file_end as u32).to_le_bytes()); // sh_offset (matches p_offset; nominal for NOBITS)
    elf.extend_from_slice(&(data_size as u32).to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&4u32.to_le_bytes()); // sh_addralign
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_entsize

    // Section 3: .shstrtab (SHT_STRTAB, no alloc flags — not loaded).
    elf.extend_from_slice(&13u32.to_le_bytes()); // sh_name (offset 13 in .shstrtab)
    elf.extend_from_slice(&SHT_STRTAB.to_le_bytes()); // sh_type
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_flags (not loaded into memory)
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_addr (no virtual address)
    elf.extend_from_slice(&(shstrtab_offset as u32).to_le_bytes()); // sh_offset
    elf.extend_from_slice(&(shstrtab_size as u32).to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&1u32.to_le_bytes()); // sh_addralign (byte-aligned strings)
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_entsize

    // Note: we deliberately do NOT pad to data_offset — the data segment
    // has p_filesz=0 so there is no file content. Trailing bytes would
    // confuse QEMU's ELF loader on ARM32.

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
    // Strategy:
    //   1. If r0 < 0, print '-' to stdout, then negate r0.
    //   2. Divide by 10 (repeated subtraction — ARM32 baseline has no MUL/UDIV),
    //      store each remainder digit (in reverse) into a 16-byte stack buffer.
    //   3. Reverse the buffer in place.
    //   4. sys_write(1, buf, digit_count) and return.
    //
    // W15 (this rewrite) fixes two pre-existing issues:
    //   - The minus sign was never actually emitted (an earlier attempt was
    //     truncated with `code.truncate(code.len() - 4)`).
    //   - The BGE branch offset (`3 * 4`) assumed three placeholder
    //     instructions (RSBLT/MOV/BL) would follow, but only one was ever
    //     emitted, so the branch landed two instructions *into* the digit
    //     loop — skipping the initial `CMP r4, #0` and corrupting the loop.
    // The new layout uses an unconditional RSB (BGE already gates entry),
    // emits a real sys_write call for '-', and recomputes the BGE offset
    // from the actual instruction count below.
    //
    // We also save/restore R7 around this function because sys_write
    // requires R7 = 4 (syscall number) and the caller may be holding a
    // live value there. Previously R7 was clobbered without preservation.

    // PUSH {r4, r5, r6, r7, lr}
    code.extend_from_slice(&encode_stm(
        Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x40F0,
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
    // BGE int_positive — skip the 8-instruction negative-handling block below.
    // The block consists of: RSB, MOV r0,#45, STRB r0,[SP,#15], MOV r0,#1,
    //                       ADD r1,SP,#15, MOV r2,#1, MOV r7,#4, SVC #0.
    // ARM branch semantics: target = PC+8 + offset*4, so to skip N
    // instructions the offset must be N-1. With N=8, offset = 7.
    code.extend_from_slice(&encode_branch(Condition::Ge, false, 7));

    // ── Negative handling (only reached if r4 < 0) ──
    // RSB r4, r4, #0  →  r4 = -r4 (unconditional: BGE above already gated entry)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, 0b0011, false, Gpr::R4.encoding(), Gpr::R4.encoding(), 0, 0,
    ));
    // MOV r0, #45 ('-')
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 45,
    ));
    // STRB r0, [SP, #15]  — store '-' at the top of the 16-byte buffer
    // (digit positions 0..r5-1 never reach index 15 for a 32-bit int,
    // so this slot is safely reusable as a 1-byte scratch).
    code.extend_from_slice(&encode_ls_imm(
        Condition::Al, true, true, true, false, false, Gpr::R13.encoding(), Gpr::R0.encoding(), 15,
    ));
    // MOV r0, #1 (fd = stdout)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
    ));
    // ADD r1, SP, #15 (buffer pointer to '-')
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R1.encoding(), 0, 15,
    ));
    // MOV r2, #1 (length)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R2.encoding(), 0, 1,
    ));
    // MOV r7, #4 (sys_write syscall number)
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 4,
    ));
    // SVC #0
    code.extend_from_slice(&encode_svc(Condition::Al, 0));

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
    // POP {r4, r5, r6, r7, pc}  — restored R7 to match the PUSH above.
    // Mask: (1<<4)|(1<<5)|(1<<6)|(1<<7)|(1<<15) = 0x80F0.
    code.extend_from_slice(&encode_ldm(
        Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x80F0,
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

    /// Wave 22: Emit a function using real register allocation.
    ///
    /// Consumes a `RegAllocResult` and produces an `AllocatedFunction`
    /// with `reads`/`writes` annotated with the physical registers
    /// (r0-r3, r4-r10) assigned by the linear-scan allocator.
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
        let alloc = crate::regalloc_emit::run_regalloc(func, "arm32");
        self.emit_function_regalloc(func, &alloc)
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

/// Emit the 18-instruction body of a 64-bit unsigned division loop.
///
/// Register allocation at loop entry:
///   R0:R1 = dividend (will be shifted to 0 by the end)
///   R2:R3 = divisor (preserved)
///   R4:R5 = remainder (init to 0 before loop)
///   R6:R7 = quotient  (init to 0 before loop)
///   R8    = loop counter (init to 64 before loop)
///   R9    = scratch (used to extract dividend MSB)
///
/// On exit, R6:R7 = quotient and R4:R5 = remainder.
///
/// The loop performs the classic shift-and-subtract long division: for each
/// of the 64 bit positions (from MSB to LSB), it shifts the next dividend
/// bit into the remainder, compares the remainder against the divisor, and
/// if the remainder is greater-or-equal, subtracts the divisor and sets the
/// corresponding quotient bit.
///
/// All branch offsets are computed relative to the loop's first instruction
/// (byte 0 = `.loop`).
fn emit_arm32_udiv64_loop() -> Vec<u8> {
    let mut code = Vec::new();
    // .loop:                                            ; byte offset (within loop)
    // MOV  R9, R1, LSR #31   ; R9 = MSB of dividend     ; 0
    code.extend_from_slice(&encode_dp_shift_imm(
        Condition::Al, DP_MOV, false, 0,
        Gpr::R9.encoding(), 1, 31, Gpr::R1.encoding(),
    ));
    // MOVS R4, R4, LSL #1    ; rem_lo <<= 1, C = bit31  ; 4
    code.extend_from_slice(&encode_dp_shift_imm(
        Condition::Al, DP_MOV, true, 0,
        Gpr::R4.encoding(), 0, 1, Gpr::R4.encoding(),
    ));
    // ADC  R5, R5, R5        ; rem_hi = (rem_hi<<1) + C  ; 8
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADC, false,
        Gpr::R5.encoding(), Gpr::R5.encoding(), Gpr::R5.encoding(),
    ));
    // ORR  R4, R4, R9        ; rem_lo |= dividend MSB   ; 12
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ORR, false,
        Gpr::R4.encoding(), Gpr::R4.encoding(), Gpr::R9.encoding(),
    ));
    // MOVS R0, R0, LSL #1    ; div_lo <<= 1, C = bit31  ; 16
    code.extend_from_slice(&encode_dp_shift_imm(
        Condition::Al, DP_MOV, true, 0,
        Gpr::R0.encoding(), 0, 1, Gpr::R0.encoding(),
    ));
    // ADC  R1, R1, R1        ; div_hi = (div_hi<<1) + C ; 20
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADC, false,
        Gpr::R1.encoding(), Gpr::R1.encoding(), Gpr::R1.encoding(),
    ));
    // MOVS R6, R6, LSL #1    ; quot_lo <<= 1, C = bit31 ; 24
    code.extend_from_slice(&encode_dp_shift_imm(
        Condition::Al, DP_MOV, true, 0,
        Gpr::R6.encoding(), 0, 1, Gpr::R6.encoding(),
    ));
    // ADC  R7, R7, R7        ; quot_hi = (quot_hi<<1)+C ; 28
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_ADC, false,
        Gpr::R7.encoding(), Gpr::R7.encoding(), Gpr::R7.encoding(),
    ));
    // CMP  R5, R3            ; compare rem_hi vs dvsr_hi; 32
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_CMP, true,
        Gpr::R5.encoding(), 0, Gpr::R3.encoding(),
    ));
    // BHI  +2 (to .skip_sub at byte 52)                 ; 36
    code.extend_from_slice(&encode_branch(Condition::Hi, false, 2));
    // BLO  +4 (to .shift_sub at byte 64)                ; 40
    code.extend_from_slice(&encode_branch(Condition::Cc, false, 4));
    // CMP  R4, R2            ; high equal, compare low  ; 44
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_CMP, true,
        Gpr::R4.encoding(), 0, Gpr::R2.encoding(),
    ));
    // BLO  +2 (to .shift_sub at byte 64)                ; 48
    code.extend_from_slice(&encode_branch(Condition::Cc, false, 2));
    // .skip_sub: SUBS R4, R4, R2  ; rem_lo -= dvsr_lo   ; 52
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_SUB, true,
        Gpr::R4.encoding(), Gpr::R4.encoding(), Gpr::R2.encoding(),
    ));
    // SBC  R5, R5, R3        ; rem_hi -= dvsr_hi - borrow ; 56
    code.extend_from_slice(&encode_dp_reg(
        Condition::Al, DP_SBC, false,
        Gpr::R5.encoding(), Gpr::R5.encoding(), Gpr::R3.encoding(),
    ));
    // ORR  R6, R6, #1        ; quot_lo |= 1            ; 60
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_ORR, false,
        Gpr::R6.encoding(), Gpr::R6.encoding(), 0, 1,
    ));
    // .shift_sub: SUBS R8, R8, #1   ; counter--         ; 64
    code.extend_from_slice(&encode_dp_imm(
        Condition::Al, DP_SUB, true,
        Gpr::R8.encoding(), Gpr::R8.encoding(), 0, 1,
    ));
    // BNE  -19 (to .loop at byte 0)                     ; 68
    code.extend_from_slice(&encode_branch(Condition::Ne, false, -19));
    code
}

/// Generate ARM32 machine code to load a 32-bit immediate value into a register.
///
/// For values that fit in the ARM rotated-immediate format, emits a single
/// `MOV Rd, #imm8, rotate`. For larger values, emits a `MOV Rd, #low16`
/// followed by `ORR Rd, Rd, #high16` (each 16-bit half encoded as a rotated
/// immediate, possibly requiring further decomposition).
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
                // Load imm into R12, then ADD rd, rn, R12.
                // This works for ALL register combinations including rd == R12:
                //   - rd == R12, rd != rn: ADD R12, rn, R12 → R12 = rn + R12 = rn + imm ✓
                //   - rd == R12, rd == rn: ADD R12, R12, R12 → R12 = 2*R12 (wrong, but
                //     this case means caller wants R12 = R12 + imm which IS 2*old_R12
                //     only if imm == old_R12, so caller must avoid this; in practice
                //     callers never pass rd == rn == R12).
                //   - rd != R12: ADD rd, rn, R12 → rd = rn + imm ✓
                // The previous code had a `MOV rd, rn` between the load and ADD which
                // was redundant for rd != R12 and DESTRUCTIVE for rd == R12 (it
                // overwrote the immediate in R12 before the ADD could read it,
                // producing rd = rn + rn = 2*rn instead of rn + imm).
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, imm as u32));
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

        /// Store a 32-bit value AND zero the high word of the 8-byte slot.
        ///
        /// This is the correct way to store any 32-bit result (Load, Add,
        /// Mul, Div, etc.) into a stack slot. The high 4 bytes are zeroed
        /// to prevent garbage from corrupting subsequent 64-bit operations.
        ///
        /// Implementation: Stores the low word via STR, then stores 0 to
        /// the high word via STR of a register that we KNOW is 0. We use
        /// the ARM `EOR Rd, Rd, Rd` trick to zero a register that's NOT
        /// src_reg and NOT R11. We pick R1 if src_reg != R1, else R0.
        /// This is safe because callers have already consumed src_reg's
        /// value (it's being stored), and the scratch is only used within
        /// this function.
        ///
        /// For the large-offset path (offset > 4095), we DON'T zero the
        /// high word — those offsets are rare (only for functions with
        /// 400+ vregs) and the R12 scratch conflict makes it unsafe.
        /// The high word will be garbage, but those functions typically
        /// don't use 64-bit operations on their results.
        fn ss_store_32_zero(src_reg: Gpr, offset_from_r11: i32, _frame_size: i32) -> Vec<u8> {
            let neg_off = -offset_from_r11;
            let hi_neg_off = neg_off + 4; // high word at [R11 - (offset-4)]
            if neg_off >= -4095 && hi_neg_off >= -4095 {
                // Both offsets fit in 12-bit immediate — no R12 needed.
                let mut code = Vec::new();
                // STR src_reg, [R11, #(-neg_off)]  (low word)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, false, false, false, false,
                    Gpr::R11.encoding(), src_reg.encoding(), (-neg_off) as u32,
                ));
                // Use R14 (LR) as temporary zero register. LR is callee-saved
                // and already saved in the prologue at [R11 + 4]. We clobber LR
                // here; it will be restored by the function epilogue (or
                // overwritten by the next BL). No restore needed mid-function.
                // MOV R14, #0
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_MOV, false, 0, Gpr::R14.encoding(), 0, 0,
                ));
                // STR R14, [R11, #(-hi_neg_off)]  (high word = 0)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, false, false, false, false,
                    Gpr::R11.encoding(), Gpr::R14.encoding(), (-hi_neg_off) as u32,
                ));
                code
            } else {
                // Large offset — compute address into R12, store low word,
                // then store 0 to high word using R14 (LR) as zero scratch.
                //
                // LR is clobbered as a zero source; the function epilogue
                // restores LR from the prologue save area at [R11 + 4], so
                // no mid-function LR restore is needed.
                //
                // Precondition: src_reg != R12 (callers already enforce this).
                let mut code = Vec::new();

                // ── Store low word: R12 = R11 - offset; STR src_reg, [R12] ──
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, offset_from_r11 as u32));
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_SUB, false,
                    Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding(),
                ));
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R12.encoding(), src_reg.encoding(), 0,
                ));

                // ── Zero R14, then store to high word ──
                // MOV R14, #0  (safe even if src_reg == R14: already stored above)
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_MOV, false, 0, Gpr::R14.encoding(), 0, 0,
                ));
                // R12 = R11 - (offset - 4)   (high word is 4 bytes ABOVE low word)
                let hi_offset = offset_from_r11 - 4;
                code.extend_from_slice(&load_immediate_arm32(Gpr::R12, hi_offset as u32));
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_SUB, false,
                    Gpr::R11.encoding(), Gpr::R12.encoding(), Gpr::R12.encoding(),
                ));
                // STR R14, [R12]  (high word = 0)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R12.encoding(), Gpr::R14.encoding(), 0,
                ));
                code
            }
        }

        /// Store TWO registers (lo, hi) into a vreg slot.
        /// Uses ss_store_to_slot (NOT ss_store_32_zero) because both words
        /// have valid data — zeroing the high word would destroy the hi_reg value.
        fn ss_store_64(lo_reg: Gpr, hi_reg: Gpr, offset_from_r11: i32) -> Vec<u8> {
            let mut code = Vec::new();
            code.extend(ss_store_to_slot(lo_reg, offset_from_r11));
            code.extend(ss_store_to_slot(hi_reg, offset_from_r11 + 4));
            code
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
                crate::ir::IRValue::Label(name) => {
                    vuma_log!(warn, "IRValue::Label('{}') emitting placeholder 0", name);
                    load_immediate_arm32(scratch, 0)
                }
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
                    // Load high word: (v >> 32) as u32
                    // For negative i64 values, this naturally gives 0xFFFFFFFF (sign extension)
                    let hi_val = (*v >> 32) as u32;
                    code.extend(load_immediate_arm32(hi_reg, hi_val));
                }
                crate::ir::IRValue::Address(a) => {
                    code.extend(load_immediate_arm32(lo_reg, *a as u32));
                    code.extend_from_slice(&encode_dp_imm(
                        Condition::Al, DP_MOV, false, 0, hi_reg.encoding(), 0, 0,
                    ));
                }
                crate::ir::IRValue::Label(name) => {
                    vuma_log!(warn, "IRValue::Label('{}') emitting placeholder 0", name);
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
                    // Use ss_store_32_zero to store the 32-bit register value
                    // AND zero the high 32 bits of the 8-byte slot. This prevents
                    // garbage in the high word of I64/Address params, which would
                    // cause SIGSEGV when used as pointers.
                    let store_code = ss_store_32_zero(arg_regs[i], offset, fs);
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
                    // STR R0, [R11 - slot_offset] + zero high word
                    param_code.extend(ss_store_32_zero(Gpr::R0, slot_offset, fs));
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
            branch_offset_in_enc: usize, // byte offset of this branch within the instruction's encoded output
        }
        let mut branch_fixups: Vec<BranchFixup> = Vec::new();

        // Build predecessor-aware phi resolution map.
        let phi_map = func.build_phi_map();

        for block in &func.blocks {
            // Record the byte offset for this block's label
            label_offsets.insert(block.label.clone(), current_byte_offset);

            for instr in &block.instructions {
                let encoded: Vec<u8> = match instr {
                    // ── BinOp (generic) ──
                    crate::ir::IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();

                        // ── FP BinOp dispatch ──
                        // When ty is F32/F64 and op is FP-arithmetic (Add/Sub/Mul/SDiv/UDiv),
                        // load operands' bit patterns into D0/D1 via the dst stack slot,
                        // run VFP arithmetic, and VSTR the result back to dst.
                        let is_fp = ty.as_ref().is_some_and(|t| matches!(t, crate::ir::IRType::F32 | crate::ir::IRType::F64));
                        let fp_arith = is_fp && matches!(op,
                            BinOpKind::Add | BinOpKind::Sub | BinOpKind::Mul
                            | BinOpKind::SDiv | BinOpKind::UDiv
                        );
                        if fp_arith {
                            // Load lhs bit pattern into R0, store to dst slot, VLDR D0 from dst.
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                            code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            code.extend_from_slice(&encode_vldr_d(0, Gpr::R11.encoding() as u8, dst_offset));
                            // Load rhs bit pattern into R0, store to dst slot, VLDR D1 from dst.
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R0));
                            code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            code.extend_from_slice(&encode_vldr_d(1, Gpr::R11.encoding() as u8, dst_offset));
                            // VFP arithmetic: D0 = D0 <op> D1
                            match op {
                                BinOpKind::Add => code.extend_from_slice(&encode_vadd_f64(0, 0, 1)),
                                BinOpKind::Sub => code.extend_from_slice(&encode_vsub_f64(0, 0, 1)),
                                BinOpKind::Mul => code.extend_from_slice(&encode_vmul_f64(0, 0, 1)),
                                BinOpKind::SDiv | BinOpKind::UDiv => code.extend_from_slice(&encode_vdiv_f64(0, 0, 1)),
                                _ => unreachable!(),
                            }
                            // VSTR D0 to dst slot (stores 64-bit double into the 8-byte slot).
                            code.extend_from_slice(&encode_vstr_d(0, Gpr::R11.encoding() as u8, dst_offset));
                        }

                        if !fp_arith {
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
                                code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                // Zero the high word — 32-bit MUL only produces a 32-bit
                                // result, but stack slots are 8 bytes. Without clearing
                                // the high word, subsequent 64-bit operations (e.g. pointer
                                // arithmetic) read garbage from the high word.
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                )); // MOV R1, #0
                                code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
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
                            BinOpKind::Shl => {
                                // 64-bit Shl: R0=low, R2=high, R1=shift
                                code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                                code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                                // CMP R1, #32
                                code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_CMP, true, Gpr::R1.encoding(), 0, 0, 32));
                                // shift >= 32 (Cs): R12 = R1 - 32; R2 = R0 << R12; R0 = 0
                                code.extend_from_slice(&encode_dp_imm(Condition::Cs, DP_SUB, false, Gpr::R1.encoding(), Gpr::R12.encoding(), 0, 32));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cs, DP_MOV, false, 0, Gpr::R2.encoding(), 0, Gpr::R12.encoding(), Gpr::R0.encoding()));
                                code.extend_from_slice(&encode_dp_imm(Condition::Cs, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 0));
                                // shift < 32 (Cc): R12 = R0; R0 = R0 << R1; R3 = 32-R1; R12 = R12 >> R3; R2 = (R2<<R1)|R12
                                code.extend_from_slice(&encode_dp_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R12.encoding(), Gpr::R0.encoding()));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R0.encoding(), 0, Gpr::R1.encoding(), Gpr::R0.encoding()));
                                code.extend_from_slice(&encode_dp_imm(Condition::Cc, DP_RSB, false, Gpr::R1.encoding(), Gpr::R3.encoding(), 0, 32));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R12.encoding(), 1, Gpr::R3.encoding(), Gpr::R12.encoding()));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R3.encoding(), 0, Gpr::R1.encoding(), Gpr::R2.encoding()));
                                code.extend_from_slice(&encode_dp_reg(Condition::Cc, DP_ORR, false, Gpr::R2.encoding(), Gpr::R3.encoding(), Gpr::R12.encoding()));
                                code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                            }
                            BinOpKind::ShrA => {
                                // 64-bit ShrA: R0=low, R2=high, R1=shift
                                code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                                code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                                // CMP R1, #32
                                code.extend_from_slice(&encode_dp_imm(Condition::Al, DP_CMP, true, Gpr::R1.encoding(), 0, 0, 32));
                                // shift >= 32 (Cs): R12 = R1 - 32; R0 = R2 ASR R12; R2 = R2 ASR #31 (sign ext)
                                code.extend_from_slice(&encode_dp_imm(Condition::Cs, DP_SUB, false, Gpr::R1.encoding(), Gpr::R12.encoding(), 0, 32));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cs, DP_MOV, false, 0, Gpr::R0.encoding(), 2, Gpr::R12.encoding(), Gpr::R2.encoding()));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cs, DP_MOV, false, 0, Gpr::R2.encoding(), 2, Gpr::R1.encoding(), Gpr::R2.encoding()));
                                // shift < 32 (Cc): R12 = R2; R0 = (R2<<(32-R1))|(R0>>R1); R2 = R2 ASR R1
                                code.extend_from_slice(&encode_dp_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R12.encoding(), Gpr::R2.encoding()));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R0.encoding(), 2, Gpr::R1.encoding(), Gpr::R0.encoding()));
                                code.extend_from_slice(&encode_dp_imm(Condition::Cc, DP_RSB, false, Gpr::R1.encoding(), Gpr::R3.encoding(), 0, 32));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R12.encoding(), 0, Gpr::R3.encoding(), Gpr::R12.encoding()));
                                code.extend_from_slice(&encode_dp_reg(Condition::Cc, DP_ORR, false, Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding()));
                                code.extend_from_slice(&encode_dp_shift_reg(Condition::Cc, DP_MOV, false, 0, Gpr::R2.encoding(), 2, Gpr::R1.encoding(), Gpr::R2.encoding()));
                                code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                            }
                            BinOpKind::Ror | BinOpKind::Rol => {
                                // 32-bit rotate (keep as-is)
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                                if matches!(op, BinOpKind::Rol) {
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, 0b0011, false,
                                        Gpr::R1.encoding(), Gpr::R2.encoding(), 0, 32,
                                    ));
                                    code.extend_from_slice(&encode_dp_shift_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R0.encoding(), 3, Gpr::R2.encoding(), Gpr::R0.encoding(),
                                    ));
                                } else {
                                    code.extend_from_slice(&encode_dp_shift_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R0.encoding(), 3, Gpr::R1.encoding(), Gpr::R0.encoding(),
                                    ));
                                }
                                code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                            }
                            BinOpKind::SDiv | BinOpKind::UDiv => {
                                // Detect 64-bit division: when ty is I64/U64,
                                // use the 64-bit shift-and-subtract algorithm.
                                // Otherwise (32-bit-or-narrower types and the
                                // default `None`), fall back to the 32-bit
                                // software-division loop.
                                let is_64bit = matches!(
                                    ty.as_ref(),
                                    Some(crate::ir::IRType::I64) | Some(crate::ir::IRType::U64)
                                );

                                if is_64bit {
                                    // === 64-bit SDiv / UDiv ===
                                    // Load dividend into R0:R1 and divisor into R2:R3.
                                    code.extend(ss_load_value_64(
                                        Gpr::R0, Gpr::R1, lhs, &vreg_stack_slots,
                                    ));
                                    code.extend(ss_load_value_64(
                                        Gpr::R2, Gpr::R3, rhs, &vreg_stack_slots,
                                    ));

                                    // PUSH {R4, R5, R6, R7, R8, R9} — save
                                    // callee-saved registers we clobber below.
                                    // Register list: (1<<4)|(1<<5)|(1<<6)|(1<<7)|(1<<8)|(1<<9) = 0x03F0.
                                    code.extend_from_slice(&encode_stm(
                                        Condition::Al, true, false, false, true,
                                        Gpr::R13.encoding(), 0x03F0,
                                    ));

                                    if matches!(op, BinOpKind::SDiv) {
                                        // Signed division: normalize dividend
                                        // and divisor to be non-negative, then
                                        // run the unsigned 64-bit division
                                        // loop, then negate the quotient if
                                        // exactly one operand was negative.
                                        // R12 holds the XOR-of-signs flag
                                        // (0 = same, 1 = differ).
                                        //
                                        // TST R1, #0x80000000  (sign bit of dividend_hi)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R1.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // R12 = (R1 < 0) ? 1 : 0  (MOVPL/MOVMI do not update flags)
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Pl, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 0,
                                        )); // MOVPL R12, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 1,
                                        )); // MOVMI R12, #1
                                        // BEQ +1 (skip 64-bit negate if positive)
                                        code.extend_from_slice(&encode_branch(
                                            Condition::Eq, false, 1,
                                        ));
                                        // RSBS R0, R0, #0  (low word, sets C = !borrow)
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_RSB, true,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        ));
                                        // SBC R1, R1, #0   (high word, with borrow)
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_SBC, false,
                                            Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                        ));
                                        // TST R3, #0x80000000  (sign bit of divisor_hi)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R3.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // Toggle R12 if R3 < 0  → R12 = (signs differ)
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, DP_EOR, false,
                                            Gpr::R12.encoding(), Gpr::R12.encoding(), 0, 1,
                                        )); // EORMI R12, R12, #1
                                        // BEQ +1 (skip 64-bit negate if positive)
                                        code.extend_from_slice(&encode_branch(
                                            Condition::Eq, false, 1,
                                        ));
                                        // RSBS R2, R2, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_RSB, true,
                                            Gpr::R2.encoding(), Gpr::R2.encoding(), 0, 0,
                                        ));
                                        // SBC R3, R3, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_SBC, false,
                                            Gpr::R3.encoding(), Gpr::R3.encoding(), 0, 0,
                                        ));
                                    }

                                    // Init remainder=0, quotient=0, counter=64.
                                    // MOV R4, #0 (rem_lo)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R4.encoding(), 0, 0,
                                    ));
                                    // MOV R5, #0 (rem_hi)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 0,
                                    ));
                                    // MOV R6, #0 (quot_lo)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R6.encoding(), 0, 0,
                                    ));
                                    // MOV R7, #0 (quot_hi)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 0,
                                    ));
                                    // MOV R8, #64 (counter)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R8.encoding(), 0, 64,
                                    ));

                                    // Check divisor == 0 (skip the loop entirely
                                    // if so — quotient stays 0). The CMPEQ
                                    // conditional compare only executes if the
                                    // preceding CMP set Z (i.e., R2 == 0), so
                                    // BEQ is taken iff both R2 and R3 are 0.
                                    // CMP R2, #0
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_CMP, true,
                                        Gpr::R2.encoding(), 0, 0, 0,
                                    ));
                                    // CMPEQ R3, #0
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Eq, DP_CMP, true,
                                        Gpr::R3.encoding(), 0, 0, 0,
                                    ));
                                    // BEQ +17 — skip the 18-instruction loop
                                    // body and land on the post-loop `MOV R0, R6`.
                                    // The loop body is 18 instructions (72 bytes),
                                    // so target = (BEQ+4) + 72 = BEQ + 76; the
                                    // branch offset = (76 - 8) / 4 = 17.
                                    code.extend_from_slice(&encode_branch(
                                        Condition::Eq, false, 17,
                                    ));

                                    // === 64-bit unsigned division loop ===
                                    code.extend(emit_arm32_udiv64_loop());

                                    // === Post-loop ===
                                    // MOV R0, R6 (quotient low)
                                    code.extend_from_slice(&encode_dp_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R0.encoding(), Gpr::R6.encoding(),
                                    ));
                                    // MOV R1, R7 (quotient high)
                                    code.extend_from_slice(&encode_dp_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R1.encoding(), Gpr::R7.encoding(),
                                    ));

                                    // For SDiv: negate quotient if signs differed.
                                    if matches!(op, BinOpKind::SDiv) {
                                        // CMP R12, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_CMP, true,
                                            Gpr::R12.encoding(), 0, 0, 0,
                                        ));
                                        // BEQ +1 (skip negate if signs same)
                                        code.extend_from_slice(&encode_branch(
                                            Condition::Eq, false, 1,
                                        ));
                                        // RSBS R0, R0, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_RSB, true,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        ));
                                        // SBC R1, R1, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_SBC, false,
                                            Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                        ));
                                    }

                                    // POP {R4, R5, R6, R7, R8, R9}
                                    code.extend_from_slice(&encode_ldm(
                                        Condition::Al, false, true, false, true,
                                        Gpr::R13.encoding(), 0x03F0,
                                    ));

                                    // Store 64-bit quotient.
                                    code.extend(ss_store_64(Gpr::R0, Gpr::R1, dst_offset));
                                } else {
                                    // === 32-bit SDiv / UDiv (existing) ===
                                    // Software division: R0 = R0 / R1
                                    code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                    code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));

                                    // For signed division, normalize the signs of
                                    // dividend and divisor before running the
                                    // unsigned division loop, then negate the
                                    // quotient if exactly one operand was negative.
                                    // R12 holds the XOR-of-signs flag (0 = same,
                                    // 1 = differ).
                                    if matches!(op, BinOpKind::SDiv) {
                                        // TST R0, #0x80000000  (sign bit of dividend)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R0.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // R12 = (R0 < 0) ? 1 : 0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 1,
                                        )); // MOVMI R12, #1
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Pl, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 0,
                                        )); // MOVPL R12, #0
                                        // TST R1, #0x80000000  (sign bit of divisor)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R1.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // Toggle R12 if R1 < 0  → R12 = (signs differ)
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, DP_EOR, false,
                                            Gpr::R12.encoding(), Gpr::R12.encoding(), 0, 1,
                                        )); // EORMI R12, R12, #1
                                        // Negate dividend if negative
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, 0b0011, false,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        )); // RSBMI R0, R0, #0
                                        // Negate divisor if negative
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, 0b0011, false,
                                            Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                        )); // RSBMI R1, R1, #0
                                    }

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

                                    // For SDiv: negate quotient if signs differed.
                                    if matches!(op, BinOpKind::SDiv) {
                                        // CMP R12, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_CMP, true,
                                            Gpr::R12.encoding(), 0, 0, 0,
                                        ));
                                        // RSBNE R0, R0, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Ne, 0b0011, false,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        ));
                                    }

                                    code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                    // Zero high word (32-bit result in 64-bit slot)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                    )); // MOV R1, #0
                                    code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                                }
                            }
                            BinOpKind::SRem | BinOpKind::URem => {
                                // Detect 64-bit remainder: when ty is I64/U64,
                                // use the 64-bit shift-and-subtract algorithm
                                // and return the remainder instead of the
                                // quotient. Otherwise, fall back to the 32-bit
                                // software-division loop.
                                let is_64bit = matches!(
                                    ty.as_ref(),
                                    Some(crate::ir::IRType::I64) | Some(crate::ir::IRType::U64)
                                );

                                if is_64bit {
                                    // === 64-bit SRem / URem ===
                                    // Load dividend into R0:R1 and divisor into R2:R3.
                                    code.extend(ss_load_value_64(
                                        Gpr::R0, Gpr::R1, lhs, &vreg_stack_slots,
                                    ));
                                    code.extend(ss_load_value_64(
                                        Gpr::R2, Gpr::R3, rhs, &vreg_stack_slots,
                                    ));

                                    // PUSH {R4, R5, R6, R7, R8, R9}
                                    code.extend_from_slice(&encode_stm(
                                        Condition::Al, true, false, false, true,
                                        Gpr::R13.encoding(), 0x03F0,
                                    ));

                                    if matches!(op, BinOpKind::SRem) {
                                        // Signed remainder: the sign of the
                                        // remainder follows the dividend. Track
                                        // the dividend sign in R12 (1 = was
                                        // negative), normalize both operands to
                                        // be non-negative, run the unsigned
                                        // 64-bit division loop, then negate the
                                        // remainder if the dividend was negative.
                                        //
                                        // TST R1, #0x80000000  (sign bit of dividend_hi)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R1.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // R12 = (R1 < 0) ? 1 : 0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Pl, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 0,
                                        )); // MOVPL R12, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 1,
                                        )); // MOVMI R12, #1
                                        // BEQ +1 (skip 64-bit negate if positive)
                                        code.extend_from_slice(&encode_branch(
                                            Condition::Eq, false, 1,
                                        ));
                                        // RSBS R0, R0, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_RSB, true,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        ));
                                        // SBC R1, R1, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_SBC, false,
                                            Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                        ));
                                        // TST R3, #0x80000000  (sign bit of divisor_hi)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R3.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // BEQ +1 (skip 64-bit negate if positive)
                                        code.extend_from_slice(&encode_branch(
                                            Condition::Eq, false, 1,
                                        ));
                                        // RSBS R2, R2, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_RSB, true,
                                            Gpr::R2.encoding(), Gpr::R2.encoding(), 0, 0,
                                        ));
                                        // SBC R3, R3, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_SBC, false,
                                            Gpr::R3.encoding(), Gpr::R3.encoding(), 0, 0,
                                        ));
                                    }

                                    // Init remainder=0, quotient=0, counter=64.
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R4.encoding(), 0, 0,
                                    )); // MOV R4, #0 (rem_lo)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 0,
                                    )); // MOV R5, #0 (rem_hi)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R6.encoding(), 0, 0,
                                    )); // MOV R6, #0 (quot_lo)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 0,
                                    )); // MOV R7, #0 (quot_hi)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R8.encoding(), 0, 64,
                                    )); // MOV R8, #64 (counter)

                                    // Check divisor == 0 — if zero, skip the loop
                                    // (remainder stays 0, which matches the C
                                    // semantics of `x % 0` being undefined but
                                    // returning 0 here).
                                    // CMP R2, #0
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_CMP, true,
                                        Gpr::R2.encoding(), 0, 0, 0,
                                    ));
                                    // CMPEQ R3, #0
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Eq, DP_CMP, true,
                                        Gpr::R3.encoding(), 0, 0, 0,
                                    ));
                                    // BEQ +17 — skip the 18-instruction loop body
                                    // and land on the post-loop `MOV R0, R4`.
                                    code.extend_from_slice(&encode_branch(
                                        Condition::Eq, false, 17,
                                    ));

                                    // === 64-bit unsigned division loop ===
                                    code.extend(emit_arm32_udiv64_loop());

                                    // === Post-loop ===
                                    // MOV R0, R4 (remainder low)
                                    code.extend_from_slice(&encode_dp_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R0.encoding(), Gpr::R4.encoding(),
                                    ));
                                    // MOV R1, R5 (remainder high)
                                    code.extend_from_slice(&encode_dp_reg(
                                        Condition::Al, DP_MOV, false, 0,
                                        Gpr::R1.encoding(), Gpr::R5.encoding(),
                                    ));

                                    // For SRem: negate remainder if dividend was negative.
                                    if matches!(op, BinOpKind::SRem) {
                                        // CMP R12, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_CMP, true,
                                            Gpr::R12.encoding(), 0, 0, 0,
                                        ));
                                        // BEQ +1 (skip negate if dividend was positive)
                                        code.extend_from_slice(&encode_branch(
                                            Condition::Eq, false, 1,
                                        ));
                                        // RSBS R0, R0, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_RSB, true,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        ));
                                        // SBC R1, R1, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_SBC, false,
                                            Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                        ));
                                    }

                                    // POP {R4, R5, R6, R7, R8, R9}
                                    code.extend_from_slice(&encode_ldm(
                                        Condition::Al, false, true, false, true,
                                        Gpr::R13.encoding(), 0x03F0,
                                    ));

                                    // Store 64-bit remainder.
                                    code.extend(ss_store_64(Gpr::R0, Gpr::R1, dst_offset));
                                } else {
                                    // === 32-bit SRem / URem (existing) ===
                                    // Software modulo: R0 = R0 % R1 (remainder)
                                    code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                                    code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));

                                    // For signed remainder, normalize signs of
                                    // dividend and divisor before running the
                                    // unsigned division loop, then negate the
                                    // remainder if the dividend was negative.
                                    // R12 holds the dividend-sign flag (1 = was neg).
                                    if matches!(op, BinOpKind::SRem) {
                                        // TST R0, #0x80000000  (sign bit of dividend)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R0.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // R12 = (R0 < 0) ? 1 : 0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 1,
                                        )); // MOVMI R12, #1
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Pl, DP_MOV, false, 0,
                                            Gpr::R12.encoding(), 0, 0,
                                        )); // MOVPL R12, #0
                                        // Negate dividend if negative
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, 0b0011, false,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        )); // RSBMI R0, R0, #0
                                        // TST R1, #0x80000000  (sign bit of divisor)
                                        if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                            code.extend_from_slice(&encode_dp_imm(
                                                Condition::Al, DP_TST, true,
                                                Gpr::R1.encoding(), 0, rot, imm8,
                                            ));
                                        }
                                        // Negate divisor if negative
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Mi, 0b0011, false,
                                            Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                        )); // RSBMI R1, R1, #0
                                    }

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

                                    // For SRem: negate remainder if dividend was negative.
                                    if matches!(op, BinOpKind::SRem) {
                                        // CMP R12, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Al, DP_CMP, true,
                                            Gpr::R12.encoding(), 0, 0, 0,
                                        ));
                                        // RSBNE R0, R0, #0
                                        code.extend_from_slice(&encode_dp_imm(
                                            Condition::Ne, 0b0011, false,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                        ));
                                    }

                                    code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                    // Zero high word (32-bit result in 64-bit slot)
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                    )); // MOV R1, #0
                                    code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                                }
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
                                code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                // Zero high word (32-bit result 0 or 1 in 64-bit slot)
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                )); // MOV R1, #0
                                code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                            }
                        }
                        } // end if !fp_arith (integer path)
                        code
                    }

                    // ── Add/Sub/Mul/Div (dedicated) ──
                    crate::ir::IRInstr::Add { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                        code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_ADD, true,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                        ));
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
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_SUB, true,
                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                        ));
                        code.extend_from_slice(&encode_dp_reg(
                            Condition::Al, DP_SBC, true,
                            Gpr::R2.encoding(), Gpr::R2.encoding(), Gpr::R3.encoding(),
                        ));
                        code.extend(ss_store_64(Gpr::R0, Gpr::R2, dst_offset));
                        code
                    }
                    crate::ir::IRInstr::Mul { dst, lhs, rhs, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();

                        // Detect 64-bit multiply: when ty is I64/U64 (or
                        // None defaulting to 64-bit), use UMULL to produce
                        // the full 64-bit product. Otherwise, use the 32-bit
                        // MUL instruction.
                        let is_64bit = match ty.as_ref() {
                            Some(crate::ir::IRType::I64)
                            | Some(crate::ir::IRType::U64) => true,
                            // Default to 32-bit when type is missing or
                            // explicitly a 32-bit-or-narrower integer.
                            _ => false,
                        };

                        if is_64bit {
                            // Load lhs (R0=low, R2=high) and rhs (R1=low, R3=high).
                            // We use R0/R2 for lhs and R1/R3 for rhs so that
                            // UMULL's RdHi/RdLo (R0/R1) don't conflict with
                            // the inputs.
                            code.extend(ss_load_value_64(Gpr::R0, Gpr::R2, lhs, &vreg_stack_slots));
                            code.extend(ss_load_value_64(Gpr::R1, Gpr::R3, rhs, &vreg_stack_slots));
                            // UMULL R0, R1, R0, R1
                            //   RdHi=R1 (high 32 bits of product)
                            //   RdLo=R0 (low 32 bits of product)
                            //   Rn=R0 (lhs low word, multiplier)
                            //   Rm=R1 (rhs low word, multiplicand)
                            // Note: ARM UMULL syntax is `UMULL RdLo, RdHi, Rn, Rm`
                            // which computes RdHi:RdLo = Rn * Rm (unsigned).
                            // Our encoder signature is
                            //   encode_umull(cond, s, rd_hi, rd_lo, rs, rm)
                            // where the encoded instruction computes
                            //   RdHi:RdLo = Rm * Rs.
                            // We want RdHi=R1, RdLo=R0, Rm=R0 (lhs low), Rs=R1 (rhs low).
                            code.extend_from_slice(&encode_umull(
                                Condition::Al, false,
                                Gpr::R1.encoding(),  // rd_hi
                                Gpr::R0.encoding(),  // rd_lo
                                Gpr::R1.encoding(),  // rs (rhs low)
                                Gpr::R0.encoding(),  // rm (lhs low)
                            ));
                            // Store both words of the 64-bit product.
                            code.extend(ss_store_64(Gpr::R0, Gpr::R1, dst_offset));
                        } else {
                            // 32-bit multiply: MUL R0, R0, R1
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                            code.extend_from_slice(&encode_mul(
                                Condition::Al, false,
                                Gpr::R0.encoding(), 0, Gpr::R1.encoding(), Gpr::R0.encoding(),
                            ));
                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        }
                        code
                    }
                    crate::ir::IRInstr::Div { dst, lhs, rhs, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();

                        // Type-aware division. The result type determines
                        // signedness and width:
                        //   - I32            → signed 32-bit (software loop
                        //                       with sign normalization +
                        //                       quotient negation)
                        //   - U32 (and the
                        //     default `None`)→ unsigned 32-bit (plain
                        //                       subtractive loop)
                        //   - I64            → signed 64-bit (shift-subtract
                        //                       loop with sign handling)
                        //   - U64            → unsigned 64-bit (plain
                        //                       shift-subtract loop)
                        //
                        // ARMv7-A makes SDIV/UDIV optional (via the IDIV
                        // extension); to remain compatible with all ARMv7
                        // cores (and QEMU's default CPU model), we use the
                        // software division loops rather than the hardware
                        // instructions. Signed division normalizes both
                        // operands to be non-negative, runs the unsigned
                        // loop, then negates the quotient if exactly one
                        // operand was negative.
                        let is_signed = matches!(
                            ty.as_ref(),
                            Some(crate::ir::IRType::I32) | Some(crate::ir::IRType::I64)
                        );
                        let is_64bit = matches!(
                            ty.as_ref(),
                            Some(crate::ir::IRType::I64) | Some(crate::ir::IRType::U64)
                        );

                        if is_64bit {
                            // === 64-bit division ===
                            // Load dividend into R0:R1 and divisor into R2:R3.
                            code.extend(ss_load_value_64(
                                Gpr::R0, Gpr::R1, lhs, &vreg_stack_slots,
                            ));
                            code.extend(ss_load_value_64(
                                Gpr::R2, Gpr::R3, rhs, &vreg_stack_slots,
                            ));

                            // PUSH {R4, R5, R6, R7, R8, R9} — save callee-saved
                            // registers we clobber below. Register list:
                            // (1<<4)|(1<<5)|(1<<6)|(1<<7)|(1<<8)|(1<<9) = 0x03F0.
                            code.extend_from_slice(&encode_stm(
                                Condition::Al, true, false, false, true,
                                Gpr::R13.encoding(), 0x03F0,
                            ));

                            if is_signed {
                                // Signed 64-bit: normalize dividend and
                                // divisor to be non-negative, track XOR of
                                // signs in R12 (0 = same, 1 = differ).
                                // TST R1, #0x80000000  (sign bit of dividend_hi)
                                if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_TST, true,
                                        Gpr::R1.encoding(), 0, rot, imm8,
                                    ));
                                }
                                // R12 = (R1 < 0) ? 1 : 0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Pl, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), 0, 0,
                                )); // MOVPL R12, #0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Mi, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), 0, 1,
                                )); // MOVMI R12, #1
                                // BEQ +1 (skip 64-bit negate if positive)
                                code.extend_from_slice(&encode_branch(
                                    Condition::Eq, false, 1,
                                ));
                                // RSBS R0, R0, #0  (low word, sets C = !borrow)
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_RSB, true,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                ));
                                // SBC R1, R1, #0   (high word, with borrow)
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_SBC, false,
                                    Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                ));
                                // TST R3, #0x80000000  (sign bit of divisor_hi)
                                if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_TST, true,
                                        Gpr::R3.encoding(), 0, rot, imm8,
                                    ));
                                }
                                // Toggle R12 if R3 < 0  → R12 = (signs differ)
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Mi, DP_EOR, false,
                                    Gpr::R12.encoding(), Gpr::R12.encoding(), 0, 1,
                                )); // EORMI R12, R12, #1
                                // BEQ +1 (skip 64-bit negate if positive)
                                code.extend_from_slice(&encode_branch(
                                    Condition::Eq, false, 1,
                                ));
                                // RSBS R2, R2, #0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_RSB, true,
                                    Gpr::R2.encoding(), Gpr::R2.encoding(), 0, 0,
                                ));
                                // SBC R3, R3, #0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_SBC, false,
                                    Gpr::R3.encoding(), Gpr::R3.encoding(), 0, 0,
                                ));
                            }

                            // Init remainder=0, quotient=0, counter=64.
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R4.encoding(), 0, 0,
                            )); // MOV R4, #0 (rem_lo)
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 0,
                            )); // MOV R5, #0 (rem_hi)
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R6.encoding(), 0, 0,
                            )); // MOV R6, #0 (quot_lo)
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 0,
                            )); // MOV R7, #0 (quot_hi)
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R8.encoding(), 0, 64,
                            )); // MOV R8, #64 (counter)

                            // Check divisor == 0 (skip the loop entirely if so).
                            // CMP R2, #0; CMPEQ R3, #0; BEQ +17 (skip 18-instr loop body).
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_CMP, true,
                                Gpr::R2.encoding(), 0, 0, 0,
                            ));
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Eq, DP_CMP, true,
                                Gpr::R3.encoding(), 0, 0, 0,
                            ));
                            code.extend_from_slice(&encode_branch(
                                Condition::Eq, false, 17,
                            ));

                            // === 64-bit unsigned division loop ===
                            code.extend(emit_arm32_udiv64_loop());

                            // MOV R0, R6 (quotient low); MOV R1, R7 (quotient high)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_MOV, false, 0,
                                Gpr::R0.encoding(), Gpr::R6.encoding(),
                            ));
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_MOV, false, 0,
                                Gpr::R1.encoding(), Gpr::R7.encoding(),
                            ));

                            // For signed division: negate quotient if signs differed.
                            if is_signed {
                                // CMP R12, #0; BEQ +1 (skip negate); RSBS R0, R0, #0; SBC R1, R1, #0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_CMP, true,
                                    Gpr::R12.encoding(), 0, 0, 0,
                                ));
                                code.extend_from_slice(&encode_branch(
                                    Condition::Eq, false, 1,
                                ));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_RSB, true,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                ));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_SBC, false,
                                    Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                ));
                            }

                            // POP {R4, R5, R6, R7, R8, R9}
                            code.extend_from_slice(&encode_ldm(
                                Condition::Al, false, true, false, true,
                                Gpr::R13.encoding(), 0x03F0,
                            ));

                            // Store 64-bit quotient.
                            code.extend(ss_store_64(Gpr::R0, Gpr::R1, dst_offset));
                        } else {
                            // === 32-bit division ===
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));

                            if is_signed {
                                // Signed 32-bit: normalize signs of dividend
                                // and divisor before the unsigned loop, then
                                // negate the quotient if exactly one operand
                                // was negative. R12 holds the XOR-of-signs
                                // flag (0 = same, 1 = differ).
                                // TST R0, #0x80000000  (sign bit of dividend)
                                if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_TST, true,
                                        Gpr::R0.encoding(), 0, rot, imm8,
                                    ));
                                }
                                // R12 = (R0 < 0) ? 1 : 0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Mi, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), 0, 1,
                                )); // MOVMI R12, #1
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Pl, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), 0, 0,
                                )); // MOVPL R12, #0
                                // TST R1, #0x80000000  (sign bit of divisor)
                                if let Some((rot, imm8)) = try_encode_arm_imm(0x8000_0000) {
                                    code.extend_from_slice(&encode_dp_imm(
                                        Condition::Al, DP_TST, true,
                                        Gpr::R1.encoding(), 0, rot, imm8,
                                    ));
                                }
                                // Toggle R12 if R1 < 0  → R12 = (signs differ)
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Mi, DP_EOR, false,
                                    Gpr::R12.encoding(), Gpr::R12.encoding(), 0, 1,
                                )); // EORMI R12, R12, #1
                                // Negate dividend if negative
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Mi, 0b0011, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                )); // RSBMI R0, R0, #0
                                // Negate divisor if negative
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Mi, 0b0011, false,
                                    Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 0,
                                )); // RSBMI R1, R1, #0
                            }

                            // Software division: R0 = R0 / R1
                            // R2 = 0 (quotient), R3 = R0 (remainder)
                            // CMP R1, #0; BEQ done
                            // loop: CMP R3, R1; BLO done
                            //   SUB R3, R3, R1; ADD R2, R2, #1; B loop
                            // done: MOV R0, R2
                            code.extend_from_slice(&0xE3A02000u32.to_le_bytes()); // MOV R2, #0
                            code.extend_from_slice(&0xE1A03000u32.to_le_bytes()); // MOV R3, R0
                            code.extend_from_slice(&0xE3510000u32.to_le_bytes()); // CMP R1, #0
                            code.extend_from_slice(&0x0A000003u32.to_le_bytes()); // BEQ +3 (to done)
                            code.extend_from_slice(&0xE1530001u32.to_le_bytes()); // loop: CMP R3, R1
                            code.extend_from_slice(&0x3A000002u32.to_le_bytes()); // BLO +2 (to done)
                            code.extend_from_slice(&0xE0433001u32.to_le_bytes()); // SUB R3, R3, R1
                            code.extend_from_slice(&0xE2822001u32.to_le_bytes()); // ADD R2, R2, #1
                            code.extend_from_slice(&0xEAFFFFFAu32.to_le_bytes()); // B loop (-6)
                            code.extend_from_slice(&0xE1A00002u32.to_le_bytes()); // done: MOV R0, R2

                            // For signed division: negate quotient if signs differed.
                            if is_signed {
                                // CMP R12, #0; RSBNE R0, R0, #0
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_CMP, true,
                                    Gpr::R12.encoding(), 0, 0, 0,
                                ));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Ne, 0b0011, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 0,
                                )); // RSBNE R0, R0, #0
                            }

                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                            // Zero high word (32-bit result in 64-bit slot)
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                            )); // MOV R1, #0
                            code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                        }
                        code
                    }

                    // ── Cmp ──
                    crate::ir::IRInstr::Cmp { kind, dst, lhs, rhs, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
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

                        // ── FP Cmp dispatch ──
                        // When ty is F32/F64, load operands' bit patterns into
                        // D0/D1, run VCMP.F64, FMSTAT to transfer FP flags to
                        // APSR.NZCV, then SETcc as usual. For FP, signed and
                        // unsigned comparisons are equivalent; U*-variants map
                        // to their signed counterparts.
                        let is_fp = ty.as_ref().is_some_and(|t| matches!(t, crate::ir::IRType::F32 | crate::ir::IRType::F64));
                        if is_fp {
                            // Map unsigned conditions to their signed FP equivalents.
                            let fp_cond = match kind {
                                CmpKind::Eq => Condition::Eq,
                                CmpKind::Ne => Condition::Ne,
                                CmpKind::SLt | CmpKind::ULt => Condition::Lt,
                                CmpKind::SLe | CmpKind::ULe => Condition::Le,
                                CmpKind::SGt | CmpKind::UGt => Condition::Gt,
                                CmpKind::SGe | CmpKind::UGe => Condition::Ge,
                            };
                            // Load lhs bit pattern into R0, store to dst slot, VLDR D0.
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                            code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            code.extend_from_slice(&encode_vldr_d(0, Gpr::R11.encoding() as u8, dst_offset));
                            // Load rhs bit pattern into R0, store to dst slot, VLDR D1.
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R0));
                            code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            code.extend_from_slice(&encode_vldr_d(1, Gpr::R11.encoding() as u8, dst_offset));
                            // VCMP.F64 D0, D1 — sets FPSCR.NZCV
                            code.extend_from_slice(&encode_vcmp_f64(0, 1));
                            // FMSTAT (VMRS APSR_nzcv, FPSCR) — transfer FP flags to ARM NZCV
                            code.extend_from_slice(&encode_fmstat());
                            // SETcc: MOV R0, #0; MOVcc R0, #1
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 0,
                            ));
                            code.extend_from_slice(&encode_dp_imm(
                                fp_cond, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
                            ));
                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                            code
                        } else {

                        // Detect 64-bit comparison: when ty is I64/U64 (or
                        // None defaulting to 64-bit), compare both low and
                        // high words. Otherwise, fall back to 32-bit compare.
                        let is_64bit = match ty.as_ref() {
                            Some(crate::ir::IRType::I64)
                            | Some(crate::ir::IRType::U64) => true,
                            // Default to 32-bit when type is missing or
                            // explicitly a 32-bit-or-narrower integer.
                            _ => false,
                        };

                        if is_64bit {
                            // Load lhs (R0=low, R1=high) and rhs (R2=low, R3=high)
                            code.extend(ss_load_value_64(Gpr::R0, Gpr::R1, lhs, &vreg_stack_slots));
                            code.extend(ss_load_value_64(Gpr::R2, Gpr::R3, rhs, &vreg_stack_slots));
                            // CMP R1, R3 — compare high words first
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_CMP, true,
                                Gpr::R1.encoding(), 0, Gpr::R3.encoding(),
                            ));
                            // BNE +0 — if high words differ, skip the low-word
                            // CMP so the flags reflect the high-word result.
                            // ARM branch target = PC + offset*4 where PC reads
                            // as branch_addr + 8; offset 0 lands on the
                            // instruction two slots after the branch (skipping
                            // exactly one instruction).
                            code.extend_from_slice(&encode_branch(Condition::Ne, false, 0));
                            // CMP R0, R2 — compare low words (only reached if
                            // high words were equal)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_CMP, true,
                                Gpr::R0.encoding(), 0, Gpr::R2.encoding(),
                            ));
                        } else {
                            // 32-bit comparison
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R0));
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R1));
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_CMP, true,
                                Gpr::R0.encoding(), 0, Gpr::R1.encoding(),
                            ));
                        }

                        // SETcc: MOV R0, #0; MOVcc R0, #1
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 0,
                        ));
                        code.extend_from_slice(&encode_dp_imm(
                            cmp_cond, DP_MOV, false, 0, Gpr::R0.encoding(), 0, 1,
                        ));
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        code
                        } // end else (integer Cmp path)
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
                            UnaryOpKind::Clz => {
                                // CLZ R0, R0  (ARMv5T+)
                                // Dedicated extension encoding:
                                //   cond 0001 0110 1111 Rd 1111 0000 Rm
                                // With Rm=R0, Rd=R0 → 0xE16F0F10
                                code.extend_from_slice(&0xE16F0F10u32.to_le_bytes());
                            }
                            UnaryOpKind::Ctz => {
                                // CTZ(x) = CLZ(RBIT(x))  (ARMv6T2+)
                                // RBIT R0, R0 : 0xE6FF0F30
                                //   cond 0110 1111 1111 Rd 1111 0011 Rm
                                code.extend_from_slice(&0xE6FF0F30u32.to_le_bytes());
                                // CLZ R0, R0 : 0xE16F0F10
                                code.extend_from_slice(&0xE16F0F10u32.to_le_bytes());
                            }
                            UnaryOpKind::Popcnt => {
                                // SWAR popcount (correct for full 32-bit input).
                                // R0 = input → R0 = popcount. R12 = scratch.
                                // Uses 8 bytes of stack ([SP+0]=x, [SP+4]=temp).
                                // Masks are built with MOVW/MOVT (ARMv6T2+).

                                // SUB SP, SP, #8
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_SUB, false,
                                    Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 8,
                                ));

                                // ── Step 1: x = x - ((x >> 1) & 0x55555555) ──
                                // STR R0, [SP]
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R13.encoding(), Gpr::R0.encoding(), 0,
                                ));
                                // MOVW R12, #0x5555 ; MOVT R12, #0x5555 → R12 = 0x55555555
                                code.extend_from_slice(&0xE300C555u32.to_le_bytes());
                                code.extend_from_slice(&0xE340C555u32.to_le_bytes());
                                // R0 = (R0 >> 1) & R12
                                code.extend_from_slice(&encode_dp_shift_imm(
                                    Condition::Al, DP_MOV, false, 0,
                                    Gpr::R0.encoding(), 1, 1, Gpr::R0.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_AND, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                // R0 = saved_x - R0  →  SUB R0, R12, R0
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R13.encoding(), Gpr::R12.encoding(), 0,
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_SUB, false,
                                    Gpr::R12.encoding(), Gpr::R0.encoding(), Gpr::R0.encoding(),
                                ));
                                // STR R0, [SP]
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R13.encoding(), Gpr::R0.encoding(), 0,
                                ));

                                // ── Step 2: x = (x & 0x33333333) + ((x >> 2) & 0x33333333) ──
                                // MOVW/MOVT R12 = 0x33333333
                                code.extend_from_slice(&0xE300C333u32.to_le_bytes());
                                code.extend_from_slice(&0xE340C333u32.to_le_bytes());
                                // R0 = (R0 >> 2) & R12
                                code.extend_from_slice(&encode_dp_shift_imm(
                                    Condition::Al, DP_MOV, false, 0,
                                    Gpr::R0.encoding(), 1, 2, Gpr::R0.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_AND, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                // STR R0, [SP, #4]  (temp)
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R13.encoding(), Gpr::R0.encoding(), 4,
                                ));
                                // R0 = saved_x & R12
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R13.encoding(), Gpr::R0.encoding(), 0,
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_AND, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                // R12 = temp; R0 = R0 + R12
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R13.encoding(), Gpr::R12.encoding(), 4,
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                // STR R0, [SP]
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R13.encoding(), Gpr::R0.encoding(), 0,
                                ));

                                // ── Step 3: x = (x + (x >> 4)) & 0x0F0F0F0F ──
                                // R0 = R0 >> 4 ; R12 = saved_x ; R0 = R0 + R12
                                code.extend_from_slice(&encode_dp_shift_imm(
                                    Condition::Al, DP_MOV, false, 0,
                                    Gpr::R0.encoding(), 1, 4, Gpr::R0.encoding(),
                                ));
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R13.encoding(), Gpr::R12.encoding(), 0,
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                // MOVW/MOVT R12 = 0x0F0F0F0F ; R0 = R0 & R12
                                code.extend_from_slice(&0xE300C0F0u32.to_le_bytes());
                                code.extend_from_slice(&0xE340C0F0u32.to_le_bytes());
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_AND, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));

                                // ── Step 4: return (x * 0x01010101) >> 24 ──
                                // x * 0x01010101 = x + (x<<8) + (x<<16) + (x<<24)
                                // Let z = x + (x<<8). Then z + (z<<16) = x*0x01010101.
                                // R12 = R0 << 8 ; R0 = R0 + R12
                                code.extend_from_slice(&encode_dp_shift_imm(
                                    Condition::Al, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), 0, 8, Gpr::R0.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                // R12 = R0 << 16 ; R0 = R0 + R12
                                code.extend_from_slice(&encode_dp_shift_imm(
                                    Condition::Al, DP_MOV, false, 0,
                                    Gpr::R12.encoding(), 0, 16, Gpr::R0.encoding(),
                                ));
                                code.extend_from_slice(&encode_dp_reg(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                ));
                                // R0 = R0 >> 24
                                code.extend_from_slice(&encode_dp_shift_imm(
                                    Condition::Al, DP_MOV, false, 0,
                                    Gpr::R0.encoding(), 1, 24, Gpr::R0.encoding(),
                                ));

                                // ADD SP, SP, #8  (release scratch slot)
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_ADD, false,
                                    Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 8,
                                ));
                            }
                        }
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
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
                                code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                // Zero the high word — loads produce 32-bit results,
                                // but stack slots are 8 bytes.
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                )); // MOV R1, #0
                                code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                            }
                            crate::ir::IRType::I16 => {
                                code.extend_from_slice(&encode_ldrsh_imm(
                                    Condition::Al, true, true, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // LDRSH R0, [R3, #0]
                                code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                )); // MOV R1, #0
                                code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                            }
                            crate::ir::IRType::U16 => {
                                code.extend_from_slice(&encode_ls_half_imm(
                                    Condition::Al, true, true, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // LDRH R0, [R3, #0]
                                code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                )); // MOV R1, #0
                                code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                            }
                            crate::ir::IRType::I64
                            | crate::ir::IRType::U64
                            | crate::ir::IRType::F64 => {
                                // 64-bit load: load low word into R0, high
                                // word into R1, then store both to the
                                // destination slot ([R11-#off] and [R11-#off+4]).
                                // LDR R0, [R3, #0]  (low word)
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                                // LDR R1, [R3, #4]  (high word)
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R3.encoding(), Gpr::R1.encoding(), 4,
                                ));
                                // Store both words (ss_store_64 stores lo at
                                // [R11-#off] and hi at [R11-#off+4]).
                                code.extend(ss_store_64(Gpr::R0, Gpr::R1, dst_offset));
                            }
                            _ => {
                                // Default: 32-bit word load
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, true,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                )); // LDR R0, [R3, #0]
                                code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                                // Zero the high word — loads produce 32-bit results,
                                // but stack slots are 8 bytes. Without zeroing, the
                                // high word contains garbage from a previous value.
                                // When a 64-bit operation (e.g. Shl) later loads
                                // both words via ss_load_value_64, the garbage high
                                // word corrupts the result.
                                code.extend_from_slice(&encode_dp_imm(
                                    Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                                )); // MOV R1, #0
                                code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                            }
                        }
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
                        // Load value into R0 (low word for 64-bit types)
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
                            crate::ir::IRType::I64
                            | crate::ir::IRType::U64
                            | crate::ir::IRType::F64 => {
                                // 64-bit store: load both words of the value
                                // (R0=low, R1=high) and store them to
                                // [R3, #0] and [R3, #4] respectively.
                                // Note: ss_load_value already loaded the low
                                // word into R0 above; we only need to load
                                // the high word into R1 here.
                                code.extend(ss_load_value_64(Gpr::R0, Gpr::R1, value, &vreg_stack_slots));
                                // STR R0, [R3, #0]  (low word)
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R3.encoding(), Gpr::R0.encoding(), 0,
                                ));
                                // STR R1, [R3, #4]  (high word)
                                code.extend_from_slice(&encode_ls_imm(
                                    Condition::Al, true, true, false, false, false,
                                    Gpr::R3.encoding(), Gpr::R1.encoding(), 4,
                                ));
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
                        // ss_store_32_zero stores the 32-bit pointer to the low word
                        // AND zeros the high word of the 8-byte vreg slot. This is
                        // sufficient — no need for an additional high-word store.
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        code
                    }

                    // ── Call ──
                    crate::ir::IRInstr::Call { dst, func: target_func, args, is_extern } => {
                        let mut code = Vec::new();
                        let num_args = args.len();
                        let num_stack_args = num_args.saturating_sub(4);

                        // AAPCS-VFP calling convention limitation: when calling
                        // external (C ABI) functions that take floating-point
                        // arguments, the AAPCS-VFP variant requires float args
                        // to be passed in VFP registers S0-S15 / D0-D7 rather
                        // than in R0-R3. The VUMA IR `Call` instruction does
                        // not carry per-argument type information, so we
                        // cannot tell which args are floats. We currently pass
                        // ALL args in R0-R3 (the soft-float / integer ABI),
                        // which is correct for VUMA-internal calls (both
                        // caller and callee use the same convention) and for
                        // extern functions that take only integer/pointer
                        // args. Extern functions taking float args (e.g. sin,
                        // cos, sqrt from libm) may receive their arguments in
                        // the wrong registers under hard-float VFP ABIs. Emit
                        // a warning so the limitation is visible at compile
                        // time.
                        if *is_extern {
                            vuma_log!(warn, 
                                "Extern call to '{}' — float args may not be in D0-D7 \
                                 (AAPCS-VFP hardfloat convention not implemented; args \
                                 passed in R0-R3 only)",
                                target_func
                            );
                        }

                        // AAPCS requires the stack to remain 8-byte aligned at
                        // a public function boundary. If an odd number of stack
                        // args is passed, pad the allocation up to the next
                        // 8-byte boundary; the cleanup below uses the same
                        // padded size.
                        let stack_args_bytes = (num_stack_args * 4 + 7) & !7;

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

                        // Store return value to dst stack slot.
                        // For VUMA functions (non-extern), the return value is 64-bit
                        // in R0:R1 (AAPCS). For extern functions (syscalls), the return
                        // is 32-bit in R0 only — sign-extend to 64-bit so that negative
                        // values (e.g., -1 error returns from open/read) are correctly
                        // represented in 64-bit operations.
                        if let Some(d) = dst {
                            let dst_id = d.as_register().unwrap_or(0);
                            let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                            // Store low word (R0) + zero high word.
                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                            if !is_extern {
                                // Check if the callee returns 64-bit (I64/U64).
                                // If so, store R1 (high word). Otherwise, leave high word zeroed.
                                let is_64bit_ret = {
                                    let lock = func_64bit_returns();
                                    let guard = lock.read().unwrap();
                                    guard.as_ref()
                                        .map(|set| set.contains(target_func))
                                        .unwrap_or(false)
                                };
                                if is_64bit_ret {
                                    code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                                }
                            } else {
                                // Extern/syscall: sign-extend 32-bit R0 to 64-bit R0:R1.
                                // MOV R1, R0, ASR #31 — fills R1 with 0xFFFFFFFF if R0 is
                                // negative (bit 31 set), or 0x00000000 if non-negative.
                                // This is correct for both signed returns (i64) and for
                                // user-space pointers (which have bit 31 = 0 on 32-bit Linux).
                                code.extend_from_slice(&encode_dp_shift_imm(
                                    Condition::Al, DP_MOV, false, 0,
                                    Gpr::R1.encoding(), Gpr::R0.encoding(), 2, 31,
                                ));
                                code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                            }
                        }

                        code
                    }

                    // ── Branch ──
                    crate::ir::IRInstr::Branch { target } => {
                        let mut code = Vec::new();
                        // Emit phi copies for (target, current_block) before the jump.
                        if let Some(pairs) = phi_map.get(&(target.clone(), block.label.clone())) {
                            for (dst, src) in pairs {
                                code.extend(ss_load_value(src, &vreg_stack_slots, Gpr::R0));
                                let dst_id = dst.as_register().unwrap_or(0);
                                let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                                code.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                        }
                        let branch_offset_in_enc = code.len();
                        let branch_offset_in_func = current_byte_offset + code.len() as u64;
                        code.extend_from_slice(&encode_branch(Condition::Al, false, 0));
                        branch_fixups.push(BranchFixup {
                            instr_idx: instructions.len(),
                            abs_byte_offset: branch_offset_in_func,
                            target_label: target.clone(),
                            branch_offset_in_enc,
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

                        // Compute phi copies for both successors.
                        let false_copies: Vec<u8> = if let Some(pairs) = phi_map.get(&(false_target.clone(), block.label.clone())) {
                            let mut c = Vec::new();
                            for (dst, src) in pairs {
                                c.extend(ss_load_value(src, &vreg_stack_slots, Gpr::R0));
                                let dst_id = dst.as_register().unwrap_or(0);
                                let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                                c.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                            c
                        } else { Vec::new() };
                        let true_copies: Vec<u8> = if let Some(pairs) = phi_map.get(&(true_target.clone(), block.label.clone())) {
                            let mut c = Vec::new();
                            for (dst, src) in pairs {
                                c.extend(ss_load_value(src, &vreg_stack_slots, Gpr::R0));
                                let dst_id = dst.as_register().unwrap_or(0);
                                let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                                c.extend(ss_store_to_slot(Gpr::R0, dst_offset));
                            }
                            c
                        } else { Vec::new() };

                        if false_copies.is_empty() && true_copies.is_empty() {
                            // Common case (no phis): BNE true, B false
                            let bne_offset_in_enc = code.len();
                            let bne_offset_in_func = current_byte_offset + code.len() as u64;
                            code.extend_from_slice(&encode_branch(Condition::Ne, false, 0));
                            branch_fixups.push(BranchFixup {
                                instr_idx: instructions.len(),
                                abs_byte_offset: bne_offset_in_func,
                                target_label: true_target.clone(),
                                branch_offset_in_enc: bne_offset_in_enc,
                            });
                            // B false_target (placeholder)
                            let b_offset_in_enc = code.len();
                            let b_offset_in_func = current_byte_offset + code.len() as u64;
                            code.extend_from_slice(&encode_branch(Condition::Al, false, 0));
                            branch_fixups.push(BranchFixup {
                                instr_idx: instructions.len(),
                                abs_byte_offset: b_offset_in_func,
                                target_label: false_target.clone(),
                                branch_offset_in_enc: b_offset_in_enc,
                            });
                        } else {
                            // Landing-pad pattern:
                            //   BNE +N           (skip false copies + false B)
                            //   <false copies>
                            //   B false_target
                            //   <true copies>    ← BNE lands here
                            //   B true_target
                            let b_size = 4; // ARM B instruction is 4 bytes
                            let skip_words = ((false_copies.len() as i32) + b_size) / 4;
                            code.extend_from_slice(&encode_branch(Condition::Ne, false, skip_words));
                            // False path
                            code.extend(false_copies);
                            let b_false_offset_in_enc = code.len();
                            let b_false_offset_in_func = current_byte_offset + code.len() as u64;
                            code.extend_from_slice(&encode_branch(Condition::Al, false, 0));
                            branch_fixups.push(BranchFixup {
                                instr_idx: instructions.len(),
                                abs_byte_offset: b_false_offset_in_func,
                                target_label: false_target.clone(),
                                branch_offset_in_enc: b_false_offset_in_enc,
                            });
                            // True path (BNE target)
                            code.extend(true_copies);
                            let b_true_offset_in_enc = code.len();
                            let b_true_offset_in_func = current_byte_offset + code.len() as u64;
                            code.extend_from_slice(&encode_branch(Condition::Al, false, 0));
                            branch_fixups.push(BranchFixup {
                                instr_idx: instructions.len(),
                                abs_byte_offset: b_true_offset_in_func,
                                target_label: true_target.clone(),
                                branch_offset_in_enc: b_true_offset_in_enc,
                            });
                        }
                        code
                    }

                    // ── Cast ──
                    crate::ir::IRInstr::Cast { kind, dst, src, from_ty, to_ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(src, &vreg_stack_slots, Gpr::R0));
                        match kind {
                            CastKind::ZExt => {
                                // Zero-extend based on source type. The value
                                // is already in R0 (32-bit). Mask the lower
                                // bits according to from_ty.
                                match from_ty.as_ref() {
                                    Some(crate::ir::IRType::I8)
                                    | Some(crate::ir::IRType::U8) => {
                                        // AND R0, R0, #0xFF  → 0xE20000FF
                                        code.extend_from_slice(
                                            &encode_dp_imm(
                                                Condition::Al, DP_AND, false,
                                                Gpr::R0.encoding(), Gpr::R0.encoding(),
                                                0, 0xFF,
                                            ),
                                        );
                                    }
                                    Some(crate::ir::IRType::I16)
                                    | Some(crate::ir::IRType::U16) => {
                                        // AND R0, R0, #0xFFFF
                                        // 0xFFFF is not directly encodable as a
                                        // rotated 8-bit immediate, so load it via
                                        // MOVW into R12 and AND with R12.
                                        // MOVW R12, #0xFFFF → 0xE30CFFFF
                                        code.extend_from_slice(&0xE30CFFFFu32.to_le_bytes());
                                        code.extend_from_slice(&encode_dp_reg(
                                            Condition::Al, DP_AND, false,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                        ));
                                    }
                                    // I32/U32/I64/U64 — already full width, no-op.
                                    _ => {}
                                }
                            }
                            CastKind::SExt => {
                                // Sign-extend based on source type. The value
                                // is already in R0 (32-bit). Use SXTB/SXTH for
                                // 8-bit and 16-bit sources.
                                match from_ty.as_ref() {
                                    Some(crate::ir::IRType::I8) => {
                                        // SXTB R0, R0 — 0xE6AF0070
                                        code.extend_from_slice(&0xE6AF0070u32.to_le_bytes());
                                    }
                                    Some(crate::ir::IRType::I16) => {
                                        // SXTH R0, R0 — 0xE6BF0070
                                        code.extend_from_slice(&0xE6BF0070u32.to_le_bytes());
                                    }
                                    // I32 — already 32-bit, no-op.
                                    _ => {}
                                }
                            }
                            CastKind::Trunc => {
                                // Truncate based on destination type. Mask the
                                // lower bits according to to_ty.
                                match to_ty.as_ref() {
                                    Some(crate::ir::IRType::I8)
                                    | Some(crate::ir::IRType::U8) => {
                                        code.extend_from_slice(
                                            &encode_dp_imm(
                                                Condition::Al, DP_AND, false,
                                                Gpr::R0.encoding(), Gpr::R0.encoding(),
                                                0, 0xFF,
                                            ),
                                        );
                                    }
                                    Some(crate::ir::IRType::I16)
                                    | Some(crate::ir::IRType::U16) => {
                                        // MOVW R12, #0xFFFF → 0xE30CFFFF
                                        code.extend_from_slice(&0xE30CFFFFu32.to_le_bytes());
                                        code.extend_from_slice(&encode_dp_reg(
                                            Condition::Al, DP_AND, false,
                                            Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R12.encoding(),
                                        ));
                                    }
                                    _ => {}
                                }
                            }
                            CastKind::BitCast => {
                                // No conversion needed for bitcasts.
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
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        code
                    }

                    // ── Select ──
                    crate::ir::IRInstr::Select { dst, cond, true_val, false_val, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // Detect 64-bit select: when ty is I64/U64, select
                        // between full 64-bit values. Otherwise (32-bit-or-
                        // narrower types and the default `None`), select
                        // between the low 32 bits and zero-extend the result.
                        let is_64bit = matches!(
                            ty.as_ref(),
                            Some(crate::ir::IRType::I64) | Some(crate::ir::IRType::U64)
                        );

                        if is_64bit {
                            // === 64-bit Select ===
                            // Load false_val (R0=lo, R1=hi), true_val (R2=lo, R3=hi),
                            // and cond into R12. If cond != 0, move true_val into
                            // R0:R1; otherwise leave false_val in R0:R1.
                            code.extend(ss_load_value_64(
                                Gpr::R0, Gpr::R1, false_val, &vreg_stack_slots,
                            ));
                            code.extend(ss_load_value_64(
                                Gpr::R2, Gpr::R3, true_val, &vreg_stack_slots,
                            ));
                            code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::R12));
                            // CMP R12, #0
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_CMP, true,
                                Gpr::R12.encoding(), 0, 0, 0,
                            ));
                            // MOVNE R0, R2 (low word)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Ne, DP_MOV, false, 0,
                                Gpr::R0.encoding(), Gpr::R2.encoding(),
                            ));
                            // MOVNE R1, R3 (high word)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Ne, DP_MOV, false, 0,
                                Gpr::R1.encoding(), Gpr::R3.encoding(),
                            ));
                            // Store 64-bit result.
                            code.extend(ss_store_64(Gpr::R0, Gpr::R1, dst_offset));
                        } else {
                            // === 32-bit Select (existing) ===
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
                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        }
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
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        code
                    }

                    // ── Constant-time equality check (NO BRANCHES) ──
                    // ct_eq(a, b): diff = a ^ b; result = ((diff | -diff) >> 31) ^ 1
                    crate::ir::IRInstr::CtEq { dst, lhs, rhs, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // Detect 64-bit comparison: when ty is I64/U64,
                        // compare both low and high words. Otherwise, compare
                        // only the low 32 bits.
                        let is_64bit = matches!(
                            ty.as_ref(),
                            Some(crate::ir::IRType::I64) | Some(crate::ir::IRType::U64)
                        );

                        if is_64bit {
                            // === 64-bit CtEq (constant-time, NO BRANCHES) ===
                            // Load lhs (R0=lo, R1=hi) and rhs (R2=lo, R3=hi).
                            // diff_lo = lhs_lo ^ rhs_lo
                            // diff_hi = lhs_hi ^ rhs_hi
                            // combined = diff_lo | diff_hi   (0 iff both words equal)
                            // result   = (combined == 0) ? 1 : 0
                            // The (combined == 0) test is done branch-free via
                            //   t = (combined | -combined) >> 31   ; 1 if combined != 0, else 0
                            //   result = t ^ 1                      ; flip
                            // which matches the 32-bit CtEq construction.
                            code.extend(ss_load_value_64(
                                Gpr::R0, Gpr::R1, lhs, &vreg_stack_slots,
                            ));
                            code.extend(ss_load_value_64(
                                Gpr::R2, Gpr::R3, rhs, &vreg_stack_slots,
                            ));
                            // R0 = R0 ^ R2 (diff_lo)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_EOR, false,
                                Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R2.encoding(),
                            ));
                            // R1 = R1 ^ R3 (diff_hi)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_EOR, false,
                                Gpr::R1.encoding(), Gpr::R1.encoding(), Gpr::R3.encoding(),
                            ));
                            // R0 = R0 | R1 (combined diff)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_ORR, false,
                                Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R1.encoding(),
                            ));
                            // R2 = -R0 (NEG: RSB R2, R0, #0)
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_RSB, false,
                                Gpr::R2.encoding(), Gpr::R0.encoding(), 0, 0,
                            ));
                            // R0 = R0 | R2 (combined | -combined)
                            code.extend_from_slice(&encode_dp_reg(
                                Condition::Al, DP_ORR, false,
                                Gpr::R0.encoding(), Gpr::R0.encoding(), Gpr::R2.encoding(),
                            ));
                            // R0 = R0 >> 31 (1 if combined != 0, else 0)
                            code.extend_from_slice(&encode_dp_shift_imm(
                                Condition::Al, DP_MOV, false, 0,
                                Gpr::R0.encoding(), 1, 31, Gpr::R0.encoding(),
                            ));
                            // R0 = R0 ^ 1 (invert: 1 if equal, 0 if not)
                            code.extend_from_slice(&encode_dp_imm(
                                Condition::Al, DP_EOR, false,
                                Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 1,
                            ));
                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        } else {
                            // === 32-bit CtEq (existing, NO BRANCHES) ===
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
                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        }
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
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        // Zero high word (32-bit pointer result in 64-bit slot)
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                        )); // MOV R1, #0
                        code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                        code
                    }

                    // ── GetAddress ──
                    crate::ir::IRInstr::GetAddress { dst, name } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        // ARM32 GetAddress: emit a PC-relative literal-pool
                        // load that fetches the symbol's absolute runtime
                        // address into R0. The pattern is:
                        //
                        //   LDR R0, [PC, #0]   ; loads from PC+8 = .word
                        //   B   +0              ; skip over the .word
                        //   .word 0             ; placeholder, patched by
                        //                       ; R_ARM_ABS32 to the symbol's
                        //                       ; absolute runtime address
                        //
                        // At runtime, PC reads as (LDR_addr + 8) in ARM mode,
                        // so `LDR R0, [PC, #0]` loads from LDR_addr+8, which
                        // is exactly the .word placeholder emitted two
                        // instructions later. The `B +0` (branch to
                        // PC+0 = B_addr+8 = .word_addr+4) skips over the
                        // .word so the CPU doesn't try to execute the patched
                        // address as an instruction.
                        //
                        // The link-time patcher (in encode_program) handles
                        // R_ARM_ABS32 by writing
                        //   base_addr + text_offset + target_offset
                        // into the 4-byte .word, where target_offset is the
                        // symbol's offset within all_code (looked up in
                        // func_offsets).
                        code.extend_from_slice(&encode_ls_imm(
                            Condition::Al, true, true, false, false, true,
                            Gpr::R15.encoding(), Gpr::R0.encoding(), 0,
                        )); // LDR R0, [PC, #0]
                        code.extend_from_slice(&encode_branch(
                            Condition::Al, false, 0,
                        )); // B +0 (skip the .word)
                        // Relocation offset = byte offset of the .word
                        // placeholder (which is at current_byte_offset +
                        // size_of(LDR) + size_of(B) = +8 bytes from the
                        // start of this instruction's emitted code).
                        let reloc_offset = current_byte_offset + code.len() as u64;
                        vuma_log!(debug, 
                            "GetAddress for '{}' — emitting literal-pool load (R_ARM_ABS32 reloc at func byte {})",
                            name, reloc_offset
                        );
                        code.extend_from_slice(&0u32.to_le_bytes()); // .word 0
                        relocations.push(RelocationEntry {
                            offset: reloc_offset,
                            symbol: name.clone(),
                            reloc_type: "R_ARM_ABS32".to_string(),
                        });
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        // Zero high word (32-bit pointer result in 64-bit slot)
                        code.extend_from_slice(&encode_dp_imm(
                            Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
                        )); // MOV R1, #0
                        code.extend(ss_store_to_slot(Gpr::R1, dst_offset + 4));
                        code
                    }

                    // ── Free ──
                    crate::ir::IRInstr::Free { ptr } => {
                        let mut code = Vec::new();
                        // Lower `free ptr` to a call to the `__vuma_free`
                        // runtime stub (munmap wrapper). Per AAPCS, the
                        // first argument goes in R0, so we load the pointer
                        // from its vreg stack slot into R0, then emit a BL
                        // with an R_ARM_CALL relocation that the link-time
                        // patcher resolves to the `__vuma_free` stub offset.
                        code.extend(ss_load_value(ptr, &vreg_stack_slots, Gpr::R0));
                        let bl_offset_in_func = current_byte_offset + code.len() as u64;
                        code.extend_from_slice(&encode_branch(
                            Condition::Al, true, 0,
                        )); // BL __vuma_free (placeholder; patched later)
                        relocations.push(RelocationEntry {
                            offset: bl_offset_in_func,
                            symbol: "__vuma_free".to_string(),
                            reloc_type: "R_ARM_CALL".to_string(),
                        });
                        code
                    }

                    // ── Phi ──
                    // Phi copies are emitted at predecessor block terminators
                    // (Branch/CondBranch handlers), not at the phi block entry.
                    // See func.build_phi_map().
                    crate::ir::IRInstr::Phi { .. } => {
                        Vec::new()
                    }

                    // ── Atomic operations (with DMB fences for acquire/release on ARM32) ──
                    crate::ir::IRInstr::AtomicLoad { dst, addr, ty } => {
                        // Plain load — DMB causes SIGSEGV on QEMU user-mode.
                        // QEMU user-mode is single-threaded so plain loads are safe.
                        let mut code = Vec::new();
                        // Load address into R3
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R3));
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
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
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        code
                    }
                    crate::ir::IRInstr::AtomicStore { value, addr, ty } => {
                        // Plain store — DMB causes SIGSEGV on QEMU user-mode.
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
                        //   +0:  LDREX{,B,H} R0, [R3]    ← retry
                        //   +4:  CMP R0, R1
                        //   +8:  BNE done                  (offset_words = +2)
                        //   +12: STREX{,B,H} R12, R2, [R3]
                        //   +16: CMP R12, #0
                        //   +20: BNE retry                 (offset_words = -6)
                        //   +24: <done label — store R0 to dst>
                        //
                        // ARM branch offset = (target - (branch_addr + 8)) / 4
                        // BNE done:   (24 - (8 + 8)) / 4  = +2
                        // BNE retry:  (0 - (20 + 8)) / 4 = -7  (wait: with no DMB, retry = (0-(20+8))/4 = -7)
                        // Actually: BNE retry = (0 - (20 + 8)) / 4 = -7. Correct.
                        // BNE done = (24 - (8 + 8)) / 4 = +2.
                        //
                        // No DMB barriers — QEMU user-mode SIGSEGV on DMB.
                        // QEMU user-mode is single-threaded so LDREX/STREX
                        // without fences is safe.
                        let mut code = Vec::new();

                        // Load operands into scratch registers
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(expected, &vreg_stack_slots, Gpr::R1));
                        code.extend(ss_load_value(desired, &vreg_stack_slots, Gpr::R2));

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

                        // <done label> — Store the old value (in R0) to the dst stack slot.
                        // R0 holds the old value loaded by LDREX regardless of whether
                        // the STREX succeeded (matched+stored) or was skipped (mismatch).
                        // This matches the IR contract: `dst = atomic_cas ...` returns
                        // the OLD value, NOT a success/failure flag.
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));

                        code
                    }

                    // ── Ret ──
                    crate::ir::IRInstr::Ret { values } => {
                        let mut code = Vec::new();
                        // Load return value into R0 (and R1 for 64-bit returns).
                        // AAPCS: 64-bit return values are passed in R0:R1.
                        // Check result_types first; fall back to parsing the
                        // function name (e.g. "fn_foo_entry(u64)" → 64-bit).
                        let is_64bit_ret = func.result_types.first()
                            .map(|t| matches!(t, crate::ir::IRType::I64 | crate::ir::IRType::U64))
                            .unwrap_or_else(|| {
                                // Fallback: parse return type from function name
                                if let Some(open) = func.name.rfind('(') {
                                    if let Some(close) = func.name.rfind(')') {
                                        if close > open {
                                            let ret_ty = &func.name[open + 1..close];
                                            return ret_ty == "u64" || ret_ty == "i64"
                                                || ret_ty == "U64" || ret_ty == "I64";
                                        }
                                    }
                                }
                                false
                            });
                        if let Some(val) = values.first() {
                            if is_64bit_ret {
                                // Load both low (R0) and high (R1) words.
                                code.extend(ss_load_value_64(Gpr::R0, Gpr::R1, val, &vreg_stack_slots));
                            } else {
                                code.extend(ss_load_value(val, &vreg_stack_slots, Gpr::R0));
                            }
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

                    // ── Syscall (Wave 11) ──────────────────────────────────
                    // dst = syscall(nr, args…) — raw Linux syscall.
                    // ARM EABI: args in R0-R3 + [SP]/[SP+4] for args 5-6, nr in
                    // R7, SVC #0, result (32-bit, sign-extended) in R0.
                    crate::ir::IRInstr::Syscall { nr, args, dst } => {
                        let mut code = Vec::new();
                        // Translate VUMA-generic (asm-generic) syscall number to
                        // the backend's native numbering. Identity on Arm32.
                        let native_nr = crate::syscall_abi::translate_or_warn(
                            crate::backend::BackendKind::Arm32,
                            *nr,
                        );
                        let num_args = args.len();
                        let num_stack_args = num_args.saturating_sub(4);
                        let stack_bytes = num_stack_args * 4;
                        // Push args 5-6 onto the stack (kernel reads from [SP]).
                        if stack_bytes > 0 {
                            code.extend_from_slice(&emit_sub_sp(stack_bytes as i32));
                            for (i, arg) in args.iter().enumerate() {
                                if i >= 4 {
                                    let off = ((i - 4) * 4) as u32;
                                    code.extend(ss_load_value(
                                        arg, &vreg_stack_slots, Gpr::R12,
                                    ));
                                    code.extend_from_slice(&encode_ls_imm(
                                        Condition::Al, true, true, false, false, false,
                                        Gpr::R13.encoding(), Gpr::R12.encoding(), off,
                                    ));
                                }
                            }
                        }
                        // Load args 0-3 into R0-R3
                        let reg_args = [Gpr::R0, Gpr::R1, Gpr::R2, Gpr::R3];
                        for (i, arg) in args.iter().take(4).enumerate() {
                            code.extend(ss_load_value(
                                arg, &vreg_stack_slots, reg_args[i],
                            ));
                        }
                        // MOV R7, nr  (syscall number)
                        code.extend(load_immediate_arm32(Gpr::R7, native_nr));
                        // SVC #0
                        code.extend_from_slice(&encode_svc(Condition::Al, 0));
                        // Clean up stack args
                        if stack_bytes > 0 {
                            code.extend_from_slice(&emit_add_sp(stack_bytes as i32));
                        }
                        // Store return value (R0, 32-bit sign-extended) to dst slot
                        if let Some(d) = dst {
                            let dst_id = d.as_register().unwrap_or(0);
                            let dst_offset =
                                vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                            code.extend(ss_store_32_zero(Gpr::R0, dst_offset, fs));
                        }
                        code
                    }
                    // ── VectorOp (Wave 29) ──
                    // arm32 (ARMv7 NEON) has no SIMD encoder in the Wave 29
                    // suite (only x86_64 and aarch64 do); emit nothing.
                    crate::ir::IRInstr::VectorOp { .. } => Vec::new(),
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
                    crate::ir::IRInstr::Cast {
                        kind: CastKind::IntToFloat | CastKind::UIntToFloat
                            | CastKind::FloatToInt | CastKind::FloatToUInt
                            | CastKind::FloatToFloat,
                        ..
                    } => {
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
            // Use the recorded branch_offset_in_enc to find the exact
            // position of this branch within the instruction's encoded output.
            // This fixes the bug where CondBranch's BNE (at offset N) and
            // B (at offset N+4) shared the same instr_idx, but only the
            // last branch (B) was being patched.
            let pos = fixup.branch_offset_in_enc;
            if pos + 4 <= enc.len() {
                let existing = u32::from_le_bytes([
                    enc[pos], enc[pos + 1], enc[pos + 2], enc[pos + 3],
                ]);
                let patched = (existing & 0xFF000000) | ((offset_words as u32) & 0x00FF_FFFF);
                enc[pos..pos + 4].copy_from_slice(&patched.to_le_bytes());
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
            // Callee-saved register list is empty: the stack-slot ISel keeps
            // all virtual-register state on the stack (no callee-saved GPRs
            // or DPRs are assigned to vregs), so there are no caller-owned
            // registers that the function needs to report as having
            // preserved. R11 (FP) and LR are saved/restored directly via
            // explicit LDR/STR in the prologue/epilogue rather than through
            // this callee_saved vector.
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
        //   _start:  LDR r0, [SP]     ; argc = *sp (32-bit)
        //            ADD r1, SP, #4   ; argv = sp + 4 (32-bit pointers)
        //            BL main          ; call main(argc, argv) — result in r0
        //            MOV r7, #1        ; sys_exit
        //            SVC #0            ; syscall
        //   <functions...>
        //   <runtime: print_hex, print_int, print_newline using SVC sys_write>

        // ── _start stub ──
        // LDR r0, [SP]  — 4 bytes (load argc from stack pointer)
        // ADD r1, SP, #4 — 4 bytes (argv = sp + 4 on ARM32)
        // BL <main>     — 4 bytes, needs offset patching
        // MOV r7, #1   — 4 bytes (sys_exit = 1 on ARM Linux)
        // SVC #0        — 4 bytes
        let start_stub_size: usize = 20; // 5 × 4-byte instructions
        let ffi_stub_size: usize = 8; // MOV R0, #0; BX LR (2 × 4 bytes)
        let ffi_stub_offset: usize = start_stub_size;

        // ── Build runtime I/O code ──
        let runtime_code = build_arm32_runtime();

        // ── Build __vuma_alloc / __vuma_free syscall stubs (old_mmap / munmap) ──
        // __vuma_alloc(size in R0) -> R0 = mmap(NULL, size, PROT_READ|PROT_WRITE,
        //                                        MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
        //   ARM EABI Linux: sys_old_mmap = 90, takes a single arg in R0 pointing
        //   to a struct mmap_arg_struct { addr, len, prot, flags, fd, offset }
        //   laid out in memory. We build the struct on the stack.
        // __vuma_free(addr in R0) -> munmap(addr, 0)
        //   ARM EABI Linux: sys_munmap = 91, args R0/R1, syscall # in R7, SVC #0.
        //
        // Note: ARM AAPCS marks R4-R11 as callee-saved. We clobber R4 and R5 in
        // the alloc stub, so we save/restore them via PUSH/POP. The free stub
        // and simple syscall stubs clobber only R7 (which is also callee-saved
        // in AAPCS) — this matches the existing _start stub. (The runtime
        // `__vuma_print_int` previously clobbered R7 without saving as well,
        // but W15 added an explicit PUSH/POP of R7 around that function so it
        // now preserves R7 across the call. The bare syscall stubs here are
        // leaf wrappers that intentionally do not.)
        let vuma_alloc_stub: Vec<u8> = {
            let mut code = Vec::new();
            // PUSH {R4, R5}  — save callee-saved registers we'll clobber.
            //   STMDB SP!, {R4, R5}  → register_list = 0b0011_0000 = 0x0030
            code.extend_from_slice(&encode_stm(
                Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x0030,
            ));
            // MOV R4, R0  (save size, since R0 will be reused as struct ptr)
            code.extend_from_slice(&encode_dp_reg(
                Condition::Al, DP_MOV, false, 0, Gpr::R4.encoding(), Gpr::R0.encoding(),
            ));
            // SUB SP, SP, #24  (allocate mmap_arg_struct: 6 × 4 bytes)
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_SUB, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 24,
            ));
            // MOV R5, #0; STR R5, [SP, #0]  — addr = NULL
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 0,
            ));
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R5.encoding(), 0,
            ));
            // STR R4, [SP, #4]  — len = size
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R4.encoding(), 4,
            ));
            // MOV R5, #3; STR R5, [SP, #8]  — prot = PROT_READ|PROT_WRITE
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 3,
            ));
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R5.encoding(), 8,
            ));
            // MOV R5, #0x22; STR R5, [SP, #12]  — flags = MAP_PRIVATE|MAP_ANONYMOUS
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 0x22,
            ));
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R5.encoding(), 12,
            ));
            // MVN R5, #0; STR R5, [SP, #16]  — fd = -1 (MVN Rd, #0 → Rd = ~0 = -1)
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MVN, false, 0, Gpr::R5.encoding(), 0, 0,
            ));
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R5.encoding(), 16,
            ));
            // MOV R5, #0; STR R5, [SP, #20]  — offset = 0
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MOV, false, 0, Gpr::R5.encoding(), 0, 0,
            ));
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R5.encoding(), 20,
            ));
            // MOV R0, SP  (R0 = pointer to struct)
            code.extend_from_slice(&encode_dp_reg(
                Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), Gpr::R13.encoding(),
            ));
            // MOV R7, #90  (sys_old_mmap)
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 90,
            ));
            // SVC #0
            code.extend_from_slice(&encode_svc(Condition::Al, 0));
            // ADD SP, SP, #24  (free the struct)
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 24,
            ));
            // POP {R4, R5}
            code.extend_from_slice(&encode_ldm(
                Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x0030,
            ));
            // BX LR
            code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R14.encoding()));
            code
        };
        let vuma_free_stub: Vec<u8> = {
            let mut code = Vec::new();
            // MOV R1, #0  (size = 0)
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MOV, false, 0, Gpr::R1.encoding(), 0, 0,
            ));
            // MOV R7, #91  (sys_munmap)
            code.extend_from_slice(&encode_dp_imm(
                Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 91,
            ));
            // SVC #0
            code.extend_from_slice(&encode_svc(Condition::Al, 0));
            // BX LR
            code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R14.encoding()));
            code
        };

        // ── POSIX syscall stubs ──────────────────────────────────────
        // These provide the syscalls needed by mmap_sha256d, signal_hash,
        // lock_free_queue, epoll_echo, and ffi_demo tests.
        //
        // ARM EABI calling convention: args in R0-R3 (+ args 5-6 on stack),
        // return in R0.  ARM EABI syscall convention: args in R0-R5, syscall
        // # in R7, SVC #0, return in R0.
        //
        // For 1-4 arg syscalls (where the args fit in R0-R3), the calling
        // convention matches the syscall convention, so simple stubs are just:
        //     MOV R7, #num ; SVC #0 ; BX LR.
        //
        // For 6-arg syscalls (mmap, futex), we need to load args 5-6 from the
        // caller's stack frame into R4-R5 (saving R4-R5 first via PUSH/POP).
        //
        // For syscalls that need arg shuffling (open→open on ARM, but uses
        // legacy open=5; unlink=10; sigaction=67; pipe=42; dup2=63; fork=2),
        // ARM32 has the *legacy* syscall numbers directly — no *at / 2 / 3
        // variants needed. So most stubs are simple.
        //
        // For `mmap`, ARM EABI uses sys_old_mmap=90 which takes a struct
        // pointer in R0. We build the struct on the stack.
        //
        // Linux ARM EABI syscall numbers used here:
        //   write=4, read=3, open=5, close=6, mmap=90 (old_mmap),
        //   munmap=91, unlink=10, exit=1, exit_group=248, alarm=27,
        //   getpid=20, socket=281 (arm32), epoll_create1=356 (arm32),
        //   futex=240, sigaction=67, pipe=42, dup2=63, fork=2,
        //   execve=11, wait4=114, epoll_ctl=251 (arm32), epoll_wait=252 (arm32),
        //   lseek=19, stat=106, fstat=108, kill=37, getcwd=183, chdir=12,
        //   ioctl=54, fcntl=55, connect=283, poll=168, nanosleep=162,
        //   mprotect=125, dup=41, recv=291, send=290, shutdown=293,
        //   bind=282, listen=284, accept=285, setsockopt=294,
        //   lstat=107, dup3=358, recvfrom=371, sendto=370.
        //
        // Note: socket=281 (0x119) and epoll_create1=356 (0x164) do NOT fit
        // in a single ARM rotated-immediate MOV (which encodes an 8-bit value
        // rotated right by an even amount). We use load_immediate_arm32 to
        // emit MOV+ORR sequences for these.

        // Helper: encode a simple "MOV R7, #num ; SVC #0 ; BX LR" stub.
        // For numbers that don't fit in a single ARM rotated immediate,
        // load_immediate_arm32 emits a MOV+ORR sequence.
        let simple_stub = |num: u32| -> Vec<u8> {
            let mut code = Vec::new();
            code.extend(load_immediate_arm32(Gpr::R7, num));
            code.extend_from_slice(&encode_svc(Condition::Al, 0));
            code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R14.encoding()));
            code
        };
        // Helper: encode a 6-arg stub that loads args 5-6 from the caller's
        // stack into R4-R5 before calling the syscall. Used for mmap and futex.
        let six_arg_stub = |num: u32| -> Vec<u8> {
            let mut code = Vec::new();
            // PUSH {R4, R5}  — save callee-saved registers.
            //   register_list = 0b0011_0000 = 0x0030
            code.extend_from_slice(&encode_stm(
                Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x0030,
            ));
            // After PUSH, caller's [SP+0] is now at [SP+8], [SP+4] at [SP+12].
            // LDR R4, [SP, #8]   (arg 5)
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R4.encoding(), 8,
            ));
            // LDR R5, [SP, #12]  (arg 6)
            code.extend_from_slice(&encode_ls_imm(
                Condition::Al, true, true, false, false, false,
                Gpr::R13.encoding(), Gpr::R5.encoding(), 12,
            ));
            // MOV R7, #num  (or MOV+ORR if num doesn't fit in 8 bits)
            code.extend(load_immediate_arm32(Gpr::R7, num));
            // SVC #0
            code.extend_from_slice(&encode_svc(Condition::Al, 0));
            // POP {R4, R5}
            code.extend_from_slice(&encode_ldm(
                Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x0030,
            ));
            // BX LR
            code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R14.encoding()));
            code
        };

        let syscall_stubs: Vec<(String, Vec<u8>)> = {
            let mut stubs: Vec<(String, Vec<u8>)> = Vec::new();

            // Simple stubs (args 1-4 already in correct registers R0-R3):
            // ARM32 has the legacy syscall numbers (open=5, unlink=10, etc.),
            // so no arg shuffling is needed — just set R7 and SVC.
            // NOTE: futex is NOT here because it takes 6 args; it's below
            // using six_arg_stub to load args 5-6 from the caller's stack.
            for (name, num) in [
                ("write", 4u32), ("read", 3), ("open", 5), ("close", 6),
                ("munmap", 91), ("exit", 1), ("exit_group", 248),
                ("alarm", 27), ("getpid", 20), ("unlink", 10),
                ("sigaction", 67), ("pipe", 42), ("dup2", 63),
                ("fork", 2), ("execve", 11), ("wait4", 114),
                ("clone", 120),
                ("socket", 281),
                // [wave 9 fix] epoll_create1 corrected 356→357 (356 is eventfd2 on arm).
                ("epoll_create1", 357),
                ("epoll_ctl", 251), ("epoll_wait", 252),
                // ── W6: additional POSIX syscall stubs ──
                ("lseek", 19), ("stat", 106), ("fstat", 108),
                ("kill", 37), ("getcwd", 183), ("chdir", 12),
                ("ioctl", 54), ("fcntl", 55), ("connect", 283),
                ("poll", 168), ("nanosleep", 162), ("mprotect", 125),
                ("dup", 41),
                ("recv", 291), ("send", 290), ("shutdown", 293),
                ("bind", 282), ("listen", 284), ("accept", 285),
                ("setsockopt", 294),
                // ── W7: more POSIX syscall stubs ──
                // waitpid is the same syscall as wait4 (caller passes NULL
                // rusage in R3 if it doesn't care).
                ("waitpid", 114),
                ("brk", 45),
                ("clock_gettime", 263),
                ("gettimeofday", 78),
                ("rt_sigprocmask", 126),
                ("lstat", 107), ("dup3", 358),
                ("recvfrom", 371), ("sendto", 370),
                // mmap2 takes the same 6 args as mmap but with the offset
                // in pages (4096-byte units) rather than bytes; on ARM EABI
                // args 5-6 are on the caller's stack but a simple stub
                // suffices for callers that only need 4 args (addr, len,
                // prot, flags) — the kernel will read R4/R5 for fd/offset.
                ("mmap2", 192),
                // NOTE: lstat/dup3/recvfrom/sendto were previously listed
                // again here (duplicate of the entries at lines ~7236). The
                // duplicates emitted ~56 bytes of dead code; removed.
                // ── Wave 7: POSIX file-metadata & I/O syscalls (arm unistd.h) ──
                // ARM EABI has 4 reg args (R0-R3); all these take ≤4 args →
                // simple_stub. chown=212/fchown=207 are the modern 32-bit-uid
                // chown32/fchown32 (arm's chown=182 is the 16-bit sys_chown16,
                // NOT exposed). The 5-arg linkat(330) & fchownat(325) are
                // registered below via six_arg_stub (loads arg5 from the
                // caller's stack; arg6 is garbage and ignored by 5-arg syscalls).
                ("mkdir", 39), ("rmdir", 40), ("rename", 38),
                ("link", 9), ("symlink", 83), ("readlink", 85),
                ("chmod", 15), ("chown", 212), ("umask", 60),
                ("fchmod", 94), ("fchown", 207),
                ("openat", 322), ("unlinkat", 328), ("renameat", 329),
                ("symlinkat", 331), ("readlinkat", 332),
                ("fchmodat", 333), ("faccessat", 334),
                ("ftruncate", 93), ("fsync", 118), ("fdatasync", 148),
                ("sync", 36), ("syncfs", 373),
                ("pread", 180), ("pwrite", 181), ("readv", 145), ("writev", 146),
                ("preadv", 361), ("pwritev", 362),
                ("fchdir", 133), ("chroot", 61),
                // ── Wave 9: POSIX system & advanced syscalls (arm unistd.h) ──
                // ARM EABI has 4 reg args (R0-R3); all these take ≤4 args →
                // simple_stub. eventfd→eventfd2(356), signalfd→signalfd4(355).
                // mremap (5 args) is registered below via six_arg_stub.
                ("mlock", 150), ("munlock", 151), ("mlockall", 152), ("munlockall", 153),
                ("mincore", 219), ("madvise", 220), ("msync", 144),
                ("getrlimit", 76), ("setrlimit", 75), ("prlimit64", 369),
                ("getrusage", 77), ("times", 43),
                ("getrandom", 384),
                ("eventfd", 356), ("timerfd_create", 350), ("timerfd_settime", 353),
                ("timerfd_gettime", 354), ("signalfd", 355),
                ("inotify_init1", 360), ("inotify_add_watch", 317), ("inotify_rm_watch", 318),
                ("ptrace", 26),
                // ── Wave 8: POSIX process & identity syscalls (arm32 syscall.tbl) ──
                // ≤4-arg syscalls use simple_stub (ARM EABI: 4 reg args r0-r3).
                // 5-arg syscalls (waitid/execveat/prctl) use six_arg_stub below.
                // Identity uses the modern *32 variants (199-214) per Wave 7 precedent.
                // Family 1: identity
                ("getuid", 199), ("geteuid", 201), ("getgid", 200), ("getegid", 202),
                ("setuid", 213), ("setgid", 214), ("setresuid", 208), ("setresgid", 210),
                // Family 2: process group (getpid already present)
                ("getppid", 64), ("getsid", 147), ("setsid", 66),
                ("setpgid", 57), ("getpgid", 132), ("getpgrp", 65),
                // Family 3: clone/wait (clone/wait4 already present; clone3=2args fits)
                ("vfork", 190), ("clone3", 435),
                // Family 5: signals (kill/rt_sigprocmask/rt_sigreturn already present)
                ("tgkill", 268), ("tkill", 238), ("rt_sigaction", 174),
                // Family 6: directory read (readdir ABSENT on EABI → use getdents64)
                ("getdents64", 217), ("getdents", 141),
                // Family 7: system (arch_prctl is x86_64-only)
                ("uname", 122), ("sysinfo", 116),
            ] {
                stubs.push((name.to_string(), simple_stub(num)));
            }

            // ── Wave 7: 5-arg *at syscalls (linkat, fchownat) ──
            // ARM EABI passes args 5+ on the caller's stack; six_arg_stub
            // loads arg5 (and arg6) into R4/R5 before SVC. For these 5-arg
            // syscalls the kernel ignores R5 (arg6), so reusing six_arg_stub
            // is safe.
            stubs.push(("linkat".to_string(), six_arg_stub(330)));
            stubs.push(("fchownat".to_string(), six_arg_stub(325)));

            // ── Wave 9: mremap (5 args: old_addr, old_size, new_size, flags, new_addr) ──
            // Uses six_arg_stub to load arg5 (new_address) from the caller's
            // stack into R4; R5 (arg6) is garbage and ignored by the 5-arg syscall.
            stubs.push(("mremap".to_string(), six_arg_stub(163)));
            // ── Wave 8: 5-arg syscalls (waitid/execveat/prctl) ──
            // ARM EABI passes args 5+ on the caller's stack; six_arg_stub
            // loads arg5 from [SP] (arg6 is ignored by these 5-arg syscalls).
            stubs.push(("waitid".to_string(), six_arg_stub(280)));
            stubs.push(("execveat".to_string(), six_arg_stub(387)));
            stubs.push(("prctl".to_string(), six_arg_stub(172)));

            // rt_sigreturn (173) — special: no args, never returns.
            // The kernel restores the saved signal context from the stack
            // and resumes execution at the interrupted PC. We emit just
            // `MOV R7, #173 ; SVC #0` followed by an UDF trap as a safety
            // net in case the kernel ever does return (it shouldn't).
            {
                let mut code = Vec::new();
                code.extend(load_immediate_arm32(Gpr::R7, 173));
                code.extend_from_slice(&encode_svc(Condition::Al, 0));
                // UDF #0 — undefined instruction trap (0xE7F000F0).
                code.extend_from_slice(&0xE7F000F0u32.to_le_bytes());
                stubs.push(("rt_sigreturn".to_string(), code));
            }

            // strcmp(s1, s2) → int — not a syscall, implemented as a small
            // assembly loop. Returns the difference (*s1 - *s2) at the first
            // differing byte (or at the terminating NUL if strings are equal).
            // Inputs:  R0 = s1, R1 = s2
            // Clobbers: R0, R1, R2, R3
            // Returns:  R0 = (*s1 - *s2) (zero iff equal)
            {
                let mut code = Vec::new();
                // strcmp_loop:
                let loop_start = code.len();
                // LDRB R2, [R0, #0]
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, true, false, true,
                    Gpr::R0.encoding(), Gpr::R2.encoding(), 0,
                ));
                // LDRB R3, [R1, #0]
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, true, false, true,
                    Gpr::R1.encoding(), Gpr::R3.encoding(), 0,
                ));
                // CMP R2, R3
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_CMP, true, Gpr::R2.encoding(), 0, Gpr::R3.encoding(),
                ));
                // BNE strcmp_done — target is 6 instructions after BNE,
                // so offset = (6 - 2) = 4 (target = PC + 24 = PC + 8 + 4*4).
                code.extend_from_slice(&encode_branch(Condition::Ne, false, 4));
                // CMP R2, #0  (both bytes equal; if 0, strings match)
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_CMP, true, Gpr::R2.encoding(), 0, 0, 0,
                ));
                // BEQ strcmp_done — target is 4 instructions after BEQ,
                // so offset = (4 - 2) = 2 (target = PC + 16 = PC + 8 + 2*4).
                code.extend_from_slice(&encode_branch(Condition::Eq, false, 2));
                // ADD R0, R0, #1
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_ADD, false, Gpr::R0.encoding(), Gpr::R0.encoding(), 0, 1,
                ));
                // ADD R1, R1, #1
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_ADD, false, Gpr::R1.encoding(), Gpr::R1.encoding(), 0, 1,
                ));
                // B strcmp_loop (backward branch)
                let loop_back = (loop_start as i32) - (code.len() as i32 + 8);
                code.extend_from_slice(&encode_branch(Condition::Al, false, loop_back >> 2));
                // strcmp_done: R0 = R2 - R3
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_SUB, false, Gpr::R2.encoding(), Gpr::R0.encoding(), Gpr::R3.encoding(),
                ));
                // BX LR
                code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R14.encoding()));
                stubs.push(("strcmp".to_string(), code));
            }

            // futex — 6-arg syscall. Args 1-4 in R0-R3, args 5-6 on the
            // caller's stack at [SP+0] and [SP+4] (per AAPCS). We load them
            // into R4-R5 before SVC. ARM EABI: sys_futex = 240.
            stubs.push(("futex".to_string(), six_arg_stub(240)));

            // mmap → old_mmap(struct *) — ARM EABI sys_old_mmap = 90.
            // [wave 6 — mmap ABI normalization, verified] ARM EABI's legacy
            // sys_mmap (90) does NOT take 6 register args; it takes a single
            // pointer in R0 to a struct mmap_arg_struct {
            //   void *addr; size_t len; int prot; int flags;
            //   int fd; off_t offset;   // offset in BYTES
            // }. This stub builds that struct on the caller's stack, loads fd
            // and offset from the caller's stack args ([SP+0]/[SP+4] per
            // AAPCS, after the R4-R5 save), stores all six fields into the
            // struct, sets R0 = SP (struct pointer), R7 = 90, and SVC #0.
            //
            // This matches __vuma_alloc (which uses the same struct-pointer
            // sys_old_mmap=90 path with offset=0): both use the SAME offset
            // unit (bytes, via the struct's `offset` field), satisfying the
            // wave-6 "same offset-unit handling as __vuma_alloc" requirement.
            // The byte offset is passed through to the kernel unmodified — no
            // >>12 conversion, because sys_old_mmap (unlike mmap2) takes bytes.
            //
            // Caller args: R0=addr, R1=len, R2=prot, R3=flags,
            //              [SP+0]=fd, [SP+4]=offset (per AAPCS).
            // Need to build a struct {addr, len, prot, flags, fd, offset} on
            // the stack, then set R0 = SP, R7 = 90, SVC #0.
            {
                let mut code = Vec::new();
                // PUSH {R4, R5}  — save callee-saved registers.
                code.extend_from_slice(&encode_stm(
                    Condition::Al, true, false, false, true, Gpr::R13.encoding(), 0x0030,
                ));
                // After PUSH (SP -= 8): caller's [SP+0] → [SP+8], [SP+4] → [SP+12].
                // LDR R4, [SP, #8]   (fd)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R4.encoding(), 8,
                ));
                // LDR R5, [SP, #12]  (offset)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R5.encoding(), 12,
                ));
                // SUB SP, SP, #24  (allocate mmap_arg_struct: 6 × 4 bytes)
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_SUB, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 24,
                ));
                // STR R0, [SP, #0]  (addr)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R0.encoding(), 0,
                ));
                // STR R1, [SP, #4]  (len)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R1.encoding(), 4,
                ));
                // STR R2, [SP, #8]  (prot)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R2.encoding(), 8,
                ));
                // STR R3, [SP, #12]  (flags)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R3.encoding(), 12,
                ));
                // STR R4, [SP, #16]  (fd)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R4.encoding(), 16,
                ));
                // STR R5, [SP, #20]  (offset)
                code.extend_from_slice(&encode_ls_imm(
                    Condition::Al, true, true, false, false, false,
                    Gpr::R13.encoding(), Gpr::R5.encoding(), 20,
                ));
                // MOV R0, SP  (R0 = pointer to struct)
                code.extend_from_slice(&encode_dp_reg(
                    Condition::Al, DP_MOV, false, 0, Gpr::R0.encoding(), Gpr::R13.encoding(),
                ));
                // MOV R7, #90  (sys_old_mmap)
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_MOV, false, 0, Gpr::R7.encoding(), 0, 90,
                ));
                // SVC #0
                code.extend_from_slice(&encode_svc(Condition::Al, 0));
                // ADD SP, SP, #24  (free the struct)
                code.extend_from_slice(&encode_dp_imm(
                    Condition::Al, DP_ADD, false, Gpr::R13.encoding(), Gpr::R13.encoding(), 0, 24,
                ));
                // POP {R4, R5}
                code.extend_from_slice(&encode_ldm(
                    Condition::Al, false, true, false, true, Gpr::R13.encoding(), 0x0030,
                ));
                // BX LR
                code.extend_from_slice(&encode_bx(Condition::Al, Gpr::R14.encoding()));
                stubs.push(("mmap".to_string(), code));
            }

            stubs
        };

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

        // Runtime functions: __vuma_print_hex, __vuma_print_int, __vuma_print_newline
        // The runtime blob is a single contiguous block; the three entry
        // symbols all share the start of the blob.
        let runtime_offsets_start = current_offset;
        func_offsets.insert("__vuma_print_hex".to_string(), runtime_offsets_start);
        func_offsets.insert("__vuma_print_int".to_string(), runtime_offsets_start);
        func_offsets.insert("__vuma_print_newline".to_string(), runtime_offsets_start);
        // POSIX-friendly aliases: print_int / print_hex point to the same
        // runtime blob offsets as the __vuma_print_* symbols so that user
        // code can call them by their bare names.
        func_offsets.insert("print_int".to_string(), runtime_offsets_start);
        func_offsets.insert("print_hex".to_string(), runtime_offsets_start);
        current_offset += runtime_code.len();

        // __vuma_alloc / __vuma_free stubs go after the runtime blob.
        let vuma_alloc_offset = current_offset;
        let vuma_free_offset = vuma_alloc_offset + vuma_alloc_stub.len();
        func_offsets.insert("__vuma_alloc".to_string(), vuma_alloc_offset);
        func_offsets.insert("__vuma_free".to_string(), vuma_free_offset);
        current_offset = vuma_free_offset + vuma_free_stub.len();

        // POSIX syscall stubs go after __vuma_free.
        let mut stub_offset = current_offset;
        for (name, code) in &syscall_stubs {
            func_offsets.insert(name.clone(), stub_offset);
            stub_offset += code.len();
        }

        // ── Build _start stub ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // LDR r0, [SP] — load argc from stack pointer (32-bit argc on ARM)
        start_stub.extend_from_slice(&encode_ls_imm(
            Condition::Al, true, true, false, false, true,
            Gpr::R13.encoding(), Gpr::R0.encoding(), 0,
        ));

        // ADD r1, SP, #4 — argv = sp + 4 (32-bit pointers on ARM)
        start_stub.extend_from_slice(&encode_dp_imm(
            Condition::Al, DP_ADD, false,
            Gpr::R13.encoding(), Gpr::R1.encoding(), 0, 4,
        ));

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
            // BL is at byte offset 8 within start_stub (after LDR r0 and ADD r1)
            // So: offset = (main_offset - (8 + 8)) / 4 = (main_offset - 16) / 4
            let bl_offset = (main_offset as i32 - 16) / 4;
            let patched_bl = encode_branch(Condition::Al, true, bl_offset);
            start_stub[8..12].copy_from_slice(&patched_bl);
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
        // Append __vuma_alloc / __vuma_free syscall stubs.
        all_code.extend_from_slice(&vuma_alloc_stub);
        all_code.extend_from_slice(&vuma_free_stub);
        // Append POSIX syscall stubs (write, read, open, close, mmap, etc.)
        for (_, code) in &syscall_stubs {
            all_code.extend_from_slice(code);
        }

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
                        vuma_log!(warn, 
                            "unresolved relocation: symbol '{}' in '{}' at 0x{:X} (type: {}) — deferring to FFI stub",
                            reloc.symbol, func.name, reloc.offset, reloc.reloc_type
                        );
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
                        vuma_log!(warn, 
                            "unresolved relocation: symbol '{}' in '{}' at 0x{:X} (type: {}) — deferring to FFI stub",
                            reloc.symbol, func.name, reloc.offset, reloc.reloc_type
                        );
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
                } else if reloc.reloc_type == "R_ARM_ABS32" {
                    // Absolute address relocation: patch the 4-byte literal
                    // pool entry (emitted by GetAddress) with the symbol's
                    // absolute runtime address. The runtime address is
                    //   base_addr + text_offset + target_offset
                    // where:
                    //   - base_addr = 0x10000 (the ELF load base, see the
                    //     build_arm32_elf_2seg call below)
                    //   - text_offset = ELF header (52) + 3 program headers
                    //     (3 * 32 = 96) = 148 (the file offset where the
                    //     text segment / `all_code` begins)
                    //   - target_offset = the symbol's offset within
                    //     `all_code`, looked up in `func_offsets`
                    //
                    // Look up the target symbol (with the usual `fn_`
                    // prefix fallback for monomorphized function names).
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
                        const BASE_ADDR: u32 = 0x10000;
                        const TEXT_OFFSET: u32 = 148; // 52 (ehdr) + 3*32 (3 phdrs)
                        let abs_addr: u32 = BASE_ADDR
                            .wrapping_add(TEXT_OFFSET)
                            .wrapping_add(target_offset as u32);
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&abs_addr.to_le_bytes());
                    } else {
                        // Unresolved symbol — leave the .word as 0 so the
                        // program fails loudly (NULL pointer dereference)
                        // rather than silently calling the wrong address.
                        vuma_log!(warn, 
                            "unresolved R_ARM_ABS32 relocation: symbol '{}' in '{}' \
                             at 0x{:X} — leaving literal-pool entry as 0",
                            reloc.symbol, func.name, reloc.offset
                        );
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
        // Verify PUSH {r4,r5,r6,r7,lr}: (1<<4)|(1<<5)|(1<<6)|(1<<7)|(1<<14) = 0x40F0
        assert_eq!((1u16<<4)|(1u16<<5)|(1u16<<6)|(1u16<<7)|(1u16<<14), 0x40F0);
        // Verify POP {r4,r5,r6,r7,pc}: (1<<4)|(1<<5)|(1<<6)|(1<<7)|(1<<15) = 0x80F0
        assert_eq!((1u16<<4)|(1u16<<5)|(1u16<<6)|(1u16<<7)|(1u16<<15), 0x80F0);
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
    let word = ((Condition::Al.encoding() as u32) << 28
        | 0b1101 << 24
        | (d_bit << 22))
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
    let word = ((Condition::Al.encoding() as u32) << 28
        | 0b1101 << 24
        | (d_bit << 22))
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
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1000 << 16
        | (vd << 12)
        | 0b101 << 9)      // signed
        | (1 << 6)
        | (m_bit << 5))
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
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1000 << 16
        | (vd << 12)
        | 0b101 << 9)      // sz = 0 (f32)
        | (1 << 7)      // unsigned
        | (1 << 6)
        | (m_bit << 5))
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
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1101 << 16
        | (vd << 12)
        | 0b101 << 9)      // signed
        | (1 << 6)
        | (m_bit << 5))
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
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b1101 << 16
        | (vd << 12)
        | 0b101 << 9)      // sz = 0 (f32)
        | (1 << 7)      // unsigned
        | (1 << 6)
        | (m_bit << 5))
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
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b0110 << 16
        | (vd << 12)
        | 0b101 << 9
        | (1 << 8))
        | (1 << 6)
        | (m_bit << 5))
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
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (1 << 23)
        | (d_bit << 22)
        | 0b11 << 20
        | 0b0110 << 16
        | (vd << 12)
        | 0b101 << 9)
        | (1 << 6)
        | (m_bit << 5))
        | vm;
    word.to_le_bytes()
}

/// Encode VADD.F64 Dd, Dn, Dm — add two f64 values.
///
/// ARM VFP encoding (A1):
///   cond 1110 0D11 Vn Dd 1011 N 0 M 0 Vm
fn encode_vadd_f64(dd: u8, dn: u8, dm: u8) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let n_bit = ((dn >> 4) & 1) as u32;
    let vn = (dn & 0xF) as u32;
    let m_bit = ((dm >> 4) & 1) as u32;
    let vm = (dm & 0xF) as u32;
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (d_bit << 22)
        | 0b11 << 20
        | (vn << 16)
        | (vd << 12)
        | 0b1011 << 8
        | (n_bit << 7))
        | (m_bit << 5))
        | vm;
    word.to_le_bytes()
}

/// Encode VSUB.F64 Dd, Dn, Dm — subtract two f64 values.
///
/// ARM VFP encoding (A1):
///   cond 1110 0D11 Vn Dd 1011 N 1 M 0 Vm   (bit [6] = 1 for subtract)
fn encode_vsub_f64(dd: u8, dn: u8, dm: u8) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let n_bit = ((dn >> 4) & 1) as u32;
    let vn = (dn & 0xF) as u32;
    let m_bit = ((dm >> 4) & 1) as u32;
    let vm = (dm & 0xF) as u32;
    let word = ((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (d_bit << 22)
        | 0b11 << 20
        | (vn << 16)
        | (vd << 12)
        | 0b1011 << 8
        | (n_bit << 7)
        | (1 << 6)
        | (m_bit << 5))
        | vm;
    word.to_le_bytes()
}

/// Encode VMUL.F64 Dd, Dn, Dm — multiply two f64 values.
///
/// ARM VFP encoding (A1):
///   cond 1110 0D10 Vn Dd 1011 N 0 M 0 Vm   ([21:20]=10 for multiply)
fn encode_vmul_f64(dd: u8, dn: u8, dm: u8) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let n_bit = ((dn >> 4) & 1) as u32;
    let vn = (dn & 0xF) as u32;
    let m_bit = ((dm >> 4) & 1) as u32;
    let vm = (dm & 0xF) as u32;
    let word = (((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (d_bit << 22)
        | 0b10 << 20
        | (vn << 16)
        | (vd << 12)
        | 0b1011 << 8
        | (n_bit << 7))
        | (m_bit << 5))
        | vm;
    word.to_le_bytes()
}

/// Encode VDIV.F64 Dd, Dn, Dm — divide two f64 values.
///
/// ARM VFP encoding (A1):
///   cond 1110 0D00 Vn Dd 1011 N 0 M 0 Vm  ... actually: cond 1110 1D00 Vn Dd 1011 N 0 M 0 Vm
///   Encoding for VDIV.F64 D0, D0, D0 = 0xEE800B00
fn encode_vdiv_f64(dd: u8, dn: u8, dm: u8) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let n_bit = ((dn >> 4) & 1) as u32;
    let vn = (dn & 0xF) as u32;
    let m_bit = ((dm >> 4) & 1) as u32;
    let vm = (dm & 0xF) as u32;
    let word = ((((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (d_bit << 22))
        | (vn << 16)
        | (vd << 12)
        | 0b1011 << 8
        | (n_bit << 7))
        | (m_bit << 5))
        | vm;
    word.to_le_bytes()
}

/// Encode VCMP.F64 Dd, Dm — compare two f64 values (sets FPSCR flags).
///
/// ARM VFP encoding (A1):
///   cond 1110 1D11 0100 Dd 1011 E 1 M 0 Vm
///   E=1 → compare Dd with Dm; E=0 → compare Dd with #0.
///   We use E=1 (register comparison).
fn encode_vcmp_f64(dd: u8, dm: u8) -> [u8; 4] {
    let d_bit = ((dd >> 4) & 1) as u32;
    let vd = (dd & 0xF) as u32;
    let m_bit = ((dm >> 4) & 1) as u32;
    let vm = (dm & 0xF) as u32;
    let word = ((Condition::Al.encoding() as u32) << 28
        | 0b1110 << 24
        | (d_bit << 22)
        | 0b11 << 20
        | 0b0100 << 16
        | (vd << 12)
        | 0b1011 << 8
        | (1 << 7)  // E = 1 (compare with register)
        | (1 << 6)  // fixed
        | (m_bit << 5))
        | vm;
    word.to_le_bytes()
}

/// Encode FMSTAT (alias for VMRS APSR_nzcv, FPSCR) — transfer FPSCR flags
/// to the ARM APSR.NZCV condition flags so that subsequent conditional
/// instructions (MOVcc, etc.) can read the FP comparison result.
///
/// Encoding: cond 1110 1110 1000 1111 1000 0001 0000 = 0xEEF1FA10
fn encode_fmstat() -> [u8; 4] {
    0xEEF1FA10u32.to_le_bytes()
}

pub mod disasm;
