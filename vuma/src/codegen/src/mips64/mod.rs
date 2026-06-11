//! # MIPS 64-bit Backend
//!
//! Implements the `Backend` trait for the MIPS 64-bit target (N64 ABI,
//! big-endian).  This module provides:
//!
//! - `Gpr` — General-purpose register enum ($0–$31)
//! - `Fpr` — Floating-point register enum ($f0–$f31)
//! - `Instruction` — MIPS64 instruction enum with correct encoding for
//!   R-type, I-type, and J-type formats
//! - `Mips64Backend` — `Backend` implementation that lowers IR to MIPS64
//!   machine code and emits ELF64 binaries
//!
//! ## MIPS64 Register Convention (N64 ABI)
//!
//! | Register(s) | ABI Name | Role                              |
//! |-------------|----------|-----------------------------------|
//! | $0          | $zero    | Hardwired zero                    |
//! | $1          | $at      | Assembler temporary               |
//! | $2–$3       | $v0–$v1  | Return values                     |
//! | $4–$7       | $a0–$a3  | Argument registers                |
//! | $8–$15      | $t0–$t7  | Caller-saved temporaries          |
//! | $16–$23     | $s0–$s7  | Callee-saved                      |
//! | $24–$25     | $t8–$t9  | Caller-saved temporaries          |
//! | $26–$27     | $k0–$k1  | Kernel registers                  |
//! | $28         | $gp      | Global pointer                    |
//! | $29         | $sp      | Stack pointer                     |
//! | $30         | $fp      | Frame pointer (callee-saved)      |
//! | $31         | $ra      | Return address                    |
//!
//! ## Branch Delay Slots
//!
//! MIPS has branch delay slots: the instruction immediately following a branch
//! or jump is always executed before the branch takes effect.  This backend
//! inserts a NOP (0x00000000) in every delay slot for correctness.
//!
//! ## Instruction Encoding
//!
//! All instructions are 32 bits, **big-endian**, with three formats:
//!
//! - **R-type**: `opcode[31:26] | rs[25:21] | rt[20:16] | rd[15:11] | sa[10:6] | funct[5:0]`
//! - **I-type**: `opcode[31:26] | rs[25:21] | rt[20:16] | imm[15:0]`
//! - **J-type**: `opcode[31:26] | target[25:0]`
//!
//! ## References
//!
//! - MIPS64 Architecture for Programmers, Volume II: Instruction Set
//! - <https://www.mips.com/products/architectures/mips64/>

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, Mips64TargetInfo, PhysicalReg, RegClass, TargetInfo,
};
use crate::ir::{BinOpKind, CmpKind, IRFunction, IRInstr, IRValue, UnaryOpKind};
use std::fmt;

// ===========================================================================
// MIPS64 Opcode Constants
// ===========================================================================

/// R-type opcode (SPECIAL): all R-type instructions use opcode = 0x00.
const OPC_SPECIAL: u32 = 0x00;

/// I-type opcodes.
const OPC_ADDI: u32 = 0x08;
const OPC_ADDIU: u32 = 0x09;
const OPC_ANDI: u32 = 0x0C;
const OPC_ORI: u32 = 0x0D;
const OPC_XORI: u32 = 0x0E;
const OPC_SLTI: u32 = 0x0A;
const OPC_SLTIU: u32 = 0x0B;
const OPC_LUI: u32 = 0x0F;
const OPC_DADDI: u32 = 0x18;
const OPC_DADDIU: u32 = 0x19;

/// Branch I-type opcodes.
const OPC_BEQ: u32 = 0x04;
const OPC_BNE: u32 = 0x05;
const OPC_BLEZ: u32 = 0x06;
const OPC_BGTZ: u32 = 0x07;

/// Load I-type opcodes.
const OPC_LB: u32 = 0x20;
const OPC_LH: u32 = 0x21;
const OPC_LW: u32 = 0x23;
const OPC_LD: u32 = 0x37;
const OPC_LBU: u32 = 0x24;
const OPC_LHU: u32 = 0x25;
const OPC_LWU: u32 = 0x27;

/// Store I-type opcodes.
const OPC_SB: u32 = 0x28;
const OPC_SH: u32 = 0x29;
const OPC_SW: u32 = 0x2B;
const OPC_SD: u32 = 0x3F;

/// FP load/store I-type opcodes.
const OPC_LWC1: u32 = 0x31;
const OPC_SWC1: u32 = 0x39;
const OPC_LDC1: u32 = 0x35;
const OPC_SDC1: u32 = 0x3D;

/// J-type opcodes.
const OPC_J: u32 = 0x02;
const OPC_JAL: u32 = 0x03;

/// R-type function codes (opcode = 0x00).
const FN_ADD: u32 = 0x20;
const FN_ADDU: u32 = 0x21;
const FN_SUB: u32 = 0x22;
const FN_SUBU: u32 = 0x23;
const FN_AND: u32 = 0x24;
const FN_OR: u32 = 0x25;
const FN_XOR: u32 = 0x26;
const FN_NOR: u32 = 0x27;
const FN_SLT: u32 = 0x2A;
const FN_SLTU: u32 = 0x2B;
const FN_SLL: u32 = 0x00;
const FN_SRL: u32 = 0x02;
const FN_SRA: u32 = 0x03;
const FN_SLLV: u32 = 0x04;
const FN_SRLV: u32 = 0x06;
const FN_SRAV: u32 = 0x07;
const FN_MULT: u32 = 0x18;
const FN_MULTU: u32 = 0x19;
const FN_DIV: u32 = 0x1A;
const FN_DIVU: u32 = 0x1B;
const FN_MFHI: u32 = 0x10;
const FN_MFLO: u32 = 0x12;
const FN_DADD: u32 = 0x2C;
const FN_DSUB: u32 = 0x2E;
const FN_DADDU: u32 = 0x2D;
const FN_DSUBU: u32 = 0x2F;
const FN_DSLL: u32 = 0x38;
const FN_DSRL: u32 = 0x3A;
const FN_DSRA: u32 = 0x3B;
const FN_DSLLV: u32 = 0x14;
const FN_DSRLV: u32 = 0x16;
const FN_DSRAV: u32 = 0x17;
const FN_DMULT: u32 = 0x1C;
const FN_DMULTU: u32 = 0x1D;
const FN_DDIV: u32 = 0x1E;
const FN_DDIVU: u32 = 0x1F;
const FN_MOVZ: u32 = 0x0A;
const FN_MOVN: u32 = 0x0B;
const FN_JR: u32 = 0x08;
const FN_JALR: u32 = 0x09;
const FN_SYSCALL: u32 = 0x0C;
const FN_BREAK: u32 = 0x0D;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// MIPS64 general-purpose registers ($0–$31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Gpr {
    Zero = 0,
    At = 1,
    V0 = 2,
    V1 = 3,
    A0 = 4,
    A1 = 5,
    A2 = 6,
    A3 = 7,
    T0 = 8,
    T1 = 9,
    T2 = 10,
    T3 = 11,
    T4 = 12,
    T5 = 13,
    T6 = 14,
    T7 = 15,
    S0 = 16,
    S1 = 17,
    S2 = 18,
    S3 = 19,
    S4 = 20,
    S5 = 21,
    S6 = 22,
    S7 = 23,
    T8 = 24,
    T9 = 25,
    K0 = 26,
    K1 = 27,
    Gp = 28,
    Sp = 29,
    Fp = 30,
    Ra = 31,
}

impl Gpr {
    /// Returns the 5-bit encoding index for this register.
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    /// Returns `true` if this register is available for register allocation.
    ///
    /// Zero ($0), At ($1), K0–K1 ($26–$27), Gp ($28), Sp ($29), and Ra ($31)
    /// are reserved.
    pub fn is_allocatable(&self) -> bool {
        !matches!(
            self,
            Gpr::Zero | Gpr::At | Gpr::K0 | Gpr::K1 | Gpr::Gp | Gpr::Sp | Gpr::Ra
        )
    }

    /// Returns `true` if this register is callee-saved (s0–s7, fp).
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
                | Gpr::Fp
        )
    }

    /// Returns `true` if this register is an argument register (a0–a3).
    pub fn is_arg_reg(&self) -> bool {
        matches!(self, Gpr::A0 | Gpr::A1 | Gpr::A2 | Gpr::A3)
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::Zero => "$zero",
            Gpr::At => "$at",
            Gpr::V0 => "$v0",
            Gpr::V1 => "$v1",
            Gpr::A0 => "$a0",
            Gpr::A1 => "$a1",
            Gpr::A2 => "$a2",
            Gpr::A3 => "$a3",
            Gpr::T0 => "$t0",
            Gpr::T1 => "$t1",
            Gpr::T2 => "$t2",
            Gpr::T3 => "$t3",
            Gpr::T4 => "$t4",
            Gpr::T5 => "$t5",
            Gpr::T6 => "$t6",
            Gpr::T7 => "$t7",
            Gpr::S0 => "$s0",
            Gpr::S1 => "$s1",
            Gpr::S2 => "$s2",
            Gpr::S3 => "$s3",
            Gpr::S4 => "$s4",
            Gpr::S5 => "$s5",
            Gpr::S6 => "$s6",
            Gpr::S7 => "$s7",
            Gpr::T8 => "$t8",
            Gpr::T9 => "$t9",
            Gpr::K0 => "$k0",
            Gpr::K1 => "$k1",
            Gpr::Gp => "$gp",
            Gpr::Sp => "$sp",
            Gpr::Fp => "$fp",
            Gpr::Ra => "$ra",
        }
    }

    /// Returns the Gpr for a given argument index (0–3). Returns `None` for
    /// indices >= 4.
    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::A0),
            1 => Some(Gpr::A1),
            2 => Some(Gpr::A2),
            3 => Some(Gpr::A3),
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

/// MIPS64 floating-point registers ($f0–$f31).
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

    /// Returns `true` if this register is callee-saved ($f20–$f31).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Fpr::F20
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

    /// Returns `true` if this register is an FP argument register ($f12–$f19).
    pub fn is_arg_reg(&self) -> bool {
        matches!(
            self,
            Fpr::F12
                | Fpr::F13
                | Fpr::F14
                | Fpr::F15
                | Fpr::F16
                | Fpr::F17
                | Fpr::F18
                | Fpr::F19
        )
    }

    /// Returns `true` if this register is available for register allocation.
    pub fn is_allocatable(&self) -> bool {
        true
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Fpr::F0 => "$f0",
            Fpr::F1 => "$f1",
            Fpr::F2 => "$f2",
            Fpr::F3 => "$f3",
            Fpr::F4 => "$f4",
            Fpr::F5 => "$f5",
            Fpr::F6 => "$f6",
            Fpr::F7 => "$f7",
            Fpr::F8 => "$f8",
            Fpr::F9 => "$f9",
            Fpr::F10 => "$f10",
            Fpr::F11 => "$f11",
            Fpr::F12 => "$f12",
            Fpr::F13 => "$f13",
            Fpr::F14 => "$f14",
            Fpr::F15 => "$f15",
            Fpr::F16 => "$f16",
            Fpr::F17 => "$f17",
            Fpr::F18 => "$f18",
            Fpr::F19 => "$f19",
            Fpr::F20 => "$f20",
            Fpr::F21 => "$f21",
            Fpr::F22 => "$f22",
            Fpr::F23 => "$f23",
            Fpr::F24 => "$f24",
            Fpr::F25 => "$f25",
            Fpr::F26 => "$f26",
            Fpr::F27 => "$f27",
            Fpr::F28 => "$f28",
            Fpr::F29 => "$f29",
            Fpr::F30 => "$f30",
            Fpr::F31 => "$f31",
        }
    }

    /// Returns the Fpr for a given FP argument index (0–7). Returns `None` for
    /// indices >= 8.
    pub fn arg_register(index: usize) -> Option<Fpr> {
        match index {
            0 => Some(Fpr::F12),
            1 => Some(Fpr::F13),
            2 => Some(Fpr::F14),
            3 => Some(Fpr::F15),
            4 => Some(Fpr::F16),
            5 => Some(Fpr::F17),
            6 => Some(Fpr::F18),
            7 => Some(Fpr::F19),
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

/// Encode an R-type instruction (big-endian).
///
/// Format: `opcode[31:26] | rs[25:21] | rt[20:16] | rd[15:11] | sa[10:6] | funct[5:0]`
fn encode_r_type(opcode: u32, rs: u32, rt: u32, rd: u32, sa: u32, funct: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26)
        | ((rs & 0x1F) << 21)
        | ((rt & 0x1F) << 16)
        | ((rd & 0x1F) << 11)
        | ((sa & 0x1F) << 6)
        | (funct & 0x3F);
    word.to_be_bytes()
}

/// Encode an I-type instruction (big-endian).
///
/// Format: `opcode[31:26] | rs[25:21] | rt[20:16] | imm[15:0]`
fn encode_i_type(opcode: u32, rs: u32, rt: u32, imm: u32) -> [u8; 4] {
    let word =
        ((opcode & 0x3F) << 26) | ((rs & 0x1F) << 21) | ((rt & 0x1F) << 16) | (imm & 0xFFFF);
    word.to_be_bytes()
}

/// Encode a J-type instruction (big-endian).
///
/// Format: `opcode[31:26] | target[25:0]`
fn encode_j_type(opcode: u32, target: u32) -> [u8; 4] {
    let word = ((opcode & 0x3F) << 26) | (target & 0x03FF_FFFF);
    word.to_be_bytes()
}

/// Encode a NOP instruction (0x00000000).
fn encode_nop() -> [u8; 4] {
    0x00000000u32.to_be_bytes()
}

// ===========================================================================
// Instruction Enum
// ===========================================================================

/// MIPS64 instruction representations for code generation.
///
/// Covers key R-type, I-type, and J-type instructions from the MIPS64 ISA.
/// Each variant captures the operands needed for encoding and disassembly.
/// The `encode()` method produces a 4-byte **big-endian** machine code word.
///
/// Branch delay slots are handled by the `has_delay_slot()` method: when it
/// returns `true`, the caller must insert a NOP after the instruction.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Instruction {
    // ── R-type: Arithmetic (32-bit) ────────────────────────────────────
    /// Add: `add rd, rs, rt`
    Add { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Add Unsigned: `addu rd, rs, rt`
    Addu { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Subtract: `sub rd, rs, rt`
    Sub { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Subtract Unsigned: `subu rd, rs, rt`
    Subu { rd: Gpr, rs: Gpr, rt: Gpr },

    // ── R-type: Logical ────────────────────────────────────────────────
    /// AND: `and rd, rs, rt`
    And { rd: Gpr, rs: Gpr, rt: Gpr },
    /// OR: `or rd, rs, rt`
    Or { rd: Gpr, rs: Gpr, rt: Gpr },
    /// XOR: `xor rd, rs, rt`
    Xor { rd: Gpr, rs: Gpr, rt: Gpr },
    /// NOR: `nor rd, rs, rt`
    Nor { rd: Gpr, rs: Gpr, rt: Gpr },

    // ── R-type: Set on Less Than ───────────────────────────────────────
    /// Set on Less Than (signed): `slt rd, rs, rt`
    Slt { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Set on Less Than (unsigned): `sltu rd, rs, rt`
    Sltu { rd: Gpr, rs: Gpr, rt: Gpr },

    // ── R-type: Shift (immediate sa) ───────────────────────────────────
    /// Shift Left Logical: `sll rd, rt, sa`
    Sll { rd: Gpr, rt: Gpr, sa: u32 },
    /// Shift Right Logical: `srl rd, rt, sa`
    Srl { rd: Gpr, rt: Gpr, sa: u32 },
    /// Shift Right Arithmetic: `sra rd, rt, sa`
    Sra { rd: Gpr, rt: Gpr, sa: u32 },

    // ── R-type: Shift (variable) ───────────────────────────────────────
    /// Shift Left Logical Variable: `sllv rd, rt, rs`
    Sllv { rd: Gpr, rt: Gpr, rs: Gpr },
    /// Shift Right Logical Variable: `srlv rd, rt, rs`
    Srlv { rd: Gpr, rt: Gpr, rs: Gpr },
    /// Shift Right Arithmetic Variable: `srav rd, rt, rs`
    Srav { rd: Gpr, rt: Gpr, rs: Gpr },

    // ── R-type: Multiply/Divide (32-bit) ───────────────────────────────
    /// Multiply (signed): `mult rs, rt` → HI:LO
    Mult { rs: Gpr, rt: Gpr },
    /// Multiply (unsigned): `multu rs, rt` → HI:LO
    Multu { rs: Gpr, rt: Gpr },
    /// Divide (signed): `div rs, rt` → LO = quotient, HI = remainder
    Div { rs: Gpr, rt: Gpr },
    /// Divide (unsigned): `divu rs, rt` → LO = quotient, HI = remainder
    Divu { rs: Gpr, rt: Gpr },

    // ── R-type: Move from HI/LO ────────────────────────────────────────
    /// Move from HI: `mfhi rd`
    Mfhi { rd: Gpr },
    /// Move from LO: `mflo rd`
    Mflo { rd: Gpr },

    // ── R-type: Arithmetic (64-bit) ────────────────────────────────────
    /// Doubleword Add: `dadd rd, rs, rt`
    Dadd { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Doubleword Subtract: `dsub rd, rs, rt`
    Dsub { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Doubleword Add Unsigned: `daddu rd, rs, rt`
    Daddu { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Doubleword Subtract Unsigned: `dsubu rd, rs, rt`
    Dsubu { rd: Gpr, rs: Gpr, rt: Gpr },

    // ── R-type: Shift (64-bit, immediate sa) ──────────────────────────
    /// Doubleword Shift Left Logical: `dsll rd, rt, sa`
    Dsll { rd: Gpr, rt: Gpr, sa: u32 },
    /// Doubleword Shift Right Logical: `dsrl rd, rt, sa`
    Dsrl { rd: Gpr, rt: Gpr, sa: u32 },
    /// Doubleword Shift Right Arithmetic: `dsra rd, rt, sa`
    Dsra { rd: Gpr, rt: Gpr, sa: u32 },

    // ── R-type: Shift (64-bit, variable) ──────────────────────────────
    /// Doubleword Shift Left Logical Variable: `dsllv rd, rt, rs`
    Dsllv { rd: Gpr, rt: Gpr, rs: Gpr },
    /// Doubleword Shift Right Logical Variable: `dsrlv rd, rt, rs`
    Dsrlv { rd: Gpr, rt: Gpr, rs: Gpr },
    /// Doubleword Shift Right Arithmetic Variable: `dsrav rd, rt, rs`
    Dsrav { rd: Gpr, rt: Gpr, rs: Gpr },

    // ── R-type: Multiply/Divide (64-bit) ───────────────────────────────
    /// Doubleword Multiply (signed): `dmult rs, rt`
    Dmult { rs: Gpr, rt: Gpr },
    /// Doubleword Multiply (unsigned): `dmultu rs, rt`
    Dmultu { rs: Gpr, rt: Gpr },
    /// Doubleword Divide (signed): `ddiv rs, rt`
    Ddiv { rs: Gpr, rt: Gpr },
    /// Doubleword Divide (unsigned): `ddivu rs, rt`
    Ddivu { rs: Gpr, rt: Gpr },

    // ── R-type: Conditional Move ───────────────────────────────────────
    /// Move Conditional on Zero: `movz rd, rs, rt`
    Movz { rd: Gpr, rs: Gpr, rt: Gpr },
    /// Move Conditional on Not Zero: `movn rd, rs, rt`
    Movn { rd: Gpr, rs: Gpr, rt: Gpr },

    // ── R-type: Jump Register ──────────────────────────────────────────
    /// Jump Register: `jr rs`
    Jr { rs: Gpr },
    /// Jump and Link Register: `jalr rd, rs`
    Jalr { rd: Gpr, rs: Gpr },

    // ── R-type: System ─────────────────────────────────────────────────
    /// System Call: `syscall code`
    Syscall { code: u32 },
    /// Break: `break code`
    Break { code: u32 },

    // ── I-type: Immediate Arithmetic (32-bit) ─────────────────────────
    /// Add Immediate: `addi rt, rs, imm`
    Addi { rt: Gpr, rs: Gpr, imm: i32 },
    /// Add Immediate Unsigned: `addiu rt, rs, imm`
    Addiu { rt: Gpr, rs: Gpr, imm: i32 },

    // ── I-type: Immediate Logical ──────────────────────────────────────
    /// AND Immediate: `andi rt, rs, imm`
    Andi { rt: Gpr, rs: Gpr, imm: u32 },
    /// OR Immediate: `ori rt, rs, imm`
    Ori { rt: Gpr, rs: Gpr, imm: u32 },
    /// XOR Immediate: `xori rt, rs, imm`
    Xori { rt: Gpr, rs: Gpr, imm: u32 },

    // ── I-type: Set on Less Than Immediate ─────────────────────────────
    /// Set on Less Than Immediate (signed): `slti rt, rs, imm`
    Slti { rt: Gpr, rs: Gpr, imm: i32 },
    /// Set on Less Than Immediate (unsigned): `sltiu rt, rs, imm`
    Sltiu { rt: Gpr, rs: Gpr, imm: i32 },

    // ── I-type: Upper Immediate ────────────────────────────────────────
    /// Load Upper Immediate: `lui rt, imm`
    Lui { rt: Gpr, imm: u32 },

    // ── I-type: Immediate Arithmetic (64-bit) ─────────────────────────
    /// Doubleword Add Immediate: `daddi rt, rs, imm`
    Daddi { rt: Gpr, rs: Gpr, imm: i32 },
    /// Doubleword Add Immediate Unsigned: `daddiu rt, rs, imm`
    Daddiu { rt: Gpr, rs: Gpr, imm: i32 },

    // ── I-type: Branch ─────────────────────────────────────────────────
    /// Branch on Equal: `beq rs, rt, offset`
    Beq { rs: Gpr, rt: Gpr, offset: i32 },
    /// Branch on Not Equal: `bne rs, rt, offset`
    Bne { rs: Gpr, rt: Gpr, offset: i32 },
    /// Branch on Less than or Equal to Zero: `blez rs, offset`
    Blez { rs: Gpr, offset: i32 },
    /// Branch on Greater than Zero: `bgtz rs, offset`
    Bgtz { rs: Gpr, offset: i32 },

    // ── I-type: Load ───────────────────────────────────────────────────
    /// Load Byte (sign-extended): `lb rt, offset(base)`
    Lb { rt: Gpr, base: Gpr, offset: i32 },
    /// Load Halfword (sign-extended): `lh rt, offset(base)`
    Lh { rt: Gpr, base: Gpr, offset: i32 },
    /// Load Word (sign-extended): `lw rt, offset(base)`
    Lw { rt: Gpr, base: Gpr, offset: i32 },
    /// Load Doubleword: `ld rt, offset(base)`
    Ld { rt: Gpr, base: Gpr, offset: i32 },
    /// Load Byte (zero-extended): `lbu rt, offset(base)`
    Lbu { rt: Gpr, base: Gpr, offset: i32 },
    /// Load Halfword (zero-extended): `lhu rt, offset(base)`
    Lhu { rt: Gpr, base: Gpr, offset: i32 },
    /// Load Word (zero-extended): `lwu rt, offset(base)`
    Lwu { rt: Gpr, base: Gpr, offset: i32 },

    // ── I-type: Store ──────────────────────────────────────────────────
    /// Store Byte: `sb rt, offset(base)`
    Sb { rt: Gpr, base: Gpr, offset: i32 },
    /// Store Halfword: `sh rt, offset(base)`
    Sh { rt: Gpr, base: Gpr, offset: i32 },
    /// Store Word: `sw rt, offset(base)`
    Sw { rt: Gpr, base: Gpr, offset: i32 },
    /// Store Doubleword: `sd rt, offset(base)`
    Sd { rt: Gpr, base: Gpr, offset: i32 },

    // ── I-type: FP Load/Store ──────────────────────────────────────────
    /// Load Word to Coprocessor 1: `lwc1 ft, offset(base)`
    Lwc1 { ft: Fpr, base: Gpr, offset: i32 },
    /// Store Word from Coprocessor 1: `swc1 ft, offset(base)`
    Swc1 { ft: Fpr, base: Gpr, offset: i32 },
    /// Load Doubleword to Coprocessor 1: `ldc1 ft, offset(base)`
    Ldc1 { ft: Fpr, base: Gpr, offset: i32 },
    /// Store Doubleword from Coprocessor 1: `sdc1 ft, offset(base)`
    Sdc1 { ft: Fpr, base: Gpr, offset: i32 },

    // ── J-type: Jump ───────────────────────────────────────────────────
    /// Jump: `j target`
    J { target: u32 },
    /// Jump and Link: `jal target`
    Jal { target: u32 },

    // ── No-op ──────────────────────────────────────────────────────────
    /// No-operation (encoded as `sll $zero, $zero, 0` = 0x00000000).
    Nop,
}

impl Instruction {
    /// Encode this instruction into a 4-byte **big-endian** machine code word.
    ///
    /// Encoding follows the MIPS64 ISA Specification.
    pub fn encode(&self) -> [u8; 4] {
        match self {
            // ── R-type: Arithmetic (32-bit) ────────────────────────────
            Instruction::Add { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_ADD)
            }
            Instruction::Addu { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_ADDU)
            }
            Instruction::Sub { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_SUB)
            }
            Instruction::Subu { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_SUBU)
            }

            // ── R-type: Logical ────────────────────────────────────────
            Instruction::And { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_AND)
            }
            Instruction::Or { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_OR)
            }
            Instruction::Xor { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_XOR)
            }
            Instruction::Nor { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_NOR)
            }

            // ── R-type: Set on Less Than ───────────────────────────────
            Instruction::Slt { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_SLT)
            }
            Instruction::Sltu { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_SLTU)
            }

            // ── R-type: Shift (immediate sa) ───────────────────────────
            Instruction::Sll { rd, rt, sa } => {
                encode_r_type(OPC_SPECIAL, 0, rt.encoding(), rd.encoding(), *sa & 0x1F, FN_SLL)
            }
            Instruction::Srl { rd, rt, sa } => {
                encode_r_type(OPC_SPECIAL, 0, rt.encoding(), rd.encoding(), *sa & 0x1F, FN_SRL)
            }
            Instruction::Sra { rd, rt, sa } => {
                encode_r_type(OPC_SPECIAL, 0, rt.encoding(), rd.encoding(), *sa & 0x1F, FN_SRA)
            }

            // ── R-type: Shift (variable) ───────────────────────────────
            Instruction::Sllv { rd, rt, rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_SLLV)
            }
            Instruction::Srlv { rd, rt, rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_SRLV)
            }
            Instruction::Srav { rd, rt, rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_SRAV)
            }

            // ── R-type: Multiply/Divide (32-bit) ───────────────────────
            Instruction::Mult { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_MULT)
            }
            Instruction::Multu { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_MULTU)
            }
            Instruction::Div { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_DIV)
            }
            Instruction::Divu { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_DIVU)
            }

            // ── R-type: Move from HI/LO ────────────────────────────────
            Instruction::Mfhi { rd } => {
                encode_r_type(OPC_SPECIAL, 0, 0, rd.encoding(), 0, FN_MFHI)
            }
            Instruction::Mflo { rd } => {
                encode_r_type(OPC_SPECIAL, 0, 0, rd.encoding(), 0, FN_MFLO)
            }

            // ── R-type: Arithmetic (64-bit) ────────────────────────────
            Instruction::Dadd { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_DADD)
            }
            Instruction::Dsub { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_DSUB)
            }
            Instruction::Daddu { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_DADDU)
            }
            Instruction::Dsubu { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_DSUBU)
            }

            // ── R-type: Shift (64-bit, immediate sa) ───────────────────
            Instruction::Dsll { rd, rt, sa } => {
                encode_r_type(OPC_SPECIAL, 0, rt.encoding(), rd.encoding(), *sa & 0x1F, FN_DSLL)
            }
            Instruction::Dsrl { rd, rt, sa } => {
                encode_r_type(OPC_SPECIAL, 0, rt.encoding(), rd.encoding(), *sa & 0x1F, FN_DSRL)
            }
            Instruction::Dsra { rd, rt, sa } => {
                encode_r_type(OPC_SPECIAL, 0, rt.encoding(), rd.encoding(), *sa & 0x1F, FN_DSRA)
            }

            // ── R-type: Shift (64-bit, variable) ──────────────────────
            Instruction::Dsllv { rd, rt, rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_DSLLV)
            }
            Instruction::Dsrlv { rd, rt, rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_DSRLV)
            }
            Instruction::Dsrav { rd, rt, rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_DSRAV)
            }

            // ── R-type: Multiply/Divide (64-bit) ──────────────────────
            Instruction::Dmult { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_DMULT)
            }
            Instruction::Dmultu { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_DMULTU)
            }
            Instruction::Ddiv { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_DDIV)
            }
            Instruction::Ddivu { rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), 0, 0, FN_DDIVU)
            }

            // ── R-type: Conditional Move ───────────────────────────────
            Instruction::Movz { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_MOVZ)
            }
            Instruction::Movn { rd, rs, rt } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), rt.encoding(), rd.encoding(), 0, FN_MOVN)
            }

            // ── R-type: Jump Register ──────────────────────────────────
            Instruction::Jr { rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), 0, 0, 0, FN_JR)
            }
            Instruction::Jalr { rd, rs } => {
                encode_r_type(OPC_SPECIAL, rs.encoding(), 0, rd.encoding(), 0, FN_JALR)
            }

            // ── R-type: System ─────────────────────────────────────────
            Instruction::Syscall { code } => {
                // syscall: bits[25:6] = code, funct = 0x0C
                let word = (OPC_SPECIAL << 26) | ((*code & 0xFFFFF) << 6) | FN_SYSCALL;
                word.to_be_bytes()
            }
            Instruction::Break { code } => {
                // break: bits[25:6] = code, funct = 0x0D
                let word = (OPC_SPECIAL << 26) | ((*code & 0xFFFFF) << 6) | FN_BREAK;
                word.to_be_bytes()
            }

            // ── I-type: Immediate Arithmetic (32-bit) ─────────────────
            Instruction::Addi { rt, rs, imm } => {
                encode_i_type(OPC_ADDI, rs.encoding(), rt.encoding(), (*imm as u32) & 0xFFFF)
            }
            Instruction::Addiu { rt, rs, imm } => {
                encode_i_type(OPC_ADDIU, rs.encoding(), rt.encoding(), (*imm as u32) & 0xFFFF)
            }

            // ── I-type: Immediate Logical ──────────────────────────────
            Instruction::Andi { rt, rs, imm } => {
                encode_i_type(OPC_ANDI, rs.encoding(), rt.encoding(), *imm & 0xFFFF)
            }
            Instruction::Ori { rt, rs, imm } => {
                encode_i_type(OPC_ORI, rs.encoding(), rt.encoding(), *imm & 0xFFFF)
            }
            Instruction::Xori { rt, rs, imm } => {
                encode_i_type(OPC_XORI, rs.encoding(), rt.encoding(), *imm & 0xFFFF)
            }

            // ── I-type: Set on Less Than Immediate ─────────────────────
            Instruction::Slti { rt, rs, imm } => {
                encode_i_type(OPC_SLTI, rs.encoding(), rt.encoding(), (*imm as u32) & 0xFFFF)
            }
            Instruction::Sltiu { rt, rs, imm } => {
                encode_i_type(OPC_SLTIU, rs.encoding(), rt.encoding(), (*imm as u32) & 0xFFFF)
            }

            // ── I-type: Upper Immediate ────────────────────────────────
            Instruction::Lui { rt, imm } => {
                encode_i_type(OPC_LUI, 0, rt.encoding(), *imm & 0xFFFF)
            }

            // ── I-type: Immediate Arithmetic (64-bit) ─────────────────
            Instruction::Daddi { rt, rs, imm } => {
                encode_i_type(OPC_DADDI, rs.encoding(), rt.encoding(), (*imm as u32) & 0xFFFF)
            }
            Instruction::Daddiu { rt, rs, imm } => {
                encode_i_type(OPC_DADDIU, rs.encoding(), rt.encoding(), (*imm as u32) & 0xFFFF)
            }

            // ── I-type: Branch ─────────────────────────────────────────
            Instruction::Beq { rs, rt, offset } => {
                // Offset is in words, shifted left 2 by the hardware.
                let off_words = (*offset >> 2) as u32;
                encode_i_type(OPC_BEQ, rs.encoding(), rt.encoding(), off_words & 0xFFFF)
            }
            Instruction::Bne { rs, rt, offset } => {
                let off_words = (*offset >> 2) as u32;
                encode_i_type(OPC_BNE, rs.encoding(), rt.encoding(), off_words & 0xFFFF)
            }
            Instruction::Blez { rs, offset } => {
                let off_words = (*offset >> 2) as u32;
                encode_i_type(OPC_BLEZ, rs.encoding(), 0, off_words & 0xFFFF)
            }
            Instruction::Bgtz { rs, offset } => {
                let off_words = (*offset >> 2) as u32;
                encode_i_type(OPC_BGTZ, rs.encoding(), 0, off_words & 0xFFFF)
            }

            // ── I-type: Load ───────────────────────────────────────────
            Instruction::Lb { rt, base, offset } => {
                encode_i_type(OPC_LB, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Lh { rt, base, offset } => {
                encode_i_type(OPC_LH, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Lw { rt, base, offset } => {
                encode_i_type(OPC_LW, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Ld { rt, base, offset } => {
                encode_i_type(OPC_LD, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Lbu { rt, base, offset } => {
                encode_i_type(OPC_LBU, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Lhu { rt, base, offset } => {
                encode_i_type(OPC_LHU, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Lwu { rt, base, offset } => {
                encode_i_type(OPC_LWU, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }

            // ── I-type: Store ──────────────────────────────────────────
            Instruction::Sb { rt, base, offset } => {
                encode_i_type(OPC_SB, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Sh { rt, base, offset } => {
                encode_i_type(OPC_SH, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Sw { rt, base, offset } => {
                encode_i_type(OPC_SW, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Sd { rt, base, offset } => {
                encode_i_type(OPC_SD, base.encoding(), rt.encoding(), (*offset as u32) & 0xFFFF)
            }

            // ── I-type: FP Load/Store ──────────────────────────────────
            Instruction::Lwc1 { ft, base, offset } => {
                encode_i_type(OPC_LWC1, base.encoding(), ft.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Swc1 { ft, base, offset } => {
                encode_i_type(OPC_SWC1, base.encoding(), ft.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Ldc1 { ft, base, offset } => {
                encode_i_type(OPC_LDC1, base.encoding(), ft.encoding(), (*offset as u32) & 0xFFFF)
            }
            Instruction::Sdc1 { ft, base, offset } => {
                encode_i_type(OPC_SDC1, base.encoding(), ft.encoding(), (*offset as u32) & 0xFFFF)
            }

            // ── J-type: Jump ───────────────────────────────────────────
            Instruction::J { target } => encode_j_type(OPC_J, *target),
            Instruction::Jal { target } => encode_j_type(OPC_JAL, *target),

            // ── No-op ──────────────────────────────────────────────────
            Instruction::Nop => encode_nop(),
        }
    }

    /// Returns `true` if this instruction has a branch delay slot.
    ///
    /// On MIPS, branches and jumps execute the next instruction (the delay
    /// slot) before the control transfer takes effect.  The backend must
    /// insert a NOP after any instruction for which this returns `true`.
    pub fn has_delay_slot(&self) -> bool {
        matches!(
            self,
            Instruction::Beq { .. }
                | Instruction::Bne { .. }
                | Instruction::Blez { .. }
                | Instruction::Bgtz { .. }
                | Instruction::Jr { .. }
                | Instruction::Jalr { .. }
                | Instruction::J { .. }
                | Instruction::Jal { .. }
        )
    }

    /// Returns the mnemonic name of this instruction.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Instruction::Add { .. } => "add",
            Instruction::Addu { .. } => "addu",
            Instruction::Sub { .. } => "sub",
            Instruction::Subu { .. } => "subu",
            Instruction::And { .. } => "and",
            Instruction::Or { .. } => "or",
            Instruction::Xor { .. } => "xor",
            Instruction::Nor { .. } => "nor",
            Instruction::Slt { .. } => "slt",
            Instruction::Sltu { .. } => "sltu",
            Instruction::Sll { .. } => "sll",
            Instruction::Srl { .. } => "srl",
            Instruction::Sra { .. } => "sra",
            Instruction::Sllv { .. } => "sllv",
            Instruction::Srlv { .. } => "srlv",
            Instruction::Srav { .. } => "srav",
            Instruction::Mult { .. } => "mult",
            Instruction::Multu { .. } => "multu",
            Instruction::Div { .. } => "div",
            Instruction::Divu { .. } => "divu",
            Instruction::Mfhi { .. } => "mfhi",
            Instruction::Mflo { .. } => "mflo",
            Instruction::Dadd { .. } => "dadd",
            Instruction::Dsub { .. } => "dsub",
            Instruction::Daddu { .. } => "daddu",
            Instruction::Dsubu { .. } => "dsubu",
            Instruction::Dsll { .. } => "dsll",
            Instruction::Dsrl { .. } => "dsrl",
            Instruction::Dsra { .. } => "dsra",
            Instruction::Dsllv { .. } => "dsllv",
            Instruction::Dsrlv { .. } => "dsrlv",
            Instruction::Dsrav { .. } => "dsrav",
            Instruction::Dmult { .. } => "dmult",
            Instruction::Dmultu { .. } => "dmultu",
            Instruction::Ddiv { .. } => "ddiv",
            Instruction::Ddivu { .. } => "ddivu",
            Instruction::Movz { .. } => "movz",
            Instruction::Movn { .. } => "movn",
            Instruction::Jr { .. } => "jr",
            Instruction::Jalr { .. } => "jalr",
            Instruction::Syscall { .. } => "syscall",
            Instruction::Break { .. } => "break",
            Instruction::Addi { .. } => "addi",
            Instruction::Addiu { .. } => "addiu",
            Instruction::Andi { .. } => "andi",
            Instruction::Ori { .. } => "ori",
            Instruction::Xori { .. } => "xori",
            Instruction::Slti { .. } => "slti",
            Instruction::Sltiu { .. } => "sltiu",
            Instruction::Lui { .. } => "lui",
            Instruction::Daddi { .. } => "daddi",
            Instruction::Daddiu { .. } => "daddiu",
            Instruction::Beq { .. } => "beq",
            Instruction::Bne { .. } => "bne",
            Instruction::Blez { .. } => "blez",
            Instruction::Bgtz { .. } => "bgtz",
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
            Instruction::Lwc1 { .. } => "lwc1",
            Instruction::Swc1 { .. } => "swc1",
            Instruction::Ldc1 { .. } => "ldc1",
            Instruction::Sdc1 { .. } => "sdc1",
            Instruction::J { .. } => "j",
            Instruction::Jal { .. } => "jal",
            Instruction::Nop => "nop",
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Add { rd, rs, rt } => write!(f, "add {}, {}, {}", rd, rs, rt),
            Instruction::Addu { rd, rs, rt } => write!(f, "addu {}, {}, {}", rd, rs, rt),
            Instruction::Sub { rd, rs, rt } => write!(f, "sub {}, {}, {}", rd, rs, rt),
            Instruction::Subu { rd, rs, rt } => write!(f, "subu {}, {}, {}", rd, rs, rt),
            Instruction::And { rd, rs, rt } => write!(f, "and {}, {}, {}", rd, rs, rt),
            Instruction::Or { rd, rs, rt } => write!(f, "or {}, {}, {}", rd, rs, rt),
            Instruction::Xor { rd, rs, rt } => write!(f, "xor {}, {}, {}", rd, rs, rt),
            Instruction::Nor { rd, rs, rt } => write!(f, "nor {}, {}, {}", rd, rs, rt),
            Instruction::Slt { rd, rs, rt } => write!(f, "slt {}, {}, {}", rd, rs, rt),
            Instruction::Sltu { rd, rs, rt } => write!(f, "sltu {}, {}, {}", rd, rs, rt),
            Instruction::Sll { rd, rt, sa } => write!(f, "sll {}, {}, {}", rd, rt, sa),
            Instruction::Srl { rd, rt, sa } => write!(f, "srl {}, {}, {}", rd, rt, sa),
            Instruction::Sra { rd, rt, sa } => write!(f, "sra {}, {}, {}", rd, rt, sa),
            Instruction::Sllv { rd, rt, rs } => write!(f, "sllv {}, {}, {}", rd, rt, rs),
            Instruction::Srlv { rd, rt, rs } => write!(f, "srlv {}, {}, {}", rd, rt, rs),
            Instruction::Srav { rd, rt, rs } => write!(f, "srav {}, {}, {}", rd, rt, rs),
            Instruction::Mult { rs, rt } => write!(f, "mult {}, {}", rs, rt),
            Instruction::Multu { rs, rt } => write!(f, "multu {}, {}", rs, rt),
            Instruction::Div { rs, rt } => write!(f, "div {}, {}", rs, rt),
            Instruction::Divu { rs, rt } => write!(f, "divu {}, {}", rs, rt),
            Instruction::Mfhi { rd } => write!(f, "mfhi {}", rd),
            Instruction::Mflo { rd } => write!(f, "mflo {}", rd),
            Instruction::Dadd { rd, rs, rt } => write!(f, "dadd {}, {}, {}", rd, rs, rt),
            Instruction::Dsub { rd, rs, rt } => write!(f, "dsub {}, {}, {}", rd, rs, rt),
            Instruction::Daddu { rd, rs, rt } => write!(f, "daddu {}, {}, {}", rd, rs, rt),
            Instruction::Dsubu { rd, rs, rt } => write!(f, "dsubu {}, {}, {}", rd, rs, rt),
            Instruction::Dsll { rd, rt, sa } => write!(f, "dsll {}, {}, {}", rd, rt, sa),
            Instruction::Dsrl { rd, rt, sa } => write!(f, "dsrl {}, {}, {}", rd, rt, sa),
            Instruction::Dsra { rd, rt, sa } => write!(f, "dsra {}, {}, {}", rd, rt, sa),
            Instruction::Dsllv { rd, rt, rs } => write!(f, "dsllv {}, {}, {}", rd, rt, rs),
            Instruction::Dsrlv { rd, rt, rs } => write!(f, "dsrlv {}, {}, {}", rd, rt, rs),
            Instruction::Dsrav { rd, rt, rs } => write!(f, "dsrav {}, {}, {}", rd, rt, rs),
            Instruction::Dmult { rs, rt } => write!(f, "dmult {}, {}", rs, rt),
            Instruction::Dmultu { rs, rt } => write!(f, "dmultu {}, {}", rs, rt),
            Instruction::Ddiv { rs, rt } => write!(f, "ddiv {}, {}", rs, rt),
            Instruction::Ddivu { rs, rt } => write!(f, "ddivu {}, {}", rs, rt),
            Instruction::Movz { rd, rs, rt } => write!(f, "movz {}, {}, {}", rd, rs, rt),
            Instruction::Movn { rd, rs, rt } => write!(f, "movn {}, {}, {}", rd, rs, rt),
            Instruction::Jr { rs } => write!(f, "jr {}", rs),
            Instruction::Jalr { rd, rs } => write!(f, "jalr {}, {}", rd, rs),
            Instruction::Syscall { code } => write!(f, "syscall 0x{:x}", code),
            Instruction::Break { code } => write!(f, "break 0x{:x}", code),
            Instruction::Addi { rt, rs, imm } => write!(f, "addi {}, {}, {}", rt, rs, imm),
            Instruction::Addiu { rt, rs, imm } => write!(f, "addiu {}, {}, {}", rt, rs, imm),
            Instruction::Andi { rt, rs, imm } => write!(f, "andi {}, {}, 0x{:x}", rt, rs, imm),
            Instruction::Ori { rt, rs, imm } => write!(f, "ori {}, {}, 0x{:x}", rt, rs, imm),
            Instruction::Xori { rt, rs, imm } => write!(f, "xori {}, {}, 0x{:x}", rt, rs, imm),
            Instruction::Slti { rt, rs, imm } => write!(f, "slti {}, {}, {}", rt, rs, imm),
            Instruction::Sltiu { rt, rs, imm } => write!(f, "sltiu {}, {}, {}", rt, rs, imm),
            Instruction::Lui { rt, imm } => write!(f, "lui {}, 0x{:x}", rt, imm),
            Instruction::Daddi { rt, rs, imm } => write!(f, "daddi {}, {}, {}", rt, rs, imm),
            Instruction::Daddiu { rt, rs, imm } => write!(f, "daddiu {}, {}, {}", rt, rs, imm),
            Instruction::Beq { rs, rt, offset } => write!(f, "beq {}, {}, {:+}", rs, rt, offset),
            Instruction::Bne { rs, rt, offset } => write!(f, "bne {}, {}, {:+}", rs, rt, offset),
            Instruction::Blez { rs, offset } => write!(f, "blez {}, {:+}", rs, offset),
            Instruction::Bgtz { rs, offset } => write!(f, "bgtz {}, {:+}", rs, offset),
            Instruction::Lb { rt, base, offset } => write!(f, "lb {}, {}({})", rt, offset, base),
            Instruction::Lh { rt, base, offset } => write!(f, "lh {}, {}({})", rt, offset, base),
            Instruction::Lw { rt, base, offset } => write!(f, "lw {}, {}({})", rt, offset, base),
            Instruction::Ld { rt, base, offset } => write!(f, "ld {}, {}({})", rt, offset, base),
            Instruction::Lbu { rt, base, offset } => write!(f, "lbu {}, {}({})", rt, offset, base),
            Instruction::Lhu { rt, base, offset } => write!(f, "lhu {}, {}({})", rt, offset, base),
            Instruction::Lwu { rt, base, offset } => write!(f, "lwu {}, {}({})", rt, offset, base),
            Instruction::Sb { rt, base, offset } => write!(f, "sb {}, {}({})", rt, offset, base),
            Instruction::Sh { rt, base, offset } => write!(f, "sh {}, {}({})", rt, offset, base),
            Instruction::Sw { rt, base, offset } => write!(f, "sw {}, {}({})", rt, offset, base),
            Instruction::Sd { rt, base, offset } => write!(f, "sd {}, {}({})", rt, offset, base),
            Instruction::Lwc1 { ft, base, offset } => write!(f, "lwc1 {}, {}({})", ft, offset, base),
            Instruction::Swc1 { ft, base, offset } => write!(f, "swc1 {}, {}({})", ft, offset, base),
            Instruction::Ldc1 { ft, base, offset } => write!(f, "ldc1 {}, {}({})", ft, offset, base),
            Instruction::Sdc1 { ft, base, offset } => write!(f, "sdc1 {}, {}({})", ft, offset, base),
            Instruction::J { target } => write!(f, "j 0x{:08x}", target),
            Instruction::Jal { target } => write!(f, "jal 0x{:08x}", target),
            Instruction::Nop => write!(f, "nop"),
        }
    }
}

// ===========================================================================
// MIPS64 ELF64 Emission
// ===========================================================================

/// Build a minimal ELF64 binary for MIPS64 (big-endian) from raw code bytes.
///
/// Produces a static executable with a single LOAD segment containing the
/// `.text` section.  The ELF header and program header are written in
/// big-endian byte order.
fn build_minimal_mips64_elf(code: &[u8], base_addr: u64) -> Vec<u8> {
    // Layout: ELF header (64) | 1 program header (56) | code
    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let text_offset = elf_header_size + phdr_size;
    let text_size = code.len() as u64;
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity(text_offset as usize + code.len());

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(2); // ELFCLASS64
    elf.push(2); // ELFDATA2MSB (big-endian)
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0); // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields (big-endian) ---
    elf.extend_from_slice(&2u16.to_be_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&8u16.to_be_bytes()); // e_machine = EM_MIPS
    elf.extend_from_slice(&1u32.to_be_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_be_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_be_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_be_bytes()); // e_shoff (no section headers)
    // e_flags: MIPS flags — MIPS64 N64 ABI.
    // EF_MIPS_ARCH_64 = 0x6000 (MIPS64 ISA)
    // EF_MIPS_ABI_O64 = 0x00002000 (O64 ABI — closest to N64 in ELF flags)
    elf.extend_from_slice(&0x8000u32.to_be_bytes()); // e_flags (MIPS64)
    elf.extend_from_slice(&64u16.to_be_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_be_bytes()); // e_phentsize
    elf.extend_from_slice(&1u16.to_be_bytes()); // e_phnum
    elf.extend_from_slice(&64u16.to_be_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shstrndx

    // --- Program Header (single LOAD segment: PF_R | PF_X, big-endian) ---
    elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_be_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_be_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_be_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_be_bytes()); // p_filesz
    elf.extend_from_slice(&text_size.to_be_bytes()); // p_memsz
    elf.extend_from_slice(&16u64.to_be_bytes()); // p_align

    // --- Code section ---
    elf.extend_from_slice(code);

    elf
}

// ===========================================================================
// Mips64Backend
// ===========================================================================

/// MIPS 64-bit code generation backend.
///
/// Implements the `Backend` trait for MIPS64 (N64 ABI, big-endian).
/// Branch delay slots are handled by inserting a NOP after every branch or
/// jump instruction.
pub struct Mips64Backend {
    target_info: Mips64TargetInfo,
}

impl Mips64Backend {
    /// Create a new MIPS64 backend.
    pub fn new() -> Self {
        Self {
            target_info: Mips64TargetInfo,
        }
    }
}

impl Default for Mips64Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the stack frame size for an IR function on MIPS64.
///
/// Sums `Alloc` instruction sizes, adds 8 bytes for the $ra save slot,
/// and rounds up to 16-byte alignment.
fn mips64_compute_frame_size(func: &IRFunction) -> usize {
    let mut total: u32 = 8; // $ra save slot
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

/// Allocatable GPR registers for MIPS64, in priority order.
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
    Gpr::T9,
    // Return value registers (also caller-saved)
    Gpr::V0,
    Gpr::V1,
    // Argument registers (also caller-saved)
    Gpr::A0,
    Gpr::A1,
    Gpr::A2,
    Gpr::A3,
    // Callee-saved (require save/restore)
    Gpr::S0,
    Gpr::S1,
    Gpr::S2,
    Gpr::S3,
    Gpr::S4,
    Gpr::S5,
    Gpr::S6,
    Gpr::S7,
    // Frame pointer is last — we prefer not to use it
    Gpr::Fp,
];

/// Map from virtual register ID to a physical GPR using a simple linear scan.
///
/// Argument virtual registers are mapped to a0–a3 first.  Remaining virtual
/// registers are assigned from the pool of allocatable registers.
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

    // Fallback: use t9 as a scratch register (will cause issues, but shouldn't
    // happen with reasonable register pressure).
    Gpr::T9
}

/// Helper: extract the virtual register ID from an IRValue, if it is a register.
fn vreg_id(val: &IRValue) -> u32 {
    match val {
        IRValue::Register(id) => *id,
        _ => 0,
    }
}

/// Lower a BinOpKind + operands to MIPS64 instructions.
fn lower_binop(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();
    let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
    let lhs_reg = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
    let rhs_reg = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);

    let mips_inst = match op {
        BinOpKind::Add => Instruction::Daddu {
            rd: dst_reg,
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::Sub => Instruction::Dsubu {
            rd: dst_reg,
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::Mul => Instruction::Dmult {
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::SDiv => Instruction::Ddiv {
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::UDiv => Instruction::Ddivu {
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::SRem => Instruction::Ddiv {
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::URem => Instruction::Ddivu {
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::And => Instruction::And {
            rd: dst_reg,
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::Or => Instruction::Or {
            rd: dst_reg,
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::Xor => Instruction::Xor {
            rd: dst_reg,
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::Shl => Instruction::Dsllv {
            rd: dst_reg,
            rt: lhs_reg,
            rs: rhs_reg,
        },
        BinOpKind::ShrL => Instruction::Dsrlv {
            rd: dst_reg,
            rt: lhs_reg,
            rs: rhs_reg,
        },
        BinOpKind::ShrA => Instruction::Dsrav {
            rd: dst_reg,
            rt: lhs_reg,
            rs: rhs_reg,
        },
        BinOpKind::SLt => Instruction::Slt {
            rd: dst_reg,
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::SLe => {
            // slt rd, rhs, lhs ; xori rd, rd, 1  (rd = !(rhs < lhs) = lhs <= rhs)
            result.push(AllocatedInstruction {
                opcode: "slt".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Slt {
                    rd: dst_reg,
                    rs: rhs_reg,
                    rt: lhs_reg,
                }
                .encode()
                .to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "xori".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Xori {
                    rt: dst_reg,
                    rs: dst_reg,
                    imm: 1,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
        BinOpKind::SGt => {
            // slt rd, rhs, lhs (rd = rhs < lhs = lhs > rhs)
            result.push(AllocatedInstruction {
                opcode: "slt".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Slt {
                    rd: dst_reg,
                    rs: rhs_reg,
                    rt: lhs_reg,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
        BinOpKind::SGe => {
            // slt rd, lhs, rhs ; xori rd, rd, 1 (rd = !(lhs < rhs) = lhs >= rhs)
            result.push(AllocatedInstruction {
                opcode: "slt".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Slt {
                    rd: dst_reg,
                    rs: lhs_reg,
                    rt: rhs_reg,
                }
                .encode()
                .to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "xori".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Xori {
                    rt: dst_reg,
                    rs: dst_reg,
                    imm: 1,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
        BinOpKind::ULt => Instruction::Sltu {
            rd: dst_reg,
            rs: lhs_reg,
            rt: rhs_reg,
        },
        BinOpKind::ULe => {
            result.push(AllocatedInstruction {
                opcode: "sltu".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Sltu {
                    rd: dst_reg,
                    rs: rhs_reg,
                    rt: lhs_reg,
                }
                .encode()
                .to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "xori".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Xori {
                    rt: dst_reg,
                    rs: dst_reg,
                    imm: 1,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
        BinOpKind::UGt => {
            result.push(AllocatedInstruction {
                opcode: "sltu".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Sltu {
                    rd: dst_reg,
                    rs: rhs_reg,
                    rt: lhs_reg,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
        BinOpKind::UGe => {
            result.push(AllocatedInstruction {
                opcode: "sltu".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Sltu {
                    rd: dst_reg,
                    rs: lhs_reg,
                    rt: rhs_reg,
                }
                .encode()
                .to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "xori".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Xori {
                    rt: dst_reg,
                    rs: dst_reg,
                    imm: 1,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
        BinOpKind::Eq => {
            // xor rd, lhs, rhs ; sltiu rd, rd, 1 (rd = (xor result) < 1)
            result.push(AllocatedInstruction {
                opcode: "xor".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Xor {
                    rd: dst_reg,
                    rs: lhs_reg,
                    rt: rhs_reg,
                }
                .encode()
                .to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "sltiu".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Sltiu {
                    rt: dst_reg,
                    rs: dst_reg,
                    imm: 1,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
        BinOpKind::Ne => {
            // xor rd, lhs, rhs ; sltu rd, $zero, rd (rd = (0 < xor_result))
            result.push(AllocatedInstruction {
                opcode: "xor".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Xor {
                    rd: dst_reg,
                    rs: lhs_reg,
                    rt: rhs_reg,
                }
                .encode()
                .to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "sltu".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: Instruction::Sltu {
                    rd: dst_reg,
                    rs: Gpr::Zero,
                    rt: dst_reg,
                }
                .encode()
                .to_vec(),
            });
            return result;
        }
    };

    let encoded = mips_inst.encode();
    let reads = vec![
        PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
        PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
    ];
    let writes = vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())];

    // mul/div write to HI/LO, then we need mflo (and possibly mfhi for rem)
    if matches!(
        op,
        BinOpKind::Mul
            | BinOpKind::SDiv
            | BinOpKind::UDiv
            | BinOpKind::SRem
            | BinOpKind::URem
    ) {
        result.push(AllocatedInstruction {
            opcode: mips_inst.mnemonic().to_string(),
            reads,
            writes: vec![
                PhysicalReg::new(RegClass::Special, 0), // HI
                PhysicalReg::new(RegClass::Special, 1), // LO
            ],
            encoded: encoded.to_vec(),
        });
        if matches!(op, BinOpKind::SRem | BinOpKind::URem) {
            // mfhi to get remainder
            let mfhi = Instruction::Mfhi { rd: dst_reg };
            result.push(AllocatedInstruction {
                opcode: "mfhi".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Special, 0)],
                writes: writes.clone(),
                encoded: mfhi.encode().to_vec(),
            });
        } else {
            // mflo to get quotient/product
            let mflo = Instruction::Mflo { rd: dst_reg };
            result.push(AllocatedInstruction {
                opcode: "mflo".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Special, 1)],
                writes: writes.clone(),
                encoded: mflo.encode().to_vec(),
            });
        }
        return result;
    }

    result.push(AllocatedInstruction {
        opcode: mips_inst.mnemonic().to_string(),
        reads,
        writes,
        encoded: encoded.to_vec(),
    });
    result
}

/// Lower an IR instruction to a sequence of MIPS64 `AllocatedInstruction`s,
/// handling branch delay slots by inserting NOPs after branches/jumps.
///
/// `frame_size` is the stack frame size (0 if not yet known), used by the
/// `Ret` handler to generate a proper epilogue that restores $ra and
/// deallocates the frame.
fn lower_ir_instr(
    instr: &IRInstr,
    vreg_map: &mut std::collections::HashMap<u32, Gpr>,
    frame_size: usize,
) -> Vec<AllocatedInstruction> {
    let mut result = Vec::new();

    match instr {
        IRInstr::BinOp {
            op,
            dst,
            lhs,
            rhs,
        } => {
            result.extend(lower_binop(op, dst, lhs, rhs, vreg_map));
        }

        IRInstr::Add { dst, lhs, rhs } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let lhs_reg = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let rhs_reg = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            let add = Instruction::Daddu {
                rd: dst_reg,
                rs: lhs_reg,
                rt: rhs_reg,
            };
            result.push(AllocatedInstruction {
                opcode: "daddu".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: add.encode().to_vec(),
            });
        }

        IRInstr::Sub { dst, lhs, rhs } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let lhs_reg = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let rhs_reg = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            let sub = Instruction::Dsubu {
                rd: dst_reg,
                rs: lhs_reg,
                rt: rhs_reg,
            };
            result.push(AllocatedInstruction {
                opcode: "dsubu".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: sub.encode().to_vec(),
            });
        }

        IRInstr::Mul { dst, lhs, rhs } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let lhs_reg = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let rhs_reg = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            let dmult = Instruction::Dmult {
                rs: lhs_reg,
                rt: rhs_reg,
            };
            result.push(AllocatedInstruction {
                opcode: "dmult".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![
                    PhysicalReg::new(RegClass::Special, 0),
                    PhysicalReg::new(RegClass::Special, 1),
                ],
                encoded: dmult.encode().to_vec(),
            });
            let mflo = Instruction::Mflo { rd: dst_reg };
            result.push(AllocatedInstruction {
                opcode: "mflo".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Special, 1)],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: mflo.encode().to_vec(),
            });
        }

        IRInstr::Div { dst, lhs, rhs } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let lhs_reg = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let rhs_reg = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            let ddiv = Instruction::Ddiv {
                rs: lhs_reg,
                rt: rhs_reg,
            };
            result.push(AllocatedInstruction {
                opcode: "ddiv".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, lhs_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, rhs_reg.encoding()),
                ],
                writes: vec![
                    PhysicalReg::new(RegClass::Special, 0),
                    PhysicalReg::new(RegClass::Special, 1),
                ],
                encoded: ddiv.encode().to_vec(),
            });
            let mflo = Instruction::Mflo { rd: dst_reg };
            result.push(AllocatedInstruction {
                opcode: "mflo".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Special, 1)],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: mflo.encode().to_vec(),
            });
        }

        IRInstr::Load { dst, addr } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let addr_reg = map_vreg_to_gpr(vreg_id(addr), None, vreg_map);
            let ld = Instruction::Ld {
                rt: dst_reg,
                base: addr_reg,
                offset: 0,
            };
            result.push(AllocatedInstruction {
                opcode: "ld".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, addr_reg.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: ld.encode().to_vec(),
            });
        }

        IRInstr::Store { value, addr } => {
            let val_reg = map_vreg_to_gpr(vreg_id(value), None, vreg_map);
            let addr_reg = map_vreg_to_gpr(vreg_id(addr), None, vreg_map);
            let sd = Instruction::Sd {
                rt: val_reg,
                base: addr_reg,
                offset: 0,
            };
            result.push(AllocatedInstruction {
                opcode: "sd".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, addr_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, val_reg.encoding()),
                ],
                writes: vec![],
                encoded: sd.encode().to_vec(),
            });
        }

        IRInstr::Alloc { dst, size } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            // Adjust stack pointer: daddiu $sp, $sp, -size
            let neg_size = -(*size as i32);
            let daddiu = Instruction::Daddiu {
                rt: Gpr::Sp,
                rs: Gpr::Sp,
                imm: neg_size,
            };
            result.push(AllocatedInstruction {
                opcode: "daddiu".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                encoded: daddiu.encode().to_vec(),
            });
            // Copy updated $sp to dst: daddu dst, $sp, $zero
            if dst_reg != Gpr::Sp {
                let mov_sp = Instruction::Daddu {
                    rd: dst_reg,
                    rs: Gpr::Sp,
                    rt: Gpr::Zero,
                };
                result.push(AllocatedInstruction {
                    opcode: "daddu".to_string(),
                    reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                    writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    encoded: mov_sp.encode().to_vec(),
                });
            }
        }

        IRInstr::Ret { values } => {
            // Move the first return value to $v0
            if let Some(val) = values.first() {
                let val_reg = map_vreg_to_gpr(vreg_id(val), None, vreg_map);
                if val_reg != Gpr::V0 {
                    let mov = Instruction::Daddu {
                        rd: Gpr::V0,
                        rs: val_reg,
                        rt: Gpr::Zero,
                    };
                    result.push(AllocatedInstruction {
                        opcode: "daddu".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, val_reg.encoding())],
                        writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::V0.encoding())],
                        encoded: mov.encode().to_vec(),
                    });
                }
            }
            // Epilogue: restore $ra and deallocate frame
            if frame_size > 0 {
                // ld $ra, frame_size-8($sp)
                let ra_offset = (frame_size - 8) as i32;
                let load_ra = Instruction::Ld {
                    rt: Gpr::Ra,
                    base: Gpr::Sp,
                    offset: ra_offset,
                };
                result.push(AllocatedInstruction {
                    opcode: "ld".to_string(),
                    reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                    writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
                    encoded: load_ra.encode().to_vec(),
                });
                // daddiu $sp, $sp, frame_size
                let frame_imm = frame_size as i32;
                let dealloc = Instruction::Daddiu {
                    rt: Gpr::Sp,
                    rs: Gpr::Sp,
                    imm: frame_imm,
                };
                result.push(AllocatedInstruction {
                    opcode: "daddiu".to_string(),
                    reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                    writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
                    encoded: dealloc.encode().to_vec(),
                });
            }
            // jr $ra
            let jr = Instruction::Jr { rs: Gpr::Ra };
            result.push(AllocatedInstruction {
                opcode: "jr".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
                writes: vec![],
                encoded: jr.encode().to_vec(),
            });
            // Branch delay slot NOP
            result.push(AllocatedInstruction {
                opcode: "nop".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: encode_nop().to_vec(),
            });
        }

        IRInstr::Cast { dst, src, .. } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let src_reg = map_vreg_to_gpr(vreg_id(src), None, vreg_map);
            if dst_reg != src_reg {
                let mov = Instruction::Daddu {
                    rd: dst_reg,
                    rs: src_reg,
                    rt: Gpr::Zero,
                };
                result.push(AllocatedInstruction {
                    opcode: "daddu".to_string(),
                    reads: vec![PhysicalReg::new(RegClass::Gpr, src_reg.encoding())],
                    writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    encoded: mov.encode().to_vec(),
                });
            }
        }

        IRInstr::Select {
            dst,
            cond,
            true_val,
            false_val,
        } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let cond_reg = map_vreg_to_gpr(vreg_id(cond), None, vreg_map);
            let true_reg = map_vreg_to_gpr(vreg_id(true_val), None, vreg_map);
            let false_reg = map_vreg_to_gpr(vreg_id(false_val), None, vreg_map);

            // First move false to dst, then conditionally move true
            if dst_reg != false_reg {
                let mov_false = Instruction::Daddu {
                    rd: dst_reg,
                    rs: false_reg,
                    rt: Gpr::Zero,
                };
                result.push(AllocatedInstruction {
                    opcode: "daddu".to_string(),
                    reads: vec![PhysicalReg::new(RegClass::Gpr, false_reg.encoding())],
                    writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                    encoded: mov_false.encode().to_vec(),
                });
            }
            // movn dst, true, cond (if cond != 0, dst = true)
            let movn = Instruction::Movn {
                rd: dst_reg,
                rs: true_reg,
                rt: cond_reg,
            };
            result.push(AllocatedInstruction {
                opcode: "movn".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, true_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, cond_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: movn.encode().to_vec(),
            });
        }

        IRInstr::Offset { dst, base, offset } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let base_reg = map_vreg_to_gpr(vreg_id(base), None, vreg_map);
            let offset_reg = map_vreg_to_gpr(vreg_id(offset), None, vreg_map);
            let daddu = Instruction::Daddu {
                rd: dst_reg,
                rs: base_reg,
                rt: offset_reg,
            };
            result.push(AllocatedInstruction {
                opcode: "daddu".to_string(),
                reads: vec![
                    PhysicalReg::new(RegClass::Gpr, base_reg.encoding()),
                    PhysicalReg::new(RegClass::Gpr, offset_reg.encoding()),
                ],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: daddu.encode().to_vec(),
            });
        }

        IRInstr::GetAddress { dst, .. } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            // Placeholder: load address using lui
            let lui = Instruction::Lui {
                rt: dst_reg,
                imm: 0,
            };
            result.push(AllocatedInstruction {
                opcode: "lui".to_string(),
                reads: vec![],
                writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                encoded: lui.encode().to_vec(),
            });
        }

        IRInstr::Cmp { kind, dst, lhs, rhs } => {
            let _dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let _lhs_reg = map_vreg_to_gpr(vreg_id(lhs), None, vreg_map);
            let _rhs_reg = map_vreg_to_gpr(vreg_id(rhs), None, vreg_map);
            let binop_kind = match kind {
                CmpKind::Eq => BinOpKind::Eq,
                CmpKind::Ne => BinOpKind::Ne,
                CmpKind::SLt => BinOpKind::SLt,
                CmpKind::SLe => BinOpKind::SLe,
                CmpKind::SGt => BinOpKind::SGt,
                CmpKind::SGe => BinOpKind::SGe,
                CmpKind::ULt => BinOpKind::ULt,
                CmpKind::ULe => BinOpKind::ULe,
                CmpKind::UGt => BinOpKind::UGt,
                CmpKind::UGe => BinOpKind::UGe,
            };
            result.extend(lower_binop(&binop_kind, dst, lhs, rhs, vreg_map));
        }

        IRInstr::UnaryOp { op, dst, operand } => {
            let dst_reg = map_vreg_to_gpr(vreg_id(dst), None, vreg_map);
            let src_reg = map_vreg_to_gpr(vreg_id(operand), None, vreg_map);
            match op {
                UnaryOpKind::Neg => {
                    let neg = Instruction::Dsubu { rd: dst_reg, rs: Gpr::Zero, rt: src_reg };
                    result.push(AllocatedInstruction {
                        opcode: "dsubu".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, src_reg.encoding())],
                        writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                        encoded: neg.encode().to_vec(),
                    });
                }
                UnaryOpKind::Not => {
                    // nor dst, src, $zero → dst = ~(src | 0) = ~src
                    let not = Instruction::Nor { rd: dst_reg, rs: src_reg, rt: Gpr::Zero };
                    result.push(AllocatedInstruction {
                        opcode: "nor".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, src_reg.encoding())],
                        writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                        encoded: not.encode().to_vec(),
                    });
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    let mov = Instruction::Daddu { rd: dst_reg, rs: src_reg, rt: Gpr::Zero };
                    result.push(AllocatedInstruction {
                        opcode: "daddu".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, src_reg.encoding())],
                        writes: vec![PhysicalReg::new(RegClass::Gpr, dst_reg.encoding())],
                        encoded: mov.encode().to_vec(),
                    });
                }
            }
        }

        IRInstr::Call { dst, func: _, args } => {
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    let a = map_vreg_to_gpr(vreg_id(arg), None, vreg_map);
                    if a != arg_reg {
                        let mov = Instruction::Daddu { rd: arg_reg, rs: a, rt: Gpr::Zero };
                        result.push(AllocatedInstruction {
                            opcode: "daddu".to_string(),
                            reads: vec![PhysicalReg::new(RegClass::Gpr, a.encoding())],
                            writes: vec![PhysicalReg::new(RegClass::Gpr, arg_reg.encoding())],
                            encoded: mov.encode().to_vec(),
                        });
                    }
                }
            }
            let jal = Instruction::Jal { target: 0 };
            result.push(AllocatedInstruction {
                opcode: "jal".to_string(),
                reads: vec![],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
                encoded: jal.encode().to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "nop".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: encode_nop().to_vec(),
            });
            if let Some(d) = dst {
                let d_reg = map_vreg_to_gpr(vreg_id(d), None, vreg_map);
                if d_reg != Gpr::V0 {
                    let mov = Instruction::Daddu { rd: d_reg, rs: Gpr::V0, rt: Gpr::Zero };
                    result.push(AllocatedInstruction {
                        opcode: "daddu".to_string(),
                        reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::V0.encoding())],
                        writes: vec![PhysicalReg::new(RegClass::Gpr, d_reg.encoding())],
                        encoded: mov.encode().to_vec(),
                    });
                }
            }
        }

        IRInstr::Branch { target: _ } => {
            let b = Instruction::Beq { rs: Gpr::Zero, rt: Gpr::Zero, offset: 0 };
            result.push(AllocatedInstruction {
                opcode: "beq".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: b.encode().to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "nop".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: encode_nop().to_vec(),
            });
        }

        IRInstr::CondBranch { cond, true_target: _, false_target: _ } => {
            let c = map_vreg_to_gpr(vreg_id(cond), None, vreg_map);
            let bnez = Instruction::Bne { rs: c, rt: Gpr::Zero, offset: 8 };
            result.push(AllocatedInstruction {
                opcode: "bne".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, c.encoding())],
                writes: vec![],
                encoded: bnez.encode().to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "nop".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: encode_nop().to_vec(),
            });
            let beq = Instruction::Beq { rs: Gpr::Zero, rt: Gpr::Zero, offset: 0 };
            result.push(AllocatedInstruction {
                opcode: "beq".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: beq.encode().to_vec(),
            });
            result.push(AllocatedInstruction {
                opcode: "nop".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: encode_nop().to_vec(),
            });
        }

        IRInstr::Free { ptr: _ } => {
            // Free is heap deallocation; the IR specifies it should be lowered
            // to a runtime call.  Until a runtime is available, emit a break
            // instruction with code 0xFF to trap if accidentally executed.
            let brk = Instruction::Break { code: 0xFF };
            result.push(AllocatedInstruction {
                opcode: "break".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: brk.encode().to_vec(),
            });
        }

        IRInstr::Phi { .. } => {
            // Phi nodes should be eliminated by SSA deconstruction before
            // instruction selection.  Emit a NOP as a safety net.
            result.push(AllocatedInstruction {
                opcode: "nop".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: encode_nop().to_vec(),
            });
        }

        #[allow(unreachable_patterns)]
        _ => {
            // Unknown/unhandled instruction: emit a NOP
            // Note: all IRInstr variants are handled above, so this is a safeguard.
            #[allow(unreachable_patterns)]
            result.push(AllocatedInstruction {
                opcode: "nop".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: encode_nop().to_vec(),
            });
        }
    }

    result
}

impl Backend for Mips64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        let func_name = func.name.clone();
        let frame_size = mips64_compute_frame_size(func);

        // Build a mapping from virtual register IDs to physical GPRs.
        let mut vreg_map: std::collections::HashMap<u32, Gpr> = std::collections::HashMap::new();

        // Map function parameters to argument registers.
        for (i, _param) in func.params.iter().enumerate() {
            if let Some(reg) = Gpr::arg_register(i) {
                vreg_map.insert(i as u32, reg);
            }
        }

        // Lower each IR instruction to MIPS64 AllocatedInstructions.
        let mut instructions: Vec<AllocatedInstruction> = Vec::new();

        // Prologue: daddiu $sp, $sp, -frame_size
        let frame_imm = -(frame_size as i32);
        let prologue = Instruction::Daddiu {
            rt: Gpr::Sp,
            rs: Gpr::Sp,
            imm: frame_imm,
        };
        instructions.push(AllocatedInstruction {
            opcode: "daddiu".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())],
            encoded: prologue.encode().to_vec(),
        });

        // Save $ra: sd $ra, frame_size-8($sp)
        let ra_offset = (frame_size - 8) as i32;
        let save_ra = Instruction::Sd {
            rt: Gpr::Ra,
            base: Gpr::Sp,
            offset: ra_offset,
        };
        instructions.push(AllocatedInstruction {
            opcode: "sd".to_string(),
            reads: vec![
                PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding()),
                PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding()),
            ],
            writes: vec![],
            encoded: save_ra.encode().to_vec(),
        });

        // Lower IR instructions.
        for block in &func.blocks {
            for instr in &block.instructions {
                let lowered = lower_ir_instr(instr, &mut vreg_map, frame_size);
                instructions.extend(lowered);
            }
        }

        let code_size = instructions.len() * 4;

        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions,
                code_offset: 0,
            }],
            frame_size,
            callee_saved: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding())],
            spill_slots: 0,
            code_size,
            relocations: Vec::new(),
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

        // Wrap in a minimal ELF64 binary for MIPS64 (big-endian).
        Ok(build_minimal_mips64_elf(&all_code, 0x120000000))
    }

    fn return_stub(&self) -> Vec<u8> {
        // jr $ra ; nop (delay slot)
        let mut code = Vec::with_capacity(8);
        let jr = Instruction::Jr { rs: Gpr::Ra };
        code.extend_from_slice(&jr.encode());
        code.extend_from_slice(&encode_nop());
        code
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // MIPS64 trampoline: load 64-bit address into a register and jump.
        // lui $t9, upper16 ; daddiu $t9, $t9, lower16 ; jr $t9 ; nop
        //
        // For a full 64-bit address, we need:
        //   lui   $t9, %highest(entry_addr)
        //   daddiu $t9, $t9, %higher(entry_addr)
        //   dsll  $t9, $t9, 16
        //   daddiu $t9, $t9, %hi(entry_addr)
        //   dsll  $t9, $t9, 16
        //   daddiu $t9, $t9, %lo(entry_addr)
        //   jr    $t9
        //   nop
        let addr = entry_addr;
        let highest = ((addr >> 48) & 0xFFFF) as u32;
        let higher = ((addr >> 32) & 0xFFFF) as u32;
        let hi = ((addr >> 16) & 0xFFFF) as u32;
        let lo = (addr & 0xFFFF) as u32;

        let mut code = Vec::with_capacity(32); // 8 instructions * 4 bytes

        // lui $t9, highest
        code.extend_from_slice(
            &Instruction::Lui {
                rt: Gpr::T9,
                imm: highest,
            }
            .encode(),
        );
        // daddiu $t9, $t9, higher
        code.extend_from_slice(
            &Instruction::Daddiu {
                rt: Gpr::T9,
                rs: Gpr::T9,
                imm: higher as i32,
            }
            .encode(),
        );
        // dsll $t9, $t9, 16
        code.extend_from_slice(
            &Instruction::Dsll {
                rd: Gpr::T9,
                rt: Gpr::T9,
                sa: 16,
            }
            .encode(),
        );
        // daddiu $t9, $t9, hi
        code.extend_from_slice(
            &Instruction::Daddiu {
                rt: Gpr::T9,
                rs: Gpr::T9,
                imm: hi as i32,
            }
            .encode(),
        );
        // dsll $t9, $t9, 16
        code.extend_from_slice(
            &Instruction::Dsll {
                rd: Gpr::T9,
                rt: Gpr::T9,
                sa: 16,
            }
            .encode(),
        );
        // daddiu $t9, $t9, lo
        code.extend_from_slice(
            &Instruction::Daddiu {
                rt: Gpr::T9,
                rs: Gpr::T9,
                imm: lo as i32,
            }
            .encode(),
        );
        // jr $t9
        code.extend_from_slice(&Instruction::Jr { rs: Gpr::T9 }.encode());
        // nop (delay slot)
        code.extend_from_slice(&encode_nop());

        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        // Simple hex-based disassembler for MIPS64 (4-byte fixed-width,
        // big-endian instructions).
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
        "mips64"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ── Gpr tests ─────────────────────────────────────────────────────

    #[test]
    fn test_gpr_encoding() {
        assert_eq!(Gpr::Zero.encoding(), 0);
        assert_eq!(Gpr::At.encoding(), 1);
        assert_eq!(Gpr::V0.encoding(), 2);
        assert_eq!(Gpr::A0.encoding(), 4);
        assert_eq!(Gpr::T0.encoding(), 8);
        assert_eq!(Gpr::S0.encoding(), 16);
        assert_eq!(Gpr::Sp.encoding(), 29);
        assert_eq!(Gpr::Fp.encoding(), 30);
        assert_eq!(Gpr::Ra.encoding(), 31);
    }

    #[test]
    fn test_gpr_allocatable() {
        // Not allocatable
        assert!(!Gpr::Zero.is_allocatable());
        assert!(!Gpr::At.is_allocatable());
        assert!(!Gpr::K0.is_allocatable());
        assert!(!Gpr::K1.is_allocatable());
        assert!(!Gpr::Gp.is_allocatable());
        assert!(!Gpr::Sp.is_allocatable());
        assert!(!Gpr::Ra.is_allocatable());
        // Allocatable
        assert!(Gpr::V0.is_allocatable());
        assert!(Gpr::A0.is_allocatable());
        assert!(Gpr::T0.is_allocatable());
        assert!(Gpr::S0.is_allocatable());
        assert!(Gpr::Fp.is_allocatable());
    }

    #[test]
    fn test_gpr_callee_saved() {
        assert!(Gpr::S0.is_callee_saved());
        assert!(Gpr::S7.is_callee_saved());
        assert!(Gpr::Fp.is_callee_saved());
        assert!(!Gpr::T0.is_callee_saved());
        assert!(!Gpr::A0.is_callee_saved());
        assert!(!Gpr::V0.is_callee_saved());
    }

    #[test]
    fn test_gpr_arg_reg() {
        assert!(Gpr::A0.is_arg_reg());
        assert!(Gpr::A3.is_arg_reg());
        assert!(!Gpr::T0.is_arg_reg()); // T0 is not an arg register
        assert!(!Gpr::T0.is_arg_reg());
        assert!(!Gpr::V0.is_arg_reg());
    }

    #[test]
    fn test_gpr_asm_name() {
        assert_eq!(Gpr::Zero.asm_name(), "$zero");
        assert_eq!(Gpr::Ra.asm_name(), "$ra");
        assert_eq!(Gpr::Sp.asm_name(), "$sp");
        assert_eq!(Gpr::A0.asm_name(), "$a0");
        assert_eq!(Gpr::T0.asm_name(), "$t0");
        assert_eq!(Gpr::S0.asm_name(), "$s0");
    }

    #[test]
    fn test_gpr_arg_register() {
        assert_eq!(Gpr::arg_register(0), Some(Gpr::A0));
        assert_eq!(Gpr::arg_register(3), Some(Gpr::A3));
        assert_eq!(Gpr::arg_register(4), None);
    }

    // ── Fpr tests ─────────────────────────────────────────────────────

    #[test]
    fn test_fpr_encoding() {
        assert_eq!(Fpr::F0.encoding(), 0);
        assert_eq!(Fpr::F12.encoding(), 12);
        assert_eq!(Fpr::F31.encoding(), 31);
    }

    #[test]
    fn test_fpr_callee_saved() {
        assert!(Fpr::F20.is_callee_saved());
        assert!(Fpr::F31.is_callee_saved());
        assert!(!Fpr::F0.is_callee_saved());
        assert!(!Fpr::F19.is_callee_saved());
    }

    #[test]
    fn test_fpr_arg_reg() {
        assert!(Fpr::F12.is_arg_reg());
        assert!(Fpr::F19.is_arg_reg());
        assert!(!Fpr::F0.is_arg_reg());
        assert!(!Fpr::F20.is_arg_reg());
    }

    #[test]
    fn test_fpr_arg_register() {
        assert_eq!(Fpr::arg_register(0), Some(Fpr::F12));
        assert_eq!(Fpr::arg_register(7), Some(Fpr::F19));
        assert_eq!(Fpr::arg_register(8), None);
    }

    // ── Instruction encoding tests ────────────────────────────────────

    #[test]
    fn test_nop_encoding() {
        let nop = Instruction::Nop;
        let encoded = nop.encode();
        assert_eq!(encoded, [0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_add_encoding() {
        // add $s0, $t0, $t1 => opcode=0, rs=$t0(8), rt=$t1(9), rd=$s0(16), sa=0, funct=0x20
        let add = Instruction::Add {
            rd: Gpr::S0,
            rs: Gpr::T0,
            rt: Gpr::T1,
        };
        let encoded = add.encode();
        let expected: u32 = (0x00 << 26) | (8 << 21) | (9 << 16) | (16 << 11) | (0 << 6) | 0x20;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_addu_encoding() {
        let addu = Instruction::Addu {
            rd: Gpr::V0,
            rs: Gpr::A0,
            rt: Gpr::A1,
        };
        let encoded = addu.encode();
        let expected: u32 = (0 << 26) | (4 << 21) | (5 << 16) | (2 << 11) | (0 << 6) | 0x21;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_lui_encoding() {
        // lui $t9, 0x1000 => opcode=0x0F, rs=0, rt=$t9(25), imm=0x1000
        let lui = Instruction::Lui {
            rt: Gpr::T9,
            imm: 0x1000,
        };
        let encoded = lui.encode();
        let expected: u32 = (0x0F << 26) | (0 << 21) | (25 << 16) | 0x1000;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_beq_encoding() {
        // beq $a0, $a1, 8 (offset 8 bytes = 2 words) => opcode=0x04, rs=4, rt=5, imm=2
        let beq = Instruction::Beq {
            rs: Gpr::A0,
            rt: Gpr::A1,
            offset: 8,
        };
        let encoded = beq.encode();
        let expected: u32 = (0x04 << 26) | (4 << 21) | (5 << 16) | 2;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_ld_encoding() {
        // ld $v0, 16($sp) => opcode=0x37, rs=$sp(29), rt=$v0(2), imm=16
        let ld = Instruction::Ld {
            rt: Gpr::V0,
            base: Gpr::Sp,
            offset: 16,
        };
        let encoded = ld.encode();
        let expected: u32 = (0x37 << 26) | (29 << 21) | (2 << 16) | 16;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_jr_encoding() {
        // jr $ra => opcode=0, rs=$ra(31), rt=0, rd=0, sa=0, funct=0x08
        let jr = Instruction::Jr { rs: Gpr::Ra };
        let encoded = jr.encode();
        let expected: u32 = (0 << 26) | (31 << 21) | (0 << 16) | (0 << 11) | (0 << 6) | 0x08;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_sll_encoding() {
        // sll $v0, $v1, 5 => opcode=0, rs=0, rt=$v1(3), rd=$v0(2), sa=5, funct=0x00
        let sll = Instruction::Sll {
            rd: Gpr::V0,
            rt: Gpr::V1,
            sa: 5,
        };
        let encoded = sll.encode();
        let expected: u32 = (0 << 26) | (0 << 21) | (3 << 16) | (2 << 11) | (5 << 6) | 0x00;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_dsll_encoding() {
        let dsll = Instruction::Dsll {
            rd: Gpr::T9,
            rt: Gpr::T9,
            sa: 16,
        };
        let encoded = dsll.encode();
        let expected: u32 = (0 << 26) | (0 << 21) | (25 << 16) | (25 << 11) | (16 << 6) | 0x38;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    #[test]
    fn test_jal_encoding() {
        // jal with target field = 0x400 (word address of 0x1000)
        // opcode=0x03, target=0x400
        let jal = Instruction::Jal { target: 0x400 };
        let encoded = jal.encode();
        let expected: u32 = (0x03 << 26) | 0x400;
        assert_eq!(u32::from_be_bytes(encoded), expected);
    }

    // ── Branch delay slot tests ───────────────────────────────────────

    #[test]
    fn test_has_delay_slot_branches() {
        assert!(Instruction::Beq {
            rs: Gpr::A0,
            rt: Gpr::Zero,
            offset: 8
        }
        .has_delay_slot());
        assert!(Instruction::Bne {
            rs: Gpr::A0,
            rt: Gpr::Zero,
            offset: 8
        }
        .has_delay_slot());
        assert!(Instruction::Blez {
            rs: Gpr::A0,
            offset: 8
        }
        .has_delay_slot());
        assert!(Instruction::Bgtz {
            rs: Gpr::A0,
            offset: 8
        }
        .has_delay_slot());
    }

    #[test]
    fn test_has_delay_slot_jumps() {
        assert!(Instruction::Jr { rs: Gpr::Ra }.has_delay_slot());
        assert!(Instruction::Jalr {
            rd: Gpr::Ra,
            rs: Gpr::T9
        }
        .has_delay_slot());
        assert!(Instruction::J { target: 0 }.has_delay_slot());
        assert!(Instruction::Jal { target: 0 }.has_delay_slot());
    }

    #[test]
    fn test_no_delay_slot_non_branches() {
        assert!(!Instruction::Add {
            rd: Gpr::V0,
            rs: Gpr::A0,
            rt: Gpr::A1
        }
        .has_delay_slot());
        assert!(!Instruction::Ld {
            rt: Gpr::V0,
            base: Gpr::Sp,
            offset: 0
        }
        .has_delay_slot());
        assert!(!Instruction::Nop.has_delay_slot());
        assert!(!Instruction::Slt {
            rd: Gpr::V0,
            rs: Gpr::A0,
            rt: Gpr::A1
        }
        .has_delay_slot());
    }

    // ── Backend tests ─────────────────────────────────────────────────

    #[test]
    fn test_backend_target_info() {
        let backend = Mips64Backend::new();
        let info = backend.target_info();
        assert_eq!(info.isa_name(), "mips64");
        assert_eq!(info.elf_machine_type(), 8);
        assert!(info.has_branch_delay_slots());
        assert_eq!(info.endianness(), crate::backend::Endianness::Big);
        assert_eq!(info.pointer_width(), 8);
        assert_eq!(info.calling_convention_name(), "n64");
        assert_eq!(info.num_int_arg_regs(), 4);
        assert_eq!(info.num_fp_arg_regs(), 8);
    }

    #[test]
    fn test_return_stub_has_delay_slot_nop() {
        let backend = Mips64Backend::new();
        let stub = backend.return_stub();
        // jr $ra (4 bytes) + nop (4 bytes) = 8 bytes
        assert_eq!(stub.len(), 8);
        // Second word should be NOP = 0x00000000
        assert_eq!(&stub[4..8], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_elf_header_big_endian() {
        let elf = build_minimal_mips64_elf(&[0x01, 0x02, 0x03, 0x04], 0x120000000);
        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // Check ELFCLASS64
        assert_eq!(elf[4], 2);
        // Check ELFDATA2MSB (big-endian)
        assert_eq!(elf[5], 2);
        // Check e_machine = EM_MIPS = 8 (big-endian u16)
        assert_eq!(&elf[18..20], &[0x00, 0x08]);
    }

    #[test]
    fn test_trampoline_has_delay_slot_nop() {
        let backend = Mips64Backend::new();
        let tramp = backend.trampoline(0x120000000);
        // Trampoline is 8 instructions * 4 bytes = 32 bytes
        assert_eq!(tramp.len(), 32);
        // Last 4 bytes should be NOP
        assert_eq!(&tramp[28..32], &[0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn test_mnemonic() {
        assert_eq!(Instruction::Daddu { rd: Gpr::V0, rs: Gpr::A0, rt: Gpr::A1 }.mnemonic(), "daddu");
        assert_eq!(Instruction::Beq { rs: Gpr::A0, rt: Gpr::Zero, offset: 8 }.mnemonic(), "beq");
        assert_eq!(Instruction::Nop.mnemonic(), "nop");
        assert_eq!(Instruction::Syscall { code: 0 }.mnemonic(), "syscall");
        assert_eq!(Instruction::Dsll { rd: Gpr::T9, rt: Gpr::T9, sa: 16 }.mnemonic(), "dsll");
    }

    #[test]
    fn test_display() {
        let add = Instruction::Add {
            rd: Gpr::V0,
            rs: Gpr::A0,
            rt: Gpr::A1,
        };
        assert_eq!(format!("{}", add), "add $v0, $a0, $a1");

        let ld = Instruction::Ld {
            rt: Gpr::V0,
            base: Gpr::Sp,
            offset: 16,
        };
        assert_eq!(format!("{}", ld), "ld $v0, 16($sp)");

        let beq = Instruction::Beq {
            rs: Gpr::A0,
            rt: Gpr::Zero,
            offset: 8,
        };
        assert_eq!(format!("{}", beq), "beq $a0, $zero, +8");
    }

    // ── ISel integration tests ──────────────────────────────────────

    #[test]
    fn test_isel_alloc_emits_daddiu() {
        // Alloc should emit daddiu $sp, $sp, -size (not a NOP)
        let backend = Mips64Backend::new();
        let mut func = IRFunction::new("test_alloc");
        func.blocks[0].instructions.push(IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 32,
        });
        func.blocks[0].terminator = crate::ir::IRTerminator::Return(vec![]);
        let allocated = backend.allocate_registers(&func).unwrap();
        // Find the daddiu instruction for the alloc (not the prologue one)
        let alloc_instrs: Vec<_> = allocated
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .filter(|i| i.opcode == "daddiu")
            .collect();
        // Should have at least the prologue daddiu and the alloc daddiu
        assert!(
            alloc_instrs.len() >= 2,
            "expected at least 2 daddiu instructions (prologue + alloc), got {}",
            alloc_instrs.len()
        );
        // The alloc daddiu should not be a NOP (0x00000000)
        let alloc_encoded = &alloc_instrs[1].encoded;
        let word = u32::from_be_bytes([alloc_encoded[0], alloc_encoded[1], alloc_encoded[2], alloc_encoded[3]]);
        assert_ne!(word, 0, "alloc daddiu should not encode as NOP");
    }

    #[test]
    fn test_isel_neg_emits_dsubu() {
        // Neg should emit dsubu dst, $zero, src
        let neg = Instruction::Dsubu {
            rd: Gpr::V0,
            rs: Gpr::Zero,
            rt: Gpr::A0,
        };
        let encoded = neg.encode();
        let word = u32::from_be_bytes(encoded);
        // dsubu is R-type: opcode=0, rs=$zero(0), rt=$a0(4), rd=$v0(2), sa=0, funct=0x2F
        let expected: u32 = (0 << 26) | (0 << 21) | (4 << 16) | (2 << 11) | (0 << 6) | 0x2F;
        assert_eq!(word, expected, "neg (dsubu dst, $zero, src) encoding mismatch");
    }

    #[test]
    fn test_isel_not_emits_nor() {
        // Not should emit nor dst, src, $zero → ~(src | 0) = ~src
        let not = Instruction::Nor {
            rd: Gpr::V0,
            rs: Gpr::A0,
            rt: Gpr::Zero,
        };
        let encoded = not.encode();
        let word = u32::from_be_bytes(encoded);
        // nor is R-type: opcode=0, rs=$a0(4), rt=$zero(0), rd=$v0(2), sa=0, funct=0x27
        let expected: u32 = (0 << 26) | (4 << 21) | (0 << 16) | (2 << 11) | (0 << 6) | 0x27;
        assert_eq!(word, expected, "not (nor dst, src, $zero) encoding mismatch");
    }

    // ── ISel integration tests ─────────────────────────────────────────

    /// Helper: build a minimal IR function with one block and the given
    /// instructions, then run allocate_registers and return the result.
    fn isel_func(name: &str, instrs: Vec<IRInstr>) -> AllocatedFunction {
        use std::collections::HashSet;
        let backend = Mips64Backend::new();
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
            }],
        };
        backend.allocate_registers(&func).unwrap()
    }

    #[test]
    fn test_isel_add_emits_daddu() {
        let result = isel_func("add_test", vec![IRInstr::Add {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
        }]);
        // Skip prologue (2 instructions: daddiu + sd), look for daddu
        let instrs = &result.blocks[0].instructions;
        // Find a daddu instruction (opcode field starts with "daddu")
        let daddu_count = instrs.iter().filter(|i| i.opcode == "daddu").count();
        assert!(daddu_count >= 1, "expected at least one daddu, found {daddu_count}");
    }

    #[test]
    fn test_isel_mul_emits_dmult_mflo() {
        let result = isel_func("mul_test", vec![IRInstr::Mul {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
        }]);
        let instrs = &result.blocks[0].instructions;
        let has_dmult = instrs.iter().any(|i| i.opcode == "dmult");
        let has_mflo = instrs.iter().any(|i| i.opcode == "mflo");
        assert!(has_dmult, "expected dmult instruction for Mul");
        assert!(has_mflo, "expected mflo instruction after dmult");
    }

    #[test]
    fn test_isel_ret_emits_epilogue() {
        let result = isel_func("ret_test", vec![IRInstr::Ret {
            values: vec![IRValue::Register(0)],
        }]);
        let instrs = &result.blocks[0].instructions;
        // With a frame, Ret should emit: ld $ra, ...; daddiu $sp, ...; jr $ra; nop
        let has_ld_ra = instrs.iter().any(|i| i.opcode == "ld" && i.reads.contains(&PhysicalReg::new(RegClass::Gpr, Gpr::Sp.encoding())));
        let has_jr = instrs.iter().any(|i| i.opcode == "jr");
        let has_nop = instrs.iter().any(|i| i.opcode == "nop");
        assert!(has_ld_ra, "expected ld to restore $ra in epilogue");
        assert!(has_jr, "expected jr $ra in epilogue");
        assert!(has_nop, "expected nop delay slot after jr");
    }

    #[test]
    fn test_isel_binop_and_emits_and() {
        let result = isel_func("and_test", vec![IRInstr::BinOp {
            op: BinOpKind::And,
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
        }]);
        let instrs = &result.blocks[0].instructions;
        let has_and = instrs.iter().any(|i| i.opcode == "and");
        assert!(has_and, "expected and instruction for BinOp::And");
    }

    #[test]
    fn test_isel_free_emits_break() {
        let result = isel_func("free_test", vec![IRInstr::Free {
            ptr: IRValue::Register(0),
        }]);
        let instrs = &result.blocks[0].instructions;
        let has_break = instrs.iter().any(|i| i.opcode == "break");
        assert!(has_break, "expected break instruction for Free (runtime trap)");
    }

    #[test]
    fn test_isel_cmp_eq_emits_xor_sltiu() {
        let result = isel_func("cmp_eq_test", vec![IRInstr::Cmp {
            kind: CmpKind::Eq,
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
        }]);
        let instrs = &result.blocks[0].instructions;
        let has_xor = instrs.iter().any(|i| i.opcode == "xor");
        let has_sltiu = instrs.iter().any(|i| i.opcode == "sltiu");
        assert!(has_xor, "expected xor for Cmp Eq");
        assert!(has_sltiu, "expected sltiu for Cmp Eq");
    }

    #[test]
    fn test_isel_load_store_roundtrip() {
        let result = isel_func("ld_sd_test", vec![
            IRInstr::Load { dst: IRValue::Register(0), addr: IRValue::Register(1) },
            IRInstr::Store { value: IRValue::Register(0), addr: IRValue::Register(1) },
        ]);
        let instrs = &result.blocks[0].instructions;
        let has_ld = instrs.iter().any(|i| i.opcode == "ld");
        let has_sd = instrs.iter().any(|i| i.opcode == "sd");
        assert!(has_ld, "expected ld instruction for Load");
        assert!(has_sd, "expected sd instruction for Store");
    }

    #[test]
    fn test_isel_alloc_emits_daddiu_sp() {
        let result = isel_func("alloc_test", vec![IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 32,
        }]);
        let instrs = &result.blocks[0].instructions;
        // Alloc should emit daddiu $sp, $sp, -32 and daddu dst, $sp, $zero
        let daddiu_count = instrs.iter().filter(|i| i.opcode == "daddiu").count();
        assert!(daddiu_count >= 2, "expected at least 2 daddiu (prologue + alloc), found {daddiu_count}");
    }
}
pub mod disasm;
