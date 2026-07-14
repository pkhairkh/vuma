//! # Shared RISC-V Types and Encoding Helpers
//!
//! This module contains types, constants, and encoding helpers that are
//! identical between the RV32 and RV64 backends. Both `riscv64.rs` and
//! `riscv32.rs` import these via `use crate::riscv_common::*`.
//!
//! Items NOT shared (kept in each backend file):
//! - `Instruction` enum (RV64 has extra W-suffix variants)
//! - `encode()` impl (different instruction sets)
//! - `ss_load_imm` (RV64 can do SLLI 32; RV32 cannot)
//! - `ss_store_64`/`ss_load_64` (RV32 uses paired-word)
//! - ELF builder (ELFCLASS32 vs ELFCLASS64)
//! - `Backend` impl (different TargetInfo)

use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// RISC-V general-purpose registers (x0–x31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Gpr {
    Zero, Ra, Sp, Gp, Tp, T0, T1, T2,
    S0, S1, A0, A1, A2, A3, A4, A5,
    A6, A7, S2, S3, S4, S5, S6, S7,
    S8, S9, S10, S11, T3, T4, T5, T6,
}

impl Gpr {
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    pub fn from_encoding(enc: u32) -> Option<Self> {
        if enc > 31 { return None; }
        // SAFETY: Gpr is a fieldless enum with repr(u32) and 32 variants 0..31
        Some(unsafe { std::mem::transmute(enc) })
    }

    pub fn is_callee_saved(&self) -> bool {
        matches!(self, Gpr::Sp | Gpr::S0 | Gpr::S1 | Gpr::S2 | Gpr::S3
            | Gpr::S4 | Gpr::S5 | Gpr::S6 | Gpr::S7 | Gpr::S8 | Gpr::S9
            | Gpr::S10 | Gpr::S11)
    }

    pub fn is_arg_reg(&self) -> bool {
        matches!(self, Gpr::A0 | Gpr::A1 | Gpr::A2 | Gpr::A3
            | Gpr::A4 | Gpr::A5 | Gpr::A6 | Gpr::A7)
    }

    pub fn is_available(&self) -> bool {
        !matches!(self, Gpr::Zero | Gpr::Ra | Gpr::Sp | Gpr::Tp)
    }

    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::Zero => "zero", Gpr::Ra => "ra", Gpr::Sp => "sp", Gpr::Gp => "gp",
            Gpr::Tp => "tp", Gpr::T0 => "t0", Gpr::T1 => "t1", Gpr::T2 => "t2",
            Gpr::S0 => "s0", Gpr::S1 => "s1", Gpr::A0 => "a0", Gpr::A1 => "a1",
            Gpr::A2 => "a2", Gpr::A3 => "a3", Gpr::A4 => "a4", Gpr::A5 => "a5",
            Gpr::A6 => "a6", Gpr::A7 => "a7", Gpr::S2 => "s2", Gpr::S3 => "s3",
            Gpr::S4 => "s4", Gpr::S5 => "s5", Gpr::S6 => "s6", Gpr::S7 => "s7",
            Gpr::S8 => "s8", Gpr::S9 => "s9", Gpr::S10 => "s10", Gpr::S11 => "s11",
            Gpr::T3 => "t3", Gpr::T4 => "t4", Gpr::T5 => "t5", Gpr::T6 => "t6",
        }
    }

    pub fn for_arg(idx: usize) -> Option<Self> {
        match idx {
            0 => Some(Gpr::A0), 1 => Some(Gpr::A1), 2 => Some(Gpr::A2),
            3 => Some(Gpr::A3), 4 => Some(Gpr::A4), 5 => Some(Gpr::A5),
            6 => Some(Gpr::A6), 7 => Some(Gpr::A7),
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

/// RISC-V floating-point registers (f0–f31).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Fpr {
    F0, F1, F2, F3, F4, F5, F6, F7,
    F8, F9, F10, F11, F12, F13, F14, F15,
    F16, F17, F18, F19, F20, F21, F22, F23,
    F24, F25, F26, F27, F28, F29, F30, F31,
}

impl Fpr {
    pub fn encoding(&self) -> u32 {
        *self as u32
    }

    pub fn from_encoding(enc: u32) -> Option<Self> {
        if enc > 31 { return None; }
        Some(unsafe { std::mem::transmute(enc) })
    }

    pub fn is_callee_saved(&self) -> bool {
        matches!(self, Fpr::F8 | Fpr::F9 | Fpr::F18 | Fpr::F19
            | Fpr::F20 | Fpr::F21 | Fpr::F22 | Fpr::F23
            | Fpr::F24 | Fpr::F25 | Fpr::F26 | Fpr::F27)
    }

    pub fn is_arg_reg(&self) -> bool {
        matches!(self, Fpr::F0 | Fpr::F1 | Fpr::F2 | Fpr::F3
            | Fpr::F4 | Fpr::F5 | Fpr::F6 | Fpr::F7)
    }

    pub fn asm_name(&self) -> &'static str {
        match self {
            Fpr::F0 => "fa0", Fpr::F1 => "fa1", Fpr::F2 => "fa2", Fpr::F3 => "fa3",
            Fpr::F4 => "fa4", Fpr::F5 => "fa5", Fpr::F6 => "fa6", Fpr::F7 => "fa7",
            Fpr::F8 => "fs0", Fpr::F9 => "fs1",
            Fpr::F10 => "ft0", Fpr::F11 => "ft1", Fpr::F12 => "ft2", Fpr::F13 => "ft3",
            Fpr::F14 => "ft4", Fpr::F15 => "ft5", Fpr::F16 => "ft6", Fpr::F17 => "ft7",
            Fpr::F18 => "fs2", Fpr::F19 => "fs3", Fpr::F20 => "fs4", Fpr::F21 => "fs5",
            Fpr::F22 => "fs6", Fpr::F23 => "fs7", Fpr::F24 => "fs8", Fpr::F25 => "fs9",
            Fpr::F26 => "fs10", Fpr::F27 => "fs11",
            Fpr::F28 => "ft8", Fpr::F29 => "ft9", Fpr::F30 => "ft10", Fpr::F31 => "ft11",
        }
    }
}

impl fmt::Display for Fpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Instruction Format Encoding Helpers
// ===========================================================================

/// R-type: funct7[31:25] | rs2[24:20] | rs1[19:15] | funct3[14:12] | rd[11:7] | opcode[6:0]
pub fn encode_r_type(funct7: u32, rs2: u32, rs1: u32, funct3: u32, rd: u32, opcode: u32) -> [u8; 4] {
    let word = ((funct7 & 0x7F) << 25)
        | ((rs2 & 0x1F) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | ((rd & 0x1F) << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// I-type: imm[31:20] | rs1[19:15] | funct3[14:12] | rd[11:7] | opcode[6:0]
pub fn encode_i_type(opcode: u32, rd: u32, funct3: u32, rs1: u32, imm: u32) -> [u8; 4] {
    let word = ((imm & 0xFFF) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | ((rd & 0x1F) << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// S-type: imm[11:5] | rs2 | rs1 | funct3 | imm[4:0] | opcode
pub fn encode_s_type(opcode: u32, funct3: u32, rs1: u32, rs2: u32, imm: u32) -> [u8; 4] {
    let imm_hi = (imm >> 5) & 0x7F;
    let imm_lo = imm & 0x1F;
    let word = (imm_hi << 25)
        | ((rs2 & 0x1F) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | (imm_lo << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// B-type: imm[12] imm[10:5] rs2 rs1 funct3 imm[4:1] imm[11] opcode
pub fn encode_b_type(opcode: u32, funct3: u32, rs1: u32, rs2: u32, imm: i32) -> [u8; 4] {
    let imm = imm as u32;
    let b12 = (imm >> 12) & 1;
    let b11 = (imm >> 11) & 1;
    let b10_5 = (imm >> 5) & 0x3F;
    let b4_1 = (imm >> 1) & 0xF;
    let word = (b12 << 31)
        | (b10_5 << 25)
        | ((rs2 & 0x1F) << 20)
        | ((rs1 & 0x1F) << 15)
        | ((funct3 & 0x7) << 12)
        | (b4_1 << 8)
        | (b11 << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

/// U-type: imm[31:12] | rd | opcode
pub fn encode_u_type(opcode: u32, rd: u32, imm: u32) -> [u8; 4] {
    let word = ((imm & 0xFFFFF) << 12) | ((rd & 0x1F) << 7) | (opcode & 0x7F);
    word.to_le_bytes()
}

/// J-type: imm[20] imm[10:1] imm[11] imm[19:12] rd opcode
pub fn encode_j_type(opcode: u32, rd: u32, imm: i32) -> [u8; 4] {
    let imm = imm as u32;
    let b20 = (imm >> 20) & 1;
    let b10_1 = (imm >> 1) & 0x3FF;
    let b11 = (imm >> 11) & 1;
    let b19_12 = (imm >> 12) & 0xFF;
    let word = (b20 << 31)
        | (b10_1 << 21)
        | (b11 << 20)
        | (b19_12 << 12)
        | ((rd & 0x1F) << 7)
        | (opcode & 0x7F);
    word.to_le_bytes()
}

// ===========================================================================
// Opcodes (shared between RV32 and RV64)
// ===========================================================================

pub const OPC_LUI: u32 = 0x37;
pub const OPC_AUIPC: u32 = 0x17;
pub const OPC_JAL: u32 = 0x6F;
pub const OPC_JALR: u32 = 0x67;
pub const OPC_BRANCH: u32 = 0x63;
pub const OPC_LOAD: u32 = 0x03;
pub const OPC_STORE: u32 = 0x23;
pub const OPC_OP_IMM: u32 = 0x13;
pub const OPC_OP: u32 = 0x33;
pub const OPC_FENCE: u32 = 0x0F;
pub const OPC_SYSTEM: u32 = 0x73;
pub const OPC_OP_FP: u32 = 0x53;
pub const OPC_BRANCH_AMO: u32 = 0x2F;
pub const OPC_LOAD_FP: u32 = 0x07;
pub const OPC_STORE_FP: u32 = 0x27;
pub const OPC_OP_IMM_32: u32 = 0x1B;
pub const OPC_OP_32: u32 = 0x3B;
pub const OPC_MADD: u32 = 0x43;
pub const OPC_NMSUB: u32 = 0x4B;
pub const OPC_NMADD: u32 = 0x53;
pub const OPC_MSUB: u32 = 0x47;

// ===========================================================================
// Helper: simple syscall stub
// ===========================================================================

/// Build a simple syscall stub: `addi a7, zero, #num; ecall; ret`
pub fn simple_stub(_num: i32, _jalr: fn() -> [u8; 4], _addi: fn(u32, u32, i32) -> [u8; 4], _ecall: fn() -> [u8; 4]) -> Vec<u8> {
    // This is a placeholder — actual implementation needs the backend's
    // Instruction enum. Each backend keeps its own simple_stub.
    // This function is here for documentation but not used directly.
    Vec::new()
}

/// Move register: `mv rd, rs` = `addi rd, rs, 0`
#[macro_export]
macro_rules! rv_mv {
    ($rd:expr, $rs:expr) => {
        $crate::riscv_common::encode_i_type($crate::riscv_common::OPC_OP_IMM, $rd.encoding(), 0, $rs.encoding(), 0)
    };
}
