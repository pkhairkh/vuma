//! # LoongArch64 Mnemonic Disassembler
//!
//! Decodes LoongArch64 32-bit little-endian machine code into `Instruction`
//! instances. Covers the key instructions lowered by the VUMA ISel.
//! Display is already provided by the parent module.
//!
//! Opcode values are taken from the LoongArch Reference Manual Volume 1 and
//! must match the constants in the parent `mod.rs`.

use super::Fpr;
use super::Gpr;
use super::Instruction;

// ---------------------------------------------------------------------------
// Decode error
// ---------------------------------------------------------------------------

/// Error produced when LoongArch64 decoding fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The byte slice is too short.
    Truncated { needed: usize, available: usize },
    /// The instruction encoding is not recognised.
    UnknownEncoding { word: u32 },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated { needed, available } => {
                write!(f, "truncated: need {needed} bytes, have {available}")
            }
            DecodeError::UnknownEncoding { word } => {
                write!(f, "unknown LoongArch64 encoding: 0x{word:08x}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn gpr_from_bits(bits: u32) -> Gpr {
    match bits {
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
        _ => Gpr::R0,
    }
}

fn fpr_from_bits(bits: u32) -> Fpr {
    match bits {
        0 => Fpr::F0,
        1 => Fpr::F1,
        2 => Fpr::F2,
        3 => Fpr::F3,
        4 => Fpr::F4,
        5 => Fpr::F5,
        6 => Fpr::F6,
        7 => Fpr::F7,
        8 => Fpr::F8,
        9 => Fpr::F9,
        10 => Fpr::F10,
        11 => Fpr::F11,
        12 => Fpr::F12,
        13 => Fpr::F13,
        14 => Fpr::F14,
        15 => Fpr::F15,
        16 => Fpr::F16,
        17 => Fpr::F17,
        18 => Fpr::F18,
        19 => Fpr::F19,
        20 => Fpr::F20,
        21 => Fpr::F21,
        22 => Fpr::F22,
        23 => Fpr::F23,
        24 => Fpr::F24,
        25 => Fpr::F25,
        26 => Fpr::F26,
        27 => Fpr::F27,
        28 => Fpr::F28,
        29 => Fpr::F29,
        30 => Fpr::F30,
        31 => Fpr::F31,
        _ => Fpr::F0,
    }
}

fn sign_extend_12(val: u32) -> i32 {
    if val & 0x800 != 0 {
        (val | 0xFFFFF000) as i32
    } else {
        val as i32
    }
}

fn sign_extend_16(val: u32) -> i32 {
    if val & 0x8000 != 0 {
        (val | 0xFFFF0000) as i32
    } else {
        val as i32
    }
}

// NOTE: Upper-immediate instructions (LU12I.W, LU32I.D, PCADDU12I,
// PCADDU18I) are not yet decoded by the disassembler. Kept for use when
// those decode paths are added.
#[allow(dead_code)]
fn sign_extend_20(val: u32) -> i32 {
    if val & 0x80000 != 0 {
        (val | 0xFFF00000) as i32
    } else {
        val as i32
    }
}

fn sign_extend_21(val: u32) -> i32 {
    if val & 0x100000 != 0 {
        (val | 0xFFE00000) as i32
    } else {
        val as i32
    }
}

fn sign_extend_26(val: u32) -> i32 {
    if val & 0x2000000 != 0 {
        (val | 0xFC000000) as i32
    } else {
        val as i32
    }
}

// ---------------------------------------------------------------------------
// Opcode constants — must match mod.rs
// ---------------------------------------------------------------------------

// 3R-format opcodes (bits[31:15])
const OPC_ADD_W: u32 = 0x0020;
const OPC_ADD_D: u32 = 0x0021;
const OPC_SUB_W: u32 = 0x0022;
const OPC_SUB_D: u32 = 0x0023;
const OPC_SLT: u32 = 0x0024;
const OPC_SLTU: u32 = 0x0025;
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

// 3R-format FP Arithmetic opcodes (bits[31:15])
const OPC_FADD_S: u32 = 0x0201;
const OPC_FADD_D: u32 = 0x0202;
const OPC_FSUB_S: u32 = 0x0205;
const OPC_FSUB_D: u32 = 0x0206;
const OPC_FMUL_S: u32 = 0x0209;
const OPC_FMUL_D: u32 = 0x020A;
const OPC_FDIV_S: u32 = 0x020D;
const OPC_FDIV_D: u32 = 0x020E;

// reg2i5-format opcodes (bits[31:15]) — .W shift immediates
const OPC_SLLI_W: u32 = 0x0081;
const OPC_SRLI_W: u32 = 0x0089;
const OPC_SRAI_W: u32 = 0x0091;

// reg2i6-format opcodes (bits[31:16]) — .D shift immediates
const OPC_SLLI_D: u32 = 0x0041;
const OPC_SRLI_D: u32 = 0x0045;
const OPC_SRAI_D: u32 = 0x0049;

// 2RI12-format opcodes (bits[31:22])
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
const OPC_FLD_S: u32 = 0x0AC;
const OPC_FLD_D: u32 = 0x0AE;
const OPC_FST_S: u32 = 0x0AD;
const OPC_FST_D: u32 = 0x0AF;

// 2RI16-format opcodes (bits[31:26])
const OPC_BEQ: u32 = 0x16;
const OPC_BNE: u32 = 0x17;
const OPC_BLT: u32 = 0x18;
const OPC_BGE: u32 = 0x19;
const OPC_BLTU: u32 = 0x1A;
const OPC_BGEU: u32 = 0x1B;
const OPC_JIRL: u32 = 0x13;

// I26-format opcodes (bits[31:26])
const OPC_B: u32 = 0x14;
const OPC_BL: u32 = 0x15;

// 1RI21-format opcodes (bits[31:26])
const OPC_BEQZ: u32 = 0x10;
const OPC_BNEZ: u32 = 0x11;

// 2R-format opcodes (bits[31:10])
const OPC_EXT_W_H: u32 = 0x0000016;
const OPC_EXT_W_B: u32 = 0x0000017;
const OPC_FMOV_S: u32 = 0x004525;
const OPC_FMOV_D: u32 = 0x004526;
const OPC_MOVFR2GR_D: u32 = 0x00452E;
const OPC_MOVGR2FR_D: u32 = 0x00452A;

// 4R-format opcodes (bits[31:20])
const OPC_FCMP_S: u32 = 0x0C1;
const OPC_FCMP_D: u32 = 0x0C2;

// ---------------------------------------------------------------------------
// Decode entry point
// ---------------------------------------------------------------------------

impl Instruction {
    /// Decode a single LoongArch64 instruction from 4 little-endian bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < 4 {
            return Err(DecodeError::Truncated {
                needed: 4,
                available: bytes.len(),
            });
        }
        let word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);

        // Extract common fields
        let rd = word & 0x1F;
        let rj = (word >> 5) & 0x1F;
        let rk = (word >> 10) & 0x1F;

        // 2R format: opcode in bits [31:10]
        let opc_2r = (word >> 10) & 0x3FFFFF;

        // 4R format: opcode in bits [31:20]
        let opc_4r = (word >> 20) & 0xFFF;

        // 3R / reg2i5 format: opcode in bits [31:15]
        let opc_3r = (word >> 15) & 0x1FFFF;

        // reg2i6 format: opcode in bits [31:16]
        let opc_reg2i6 = (word >> 16) & 0xFFFF;

        // 2RI12 format: opcode in bits [31:22]
        let opc_2ri12 = (word >> 22) & 0x3FF;
        let imm12_raw = (word >> 10) & 0xFFF;

        // 2RI16 format: opcode in bits [31:26]
        let opc_2ri16 = (word >> 26) & 0x3F;
        let imm16_raw = (word >> 10) & 0xFFFF;

        // 1RI21 format: opcode in bits [31:26]
        let opc_1ri21 = (word >> 26) & 0x3F;

        // I26 format: opcode in bits [31:26]
        let opc_i26 = (word >> 26) & 0x3F;

        // ── 2R format (longest opcode, check first) ────────────────
        match opc_2r {
            OPC_EXT_W_H => {
                return Ok(Instruction::ExtWH {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                });
            }
            OPC_EXT_W_B => {
                return Ok(Instruction::ExtWB {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                });
            }
            // FP Move: fmov.s fd, fj
            OPC_FMOV_S => {
                return Ok(Instruction::FmovS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                });
            }
            // FP Move: fmov.d fd, fj
            OPC_FMOV_D => {
                return Ok(Instruction::FmovD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                });
            }
            // FP Move: movfr2gr.d rd, fj
            OPC_MOVFR2GR_D => {
                return Ok(Instruction::FmovGr2FprD {
                    rd: gpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                });
            }
            // FP Move: movgr2fr.d fd, rj
            OPC_MOVGR2FR_D => {
                return Ok(Instruction::FmovFpr2GrD {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                });
            }
            _ => {}
        }

        // ── 4R format (FP Compare: fcmp.cond.s/d) ─────────────────
        let cond = ((word >> 15) & 0x1F) as u8;
        let cd = (rd & 0x1F) as u8;
        match opc_4r {
            OPC_FCMP_S => {
                return Ok(Instruction::FCmpS {
                    cond,
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                    cd,
                });
            }
            OPC_FCMP_D => {
                return Ok(Instruction::FCmpD {
                    cond,
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                    cd,
                });
            }
            _ => {}
        }

        // ── 3R / reg2i5 format ─────────────────────────────────────
        // reg2i5 shares the same opcode field (bits[31:15]) with 3R.
        // For reg2i5 instructions, bits[14:10] hold a 5-bit immediate
        // (same position as the rk register field in 3R).
        match opc_3r {
            // ── Arithmetic (3R) ──
            OPC_ADD_W => {
                return Ok(Instruction::AddW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_ADD_D => {
                return Ok(Instruction::AddD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SUB_W => {
                return Ok(Instruction::SubW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SUB_D => {
                return Ok(Instruction::SubD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SLT => {
                return Ok(Instruction::Slt {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SLTU => {
                return Ok(Instruction::Sltu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            // ── Logical (3R) ──
            OPC_AND => {
                return Ok(Instruction::And {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_OR => {
                return Ok(Instruction::Or {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_XOR => {
                return Ok(Instruction::Xor {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_NOR => {
                return Ok(Instruction::Nor {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_ANDN => {
                return Ok(Instruction::Andn {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_ORN => {
                return Ok(Instruction::Orn {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            // ── Shift (3R) ──
            OPC_SLL_W => {
                return Ok(Instruction::SllW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SRL_W => {
                return Ok(Instruction::SrlW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SRA_W => {
                return Ok(Instruction::SraW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SLL_D => {
                return Ok(Instruction::SllD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SRL_D => {
                return Ok(Instruction::SrlD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_SRA_D => {
                return Ok(Instruction::SraD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_ROTR_W => {
                return Ok(Instruction::RotrW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_ROTR_D => {
                return Ok(Instruction::RotrD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            // ── Multiply / Divide (3R) ──
            OPC_MUL_W => {
                return Ok(Instruction::MulW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_MUL_D => {
                return Ok(Instruction::MulD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_DIV_W => {
                return Ok(Instruction::DivW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_MOD_W => {
                return Ok(Instruction::ModW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_DIV_WU => {
                return Ok(Instruction::DivWu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_MOD_WU => {
                return Ok(Instruction::ModWu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_DIV_D => {
                return Ok(Instruction::DivD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_MOD_D => {
                return Ok(Instruction::ModD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_DIV_DU => {
                return Ok(Instruction::DivDu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            OPC_MOD_DU => {
                return Ok(Instruction::ModDu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                });
            }
            // ── Shift Immediate .W (reg2i5, shares opcode field with 3R) ──
            // For reg2i5, bits[14:10] hold a 5-bit immediate (ui5),
            // which occupies the same position as the rk field.
            OPC_SLLI_W => {
                return Ok(Instruction::SlliW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: rk, // 5-bit immediate at bits[14:10]
                });
            }
            OPC_SRLI_W => {
                return Ok(Instruction::SrliW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: rk,
                });
            }
            OPC_SRAI_W => {
                return Ok(Instruction::SraiW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: rk,
                });
            }
            // ── FP Arithmetic (3R) ──
            OPC_FADD_S => {
                return Ok(Instruction::FaddS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            OPC_FADD_D => {
                return Ok(Instruction::FaddD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            OPC_FSUB_S => {
                return Ok(Instruction::FsubS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            OPC_FSUB_D => {
                return Ok(Instruction::FsubD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            OPC_FMUL_S => {
                return Ok(Instruction::FmulS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            OPC_FMUL_D => {
                return Ok(Instruction::FmulD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            OPC_FDIV_S => {
                return Ok(Instruction::FdivS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            OPC_FDIV_D => {
                return Ok(Instruction::FdivD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                });
            }
            _ => {}
        }

        // ── reg2i6 format — .D shift immediates ────────────────────
        // opcode in bits[31:16], 6-bit immediate in bits[15:10]
        let imm6 = (word >> 10) & 0x3F;
        match opc_reg2i6 {
            OPC_SLLI_D => {
                return Ok(Instruction::SlliD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm6,
                });
            }
            OPC_SRLI_D => {
                return Ok(Instruction::SrliD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm6,
                });
            }
            OPC_SRAI_D => {
                return Ok(Instruction::SraiD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm6,
                });
            }
            _ => {}
        }

        // ── 2RI12 opcodes ──────────────────────────────────────────
        match opc_2ri12 {
            OPC_ADDI_W => {
                return Ok(Instruction::AddiW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_ADDI_D => {
                return Ok(Instruction::AddiD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_SLTI => {
                return Ok(Instruction::Slti {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_SLTUI => {
                return Ok(Instruction::Sltui {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_ANDI => {
                return Ok(Instruction::Andi {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: imm12_raw,
                });
            }
            OPC_ORI => {
                return Ok(Instruction::Ori {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: imm12_raw,
                });
            }
            OPC_XORI => {
                return Ok(Instruction::Xori {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: imm12_raw,
                });
            }
            // Load
            OPC_LD_B => {
                return Ok(Instruction::LdB {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_LD_H => {
                return Ok(Instruction::LdH {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_LD_W => {
                return Ok(Instruction::LdW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_LD_D => {
                return Ok(Instruction::LdD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            // Store
            OPC_ST_B => {
                return Ok(Instruction::StB {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_ST_H => {
                return Ok(Instruction::StH {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_ST_W => {
                return Ok(Instruction::StW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_ST_D => {
                return Ok(Instruction::StD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            // Unsigned load
            OPC_LD_BU => {
                return Ok(Instruction::LdBu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_LD_HU => {
                return Ok(Instruction::LdHu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_LD_WU => {
                return Ok(Instruction::LdWu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            // FP Load/Store
            OPC_FLD_S => {
                return Ok(Instruction::FldS {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_FLD_D => {
                return Ok(Instruction::FldD {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_FST_S => {
                return Ok(Instruction::FstS {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            OPC_FST_D => {
                return Ok(Instruction::FstD {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                });
            }
            _ => {}
        }

        // ── 2RI16 opcodes (branches + JIRL) ───────────────────────
        match opc_2ri16 {
            OPC_JIRL => {
                return Ok(Instruction::Jirl {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    offs16: sign_extend_16(imm16_raw),
                });
            }
            OPC_BEQ => {
                return Ok(Instruction::Beq {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                });
            }
            OPC_BNE => {
                return Ok(Instruction::Bne {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                });
            }
            OPC_BLT => {
                return Ok(Instruction::Blt {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                });
            }
            OPC_BGE => {
                return Ok(Instruction::Bge {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                });
            }
            OPC_BLTU => {
                return Ok(Instruction::Bltu {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                });
            }
            OPC_BGEU => {
                return Ok(Instruction::Bgeu {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                });
            }
            _ => {}
        }

        // ── I26 format (unconditional branch) ──────────────────────
        // Encoding: opcode[31:26] | offs[15:0] at bits[25:10] | offs[25:16] at bits[9:0]
        {
            let lo16 = (word >> 10) & 0xFFFF;
            let hi10 = word & 0x3FF;
            let offs26 = (hi10 << 16) | lo16;
            match opc_i26 {
                OPC_B => {
                    return Ok(Instruction::B {
                        offs26: sign_extend_26(offs26),
                    });
                }
                OPC_BL => {
                    return Ok(Instruction::Bl {
                        offs26: sign_extend_26(offs26),
                    });
                }
                _ => {}
            }
        }

        // ── 1RI21 format (beqz/bnez) ──────────────────────────────
        // Encoding: opcode[31:26] | offs[15:0] at bits[25:10] | rj[9:5] | offs[20:16] at bits[4:0]
        {
            let lo16 = (word >> 10) & 0xFFFF;
            let hi5 = word & 0x1F;
            let imm21 = (hi5 << 16) | lo16;
            match opc_1ri21 {
                OPC_BEQZ => {
                    return Ok(Instruction::Beqz {
                        rj: gpr_from_bits(rj),
                        offs21: sign_extend_21(imm21),
                    });
                }
                OPC_BNEZ => {
                    return Ok(Instruction::Bnez {
                        rj: gpr_from_bits(rj),
                        offs21: sign_extend_21(imm21),
                    });
                }
                _ => {}
            }
        }

        Err(DecodeError::UnknownEncoding { word })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loongarch64::Gpr as G;

    #[test]
    fn test_decode_add_d() {
        let instr = Instruction::AddD {
            rd: G::A0,
            rj: G::A1,
            rk: G::A2,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_sub_w() {
        let instr = Instruction::SubW {
            rd: G::T0,
            rj: G::T1,
            rk: G::T2,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_and_or_xor() {
        for instr in [
            Instruction::And {
                rd: G::A0,
                rj: G::A1,
                rk: G::A2,
            },
            Instruction::Or {
                rd: G::A0,
                rj: G::A1,
                rk: G::A2,
            },
            Instruction::Xor {
                rd: G::A0,
                rj: G::A1,
                rk: G::A2,
            },
        ] {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"));
        }
    }

    #[test]
    fn test_decode_addi_d() {
        let instr = Instruction::AddiD {
            rd: G::A0,
            rj: G::Sp,
            imm12: 16,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_ld_st() {
        let ld = Instruction::LdD {
            rd: G::A0,
            rj: G::Sp,
            imm12: 0,
        };
        let bytes = ld.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{ld}"));

        let st = Instruction::StD {
            rd: G::A0,
            rj: G::Sp,
            imm12: 8,
        };
        let bytes = st.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{st}"));
    }

    #[test]
    fn test_decode_truncated() {
        let result = Instruction::decode(&[0x00, 0x00]);
        assert!(matches!(result, Err(DecodeError::Truncated { .. })));
    }

    #[test]
    fn test_decode_mul_div() {
        let instr = Instruction::MulD {
            rd: G::A0,
            rj: G::A1,
            rk: G::A2,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_all_3r_arithmetic() {
        let operands = [
            Instruction::AddW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::AddD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::SubW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::SubD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::Slt  { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::Sltu { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::MulW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::MulD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::DivW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::ModW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::DivWu { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::ModWu { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::DivD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::ModD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::DivDu { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::ModDu { rd: G::A0, rj: G::A1, rk: G::A2 },
        ];
        for instr in operands {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"), "round-trip failed for {:?}", instr);
        }
    }

    #[test]
    fn test_decode_all_3r_logical() {
        let operands = [
            Instruction::And { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::Or  { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::Xor { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::Nor { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::Andn { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::Orn  { rd: G::A0, rj: G::A1, rk: G::A2 },
        ];
        for instr in operands {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"), "round-trip failed for {:?}", instr);
        }
    }

    #[test]
    fn test_decode_all_3r_shift() {
        let operands = [
            Instruction::SllW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::SrlW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::SraW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::SllD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::SrlD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::SraD { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::RotrW { rd: G::A0, rj: G::A1, rk: G::A2 },
            Instruction::RotrD { rd: G::A0, rj: G::A1, rk: G::A2 },
        ];
        for instr in operands {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"), "round-trip failed for {:?}", instr);
        }
    }

    #[test]
    fn test_decode_shift_immediate() {
        let operands = [
            Instruction::SlliW { rd: G::A0, rj: G::A1, imm8: 5 },
            Instruction::SrliW { rd: G::A0, rj: G::A1, imm8: 10 },
            Instruction::SraiW { rd: G::A0, rj: G::A1, imm8: 15 },
            Instruction::SlliD { rd: G::A0, rj: G::A1, imm8: 20 },
            Instruction::SrliD { rd: G::A0, rj: G::A1, imm8: 30 },
            Instruction::SraiD { rd: G::A0, rj: G::A1, imm8: 40 },
        ];
        for instr in operands {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"), "round-trip failed for {:?}", instr);
        }
    }

    #[test]
    fn test_decode_branches() {
        let operands = [
            Instruction::Beq { rj: G::A0, rd: G::A1, offs16: 8 },
            Instruction::Bne { rj: G::A0, rd: G::A1, offs16: -16 },
            Instruction::Blt { rj: G::A0, rd: G::A1, offs16: 32 },
            Instruction::Bge { rj: G::A0, rd: G::A1, offs16: -4 },
            Instruction::Bltu { rj: G::A0, rd: G::A1, offs16: 12 },
            Instruction::Bgeu { rj: G::A0, rd: G::A1, offs16: -8 },
            Instruction::Jirl { rd: G::Ra, rj: G::A0, offs16: 4 },
        ];
        for instr in operands {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"), "round-trip failed for {:?}", instr);
        }
    }

    #[test]
    fn test_decode_unconditional_branch() {
        let b_instr = Instruction::B { offs26: 100 };
        let bytes = b_instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{b_instr}"));

        let bl_instr = Instruction::Bl { offs26: -200 };
        let bytes = bl_instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{bl_instr}"));
    }

    #[test]
    fn test_decode_beqz_bnez() {
        let beqz = Instruction::Beqz { rj: G::A0, offs21: 64 };
        let bytes = beqz.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{beqz}"));

        let bnez = Instruction::Bnez { rj: G::T0, offs21: -128 };
        let bytes = bnez.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{bnez}"));
    }

    #[test]
    fn test_decode_ext() {
        let extwh = Instruction::ExtWH { rd: G::A0, rj: G::A1 };
        let bytes = extwh.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{extwh}"));

        let extwb = Instruction::ExtWB { rd: G::A0, rj: G::A1 };
        let bytes = extwb.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{extwb}"));
    }

    // ── FP instruction decode tests ───────────────────────────────────

    #[test]
    fn test_decode_fp_arithmetic_s_d() {
        use crate::loongarch64::Fpr as F;
        // Test single-precision FP arithmetic: fadd.s, fsub.s, fmul.s, fdiv.s
        for instr in [
            Instruction::FaddS {
                fd: F::F0,
                fj: F::F1,
                fk: F::F2,
            },
            Instruction::FsubS {
                fd: F::F0,
                fj: F::F1,
                fk: F::F2,
            },
            Instruction::FmulS {
                fd: F::F3,
                fj: F::F4,
                fk: F::F5,
            },
            Instruction::FdivS {
                fd: F::F6,
                fj: F::F7,
                fk: F::F8,
            },
            // Double-precision FP arithmetic: fadd.d, fsub.d, fmul.d, fdiv.d
            Instruction::FaddD {
                fd: F::F0,
                fj: F::F1,
                fk: F::F2,
            },
            Instruction::FsubD {
                fd: F::F0,
                fj: F::F1,
                fk: F::F2,
            },
            Instruction::FmulD {
                fd: F::F3,
                fj: F::F4,
                fk: F::F5,
            },
            Instruction::FdivD {
                fd: F::F6,
                fj: F::F7,
                fk: F::F8,
            },
        ] {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(
                format!("{decoded}"),
                format!("{instr}"),
                "round-trip failed for {:?}",
                instr
            );
        }
    }

    #[test]
    fn test_decode_fp_mov_fcmp() {
        use crate::loongarch64::Fpr as F;
        // FP register-to-register move
        for instr in [
            Instruction::FmovS {
                fd: F::F0,
                fj: F::F1,
            },
            Instruction::FmovD {
                fd: F::F0,
                fj: F::F1,
            },
            // FP compare (CEQ = condition 0x02)
            Instruction::FCmpS {
                cond: 0x02,
                fj: F::F0,
                fk: F::F1,
                cd: 0,
            },
            Instruction::FCmpD {
                cond: 0x01,
                fj: F::F2,
                fk: F::F3,
                cd: 1,
            },
            // FP load/store
            Instruction::FldS {
                fd: F::F0,
                rj: G::Sp,
                imm12: 0,
            },
            Instruction::FldD {
                fd: F::F1,
                rj: G::Sp,
                imm12: 8,
            },
            Instruction::FstS {
                fd: F::F0,
                rj: G::Sp,
                imm12: 0,
            },
            Instruction::FstD {
                fd: F::F1,
                rj: G::Sp,
                imm12: 8,
            },
        ] {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(
                format!("{decoded}"),
                format!("{instr}"),
                "round-trip failed for {:?}",
                instr
            );
        }
    }

    #[test]
    fn test_decode_fp_gpr_moves() {
        use crate::loongarch64::Fpr as F;
        for instr in [
            Instruction::FmovGr2FprD {
                rd: G::A0,
                fj: F::F0,
            },
            Instruction::FmovFpr2GrD {
                fd: F::F0,
                rj: G::A0,
            },
        ] {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(
                format!("{decoded}"),
                format!("{instr}"),
                "round-trip failed for {:?}",
                instr
            );
        }
    }

    #[test]
    fn test_decode_negative_immediates() {
        // Test sign extension of negative immediates
        let addi = Instruction::AddiD {
            rd: G::A0,
            rj: G::Sp,
            imm12: -1,
        };
        let bytes = addi.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{addi}"));

        let ld = Instruction::LdW {
            rd: G::A0,
            rj: G::Sp,
            imm12: -256,
        };
        let bytes = ld.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{ld}"));
    }
}
