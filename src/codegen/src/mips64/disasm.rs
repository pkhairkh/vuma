//! # MIPS64 Mnemonic Disassembler
//!
//! Decodes MIPS64 32-bit big-endian machine code into `Instruction`
//! instances. Covers the R-type, I-type, and J-type instructions lowered
//! by the VUMA ISel. Display is already provided by the parent module.

use super::Fpr;
use super::Gpr;
use super::Instruction;

// ---------------------------------------------------------------------------
// Opcode constants (must match parent module)
// ---------------------------------------------------------------------------

const OPC_SPECIAL: u32 = 0x00;
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
const OPC_BEQ: u32 = 0x04;
const OPC_BNE: u32 = 0x05;
const OPC_BLEZ: u32 = 0x06;
const OPC_BGTZ: u32 = 0x07;
const OPC_LB: u32 = 0x20;
const OPC_LH: u32 = 0x21;
const OPC_LW: u32 = 0x23;
const OPC_LD: u32 = 0x37;
const OPC_LBU: u32 = 0x24;
const OPC_LHU: u32 = 0x25;
const OPC_LWU: u32 = 0x27;
const OPC_SB: u32 = 0x28;
const OPC_SH: u32 = 0x29;
const OPC_SW: u32 = 0x2B;
const OPC_SD: u32 = 0x3F;
const OPC_LWC1: u32 = 0x31;
const OPC_SWC1: u32 = 0x39;
const OPC_LDC1: u32 = 0x35;
const OPC_SDC1: u32 = 0x3D;
const OPC_J: u32 = 0x02;
const OPC_JAL: u32 = 0x03;

// COP1 (coprocessor 1 / FPU) opcode and field constants — must match the
// encoder constants in the parent module. COP1 R-type format is:
//   `COP1[31:26] | fmt[25:21] | ft[20:16] | fs[15:11] | fd[10:6] | func[5:0]`
// For the GPR<->FPR move instructions, `ft` holds the GPR (`rt`) and `fs`
// holds the FPR; `fd` and `func` are zero.
const OPC_COP1: u32 = 0x11;

// COP1 fmt field values (bits 25:21) for type conversions.
const FMT_S: u32 = 16; // single
const FMT_D: u32 = 17; // double
const FMT_W: u32 = 20; // word (32-bit integer)
const FMT_L: u32 = 21; // long  (64-bit integer)

// COP1 fmt field values for the GPR<->FPR move instructions.
const FMT_MF: u32 = 0x00; // MFC1
const FMT_DMF: u32 = 0x01; // DMFC1
const FMT_MT: u32 = 0x04; // MTC1
const FMT_DMT: u32 = 0x05; // DMTC1

// COP1 function codes for FP conversion instructions. Several conversions
// share the same funct code (e.g. all `cvt.*.s` use 0x20); the (fmt, funct)
// pair is the unique key.
const FN_CVT_S: u32 = 0x20; // cvt.*.s  (cvt.s.d / cvt.s.w / cvt.s.l)
const FN_CVT_D: u32 = 0x21; // cvt.*.d  (cvt.d.s / cvt.d.w / cvt.d.l)
const FN_CVT_W: u32 = 0x24; // cvt.*.w  (cvt.w.s / cvt.w.d)
const FN_CVT_L: u32 = 0x25; // cvt.*.l  (cvt.l.s / cvt.l.d)

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
const FN_JR: u32 = 0x08;
const FN_JALR: u32 = 0x09;
const FN_SYSCALL: u32 = 0x0C;
const FN_BREAK: u32 = 0x0D;

// ---------------------------------------------------------------------------
// Decode error
// ---------------------------------------------------------------------------

/// Error produced when MIPS64 decoding fails.
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
                write!(f, "unknown MIPS64 encoding: 0x{word:08x}")
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
        0 => Gpr::Zero,
        1 => Gpr::At,
        2 => Gpr::V0,
        3 => Gpr::V1,
        4 => Gpr::A0,
        5 => Gpr::A1,
        6 => Gpr::A2,
        7 => Gpr::A3,
        8 => Gpr::T0,
        9 => Gpr::T1,
        10 => Gpr::T2,
        11 => Gpr::T3,
        12 => Gpr::T4,
        13 => Gpr::T5,
        14 => Gpr::T6,
        15 => Gpr::T7,
        16 => Gpr::S0,
        17 => Gpr::S1,
        18 => Gpr::S2,
        19 => Gpr::S3,
        20 => Gpr::S4,
        21 => Gpr::S5,
        22 => Gpr::S6,
        23 => Gpr::S7,
        24 => Gpr::T8,
        25 => Gpr::T9,
        26 => Gpr::K0,
        27 => Gpr::K1,
        28 => Gpr::Gp,
        29 => Gpr::Sp,
        30 => Gpr::Fp,
        31 => Gpr::Ra,
        _ => Gpr::Zero,
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

fn sign_extend_16(val: u32) -> i32 {
    if val & 0x8000 != 0 {
        (val | 0xFFFF0000) as i32
    } else {
        val as i32
    }
}

// ---------------------------------------------------------------------------
// Decode entry point
// ---------------------------------------------------------------------------

impl Instruction {
    /// Decode a single MIPS64 instruction from 4 big-endian bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        if bytes.len() < 4 {
            return Err(DecodeError::Truncated {
                needed: 4,
                available: bytes.len(),
            });
        }
        let word = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let opcode = (word >> 26) & 0x3F;
        let rs = (word >> 21) & 0x1F;
        let rt = (word >> 16) & 0x1F;
        let rd = (word >> 11) & 0x1F;
        let sa = (word >> 6) & 0x1F;
        let funct = word & 0x3F;
        let imm = word & 0xFFFF;
        let target = word & 0x03FF_FFFF;

        // NOP
        if word == 0 {
            return Ok(Instruction::Nop);
        }

        match opcode {
            OPC_SPECIAL => match funct {
                FN_ADD => Ok(Instruction::Add {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_ADDU => Ok(Instruction::Addu {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_SUB => Ok(Instruction::Sub {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_SUBU => Ok(Instruction::Subu {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_AND => Ok(Instruction::And {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_OR => Ok(Instruction::Or {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_XOR => Ok(Instruction::Xor {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_NOR => Ok(Instruction::Nor {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_SLT => Ok(Instruction::Slt {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_SLTU => Ok(Instruction::Sltu {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_SLL => Ok(Instruction::Sll {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    sa,
                }),
                FN_SRL => Ok(Instruction::Srl {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    sa,
                }),
                FN_SRA => Ok(Instruction::Sra {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    sa,
                }),
                FN_SLLV => Ok(Instruction::Sllv {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    rs: gpr_from_bits(rs),
                }),
                FN_SRLV => Ok(Instruction::Srlv {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    rs: gpr_from_bits(rs),
                }),
                FN_SRAV => Ok(Instruction::Srav {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    rs: gpr_from_bits(rs),
                }),
                FN_MULT => Ok(Instruction::Mult {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_MULTU => Ok(Instruction::Multu {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DIV => Ok(Instruction::Div {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DIVU => Ok(Instruction::Divu {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_MFHI => Ok(Instruction::Mfhi {
                    rd: gpr_from_bits(rd),
                }),
                FN_MFLO => Ok(Instruction::Mflo {
                    rd: gpr_from_bits(rd),
                }),
                FN_DADD => Ok(Instruction::Dadd {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DSUB => Ok(Instruction::Dsub {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DADDU => Ok(Instruction::Daddu {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DSUBU => Ok(Instruction::Dsubu {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DSLL => Ok(Instruction::Dsll {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    sa,
                }),
                FN_DSRL => Ok(Instruction::Dsrl {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    sa,
                }),
                FN_DSRA => Ok(Instruction::Dsra {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    sa,
                }),
                FN_DSLLV => Ok(Instruction::Dsllv {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    rs: gpr_from_bits(rs),
                }),
                FN_DSRLV => Ok(Instruction::Dsrlv {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    rs: gpr_from_bits(rs),
                }),
                FN_DSRAV => Ok(Instruction::Dsrav {
                    rd: gpr_from_bits(rd),
                    rt: gpr_from_bits(rt),
                    rs: gpr_from_bits(rs),
                }),
                FN_DMULT => Ok(Instruction::Dmult {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DMULTU => Ok(Instruction::Dmultu {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DDIV => Ok(Instruction::Ddiv {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_DDIVU => Ok(Instruction::Ddivu {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                }),
                FN_JR => Ok(Instruction::Jr {
                    rs: gpr_from_bits(rs),
                }),
                FN_JALR => Ok(Instruction::Jalr {
                    rd: gpr_from_bits(rd),
                    rs: gpr_from_bits(rs),
                }),
                FN_SYSCALL => Ok(Instruction::Syscall {
                    code: (word >> 6) & 0xFFFFF,
                }),
                FN_BREAK => Ok(Instruction::Break {
                    code: (word >> 6) & 0xFFFFF,
                }),
                _ => Err(DecodeError::UnknownEncoding { word }),
            },

            OPC_ADDI => Ok(Instruction::Addi {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm: sign_extend_16(imm),
            }),
            OPC_ADDIU => Ok(Instruction::Addiu {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm: sign_extend_16(imm),
            }),
            OPC_ANDI => Ok(Instruction::Andi {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm,
            }),
            OPC_ORI => Ok(Instruction::Ori {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm,
            }),
            OPC_XORI => Ok(Instruction::Xori {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm,
            }),
            OPC_SLTI => Ok(Instruction::Slti {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm: sign_extend_16(imm),
            }),
            OPC_SLTIU => Ok(Instruction::Sltiu {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm: sign_extend_16(imm),
            }),
            OPC_LUI => Ok(Instruction::Lui {
                rt: gpr_from_bits(rt),
                imm,
            }),
            OPC_DADDI => Ok(Instruction::Daddi {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm: sign_extend_16(imm),
            }),
            OPC_DADDIU => Ok(Instruction::Daddiu {
                rt: gpr_from_bits(rt),
                rs: gpr_from_bits(rs),
                imm: sign_extend_16(imm),
            }),

            OPC_BEQ => {
                let off_words = sign_extend_16(imm);
                Ok(Instruction::Beq {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                    offset: off_words << 2,
                })
            }
            OPC_BNE => {
                let off_words = sign_extend_16(imm);
                Ok(Instruction::Bne {
                    rs: gpr_from_bits(rs),
                    rt: gpr_from_bits(rt),
                    offset: off_words << 2,
                })
            }
            OPC_BLEZ => {
                let off_words = sign_extend_16(imm);
                Ok(Instruction::Blez {
                    rs: gpr_from_bits(rs),
                    offset: off_words << 2,
                })
            }
            OPC_BGTZ => {
                let off_words = sign_extend_16(imm);
                Ok(Instruction::Bgtz {
                    rs: gpr_from_bits(rs),
                    offset: off_words << 2,
                })
            }

            // Loads
            OPC_LB => Ok(Instruction::Lb {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_LH => Ok(Instruction::Lh {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_LW => Ok(Instruction::Lw {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_LD => Ok(Instruction::Ld {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_LBU => Ok(Instruction::Lbu {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_LHU => Ok(Instruction::Lhu {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_LWU => Ok(Instruction::Lwu {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),

            // Stores
            OPC_SB => Ok(Instruction::Sb {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_SH => Ok(Instruction::Sh {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_SW => Ok(Instruction::Sw {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_SD => Ok(Instruction::Sd {
                rt: gpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),

            // FP load/store
            OPC_LWC1 => Ok(Instruction::Lwc1 {
                ft: fpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_SWC1 => Ok(Instruction::Swc1 {
                ft: fpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_LDC1 => Ok(Instruction::Ldc1 {
                ft: fpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),
            OPC_SDC1 => Ok(Instruction::Sdc1 {
                ft: fpr_from_bits(rt),
                base: gpr_from_bits(rs),
                offset: sign_extend_16(imm),
            }),

            // J-type
            OPC_J => Ok(Instruction::J { target }),
            OPC_JAL => Ok(Instruction::Jal { target }),

            // COP1 (coprocessor 1 / FPU) — FP moves and conversions.
            //
            // Field layout for our encodings:
            //   fmt[25:21] (== `rs`) | ft/rt[20:16] (== `rt`)
            //   | fs[15:11] (== `rd`) | fd[10:6] (== `sa`) | func[5:0]
            // For GPR<->FPR moves: funct == 0, ft holds the GPR (`rt`), fs
            // holds the FPR, fd == 0. For CVT.*.*: ft == 0, fs is the source
            // FPR, fd is the destination FPR, and (fmt, funct) is the
            // unique key (several conversions share a funct code).
            OPC_COP1 => {
                let fmt = rs;
                let fs_bits = rd;
                let fd_bits = sa;
                match (fmt, funct) {
                    // GPR<-FPR / GPR->FPR moves (funct == 0)
                    (FMT_MF, 0) => Ok(Instruction::Mfc1 {
                        rt: gpr_from_bits(rt),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_DMF, 0) => Ok(Instruction::Dmfc1 {
                        rt: gpr_from_bits(rt),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_MT, 0) => Ok(Instruction::Mtc1 {
                        rt: gpr_from_bits(rt),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_DMT, 0) => Ok(Instruction::Dmtc1 {
                        rt: gpr_from_bits(rt),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    // FP type conversions: (fmt, funct) is the unique key.
                    (FMT_S, FN_CVT_D) => Ok(Instruction::CvtDS {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_D, FN_CVT_S) => Ok(Instruction::CvtSD {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_W, FN_CVT_S) => Ok(Instruction::CvtSW {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_W, FN_CVT_D) => Ok(Instruction::CvtDW {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_S, FN_CVT_W) => Ok(Instruction::CvtWS {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_D, FN_CVT_W) => Ok(Instruction::CvtWD {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_L, FN_CVT_S) => Ok(Instruction::CvtSL {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_L, FN_CVT_D) => Ok(Instruction::CvtDL {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_S, FN_CVT_L) => Ok(Instruction::CvtLS {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    (FMT_D, FN_CVT_L) => Ok(Instruction::CvtLD {
                        fd: fpr_from_bits(fd_bits),
                        fs: fpr_from_bits(fs_bits),
                    }),
                    _ => Err(DecodeError::UnknownEncoding { word }),
                }
            }

            _ => Err(DecodeError::UnknownEncoding { word }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mips64::Gpr as G;

    #[test]
    fn test_decode_add() {
        let instr = Instruction::Add {
            rd: G::T0,
            rs: G::A0,
            rt: G::A1,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_sub() {
        let instr = Instruction::Sub {
            rd: G::V0,
            rs: G::A0,
            rt: G::A1,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_and_or_xor() {
        for instr in [
            Instruction::And {
                rd: G::T0,
                rs: G::T1,
                rt: G::T2,
            },
            Instruction::Or {
                rd: G::T0,
                rs: G::T1,
                rt: G::T2,
            },
            Instruction::Xor {
                rd: G::T0,
                rs: G::T1,
                rt: G::T2,
            },
        ] {
            let bytes = instr.encode();
            let decoded = Instruction::decode(&bytes).unwrap();
            assert_eq!(format!("{decoded}"), format!("{instr}"));
        }
    }

    #[test]
    fn test_decode_addiu() {
        let instr = Instruction::Addiu {
            rt: G::T0,
            rs: G::Sp,
            imm: 16,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_ld_sd() {
        let ld = Instruction::Ld {
            rt: G::T0,
            base: G::Sp,
            offset: 8,
        };
        let bytes = ld.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{ld}"));

        let sd = Instruction::Sd {
            rt: G::T0,
            base: G::Sp,
            offset: 8,
        };
        let bytes = sd.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{sd}"));
    }

    #[test]
    fn test_decode_sll() {
        let instr = Instruction::Sll {
            rd: G::T0,
            rt: G::T1,
            sa: 2,
        };
        let bytes = instr.encode();
        let decoded = Instruction::decode(&bytes).unwrap();
        assert_eq!(format!("{decoded}"), format!("{instr}"));
    }

    #[test]
    fn test_decode_nop() {
        let decoded = Instruction::decode(&[0x00, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(decoded, Instruction::Nop);
    }

    #[test]
    fn test_decode_truncated() {
        let result = Instruction::decode(&[0x00, 0x00]);
        assert!(matches!(result, Err(DecodeError::Truncated { .. })));
    }
}
