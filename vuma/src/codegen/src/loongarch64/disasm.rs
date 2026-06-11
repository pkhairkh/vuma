//! # LoongArch64 Mnemonic Disassembler
//!
//! Decodes LoongArch64 32-bit little-endian machine code into `Instruction`
//! instances. Covers the key instructions lowered by the VUMA ISel.
//! Display is already provided by the parent module.

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

        // 3R format: opcode in bits [31:15]
        let opc_3r = (word >> 15) & 0x1FFFF;

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

        // 2RI8 format: opcode in bits [31:22]
        let _opc_2ri8 = (word >> 22) & 0x3FF;
        let imm8_raw = (word >> 14) & 0xFF;

        // 2R format: opcode in bits [31:10]
        let opc_2r = (word >> 10) & 0x3FFFFFF;

        // 3R opcodes
        match opc_3r {
            0x0020 => {
                return Ok(Instruction::AddW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0021 => {
                return Ok(Instruction::AddD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0030 => {
                return Ok(Instruction::SubW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0031 => {
                return Ok(Instruction::SubD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0040 => {
                return Ok(Instruction::Slt {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0041 => {
                return Ok(Instruction::Sltu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0098 => {
                return Ok(Instruction::MulW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0099 => {
                return Ok(Instruction::MulD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x009E => {
                return Ok(Instruction::DivW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x009F => {
                return Ok(Instruction::ModW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x00A0 => {
                return Ok(Instruction::DivD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x00A1 => {
                return Ok(Instruction::ModD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0080 => {
                return Ok(Instruction::And {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0081 => {
                return Ok(Instruction::Or {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0082 => {
                return Ok(Instruction::Xor {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0083 => {
                return Ok(Instruction::Nor {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0084 => {
                return Ok(Instruction::Andn {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0085 => {
                return Ok(Instruction::Orn {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0089 => {
                return Ok(Instruction::SllW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x008A => {
                return Ok(Instruction::SrlW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x008B => {
                return Ok(Instruction::SraW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x008C => {
                return Ok(Instruction::SllD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x008D => {
                return Ok(Instruction::SrlD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x008E => {
                return Ok(Instruction::SraD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x008F => {
                return Ok(Instruction::RotrW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            0x0090 => {
                return Ok(Instruction::RotrD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    rk: gpr_from_bits(rk),
                })
            }
            // ── FP Arithmetic (3R) ──────────────────────────────
            0x0100 => {
                return Ok(Instruction::FaddS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            0x0101 => {
                return Ok(Instruction::FaddD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            0x0102 => {
                return Ok(Instruction::FsubS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            0x0103 => {
                return Ok(Instruction::FsubD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            0x0104 => {
                return Ok(Instruction::FmulS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            0x0105 => {
                return Ok(Instruction::FmulD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            0x0106 => {
                return Ok(Instruction::FdivS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            0x0107 => {
                return Ok(Instruction::FdivD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                })
            }
            _ => {}
        }

        // 2RI8 opcodes (shift immediate)
        // Note: SLLI.D (0x00B) shares its opcode with ADDI.D (2RI12 format).
        // In 2RI8 format, bits [15:14] must be 0; if they're non-zero, it's
        // actually a 2RI12 instruction and we skip the 2RI8 match.
        match _opc_2ri8 {
            0x008 => {
                return Ok(Instruction::SlliW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm8_raw,
                })
            }
            0x009 => {
                return Ok(Instruction::SrliW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm8_raw,
                })
            }
            0x00A => {
                return Ok(Instruction::SraiW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm8_raw,
                })
            }
            // SLLI.D shares opcode with ADDI.D — only match if bits [15:14] are 0
            0x00B if (word >> 14) & 0x3 == 0 => {
                return Ok(Instruction::SlliD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm8_raw,
                })
            }
            0x00C => {
                return Ok(Instruction::SrliD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm8_raw,
                })
            }
            0x00D => {
                return Ok(Instruction::SraiD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm8: imm8_raw,
                })
            }
            _ => {}
        }

        // 2RI12 opcodes
        match opc_2ri12 {
            0x00A => {
                return Ok(Instruction::AddiW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x00B => {
                return Ok(Instruction::AddiD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x008 => {
                return Ok(Instruction::Slti {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x009 => {
                return Ok(Instruction::Sltui {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x00D => {
                return Ok(Instruction::Andi {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: imm12_raw,
                })
            }
            0x00E => {
                return Ok(Instruction::Ori {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: imm12_raw,
                })
            }
            0x00F => {
                return Ok(Instruction::Xori {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: imm12_raw,
                })
            }
            // Load
            0x0A0 => {
                return Ok(Instruction::LdB {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A1 => {
                return Ok(Instruction::LdH {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A2 => {
                return Ok(Instruction::LdW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A3 => {
                return Ok(Instruction::LdD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A4 => {
                return Ok(Instruction::LdBu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A5 => {
                return Ok(Instruction::LdHu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A6 => {
                return Ok(Instruction::LdWu {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            // Store
            0x0A7 => {
                return Ok(Instruction::StB {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A8 => {
                return Ok(Instruction::StH {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0A9 => {
                return Ok(Instruction::StW {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0AA => {
                return Ok(Instruction::StD {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            // FP Load/Store
            0x0AB => {
                return Ok(Instruction::FldS {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0AC => {
                return Ok(Instruction::FldD {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0AD => {
                return Ok(Instruction::FstS {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            0x0AE => {
                return Ok(Instruction::FstD {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    imm12: sign_extend_12(imm12_raw),
                })
            }
            _ => {}
        }

        // 2RI16 opcodes (branches)
        match opc_2ri16 {
            0x16 => {
                return Ok(Instruction::Beq {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                })
            }
            0x17 => {
                return Ok(Instruction::Bne {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                })
            }
            0x18 => {
                return Ok(Instruction::Blt {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                })
            }
            0x19 => {
                return Ok(Instruction::Bge {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                })
            }
            0x1A => {
                return Ok(Instruction::Bltu {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                })
            }
            0x1B => {
                return Ok(Instruction::Bgeu {
                    rj: gpr_from_bits(rj),
                    rd: gpr_from_bits(rd),
                    offs16: sign_extend_16(imm16_raw),
                })
            }
            0x13 => {
                return Ok(Instruction::Jirl {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                    offs16: sign_extend_16(imm16_raw),
                })
            }
            _ => {}
        }

        // I26 format (unconditional branch)
        let hi10 = (word >> 16) & 0x3FF;
        let lo16 = word & 0xFFFF;
        let offs26 = (hi10 << 16) | lo16;
        match opc_i26 {
            0x14 => {
                return Ok(Instruction::B {
                    offs26: offs26 as i32,
                })
            }
            0x15 => {
                return Ok(Instruction::Bl {
                    offs26: offs26 as i32,
                })
            }
            _ => {}
        }

        // 1RI21 format (beqz/bnez)
        let imm21_hi5 = (word >> 21) & 0x1F;
        let imm21_lo16 = (word >> 5) & 0xFFFF;
        let imm21 = (imm21_hi5 << 16) | imm21_lo16;
        match opc_1ri21 {
            0x1C => {
                return Ok(Instruction::Beqz {
                    rj: gpr_from_bits(rj),
                    offs21: sign_extend_21(imm21),
                })
            }
            0x1D => {
                return Ok(Instruction::Bnez {
                    rj: gpr_from_bits(rj),
                    offs21: sign_extend_21(imm21),
                })
            }
            _ => {}
        }

        // 2R format (ext.w.h, ext.w.b)
        match opc_2r {
            0x000005A => {
                return Ok(Instruction::ExtWH {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                })
            }
            0x000005B => {
                return Ok(Instruction::ExtWB {
                    rd: gpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                })
            }
            // FP Move: fmov.s fd, fj
            0x000004E => {
                return Ok(Instruction::FmovS {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                })
            }
            // FP Move: fmov.d fd, fj
            0x000004F => {
                return Ok(Instruction::FmovD {
                    fd: fpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                })
            }
            // FP Move: movfr2gr.d rd, fj
            0x0000052 => {
                return Ok(Instruction::FmovGr2FprD {
                    rd: gpr_from_bits(rd),
                    fj: fpr_from_bits(rj),
                })
            }
            // FP Move: movgr2fr.d fd, rj
            0x0000053 => {
                return Ok(Instruction::FmovFpr2GrD {
                    fd: fpr_from_bits(rd),
                    rj: gpr_from_bits(rj),
                })
            }
            _ => {}
        }

        // 4R format (FP Compare: fcmp.cond.s/d)
        let opc_4r = (word >> 20) & 0xFFF;
        let cond = ((word >> 15) & 0x1F) as u8;
        let cd = (rd & 0x1F) as u8; // condition register destination in rd field
        match opc_4r {
            0x0C4 => {
                return Ok(Instruction::FCmpS {
                    cond,
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                    cd,
                })
            }
            0x0C5 => {
                return Ok(Instruction::FCmpD {
                    cond,
                    fj: fpr_from_bits(rj),
                    fk: fpr_from_bits(rk),
                    cd,
                })
            }
            _ => {}
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
}
