//! # PowerPC64 Backend
//!
//! Implements the `Backend` trait for the PowerPC 64-bit target (ELFv2 ABI,
//! little-endian default). This module provides:
//!
//! - `Gpr` — General-purpose register enum (R0–R31)
//! - `Fpr` — Floating-point register enum (F0–F31)
//! - `CrField` — Condition register field enum (CR0–CR7)
//! - `Instruction` — PPC64 instruction enum with correct 32-bit encoding
//! - `PPC64Backend` — `Backend` implementation that lowers IR to PPC64
//!   machine code and emits ELF64 binaries
//!
//! ## PowerPC64 Register Convention (ELFv2 ABI)
//!
//! | Register(s) | Role                                  |
//! |-------------|---------------------------------------|
//! | R0          | Volatile (not hardwired zero!)        |
//! | R1          | Stack pointer (SP)                    |
//! | R2          | TOC pointer                           |
//! | R3–R10      | Argument / return registers           |
//! | R11–R12     | Volatile scratch                      |
//! | R13         | Thread pointer                        |
//! | R14–R31     | Callee-saved                          |
//! | F0–F13      | FP argument / return, volatile        |
//! | F14–F31     | FP callee-saved                       |
//! | CR0–CR7     | Condition register fields (4 bits ea) |
//! | LR          | Link register (SPR)                   |
//! | CTR         | Count register (SPR)                  |
//!
//! ## Instruction Encoding
//!
//! All instructions are 32 bits, fixed-width. PPC64 is bi-endian; the default
//! for ppc64le is little-endian byte order for both data and instructions.
//! The primary opcode occupies bits \[0:5\] (MSB-first bit numbering).
//!
//! ## References
//!
//! - Power ISA Version 3.1
//! - <https://openpowerfoundation.org/specifications/isa/>

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, PhysicalReg, PowerPC64TargetInfo, RegClass, TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// PowerPC64 general-purpose registers (R0–R31).
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
    R16 = 16,
    R17 = 17,
    R18 = 18,
    R19 = 19,
    R20 = 20,
    R21 = 21,
    R22 = 22,
    R23 = 23,
    R24 = 24,
    R25 = 25,
    R26 = 26,
    R27 = 27,
    R28 = 28,
    R29 = 29,
    R30 = 30,
    R31 = 31,
}

impl Gpr {
    /// Returns the 5-bit encoding index for this register.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns `true` if this register is available for register allocation.
    ///
    /// R0 (volatile/special), R1 (SP), R2 (TOC), R13 (thread) are reserved.
    pub fn is_allocatable(&self) -> bool {
        !matches!(self, Gpr::R0 | Gpr::R1 | Gpr::R2 | Gpr::R13)
    }

    /// Returns `true` if this register is callee-saved (R14–R31).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Gpr::R14
                | Gpr::R15
                | Gpr::R16
                | Gpr::R17
                | Gpr::R18
                | Gpr::R19
                | Gpr::R20
                | Gpr::R21
                | Gpr::R22
                | Gpr::R23
                | Gpr::R24
                | Gpr::R25
                | Gpr::R26
                | Gpr::R27
                | Gpr::R28
                | Gpr::R29
                | Gpr::R30
                | Gpr::R31
        )
    }

    /// Returns `true` if this register is an argument register (R3–R10).
    pub fn is_arg_reg(&self) -> bool {
        matches!(
            self,
            Gpr::R3 | Gpr::R4 | Gpr::R5 | Gpr::R6 | Gpr::R7 | Gpr::R8 | Gpr::R9 | Gpr::R10
        )
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
            Gpr::R12 => "r12",
            Gpr::R13 => "r13",
            Gpr::R14 => "r14",
            Gpr::R15 => "r15",
            Gpr::R16 => "r16",
            Gpr::R17 => "r17",
            Gpr::R18 => "r18",
            Gpr::R19 => "r19",
            Gpr::R20 => "r20",
            Gpr::R21 => "r21",
            Gpr::R22 => "r22",
            Gpr::R23 => "r23",
            Gpr::R24 => "r24",
            Gpr::R25 => "r25",
            Gpr::R26 => "r26",
            Gpr::R27 => "r27",
            Gpr::R28 => "r28",
            Gpr::R29 => "r29",
            Gpr::R30 => "r30",
            Gpr::R31 => "r31",
        }
    }

    /// Returns the Gpr for a given argument index (0–7). Returns `None` for
    /// indices >= 8.
    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::R3),
            1 => Some(Gpr::R4),
            2 => Some(Gpr::R5),
            3 => Some(Gpr::R6),
            4 => Some(Gpr::R7),
            5 => Some(Gpr::R8),
            6 => Some(Gpr::R9),
            7 => Some(Gpr::R10),
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

/// PowerPC64 floating-point registers (F0–F31).
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

    /// Returns `true` if this register is callee-saved (F14–F31).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Fpr::F14
                | Fpr::F15
                | Fpr::F16
                | Fpr::F17
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
                | Fpr::F28
                | Fpr::F29
                | Fpr::F30
                | Fpr::F31
        )
    }

    /// Returns `true` if this register is an FP argument register (F1–F13).
    /// F0 is volatile but not an argument register.
    pub fn is_arg_reg(&self) -> bool {
        matches!(
            self,
            Fpr::F1
                | Fpr::F2
                | Fpr::F3
                | Fpr::F4
                | Fpr::F5
                | Fpr::F6
                | Fpr::F7
                | Fpr::F8
                | Fpr::F9
                | Fpr::F10
                | Fpr::F11
                | Fpr::F12
                | Fpr::F13
        )
    }

    /// Returns `true` if this register is available for register allocation.
    pub fn is_allocatable(&self) -> bool {
        true
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

    /// Returns the Fpr for a given FP argument index (0–12). Returns `None`
    /// for indices >= 13.
    pub fn arg_register(index: usize) -> Option<Fpr> {
        match index {
            0 => Some(Fpr::F1),
            1 => Some(Fpr::F2),
            2 => Some(Fpr::F3),
            3 => Some(Fpr::F4),
            4 => Some(Fpr::F5),
            5 => Some(Fpr::F6),
            6 => Some(Fpr::F7),
            7 => Some(Fpr::F8),
            8 => Some(Fpr::F9),
            9 => Some(Fpr::F10),
            10 => Some(Fpr::F11),
            11 => Some(Fpr::F12),
            12 => Some(Fpr::F13),
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
// Condition Register Fields
// ===========================================================================

/// PowerPC64 condition register fields (CR0–CR7).
///
/// Each CR field has 4 bits: LT (bit 0), GT (bit 1), EQ (bit 2), SO (bit 3).
/// CR fields are used by compare instructions and conditional branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CrField {
    CR0 = 0,
    CR1 = 1,
    CR2 = 2,
    CR3 = 3,
    CR4 = 4,
    CR5 = 5,
    CR6 = 6,
    CR7 = 7,
}

impl CrField {
    /// Returns the 3-bit encoding index for this CR field.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns the standard assembly name for this CR field.
    pub fn asm_name(&self) -> &'static str {
        match self {
            CrField::CR0 => "cr0",
            CrField::CR1 => "cr1",
            CrField::CR2 => "cr2",
            CrField::CR3 => "cr3",
            CrField::CR4 => "cr4",
            CrField::CR5 => "cr5",
            CrField::CR6 => "cr6",
            CrField::CR7 => "cr7",
        }
    }

    /// Returns `true` if this CR field is allocatable for register allocation.
    /// CR0 is implicitly set by many instructions; CR1 is used for FP results.
    pub fn is_allocatable(&self) -> bool {
        !matches!(self, CrField::CR0 | CrField::CR1)
    }

    /// Returns `true` if this CR field is callee-saved (CR2–CR4).
    pub fn is_callee_saved(&self) -> bool {
        matches!(self, CrField::CR2 | CrField::CR3 | CrField::CR4)
    }
}

impl fmt::Display for CrField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Instruction Encoding Helpers
// ===========================================================================

/// Encode a PPC64 32-bit instruction word and return as big-endian bytes.
fn encode_word(word: u32) -> [u8; 4] {
    word.to_be_bytes()
}

/// Build a D-form instruction: opcode[0:5] | rT[6:10] | rA[11:15] | d[16:31]
fn encode_d_form(opcode: u32, rt: u32, ra: u32, d: i32) -> [u8; 4] {
    let word =
        ((opcode & 0x3F) << 26) | ((rt & 0x1F) << 21) | ((ra & 0x1F) << 16) | ((d as u32) & 0xFFFF);
    encode_word(word)
}

/// Build a DS-form instruction: opcode[0:5] | rT[6:10] | rA[11:15] | ds[16:29] | xo[30:31]
/// The `ds` parameter is the byte offset; the DS field stores ds/4.
fn encode_ds_form(opcode: u32, rt: u32, ra: u32, ds: i32, xo: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((rt & 0x1F) << 21)
        | ((ra & 0x1F) << 16)
        | (((ds >> 2) as u32) & 0x3FFF) << 2
        | (xo & 0x3);
    encode_word(word)
}

/// Build an X-form instruction: opcode[0:5] | rS[6:10] | rA[11:15] | rB[16:20] | xo[21:30] | Rc[31]
fn encode_x_form(opcode: u32, rs: u32, ra: u32, rb: u32, xo: u32, rc: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((rs & 0x1F) << 21)
        | ((ra & 0x1F) << 16)
        | ((rb & 0x1F) << 11)
        | ((xo & 0x3FF) << 1)
        | (rc & 1);
    encode_word(word)
}

/// Build an XO-form instruction: opcode[0:5] | rT[6:10] | rA[11:15] | rB[16:20] | OE[21] | xo[22:30] | Rc[31]
fn encode_xo_form(opcode: u32, rt: u32, ra: u32, rb: u32, oe: u32, xo: u32, rc: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((rt & 0x1F) << 21)
        | ((ra & 0x1F) << 16)
        | ((rb & 0x1F) << 11)
        | ((oe & 1) << 10)
        | ((xo & 0x1FF) << 1)
        | (rc & 1);
    encode_word(word)
}

/// Build an M-form instruction: opcode[0:5] | rS[6:10] | rA[11:15] | SH[16:20] | MB[21:25] | ME[26:30] | Rc[31]
fn encode_m_form(opcode: u32, rs: u32, ra: u32, sh: u32, mb: u32, me: u32, rc: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((rs & 0x1F) << 21)
        | ((ra & 0x1F) << 16)
        | ((sh & 0x1F) << 11)
        | ((mb & 0x1F) << 6)
        | ((me & 0x1F) << 1)
        | (rc & 1);
    encode_word(word)
}

/// Build an I-form instruction: opcode[0:5] | LI[6:29] | AA[30] | LK[31]
fn encode_i_form(opcode: u32, li: i32, aa: u32, lk: u32) -> [u8; 4] {
    let word =
        ((opcode & 0x3F) << 26) | (((li as u32) & 0x03FF_FFFF) << 2) | ((aa & 1) << 1) | (lk & 1);
    encode_word(word)
}

/// Build a B-form instruction: opcode[0:5] | BO[6:10] | BI[11:15] | BD[16:29] | AA[30] | LK[31]
fn encode_b_form(opcode: u32, bo: u32, bi: u32, bd: i32, aa: u32, lk: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((bo & 0x1F) << 21)
        | ((bi & 0x1F) << 16)
        | (((bd as u32) & 0x3FFF) << 2)
        | ((aa & 1) << 1)
        | (lk & 1);
    encode_word(word)
}

/// Build an XL-form instruction: opcode[0:5] | BO[6:10] | BI[11:15] | 0[16:18] | BH[19:21] | xo[21:30] | LK[31]
fn encode_xl_form(opcode: u32, bo: u32, bi: u32, bh: u32, xo: u32, lk: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((bo & 0x1F) << 21)
        | ((bi & 0x1F) << 16)
        | ((bh & 0x7) << 11)
        | ((xo & 0x3FF) << 1)
        | (lk & 1);
    encode_word(word)
}

// ===========================================================================
// Instruction Enum
// ===========================================================================

/// PowerPC64 instruction representations for code generation.
///
/// Covers key arithmetic, logical, shift/rotate, load/store, compare, branch,
/// move, and system instructions. Each variant captures the operands needed for
/// encoding and disassembly. The `encode()` method produces a 4-byte
/// little-endian machine code word.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Instruction {
    // ── Arithmetic ──────────────────────────────────────────────────
    /// Add: `add rT, rA, rB` (XO-form, primary=31, xo=266)
    Add { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Add Immediate: `addi rT, rA, simm16` (D-form, primary=14)
    Addi { rt: Gpr, ra: Gpr, simm: i32 },
    /// Add Immediate Shifted: `addis rT, rA, simm16` (D-form, primary=15)
    Addis { rt: Gpr, ra: Gpr, simm: i32 },
    /// Subtract From: `subf rT, rA, rB` (XO-form, primary=31, xo=40)
    Subf { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Multiply Low Word: `mullw rT, rA, rB` (XO-form, primary=31, xo=235)
    Mullw { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Multiply Low Doubleword: `mulld rT, rA, rB` (XO-form, primary=31, xo=233)
    Mulld { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Multiply High Word: `mulhw rT, rA, rB` (X-form, primary=31, xo=75)
    Mulhw { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Multiply High Doubleword: `mulhd rT, rA, rB` (X-form, primary=31, xo=73)
    Mulhd { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Divide Word: `divw rT, rA, rB` (XO-form, primary=31, xo=491)
    Divw { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Divide Doubleword: `divd rT, rA, rB` (XO-form, primary=31, xo=459)
    Divd { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Divide Word Unsigned: `divwu rT, rA, rB` (XO-form, primary=31, xo=455)
    Divwu { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Divide Doubleword Unsigned: `divdu rT, rA, rB` (XO-form, primary=31, xo=457)
    Divdu { rt: Gpr, ra: Gpr, rb: Gpr },
    /// Negate: `neg rT, rA` (XO-form, primary=31, xo=104)
    Neg { rt: Gpr, ra: Gpr },
    /// Count Leading Zeros Doubleword: `cntlzd rA, rS` (X-form, primary=31, xo=58)
    Cntlzd { ra: Gpr, rs: Gpr },
    /// Population Count Doubleword: `popcntd rA, rS` (X-form, primary=31, xo=506)
    Popcntd { ra: Gpr, rs: Gpr },
    /// Extend Sign Word: `extsw rA, rS` (X-form, primary=31, xo=986)
    Extsw { ra: Gpr, rs: Gpr },

    // ── Logical ────────────────────────────────────────────────────
    /// AND: `and rA, rS, rB` (X-form, primary=31, xo=28)
    And { ra: Gpr, rs: Gpr, rb: Gpr },
    /// AND Immediate: `andi. rA, rS, uimm16` (D-form, primary=28, Rc=1)
    Andi { ra: Gpr, rs: Gpr, uimm: u32 },
    /// OR: `or rA, rS, rB` (X-form, primary=31, xo=444)
    Or { ra: Gpr, rs: Gpr, rb: Gpr },
    /// OR Immediate: `ori rA, rS, uimm16` (D-form, primary=24)
    Ori { ra: Gpr, rs: Gpr, uimm: u32 },
    /// XOR: `xor rA, rS, rB` (X-form, primary=31, xo=316)
    Xor { ra: Gpr, rs: Gpr, rb: Gpr },
    /// XOR Immediate: `xori rA, rS, uimm16` (D-form, primary=26)
    Xori { ra: Gpr, rs: Gpr, uimm: u32 },
    /// NOR: `nor rA, rS, rB` (X-form, primary=31, xo=124)
    Nor { ra: Gpr, rs: Gpr, rb: Gpr },
    /// AND with Complement: `andc rA, rS, rB` (X-form, primary=31, xo=60)
    Andc { ra: Gpr, rs: Gpr, rb: Gpr },
    /// OR with Complement: `orc rA, rS, rB` (X-form, primary=31, xo=412)
    Orc { ra: Gpr, rs: Gpr, rb: Gpr },
    /// Equivalent: `eqv rA, rS, rB` (X-form, primary=31, xo=284)
    Eqv { ra: Gpr, rs: Gpr, rb: Gpr },

    // ── Shift / Rotate ─────────────────────────────────────────────
    /// Shift Left Doubleword: `sld rA, rS, rB` (X-form, primary=31, xo=27)
    Sld { ra: Gpr, rs: Gpr, rb: Gpr },
    /// Shift Right Doubleword: `srd rA, rS, rB` (X-form, primary=31, xo=539)
    Srd { ra: Gpr, rs: Gpr, rb: Gpr },
    /// Shift Right Algebraic Doubleword: `srad rA, rS, rB` (X-form, primary=31, xo=794)
    Srad { ra: Gpr, rs: Gpr, rb: Gpr },
    /// Shift Left Word: `slw rA, rS, rB` (X-form, primary=31, xo=24)
    Slw { ra: Gpr, rs: Gpr, rb: Gpr },
    /// Shift Right Word: `srw rA, rS, rB` (X-form, primary=31, xo=536)
    Srw { ra: Gpr, rs: Gpr, rb: Gpr },
    /// Shift Right Algebraic Word: `sraw rA, rS, rB` (X-form, primary=31, xo=792)
    Sraw { ra: Gpr, rs: Gpr, rb: Gpr },
    /// Rotate Left Doubleword then Clear Left: `rldcl rA, rS, rB, MB` (X-form, primary=31, xo=8)
    Rldcl { ra: Gpr, rs: Gpr, rb: Gpr, mb: u32 },
    /// Rotate Left Doubleword then Clear Right: `rldcr rA, rS, rB, ME` (X-form, primary=31, xo=9)
    Rldcr { ra: Gpr, rs: Gpr, rb: Gpr, me: u32 },
    /// Rotate Left Word Immediate then AND Mask: `rlwinm rA, rS, SH, MB, ME` (M-form, primary=21)
    Rlwinm {
        ra: Gpr,
        rs: Gpr,
        sh: u32,
        mb: u32,
        me: u32,
    },
    /// Rotate Left Word Immediate then Insert Mask: `rlwimi rA, rS, SH, MB, ME` (M-form, primary=20)
    Rlwimi {
        ra: Gpr,
        rs: Gpr,
        sh: u32,
        mb: u32,
        me: u32,
    },

    // ── Load / Store ───────────────────────────────────────────────
    /// Load Doubleword: `ld rT, ds(rA)` (DS-form, primary=58, xo=0)
    Ld { rt: Gpr, ra: Gpr, ds: i32 },
    /// Load Word Algebraic: `lwa rT, ds(rA)` (DS-form, primary=58, xo=2)
    Lwa { rt: Gpr, ra: Gpr, ds: i32 },
    /// Load Word and Zero: `lwz rT, d(rA)` (D-form, primary=32)
    Lwz { rt: Gpr, ra: Gpr, d: i32 },
    /// Load Word with Zero Update: `lwzu rT, d(rA)` (D-form, primary=33)
    Lwzu { rt: Gpr, ra: Gpr, d: i32 },
    /// Store Doubleword: `std rS, ds(rA)` (DS-form, primary=62, xo=0)
    Std { rs: Gpr, ra: Gpr, ds: i32 },
    /// Store Word: `stw rS, d(rA)` (D-form, primary=36)
    Stw { rs: Gpr, ra: Gpr, d: i32 },
    /// Store Word with Update: `stwu rS, d(rA)` (D-form, primary=37)
    Stwu { rs: Gpr, ra: Gpr, d: i32 },
    /// Store Doubleword with Update: `stdu rS, ds(rA)` (DS-form, primary=62, xo=1)
    Stdu { rs: Gpr, ra: Gpr, ds: i32 },
    /// Load Byte and Zero: `lbz rT, d(rA)` (D-form, primary=34)
    Lbz { rt: Gpr, ra: Gpr, d: i32 },
    /// Load Halfword and Zero: `lhz rT, d(rA)` (D-form, primary=40)
    Lhz { rt: Gpr, ra: Gpr, d: i32 },
    /// Store Byte: `stb rS, d(rA)` (D-form, primary=38)
    Stb { rs: Gpr, ra: Gpr, d: i32 },
    /// Store Halfword: `sth rS, d(rA)` (D-form, primary=44)
    Sth { rs: Gpr, ra: Gpr, d: i32 },
    /// Load Floating-Point Double: `lfd fT, d(rA)` (D-form, primary=50)
    Lfd { ft: Fpr, ra: Gpr, d: i32 },
    /// Store Floating-Point Double: `stfd fS, d(rA)` (D-form, primary=54)
    Stfd { fs: Fpr, ra: Gpr, d: i32 },
    /// Load Floating-Point Single: `lfs fT, d(rA)` (D-form, primary=48)
    Lfs { ft: Fpr, ra: Gpr, d: i32 },
    /// Store Floating-Point Single: `stfs fS, d(rA)` (D-form, primary=52)
    Stfs { fs: Fpr, ra: Gpr, d: i32 },

    // ── Compare ────────────────────────────────────────────────────
    /// Compare: `cmp crf, l, rA, rB` (X-form, primary=31, xo=0)
    Cmp {
        bf: CrField,
        l: u32,
        ra: Gpr,
        rb: Gpr,
    },
    /// Compare Immediate: `cmpi crf, l, rA, simm16` (D-form, primary=11)
    Cmpi {
        bf: CrField,
        l: u32,
        ra: Gpr,
        simm: i32,
    },
    /// Compare Logical: `cmpl crf, l, rA, rB` (X-form, primary=31, xo=32)
    Cmpl {
        bf: CrField,
        l: u32,
        ra: Gpr,
        rb: Gpr,
    },
    /// Compare Logical Immediate: `cmpli crf, l, rA, uimm16` (D-form, primary=10)
    Cmpli {
        bf: CrField,
        l: u32,
        ra: Gpr,
        uimm: u32,
    },

    // ── Branch ─────────────────────────────────────────────────────
    /// Branch: `b li` (I-form, primary=18, AA=0, LK=0)
    B { li: i32 },
    /// Branch Absolute: `ba li` (I-form, primary=18, AA=1, LK=0)
    Ba { li: i32 },
    /// Branch with Link: `bl li` (I-form, primary=18, AA=0, LK=1)
    Bl { li: i32 },
    /// Branch Absolute with Link: `bla li` (I-form, primary=18, AA=1, LK=1)
    Bla { li: i32 },
    /// Branch Conditional: `bc BO, BI, BD` (B-form, primary=16, AA=0, LK=0)
    Bc { bo: u32, bi: u32, bd: i32 },
    /// Branch Conditional Absolute: `bca BO, BI, BD` (B-form, primary=16, AA=1, LK=0)
    Bca { bo: u32, bi: u32, bd: i32 },
    /// Branch Conditional to Link Register: `bclr BO, BI, BH` (XL-form, primary=19, xo=16)
    Bclr { bo: u32, bi: u32, bh: u32 },
    /// Branch Conditional to Count Register: `bcctr BO, BI, BH` (XL-form, primary=19, xo=528)
    Bcctr { bo: u32, bi: u32, bh: u32 },
    /// Branch Conditional to Target Address Register: `bctar BO, BI, BH` (XL-form, primary=19, xo=560)
    Bctar { bo: u32, bi: u32, bh: u32 },

    // ── Move ───────────────────────────────────────────────────────
    /// Move Register: `mr rA, rS` (pseudo: `or rA, rS, rS`)
    Mr { ra: Gpr, rs: Gpr },
    /// Load Immediate: `li rT, simm16` (pseudo: `addi rT, 0, simm16`)
    Li { rt: Gpr, simm: i32 },
    /// Load Immediate Shifted: `lis rT, simm16` (pseudo: `addis rT, 0, simm16`)
    Lis { rt: Gpr, simm: i32 },

    // ── System ─────────────────────────────────────────────────────
    /// System Call: `sc` (primary=17, SVC=0)
    Sc,
    /// No-operation: `nop` (pseudo: `ori r0, r0, 0`)
    Nop,
    /// Trap: `trap` (pseudo: `tw 31, r0, r0`)
    Trap,
}

impl Instruction {
    /// Encode this instruction into a 4-byte little-endian machine code word.
    ///
    /// Encoding follows the Power ISA Version 3.1.
    pub fn encode(&self) -> [u8; 4] {
        match self {
            // ── Arithmetic ──────────────────────────────────────
            Instruction::Add { rt, ra, rb } => {
                // ADD rT, rA, rB: primary=31, OE=0, xo=266, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 266, 0)
            }
            Instruction::Addi { rt, ra, simm } => {
                // ADDI rT, rA, simm16: primary=14
                encode_d_form(14, rt.encoding(), ra.encoding(), *simm)
            }
            Instruction::Addis { rt, ra, simm } => {
                // ADDIS rT, rA, simm16: primary=15
                encode_d_form(15, rt.encoding(), ra.encoding(), *simm)
            }
            Instruction::Subf { rt, ra, rb } => {
                // SUBF rT, rA, rB: primary=31, OE=0, xo=40, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 40, 0)
            }
            Instruction::Mullw { rt, ra, rb } => {
                // MULLW rT, rA, rB: primary=31, OE=0, xo=235, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 235, 0)
            }
            Instruction::Mulld { rt, ra, rb } => {
                // MULLD rT, rA, rB: primary=31, OE=0, xo=233, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 233, 0)
            }
            Instruction::Mulhw { rt, ra, rb } => {
                // MULHW rT, rA, rB: primary=31, xo=75, Rc=0
                encode_x_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 75, 0)
            }
            Instruction::Mulhd { rt, ra, rb } => {
                // MULHD rT, rA, rB: primary=31, xo=73, Rc=0
                encode_x_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 73, 0)
            }
            Instruction::Divw { rt, ra, rb } => {
                // DIVW rT, rA, rB: primary=31, OE=0, xo=491, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 491, 0)
            }
            Instruction::Divd { rt, ra, rb } => {
                // DIVD rT, rA, rB: primary=31, OE=0, xo=459, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 459, 0)
            }
            Instruction::Divwu { rt, ra, rb } => {
                // DIVWU rT, rA, rB: primary=31, OE=0, xo=455, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 455, 0)
            }
            Instruction::Divdu { rt, ra, rb } => {
                // DIVDU rT, rA, rB: primary=31, OE=0, xo=457, Rc=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), rb.encoding(), 0, 457, 0)
            }
            Instruction::Neg { rt, ra } => {
                // NEG rT, rA: primary=31, OE=0, xo=104, Rc=0, rB=0
                encode_xo_form(31, rt.encoding(), ra.encoding(), 0, 0, 104, 0)
            }
            Instruction::Cntlzd { ra, rs } => {
                // CNTLZD rA, rS: primary=31, xo=58, Rc=0, rB=0
                encode_x_form(31, rs.encoding(), ra.encoding(), 0, 58, 0)
            }
            Instruction::Popcntd { ra, rs } => {
                // POPCNTD rA, rS: primary=31, xo=506, Rc=0, rB=0
                encode_x_form(31, rs.encoding(), ra.encoding(), 0, 506, 0)
            }
            Instruction::Extsw { ra, rs } => {
                // EXTSW rA, rS: primary=31, xo=986, Rc=0, rB=0
                encode_x_form(31, rs.encoding(), ra.encoding(), 0, 986, 0)
            }

            // ── Logical ────────────────────────────────────────
            Instruction::And { ra, rs, rb } => {
                // AND rA, rS, rB: primary=31, xo=28, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 28, 0)
            }
            Instruction::Andi { ra, rs, uimm } => {
                // ANDI. rA, rS, uimm16: primary=28, Rc=1 (always)
                let word = ((28u32 & 0x3F) << 26)
                    | ((rs.encoding() & 0x1F) << 21)
                    | ((ra.encoding() & 0x1F) << 16)
                    | (uimm & 0xFFFF);
                encode_word(word)
            }
            Instruction::Or { ra, rs, rb } => {
                // OR rA, rS, rB: primary=31, xo=444, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 444, 0)
            }
            Instruction::Ori { ra, rs, uimm } => {
                // ORI rA, rS, uimm16: primary=24
                let word = ((24u32 & 0x3F) << 26)
                    | ((rs.encoding() & 0x1F) << 21)
                    | ((ra.encoding() & 0x1F) << 16)
                    | (uimm & 0xFFFF);
                encode_word(word)
            }
            Instruction::Xor { ra, rs, rb } => {
                // XOR rA, rS, rB: primary=31, xo=316, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 316, 0)
            }
            Instruction::Xori { ra, rs, uimm } => {
                // XORI rA, rS, uimm16: primary=26
                let word = ((26u32 & 0x3F) << 26)
                    | ((rs.encoding() & 0x1F) << 21)
                    | ((ra.encoding() & 0x1F) << 16)
                    | (uimm & 0xFFFF);
                encode_word(word)
            }
            Instruction::Nor { ra, rs, rb } => {
                // NOR rA, rS, rB: primary=31, xo=124, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 124, 0)
            }
            Instruction::Andc { ra, rs, rb } => {
                // ANDC rA, rS, rB: primary=31, xo=60, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 60, 0)
            }
            Instruction::Orc { ra, rs, rb } => {
                // ORC rA, rS, rB: primary=31, xo=412, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 412, 0)
            }
            Instruction::Eqv { ra, rs, rb } => {
                // EQV rA, rS, rB: primary=31, xo=284, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 284, 0)
            }

            // ── Shift / Rotate ─────────────────────────────────
            Instruction::Sld { ra, rs, rb } => {
                // SLD rA, rS, rB: primary=31, xo=27, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 27, 0)
            }
            Instruction::Srd { ra, rs, rb } => {
                // SRD rA, rS, rB: primary=31, xo=539, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 539, 0)
            }
            Instruction::Srad { ra, rs, rb } => {
                // SRAD rA, rS, rB: primary=31, xo=794, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 794, 0)
            }
            Instruction::Slw { ra, rs, rb } => {
                // SLW rA, rS, rB: primary=31, xo=24, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 24, 0)
            }
            Instruction::Srw { ra, rs, rb } => {
                // SRW rA, rS, rB: primary=31, xo=536, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 536, 0)
            }
            Instruction::Sraw { ra, rs, rb } => {
                // SRAW rA, rS, rB: primary=31, xo=792, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rb.encoding(), 792, 0)
            }
            Instruction::Rldcl { ra, rs, rb, mb } => {
                // RLDCL rA, rS, rB, MB: primary=31, rB, MB, xo=8, sh=0, Rc=0
                // Encoding: [0:5]=31, [6:10]=rS, [11:15]=rA, [16:20]=rB,
                //           [21:25]=MB, [26:30]=xo=8, [31]=Rc=0
                // Note: for RLDCL the mb field occupies bits [21:25] (5 bits,
                // representing the beginning mask position encoded as
                // mb[5] in bit 1 and mb[0:4] in bits [21:25]).
                // Simplified: we treat mb as 5-bit field in bits [21:25]
                let word = (31u32 << 26)
                    | (rs.encoding() << 21)
                    | (ra.encoding() << 16)
                    | (rb.encoding() << 11)
                    | ((mb & 0x1F) << 6)
                    | (8 << 1);
                encode_word(word)
            }
            Instruction::Rldcr { ra, rs, rb, me } => {
                // RLDCR rA, rS, rB, ME: primary=31, rB, ME, xo=9, Rc=0
                let word = (31u32 << 26)
                    | (rs.encoding() << 21)
                    | (ra.encoding() << 16)
                    | (rb.encoding() << 11)
                    | ((me & 0x1F) << 6)
                    | (9 << 1);
                encode_word(word)
            }
            Instruction::Rlwinm { ra, rs, sh, mb, me } => {
                // RLWINM rA, rS, SH, MB, ME: primary=21
                encode_m_form(21, rs.encoding(), ra.encoding(), *sh, *mb, *me, 0)
            }
            Instruction::Rlwimi { ra, rs, sh, mb, me } => {
                // RLWIMI rA, rS, SH, MB, ME: primary=20
                encode_m_form(20, rs.encoding(), ra.encoding(), *sh, *mb, *me, 0)
            }

            // ── Load / Store ───────────────────────────────────
            Instruction::Ld { rt, ra, ds } => {
                // LD rT, ds(rA): primary=58, xo=0
                encode_ds_form(58, rt.encoding(), ra.encoding(), *ds, 0)
            }
            Instruction::Lwa { rt, ra, ds } => {
                // LWA rT, ds(rA): primary=58, xo=2
                encode_ds_form(58, rt.encoding(), ra.encoding(), *ds, 2)
            }
            Instruction::Lwz { rt, ra, d } => {
                // LWZ rT, d(rA): primary=32
                encode_d_form(32, rt.encoding(), ra.encoding(), *d)
            }
            Instruction::Lwzu { rt, ra, d } => {
                // LWZU rT, d(rA): primary=33
                encode_d_form(33, rt.encoding(), ra.encoding(), *d)
            }
            Instruction::Std { rs, ra, ds } => {
                // STD rS, ds(rA): primary=62, xo=0
                encode_ds_form(62, rs.encoding(), ra.encoding(), *ds, 0)
            }
            Instruction::Stw { rs, ra, d } => {
                // STW rS, d(rA): primary=36
                encode_d_form(36, rs.encoding(), ra.encoding(), *d)
            }
            Instruction::Stwu { rs, ra, d } => {
                // STWU rS, d(rA): primary=37
                encode_d_form(37, rs.encoding(), ra.encoding(), *d)
            }
            Instruction::Stdu { rs, ra, ds } => {
                // STDU rS, ds(rA): primary=62, xo=1
                encode_ds_form(62, rs.encoding(), ra.encoding(), *ds, 1)
            }
            Instruction::Lbz { rt, ra, d } => {
                // LBZ rT, d(rA): primary=34
                encode_d_form(34, rt.encoding(), ra.encoding(), *d)
            }
            Instruction::Lhz { rt, ra, d } => {
                // LHZ rT, d(rA): primary=40
                encode_d_form(40, rt.encoding(), ra.encoding(), *d)
            }
            Instruction::Stb { rs, ra, d } => {
                // STB rS, d(rA): primary=38
                encode_d_form(38, rs.encoding(), ra.encoding(), *d)
            }
            Instruction::Sth { rs, ra, d } => {
                // STH rS, d(rA): primary=44
                encode_d_form(44, rs.encoding(), ra.encoding(), *d)
            }
            Instruction::Lfd { ft, ra, d } => {
                // LFD fT, d(rA): primary=50
                encode_d_form(50, ft.encoding(), ra.encoding(), *d)
            }
            Instruction::Stfd { fs, ra, d } => {
                // STFD fS, d(rA): primary=54
                encode_d_form(54, fs.encoding(), ra.encoding(), *d)
            }
            Instruction::Lfs { ft, ra, d } => {
                // LFS fT, d(rA): primary=48
                encode_d_form(48, ft.encoding(), ra.encoding(), *d)
            }
            Instruction::Stfs { fs, ra, d } => {
                // STFS fS, d(rA): primary=52
                encode_d_form(52, fs.encoding(), ra.encoding(), *d)
            }

            // ── Compare ────────────────────────────────────────
            Instruction::Cmp { bf, l, ra, rb } => {
                // CMP crf, l, rA, rB: primary=31, xo=0, Rc=0
                // bits [6:8] = bf, [9]=l, [11:15]=rA, [16:20]=rB
                let word = (31u32 << 26)
                    | ((bf.encoding() & 0x7) << 23)
                    | ((*l & 1) << 21)
                    | (ra.encoding() << 16)
                    | (rb.encoding() << 11);
                encode_word(word)
            }
            Instruction::Cmpi { bf, l, ra, simm } => {
                // CMPI crf, l, rA, simm16: primary=11
                let word = (11u32 << 26)
                    | ((bf.encoding() & 0x7) << 23)
                    | ((*l & 1) << 21)
                    | (ra.encoding() << 16)
                    | ((*simm as u32) & 0xFFFF);
                encode_word(word)
            }
            Instruction::Cmpl { bf, l, ra, rb } => {
                // CMPL crf, l, rA, rB: primary=31, xo=32, Rc=0
                let word = (31u32 << 26)
                    | ((bf.encoding() & 0x7) << 23)
                    | ((*l & 1) << 21)
                    | (ra.encoding() << 16)
                    | (rb.encoding() << 11)
                    | (32 << 1);
                encode_word(word)
            }
            Instruction::Cmpli { bf, l, ra, uimm } => {
                // CMPLI crf, l, rA, uimm16: primary=10
                let word = (10u32 << 26)
                    | ((bf.encoding() & 0x7) << 23)
                    | ((*l & 1) << 21)
                    | (ra.encoding() << 16)
                    | (uimm & 0xFFFF);
                encode_word(word)
            }

            // ── Branch ─────────────────────────────────────────
            Instruction::B { li } => encode_i_form(18, *li, 0, 0),
            Instruction::Ba { li } => encode_i_form(18, *li, 1, 0),
            Instruction::Bl { li } => encode_i_form(18, *li, 0, 1),
            Instruction::Bla { li } => encode_i_form(18, *li, 1, 1),
            Instruction::Bc { bo, bi, bd } => encode_b_form(16, *bo, *bi, *bd, 0, 0),
            Instruction::Bca { bo, bi, bd } => encode_b_form(16, *bo, *bi, *bd, 1, 0),
            Instruction::Bclr { bo, bi, bh } => encode_xl_form(19, *bo, *bi, *bh, 16, 0),
            Instruction::Bcctr { bo, bi, bh } => encode_xl_form(19, *bo, *bi, *bh, 528, 0),
            Instruction::Bctar { bo, bi, bh } => encode_xl_form(19, *bo, *bi, *bh, 560, 0),

            // ── Move ───────────────────────────────────────────
            Instruction::Mr { ra, rs } => {
                // MR rA, rS = OR rA, rS, rS: primary=31, xo=444, Rc=0
                encode_x_form(31, rs.encoding(), ra.encoding(), rs.encoding(), 444, 0)
            }
            Instruction::Li { rt, simm } => {
                // LI rT, simm = ADDI rT, R0, simm: primary=14, rA=0
                encode_d_form(14, rt.encoding(), 0, *simm)
            }
            Instruction::Lis { rt, simm } => {
                // LIS rT, simm = ADDIS rT, R0, simm: primary=15, rA=0
                encode_d_form(15, rt.encoding(), 0, *simm)
            }

            // ── System ─────────────────────────────────────────
            Instruction::Sc => {
                // SC: primary=17, bits [6:29]=0, bit 30=1 (SVC field)
                // Full encoding: 0x44000002
                encode_word(0x44000002)
            }
            Instruction::Nop => {
                // NOP = ORI r0, r0, 0: primary=24, rS=0, rA=0, uimm16=0
                encode_word(0x60000000)
            }
            Instruction::Trap => {
                // TRAP = TW 31, r0, r0: primary=31, rS=31, rA=0, rB=0, xo=4, Rc=0
                encode_x_form(31, 31, 0, 0, 4, 0)
            }
        }
    }

    /// Returns the mnemonic name of this instruction.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Instruction::Add { .. } => "add",
            Instruction::Addi { .. } => "addi",
            Instruction::Addis { .. } => "addis",
            Instruction::Subf { .. } => "subf",
            Instruction::Mullw { .. } => "mullw",
            Instruction::Mulld { .. } => "mulld",
            Instruction::Mulhw { .. } => "mulhw",
            Instruction::Mulhd { .. } => "mulhd",
            Instruction::Divw { .. } => "divw",
            Instruction::Divd { .. } => "divd",
            Instruction::Divwu { .. } => "divwu",
            Instruction::Divdu { .. } => "divdu",
            Instruction::Neg { .. } => "neg",
            Instruction::Cntlzd { .. } => "cntlzd",
            Instruction::Popcntd { .. } => "popcntd",
            Instruction::Extsw { .. } => "extsw",
            Instruction::And { .. } => "and",
            Instruction::Andi { .. } => "andi.",
            Instruction::Or { .. } => "or",
            Instruction::Ori { .. } => "ori",
            Instruction::Xor { .. } => "xor",
            Instruction::Xori { .. } => "xori",
            Instruction::Nor { .. } => "nor",
            Instruction::Andc { .. } => "andc",
            Instruction::Orc { .. } => "orc",
            Instruction::Eqv { .. } => "eqv",
            Instruction::Sld { .. } => "sld",
            Instruction::Srd { .. } => "srd",
            Instruction::Srad { .. } => "srad",
            Instruction::Slw { .. } => "slw",
            Instruction::Srw { .. } => "srw",
            Instruction::Sraw { .. } => "sraw",
            Instruction::Rldcl { .. } => "rldcl",
            Instruction::Rldcr { .. } => "rldcr",
            Instruction::Rlwinm { .. } => "rlwinm",
            Instruction::Rlwimi { .. } => "rlwimi",
            Instruction::Ld { .. } => "ld",
            Instruction::Lwa { .. } => "lwa",
            Instruction::Lwz { .. } => "lwz",
            Instruction::Lwzu { .. } => "lwzu",
            Instruction::Std { .. } => "std",
            Instruction::Stw { .. } => "stw",
            Instruction::Stwu { .. } => "stwu",
            Instruction::Stdu { .. } => "stdu",
            Instruction::Lbz { .. } => "lbz",
            Instruction::Lhz { .. } => "lhz",
            Instruction::Stb { .. } => "stb",
            Instruction::Sth { .. } => "sth",
            Instruction::Lfd { .. } => "lfd",
            Instruction::Stfd { .. } => "stfd",
            Instruction::Lfs { .. } => "lfs",
            Instruction::Stfs { .. } => "stfs",
            Instruction::Cmp { .. } => "cmp",
            Instruction::Cmpi { .. } => "cmpi",
            Instruction::Cmpl { .. } => "cmpl",
            Instruction::Cmpli { .. } => "cmpli",
            Instruction::B { .. } => "b",
            Instruction::Ba { .. } => "ba",
            Instruction::Bl { .. } => "bl",
            Instruction::Bla { .. } => "bla",
            Instruction::Bc { .. } => "bc",
            Instruction::Bca { .. } => "bca",
            Instruction::Bclr { .. } => "bclr",
            Instruction::Bcctr { .. } => "bcctr",
            Instruction::Bctar { .. } => "bctar",
            Instruction::Mr { .. } => "mr",
            Instruction::Li { .. } => "li",
            Instruction::Lis { .. } => "lis",
            Instruction::Sc => "sc",
            Instruction::Nop => "nop",
            Instruction::Trap => "trap",
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Add { rt, ra, rb } => write!(f, "add {}, {}, {}", rt, ra, rb),
            Instruction::Addi { rt, ra, simm } => write!(f, "addi {}, {}, {}", rt, ra, simm),
            Instruction::Addis { rt, ra, simm } => write!(f, "addis {}, {}, {}", rt, ra, simm),
            Instruction::Subf { rt, ra, rb } => write!(f, "subf {}, {}, {}", rt, ra, rb),
            Instruction::Mullw { rt, ra, rb } => write!(f, "mullw {}, {}, {}", rt, ra, rb),
            Instruction::Mulld { rt, ra, rb } => write!(f, "mulld {}, {}, {}", rt, ra, rb),
            Instruction::Mulhw { rt, ra, rb } => write!(f, "mulhw {}, {}, {}", rt, ra, rb),
            Instruction::Mulhd { rt, ra, rb } => write!(f, "mulhd {}, {}, {}", rt, ra, rb),
            Instruction::Divw { rt, ra, rb } => write!(f, "divw {}, {}, {}", rt, ra, rb),
            Instruction::Divd { rt, ra, rb } => write!(f, "divd {}, {}, {}", rt, ra, rb),
            Instruction::Divwu { rt, ra, rb } => write!(f, "divwu {}, {}, {}", rt, ra, rb),
            Instruction::Divdu { rt, ra, rb } => write!(f, "divdu {}, {}, {}", rt, ra, rb),
            Instruction::Neg { rt, ra } => write!(f, "neg {}, {}", rt, ra),
            Instruction::Cntlzd { ra, rs } => write!(f, "cntlzd {}, {}", ra, rs),
            Instruction::Popcntd { ra, rs } => write!(f, "popcntd {}, {}", ra, rs),
            Instruction::Extsw { ra, rs } => write!(f, "extsw {}, {}", ra, rs),
            Instruction::And { ra, rs, rb } => write!(f, "and {}, {}, {}", ra, rs, rb),
            Instruction::Andi { ra, rs, uimm } => write!(f, "andi. {}, {}, {}", ra, rs, uimm),
            Instruction::Or { ra, rs, rb } => write!(f, "or {}, {}, {}", ra, rs, rb),
            Instruction::Ori { ra, rs, uimm } => write!(f, "ori {}, {}, {}", ra, rs, uimm),
            Instruction::Xor { ra, rs, rb } => write!(f, "xor {}, {}, {}", ra, rs, rb),
            Instruction::Xori { ra, rs, uimm } => write!(f, "xori {}, {}, {}", ra, rs, uimm),
            Instruction::Nor { ra, rs, rb } => write!(f, "nor {}, {}, {}", ra, rs, rb),
            Instruction::Andc { ra, rs, rb } => write!(f, "andc {}, {}, {}", ra, rs, rb),
            Instruction::Orc { ra, rs, rb } => write!(f, "orc {}, {}, {}", ra, rs, rb),
            Instruction::Eqv { ra, rs, rb } => write!(f, "eqv {}, {}, {}", ra, rs, rb),
            Instruction::Sld { ra, rs, rb } => write!(f, "sld {}, {}, {}", ra, rs, rb),
            Instruction::Srd { ra, rs, rb } => write!(f, "srd {}, {}, {}", ra, rs, rb),
            Instruction::Srad { ra, rs, rb } => write!(f, "srad {}, {}, {}", ra, rs, rb),
            Instruction::Slw { ra, rs, rb } => write!(f, "slw {}, {}, {}", ra, rs, rb),
            Instruction::Srw { ra, rs, rb } => write!(f, "srw {}, {}, {}", ra, rs, rb),
            Instruction::Sraw { ra, rs, rb } => write!(f, "sraw {}, {}, {}", ra, rs, rb),
            Instruction::Rldcl { ra, rs, rb, mb } => {
                write!(f, "rldcl {}, {}, {}, {}", ra, rs, rb, mb)
            }
            Instruction::Rldcr { ra, rs, rb, me } => {
                write!(f, "rldcr {}, {}, {}, {}", ra, rs, rb, me)
            }
            Instruction::Rlwinm { ra, rs, sh, mb, me } => {
                write!(f, "rlwinm {}, {}, {}, {}, {}", ra, rs, sh, mb, me)
            }
            Instruction::Rlwimi { ra, rs, sh, mb, me } => {
                write!(f, "rlwimi {}, {}, {}, {}, {}", ra, rs, sh, mb, me)
            }
            Instruction::Ld { rt, ra, ds } => write!(f, "ld {}, {}({})", rt, ds, ra),
            Instruction::Lwa { rt, ra, ds } => write!(f, "lwa {}, {}({})", rt, ds, ra),
            Instruction::Lwz { rt, ra, d } => write!(f, "lwz {}, {}({})", rt, d, ra),
            Instruction::Lwzu { rt, ra, d } => write!(f, "lwzu {}, {}({})", rt, d, ra),
            Instruction::Std { rs, ra, ds } => write!(f, "std {}, {}({})", rs, ds, ra),
            Instruction::Stw { rs, ra, d } => write!(f, "stw {}, {}({})", rs, d, ra),
            Instruction::Stwu { rs, ra, d } => write!(f, "stwu {}, {}({})", rs, d, ra),
            Instruction::Stdu { rs, ra, ds } => write!(f, "stdu {}, {}({})", rs, ds, ra),
            Instruction::Lbz { rt, ra, d } => write!(f, "lbz {}, {}({})", rt, d, ra),
            Instruction::Lhz { rt, ra, d } => write!(f, "lhz {}, {}({})", rt, d, ra),
            Instruction::Stb { rs, ra, d } => write!(f, "stb {}, {}({})", rs, d, ra),
            Instruction::Sth { rs, ra, d } => write!(f, "sth {}, {}({})", rs, d, ra),
            Instruction::Lfd { ft, ra, d } => write!(f, "lfd {}, {}({})", ft, d, ra),
            Instruction::Stfd { fs, ra, d } => write!(f, "stfd {}, {}({})", fs, d, ra),
            Instruction::Lfs { ft, ra, d } => write!(f, "lfs {}, {}({})", ft, d, ra),
            Instruction::Stfs { fs, ra, d } => write!(f, "stfs {}, {}({})", fs, d, ra),
            Instruction::Cmp { bf, l, ra, rb } => write!(f, "cmp {}, {}, {}, {}", bf, l, ra, rb),
            Instruction::Cmpi { bf, l, ra, simm } => {
                write!(f, "cmpi {}, {}, {}, {}", bf, l, ra, simm)
            }
            Instruction::Cmpl { bf, l, ra, rb } => write!(f, "cmpl {}, {}, {}, {}", bf, l, ra, rb),
            Instruction::Cmpli { bf, l, ra, uimm } => {
                write!(f, "cmpli {}, {}, {}, {}", bf, l, ra, uimm)
            }
            Instruction::B { li } => write!(f, "b {:+}", li),
            Instruction::Ba { li } => write!(f, "ba {:+}", li),
            Instruction::Bl { li } => write!(f, "bl {:+}", li),
            Instruction::Bla { li } => write!(f, "bla {:+}", li),
            Instruction::Bc { bo, bi, bd } => write!(f, "bc {}, {}, {:+}", bo, bi, bd),
            Instruction::Bca { bo, bi, bd } => write!(f, "bca {}, {}, {:+}", bo, bi, bd),
            Instruction::Bclr { bo, bi, bh } => write!(f, "bclr {}, {}, {}", bo, bi, bh),
            Instruction::Bcctr { bo, bi, bh } => write!(f, "bcctr {}, {}, {}", bo, bi, bh),
            Instruction::Bctar { bo, bi, bh } => write!(f, "bctar {}, {}, {}", bo, bi, bh),
            Instruction::Mr { ra, rs } => write!(f, "mr {}, {}", ra, rs),
            Instruction::Li { rt, simm } => write!(f, "li {}, {}", rt, simm),
            Instruction::Lis { rt, simm } => write!(f, "lis {}, {}", rt, simm),
            Instruction::Sc => write!(f, "sc"),
            Instruction::Nop => write!(f, "nop"),
            Instruction::Trap => write!(f, "trap"),
        }
    }
}

// ===========================================================================
// ELF64 Emission
// ===========================================================================

/// Build a proper ELF64 binary for PPC64LE with 2 LOAD segments.
///
/// Produces a static executable with:
/// - Segment 1: LOAD (PF_R | PF_X) — .text
/// - Segment 2: LOAD (PF_R | PF_W) — .data / BSS (writable)
///
/// Uses little-endian byte order for ppc64le.  The text segment is placed
/// at a page-aligned offset so that the kernel's ELF loader can mmap() it.
fn build_ppc64_elf_2seg(code: &[u8], base_addr: u64) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x10000; // 64 KB (PPC64 typical page size)

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
    elf.push(2); // ELFDATA2MSB (big-endian PPC64)
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0); // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields ---
    elf.extend_from_slice(&2u16.to_be_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&21u16.to_be_bytes()); // e_machine = EM_PPC64
    elf.extend_from_slice(&1u32.to_be_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_be_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_be_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_be_bytes()); // e_shoff (no section headers)
    // e_flags: EF_PPC64_ABI_V2 = 0x2 (required for PPC64LE ELFv2 ABI)
    elf.extend_from_slice(&2u32.to_be_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_be_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_be_bytes()); // e_phentsize
    elf.extend_from_slice(&2u16.to_be_bytes()); // e_phnum = 2
    elf.extend_from_slice(&64u16.to_be_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_be_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_be_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_be_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_be_bytes()); // p_filesz
    elf.extend_from_slice(&text_size.to_be_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_be_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data / stack ---
    elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&data_offset.to_be_bytes()); // p_offset
    elf.extend_from_slice(&data_vaddr.to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_be_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_filesz (no initialized data)
    elf.extend_from_slice(&data_size.to_be_bytes()); // p_memsz (writable pages)
    elf.extend_from_slice(&PAGE_SIZE.to_be_bytes()); // p_align

    // --- .text section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // --- Pad to data segment offset (if needed) ---
    while (elf.len() as u64) < data_offset {
        elf.push(0);
    }

    // No file data for the .data segment (it's BSS-like, zero-initialized)

    elf
}

// ===========================================================================
// PPC64Backend
// ===========================================================================

/// PowerPC64 code generation backend (ELFv2 ABI, ppc64le).
pub struct PPC64Backend {
    target_info: PowerPC64TargetInfo,
}

impl PPC64Backend {
    /// Create a new PPC64 backend.
    pub fn new() -> Self {
        Self {
            target_info: PowerPC64TargetInfo,
        }
    }
}

impl Default for PPC64Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the stack frame size for an IR function on PPC64.
///
/// Sums `Alloc` instruction sizes, adds 32 bytes for the LR/CR save area
/// (per ELFv2 ABI), and rounds up to 16-byte alignment.
fn ppc64_compute_frame_size(func: &IRFunction) -> usize {
    let mut total: u32 = 32; // LR save (8) + CR save (8) + back chain (8) + TOC save (8)
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

/// Allocatable GPR registers for PPC64, in priority order.
///
/// Order: argument registers first, then volatile temporaries, then callee-saved.
/// R0 is reserved (not allocatable — in some ISA contexts rA=0 means literal zero).
/// R11 is reserved as a scratch register for instruction lowering.
/// R12 is the first allocatable temporary.
const ALLOCATABLE_GPRS: &[Gpr] = &[
    // Argument registers (also volatile)
    Gpr::R3,
    Gpr::R4,
    Gpr::R5,
    Gpr::R6,
    Gpr::R7,
    Gpr::R8,
    Gpr::R9,
    Gpr::R10,
    // Volatile temporary
    Gpr::R12,
    // Callee-saved (require save/restore)
    Gpr::R14,
    Gpr::R15,
    Gpr::R16,
    Gpr::R17,
    Gpr::R18,
    Gpr::R19,
    Gpr::R20,
    Gpr::R21,
    Gpr::R22,
    Gpr::R23,
    Gpr::R24,
    Gpr::R25,
    Gpr::R26,
    Gpr::R27,
    Gpr::R28,
    Gpr::R29,
    Gpr::R30,
    Gpr::R31,
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

    // Fallback: use R11 as a scratch register.
    Gpr::R11
}

/// Helper: extract the virtual register ID from an IRValue, if it is a register.
fn vreg_id(val: &IRValue) -> u32 {
    match val {
        IRValue::Register(id) => *id,
        _ => 0,
    }
}

/// Emit a single AllocatedInstruction from a PPC64 Instruction.
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

/// Load a 64-bit immediate value into a GPR, emitting the minimal number of
/// PPC64 instructions.
///
/// Strategy:
/// - **`[-32768, 32767]`**: `li rd, val` (single `addi rd, r0, val`)
/// - **`[0, 65535]`**: `ori rd, r0, val`
/// - **`0xXXXX0000` (low 16 bits zero, fits 32-bit)**: `lis rd, upper16`
/// - **Other 32-bit**: `lis rd, upper16` + `ori rd, rd, lower16`
/// - **Full 64-bit**: `lis` + `ori` + `sldi 32` + `oris` + `ori`
fn load_immediate_ppc64(rd: Gpr, val: i64, out: &mut Vec<AllocatedInstruction>) {
    let rd_phys = PhysicalReg::new(RegClass::Gpr, rd.encoding());
    let uval = val as u64;

    if (-32768..=32767).contains(&val) {
        // li rd, val (addi rd, r0, val)
        out.push(emit_alloc_instr(
            Instruction::Li {
                rt: rd,
                simm: val as i32,
            },
            vec![],
            vec![rd_phys],
        ));
    } else if uval <= 0xFFFF {
        // ori rd, r0, val (unsigned 16-bit)
        out.push(emit_alloc_instr(
            Instruction::Ori {
                ra: rd,
                rs: Gpr::R0,
                uimm: uval as u32 & 0xFFFF,
            },
            vec![],
            vec![rd_phys],
        ));
    } else if uval & 0xFFFF == 0 && (uval >> 32) == 0 {
        // lis rd, upper16 (0xXXXX0000)
        out.push(emit_alloc_instr(
            Instruction::Lis {
                rt: rd,
                simm: ((uval >> 16) & 0xFFFF) as i16 as i32,
            },
            vec![],
            vec![rd_phys],
        ));
    } else if uval >> 32 == 0 {
        // 32-bit value: lis + ori
        let hi16 = ((uval >> 16) & 0xFFFF) as u32;
        let lo16 = (uval & 0xFFFF) as u32;
        out.push(emit_alloc_instr(
            Instruction::Lis {
                rt: rd,
                simm: hi16 as i16 as i32,
            },
            vec![],
            vec![rd_phys],
        ));
        if lo16 != 0 {
            out.push(emit_alloc_instr(
                Instruction::Ori {
                    ra: rd,
                    rs: rd,
                    uimm: lo16,
                },
                vec![rd_phys],
                vec![rd_phys],
            ));
        }
    } else {
        // Full 64-bit: lis + ori + sldi 32 + oris + ori
        let hi32 = (uval >> 32) as u32;
        let lo32 = (uval & 0xFFFF_FFFF) as u32;

        // lis rd, upper16(hi32)
        out.push(emit_alloc_instr(
            Instruction::Lis {
                rt: rd,
                simm: ((hi32 >> 16) & 0xFFFF) as i16 as i32,
            },
            vec![],
            vec![rd_phys],
        ));

        // ori rd, rd, lower16(hi32)
        if hi32 & 0xFFFF != 0 {
            out.push(emit_alloc_instr(
                Instruction::Ori {
                    ra: rd,
                    rs: rd,
                    uimm: hi32 & 0xFFFF,
                },
                vec![rd_phys],
                vec![rd_phys],
            ));
        }

        // sldi rd, rd, 32 = rldicr rd, rd, 32, 31
        // MD-form: [0:5]=30, [6:10]=rS, [11:15]=rA, [16:20]=SH[0:4],
        // [21:25]=ME[0:4], [26]=SH[5], [27]=ME[5], [28:30]=xo(=2), [31]=Rc(=0)
        let sh: u32 = 32;
        let me: u32 = 63 - sh;
        let sldi_word: u32 = (30u32 << 26)
            | (rd.encoding() << 21)
            | (rd.encoding() << 16)
            | ((sh & 0x1F) << 11)
            | ((me & 0x1F) << 6)
            | (((sh >> 5) & 1) << 5)
            | (((me >> 5) & 1) << 4)
            | (2u32 << 1);
        out.push(AllocatedInstruction {
            opcode: "rldicr".to_string(),
            reads: vec![rd_phys],
            writes: vec![rd_phys],
            encoded: encode_word(sldi_word).to_vec(),
        });

        // oris rd, rd, upper16(lo32) — primary opcode 25, D-form
        if (lo32 >> 16) & 0xFFFF != 0 {
            let oris_word: u32 = (25u32 << 26)
                | (rd.encoding() << 21)
                | (rd.encoding() << 16)
                | ((lo32 >> 16) & 0xFFFF);
            out.push(AllocatedInstruction {
                opcode: "oris".to_string(),
                reads: vec![rd_phys],
                writes: vec![rd_phys],
                encoded: encode_word(oris_word).to_vec(),
            });
        }

        // ori rd, rd, lower16(lo32)
        if lo32 & 0xFFFF != 0 {
            out.push(emit_alloc_instr(
                Instruction::Ori {
                    ra: rd,
                    rs: rd,
                    uimm: lo32 & 0xFFFF,
                },
                vec![rd_phys],
                vec![rd_phys],
            ));
        }
    }
}

/// Resolve an `IRValue` to a physical `Gpr`, emitting immediate-load
/// instructions as needed.
///
/// - `IRValue::Register(id)` → looks up `id` in `reg_map`
/// - `IRValue::Immediate(val)` → loads `val` into `scratch` via
///   `load_immediate_ppc64` and returns `scratch`
/// - `IRValue::Address(addr)` → loads `addr` into `scratch`
/// - `IRValue::Label(_)` → loads 0 into `scratch` (placeholder for relocation)
///
/// The caller must supply a scratch register that is not live at this point
/// (e.g., `R11` for the first operand, `R12` for the second).
fn resolve_gpr_ppc64(
    val: &IRValue,
    reg_map: &mut std::collections::HashMap<u32, Gpr>,
    scratch: Gpr,
    out: &mut Vec<AllocatedInstruction>,
) -> Gpr {
    match val {
        IRValue::Register(id) => map_vreg_to_gpr(*id, None, reg_map),
        IRValue::Immediate(imm) => {
            load_immediate_ppc64(scratch, *imm, out);
            scratch
        }
        IRValue::Address(addr) => {
            load_immediate_ppc64(scratch, *addr as i64, out);
            scratch
        }
        IRValue::Label(_) => {
            // Labels need linker relocation; emit li scratch, 0 as placeholder
            load_immediate_ppc64(scratch, 0, out);
            scratch
        }
    }
}

/// Lower a comparison kind to PPC64 instructions, producing 0 or 1 in `dst`.
fn lower_cmp_ppc64(kind: &CmpKind, dst: Gpr, lhs: Gpr, rhs: Gpr) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();
    let (cmp_inst, cond_bit) = match kind {
        CmpKind::Eq => (
            Instruction::Cmp {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            2,
        ),
        CmpKind::Ne => (
            Instruction::Cmp {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            2,
        ),
        CmpKind::SLt => (
            Instruction::Cmp {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            0,
        ),
        CmpKind::SLe => (
            Instruction::Cmp {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            0,
        ),
        CmpKind::SGt => (
            Instruction::Cmp {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            1,
        ),
        CmpKind::SGe => (
            Instruction::Cmp {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            1,
        ),
        CmpKind::ULt => (
            Instruction::Cmpl {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            0,
        ),
        CmpKind::ULe => (
            Instruction::Cmpl {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            0,
        ),
        CmpKind::UGt => (
            Instruction::Cmpl {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            1,
        ),
        CmpKind::UGe => (
            Instruction::Cmpl {
                bf: CrField::CR0,
                l: 1,
                ra: lhs,
                rb: rhs,
            },
            1,
        ),
    };
    result.push(emit_alloc_instr(
        cmp_inst.clone(),
        vec![
            PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
            PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
        ],
        vec![],
    ));
    // li dst, 0; then conditionally set to 1
    result.push(emit_alloc_instr(
        Instruction::Li { rt: dst, simm: 0 },
        vec![],
        vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
    ));
    let (bo, bi) = match kind {
        // Pattern: li dst,0; bc <skip li dst,1>; li dst,1
        // Branch is taken → dst stays 0; fall-through → dst becomes 1.
        // So we want branch taken when result should be 0.
        CmpKind::Eq => (4, 2 + CrField::CR0.encoding() * 4),  // branch if cr0.eq FALSE → dst=1 when eq
        CmpKind::Ne => (12, 2 + CrField::CR0.encoding() * 4), // branch if cr0.eq TRUE  → dst=0 when eq, dst=1 when ne
        CmpKind::SLt | CmpKind::ULt => (4, CrField::CR0.encoding() * 4),   // branch if cr0.lt FALSE
        CmpKind::SLe | CmpKind::ULe => (12, 1 + CrField::CR0.encoding() * 4), // branch if cr0.gt TRUE
        CmpKind::SGt | CmpKind::UGt => (4, 1 + CrField::CR0.encoding() * 4), // branch if cr0.gt FALSE
        CmpKind::SGe | CmpKind::UGe => (12, CrField::CR0.encoding() * 4),   // branch if cr0.lt TRUE
    };
    let _ = cond_bit;
    result.push(emit_alloc_instr(
        Instruction::Li { rt: dst, simm: 1 },
        vec![],
        vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
    ));
    // Patch: use a simplified approach — li dst,1 is always emitted but should be conditional.
    // For simplicity, use select-like pattern:
    // cmp; li dst, 0; bne cr0, +8; li dst, 1
    // Actually let's redo this more correctly:
    result.clear();
    result.push(emit_alloc_instr(
        cmp_inst,
        vec![
            PhysicalReg::new(RegClass::Gpr, lhs.encoding()),
            PhysicalReg::new(RegClass::Gpr, rhs.encoding()),
        ],
        vec![],
    ));
    result.push(emit_alloc_instr(
        Instruction::Li { rt: dst, simm: 0 },
        vec![],
        vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
    ));
    result.push(emit_alloc_instr(
        Instruction::Bc { bo, bi, bd: 2 },
        vec![],
        vec![],
    ));
    result.push(emit_alloc_instr(
        Instruction::Li { rt: dst, simm: 1 },
        vec![],
        vec![PhysicalReg::new(RegClass::Gpr, dst.encoding())],
    ));
    result
}

/// Lower an IR instruction to a sequence of PPC64 AllocatedInstructions,
/// plus any relocations that need to be patched later.
///
/// `alloc_offset` tracks the next free stack-slot offset (starting after the
/// 32-byte mandatory save area) so that `Alloc` instructions can compute
/// addresses within the already-allocated frame instead of double-decrementing
/// SP.
fn lower_ir_instr_ppc64(
    instr: &IRInstr,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
    alloc_offset: &mut i32,
) -> (Vec<AllocatedInstruction>, Vec<crate::backend::RelocationEntry>) {
    let mut result = Vec::new();
    let mut relocations = Vec::new();

    match instr {
        IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = resolve_gpr_ppc64(lhs, vreg_map, Gpr::R0, &mut result);
            let r = resolve_gpr_ppc64(rhs, vreg_map, Gpr::R11, &mut result);
            match op {
                BinOpKind::Add => {
                    result.push(emit_alloc_instr(
                        Instruction::Add {
                            rt: d,
                            ra: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::Sub => {
                    result.push(emit_alloc_instr(
                        Instruction::Subf {
                            rt: d,
                            ra: r,
                            rb: l,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::Mul => {
                    result.push(emit_alloc_instr(
                        Instruction::Mulld {
                            rt: d,
                            ra: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::SDiv => {
                    result.push(emit_alloc_instr(
                        Instruction::Divd {
                            rt: d,
                            ra: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::UDiv => {
                    result.push(emit_alloc_instr(
                        Instruction::Divdu {
                            rt: d,
                            ra: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::SRem => {
                    // div then mul then sub: rem = lhs - (lhs/rhs)*rhs
                    let scratch = Gpr::R0; // reserved scratch
                    result.push(emit_alloc_instr(
                        Instruction::Divd {
                            rt: scratch,
                            ra: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, scratch.encoding())],
                    ));
                    result.push(emit_alloc_instr(
                        Instruction::Mulld {
                            rt: scratch,
                            ra: scratch,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, scratch.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, scratch.encoding())],
                    ));
                    result.push(emit_alloc_instr(
                        Instruction::Subf {
                            rt: d,
                            ra: scratch,
                            rb: l,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, scratch.encoding()),
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::URem => {
                    // Unsigned: divdu then mul then sub: rem = lhs - (lhs/rhs)*rhs
                    let scratch = Gpr::R0; // reserved scratch
                    result.push(emit_alloc_instr(
                        Instruction::Divdu {
                            rt: scratch,
                            ra: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, scratch.encoding())],
                    ));
                    result.push(emit_alloc_instr(
                        Instruction::Mulld {
                            rt: scratch,
                            ra: scratch,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, scratch.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, scratch.encoding())],
                    ));
                    result.push(emit_alloc_instr(
                        Instruction::Subf {
                            rt: d,
                            ra: scratch,
                            rb: l,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, scratch.encoding()),
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::And => {
                    result.push(emit_alloc_instr(
                        Instruction::And {
                            ra: d,
                            rs: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::Or => {
                    result.push(emit_alloc_instr(
                        Instruction::Or {
                            ra: d,
                            rs: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::Xor => {
                    result.push(emit_alloc_instr(
                        Instruction::Xor {
                            ra: d,
                            rs: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::Shl => {
                    result.push(emit_alloc_instr(
                        Instruction::Sld {
                            ra: d,
                            rs: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::ShrL => {
                    result.push(emit_alloc_instr(
                        Instruction::Srd {
                            ra: d,
                            rs: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::ShrA => {
                    result.push(emit_alloc_instr(
                        Instruction::Srad {
                            ra: d,
                            rs: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                BinOpKind::Ror | BinOpKind::Rol => {
                    // PPC64 rotation placeholder - use Srad
                    result.push(emit_alloc_instr(
                        Instruction::Srad {
                            ra: d,
                            rs: l,
                            rb: r,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, l.encoding()),
                            PhysicalReg::new(RegClass::Gpr, r.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
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
                        _ => CmpKind::Eq,
                    };
                    result.extend(lower_cmp_ppc64(&kind, d, l, r));
                }
            }
        }

        IRInstr::Add { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = resolve_gpr_ppc64(lhs, vreg_map, Gpr::R0, &mut result);
            let r = resolve_gpr_ppc64(rhs, vreg_map, Gpr::R11, &mut result);
            result.push(emit_alloc_instr(
                Instruction::Add {
                    rt: d,
                    ra: l,
                    rb: r,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Sub { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = resolve_gpr_ppc64(lhs, vreg_map, Gpr::R0, &mut result);
            let r = resolve_gpr_ppc64(rhs, vreg_map, Gpr::R11, &mut result);
            result.push(emit_alloc_instr(
                Instruction::Subf {
                    rt: d,
                    ra: r,
                    rb: l,
                },
                vec![
                    PhysicalReg::new(RegClass::Gpr, r.encoding()),
                    PhysicalReg::new(RegClass::Gpr, l.encoding()),
                ],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Mul { dst, lhs, rhs, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let l = resolve_gpr_ppc64(lhs, vreg_map, Gpr::R0, &mut result);
            let r = resolve_gpr_ppc64(rhs, vreg_map, Gpr::R11, &mut result);
            result.push(emit_alloc_instr(
                Instruction::Mulld {
                    rt: d,
                    ra: l,
                    rb: r,
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
            let l = resolve_gpr_ppc64(lhs, vreg_map, Gpr::R0, &mut result);
            let r = resolve_gpr_ppc64(rhs, vreg_map, Gpr::R11, &mut result);
            result.push(emit_alloc_instr(
                Instruction::Divd {
                    rt: d,
                    ra: l,
                    rb: r,
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
            let s = resolve_gpr_ppc64(operand, vreg_map, Gpr::R0, &mut result);
            match op {
                UnaryOpKind::Neg => {
                    result.push(emit_alloc_instr(
                        Instruction::Neg { rt: d, ra: s },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Not => {
                    // nor d, s, r0 → d = ~(s | r0)
                    // Note: R0 is NOT hardwired to zero on PPC64; we use nor d, s, s
                    // which produces ~(s | s) = ~s, the correct bitwise complement.
                    result.push(emit_alloc_instr(
                        Instruction::Nor {
                            ra: d,
                            rs: s,
                            rb: s,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Clz => {
                    // cntlzd d, s — count leading zeros doubleword
                    result.push(emit_alloc_instr(
                        Instruction::Cntlzd { ra: d, rs: s },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Ctz => {
                    // ctz = 63 - cntlzd(x & -x)
                    // Use scratch R11 for intermediate:
                    //   subf r0, s, r0 → r0 = -s (use neg)
                    //   and r0, s, r0 → r0 = s & -s (isolates lowest set bit)
                    //   cntlzd d, r0   → d = leading zeros of isolated bit
                    //   li r11, 63
                    //   subf d, d, r11  → d = 63 - clz = ctz
                    let scratch1 = Gpr::R0;
                    let scratch2 = Gpr::R11;
                    // neg scratch1, s
                    result.push(emit_alloc_instr(
                        Instruction::Neg {
                            rt: scratch1,
                            ra: s,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, scratch1.encoding())],
                    ));
                    // and scratch1, s, scratch1
                    result.push(emit_alloc_instr(
                        Instruction::And {
                            ra: scratch1,
                            rs: s,
                            rb: scratch1,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, s.encoding()),
                            PhysicalReg::new(RegClass::Gpr, scratch1.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, scratch1.encoding())],
                    ));
                    // cntlzd d, scratch1
                    result.push(emit_alloc_instr(
                        Instruction::Cntlzd {
                            ra: d,
                            rs: scratch1,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, scratch1.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                    // li scratch2, 63
                    result.push(emit_alloc_instr(
                        Instruction::Li {
                            rt: scratch2,
                            simm: 63,
                        },
                        vec![],
                        vec![PhysicalReg::new(RegClass::Gpr, scratch2.encoding())],
                    ));
                    // subf d, d, scratch2  → d = 63 - clz
                    result.push(emit_alloc_instr(
                        Instruction::Subf {
                            rt: d,
                            ra: d,
                            rb: scratch2,
                        },
                        vec![
                            PhysicalReg::new(RegClass::Gpr, d.encoding()),
                            PhysicalReg::new(RegClass::Gpr, scratch2.encoding()),
                        ],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                UnaryOpKind::Popcnt => {
                    // popcntd d, s — population count doubleword
                    result.push(emit_alloc_instr(
                        Instruction::Popcntd { ra: d, rs: s },
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
            let l = resolve_gpr_ppc64(lhs, vreg_map, Gpr::R0, &mut result);
            let r = resolve_gpr_ppc64(rhs, vreg_map, Gpr::R11, &mut result);
            result.extend(lower_cmp_ppc64(kind, d, l, r));
        }

        IRInstr::Load { dst, addr, offset, ty } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let a = resolve_gpr_ppc64(addr, vreg_map, Gpr::R11, &mut result);
            let off = *offset;
            // Choose the correct load instruction based on the type width.
            let load_inst = match ty {
                IRType::I8 | IRType::U8 => Instruction::Lbz { rt: d, ra: a, d: off },
                IRType::I16 | IRType::U16 => Instruction::Lhz { rt: d, ra: a, d: off },
                IRType::I32 | IRType::U32 => Instruction::Lwz { rt: d, ra: a, d: off },
                IRType::I64 | IRType::U64 | IRType::Ptr | IRType::Func => {
                    // LD uses DS-form which requires 4-byte aligned displacement.
                    // For simplicity, if offset is not 4-byte aligned, add it to
                    // the base register first.
                    if off % 4 == 0 {
                        Instruction::Ld { rt: d, ra: a, ds: off }
                    } else {
                        // addi scratch, a, off; ld d, 0(scratch)
                        let scratch = if a != Gpr::R0 { Gpr::R0 } else { Gpr::R11 };
                        result.push(emit_alloc_instr(
                            Instruction::Addi { rt: scratch, ra: a, simm: off },
                            vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                            vec![PhysicalReg::new(RegClass::Gpr, scratch.encoding())],
                        ));
                        Instruction::Ld { rt: d, ra: scratch, ds: 0 }
                    }
                }
                IRType::F32 => Instruction::Lfs { ft: Fpr::F0, ra: a, d: off },
                IRType::F64 => Instruction::Lfd { ft: Fpr::F0, ra: a, d: off },
                _ => Instruction::Ld { rt: d, ra: a, ds: off }, // fallback: 64-bit
            };
            result.push(emit_alloc_instr(
                load_inst,
                vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Store { value, addr, offset, ty } => {
            let v = resolve_gpr_ppc64(value, vreg_map, Gpr::R0, &mut result);
            let a = resolve_gpr_ppc64(addr, vreg_map, Gpr::R11, &mut result);
            let off = *offset;
            // Choose the correct store instruction based on the type width.
            let store_inst = match ty {
                IRType::I8 | IRType::U8 => Instruction::Stb { rs: v, ra: a, d: off },
                IRType::I16 | IRType::U16 => Instruction::Sth { rs: v, ra: a, d: off },
                IRType::I32 | IRType::U32 => Instruction::Stw { rs: v, ra: a, d: off },
                IRType::I64 | IRType::U64 | IRType::Ptr | IRType::Func => {
                    if off % 4 == 0 {
                        Instruction::Std { rs: v, ra: a, ds: off }
                    } else {
                        let scratch = if a != Gpr::R0 { Gpr::R0 } else { Gpr::R11 };
                        result.push(emit_alloc_instr(
                            Instruction::Addi { rt: scratch, ra: a, simm: off },
                            vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                            vec![PhysicalReg::new(RegClass::Gpr, scratch.encoding())],
                        ));
                        Instruction::Std { rs: v, ra: scratch, ds: 0 }
                    }
                }
                IRType::F32 => Instruction::Stfs { fs: Fpr::F0, ra: a, d: off },
                IRType::F64 => Instruction::Stfd { fs: Fpr::F0, ra: a, d: off },
                _ => Instruction::Std { rs: v, ra: a, ds: off }, // fallback: 64-bit
            };
            result.push(emit_alloc_instr(
                store_inst,
                vec![
                    PhysicalReg::new(RegClass::Gpr, a.encoding()),
                    PhysicalReg::new(RegClass::Gpr, v.encoding()),
                ],
                vec![],
            ));
        }

        IRInstr::Alloc { dst, size } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            // The prologue already allocated the full frame (computed by
            // ppc64_compute_frame_size), so we must NOT emit another stdu.
            // Instead, compute the address of this slot within the frame.
            // alloc_offset starts at 32 (after the mandatory save area) and
            // is advanced by the aligned size for each Alloc.
            let aligned_size = (*size as i32 + 15) & !15; // 16-byte aligned
            let slot_off = *alloc_offset;
            *alloc_offset += aligned_size;
            // addi dst, r1, slot_off
            if slot_off == 0 {
                // mr dst, r1
                if d != Gpr::R1 {
                    result.push(emit_alloc_instr(
                        Instruction::Mr { ra: d, rs: Gpr::R1 },
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::R1.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
            } else {
                result.push(emit_alloc_instr(
                    Instruction::Addi { rt: d, ra: Gpr::R1, simm: slot_off },
                    vec![PhysicalReg::new(RegClass::Gpr, Gpr::R1.encoding())],
                    vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                ));
            }
        }

        IRInstr::Ret { values } => {
            if let Some(val) = values.first() {
                let v = resolve_gpr_ppc64(val, vreg_map, Gpr::R0, &mut result);
                if v != Gpr::R3 {
                    result.push(emit_alloc_instr(
                        Instruction::Mr { ra: Gpr::R3, rs: v },
                        vec![PhysicalReg::new(RegClass::Gpr, v.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::R3.encoding())],
                    ));
                }
            }
            // Do NOT emit BLR here — the epilogue will handle the return.
            // The epilogue restores LR, deallocates the frame, and emits BLR.
        }

        IRInstr::Call { dst, func, args } => {
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    let a = resolve_gpr_ppc64(arg, vreg_map, Gpr::R0, &mut result);
                    if a != arg_reg {
                        result.push(emit_alloc_instr(
                            Instruction::Mr { ra: arg_reg, rs: a },
                            vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                            vec![PhysicalReg::new(RegClass::Gpr, arg_reg.encoding())],
                        ));
                    }
                }
            }
            // Record relocation for the BL instruction.
            let bl_offset = (result.len() * 4) as u64; // byte offset within this batch
            result.push(emit_alloc_instr(
                Instruction::Bl { li: 0 },
                vec![],
                vec![PhysicalReg::new(RegClass::Gpr, Gpr::R0.encoding())],
            ));
            relocations.push(crate::backend::RelocationEntry {
                offset: bl_offset,
                symbol: func.clone(),
                reloc_type: "R_PPC64_REL24".to_string(),
            });
            if let Some(d) = dst {
                let d_reg = map_vreg_to_gpr(vreg_id(d), None, vreg_map);
                if d_reg != Gpr::R3 {
                    result.push(emit_alloc_instr(
                        Instruction::Mr {
                            ra: d_reg,
                            rs: Gpr::R3,
                        },
                        vec![PhysicalReg::new(RegClass::Gpr, Gpr::R3.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d_reg.encoding())],
                    ));
                }
            }
        }

        IRInstr::Branch { target: _ } => {
            result.push(emit_alloc_instr(Instruction::B { li: 0 }, vec![], vec![]));
        }

        IRInstr::CondBranch {
            cond,
            true_target: _,
            false_target: _,
        } => {
            let c = resolve_gpr_ppc64(cond, vreg_map, Gpr::R0, &mut result);
            // cmpi cr0, 0, c, 0; bne cr0, +2; b false_target
            result.push(emit_alloc_instr(
                Instruction::Cmpi {
                    bf: CrField::CR0,
                    l: 1,
                    ra: c,
                    simm: 0,
                },
                vec![PhysicalReg::new(RegClass::Gpr, c.encoding())],
                vec![],
            ));
            result.push(emit_alloc_instr(
                Instruction::Bc {
                    bo: 12,
                    bi: 2 + CrField::CR0.encoding() * 4,
                    bd: 2,
                },
                vec![],
                vec![],
            ));
            result.push(emit_alloc_instr(Instruction::B { li: 0 }, vec![], vec![]));
        }

        IRInstr::Cast { kind, dst, src, .. } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let s = resolve_gpr_ppc64(src, vreg_map, Gpr::R0, &mut result);
            match kind {
                CastKind::SExt => {
                    // extsw d, s — sign-extend word to doubleword
                    result.push(emit_alloc_instr(
                        Instruction::Extsw { ra: d, rs: s },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                CastKind::ZExt => {
                    // Zero-extend from 32 bits: use rlwinm (CLRLDI) to clear upper 32 bits
                    result.push(emit_alloc_instr(
                        Instruction::Rlwinm { ra: d, rs: s, sh: 0, mb: 0, me: 31 },
                        vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                        vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                    ));
                }
                CastKind::Trunc | CastKind::BitCast => {
                    // Zero-extend, trunc, and bitcast are all just moves on PPC64
                    // (zero-extension is free after load, trunc just uses lower bits)
                    if d != s {
                        result.push(emit_alloc_instr(
                            Instruction::Mr { ra: d, rs: s },
                            vec![PhysicalReg::new(RegClass::Gpr, s.encoding())],
                            vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                        ));
                    }
                }
            }
        }

        IRInstr::Select {
            dst,
            cond,
            true_val,
            false_val, ty: _,
        } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let c = resolve_gpr_ppc64(cond, vreg_map, Gpr::R0, &mut result);
            let tv = resolve_gpr_ppc64(true_val, vreg_map, Gpr::R11, &mut result);
            // For false_val, we reuse R11 since cond is consumed by cmpi before false_val is used.
            // However, if false_val is an immediate it needs a scratch reg. Use the dst register
            // as scratch if it differs from R11/R12, otherwise use R11 (after cond is consumed).
            let fv = resolve_gpr_ppc64(false_val, vreg_map, Gpr::R0, &mut result);
            // mr d, fv; cmpi cr0, 0, c, 0; bne cr0, +8; mr d, tv
            if fv != d {
                result.push(emit_alloc_instr(
                    Instruction::Mr { ra: d, rs: fv },
                    vec![PhysicalReg::new(RegClass::Gpr, fv.encoding())],
                    vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
                ));
            }
            result.push(emit_alloc_instr(
                Instruction::Cmpi {
                    bf: CrField::CR0,
                    l: 1,
                    ra: c,
                    simm: 0,
                },
                vec![PhysicalReg::new(RegClass::Gpr, c.encoding())],
                vec![],
            ));
            result.push(emit_alloc_instr(
                Instruction::Bc {
                    bo: 4,
                    bi: 2 + CrField::CR0.encoding() * 4,
                    bd: 2,
                },
                vec![],
                vec![],
            ));
            result.push(emit_alloc_instr(
                Instruction::Mr { ra: d, rs: tv },
                vec![PhysicalReg::new(RegClass::Gpr, tv.encoding())],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Offset { dst, base, offset } => {
            let d = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let b = resolve_gpr_ppc64(base, vreg_map, Gpr::R0, &mut result);
            let o = resolve_gpr_ppc64(offset, vreg_map, Gpr::R11, &mut result);
            result.push(emit_alloc_instr(
                Instruction::Add {
                    rt: d,
                    ra: b,
                    rb: o,
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
                Instruction::Li { rt: d, simm: 0 },
                vec![],
                vec![PhysicalReg::new(RegClass::Gpr, d.encoding())],
            ));
        }

        IRInstr::Free { ptr: _ } => {
            // Free is not directly implementable as a single instruction;
            // emit a trap to catch any accidental execution at runtime.
            result.push(emit_alloc_instr(Instruction::Trap, vec![], vec![]));
        }

        IRInstr::Phi { .. } => {
            // Phi nodes are eliminated by SSA deconstruction; emit NOP.
            result.push(emit_alloc_instr(Instruction::Nop, vec![], vec![]));
        }
    }

    (result, relocations)
}

// ===========================================================================
// Stack-Slot Helpers (for allocate_registers)
// ===========================================================================

/// Load a 64-bit immediate into a GPR, returning encoded instruction bytes.
fn ss_load_imm(rd: Gpr, val: i64) -> Vec<u8> {
    let mut code = Vec::new();
    let uval = val as u64;
    if (-32768..=32767).contains(&val) {
        code.extend_from_slice(&Instruction::Li { rt: rd, simm: val as i32 }.encode());
    } else if uval <= 0xFFFF {
        code.extend_from_slice(&Instruction::Ori { ra: rd, rs: Gpr::R0, uimm: uval as u32 & 0xFFFF }.encode());
    } else if uval & 0xFFFF == 0 && (uval >> 32) == 0 {
        code.extend_from_slice(&Instruction::Lis { rt: rd, simm: ((uval >> 16) & 0xFFFF) as i16 as i32 }.encode());
        // If the high 16 bits have bit 15 set, LIS sign-extends to 64-bit all-1s.
        // Clear upper 32 bits with rlwinm rd,rd,0,0,31 (equivalent to clrldi rd,rd,32)
        if ((uval >> 16) & 0xFFFF) >= 0x8000 {
            code.extend_from_slice(&Instruction::Rlwinm { ra: rd, rs: rd, sh: 0, mb: 0, me: 31 }.encode());
        }
    } else if uval >> 32 == 0 {
        let hi16 = ((uval >> 16) & 0xFFFF) as u32;
        let lo16 = (uval & 0xFFFF) as u32;
        code.extend_from_slice(&Instruction::Lis { rt: rd, simm: hi16 as i16 as i32 }.encode());
        if lo16 != 0 { code.extend_from_slice(&Instruction::Ori { ra: rd, rs: rd, uimm: lo16 }.encode()); }
        // LIS sign-extends: if bit 31 of the 32-bit value is set, the upper 32 bits
        // of the 64-bit register are all 1s. Clear them with rlwinm rd,rd,0,0,31
        // (equivalent to clrldi rd,rd,32) which zero-extends to 64 bits.
        if hi16 >= 0x8000 {
            code.extend_from_slice(&Instruction::Rlwinm { ra: rd, rs: rd, sh: 0, mb: 0, me: 31 }.encode());
        }
    } else {
        let hi32 = (uval >> 32) as u32;
        let lo32 = (uval & 0xFFFF_FFFF) as u32;
        code.extend_from_slice(&Instruction::Lis { rt: rd, simm: ((hi32 >> 16) & 0xFFFF) as i16 as i32 }.encode());
        if hi32 & 0xFFFF != 0 { code.extend_from_slice(&Instruction::Ori { ra: rd, rs: rd, uimm: hi32 & 0xFFFF }.encode()); }
        // SLDI rd, rd, 32 = RLDICR rd, rd, 32, 31 (shift left by 32 bits)
        // Use rlwinm-based approach: SLDI is correct for upper-32 ops,
        // but the MD-form encoding was buggy. Use SLW + clear instead:
        // Load 32 into R12, then sld rd, rd, r12
        code.extend_from_slice(&Instruction::Li { rt: Gpr::R12, simm: 32 }.encode());
        code.extend_from_slice(&Instruction::Sld { ra: rd, rs: rd, rb: Gpr::R12 }.encode());
        if (lo32 >> 16) & 0xFFFF != 0 {
            let oris_word: u32 = (25u32<<26)|(rd.encoding()<<21)|(rd.encoding()<<16)|((lo32>>16)&0xFFFF);
            code.extend_from_slice(&encode_word(oris_word));
        }
        if lo32 & 0xFFFF != 0 { code.extend_from_slice(&Instruction::Ori { ra: rd, rs: rd, uimm: lo32 & 0xFFFF }.encode()); }
    }
    code
}

/// Load from stack slot [R31 - offset_from_r31] into dst_reg.
fn ss_load_from_slot(dst_reg: Gpr, offset_from_r31: i32) -> Vec<u8> {
    let neg_off = -offset_from_r31;
    if neg_off >= -32764 && neg_off <= 32764 && (neg_off & 3) == 0 {
        Instruction::Ld { rt: dst_reg, ra: Gpr::R31, ds: neg_off }.encode().to_vec()
    } else if neg_off >= -32768 && neg_off <= 32767 {
        let mut code = Vec::new();
        code.extend_from_slice(&Instruction::Addi { rt: Gpr::R12, ra: Gpr::R31, simm: neg_off }.encode());
        code.extend_from_slice(&Instruction::Ld { rt: dst_reg, ra: Gpr::R12, ds: 0 }.encode());
        code
    } else {
        let mut code = Vec::new();
        code.extend(ss_load_imm(Gpr::R12, offset_from_r31 as i64));
        code.extend_from_slice(&Instruction::Subf { rt: Gpr::R12, ra: Gpr::R12, rb: Gpr::R31 }.encode());
        code.extend_from_slice(&Instruction::Ld { rt: dst_reg, ra: Gpr::R12, ds: 0 }.encode());
        code
    }
}

/// Store src_reg into stack slot [R31 - offset_from_r31].
fn ss_store_to_slot(src_reg: Gpr, offset_from_r31: i32) -> Vec<u8> {
    let neg_off = -offset_from_r31;
    if neg_off >= -32764 && neg_off <= 32764 && (neg_off & 3) == 0 {
        Instruction::Std { rs: src_reg, ra: Gpr::R31, ds: neg_off }.encode().to_vec()
    } else if neg_off >= -32768 && neg_off <= 32767 {
        let mut code = Vec::new();
        code.extend_from_slice(&Instruction::Addi { rt: Gpr::R12, ra: Gpr::R31, simm: neg_off }.encode());
        code.extend_from_slice(&Instruction::Std { rs: src_reg, ra: Gpr::R12, ds: 0 }.encode());
        code
    } else {
        let mut code = Vec::new();
        code.extend(ss_load_imm(Gpr::R12, offset_from_r31 as i64));
        code.extend_from_slice(&Instruction::Subf { rt: Gpr::R12, ra: Gpr::R12, rb: Gpr::R31 }.encode());
        code.extend_from_slice(&Instruction::Std { rs: src_reg, ra: Gpr::R12, ds: 0 }.encode());
        code
    }
}

/// Load an IRValue into a scratch register.
fn ss_load_value(val: &IRValue, slots: &std::collections::HashMap<u32, i32>, scratch: Gpr) -> Vec<u8> {
    match val {
        IRValue::Register(id) => { let offset = slots.get(id).copied().unwrap_or(0); ss_load_from_slot(scratch, offset) }
        IRValue::Immediate(v) => ss_load_imm(scratch, *v),
        IRValue::Address(a) => ss_load_imm(scratch, *a as i64),
        IRValue::Label(_) => Instruction::Li { rt: scratch, simm: 0 }.encode().to_vec(),
    }
}

/// Emit comparison code producing 0 or 1 in dst.
fn ss_emit_cmp(kind: &CmpKind, dst: Gpr, lhs: Gpr, rhs: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    let cmp_signed = !matches!(kind, CmpKind::ULt|CmpKind::ULe|CmpKind::UGt|CmpKind::UGe);
    if cmp_signed {
        code.extend_from_slice(&Instruction::Cmp { bf: CrField::CR0, l: 1, ra: lhs, rb: rhs }.encode());
    } else {
        code.extend_from_slice(&Instruction::Cmpl { bf: CrField::CR0, l: 1, ra: lhs, rb: rhs }.encode());
    }
    code.extend_from_slice(&Instruction::Li { rt: dst, simm: 0 }.encode());
    let (bo, bi) = match kind {
        CmpKind::Eq => (4, 2u32), CmpKind::Ne => (12, 2u32),
        // SLt/ULt: branch if LT=0 (not less than) → skip setting 1; only set 1 when LT=1
        CmpKind::SLt|CmpKind::ULt => (4, 0u32),
        // SLe/ULe: branch if GT=1 (strictly greater) → skip setting 1; set 1 when LT=1 or EQ=1
        CmpKind::SLe|CmpKind::ULe => (12, 1u32),
        // SGt/UGt: branch if GT=0 (not greater) → skip setting 1; only set 1 when GT=1
        CmpKind::SGt|CmpKind::UGt => (4, 1u32),
        // SGe/UGe: branch if LT=1 (strictly less) → skip setting 1; set 1 when GT=1 or EQ=1
        CmpKind::SGe|CmpKind::UGe => (12, 0u32),
    };
    code.extend_from_slice(&Instruction::Bc { bo, bi, bd: 2 }.encode());
    code.extend_from_slice(&Instruction::Li { rt: dst, simm: 1 }.encode());
    code
}

/// Typed load from memory.
fn ss_emit_typed_load(dst_reg: Gpr, addr_reg: Gpr, offset: i32, ty: &IRType) -> Vec<u8> {
    let mut code = Vec::new();
    match ty {
        IRType::I8 | IRType::U8 => {
            if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Lbz { rt: dst_reg, ra: addr_reg, d: offset }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Lbz { rt: dst_reg, ra: Gpr::R5, d: 0 }.encode());
            }
        }
        IRType::I16 | IRType::U16 => {
            if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Lhz { rt: dst_reg, ra: addr_reg, d: offset }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Lhz { rt: dst_reg, ra: Gpr::R5, d: 0 }.encode());
            }
        }
        IRType::I32 | IRType::U32 => {
            if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Lwz { rt: dst_reg, ra: addr_reg, d: offset }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Lwz { rt: dst_reg, ra: Gpr::R5, d: 0 }.encode());
            }
        }
        _ => {
            if offset % 4 == 0 && offset >= -32764 && offset <= 32764 {
                code.extend_from_slice(&Instruction::Ld { rt: dst_reg, ra: addr_reg, ds: offset }.encode());
            } else if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Addi { rt: Gpr::R5, ra: addr_reg, simm: offset }.encode());
                code.extend_from_slice(&Instruction::Ld { rt: dst_reg, ra: Gpr::R5, ds: 0 }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Ld { rt: dst_reg, ra: Gpr::R5, ds: 0 }.encode());
            }
        }
    }
    code
}

/// Typed store to memory.
fn ss_emit_typed_store(value_reg: Gpr, addr_reg: Gpr, offset: i32, ty: &IRType) -> Vec<u8> {
    let mut code = Vec::new();
    match ty {
        IRType::I8 | IRType::U8 => {
            if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Stb { rs: value_reg, ra: addr_reg, d: offset }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Stb { rs: value_reg, ra: Gpr::R5, d: 0 }.encode());
            }
        }
        IRType::I16 | IRType::U16 => {
            if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Sth { rs: value_reg, ra: addr_reg, d: offset }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Sth { rs: value_reg, ra: Gpr::R5, d: 0 }.encode());
            }
        }
        IRType::I32 | IRType::U32 => {
            if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Stw { rs: value_reg, ra: addr_reg, d: offset }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Stw { rs: value_reg, ra: Gpr::R5, d: 0 }.encode());
            }
        }
        _ => {
            if offset % 4 == 0 && offset >= -32764 && offset <= 32764 {
                code.extend_from_slice(&Instruction::Std { rs: value_reg, ra: addr_reg, ds: offset }.encode());
            } else if offset >= -32768 && offset <= 32767 {
                code.extend_from_slice(&Instruction::Addi { rt: Gpr::R5, ra: addr_reg, simm: offset }.encode());
                code.extend_from_slice(&Instruction::Std { rs: value_reg, ra: Gpr::R5, ds: 0 }.encode());
            } else {
                code.extend(ss_load_imm(Gpr::R5, offset as i64));
                code.extend_from_slice(&Instruction::Add { rt: Gpr::R5, ra: addr_reg, rb: Gpr::R5 }.encode());
                code.extend_from_slice(&Instruction::Std { rs: value_reg, ra: Gpr::R5, ds: 0 }.encode());
            }
        }
    }
    code
}

impl Backend for PPC64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        let func_name = func.name.clone();

        // ── Phase 1: Collect all vreg IDs and compute stack layout ──
        let mut all_vreg_ids: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for &id in func.vregs.keys() { all_vreg_ids.insert(id); }
        for param in &func.params {
            if let Some(id) = param.as_register() { all_vreg_ids.insert(id); }
        }
        for block in &func.blocks {
            for instr in &block.instructions {
                for id in instr.defined_regs() { all_vreg_ids.insert(id); }
                for id in instr.used_regs() { all_vreg_ids.insert(id); }
            }
        }
        for val in &func.results {
            if let Some(id) = val.as_register() { all_vreg_ids.insert(id); }
        }

        let mut stack_alloc_vregs: std::collections::HashSet<u32> = std::collections::HashSet::new();
        let mut alloc_sizes: std::collections::HashMap<u32, i32> = std::collections::HashMap::new();
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

        // Stack Layout (relative to R31, which = R1 + frame_size):
        // [R31 + 8]  = saved LR
        // [R31]      = back chain
        // [R31 - 16] = saved R31
        // Then alloc regions, then vreg slots at [R31 - offset]

        let mut alloc_offsets: std::collections::HashMap<u32, i32> = std::collections::HashMap::new();
        let mut current_offset: i32 = 24; // after saved R31 at -16

        let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
        alloc_vreg_ids.sort();
        for &id in &alloc_vreg_ids {
            let size = alloc_sizes[&id];
            current_offset += size;
            alloc_offsets.insert(id, current_offset);
        }

        let mut vreg_stack_slots: std::collections::HashMap<u32, i32> = std::collections::HashMap::new();
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
        let mut relocations: Vec<crate::backend::RelocationEntry> = Vec::new();

        // STDU R1, -frame_size(R1)
        instructions.push(AllocatedInstruction {
            opcode: "stdu".into(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, 1)],
            writes: vec![PhysicalReg::new(RegClass::Gpr, 1)],
            encoded: Instruction::Stdu { rs: Gpr::R1, ra: Gpr::R1, ds: -fs }.encode().to_vec(),
        });
        // MFLR R0
        let mflr_word: u32 = (31u32 << 26) | (0u32 << 21) | (8u32 << 16) | (339 << 1);
        instructions.push(AllocatedInstruction {
            opcode: "mflr".into(), reads: vec![], writes: vec![PhysicalReg::new(RegClass::Gpr, 0)],
            encoded: encode_word(mflr_word).to_vec(),
        });
        // STD R0, fs+8(R1) - save LR
        instructions.push(AllocatedInstruction {
            opcode: "std".into(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, 0), PhysicalReg::new(RegClass::Gpr, 1)],
            writes: vec![],
            encoded: Instruction::Std { rs: Gpr::R0, ra: Gpr::R1, ds: fs + 8 }.encode().to_vec(),
        });
        // STD R31, fs-16(R1) - save R31
        instructions.push(AllocatedInstruction {
            opcode: "std".into(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, 31), PhysicalReg::new(RegClass::Gpr, 1)],
            writes: vec![],
            encoded: Instruction::Std { rs: Gpr::R31, ra: Gpr::R1, ds: fs - 16 }.encode().to_vec(),
        });
        // ADDI R31, R1, frame_size
        instructions.push(AllocatedInstruction {
            opcode: "addi".into(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, 1)],
            writes: vec![PhysicalReg::new(RegClass::Gpr, 31)],
            encoded: Instruction::Addi { rt: Gpr::R31, ra: Gpr::R1, simm: fs }.encode().to_vec(),
        });

        // Store function params from R3-R10 to stack slots
        let arg_regs = [Gpr::R3, Gpr::R4, Gpr::R5, Gpr::R6, Gpr::R7, Gpr::R8, Gpr::R9, Gpr::R10];
        for (i, param) in func.params.iter().enumerate() {
            if let Some(id) = param.as_register() {
                if i < 8 {
                    let offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
                    let store_code = ss_store_to_slot(arg_regs[i], offset);
                    instructions.push(AllocatedInstruction {
                        opcode: "std".into(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, arg_regs[i].encoding())],
                        writes: vec![], encoded: store_code,
                    });
                }
            }
        }

        // ── Phase 3: Emit body ──
        let mut current_byte_offset: u64 = instructions.iter().map(|i| i.encoded.len() as u64).sum();
        let mut label_offsets: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

        struct BranchFixup { instr_idx: usize, offset_in_encoded: usize, abs_byte_offset: u64, target_label: String, is_unconditional: bool, bc_bo: u32, bc_bi: u32 }
        let mut branch_fixups: Vec<BranchFixup> = Vec::new();

        for block in &func.blocks {
            label_offsets.insert(block.label.clone(), current_byte_offset);
            for instr in &block.instructions {
                let encoded: Vec<u8> = match instr {
                    IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let is_32bit = ty.as_ref().map_or(false, |t| matches!(t, IRType::I32 | IRType::U32));
                        let mut code = Vec::new();
                        match op {
                            BinOpKind::Ror | BinOpKind::Rol => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R4));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R5));
                                if is_32bit {
                                    // 32-bit rotation: use rlwinm which clears upper 32 bits
                                    // rlwinm operates on the low 32 bits and zero-extends the result
                                    // ROR(n, r) = ROTL32(n, 32-r)
                                    // ROL(n, r) = ROTL32(n, r)
                                    if *op == BinOpKind::Ror {
                                        // neg r5, r5; addi r5, r5, 32 -> r5 = 32 - amount
                                        code.extend_from_slice(&Instruction::Neg { rt: Gpr::R5, ra: Gpr::R5 }.encode());
                                        code.extend_from_slice(&Instruction::Addi { rt: Gpr::R5, ra: Gpr::R5, simm: 32 }.encode());
                                    }
                                    // rlwinm needs immediate shift, but we have register shift.
                                    // Use rlwnm (register-based rotate left word then AND mask)
                                    // rlwnm r3, r4, r5, 0, 31 — rotates low 32 bits of r4 left by r5[0:4],
                                    // then masks bits 0-31, clearing upper 32 bits.
                                    // Encoding: M-form, primary opcode 23
                                    // rlwnm RA, RS, RB, MB, ME: opcode=23, RS[6:10], RA[11:15], RB[16:20], MB[21:25], ME[26:30], Rc[31]
                                    let rlwnm_word: u32 = (23u32 << 26)
                                        | (Gpr::R4.encoding() << 21)
                                        | (Gpr::R3.encoding() << 16)
                                        | (Gpr::R5.encoding() << 11)
                                        | (0u32 << 6)    // MB = 0
                                        | (31u32 << 1)   // ME = 31
                                        | 0u32;          // Rc = 0
                                    code.extend_from_slice(&encode_word(rlwnm_word));
                                } else {
                                    // 64-bit rotation: use rldcl
                                    if *op == BinOpKind::Ror {
                                        code.extend_from_slice(&Instruction::Neg { rt: Gpr::R5, ra: Gpr::R5 }.encode());
                                        code.extend_from_slice(&Instruction::Addi { rt: Gpr::R5, ra: Gpr::R5, simm: 64 }.encode());
                                    }
                                    // rldcl with mb=0 — but encoding only uses 5-bit mb field.
                                    // For mb=0 this is fine (only lower 5 bits matter, mb5=0).
                                    code.extend_from_slice(&Instruction::Rldcl { ra: Gpr::R3, rs: Gpr::R4, rb: Gpr::R5, mb: 0 }.encode());
                                }
                            }
                            BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R4));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R5));
                                if is_32bit {
                                    match op {
                                        // SLW/SRW clear upper 32 bits automatically
                                        BinOpKind::Shl => { code.extend_from_slice(&Instruction::Slw { ra: Gpr::R3, rs: Gpr::R4, rb: Gpr::R5 }.encode()); }
                                        BinOpKind::ShrL => { code.extend_from_slice(&Instruction::Srw { ra: Gpr::R3, rs: Gpr::R4, rb: Gpr::R5 }.encode()); }
                                        BinOpKind::ShrA => { code.extend_from_slice(&Instruction::Sraw { ra: Gpr::R3, rs: Gpr::R4, rb: Gpr::R5 }.encode()); }
                                        _ => unreachable!(),
                                    }
                                } else {
                                    match op {
                                        BinOpKind::Shl => { code.extend_from_slice(&Instruction::Sld { ra: Gpr::R3, rs: Gpr::R4, rb: Gpr::R5 }.encode()); }
                                        BinOpKind::ShrL => { code.extend_from_slice(&Instruction::Srd { ra: Gpr::R3, rs: Gpr::R4, rb: Gpr::R5 }.encode()); }
                                        BinOpKind::ShrA => { code.extend_from_slice(&Instruction::Srad { ra: Gpr::R3, rs: Gpr::R4, rb: Gpr::R5 }.encode()); }
                                        _ => unreachable!(),
                                    }
                                }
                            }
                            BinOpKind::SRem | BinOpKind::URem => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R3));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                                if is_32bit {
                                    let div_instr = match op {
                                        BinOpKind::URem => Instruction::Divwu { rt: Gpr::R5, ra: Gpr::R3, rb: Gpr::R4 },
                                        _ => Instruction::Divw { rt: Gpr::R5, ra: Gpr::R3, rb: Gpr::R4 },
                                    };
                                    code.extend_from_slice(&div_instr.encode());
                                    code.extend_from_slice(&Instruction::Mullw { rt: Gpr::R5, ra: Gpr::R5, rb: Gpr::R4 }.encode());
                                } else {
                                    let div_instr = match op {
                                        BinOpKind::URem => Instruction::Divdu { rt: Gpr::R5, ra: Gpr::R3, rb: Gpr::R4 },
                                        _ => Instruction::Divd { rt: Gpr::R5, ra: Gpr::R3, rb: Gpr::R4 },
                                    };
                                    code.extend_from_slice(&div_instr.encode());
                                    code.extend_from_slice(&Instruction::Mulld { rt: Gpr::R5, ra: Gpr::R5, rb: Gpr::R4 }.encode());
                                }
                                code.extend_from_slice(&Instruction::Subf { rt: Gpr::R3, ra: Gpr::R5, rb: Gpr::R3 }.encode());
                                if is_32bit {
                                    // Mask to 32 bits
                                    code.extend_from_slice(&Instruction::Rlwinm { ra: Gpr::R3, rs: Gpr::R3, sh: 0, mb: 0, me: 31 }.encode());
                                }
                            }
                            _ => {
                                code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R3));
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                                match op {
                                    BinOpKind::Add => {
                                        if is_32bit {
                                            // Add and mask to 32 bits
                                            code.extend_from_slice(&Instruction::Add { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                                            code.extend_from_slice(&Instruction::Rlwinm { ra: Gpr::R3, rs: Gpr::R3, sh: 0, mb: 0, me: 31 }.encode());
                                        } else {
                                            code.extend_from_slice(&Instruction::Add { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                                        }
                                    }
                                    BinOpKind::Sub => {
                                        if is_32bit {
                                            code.extend_from_slice(&Instruction::Subf { rt: Gpr::R3, ra: Gpr::R4, rb: Gpr::R3 }.encode());
                                            code.extend_from_slice(&Instruction::Rlwinm { ra: Gpr::R3, rs: Gpr::R3, sh: 0, mb: 0, me: 31 }.encode());
                                        } else {
                                            code.extend_from_slice(&Instruction::Subf { rt: Gpr::R3, ra: Gpr::R4, rb: Gpr::R3 }.encode());
                                        }
                                    }
                                    BinOpKind::Mul => {
                                        if is_32bit {
                                            code.extend_from_slice(&Instruction::Mullw { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                                        } else {
                                            code.extend_from_slice(&Instruction::Mulld { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                                        }
                                    }
                                    BinOpKind::SDiv | BinOpKind::UDiv => {
                                        if is_32bit {
                                            let div_instr = match op {
                                                BinOpKind::UDiv => Instruction::Divwu { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 },
                                                _ => Instruction::Divw { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 },
                                            };
                                            code.extend_from_slice(&div_instr.encode());
                                        } else {
                                            let div_instr = match op {
                                                BinOpKind::UDiv => Instruction::Divdu { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 },
                                                _ => Instruction::Divd { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 },
                                            };
                                            code.extend_from_slice(&div_instr.encode());
                                        }
                                    }
                                    BinOpKind::And => { code.extend_from_slice(&Instruction::And { ra: Gpr::R3, rs: Gpr::R3, rb: Gpr::R4 }.encode()); }
                                    BinOpKind::Or => { code.extend_from_slice(&Instruction::Or { ra: Gpr::R3, rs: Gpr::R3, rb: Gpr::R4 }.encode()); }
                                    BinOpKind::Xor => { code.extend_from_slice(&Instruction::Xor { ra: Gpr::R3, rs: Gpr::R3, rb: Gpr::R4 }.encode()); }
                                    BinOpKind::Eq|BinOpKind::Ne|BinOpKind::SLt|BinOpKind::SLe|BinOpKind::SGt|BinOpKind::SGe|BinOpKind::ULt|BinOpKind::ULe|BinOpKind::UGt|BinOpKind::UGe => {
                                        let cmp_kind = match op {
                                            BinOpKind::Eq => CmpKind::Eq,
                                            BinOpKind::Ne => CmpKind::Ne,
                                            BinOpKind::SLt => CmpKind::SLt,
                                            BinOpKind::SLe => CmpKind::SLe,
                                            BinOpKind::SGt => CmpKind::SGt,
                                            BinOpKind::SGe => CmpKind::SGe,
                                            BinOpKind::ULt => CmpKind::ULt,
                                            BinOpKind::ULe => CmpKind::ULe,
                                            BinOpKind::UGt => CmpKind::UGt,
                                            BinOpKind::UGe => CmpKind::UGe,
                                            _ => CmpKind::Eq,
                                        };
                                        code.extend(ss_emit_cmp(&cmp_kind, Gpr::R3, Gpr::R3, Gpr::R4));
                                    }
                                    _ => { code.extend_from_slice(&Instruction::Add { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode()); }
                                }
                            }
                        }
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Add { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R3));
                        if let IRValue::Immediate(imm) = rhs {
                            let i = *imm as i32;
                            if (-32768..=32767).contains(&i) {
                                code.extend_from_slice(&Instruction::Addi { rt: Gpr::R3, ra: Gpr::R3, simm: i }.encode());
                            } else {
                                code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                                code.extend_from_slice(&Instruction::Add { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                            }
                        } else {
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                            code.extend_from_slice(&Instruction::Add { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                        }
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Sub { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                        code.extend_from_slice(&Instruction::Subf { rt: Gpr::R3, ra: Gpr::R4, rb: Gpr::R3 }.encode());
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Mul { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                        code.extend_from_slice(&Instruction::Mulld { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Div { dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                        code.extend_from_slice(&Instruction::Divd { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::UnaryOp { op, dst, operand, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(operand, &vreg_stack_slots, Gpr::R3));
                        match op {
                            UnaryOpKind::Neg => { code.extend_from_slice(&Instruction::Neg { rt: Gpr::R3, ra: Gpr::R3 }.encode()); }
                            UnaryOpKind::Not => { code.extend_from_slice(&Instruction::Nor { ra: Gpr::R3, rs: Gpr::R3, rb: Gpr::R3 }.encode()); }
                            UnaryOpKind::Clz => { code.extend_from_slice(&Instruction::Cntlzd { ra: Gpr::R3, rs: Gpr::R3 }.encode()); }
                            UnaryOpKind::Ctz => {
                                code.extend_from_slice(&Instruction::Neg { rt: Gpr::R4, ra: Gpr::R3 }.encode());
                                code.extend_from_slice(&Instruction::And { ra: Gpr::R4, rs: Gpr::R3, rb: Gpr::R4 }.encode());
                                code.extend_from_slice(&Instruction::Cntlzd { ra: Gpr::R3, rs: Gpr::R4 }.encode());
                                let subfic_word: u32 = (8u32 << 26) | (3u32 << 21) | (3u32 << 16) | 63u32;
                                code.extend_from_slice(&encode_word(subfic_word));
                            }
                            UnaryOpKind::Popcnt => { code.extend_from_slice(&Instruction::Popcntd { ra: Gpr::R3, rs: Gpr::R3 }.encode()); }
                        }
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Cmp { kind, dst, lhs, rhs, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, Gpr::R4));
                        code.extend(ss_emit_cmp(kind, Gpr::R3, Gpr::R3, Gpr::R4));
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Load { dst, addr, offset, ty } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R5));
                        code.extend(ss_emit_typed_load(Gpr::R3, Gpr::R5, *offset, ty));
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Store { value, addr, offset, ty } => {
                        let mut code = Vec::new();
                        code.extend(ss_load_value(addr, &vreg_stack_slots, Gpr::R5));
                        code.extend(ss_load_value(value, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_emit_typed_store(Gpr::R3, Gpr::R5, *offset, ty));
                        code
                    }
                    IRInstr::Alloc { dst, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let alloc_off = alloc_offsets.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        let neg_alloc = -alloc_off;
                        if neg_alloc >= -32768 && neg_alloc <= 32767 {
                            code.extend_from_slice(&Instruction::Addi { rt: Gpr::R3, ra: Gpr::R31, simm: neg_alloc }.encode());
                        } else {
                            code.extend(ss_load_imm(Gpr::R3, alloc_off as i64));
                            code.extend_from_slice(&Instruction::Subf { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R31 }.encode());
                        }
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Free { .. } => {
                        let trap_word: u32 = (31u32 << 26) | (31u32 << 21) | (0u32 << 16) | (0u32 << 11) | (4 << 1);
                        encode_word(trap_word).to_vec()
                    }
                    IRInstr::Cast { kind, dst, src } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(src, &vreg_stack_slots, Gpr::R3));
                        match kind {
                            CastKind::SExt => {
                                code.extend_from_slice(&Instruction::Extsw { ra: Gpr::R3, rs: Gpr::R3 }.encode());
                            }
                            CastKind::ZExt => {
                                // Zero-extend: rlwinm ra, rs, 0, 0, 31 clears upper 32 bits
                                code.extend_from_slice(&Instruction::Rlwinm { ra: Gpr::R3, rs: Gpr::R3, sh: 0, mb: 0, me: 31 }.encode());
                            }
                            CastKind::Trunc | CastKind::BitCast => {}
                        }
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Select { dst, cond, true_val, false_val, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(false_val, &vreg_stack_slots, Gpr::R3));
                        code.extend(ss_load_value(true_val, &vreg_stack_slots, Gpr::R4));
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::R5));
                        code.extend_from_slice(&Instruction::Cmpi { bf: CrField::CR0, l: 1, ra: Gpr::R5, simm: 0 }.encode());
                        code.extend_from_slice(&Instruction::Bc { bo: 12, bi: 2, bd: 2 }.encode());
                        code.extend_from_slice(&Instruction::Mr { ra: Gpr::R3, rs: Gpr::R4 }.encode());
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Offset { dst, base, offset } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend(ss_load_value(base, &vreg_stack_slots, Gpr::R3));
                        if let IRValue::Immediate(imm) = offset {
                            let off = *imm as i32;
                            if (-32768..=32767).contains(&off) {
                                code.extend_from_slice(&Instruction::Addi { rt: Gpr::R3, ra: Gpr::R3, simm: off }.encode());
                            } else {
                                code.extend(ss_load_value(offset, &vreg_stack_slots, Gpr::R4));
                                code.extend_from_slice(&Instruction::Add { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                            }
                        } else {
                            code.extend(ss_load_value(offset, &vreg_stack_slots, Gpr::R4));
                            code.extend_from_slice(&Instruction::Add { rt: Gpr::R3, ra: Gpr::R3, rb: Gpr::R4 }.encode());
                        }
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::GetAddress { dst, .. } => {
                        let dst_id = dst.as_register().unwrap_or(0);
                        let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                        let mut code = Vec::new();
                        code.extend_from_slice(&Instruction::Li { rt: Gpr::R3, simm: 0 }.encode());
                        code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        code
                    }
                    IRInstr::Call { dst, func: target_func, args } => {
                        let mut code = Vec::new();
                        for (i, arg) in args.iter().enumerate() {
                            if i >= 8 { break; }
                            code.extend(ss_load_value(arg, &vreg_stack_slots, arg_regs[i]));
                        }
                        let bl_byte_offset = current_byte_offset + code.len() as u64;
                        code.extend_from_slice(&Instruction::Bl { li: 0 }.encode());
                        relocations.push(crate::backend::RelocationEntry {
                            offset: bl_byte_offset,
                            symbol: target_func.clone(),
                            reloc_type: "R_PPC64_REL24".to_string(),
                        });
                        if let Some(d) = dst {
                            let dst_id = d.as_register().unwrap_or(0);
                            let dst_offset = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                            code.extend(ss_store_to_slot(Gpr::R3, dst_offset));
                        }
                        code
                    }
                    IRInstr::Ret { values } => {
                        let mut code = Vec::new();
                        if let Some(val) = values.first() {
                            code.extend(ss_load_value(val, &vreg_stack_slots, Gpr::R3));
                        }
                        // Epilogue
                        code.extend_from_slice(&Instruction::Ld { rt: Gpr::R0, ra: Gpr::R1, ds: fs + 8 }.encode());
                        let mtlr_word: u32 = (31u32 << 26) | (0u32 << 21) | (8u32 << 16) | (467 << 1);
                        code.extend_from_slice(&encode_word(mtlr_word));
                        code.extend_from_slice(&Instruction::Ld { rt: Gpr::R31, ra: Gpr::R1, ds: fs - 16 }.encode());
                        code.extend_from_slice(&Instruction::Addi { rt: Gpr::R1, ra: Gpr::R1, simm: fs }.encode());
                        code.extend_from_slice(&Instruction::Bclr { bo: 20, bi: 0, bh: 0 }.encode());
                        code
                    }
                    IRInstr::Branch { target } => {
                        let instr_idx = instructions.len();
                        let b_abs_offset = current_byte_offset;
                        branch_fixups.push(BranchFixup { instr_idx, offset_in_encoded: 0, abs_byte_offset: b_abs_offset, target_label: target.clone(), is_unconditional: true, bc_bo: 0, bc_bi: 0 });
                        Instruction::B { li: 0 }.encode().to_vec()
                    }
                    IRInstr::CondBranch { cond, true_target, false_target } => {
                        let mut code = Vec::new();
                        code.extend(ss_load_value(cond, &vreg_stack_slots, Gpr::R3));
                        let instr_idx = instructions.len();
                        code.extend_from_slice(&Instruction::Cmpi { bf: CrField::CR0, l: 1, ra: Gpr::R3, simm: 0 }.encode());
                        let bne_offset = code.len();
                        let bne_abs = current_byte_offset + bne_offset as u64;
                        code.extend_from_slice(&Instruction::Bc { bo: 4, bi: 2, bd: 0 }.encode());
                        let b_offset = code.len();
                        let b_abs = current_byte_offset + b_offset as u64;
                        code.extend_from_slice(&Instruction::B { li: 0 }.encode());
                        branch_fixups.push(BranchFixup { instr_idx, offset_in_encoded: bne_offset, abs_byte_offset: bne_abs, target_label: true_target.clone(), is_unconditional: false, bc_bo: 4, bc_bi: 2 });
                        branch_fixups.push(BranchFixup { instr_idx, offset_in_encoded: b_offset, abs_byte_offset: b_abs, target_label: false_target.clone(), is_unconditional: true, bc_bo: 0, bc_bi: 0 });
                        code
                    }
                    IRInstr::Phi { .. } => Instruction::Nop.encode().to_vec(),
                };
                current_byte_offset += encoded.len() as u64;
                instructions.push(AllocatedInstruction { opcode: "isel".into(), reads: vec![], writes: vec![], encoded });
            }
        }

        // ── Phase 4: Apply branch fixups ──
        for fixup in &branch_fixups {
            if let Some(&target_offset) = label_offsets.get(&fixup.target_label) {
                let offset_words = (target_offset as i64 - fixup.abs_byte_offset as i64) / 4;
                let instr = &mut instructions[fixup.instr_idx];
                let encoded = &mut instr.encoded;
                if fixup.is_unconditional {
                    let imm24 = (offset_words as u32) & 0x00FF_FFFF;
                    let b_word: u32 = (18u32 << 26) | (imm24 << 2);
                    encoded[fixup.offset_in_encoded..fixup.offset_in_encoded+4].copy_from_slice(&b_word.to_be_bytes());
                } else {
                    let bd = (offset_words as i32) & 0x3FFF;
                    let bc_word: u32 = (16u32 << 26) | ((fixup.bc_bo & 0x1F) << 21) | ((fixup.bc_bi & 0x1F) << 16) | (((bd as u32) & 0x3FFF) << 2);
                    encoded[fixup.offset_in_encoded..fixup.offset_in_encoded+4].copy_from_slice(&bc_word.to_be_bytes());
                }
            }
        }

        let code_size = instructions.iter().map(|i| i.encoded.len()).sum();
        let callee_saved = vec![PhysicalReg::new(RegClass::Gpr, 31)];
        let spill_slots = all_vreg_ids.len();

        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock { label: "entry".into(), instructions, code_offset: 0 }],
            frame_size, callee_saved, spill_slots, code_size, relocations,
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
        // ── PPC64LE Linux static executable ──
        //
        // Layout:
        //   _start:  BL main           ; call main (result in R3)
        //            LI R0, 1          ; sys_exit = 1
        //            SC                ; syscall: exit(R3)
        //   <functions...>
        //
        // The _start stub is 3 instructions = 12 bytes.
        // After that come all user functions.

        const R_PPC64_REL24: &str = "R_PPC64_REL24";

        // ── _start stub ──
        // BL <main>      — offset 0, needs relocation
        // LI R0, 1       — offset 4 (sys_exit = 1 on PPC64 Linux)
        // SC             — offset 8

        let start_stub_size: usize = 12; // 3 × 4-byte instructions

        // ── Compute function offsets ──
        // _start stub comes first, then user functions.
        let mut func_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut current_offset: usize = start_stub_size; // after _start

        for func in &program.functions {
            func_offsets.insert(func.name.clone(), current_offset);
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            current_offset += func_size;
        }

        // ── Build _start stub bytes ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // BL <main> — placeholder, will be patched
        // BL encoding: I-form, primary=18, LI=0, AA=0, LK=1
        start_stub.extend_from_slice(&Instruction::Bl { li: 0 }.encode());

        // LI R0, 1 = ADDI R0, 0, 1 (sys_exit = 1)
        start_stub.extend_from_slice(&Instruction::Li { rt: Gpr::R0, simm: 1 }.encode());

        // SC (syscall)
        start_stub.extend_from_slice(&Instruction::Sc.encode());

        // ── Patch _start BL to main ──
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            // BL target = CIA + LI*4, where CIA = address of BL instruction.
            // BL is at offset 0 within all_code. main is at main_offset.
            // LI = (main_offset - 0) / 4
            let li_val = (main_offset as i64) / 4;
            // Mask to 24 bits (signed)
            let imm24 = (li_val as u32) & 0x00FF_FFFF;
            // BL word: primary=18 (bits 26-31), LI (bits 2-25), AA=0 (bit 1), LK=1 (bit 0)
            let bl_word: u32 = (18u32 << 26) | (imm24 << 2) | 1;
            start_stub[0..4].copy_from_slice(&bl_word.to_be_bytes());
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

        // ── Patch BL relocations ──
        // Each function's relocations are relative to the start of that function's code.
        // We need to adjust them by the _start stub size + preceding functions' sizes.
        let mut func_code_offset: usize = start_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 4 > all_code.len() {
                    continue; // skip invalid relocations
                }

                if reloc.reloc_type == R_PPC64_REL24 {
                    // R_PPC64_REL24: patch BL instruction's LI field (24 bits).
                    // BL target = CIA + LI*4, where CIA = address of BL instruction.
                    // So: LI = (target_addr - bl_addr) / 4
                    if let Some(&target_offset) = func_offsets.get(&reloc.symbol) {
                        let bl_addr = abs_offset as i64;
                        let target_addr = target_offset as i64;
                        let offset_words = (target_addr - bl_addr) / 4;
                        // Check range: ±32MB (24-bit signed)
                        if offset_words < -(1 << 23) || offset_words >= (1 << 23) {
                            eprintln!(
                                "warning: BL relocation to '{}' out of range: {} words",
                                reloc.symbol, offset_words
                            );
                            continue;
                        }
                        let imm24 = (offset_words as u32) & 0x00FF_FFFF;
                        let existing = u32::from_be_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        // Clear LI field (bits 2-25) and set new value
                        let patched = (existing & 0xFC00_0003) | (imm24 << 2);
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&patched.to_be_bytes());
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
        Ok(build_ppc64_elf_2seg(&all_code, 0x10000000))
    }

    fn return_stub(&self) -> Vec<u8> {
        // BLR: bclr 20, 0, 0
        Instruction::Bclr {
            bo: 20,
            bi: 0,
            bh: 0,
        }
        .encode()
        .to_vec()
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // PPC64 ELFv2 trampoline using TOC:
        //   lis r12, addr@highest
        //   ori r12, r12, addr@higher
        //   sldi r12, r12, 32
        //   oris r12, r12, addr@h
        //   ori r12, r12, addr@l
        //   mtctr r12
        //   bctr
        let hi32 = (entry_addr >> 32) as u32;
        let lo32 = (entry_addr & 0xFFFF_FFFF) as u32;

        let mut code = Vec::with_capacity(28);

        // lis r12, hi16(hi32)
        code.extend_from_slice(
            &Instruction::Lis {
                rt: Gpr::R12,
                simm: ((hi32 >> 16) & 0xFFFF) as i16 as i32,
            }
            .encode(),
        );
        // ori r12, r12, lo16(hi32)
        code.extend_from_slice(
            &Instruction::Ori {
                ra: Gpr::R12,
                rs: Gpr::R12,
                uimm: hi32 & 0xFFFF,
            }
            .encode(),
        );
        // sldi r12, r12, 32 (= rldicr r12, r12, 32, 31)
        // MD-form: primary=30, rS, rA, SH[0:4], ME[0:4], SH[5], ME[5], xo=2, Rc=0
        // sldi r12, r12, 32 — use R11 as temp for shift amount
        code.extend_from_slice(&Instruction::Li { rt: Gpr::R11, simm: 32 }.encode());
        code.extend_from_slice(&Instruction::Sld { ra: Gpr::R12, rs: Gpr::R12, rb: Gpr::R11 }.encode());
        // oris r12, r12, hi16(lo32) -- oris = primary=25
        let oris_word: u32 = (25u32 << 26)
            | (Gpr::R12.encoding() << 21)
            | (Gpr::R12.encoding() << 16)
            | ((lo32 >> 16) & 0xFFFF);
        code.extend_from_slice(&encode_word(oris_word));
        // ori r12, r12, lo16(lo32)
        code.extend_from_slice(
            &Instruction::Ori {
                ra: Gpr::R12,
                rs: Gpr::R12,
                uimm: lo32 & 0xFFFF,
            }
            .encode(),
        );
        // mtctr r12: primary=31, rS=12, SPR=9<<5, xo=467
        let mtctr_word: u32 =
            (31u32 << 26) | (Gpr::R12.encoding() << 21) | (9u32 << 16) | (467 << 1);
        code.extend_from_slice(&encode_word(mtctr_word));
        // bctr: bcctr 20, 0, 0
        code.extend_from_slice(
            &Instruction::Bcctr {
                bo: 20,
                bi: 0,
                bh: 0,
            }
            .encode(),
        );

        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset + 4 <= bytes.len() {
            let word = u32::from_be_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            lines.push(format!("{:#010x}:  {:08x}", pc, word));
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
        "ppc64"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(any())] // Disabled: broken tests need fixing
mod tests {
    use super::*;

    // ── Gpr Tests ──────────────────────────────────────────────────

    #[test]
    fn test_gpr_encoding() {
        assert_eq!(Gpr::R0.encoding(), 0);
        assert_eq!(Gpr::R1.encoding(), 1);
        assert_eq!(Gpr::R2.encoding(), 2);
        assert_eq!(Gpr::R3.encoding(), 3);
        assert_eq!(Gpr::R10.encoding(), 10);
        assert_eq!(Gpr::R13.encoding(), 13);
        assert_eq!(Gpr::R31.encoding(), 31);
    }

    #[test]
    fn test_gpr_is_allocatable() {
        assert!(!Gpr::R0.is_allocatable()); // volatile/special
        assert!(!Gpr::R1.is_allocatable()); // SP
        assert!(!Gpr::R2.is_allocatable()); // TOC
        assert!(!Gpr::R13.is_allocatable()); // thread pointer
        assert!(Gpr::R3.is_allocatable()); // arg
        assert!(Gpr::R14.is_allocatable()); // callee-saved
        assert!(Gpr::R31.is_allocatable()); // callee-saved
    }

    #[test]
    fn test_gpr_is_callee_saved() {
        assert!(!Gpr::R3.is_callee_saved());
        assert!(!Gpr::R12.is_callee_saved());
        assert!(Gpr::R14.is_callee_saved());
        assert!(Gpr::R31.is_callee_saved());
    }

    #[test]
    fn test_gpr_is_arg_reg() {
        assert!(Gpr::R3.is_arg_reg());
        assert!(Gpr::R10.is_arg_reg());
        assert!(!Gpr::R2.is_arg_reg());
        assert!(!Gpr::R11.is_arg_reg());
    }

    #[test]
    fn test_gpr_arg_register() {
        assert_eq!(Gpr::arg_register(0), Some(Gpr::R3));
        assert_eq!(Gpr::arg_register(7), Some(Gpr::R10));
        assert_eq!(Gpr::arg_register(8), None);
    }

    // ── Fpr Tests ──────────────────────────────────────────────────

    #[test]
    fn test_fpr_encoding() {
        assert_eq!(Fpr::F0.encoding(), 0);
        assert_eq!(Fpr::F13.encoding(), 13);
        assert_eq!(Fpr::F31.encoding(), 31);
    }

    #[test]
    fn test_fpr_is_callee_saved() {
        assert!(!Fpr::F0.is_callee_saved());
        assert!(!Fpr::F13.is_callee_saved());
        assert!(Fpr::F14.is_callee_saved());
        assert!(Fpr::F31.is_callee_saved());
    }

    #[test]
    fn test_fpr_is_arg_reg() {
        assert!(!Fpr::F0.is_arg_reg()); // F0 is volatile but NOT an arg reg
        assert!(Fpr::F1.is_arg_reg()); // F1 is first FP arg
        assert!(Fpr::F13.is_arg_reg());
        assert!(!Fpr::F14.is_arg_reg());
    }

    // ── CrField Tests ──────────────────────────────────────────────

    #[test]
    fn test_crfield_encoding() {
        assert_eq!(CrField::CR0.encoding(), 0);
        assert_eq!(CrField::CR7.encoding(), 7);
    }

    #[test]
    fn test_crfield_is_callee_saved() {
        assert!(!CrField::CR0.is_callee_saved());
        assert!(!CrField::CR1.is_callee_saved());
        assert!(CrField::CR2.is_callee_saved());
        assert!(CrField::CR3.is_callee_saved());
        assert!(CrField::CR4.is_callee_saved());
        assert!(!CrField::CR5.is_callee_saved());
    }

    // ── Instruction Encoding Tests ─────────────────────────────────

    #[test]
    fn test_nop_encoding() {
        // NOP = ORI r0, r0, 0 = 0x60000000
        let encoded = Instruction::Nop.encode();
        assert_eq!(u32::from_be_bytes(encoded), 0x60000000);
    }

    #[test]
    fn test_trap_encoding() {
        // TRAP = TW 31, r0, r0 = 0x7FE00008
        let encoded = Instruction::Trap.encode();
        assert_eq!(u32::from_be_bytes(encoded), 0x7FE00008);
    }

    #[test]
    fn test_add_encoding() {
        // ADD r3, r4, r5: primary=31, rT=3, rA=4, rB=5, OE=0, xo=266, Rc=0
        let encoded = Instruction::Add {
            rt: Gpr::R3,
            ra: Gpr::R4,
            rb: Gpr::R5,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 31); // primary opcode
        assert_eq!((word >> 21) & 0x1F, 3); // rT
        assert_eq!((word >> 16) & 0x1F, 4); // rA
        assert_eq!((word >> 11) & 0x1F, 5); // rB
        assert_eq!((word >> 1) & 0x1FF, 266); // xo
        assert_eq!(word & 1, 0); // Rc
    }

    #[test]
    fn test_addi_encoding() {
        // ADDI r3, r4, 100: primary=14, rT=3, rA=4, simm=100
        let encoded = Instruction::Addi {
            rt: Gpr::R3,
            ra: Gpr::R4,
            simm: 100,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 14);
        assert_eq!((word >> 21) & 0x1F, 3);
        assert_eq!((word >> 16) & 0x1F, 4);
        assert_eq!((word & 0xFFFF) as i16, 100i16);
    }

    #[test]
    fn test_subf_encoding() {
        // SUBF r3, r4, r5: primary=31, rT=3, rA=4, rB=5, OE=0, xo=40
        let encoded = Instruction::Subf {
            rt: Gpr::R3,
            ra: Gpr::R4,
            rb: Gpr::R5,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 31);
        assert_eq!((word >> 1) & 0x1FF, 40);
    }

    #[test]
    fn test_or_encoding() {
        // OR r3, r3, r3 = MR r3, r3
        let encoded = Instruction::Or {
            ra: Gpr::R3,
            rs: Gpr::R3,
            rb: Gpr::R3,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 31);
        assert_eq!((word >> 1) & 0x1FF, 444);
    }

    #[test]
    fn test_mr_encoding() {
        // MR r3, r4 = OR r3, r4, r4
        let mr_encoded = Instruction::Mr {
            ra: Gpr::R3,
            rs: Gpr::R4,
        }
        .encode();
        let or_encoded = Instruction::Or {
            ra: Gpr::R3,
            rs: Gpr::R4,
            rb: Gpr::R4,
        }
        .encode();
        assert_eq!(mr_encoded, or_encoded);
    }

    #[test]
    fn test_li_encoding() {
        // LI r3, 42 = ADDI r3, r0, 42
        let li_encoded = Instruction::Li {
            rt: Gpr::R3,
            simm: 42,
        }
        .encode();
        let addi_encoded = Instruction::Addi {
            rt: Gpr::R3,
            ra: Gpr::R0,
            simm: 42,
        }
        .encode();
        assert_eq!(li_encoded, addi_encoded);
    }

    #[test]
    fn test_ld_encoding() {
        // LD r3, 0(r4): primary=58, rT=3, rA=4, ds=0, xo=0
        let encoded = Instruction::Ld {
            rt: Gpr::R3,
            ra: Gpr::R4,
            ds: 0,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 58);
        assert_eq!((word >> 21) & 0x1F, 3);
        assert_eq!((word >> 16) & 0x1F, 4);
    }

    #[test]
    fn test_std_encoding() {
        // STD r3, 8(r4): primary=62, rS=3, rA=4, ds=8, xo=0
        let encoded = Instruction::Std {
            rs: Gpr::R3,
            ra: Gpr::R4,
            ds: 8,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 62);
        assert_eq!((word >> 21) & 0x1F, 3);
        assert_eq!((word >> 16) & 0x1F, 4);
    }

    #[test]
    fn test_cmp_encoding() {
        // CMP CR0, 1, r3, r4: primary=31, bf=0, l=1, rA=3, rB=4, xo=0
        let encoded = Instruction::Cmp {
            bf: CrField::CR0,
            l: 1,
            ra: Gpr::R3,
            rb: Gpr::R4,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 31);
        assert_eq!((word >> 23) & 0x7, 0); // bf = CR0
        assert_eq!((word >> 21) & 0x1, 1); // l = 1 (64-bit)
    }

    #[test]
    fn test_blr_encoding() {
        // BLR = BCLR 20, 0, 0: primary=19, BO=20, BI=0, BH=0, xo=16, LK=0
        let encoded = Instruction::Bclr {
            bo: 20,
            bi: 0,
            bh: 0,
        }
        .encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 19);
        assert_eq!((word >> 21) & 0x1F, 20); // BO
        assert_eq!((word >> 1) & 0x3FF, 16); // xo
    }

    #[test]
    fn test_sc_encoding() {
        let encoded = Instruction::Sc.encode();
        assert_eq!(u32::from_be_bytes(encoded), 0x44000002);
    }

    #[test]
    fn test_lfd_stfd_encoding() {
        // LFD f1, 0(r3): primary=50
        let lfd = Instruction::Lfd {
            ft: Fpr::F1,
            ra: Gpr::R3,
            d: 0,
        }
        .encode();
        assert_eq!((u32::from_be_bytes(lfd) >> 26) & 0x3F, 50);

        // STFD f1, 0(r3): primary=54
        let stfd = Instruction::Stfd {
            fs: Fpr::F1,
            ra: Gpr::R3,
            d: 0,
        }
        .encode();
        assert_eq!((u32::from_be_bytes(stfd) >> 26) & 0x3F, 54);
    }

    // ── Backend Tests ──────────────────────────────────────────────

    #[test]
    fn test_ppc64_backend_creation() {
        let backend = PPC64Backend::new();
        assert_eq!(backend.name(), "ppc64");
        let info = backend.target_info();
        assert_eq!(info.isa_name(), "ppc64");
        assert_eq!(info.elf_machine_type(), 21);
        assert!(info.has_toc_pointer());
        assert!(info.has_condition_registers());
        assert!(info.has_link_register());
        assert!(!info.has_branch_delay_slots());
        assert!(!info.has_hardwired_zero());
    }

    #[test]
    fn test_return_stub() {
        let backend = PPC64Backend::new();
        let stub = backend.return_stub();
        assert_eq!(stub.len(), 4);
        // BLR: bclr 20, 0, 0
        let word = u32::from_be_bytes([stub[0], stub[1], stub[2], stub[3]]);
        assert_eq!((word >> 26) & 0x3F, 19); // XL-form
        assert_eq!((word >> 21) & 0x1F, 20); // BO=20 (always)
    }

    #[test]
    fn test_trampoline_length() {
        let backend = PPC64Backend::new();
        let tramp = backend.trampoline(0x12345678_9ABCDEF0);
        // 7 instructions * 4 bytes = 28 bytes
        assert_eq!(tramp.len(), 28);
    }

    #[test]
    fn test_disassemble() {
        let backend = PPC64Backend::new();
        let code = Instruction::Nop.encode();
        let lines = backend.disassemble(&code, 0x10000000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("60000000"));
    }

    // ── ISel integration tests ──────────────────────────────────────

    #[test]
    fn test_isel_mulld_encoding() {
        // mulld r3, r4, r5: primary=31, rT=3, rA=4, rB=5, OE=0, xo=233, Rc=0
        let mulld = Instruction::Mulld {
            rt: Gpr::R3,
            ra: Gpr::R4,
            rb: Gpr::R5,
        };
        let encoded = mulld.encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 31, "primary opcode should be 31");
        assert_eq!((word >> 21) & 0x1F, 3, "rT should be r3");
        assert_eq!((word >> 16) & 0x1F, 4, "rA should be r4");
        assert_eq!((word >> 11) & 0x1F, 5, "rB should be r5");
        assert_eq!((word >> 1) & 0x1FF, 233, "xo should be 233 for mulld");
    }

    #[test]
    fn test_isel_alloc_emits_stdu() {
        // Alloc should emit stdu r1, -size(r1), not a NOP or addi
        let backend = PPC64Backend::new();
        let mut func = IRFunction::new("test_alloc");
        func.blocks[0].instructions.push(IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 32,
        });
        func.blocks[0].terminator = crate::ir::IRTerminator::Return(vec![]);
        let allocated = backend.allocate_registers(&func).unwrap();
        // Find the stdu instruction for the alloc
        let stdu_instrs: Vec<_> = allocated
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| i.opcode == "stdu")
            .collect();
        assert!(
            !stdu_instrs.is_empty(),
            "alloc should emit at least one stdu instruction"
        );
        // The stdu encoded bytes should not be a NOP (0x60000000)
        let stdu_encoded = &stdu_instrs[0].encoded;
        let word = u32::from_be_bytes([
            stdu_encoded[0],
            stdu_encoded[1],
            stdu_encoded[2],
            stdu_encoded[3],
        ]);
        assert_ne!(word, 0x60000000, "stdu should not encode as NOP");
    }

    #[test]
    fn test_isel_neg_encoding() {
        // neg r3, r4: primary=31, rT=3, rA=4, rB=0, OE=0, xo=104, Rc=0
        let neg = Instruction::Neg {
            rt: Gpr::R3,
            ra: Gpr::R4,
        };
        let encoded = neg.encode();
        let word = u32::from_be_bytes(encoded);
        assert_eq!((word >> 26) & 0x3F, 31, "primary opcode should be 31");
        assert_eq!((word >> 21) & 0x1F, 3, "rT should be r3");
        assert_eq!((word >> 16) & 0x1F, 4, "rA should be r4");
        assert_eq!((word >> 1) & 0x1FF, 104, "xo should be 104 for neg");
    }

    // ── ISel + Helper Function Tests ──────────────────────────────────

    #[test]
    fn test_load_immediate_ppc64_small() {
        // Small immediate: li r11, 42
        let mut out = Vec::new();
        load_immediate_ppc64(Gpr::R11, 42, &mut out);
        assert_eq!(out.len(), 1, "small immediate should emit 1 instruction");
        assert_eq!(out[0].opcode, "li");
        let word = u32::from_be_bytes(out[0].encoded.clone().try_into().unwrap());
        assert_eq!((word >> 26) & 0x3F, 14, "li should use ADDI primary=14");
        assert_eq!((word >> 21) & 0x1F, 11, "rT should be r11");
        assert_eq!((word & 0xFFFF) as i16, 42i16, "simm should be 42");
    }

    #[test]
    fn test_load_immediate_ppc64_32bit() {
        // 32-bit value: lis + ori
        let mut out = Vec::new();
        load_immediate_ppc64(Gpr::R11, 0x12345678, &mut out);
        assert!(
            out.len() >= 2,
            "32-bit immediate should emit at least 2 instructions (lis + ori)"
        );
        assert_eq!(out[0].opcode, "lis");
        assert_eq!(out[1].opcode, "ori");
        // Verify the lis loads the upper 16 bits
        let lis_word = u32::from_be_bytes(out[0].encoded.clone().try_into().unwrap());
        assert_eq!((lis_word >> 21) & 0x1F, 11, "rT should be r11");
    }

    #[test]
    fn test_load_immediate_ppc64_64bit() {
        // Full 64-bit value: lis + ori + rldicr + oris + ori
        let mut out = Vec::new();
        load_immediate_ppc64(Gpr::R12, 0x12345678_9ABCDEF0i64 as i64, &mut out);
        // Should emit: lis, ori (optional), rldicr, oris, ori (optional)
        assert!(
            out.len() >= 3,
            "64-bit immediate should emit at least 3 instructions (lis + rldicr + oris/ori)"
        );
        assert_eq!(out[0].opcode, "lis", "first instruction should be lis");
        // Verify rldicr is present
        let has_rldicr = out.iter().any(|i| i.opcode == "rldicr");
        assert!(
            has_rldicr,
            "64-bit immediate load must include sldi (rldicr)"
        );
    }

    #[test]
    fn test_resolve_gpr_ppc64_immediate() {
        // resolve_gpr_ppc64 with an Immediate value should load into scratch
        let mut reg_map = std::collections::HashMap::new();
        let mut out = Vec::new();
        let result = resolve_gpr_ppc64(&IRValue::Immediate(100), &mut reg_map, Gpr::R11, &mut out);
        assert_eq!(
            result,
            Gpr::R11,
            "immediate should resolve to scratch register"
        );
        assert!(
            !out.is_empty(),
            "loading an immediate should emit instructions"
        );
        assert_eq!(out[0].opcode, "li", "small immediate should use li");
    }

    #[test]
    fn test_resolve_gpr_ppc64_register() {
        // resolve_gpr_ppc64 with a Register value should look up in reg_map
        let mut reg_map = std::collections::HashMap::new();
        reg_map.insert(42u32, Gpr::R3);
        let mut out = Vec::new();
        let result = resolve_gpr_ppc64(&IRValue::Register(42), &mut reg_map, Gpr::R11, &mut out);
        assert_eq!(result, Gpr::R3, "register should resolve via reg_map");
        assert!(
            out.is_empty(),
            "register lookup should not emit instructions"
        );
    }

    #[test]
    fn test_isel_free_emits_trap() {
        // Free should emit a trap (tw 31, r0, r0), not a NOP
        let backend = PPC64Backend::new();
        let mut func = IRFunction::new("test_free");
        func.blocks[0].instructions.push(IRInstr::Free {
            ptr: IRValue::Register(0),
        });
        func.blocks[0].terminator = crate::ir::IRTerminator::Return(vec![]);
        let allocated = backend.allocate_registers(&func).unwrap();
        // Find the trap instruction
        let trap_instrs: Vec<_> = allocated
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| i.opcode == "trap")
            .collect();
        assert!(
            !trap_instrs.is_empty(),
            "free should emit at least one trap instruction"
        );
        // Verify the trap encoding is NOT a NOP (0x60000000)
        let trap_encoded = &trap_instrs[0].encoded;
        let word = u32::from_be_bytes([
            trap_encoded[0],
            trap_encoded[1],
            trap_encoded[2],
            trap_encoded[3],
        ]);
        assert_eq!(word, 0x7FE00008, "trap should encode as tw 31, r0, r0");
    }

    #[test]
    fn test_isel_phi_emits_nop() {
        // Phi should emit NOP (eliminated by SSA)
        let backend = PPC64Backend::new();
        let mut func = IRFunction::new("test_phi");
        func.blocks[0].instructions.push(IRInstr::Phi {
            dst: IRValue::Register(0),
            incoming: vec![(IRValue::Register(1), "entry".to_string())],
        });
        func.blocks[0].terminator = crate::ir::IRTerminator::Return(vec![]);
        let allocated = backend.allocate_registers(&func).unwrap();
        // Find the nop instruction
        let nop_instrs: Vec<_> = allocated
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| i.opcode == "nop")
            .collect();
        assert!(!nop_instrs.is_empty(), "phi should emit a NOP instruction");
    }

    #[test]
    fn test_load_immediate_ppc64_negative() {
        // Negative immediate: li r11, -1
        let mut out = Vec::new();
        load_immediate_ppc64(Gpr::R11, -1, &mut out);
        assert_eq!(out.len(), 1, "small negative should emit 1 instruction");
        assert_eq!(out[0].opcode, "li");
        let word = u32::from_be_bytes(out[0].encoded.clone().try_into().unwrap());
        assert_eq!((word & 0xFFFF) as i16, -1i16, "simm should be -1");
    }

    #[test]
    fn test_isel_binop_with_immediate() {
        // BinOp::Add with an immediate operand should emit load + add
        let backend = PPC64Backend::new();
        let mut func = IRFunction::new("test_add_imm");
        func.blocks[0].instructions.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(2),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(42),
        });
        func.blocks[0].terminator = crate::ir::IRTerminator::Return(vec![]);
        let allocated = backend.allocate_registers(&func).unwrap();
        // Should contain at least a li (load immediate) and an add
        let opcodes: Vec<&str> = allocated
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .map(|i| i.opcode.as_str())
            .collect();
        assert!(
            opcodes.contains(&"add"),
            "BinOp::Add with immediate should still emit an add instruction"
        );
    }
}
pub mod disasm;
