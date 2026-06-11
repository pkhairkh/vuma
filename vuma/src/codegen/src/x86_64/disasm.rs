//! # x86_64 Mnemonic Disassembler
//!
//! Decodes x86_64 machine-code bytes into human-readable mnemonic form.
//! Covers the most common REX-prefixed 64-bit instructions used by the
//! VUMA ISel lowering: add, sub, imul, idiv, and, or, xor, shl, shr,
//! sar, cmp, test, mov, lea, push, pop, call, ret, jmp, jcc, nop,
//! movsxd, cqo, setcc, cmovcc, neg, not, mul, div, xchg, syscall,
//! int3.

#[allow(unused_imports)]
use super::Gpr;

// ---------------------------------------------------------------------------
// Decoded instruction
// ---------------------------------------------------------------------------

/// A decoded x86_64 instruction with mnemonic and operand information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedInstruction {
    /// The mnemonic string (e.g. "add", "mov", "ret").
    pub mnemonic: String,
    /// The full assembly line (e.g. "add rax, rcx").
    pub text: String,
    /// Number of bytes consumed.
    pub len: usize,
}

impl std::fmt::Display for DecodedInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

// ---------------------------------------------------------------------------
// Decode error
// ---------------------------------------------------------------------------

/// Error produced when decoding fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The byte slice is too short.
    Truncated { needed: usize, available: usize },
    /// The byte sequence is not a recognised instruction.
    UnknownEncoding { bytes: Vec<u8> },
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated { needed, available } => {
                write!(f, "truncated: need {needed} bytes, have {available}")
            }
            DecodeError::UnknownEncoding { bytes } => {
                let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02x}")).collect();
                write!(f, "unknown encoding: {}", hex.join(" "))
            }
        }
    }
}

impl std::error::Error for DecodeError {}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn gpr_name_64(idx: u8) -> String {
    match idx {
        0 => "rax".into(),
        1 => "rcx".into(),
        2 => "rdx".into(),
        3 => "rbx".into(),
        4 => "rsp".into(),
        5 => "rbp".into(),
        6 => "rsi".into(),
        7 => "rdi".into(),
        8 => "r8".into(),
        9 => "r9".into(),
        10 => "r10".into(),
        11 => "r11".into(),
        12 => "r12".into(),
        13 => "r13".into(),
        14 => "r14".into(),
        15 => "r15".into(),
        _ => format!("r??({idx})"),
    }
}

fn gpr_name_8(idx: u8, has_rex: bool) -> String {
    match idx {
        0 => "al".into(),
        1 => "cl".into(),
        2 => "dl".into(),
        3 => "bl".into(),
        4 if has_rex => "spl".into(),
        4 => "ah".into(),
        5 if has_rex => "bpl".into(),
        5 => "ch".into(),
        6 if has_rex => "sil".into(),
        6 => "dh".into(),
        7 if has_rex => "dil".into(),
        7 => "bh".into(),
        8 => "r8b".into(),
        9 => "r9b".into(),
        10 => "r10b".into(),
        11 => "r11b".into(),
        12 => "r12b".into(),
        13 => "r13b".into(),
        14 => "r14b".into(),
        15 => "r15b".into(),
        _ => format!("??b({idx})"),
    }
}

/// Decode ModR/M reg and r/m fields (register-register mode, mod=3 only).
fn decode_modrm_reg_rm(
    bytes: &[u8],
    pos: usize,
    rex_r: bool,
    rex_b: bool,
) -> (u8, u8, usize) {
    if pos >= bytes.len() {
        return (0, 0, pos);
    }
    let modrm = bytes[pos];
    let new_pos = pos + 1;
    let reg = ((modrm >> 3) & 7) | (if rex_r { 8 } else { 0 });
    let rm = (modrm & 7) | (if rex_b { 8 } else { 0 });
    (reg, rm, new_pos)
}

/// Decode a ModR/M byte for memory operands (simplified: only reg-reg and
/// base+disp modes used by our encoder).
fn decode_modrm_mem(
    bytes: &[u8],
    pos: usize,
    rex_r: bool,
    _rex_x: bool,
    rex_b: bool,
) -> (u8, u8, i32, usize) {
    if pos >= bytes.len() {
        return (0, 0, 0, pos);
    }
    let modrm = bytes[pos];
    let new_pos = pos + 1;
    let reg = ((modrm >> 3) & 7) | (if rex_r { 8 } else { 0 });
    let mod_bits = (modrm >> 6) & 3;
    let rm_raw = modrm & 7;

    if mod_bits == 3 {
        // register-direct
        let rm = rm_raw | (if rex_b { 8 } else { 0 });
        return (reg, rm, 0, new_pos);
    }

    let base = rm_raw | (if rex_b { 8 } else { 0 });

    // SIB byte if rm == 4 (RSP)
    let _has_sib = rm_raw == 4;
    let mut adv = new_pos;
    if _has_sib && adv < bytes.len() {
        adv += 1; // skip SIB
    }

    let disp = match mod_bits {
        // RBP/R13 with mod=0 needs disp32
        0 if rm_raw == 5 && adv + 4 <= bytes.len() => {
            let d = i32::from_le_bytes(bytes[adv..adv + 4].try_into().unwrap_or([0; 4]));
            adv += 4;
            d
        }
        1 if adv < bytes.len() => {
            let d = bytes[adv] as i8 as i32;
            adv += 1;
            d
        }
        2 if adv + 4 <= bytes.len() => {
            let d = i32::from_le_bytes(bytes[adv..adv + 4].try_into().unwrap_or([0; 4]));
            adv += 4;
            d
        }
        _ => 0,
    };

    (reg, base, disp, adv)
}

// ---------------------------------------------------------------------------
// Main decode entry point
// ---------------------------------------------------------------------------

/// Decode a single x86_64 instruction from the byte stream.
///
/// Returns the decoded instruction and the number of bytes consumed.
pub fn decode_one(bytes: &[u8]) -> Result<DecodedInstruction, DecodeError> {
    if bytes.is_empty() {
        return Err(DecodeError::Truncated {
            needed: 1,
            available: 0,
        });
    }

    let mut pos = 0usize;

    // Skip legacy prefixes
    while pos < bytes.len() && matches!(bytes[pos], 0x66 | 0x67 | 0xF2 | 0xF3) {
        pos += 1;
    }

    if pos >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: 1,
            available: bytes.len(),
        });
    }

    // REX prefix
    #[allow(unused_assignments)]
    let mut rex = 0u8;
    let mut rex_w = false;
    let mut rex_r = false;
    let mut rex_x = false;
    let mut rex_b = false;
    let has_rex = if bytes[pos] >= 0x40 && bytes[pos] <= 0x4F {
        rex = bytes[pos];
        rex_w = (rex & 0x08) != 0;
        rex_r = (rex & 0x04) != 0;
        rex_x = (rex & 0x02) != 0;
        rex_b = (rex & 0x01) != 0;
        pos += 1;
        true
    } else {
        false
    };

    if pos >= bytes.len() {
        return Err(DecodeError::Truncated {
            needed: pos + 1,
            available: bytes.len(),
        });
    }

    let opcode = bytes[pos];
    pos += 1;

    let text = match opcode {
        // NOP
        0x90 => "nop".to_string(),

        // RET
        0xC3 => "ret".to_string(),

        // INT3
        0xCC => "int3".to_string(),

        // CQO (REX.W + 99)
        0x99 if rex_w => "cqo".to_string(),

        // PUSH r64
        0x50..=0x57 => {
            let reg_idx = (opcode - 0x50) | (if rex_b { 8 } else { 0 });
            format!("push {}", gpr_name_64(reg_idx))
        }

        // POP r64
        0x58..=0x5F => {
            let reg_idx = (opcode - 0x58) | (if rex_b { 8 } else { 0 });
            format!("pop {}", gpr_name_64(reg_idx))
        }

        // MOV r64, imm64 (B8+rd with REX.W)
        0xB8..=0xBF if rex_w => {
            let reg_idx = (opcode - 0xB8) | (if rex_b { 8 } else { 0 });
            if pos + 8 <= bytes.len() {
                let imm =
                    u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap_or([0; 8]));
                pos += 8;
                format!("mov {}, {imm:#x}", gpr_name_64(reg_idx))
            } else {
                return Err(DecodeError::Truncated {
                    needed: pos + 8,
                    available: bytes.len(),
                });
            }
        }

        // JMP rel32
        0xE9 => {
            if pos + 4 <= bytes.len() {
                let rel =
                    i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                pos += 4;
                let target = (pos as u64).wrapping_add(rel as u64);
                format!("jmp {target:#x}")
            } else {
                return Err(DecodeError::Truncated {
                    needed: pos + 4,
                    available: bytes.len(),
                });
            }
        }

        // CALL rel32
        0xE8 => {
            if pos + 4 <= bytes.len() {
                let rel =
                    i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                pos += 4;
                let target = (pos as u64).wrapping_add(rel as u64);
                format!("call {target:#x}")
            } else {
                return Err(DecodeError::Truncated {
                    needed: pos + 4,
                    available: bytes.len(),
                });
            }
        }

        // Two-byte opcode (0F xx)
        0x0F => {
            if pos >= bytes.len() {
                return Err(DecodeError::Truncated {
                    needed: pos + 1,
                    available: bytes.len(),
                });
            }
            let op2 = bytes[pos];
            pos += 1;
            match op2 {
                // SYSCALL
                0x05 => "syscall".to_string(),

                // IMUL r64, r64
                0xAF => {
                    let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                    pos = np;
                    format!("imul {}, {}", gpr_name_64(r), gpr_name_64(rm))
                }

                // Jcc rel32
                0x80..=0x8F => {
                    let cc_name = match op2 & 0xF {
                        0 => "jo",
                        1 => "jno",
                        2 => "jb",
                        3 => "jae",
                        4 => "je",
                        5 => "jne",
                        6 => "jbe",
                        7 => "ja",
                        8 => "js",
                        9 => "jns",
                        0xA => "jp",
                        0xB => "jnp",
                        0xC => "jl",
                        0xD => "jge",
                        0xE => "jle",
                        0xF => "jg",
                        _ => "j??",
                    };
                    if pos + 4 <= bytes.len() {
                        let rel = i32::from_le_bytes(
                            bytes[pos..pos + 4].try_into().unwrap_or([0; 4]),
                        );
                        pos += 4;
                        let target = (pos as u64).wrapping_add(rel as u64);
                        format!("{cc_name} {target:#x}")
                    } else {
                        return Err(DecodeError::Truncated {
                            needed: pos + 4,
                            available: bytes.len(),
                        });
                    }
                }

                // MOVZX r64, r8
                0xB6 => {
                    let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                    pos = np;
                    format!("movzx {}, {}", gpr_name_64(r), gpr_name_8(rm, has_rex))
                }

                // MOVZX r64, r16
                0xB7 => {
                    let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                    pos = np;
                    format!("movzx {}, {}", gpr_name_64(r), gpr_name_64(rm))
                }

                // MOVSX r64, r8
                0xBE => {
                    let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                    pos = np;
                    format!("movsx {}, {}", gpr_name_64(r), gpr_name_8(rm, has_rex))
                }

                // MOVSX r64, r16
                0xBF => {
                    let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                    pos = np;
                    format!("movsx {}, {}", gpr_name_64(r), gpr_name_64(rm))
                }

                // SETcc r/m8
                0x90..=0x9F => {
                    let (_, rm, np) = decode_modrm_reg_rm(bytes, pos, false, rex_b);
                    pos = np;
                    let cc_name = match op2 & 0xF {
                        0 => "seto",
                        1 => "setno",
                        2 => "setb",
                        3 => "setae",
                        4 => "sete",
                        5 => "setne",
                        6 => "setbe",
                        7 => "seta",
                        8 => "sets",
                        9 => "setns",
                        0xA => "setp",
                        0xB => "setnp",
                        0xC => "setl",
                        0xD => "setge",
                        0xE => "setle",
                        0xF => "setg",
                        _ => "set??",
                    };
                    format!("{cc_name} {}", gpr_name_8(rm, has_rex))
                }

                // CMOVcc r64, r64
                0x40..=0x4F => {
                    let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                    pos = np;
                    let cc_name = match op2 & 0xF {
                        0 => "cmovo",
                        1 => "cmovno",
                        2 => "cmovb",
                        3 => "cmovae",
                        4 => "cmove",
                        5 => "cmovne",
                        6 => "cmovbe",
                        7 => "cmova",
                        8 => "cmovs",
                        9 => "cmovns",
                        0xA => "cmovp",
                        0xB => "cmovnp",
                        0xC => "cmovl",
                        0xD => "cmovge",
                        0xE => "cmovle",
                        0xF => "cmovg",
                        _ => "cmov??",
                    };
                    format!("{cc_name} {}, {}", gpr_name_64(r), gpr_name_64(rm))
                }

                _ => {
                    return Err(DecodeError::UnknownEncoding {
                        bytes: bytes[..pos].to_vec(),
                    });
                }
            }
        }

        // ALU reg-reg opcodes
        0x01 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("add {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x03 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("add {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }
        0x09 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("or {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x0B => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("or {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }
        0x21 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("and {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x23 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("and {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }
        0x29 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("sub {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x2B => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("sub {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }
        0x31 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("xor {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x33 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("xor {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }
        0x39 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("cmp {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x3B => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("cmp {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }
        0x85 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("test {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x89 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("mov {}, {}", gpr_name_64(rm), gpr_name_64(r))
        }
        0x8B => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("mov {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }

        // LEA r64, [r64+offset]
        0x8D => {
            let (r, base, disp, np) = decode_modrm_mem(bytes, pos, rex_r, rex_x, rex_b);
            pos = np;
            if disp == 0 {
                format!("lea {}, [{}]", gpr_name_64(r), gpr_name_64(base))
            } else {
                format!("lea {}, [{}+{disp}]", gpr_name_64(r), gpr_name_64(base))
            }
        }

        // MOVSXD r64, r64 (REX.W + 63)
        0x63 if rex_w => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            format!("movsxd {}, {}", gpr_name_64(r), gpr_name_64(rm))
        }

        // F7 /x (NEG, NOT, MUL, DIV, IDIV)
        0xF7 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            match r {
                2 => format!("not {}", gpr_name_64(rm)),
                3 => format!("neg {}", gpr_name_64(rm)),
                4 => format!("mul {}", gpr_name_64(rm)),
                6 => format!("div {}", gpr_name_64(rm)),
                7 => format!("idiv {}", gpr_name_64(rm)),
                _ => format!("f7 /{r}, {}", gpr_name_64(rm)),
            }
        }

        // D3 /x (shift by CL)
        0xD3 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            match r {
                4 => format!("shl {}, cl", gpr_name_64(rm)),
                5 => format!("shr {}, cl", gpr_name_64(rm)),
                7 => format!("sar {}, cl", gpr_name_64(rm)),
                _ => format!("d3 /{r}, {}", gpr_name_64(rm)),
            }
        }

        // C7 /0 + imm32 (MOV r/m64, imm32)
        0xC7 => {
            let (_, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            if pos + 4 <= bytes.len() {
                let imm =
                    i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                pos += 4;
                format!("mov {}, {imm}", gpr_name_64(rm))
            } else {
                return Err(DecodeError::Truncated {
                    needed: pos + 4,
                    available: bytes.len(),
                });
            }
        }

        // 81 /x + imm32 (ADD/SUB/etc r/m64, imm32)
        0x81 => {
            let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
            pos = np;
            if pos + 4 <= bytes.len() {
                let imm =
                    i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                pos += 4;
                let op_name = match r {
                    0 => "add",
                    1 => "or",
                    4 => "and",
                    5 => "sub",
                    6 => "xor",
                    7 => "cmp",
                    _ => "???",
                };
                format!("{op_name} {}, {imm}", gpr_name_64(rm))
            } else {
                return Err(DecodeError::Truncated {
                    needed: pos + 4,
                    available: bytes.len(),
                });
            }
        }

        // XCHG rax, r64
        0x91..=0x97 => {
            let reg_idx = (opcode - 0x90) | (if rex_b { 8 } else { 0 });
            format!("xchg rax, {}", gpr_name_64(reg_idx))
        }

        _ => {
            return Err(DecodeError::UnknownEncoding {
                bytes: bytes[..pos].to_vec(),
            });
        }
    };

    Ok(DecodedInstruction {
        mnemonic: text.split_whitespace().next().unwrap_or("").to_string(),
        text,
        len: pos,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::x86_64::{
        encode_add_reg_reg, encode_and_reg_reg, encode_call_rel32, encode_cmp_reg_reg,
        encode_imul_reg_reg, encode_jcc_rel32, encode_jmp_rel32, encode_mov_reg_reg,
        encode_or_reg_reg, encode_ret, encode_sub_reg_reg, encode_xor_reg_reg,
        encode_nop, encode_push, encode_pop, encode_shl_reg_cl, encode_shr_reg_cl,
        encode_sar_reg_cl, encode_lea_reg_mem, encode_mov_reg_mem, encode_mov_mem_reg,
        encode_neg_reg, encode_not_reg, encode_test_reg_reg, encode_idiv_reg,
        Gpr,
    };

    #[test]
    fn test_decode_add() {
        let bytes = encode_add_reg_reg(Gpr::Rax, Gpr::Rcx);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "add");
        assert!(decoded.text.contains("rax"));
        assert!(decoded.text.contains("rcx"));
    }

    #[test]
    fn test_decode_sub() {
        let bytes = encode_sub_reg_reg(Gpr::Rax, Gpr::Rdx);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "sub");
    }

    #[test]
    fn test_decode_mov_reg_reg() {
        let bytes = encode_mov_reg_reg(Gpr::Rax, Gpr::Rbx);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "mov");
    }

    #[test]
    fn test_decode_ret() {
        let bytes = encode_ret();
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.text, "ret");
    }

    #[test]
    fn test_decode_nop() {
        let bytes = encode_nop();
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.text, "nop");
    }

    #[test]
    fn test_decode_imul() {
        let bytes = encode_imul_reg_reg(Gpr::Rax, Gpr::Rcx);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "imul");
    }

    #[test]
    fn test_decode_xor() {
        let bytes = encode_xor_reg_reg(Gpr::Rax, Gpr::Rax);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "xor");
    }

    #[test]
    fn test_decode_and_or() {
        let and_bytes = encode_and_reg_reg(Gpr::Rax, Gpr::Rcx);
        let decoded = decode_one(&and_bytes).unwrap();
        assert_eq!(decoded.mnemonic, "and");

        let or_bytes = encode_or_reg_reg(Gpr::Rax, Gpr::Rdx);
        let decoded = decode_one(&or_bytes).unwrap();
        assert_eq!(decoded.mnemonic, "or");
    }

    #[test]
    fn test_decode_push_pop() {
        let push_bytes = encode_push(Gpr::Rbp);
        let decoded = decode_one(&push_bytes).unwrap();
        assert_eq!(decoded.mnemonic, "push");

        let pop_bytes = encode_pop(Gpr::Rbp);
        let decoded = decode_one(&pop_bytes).unwrap();
        assert_eq!(decoded.mnemonic, "pop");
    }

    #[test]
    fn test_decode_shifts() {
        let shl = encode_shl_reg_cl(Gpr::Rax);
        assert_eq!(decode_one(&shl).unwrap().mnemonic, "shl");

        let shr = encode_shr_reg_cl(Gpr::Rax);
        assert_eq!(decode_one(&shr).unwrap().mnemonic, "shr");

        let sar = encode_sar_reg_cl(Gpr::Rax);
        assert_eq!(decode_one(&sar).unwrap().mnemonic, "sar");
    }

    #[test]
    fn test_decode_cmp() {
        let bytes = encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "cmp");
    }

    #[test]
    fn test_decode_neg_not() {
        let neg = encode_neg_reg(Gpr::Rax);
        assert_eq!(decode_one(&neg).unwrap().mnemonic, "neg");

        let not = encode_not_reg(Gpr::Rax);
        assert_eq!(decode_one(&not).unwrap().mnemonic, "not");
    }

    #[test]
    fn test_decode_lea() {
        let bytes = encode_lea_reg_mem(Gpr::Rax, Gpr::Rbp, 16);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "lea");
    }

    #[test]
    fn test_decode_load_store() {
        let load = encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, 8);
        assert_eq!(decode_one(&load).unwrap().mnemonic, "mov");

        let store = encode_mov_mem_reg(Gpr::Rbp, 8, Gpr::Rax);
        assert_eq!(decode_one(&store).unwrap().mnemonic, "mov");
    }

    #[test]
    fn test_decode_idiv() {
        let bytes = encode_idiv_reg(Gpr::Rcx);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "idiv");
    }

    #[test]
    fn test_decode_jcc() {
        use crate::x86_64::Cc;
        let bytes = encode_jcc_rel32(Cc::Equal, 0x100);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "je");
    }

    #[test]
    fn test_decode_call() {
        let bytes = encode_call_rel32(0);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "call");
    }

    #[test]
    fn test_decode_test() {
        let bytes = encode_test_reg_reg(Gpr::Rax, Gpr::Rax);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "test");
    }

    #[test]
    fn test_roundtrip_encode_decode() {
        // Encode then decode, verify the mnemonic matches
        let bytes = encode_add_reg_reg(Gpr::Rdi, Gpr::Rsi);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "add");

        let bytes = encode_sub_reg_reg(Gpr::R8, Gpr::R9);
        let decoded = decode_one(&bytes).unwrap();
        assert_eq!(decoded.mnemonic, "sub");
    }

    #[test]
    fn test_decode_truncated() {
        let result = decode_one(&[]);
        assert!(matches!(result, Err(DecodeError::Truncated { .. })));
    }

    #[test]
    fn test_decode_unknown() {
        // 0x0F 0xFF is not a standard encoding
        let result = decode_one(&[0x0F, 0xFF]);
        assert!(matches!(result, Err(DecodeError::UnknownEncoding { .. })));
    }
}
