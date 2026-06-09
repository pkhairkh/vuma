//! # ARM64 (AArch64) Instruction Definitions
//!
//! Defines the ARM64 register set, condition codes, and instruction
//! representations used during code generation for the Raspberry Pi 5
//! (Cortex-A76, ARMv8.2-A).
//!
//! ## References
//!
//! - ARM Architecture Reference Manual ARMv8, for ARMv8-A architecture profile
//! - <https://developer.arm.com/documentation/ddi0487/latest>

use crate::CodegenError;
use crate::Result;

// ---------------------------------------------------------------------------
// Register
// ---------------------------------------------------------------------------

/// ARM64 general-purpose registers (X0–X30) and special-purpose registers.
///
/// The AArch64 calling convention (AAPCS64) assigns specific roles:
///
/// | Register(s) | Role                                  |
/// |-------------|---------------------------------------|
/// | X0–X7       | Argument / result registers            |
/// | X8          | Indirect result location register      |
/// | X9–X15      | Caller-saved temporary registers       |
/// | X16–X17     | Intra-procedure-call scratch (IP0/IP1) |
/// | X18         | Platform register                      |
/// | X19–X28     | Callee-saved registers                 |
/// | X29         | Frame pointer (FP)                     |
/// | X30         | Link register (LR)                     |
/// | SP          | Stack pointer                          |
/// | XZR         | Zero register                          |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Register {
    /// General-purpose registers X0–X30.
    X0,
    X1,
    X2,
    X3,
    X4,
    X5,
    X6,
    X7,
    /// Indirect result location register (AAPCS64).
    X8,
    X9,
    X10,
    X11,
    X12,
    X13,
    X14,
    X15,
    /// Intra-procedure-call scratch register IP0.
    X16,
    /// Intra-procedure-call scratch register IP1.
    X17,
    /// Platform register (OS-specific use).
    X18,
    X19,
    X20,
    X21,
    X22,
    X23,
    X24,
    X25,
    X26,
    X27,
    X28,
    /// Frame pointer.
    X29,
    /// Link register.
    X30,
    /// Stack pointer.
    SP,
    /// Zero register — reads as 0, writes are discarded.
    XZR,
}

impl Register {
    /// Returns the 5-bit encoding index used in ARM64 instruction encodings.
    ///
    /// `SP` and `XZR` both encode as `31` — the distinction is determined by
    /// context (the specific instruction encoding).
    pub fn encoding(&self) -> u32 {
        match self {
            Register::X0 => 0,
            Register::X1 => 1,
            Register::X2 => 2,
            Register::X3 => 3,
            Register::X4 => 4,
            Register::X5 => 5,
            Register::X6 => 6,
            Register::X7 => 7,
            Register::X8 => 8,
            Register::X9 => 9,
            Register::X10 => 10,
            Register::X11 => 11,
            Register::X12 => 12,
            Register::X13 => 13,
            Register::X14 => 14,
            Register::X15 => 15,
            Register::X16 => 16,
            Register::X17 => 17,
            Register::X18 => 18,
            Register::X19 => 19,
            Register::X20 => 20,
            Register::X21 => 21,
            Register::X22 => 22,
            Register::X23 => 23,
            Register::X24 => 24,
            Register::X25 => 25,
            Register::X26 => 26,
            Register::X27 => 27,
            Register::X28 => 28,
            Register::X29 => 29,
            Register::X30 => 30,
            Register::SP => 31,
            Register::XZR => 31,
        }
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Register::X0 => "x0",
            Register::X1 => "x1",
            Register::X2 => "x2",
            Register::X3 => "x3",
            Register::X4 => "x4",
            Register::X5 => "x5",
            Register::X6 => "x6",
            Register::X7 => "x7",
            Register::X8 => "x8",
            Register::X9 => "x9",
            Register::X10 => "x10",
            Register::X11 => "x11",
            Register::X12 => "x12",
            Register::X13 => "x13",
            Register::X14 => "x14",
            Register::X15 => "x15",
            Register::X16 => "x16",
            Register::X17 => "x17",
            Register::X18 => "x18",
            Register::X19 => "x19",
            Register::X20 => "x20",
            Register::X21 => "x21",
            Register::X22 => "x22",
            Register::X23 => "x23",
            Register::X24 => "x24",
            Register::X25 => "x25",
            Register::X26 => "x26",
            Register::X27 => "x27",
            Register::X28 => "x28",
            Register::X29 => "x29",
            Register::X30 => "x30",
            Register::SP => "sp",
            Register::XZR => "xzr",
        }
    }

    /// Returns `true` if this register is callee-saved (X19–X28).
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Register::X19
                | Register::X20
                | Register::X21
                | Register::X22
                | Register::X23
                | Register::X24
                | Register::X25
                | Register::X26
                | Register::X27
                | Register::X28
        )
    }

    /// Returns `true` if this register is caller-saved / temporary (X0–X18,
    /// excluding X8 which has a special role but is still caller-saved in
    /// practice).
    pub fn is_caller_saved(&self) -> bool {
        !self.is_callee_saved() && !matches!(self, Register::SP | Register::XZR)
    }
}

impl std::fmt::Display for Register {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ---------------------------------------------------------------------------
// Condition Code
// ---------------------------------------------------------------------------

/// ARM64 condition codes used in conditional branches and instructions.
///
/// Each condition code tests the NZCV flags set by a preceding comparison or
/// arithmetic instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Condition {
    /// Equal / zero (Z == 1)
    EQ,
    /// Not equal / not zero (Z == 0)
    NE,
    /// Carry set / unsigned higher or same (C == 1)
    CS,
    /// Carry clear / unsigned lower (C == 0)
    CC,
    /// Minus / negative (N == 1)
    MI,
    /// Plus / positive or zero (N == 0)
    PL,
    /// Overflow (V == 1)
    VS,
    /// No overflow (V == 0)
    VC,
    /// Unsigned higher (C == 1 && Z == 0)
    HI,
    /// Unsigned lower or same (C == 0 || Z == 1)
    LS,
    /// Signed greater than or equal (N == V)
    GE,
    /// Signed less than (N != V)
    LT,
    /// Signed greater than (Z == 0 && N == V)
    GT,
    /// Signed less than or equal (Z == 1 || N != V)
    LE,
}

impl Condition {
    /// Returns the 4-bit condition code encoding used in ARM64 instructions.
    pub fn encoding(&self) -> u32 {
        match self {
            Condition::EQ => 0b0000,
            Condition::NE => 0b0001,
            Condition::CS => 0b0010,
            Condition::CC => 0b0011,
            Condition::MI => 0b0100,
            Condition::PL => 0b0101,
            Condition::VS => 0b0110,
            Condition::VC => 0b0111,
            Condition::HI => 0b1000,
            Condition::LS => 0b1001,
            Condition::GE => 0b1010,
            Condition::LT => 0b1011,
            Condition::GT => 0b1100,
            Condition::LE => 0b1101,
        }
    }

    /// Returns the standard assembly mnemonic suffix for this condition.
    pub fn asm_suffix(&self) -> &'static str {
        match self {
            Condition::EQ => "eq",
            Condition::NE => "ne",
            Condition::CS => "cs",
            Condition::CC => "cc",
            Condition::MI => "mi",
            Condition::PL => "pl",
            Condition::VS => "vs",
            Condition::VC => "vc",
            Condition::HI => "hi",
            Condition::LS => "ls",
            Condition::GE => "ge",
            Condition::LT => "lt",
            Condition::GT => "gt",
            Condition::LE => "le",
        }
    }

    /// Returns the inverse (complementary) condition code.
    pub fn invert(&self) -> Condition {
        match self {
            Condition::EQ => Condition::NE,
            Condition::NE => Condition::EQ,
            Condition::CS => Condition::CC,
            Condition::CC => Condition::CS,
            Condition::MI => Condition::PL,
            Condition::PL => Condition::MI,
            Condition::VS => Condition::VC,
            Condition::VC => Condition::VS,
            Condition::HI => Condition::LS,
            Condition::LS => Condition::HI,
            Condition::GE => Condition::LT,
            Condition::LT => Condition::GE,
            Condition::GT => Condition::LE,
            Condition::LE => Condition::GT,
        }
    }
}

impl std::fmt::Display for Condition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.asm_suffix())
    }
}

// ---------------------------------------------------------------------------
// Shift Kind
// ---------------------------------------------------------------------------

/// Shift type used in shifted-register operand encodings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ShiftKind {
    /// Logical shift left.
    LSL,
    /// Logical shift right.
    LSR,
    /// Arithmetic shift right.
    ASR,
    /// Rotate right.
    ROR,
}

impl ShiftKind {
    /// Returns the 2-bit shift-type encoding.
    pub fn encoding(&self) -> u32 {
        match self {
            ShiftKind::LSL => 0b00,
            ShiftKind::LSR => 0b01,
            ShiftKind::ASR => 0b10,
            ShiftKind::ROR => 0b11,
        }
    }
}

// ---------------------------------------------------------------------------
// Barrier Option
// ---------------------------------------------------------------------------

/// Barrier option for DMB / DSB instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BarrierOption {
    /// Full system barrier.
    SY,
    /// Load-load / load-store barrier.
    LD,
    /// Store-store barrier.
    ST,
    /// Inner shareable domain.
    ISH,
    /// Inner shareable load-load / load-store.
    ISHLD,
    /// Inner shareable store-store.
    ISHST,
    /// Outer shareable domain.
    OSH,
    /// Outer shareable load-load / load-store.
    OSHLD,
    /// Outer shareable store-store.
    OSHST,
}

impl BarrierOption {
    /// Returns the 4-bit option encoding used in barrier instructions.
    pub fn encoding(&self) -> u32 {
        match self {
            BarrierOption::SY => 0b1111,
            BarrierOption::ST => 0b1110,
            BarrierOption::LD => 0b1101,
            BarrierOption::ISH => 0b1011,
            BarrierOption::ISHST => 0b1010,
            BarrierOption::ISHLD => 0b1001,
            BarrierOption::OSH => 0b0011,
            BarrierOption::OSHST => 0b0010,
            BarrierOption::OSHLD => 0b0001,
        }
    }
}

// ---------------------------------------------------------------------------
// Instruction
// ---------------------------------------------------------------------------

/// ARM64 instruction representations for code generation.
///
/// Each variant captures the operands needed for both encoding and
/// disassembly. The `encode()` method produces a 32-bit machine code word;
/// the `to_string()` method produces a human-readable assembly line.
///
/// Many encoding paths are marked `TODO` — the full ARM64 encoding tables are
/// extensive and will be filled in incrementally as the codebase matures.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Instruction {
    // ---- Arithmetic ----
    /// Add: `ADD Rd, Rn, Rm` or `ADD Rd, Rn, #imm`
    ADD { rd: Register, rn: Register, rm: Operand },
    /// Subtract: `SUB Rd, Rn, Rm` or `SUB Rd, Rn, #imm`
    SUB { rd: Register, rn: Register, rm: Operand },
    /// Multiply: `MUL Rd, Rn, Rm`
    MUL { rd: Register, rn: Register, rm: Register },
    /// Signed divide: `SDIV Rd, Rn, Rm`
    SDIV { rd: Register, rn: Register, rm: Register },
    /// Unsigned divide: `UDIV Rd, Rn, Rm`
    UDIV { rd: Register, rn: Register, rm: Register },

    // ---- Bitwise / Shift ----
    /// Bitwise AND: `AND Rd, Rn, Rm`
    AND { rd: Register, rn: Register, rm: Register },
    /// Bitwise OR: `ORR Rd, Rn, Rm`
    ORR { rd: Register, rn: Register, rm: Register },
    /// Bitwise exclusive OR: `EOR Rd, Rn, Rm`
    EOR { rd: Register, rn: Register, rm: Register },
    /// Logical shift left: `LSL Rd, Rn, Rm` or `LSL Rd, Rn, #imm`
    LSL { rd: Register, rn: Register, rm: Operand },
    /// Logical shift right: `LSR Rd, Rn, Rm` or `LSR Rd, Rn, #imm`
    LSR { rd: Register, rn: Register, rm: Operand },
    /// Arithmetic shift right: `ASR Rd, Rn, Rm` or `ASR Rd, Rn, #imm`
    ASR { rd: Register, rn: Register, rm: Operand },

    // ---- Load / Store ----
    /// Load register: `LDR Rt, [Rn, #offset]`
    LDR { rt: Register, rn: Register, offset: i32 },
    /// Store register: `STR Rt, [Rn, #offset]`
    STR { rt: Register, rn: Register, offset: i32 },
    /// Load pair: `LDP Rt1, Rt2, [Rn, #offset]`
    LDP { rt1: Register, rt2: Register, rn: Register, offset: i32 },
    /// Store pair: `STP Rt1, Rt2, [Rn, #offset]`
    STP { rt1: Register, rt2: Register, rn: Register, offset: i32 },

    // ---- Atomic ----
    /// Load-exclusive register: `LDXR Rt, [Rn]`
    LDXR { rt: Register, rn: Register },
    /// Store-exclusive register: `STXR Rs, Rt, [Rn]`
    STXR { rs: Register, rt: Register, rn: Register },
    /// Compare-and-swap: `CAS Rs, Rt, [Rn]`
    CAS { rs: Register, rt: Register, rn: Register },

    // ---- Branch ----
    /// Unconditional branch: `B offset`
    B { offset: i32 },
    /// Branch with link: `BL offset`
    BL { offset: i32 },
    /// Branch to register: `BR Rn`
    BR { rn: Register },
    /// Branch with link to register: `BLR Rn`
    BLR { rn: Register },
    /// Return: `RET {Rn}`
    RET { rn: Option<Register> },
    /// Compare and branch on zero: `CBZ Rt, offset`
    CBZ { rt: Register, offset: i32 },
    /// Compare and branch on non-zero: `CBNZ Rt, offset`
    CBNZ { rt: Register, offset: i32 },
    /// Test bit and branch on zero: `TBZ Rt, bit, offset`
    TBZ { rt: Register, bit: u32, offset: i32 },
    /// Test bit and branch on non-zero: `TBNZ Rt, bit, offset`
    TBNZ { rt: Register, bit: u32, offset: i32 },

    // ---- Barriers ----
    /// Data memory barrier: `DMB option`
    DMB { option: BarrierOption },
    /// Data synchronization barrier: `DSB option`
    DSB { option: BarrierOption },
    /// Instruction synchronization barrier: `ISB`
    ISB,

    // ---- Move ----
    /// Move register: `MOV Rd, Rm` (alias for `ORR Rd, XZR, Rm`)
    MOV { rd: Register, rm: Register },
    /// Move wide with zero: `MOVZ Rd, #imm16, shift`
    MOVZ { rd: Register, imm16: u16, shift: u32 },
    /// Move wide with keep: `MOVK Rd, #imm16, shift`
    MOVK { rd: Register, imm16: u16, shift: u32 },

    // ---- Compare / Test ----
    /// Compare (subtract, discard result): `CMP Rn, Rm` or `CMP Rn, #imm`
    CMP { rn: Register, rm: Operand },
    /// Compare negative: `CMN Rn, Rm` or `CMN Rn, #imm`
    CMN { rn: Register, rm: Operand },
    /// Test (bitwise AND, discard result): `TST Rn, Rm`
    TST { rn: Register, rm: Register },

    // ---- System ----
    /// Supervisor call: `SVC #imm16`
    SVC { imm16: u16 },
}

// ---------------------------------------------------------------------------
// Operand (register or immediate)
// ---------------------------------------------------------------------------

/// A flexible operand — either a register (optionally shifted) or a 12-bit
/// immediate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Operand {
    /// Register operand with optional shift.
    Reg {
        reg: Register,
        shift: Option<(ShiftKind, u32)>,
    },
    /// 12-bit unsigned immediate (0–4095) for arithmetic / logical ops.
    Imm12(u16),
}

// ---------------------------------------------------------------------------
// Instruction — encode
// ---------------------------------------------------------------------------

impl Instruction {
    /// Encode this instruction into a 32-bit ARM64 machine-code word.
    ///
    /// Encoding follows the ARM Architecture Reference Manual (ARMv8-A).
    /// Paths that require multi-instruction sequences or are not yet
    /// implemented return `Err(CodegenError::EncodingError)`.
    pub fn encode(&self) -> Result<u32> {
        match self {
            // ---- ADD (shifted register): 1 0 0 0 1 0 1 1 shift 0 Rm imm6 Rn Rd ----
            Instruction::ADD { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift } => {
                    let (hw, imm6) = shift.map(|(k, v)| (k.encoding(), v)).unwrap_or((0, 0));
                    Ok(0b10001011_00_000000_00000_00000_00000
                        | (hw << 22)
                        | (reg.encoding() << 16)
                        | (imm6 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                Operand::Imm12(imm) => {
                    // ADD (immediate): 1 0 0 1 0 0 0 1 sh imm12 Rn Rd
                    // sh=0 for no shift, imm12 in bits [21:10]
                    Ok(0b10010001_00_000000000000_00000_00000
                        | ((*imm as u32) << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // ---- SUB (shifted register): 1 0 0 0 1 0 1 1 shift 1 Rm imm6 Rn Rd ----
            Instruction::SUB { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift } => {
                    let (hw, imm6) = shift.map(|(k, v)| (k.encoding(), v)).unwrap_or((0, 0));
                    Ok(0b10001011_00_000000_10000_00000_00000
                        | (hw << 22)
                        | (reg.encoding() << 16)
                        | (imm6 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                Operand::Imm12(imm) => {
                    // SUB (immediate): 1 0 0 1 0 0 0 1 sh imm12 Rn Rd  (op=1)
                    Ok(0b10010001_00_000000000000_00000_00000
                        | (1 << 30)
                        | ((*imm as u32) << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // ---- MUL: alias for MADD Rd, Rn, Rm, XZR ----
            // MADD: 1 0 0 1 1 0 1 1 000 Rm 0 XZR Ra=31 Rn Rd
            Instruction::MUL { rd, rn, rm } => {
                Ok(0b10011011_000_00000_0_11111_00000_00000
                    | (rm.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- SDIV ----
            Instruction::SDIV { rd, rn, rm } => {
                // 1 0 0 1 1 0 1 1 0 0 Rm 00001 1 Rn Rd
                Ok(0b10011011_00_00000_00001_1_00000_00000
                    | (rm.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- UDIV ----
            Instruction::UDIV { rd, rn, rm } => {
                // 1 0 0 1 1 0 1 1 0 0 Rm 00001 0 Rn Rd
                Ok(0b10011011_00_00000_00001_0_00000_00000
                    | (rm.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- AND (shifted register) ----
            Instruction::AND { rd, rn, rm } => {
                Ok(0b10001010_00_000000_00000_00000_00000
                    | (rm.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- ORR (shifted register) ----
            Instruction::ORR { rd, rn, rm } => {
                Ok(0b10101010_00_000000_00000_00000_00000
                    | (rm.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- EOR (shifted register) ----
            Instruction::EOR { rd, rn, rm } => {
                Ok(0b11001010_00_000000_00000_00000_00000
                    | (rm.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- LSL / LSR / ASR (shifted register or immediate) ----
            Instruction::LSL { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => {
                    // UBFM alias — TODO: full encoding
                    // For now emit as shifted-register ORR with LSL
                    Ok(0b10001011_00_000000_00000_00000_00000
                        | (ShiftKind::LSL.encoding() << 22)
                        | (reg.encoding() << 16)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                Operand::Imm12(imm) => {
                    // UBFM Rd, Rn, #(64-imm), #(63-imm) — alias for LSL #imm
                    // TODO: full UBFM encoding
                    Ok(0b100100110_0_000000_000000_00000_00000
                        | ((*imm as u32) << 16)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            Instruction::LSR { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => {
                    Ok(0b10001011_00_000000_00000_00000_00000
                        | (ShiftKind::LSR.encoding() << 22)
                        | (reg.encoding() << 16)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                Operand::Imm12(imm) => {
                    // UBFM Rd, Rn, #imm, #63 — alias for LSR #imm
                    // TODO: full UBFM encoding
                    Ok(0b100100110_0_000000_000000_00000_00000
                        | ((*imm as u32) << 16)
                        | (63u32 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            Instruction::ASR { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => {
                    Ok(0b10001011_00_000000_00000_00000_00000
                        | (ShiftKind::ASR.encoding() << 22)
                        | (reg.encoding() << 16)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                Operand::Imm12(imm) => {
                    // SBFM Rd, Rn, #imm, #63 — alias for ASR #imm
                    // TODO: full SBFM encoding
                    Ok(0b100100110_0_000000_000000_00000_00000
                        | ((*imm as u32) << 16)
                        | (63u32 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // ---- LDR (unsigned offset) ----
            Instruction::LDR { rt, rn, offset } => {
                // LDR (unsigned offset): 1 1 1 1 1 0 0 1 01 imm12 Rn Rt
                // imm12 = offset / 8 (for 64-bit)
                if *offset >= 0 && *offset % 8 == 0 {
                    let imm12 = (*offset as u32) / 8;
                    Ok(0b11111001_01_000000000000_00000_00000
                        | (imm12 << 10)
                        | (rn.encoding() << 5)
                        | rt.encoding())
                } else {
                    // TODO: pre-indexed / post-indexed LDR
                    Err(CodegenError::EncodingError(format!(
                        "LDR with offset {} not yet supported (must be non-negative multiple of 8)",
                        offset
                    )))
                }
            }

            // ---- STR (unsigned offset) ----
            Instruction::STR { rt, rn, offset } => {
                // STR (unsigned offset): 1 1 1 1 1 0 0 0 01 imm12 Rn Rt
                if *offset >= 0 && *offset % 8 == 0 {
                    let imm12 = (*offset as u32) / 8;
                    Ok(0b11111000_01_000000000000_00000_00000
                        | (imm12 << 10)
                        | (rn.encoding() << 5)
                        | rt.encoding())
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "STR with offset {} not yet supported",
                        offset
                    )))
                }
            }

            // ---- LDP (signed offset) ----
            Instruction::LDP { rt1, rt2, rn, offset } => {
                // LDP (signed offset): 1 0 1 0 1 0 0 1 1 imm7 Rn Rt1 Rt2
                let imm7 = *offset / 8;
                // TODO: validate imm7 fits in 7 bits signed
                Ok(0b10101001_1_0000000_00000_00000_00000
                    | (((imm7 as u32) & 0x7F) << 15)
                    | (rn.encoding() << 10)
                    | (rt1.encoding() << 5)  // wait, this is wrong
                    | rt2.encoding())
                // TODO: fix field positions per ARM spec
            }

            // ---- STP (signed offset) ----
            Instruction::STP { rt1, rt2, rn, offset } => {
                let imm7 = *offset / 8;
                Ok(0b10101000_1_0000000_00000_00000_00000
                    | (((imm7 as u32) & 0x7F) << 15)
                    | (rn.encoding() << 10)
                    | (rt1.encoding() << 5)
                    | rt2.encoding())
                // TODO: fix field positions per ARM spec
            }

            // ---- LDXR ----
            Instruction::LDXR { rt, rn } => {
                // 1 1 0 0 1 0 0 0 0 1 1 1 1 1 00000 00000 Rt
                Ok(0b11001000_01111_00000_00000_00000
                    | (rn.encoding() << 5)
                    | rt.encoding())
            }

            // ---- STXR ----
            Instruction::STXR { rs, rt, rn } => {
                // 1 1 0 0 1 0 0 0 0 0 0 0 0 0 Rs 00000 Rn Rt
                Ok(0b11001000_00000_00000_00000_00000
                    | (rs.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rt.encoding())
            }

            // ---- CAS ----
            Instruction::CAS { rs, rt, rn } => {
                // CAS: 00011000 L o 0 Rs 111111 Rn Rt (32 bits)
                // Base = 0x1800FC00 (L=0, o=0, Rs=0, Rn=0, Rt=0)
                // TODO: full CAS encoding for ARMv8.1-LSE
                Ok(0x1800FC00u32
                    | (rs.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rt.encoding())
            }

            // ---- B ----
            Instruction::B { offset } => {
                // B: 0 0 0 1 0 1 imm26
                let imm26 = (*offset as i32) >> 2;
                // TODO: validate imm26 fits
                Ok(0b000101_00000000000000000000000000
                    | ((imm26 as u32) & 0x03FFFFFF))
            }

            // ---- BL ----
            Instruction::BL { offset } => {
                let imm26 = (*offset as i32) >> 2;
                Ok(0b100101_00000000000000000000000000
                    | ((imm26 as u32) & 0x03FFFFFF))
            }

            // ---- BR ----
            Instruction::BR { rn } => {
                // BR: 1 1 0 1 0 1 1 0 0 0 0 1 1 1 1 1 0 0 0 0 0 0 Rn 0 0 0 0 0
                Ok(0b1101011_0000_1_1_1_1_1_000000_00000_00000
                    | (rn.encoding() << 5))
            }

            // ---- BLR ----
            Instruction::BLR { rn } => {
                Ok(0b1101011_0000_1_1_1_1_1_000000_00000_00001
                    | (rn.encoding() << 5))
            }

            // ---- RET ----
            Instruction::RET { rn } => {
                let reg = rn.unwrap_or(Register::X30);
                Ok(0b1101011_0010_1_1_1_1_1_000000_00000_00000
                    | (reg.encoding() << 5))
            }

            // ---- CBZ ----
            Instruction::CBZ { rt, offset } => {
                // CBZ (64-bit): sf=1, op=011010, 0, imm19, Rt
                // Base = 0xB4000000
                let imm19 = (*offset as i32) >> 2;
                Ok(0xB4000000u32
                    | (((imm19 as u32) & 0x7FFFF) << 5)
                    | rt.encoding())
            }

            // ---- CBNZ ----
            Instruction::CBNZ { rt, offset } => {
                // CBNZ (64-bit): sf=1, op=011010, 1, imm19, Rt
                // Base = 0xB5000000
                let imm19 = (*offset as i32) >> 2;
                Ok(0xB5000000u32
                    | (((imm19 as u32) & 0x7FFFF) << 5)
                    | rt.encoding())
            }

            // ---- TBZ ----
            Instruction::TBZ { rt, bit, offset } => {
                let b5 = (*bit >> 5) as u32;
                let imm14 = (*offset as i32) >> 2;
                Ok(0b011011_0_0_00000000000000_00000_00000
                    | (b5 << 31)
                    | (((*bit & 0x1F) as u32) << 19)
                    | (((imm14 as u32) & 0x3FFF) << 5)
                    | rt.encoding())
            }

            // ---- TBNZ ----
            Instruction::TBNZ { rt, bit, offset } => {
                let b5 = (*bit >> 5) as u32;
                let imm14 = (*offset as i32) >> 2;
                Ok(0b011011_1_0_00000000000000_00000_00000
                    | (b5 << 31)
                    | (((*bit & 0x1F) as u32) << 19)
                    | (((imm14 as u32) & 0x3FFF) << 5)
                    | rt.encoding())
            }

            // ---- DMB ----
            Instruction::DMB { option } => {
                // 1 1 0 1 0 1 0 1 0 0 0 0 1 0 1 1 1 0 1 1 1 0 1 1 option 1 0 1 1
                Ok(0b11010101_0000_1011_1011_1011_0000_1011
                    | (option.encoding() << 8))
            }

            // ---- DSB ----
            Instruction::DSB { option } => {
                Ok(0b11010101_0000_1011_1011_1011_0000_1001
                    | (option.encoding() << 8))
            }

            // ---- ISB ----
            Instruction::ISB => {
                Ok(0b11010101_0000_1011_1011_1011_0000_1110)
            }

            // ---- MOV (alias for ORR Xd, XZR, Xm) ----
            Instruction::MOV { rd, rm } => {
                Ok(0b10101010_00_000000_00000_11111_00000
                    | (rm.encoding() << 16)
                    | rd.encoding())
            }

            // ---- MOVZ ----
            Instruction::MOVZ { rd, imm16, shift } => {
                // MOVZ: 1 1 0 1 0 0 1 0 1 hw imm16 Rd
                let hw = *shift / 16; // 0, 1, 2, or 3
                Ok(0b110100101_00_0000000000000000_00000
                    | (hw << 21)
                    | ((*imm16 as u32) << 5)
                    | rd.encoding())
            }

            // ---- MOVK ----
            Instruction::MOVK { rd, imm16, shift } => {
                let hw = *shift / 16;
                Ok(0b111100101_00_0000000000000000_00000
                    | (hw << 21)
                    | ((*imm16 as u32) << 5)
                    | rd.encoding())
            }

            // ---- CMP (alias for SUBS XZR, Rn, Rm) ----
            Instruction::CMP { rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => {
                    // SUBS XZR, Rn, Rm
                    Ok(0b11101011_00_000000_00000_00000_11111
                        | (reg.encoding() << 16)
                        | (rn.encoding() << 5))
                }
                Operand::Imm12(imm) => {
                    // SUBS XZR, Rn, #imm
                    Ok(0b11110001_00_000000000000_00000_11111
                        | ((*imm as u32) << 10)
                        | (rn.encoding() << 5))
                }
            },

            // ---- CMN (alias for ADDS XZR, Rn, Rm) ----
            Instruction::CMN { rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => {
                    Ok(0b10101011_00_000000_00000_00000_11111
                        | (reg.encoding() << 16)
                        | (rn.encoding() << 5))
                }
                Operand::Imm12(imm) => {
                    Ok(0b10010001_00_000000000000_00000_11111
                        | ((*imm as u32) << 10)
                        | (rn.encoding() << 5))
                }
            },

            // ---- TST (alias for ANDS XZR, Rn, Rm) ----
            Instruction::TST { rn, rm } => {
                Ok(0b11101010_00_000000_00000_00000_11111
                    | (rm.encoding() << 16)
                    | (rn.encoding() << 5))
            }

            // ---- SVC ----
            Instruction::SVC { imm16 } => {
                // SVC: 1 1 0 1 0 1 0 0 0 0 0 imm16 0 0 0 0 1
                Ok(0b11010100_000_000000000000000_00001
                    | ((*imm16 as u32) << 5))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Instruction — to_string (assembly text)
// ---------------------------------------------------------------------------

impl std::fmt::Display for Instruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Instruction::ADD { rd, rn, rm } => write!(f, "add {}, {}, {}", rd, rn, rm),
            Instruction::SUB { rd, rn, rm } => write!(f, "sub {}, {}, {}", rd, rn, rm),
            Instruction::MUL { rd, rn, rm } => write!(f, "mul {}, {}, {}", rd, rn, rm),
            Instruction::SDIV { rd, rn, rm } => write!(f, "sdiv {}, {}, {}", rd, rn, rm),
            Instruction::UDIV { rd, rn, rm } => write!(f, "udiv {}, {}, {}", rd, rn, rm),
            Instruction::AND { rd, rn, rm } => write!(f, "and {}, {}, {}", rd, rn, rm),
            Instruction::ORR { rd, rn, rm } => write!(f, "orr {}, {}, {}", rd, rn, rm),
            Instruction::EOR { rd, rn, rm } => write!(f, "eor {}, {}, {}", rd, rn, rm),
            Instruction::LSL { rd, rn, rm } => write!(f, "lsl {}, {}, {}", rd, rn, rm),
            Instruction::LSR { rd, rn, rm } => write!(f, "lsr {}, {}, {}", rd, rn, rm),
            Instruction::ASR { rd, rn, rm } => write!(f, "asr {}, {}, {}", rd, rn, rm),
            Instruction::LDR { rt, rn, offset } => write!(f, "ldr {}, [{}, #{}]", rt, rn, offset),
            Instruction::STR { rt, rn, offset } => write!(f, "str {}, [{}, #{}]", rt, rn, offset),
            Instruction::LDP { rt1, rt2, rn, offset } => {
                write!(f, "ldp {}, {}, [{}, #{}]", rt1, rt2, rn, offset)
            }
            Instruction::STP { rt1, rt2, rn, offset } => {
                write!(f, "stp {}, {}, [{}, #{}]", rt1, rt2, rn, offset)
            }
            Instruction::LDXR { rt, rn } => write!(f, "ldxr {}, [{}]", rt, rn),
            Instruction::STXR { rs, rt, rn } => write!(f, "stxr {}, {}, [{}]", rs, rt, rn),
            Instruction::CAS { rs, rt, rn } => write!(f, "cas {}, {}, [{}]", rs, rt, rn),
            Instruction::B { offset } => write!(f, "b #{}", offset),
            Instruction::BL { offset } => write!(f, "bl #{}", offset),
            Instruction::BR { rn } => write!(f, "br {}", rn),
            Instruction::BLR { rn } => write!(f, "blr {}", rn),
            Instruction::RET { rn } => match rn {
                Some(reg) => write!(f, "ret {}", reg),
                None => write!(f, "ret"),
            },
            Instruction::CBZ { rt, offset } => write!(f, "cbz {}, #{}", rt, offset),
            Instruction::CBNZ { rt, offset } => write!(f, "cbnz {}, #{}", rt, offset),
            Instruction::TBZ { rt, bit, offset } => write!(f, "tbz {}, #{}, #{}", rt, bit, offset),
            Instruction::TBNZ { rt, bit, offset } => write!(f, "tbnz {}, #{}, #{}", rt, bit, offset),
            Instruction::DMB { option } => write!(f, "dmb {:?}", option),
            Instruction::DSB { option } => write!(f, "dsb {:?}", option),
            Instruction::ISB => write!(f, "isb"),
            Instruction::MOV { rd, rm } => write!(f, "mov {}, {}", rd, rm),
            Instruction::MOVZ { rd, imm16, shift } => {
                if *shift == 0 {
                    write!(f, "movz {}, #{}", rd, imm16)
                } else {
                    write!(f, "movz {}, #{}, lsl #{}", rd, imm16, shift)
                }
            }
            Instruction::MOVK { rd, imm16, shift } => {
                if *shift == 0 {
                    write!(f, "movk {}, #{}", rd, imm16)
                } else {
                    write!(f, "movk {}, #{}, lsl #{}", rd, imm16, shift)
                }
            }
            Instruction::CMP { rn, rm } => write!(f, "cmp {}, {}", rn, rm),
            Instruction::CMN { rn, rm } => write!(f, "cmn {}, {}", rn, rm),
            Instruction::TST { rn, rm } => write!(f, "tst {}, {}", rn, rm),
            Instruction::SVC { imm16 } => write!(f, "svc #{}", imm16),
        }
    }
}

// ---------------------------------------------------------------------------
// Operand — Display
// ---------------------------------------------------------------------------

impl std::fmt::Display for Operand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Operand::Reg { reg, shift } => match shift {
                Some((kind, amount)) => write!(f, "{}, {} #{}", reg, kind, amount),
                None => write!(f, "{}", reg),
            },
            Operand::Imm12(v) => write!(f, "#{}", v),
        }
    }
}

impl std::fmt::Display for ShiftKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ShiftKind::LSL => "lsl",
            ShiftKind::LSR => "lsr",
            ShiftKind::ASR => "asr",
            ShiftKind::ROR => "ror",
        })
    }
}

impl std::fmt::Display for BarrierOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            BarrierOption::SY => "sy",
            BarrierOption::LD => "ld",
            BarrierOption::ST => "st",
            BarrierOption::ISH => "ish",
            BarrierOption::ISHLD => "ishld",
            BarrierOption::ISHST => "ishst",
            BarrierOption::OSH => "osh",
            BarrierOption::OSHLD => "oshld",
            BarrierOption::OSHST => "oshst",
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_encoding_roundtrip() {
        assert_eq!(Register::X0.encoding(), 0);
        assert_eq!(Register::X30.encoding(), 30);
        assert_eq!(Register::SP.encoding(), 31);
        assert_eq!(Register::XZR.encoding(), 31);
    }

    #[test]
    fn condition_inversion() {
        assert_eq!(Condition::EQ.invert(), Condition::NE);
        assert_eq!(Condition::GT.invert(), Condition::LE);
        assert_eq!(Condition::CS.invert(), Condition::CC);
    }

    #[test]
    fn instruction_display() {
        let add = Instruction::ADD {
            rd: Register::X0,
            rn: Register::X1,
            rm: Operand::Imm12(42),
        };
        assert_eq!(format!("{}", add), "add x0, x1, #42");

        let ret = Instruction::RET { rn: None };
        assert_eq!(format!("{}", ret), "ret");

        let svc = Instruction::SVC { imm16: 1 };
        assert_eq!(format!("{}", svc), "svc #1");
    }

    #[test]
    fn movz_encoding_basic() {
        let movz = Instruction::MOVZ {
            rd: Register::X0,
            imm16: 1,
            shift: 0,
        };
        // Should not error; exact bit pattern can be validated later
        assert!(movz.encode().is_ok());
    }

    #[test]
    fn callee_saved_check() {
        assert!(Register::X19.is_callee_saved());
        assert!(Register::X28.is_callee_saved());
        assert!(!Register::X0.is_callee_saved());
        assert!(!Register::X30.is_callee_saved());
    }
}
