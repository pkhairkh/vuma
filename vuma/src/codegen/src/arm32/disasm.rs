//! # ARM32 Mnemonic Disassembler
//!
//! Decodes ARM32 32-bit little-endian machine code into `Instruction`
//! instances. Covers the data-processing, load/store, branch, and multiply
//! instructions lowered by the VUMA ISel. Display is already provided by
//! the parent module.

use super::Condition;
use super::Gpr;
use super::Instruction;

// ---------------------------------------------------------------------------
// Decode error
// ---------------------------------------------------------------------------

/// Error produced when ARM32 decoding fails.
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
                write!(f, "unknown ARM32 encoding: 0x{word:08x}")
            }
        }
    }
}

impl std::error::Error for DecodeError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn cond_from_bits(bits: u32) -> Condition {
    match bits {
        0b0000 => Condition::Eq,
        0b0001 => Condition::Ne,
        0b0010 => Condition::Cs,
        0b0011 => Condition::Cc,
        0b0100 => Condition::Mi,
        0b0101 => Condition::Pl,
        0b0110 => Condition::Vs,
        0b0111 => Condition::Vc,
        0b1000 => Condition::Hi,
        0b1001 => Condition::Ls,
        0b1010 => Condition::Ge,
        0b1011 => Condition::Lt,
        0b1100 => Condition::Gt,
        0b1101 => Condition::Le,
        0b1110 => Condition::Al,
        _ => Condition::Al,
    }
}

fn gpr_from_bits(bits: u32) -> Gpr {
    match bits {
        0 => Gpr::R0,
        1 => Gpr::R1,
        2 => Gpr::R2,
        3 => Gpr::R3,
        4 => Gpr::R4,
        5 => Gpr::R5,
        6 => Gpr::R6,
        7 => Gpr::R7,
        8 => Gpr::R8,
        9 => Gpr::R9,
        10 => Gpr::R10,
        11 => Gpr::R11,
        12 => Gpr::R12,
        13 => Gpr::R13,
        14 => Gpr::R14,
        15 => Gpr::R15,
        _ => Gpr::R0,
    }
}

fn sign_extend_24(val: u32) -> i32 {
    if val & 0x800000 != 0 {
        (val | 0xFF000000) as i32
    } else {
        val as i32
    }
}

fn sign_extend_12(val: u32) -> i32 {
    if val & 0x800 != 0 {
        (val | 0xFFFFF000) as i32
    } else {
        val as i32
    }
}

// ---------------------------------------------------------------------------
// Decode entry point
// ---------------------------------------------------------------------------

impl Instruction {
    /// Decode a single ARM32 instruction from 4 little-endian bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < 4 {
            return Err(DecodeError::Truncated {
                needed: 4,
                available: bytes.len(),
            });
        }
        let word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let cond = cond_from_bits((word >> 28) & 0xF);

        // Branch: cond 101 L offset24
        if (word >> 25) & 0x7 == 0b101 {
            let link = (word >> 24) & 1 != 0;
            let offset24 = word & 0x00FF_FFFF;
            let offset = sign_extend_24(offset24) << 2;
            if link {
                return Ok(Instruction::Bl { offset, cond });
            }
            return Ok(Instruction::B { offset, cond });
        }

        // BX: cond 00010010 111111111111 0001 Rm
        if (word & 0x0FFF_FFF0) == 0x012F_FF10 {
            let rm = (word) & 0xF;
            return Ok(Instruction::Bx { rm: gpr_from_bits(rm), cond });
        }

        // BLX reg: cond 00010010 111111111111 0011 Rm
        if (word & 0x0FFF_FFF0) == 0x012F_FF30 {
            let rm = word & 0xF;
            return Ok(Instruction::BlxReg { rm: gpr_from_bits(rm), cond });
        }

        // Load/store word/byte with immediate offset: cond 01 I P U B W L Rn Rd offset12
        let ls_bits = (word >> 25) & 0x7;
        if ls_bits == 0b010 {
            let pre = (word >> 24) & 1 != 0;
            let up = (word >> 23) & 1 != 0;
            let b = (word >> 22) & 1 != 0;
            let w = (word >> 21) & 1 != 0;
            let load = (word >> 20) & 1 != 0;
            let rn = (word >> 16) & 0xF;
            let rd = (word >> 12) & 0xF;
            let offset12 = word & 0xFFF;

            if pre && !w {
                let off = if up {
                    offset12 as i32
                } else {
                    -(offset12 as i32)
                };
                if load && !b {
                    return Ok(Instruction::Ldr { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), offset: off, cond });
                }
                if !load && !b {
                    return Ok(Instruction::Str { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), offset: off, cond });
                }
                if load && b {
                    return Ok(Instruction::Ldrb { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), offset: off, cond });
                }
                if !load && b {
                    return Ok(Instruction::Strb { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), offset: off, cond });
                }
            }
        }

        // Data processing: cond 00 I opcode S Rn Rd operand2
        let dp_bits = (word >> 26) & 0x3;
        if dp_bits == 0b00 {
            let i_bit = (word >> 25) & 1;
            let opcode = (word >> 21) & 0xF;
            let s_bit = (word >> 20) & 1;
            let rn = (word >> 16) & 0xF;
            let rd = (word >> 12) & 0xF;

            // Only handle register operand2 (I=0) for simplicity
            if i_bit == 0 {
                let rm = word & 0xF;
                let shift_type = (word >> 5) & 0x3;
                let shift_imm = (word >> 7) & 0x1F;
                let shift_by_reg = (word >> 4) & 1 != 0;
                let rs = (word >> 8) & 0xF;

                if !shift_by_reg && shift_imm == 0 && shift_type == 0 {
                    // No shift
                    match opcode {
                        0b0100 if s_bit == 0 => return Ok(Instruction::Add { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b0010 if s_bit == 0 => return Ok(Instruction::Sub { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b0000 if s_bit == 0 => return Ok(Instruction::And { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b1100 if s_bit == 0 => return Ok(Instruction::Orr { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b0001 if s_bit == 0 => return Ok(Instruction::Eor { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b1110 if s_bit == 0 => return Ok(Instruction::Bic { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b1101 if s_bit == 0 && rn == 0 => return Ok(Instruction::Mov { rd: gpr_from_bits(rd), rm: gpr_from_bits(rm), cond }),
                        0b1111 if s_bit == 0 && rn == 0 => return Ok(Instruction::Mvn { rd: gpr_from_bits(rd), rm: gpr_from_bits(rm), cond }),
                        0b1010 if s_bit == 1 => return Ok(Instruction::Cmp { rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b1011 if s_bit == 1 => return Ok(Instruction::Cmn { rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b1000 if s_bit == 1 => return Ok(Instruction::Tst { rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        0b1001 if s_bit == 1 => return Ok(Instruction::Teq { rn: gpr_from_bits(rn), rm: gpr_from_bits(rm), cond }),
                        _ => {}
                    }
                }

                // Shift by immediate (encoded as MOV Rd, Rm, shift #imm)
                if !shift_by_reg && shift_imm != 0 && opcode == 0b1101 && s_bit == 0 {
                    match shift_type {
                        0 => return Ok(Instruction::LslImm { rd: gpr_from_bits(rd), rm: gpr_from_bits(rm), shift_imm, cond }),
                        1 => return Ok(Instruction::LsrImm { rd: gpr_from_bits(rd), rm: gpr_from_bits(rm), shift_imm, cond }),
                        2 => return Ok(Instruction::AsrImm { rd: gpr_from_bits(rd), rm: gpr_from_bits(rm), shift_imm, cond }),
                        3 => return Ok(Instruction::RorImm { rd: gpr_from_bits(rd), rm: gpr_from_bits(rm), shift_imm, cond }),
                        _ => {}
                    }
                }

                // Shift by register
                if shift_by_reg && opcode == 0b1101 && s_bit == 0 {
                    match shift_type {
                        0 => return Ok(Instruction::LslReg { rd: gpr_from_bits(rd), rn: gpr_from_bits(rm), rs: gpr_from_bits(rs), cond }),
                        1 => return Ok(Instruction::LsrReg { rd: gpr_from_bits(rd), rn: gpr_from_bits(rm), rs: gpr_from_bits(rs), cond }),
                        2 => return Ok(Instruction::AsrReg { rd: gpr_from_bits(rd), rn: gpr_from_bits(rm), rs: gpr_from_bits(rs), cond }),
                        3 => return Ok(Instruction::RorReg { rd: gpr_from_bits(rd), rn: gpr_from_bits(rm), rs: gpr_from_bits(rs), cond }),
                        _ => {}
                    }
                }
            }

            // Immediate operand2 (I=1)
            if i_bit == 1 {
                let rotate = (word >> 8) & 0xF;
                let imm8 = word & 0xFF;
                match opcode {
                    0b0100 if s_bit == 0 => return Ok(Instruction::AddImm { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rotate, imm8, cond }),
                    0b0010 if s_bit == 0 => return Ok(Instruction::SubImm { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rotate, imm8, cond }),
                    0b1101 if s_bit == 0 && rn == 0 => return Ok(Instruction::MovImm { rd: gpr_from_bits(rd), rotate, imm8, cond }),
                    0b1010 if s_bit == 1 => return Ok(Instruction::CmpImm { rn: gpr_from_bits(rn), rotate, imm8, cond }),
                    _ => {}
                }
            }
        }

        // MUL: cond 000000 S Rd Rn Rs 1001 Rm
        if (word & 0x0FE000F0) == 0x00000090 {
            let s_bit = (word >> 20) & 1 != 0;
            let rd = (word >> 16) & 0xF;
            let rn = (word >> 12) & 0xF;
            let rs = (word >> 8) & 0xF;
            let rm = word & 0xF;
            if !s_bit {
                return Ok(Instruction::Mul { rd: gpr_from_bits(rd), rn: gpr_from_bits(rn), rs: gpr_from_bits(rs), rm: gpr_from_bits(rm), cond });
            }
        }

        // NOP: MOV R0, R0 = 0xE1A00000
        if word == 0xE1A0_0000 {
            return Ok(Instruction::Nop);
        }

        // SVC: cond 1111 imm24
        if (word >> 24) & 0xF == 0b1111 {
            let imm24 = word & 0x00FF_FFFF;
            return Ok(Instruction::Svc { imm24, cond });
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
    use crate::arm32::Gpr as G;
    use crate::arm32::Condition as C;

    #[test]
    fn test_decode_add() {
        let instr = Instruction::Add { rd: G::R0, rn: G::R1, rm: G::R2, cond: C::Al };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_sub() {
        let instr = Instruction::Sub { rd: G::R3, rn: G::R4, rm: G::R5, cond: C::Al };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_and_orr_eor() {
        for instr in [
            Instruction::And { rd: G::R0, rn: G::R1, rm: G::R2, cond: C::Al },
            Instruction::Orr { rd: G::R0, rn: G::R1, rm: G::R2, cond: C::Al },
            Instruction::Eor { rd: G::R0, rn: G::R1, rm: G::R2, cond: C::Al },
        ] {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"));
        }
    }

    #[test]
    fn test_decode_mov() {
        let instr = Instruction::Mov { rd: G::R0, rm: G::R1, cond: C::Al };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_ldr_str() {
        let ldr = Instruction::Ldr { rd: G::R0, rn: G::R1, offset: 4, cond: C::Al };
        let bytes = ldr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{ldr}"));

        let str = Instruction::Str { rd: G::R0, rn: G::R1, offset: 4, cond: C::Al };
        let bytes = str.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{str}"));
    }

    #[test]
    fn test_decode_cmp() {
        let instr = Instruction::Cmp { rn: G::R0, rm: G::R1, cond: C::Al };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_branch() {
        let b = Instruction::B { offset: 8, cond: C::Al };
        let bytes = b.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{b}"));

        let bl = Instruction::Bl { offset: 12, cond: C::Al };
        let bytes = bl.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{bl}"));
    }

    #[test]
    fn test_decode_nop() {
        let decoded = Instruction::decode(&0xE1A0_0000u32.to_le_bytes()).unwrap();
        assert_eq!(decoded, Instruction::Nop);
    }

    #[test]
    fn test_decode_truncated() {
        let result = Instruction::decode(&[0x00, 0x00]);
        assert!(matches!(result, Err(DecodeError::Truncated { .. })));
    }
}
