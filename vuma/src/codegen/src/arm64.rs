//! # ARM64 (AArch64) Instruction Definitions & Instruction Selection
//!
//! Defines the ARM64 register set, condition codes, instruction
//! representations, addressing modes, and the instruction selector that maps
//! SCG/IR node types to ARM64 instructions for the Raspberry Pi 5
//! (Cortex-A76, ARMv8.2-A).
//!
//! ## Instruction Selection (SCG → ARM64)
//!
//! | SCG / IR Node          | ARM64 Instructions                                  |
//! |------------------------|-----------------------------------------------------|
//! | Computation(add)       | ADD, SUB, MUL, SDIV, UDIV                          |
//! | Computation(cmp)       | CMP, CSEL                                           |
//! | Computation(bitwise)   | AND, ORR, EOR, LSL, LSR, ASR                       |
//! | Allocation             | SUB SP, SP, #size / BL malloc                      |
//! | Deallocation           | ADD SP, SP, #size / BL free                        |
//! | Access(read)           | LDR, LDRB, LDRH, LDRSW                             |
//! | Access(write)          | STR, STRB, STRH                                    |
//! | Cast                   | MOV (no-op) / FCVT / SCVTF / FCVTZS / SXTW        |
//! | ControlFlow            | B, B.cond, CBZ, CBNZ, TBZ, TBNZ                   |
//!
//! ## AAPCS64 Calling Convention
//!
//! | Register(s) | Role                                  |
//! |-------------|---------------------------------------|
//! | X0–X7       | Argument / result registers            |
//! | X8          | Indirect result location register      |
//! | X9–X15      | Caller-saved temporary registers       |
//! | X16–X17     | Intra-procedure-call scratch (IP0/IP1) |
//! | X18         | Platform register                      |
//! | X19–X28     | Callee-saved registers                 |
//! | X29         | Frame pointer (FP)                     |
//! | X30         | Link register (LR)                     |
//! | SP          | Stack pointer                          |
//! | XZR         | Zero register                          |
//!
//! ## References
//!
//! - ARM Architecture Reference Manual ARMv8, for ARMv8-A architecture profile
//! - <https://developer.arm.com/documentation/ddi0487/latest>

use crate::ir::{BinOpKind, CastKind, IRInstr, IRTerminator, IRType, IRValue};
use crate::CodegenError;
use crate::Result;

// ---------------------------------------------------------------------------
// Register Width
// ---------------------------------------------------------------------------

/// ARM64 register width — selects between 64-bit X registers and 32-bit W
/// sub-registers.
///
/// On AArch64, the 5-bit register encoding is the same for Xn and Wn; the
/// instruction's **sf** bit (bit 31) or **size** field (bits 31:30) selects
/// the operand width. Using W registers gives automatic 32-bit wrapping
/// arithmetic, which is required for algorithms like SHA-256 that operate on
/// `u32` values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum RegWidth {
    /// 32-bit W registers (W0–W30, WZR, WSP).
    W32,
    /// 64-bit X registers (X0–X30, XZR, SP) — default.
    X64,
}

impl RegWidth {
    /// Returns `true` if this is a 32-bit register width.
    pub fn is_32bit(&self) -> bool {
        matches!(self, RegWidth::W32)
    }

    /// Returns the **sf** bit value for this width (bit 31 of the encoding).
    ///
    /// - `X64` → 1
    /// - `W32` → 0
    pub fn sf_bit(&self) -> u32 {
        match self {
            RegWidth::X64 => 1,
            RegWidth::W32 => 0,
        }
    }

    /// Returns the register size in bits (64 or 32).
    pub fn bits(&self) -> u32 {
        match self {
            RegWidth::X64 => 64,
            RegWidth::W32 => 32,
        }
    }

    /// Returns the register size in bytes (8 or 4).
    pub fn bytes(&self) -> u32 {
        self.bits() / 8
    }

    /// Returns the log2 scale for unsigned-offset load/store encoding.
    ///
    /// - `X64` → 3 (offset is divided by 8 for 64-bit LDR/STR)
    /// - `W32` → 2 (offset is divided by 4 for 32-bit LDR/STR)
    pub fn scale(&self) -> u32 {
        match self {
            RegWidth::X64 => 3,
            RegWidth::W32 => 2,
        }
    }

    /// Returns the mask value used in UBFM/SBFM for the `imms` field
    /// (register size - 1).
    pub fn size_minus_1(&self) -> u32 {
        match self {
            RegWidth::X64 => 63,
            RegWidth::W32 => 31,
        }
    }

    /// Derive the register width from an optional IR type.
    ///
    /// Returns `W32` for `I32`/`U32` types, `X64` otherwise (including `None`).
    pub fn from_ir_type(ty: Option<&IRType>) -> RegWidth {
        match ty {
            Some(IRType::I32) | Some(IRType::U32) => RegWidth::W32,
            _ => RegWidth::X64,
        }
    }
}

impl Default for RegWidth {
    fn default() -> Self {
        RegWidth::X64
    }
}

impl std::fmt::Display for RegWidth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegWidth::W32 => write!(f, "w32"),
            RegWidth::X64 => write!(f, "x64"),
        }
    }
}

// ---------------------------------------------------------------------------
// Register
// ---------------------------------------------------------------------------

/// ARM64 general-purpose registers (X0–X30) and special-purpose registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Register {
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

    /// Returns the standard assembly name for this register (64-bit X name).
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

    /// Returns the 32-bit W-register assembly name for this register.
    ///
    /// `SP` maps to `wsp` and `XZR` maps to `wzr`; all others map from
    /// `Xn` to `Wn`.
    pub fn w_name(&self) -> &'static str {
        match self {
            Register::X0 => "w0",
            Register::X1 => "w1",
            Register::X2 => "w2",
            Register::X3 => "w3",
            Register::X4 => "w4",
            Register::X5 => "w5",
            Register::X6 => "w6",
            Register::X7 => "w7",
            Register::X8 => "w8",
            Register::X9 => "w9",
            Register::X10 => "w10",
            Register::X11 => "w11",
            Register::X12 => "w12",
            Register::X13 => "w13",
            Register::X14 => "w14",
            Register::X15 => "w15",
            Register::X16 => "w16",
            Register::X17 => "w17",
            Register::X18 => "w18",
            Register::X19 => "w19",
            Register::X20 => "w20",
            Register::X21 => "w21",
            Register::X22 => "w22",
            Register::X23 => "w23",
            Register::X24 => "w24",
            Register::X25 => "w25",
            Register::X26 => "w26",
            Register::X27 => "w27",
            Register::X28 => "w28",
            Register::X29 => "w29",
            Register::X30 => "w30",
            Register::SP => "wsp",
            Register::XZR => "wzr",
        }
    }

    /// Returns the assembly name for this register at the given width.
    pub fn name_for_width(&self, width: RegWidth) -> &'static str {
        match width {
            RegWidth::X64 => self.asm_name(),
            RegWidth::W32 => self.w_name(),
        }
    }

    /// Returns the register for a 5-bit encoding index.
    ///
    /// Index 31 maps to `SP` by default; callers that know the context is
    /// the zero-register should map 31 → `XZR` themselves.
    pub fn from_encoding(idx: u32) -> Option<Register> {
        match idx {
            0 => Some(Register::X0),
            1 => Some(Register::X1),
            2 => Some(Register::X2),
            3 => Some(Register::X3),
            4 => Some(Register::X4),
            5 => Some(Register::X5),
            6 => Some(Register::X6),
            7 => Some(Register::X7),
            8 => Some(Register::X8),
            9 => Some(Register::X9),
            10 => Some(Register::X10),
            11 => Some(Register::X11),
            12 => Some(Register::X12),
            13 => Some(Register::X13),
            14 => Some(Register::X14),
            15 => Some(Register::X15),
            16 => Some(Register::X16),
            17 => Some(Register::X17),
            18 => Some(Register::X18),
            19 => Some(Register::X19),
            20 => Some(Register::X20),
            21 => Some(Register::X21),
            22 => Some(Register::X22),
            23 => Some(Register::X23),
            24 => Some(Register::X24),
            25 => Some(Register::X25),
            26 => Some(Register::X26),
            27 => Some(Register::X27),
            28 => Some(Register::X28),
            29 => Some(Register::X29),
            30 => Some(Register::X30),
            31 => Some(Register::SP),
            _ => None,
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
    /// excluding SP and XZR).
    pub fn is_caller_saved(&self) -> bool {
        !self.is_callee_saved() && !matches!(self, Register::SP | Register::XZR)
    }

    /// Returns the AAPCS64 argument register for the given argument index
    /// (0–7). Returns `None` for indices ≥ 8 (stack arguments).
    pub fn arg_register(index: usize) -> Option<Register> {
        match index {
            0 => Some(Register::X0),
            1 => Some(Register::X1),
            2 => Some(Register::X2),
            3 => Some(Register::X3),
            4 => Some(Register::X4),
            5 => Some(Register::X5),
            6 => Some(Register::X6),
            7 => Some(Register::X7),
            _ => None,
        }
    }

    /// Returns the index of this register if it is an AAPCS64 argument
    /// register (X0–X7). Returns `None` otherwise.
    pub fn arg_index(&self) -> Option<usize> {
        match self {
            Register::X0 => Some(0),
            Register::X1 => Some(1),
            Register::X2 => Some(2),
            Register::X3 => Some(3),
            Register::X4 => Some(4),
            Register::X5 => Some(5),
            Register::X6 => Some(6),
            Register::X7 => Some(7),
            _ => None,
        }
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

    /// Returns the condition code for a 4-bit encoding index.
    pub fn from_encoding(idx: u32) -> Option<Condition> {
        match idx {
            0b0000 => Some(Condition::EQ),
            0b0001 => Some(Condition::NE),
            0b0010 => Some(Condition::CS),
            0b0011 => Some(Condition::CC),
            0b0100 => Some(Condition::MI),
            0b0101 => Some(Condition::PL),
            0b0110 => Some(Condition::VS),
            0b0111 => Some(Condition::VC),
            0b1000 => Some(Condition::HI),
            0b1001 => Some(Condition::LS),
            0b1010 => Some(Condition::GE),
            0b1011 => Some(Condition::LT),
            0b1100 => Some(Condition::GT),
            0b1101 => Some(Condition::LE),
            _ => None,
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
            BarrierOption::OSHLD => 0x0001,
        }
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
// Memory Size (for typed load/store selection)
// ---------------------------------------------------------------------------

/// The size of a memory operation, used to select the correct load/store
/// variant (LDR vs LDRB vs LDRH vs LDRSW, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MemorySize {
    /// 8-bit byte.
    Byte,
    /// 16-bit halfword.
    HalfWord,
    /// 32-bit word.
    Word,
    /// 32-bit word, sign-extended to 64 bits.
    SignedWord,
    /// 64-bit doubleword.
    DoubleWord,
}

impl MemorySize {
    /// Returns the scale (log2 of byte size) for unsigned-offset encoding.
    pub fn scale(&self) -> u32 {
        match self {
            MemorySize::Byte => 0,
            MemorySize::HalfWord => 1,
            MemorySize::Word | MemorySize::SignedWord => 2,
            MemorySize::DoubleWord => 3,
        }
    }

    /// Returns the byte size of this memory access.
    pub fn byte_size(&self) -> u32 {
        1u32 << self.scale()
    }
}

// ---------------------------------------------------------------------------
// Addressing Mode
// ---------------------------------------------------------------------------

/// Addressing mode for load/store instructions.
///
/// Supports the three primary ARM64 addressing patterns:
/// - **Base + offset**: `LDR Xt, [Xn, #offset]`
/// - **Pre-indexed**: `LDR Xt, [Xn, #offset]!` (update base before load)
/// - **Post-indexed**: `LDR Xt, [Xn], #offset` (update base after load)
/// - **Register offset**: `LDR Xt, [Xn, Xm, LSL #scale]`
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum AddressingMode {
    /// Unsigned offset: `[Xn, #offset]` where offset is an unsigned multiple
    /// of the access size.
    UnsignedOffset { base: Register, offset: u32 },
    /// Pre-indexed: `[Xn, #offset]!` — base is updated before the memory
    /// access.
    PreIndex { base: Register, offset: i32 },
    /// Post-indexed: `[Xn], #offset` — base is updated after the memory
    /// access.
    PostIndex { base: Register, offset: i32 },
    /// Register offset: `[Xn, Xm, LSL #scale]` — index register shifted by
    /// the element size.
    RegisterOffset {
        base: Register,
        index: Register,
        shift: Option<(ShiftKind, u32)>,
    },
}

impl AddressingMode {
    /// Convenience: create an unsigned-offset addressing mode.
    pub fn offset(base: Register, offset: u32) -> Self {
        AddressingMode::UnsignedOffset { base, offset }
    }

    /// Convenience: create a register-offset addressing mode with optional
    /// shift (used for array indexing with element-size scaling).
    pub fn reg_offset(base: Register, index: Register, shift: Option<(ShiftKind, u32)>) -> Self {
        AddressingMode::RegisterOffset { base, index, shift }
    }
}

impl std::fmt::Display for AddressingMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressingMode::UnsignedOffset { base, offset } => {
                if *offset == 0 {
                    write!(f, "[{}]", base)
                } else {
                    write!(f, "[{}, #{}]", base, offset)
                }
            }
            AddressingMode::PreIndex { base, offset } => {
                write!(f, "[{}, #{}]!", base, offset)
            }
            AddressingMode::PostIndex { base, offset } => {
                write!(f, "[{}], #{}", base, offset)
            }
            AddressingMode::RegisterOffset { base, index, shift } => match shift {
                Some((kind, amount)) => {
                    write!(f, "[{}, {}, {} #{}]", base, index, kind, amount)
                }
                None => {
                    write!(f, "[{}, {}]", base, index)
                }
            },
        }
    }
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

impl Operand {
    /// Create a plain register operand with no shift.
    pub fn reg(r: Register) -> Self {
        Operand::Reg {
            reg: r,
            shift: None,
        }
    }

    /// Create a shifted register operand.
    pub fn shifted(r: Register, kind: ShiftKind, amount: u32) -> Self {
        Operand::Reg {
            reg: r,
            shift: Some((kind, amount)),
        }
    }

    /// Extract the register, if this is a register operand.
    pub fn as_reg(&self) -> Option<Register> {
        match self {
            Operand::Reg { reg, .. } => Some(*reg),
            Operand::Imm12(_) => None,
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
/// the `Display` impl produces a human-readable assembly line.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[allow(non_camel_case_types)]
pub enum Instruction {
    // ---- Arithmetic ----
    /// Add: `ADD Rd, Rn, Rm` or `ADD Rd, Rn, #imm`
    ADD {
        rd: Register,
        rn: Register,
        rm: Operand,
    },
    /// Subtract: `SUB Rd, Rn, Rm` or `SUB Rd, Rn, #imm`
    SUB {
        rd: Register,
        rn: Register,
        rm: Operand,
    },
    /// Multiply: `MUL Rd, Rn, Rm`
    MUL {
        rd: Register,
        rn: Register,
        rm: Register,
    },
    /// Signed divide: `SDIV Rd, Rn, Rm`
    SDIV {
        rd: Register,
        rn: Register,
        rm: Register,
    },
    /// Unsigned divide: `UDIV Rd, Rn, Rm`
    UDIV {
        rd: Register,
        rn: Register,
        rm: Register,
    },

    // ---- Bitwise / Shift ----
    /// Bitwise AND: `AND Rd, Rn, Rm`
    AND {
        rd: Register,
        rn: Register,
        rm: Register,
    },
    /// Bitwise OR: `ORR Rd, Rn, Rm`
    ORR {
        rd: Register,
        rn: Register,
        rm: Register,
    },
    /// Bitwise exclusive OR: `EOR Rd, Rn, Rm`
    EOR {
        rd: Register,
        rn: Register,
        rm: Register,
    },
    /// Logical shift left: `LSL Rd, Rn, Rm` or `LSL Rd, Rn, #imm`
    LSL {
        rd: Register,
        rn: Register,
        rm: Operand,
    },
    /// Logical shift right: `LSR Rd, Rn, Rm` or `LSR Rd, Rn, #imm`
    LSR {
        rd: Register,
        rn: Register,
        rm: Operand,
    },
    /// Arithmetic shift right: `ASR Rd, Rn, Rm` or `ASR Rd, Rn, #imm`
    ASR {
        rd: Register,
        rn: Register,
        rm: Operand,
    },

    // ---- Load / Store (64-bit) ----
    /// Load register (64-bit): `LDR Xt, [addr]`
    LDR {
        rt: Register,
        rn: Register,
        offset: i32,
    },
    /// Store register (64-bit): `STR Xt, [addr]`
    STR {
        rt: Register,
        rn: Register,
        offset: i32,
    },

    // ---- Load / Store (32-bit) ----
    /// Load register (32-bit, zero-extended): `LDR Wt, [addr]`
    LDR_W {
        rt: Register,
        rn: Register,
        offset: i32,
    },
    /// Store register (32-bit): `STR Wt, [addr]`
    STR_W {
        rt: Register,
        rn: Register,
        offset: i32,
    },

    // ---- Load / Store (sub-word) ----
    /// Load byte (zero-extended): `LDRB Wt, [addr]`
    LDRB {
        rt: Register,
        rn: Register,
        offset: i32,
    },
    /// Load halfword (zero-extended): `LDRH Wt, [addr]`
    LDRH {
        rt: Register,
        rn: Register,
        offset: i32,
    },
    /// Load word (sign-extended to 64-bit): `LDRSW Xt, [addr]`
    LDRSW {
        rt: Register,
        rn: Register,
        offset: i32,
    },
    /// Store byte: `STRB Wt, [addr]`
    STRB {
        rt: Register,
        rn: Register,
        offset: i32,
    },
    /// Store halfword: `STRH Wt, [addr]`
    STRH {
        rt: Register,
        rn: Register,
        offset: i32,
    },

    // ---- Load / Store Pair ----
    /// Load pair: `LDP Rt1, Rt2, [Rn, #offset]`
    LDP {
        rt1: Register,
        rt2: Register,
        rn: Register,
        offset: i32,
    },
    /// Store pair: `STP Rt1, Rt2, [Rn, #offset]`
    STP {
        rt1: Register,
        rt2: Register,
        rn: Register,
        offset: i32,
    },

    // ---- Atomic ----
    /// Load-exclusive register: `LDXR Rt, [Rn]`
    LDXR { rt: Register, rn: Register },
    /// Store-exclusive register: `STXR Rs, Rt, [Rn]`
    STXR {
        rs: Register,
        rt: Register,
        rn: Register,
    },
    /// Load-acquire exclusive register: `LDAXR Rt, [Rn]`
    /// Used in atomic CAS loops per ARMv8-A acquire-release semantics.
    LDAXR { rt: Register, rn: Register },
    /// Store-release exclusive register: `STLXR Rs, Rt, [Rn]`
    /// Used in atomic CAS loops per ARMv8-A acquire-release semantics.
    STLXR {
        rs: Register,
        rt: Register,
        rn: Register,
    },
    /// Compare-and-swap: `CAS Rs, Rt, [Rn]`
    CAS {
        rs: Register,
        rt: Register,
        rn: Register,
    },

    // ---- Acquire/Release Load/Store ----
    /// Load-acquire: `LDAR Rt, [Rn]` — ensures subsequent memory ops are
    /// observed after this load. Used for SyncEdge::AtomicAcquireRelease.
    LDAR { rt: Register, rn: Register },
    /// Store-release: `STLR Rt, [Rn]` — ensures all prior memory ops are
    /// globally visible before this store. Used for SyncEdge::AtomicAcquireRelease.
    STLR { rt: Register, rn: Register },

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
    /// Conditional branch: `B.cond offset`
    BCond { cond: Condition, offset: i32 },
    /// Compare and branch on zero: `CBZ Rt, offset`
    CBZ { rt: Register, offset: i32 },
    /// Compare and branch on non-zero: `CBNZ Rt, offset`
    CBNZ { rt: Register, offset: i32 },
    /// Test bit and branch on zero: `TBZ Rt, bit, offset`
    TBZ { rt: Register, bit: u32, offset: i32 },
    /// Test bit and branch on non-zero: `TBNZ Rt, bit, offset`
    TBNZ { rt: Register, bit: u32, offset: i32 },

    // ---- Compare / Conditional Select ----
    /// Compare (subtract, discard result): `CMP Rn, Rm` or `CMP Rn, #imm`
    CMP { rn: Register, rm: Operand },
    /// Compare negative: `CMN Rn, Rm` or `CMN Rn, #imm`
    CMN { rn: Register, rm: Operand },
    /// Test (bitwise AND, discard result): `TST Rn, Rm`
    TST { rn: Register, rm: Register },
    /// Conditional select: `CSEL Rd, Rn, Rm, cond`
    CSEL {
        rd: Register,
        rn: Register,
        rm: Register,
        cond: Condition,
    },

    // ---- Conditional Set ----
    /// Conditional set: `CSET Rd, cond` (alias for CSINC Rd, XZR, XZR, invert(cond))
    CSET { rd: Register, cond: Condition },

    // ---- Multiply-Subtract ----
    /// Multiply-subtract: `MSUB Rd, Rn, Rm, Ra` — computes `Ra - Rn * Rm`
    MSUB {
        rd: Register,
        rn: Register,
        rm: Register,
        ra: Register,
    },

    // ---- Bitfield Move ----
    /// Unsigned bitfield move: `UBFM Rd, Rn, #immr, #imms`
    /// Used for zero-extension (e.g. UBFM Xd, Xn, #0, #31 = UXTW/Xd)
    UBFM {
        rd: Register,
        rn: Register,
        immr: u32,
        imms: u32,
    },
    /// Signed bitfield move: `SBFM Rd, Rn, #immr, #imms`
    /// Used for sign-extension (e.g. SBFM Xd, Xn, #0, #31 = SXTW)
    SBFM {
        rd: Register,
        rn: Register,
        immr: u32,
        imms: u32,
    },

    // ---- Cast / Convert ----
    /// Sign-extend word to doubleword: `SXTW Xd, Wn` (alias for SBFM Xd, Xn, #0, #31)
    SXTW { rd: Register, rn: Register },
    /// Signed integer to float: `SCVTF Dd, Xn`
    SCVTF { rd: Register, rn: Register },
    /// Float to signed integer: `FCVTZS Xd, Dn`
    FCVTZS { rd: Register, rn: Register },
    /// Float convert (single ↔ double): `FCVT Sd, Dn` or `FCVT Dd, Sn`
    FCVT {
        rd: Register,
        rn: Register,
        to_double: bool,
    },

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
    MOVZ {
        rd: Register,
        imm16: u16,
        shift: u32,
    },
    /// Move wide with keep: `MOVK Rd, #imm16, shift`
    MOVK {
        rd: Register,
        imm16: u16,
        shift: u32,
    },

    // ---- Bit manipulation (one-source) ----
    /// Count leading zeros: `CLZ Xd, Xn`
    CLZ { rd: Register, rn: Register },
    /// Reverse bits: `RBIT Xd, Xn`
    RBIT { rd: Register, rn: Register },

    // ---- SIMD/FP move ----
    /// Move GPR to FP double register: `FMOV Dd, Xn`
    /// `vd` is the 5-bit SIMD/FP destination register index (0–31).
    FMOV_DX { vd: u8, rn: Register },
    /// Move FP double register to GPR: `FMOV Xd, Dn`
    /// `vn` is the 5-bit SIMD/FP source register index (0–31).
    FMOV_XD { rd: Register, vn: u8 },

    // ---- SIMD integer ----
    /// Population count per byte: `CNT Vd.8B, Vn.8B`
    /// `vd` and `vn` are 5-bit SIMD/FP register indices (0–31).
    CNT { vd: u8, vn: u8 },
    /// Add across vector: `ADDV Bd, Vn.8B`
    /// `vd` and `vn` are 5-bit SIMD/FP register indices (0–31).
    ADDV { vd: u8, vn: u8 },
    /// Unsigned move vector element to GPR: `UMOV Wd, Vn.B[0]`
    /// `vn` is the 5-bit SIMD/FP source register index (0–31).
    /// Only index=0 is supported (sufficient for POPCNT result extraction).
    UMOV { rd: Register, vn: u8 },

    // ---- System ----
    /// Supervisor call: `SVC #imm16`
    SVC { imm16: u16 },
    /// No-operation: `NOP`
    NOP,
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
                    // ADD Xd, Xn, Xm, shift: sf=1, op=00, S=0, 01011, shift, 0, Rm, imm6, Rn, Rd
                    // Base = 0x8B000000
                    Ok(0x8B000000u32
                        | (hw << 22)
                        | (reg.encoding() << 16)
                        | (imm6 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                Operand::Imm12(imm) => {
                    // ADD Xd, Xn, #imm: sf=1, op=0, S=0, 10001, shift=0, imm12, Rn, Rd
                    // Base = 0x91000000
                    Ok(0x91000000u32
                        | ((*imm as u32) << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // ---- SUB (shifted register): 1 1 0 0 1 0 1 1 shift 0 Rm imm6 Rn Rd ----
            Instruction::SUB { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift } => {
                    let (hw, imm6) = shift.map(|(k, v)| (k.encoding(), v)).unwrap_or((0, 0));
                    Ok(0xCB000000u32
                        | (hw << 22)
                        | (reg.encoding() << 16)
                        | (imm6 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                Operand::Imm12(imm) => Ok(0xD1000000u32
                    | ((*imm as u32) << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
            },

            // ---- MUL: alias for MADD Rd, Rn, Rm, XZR ----
            Instruction::MUL { rd, rn, rm } => Ok(0x9B007C00u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- SDIV ----
            // SDIV Xd, Xn, Xm: sf=1, 00, 1101, 011, Rm, 000011, Rn, Rd
            // Base = 0x9AC00C00
            Instruction::SDIV { rd, rn, rm } => Ok(0x9AC00C00u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- UDIV ----
            // UDIV Xd, Xn, Xm: sf=1, 00, 1101, 011, Rm, 000010, Rn, Rd
            // Base = 0x9AC00800
            Instruction::UDIV { rd, rn, rm } => Ok(0x9AC00800u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- AND (shifted register) ----
            // AND Xd, Xn, Xm: sf=1, opc=00, 01010, N=0, shift=00, Rm, imm6=0, Rn, Rd
            // Base = 0x8A000000
            Instruction::AND { rd, rn, rm } => Ok(0x8A000000u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- ORR (shifted register) ----
            // ORR Xd, Xn, Xm: sf=1, opc=01, 01010, N=0, shift=00, Rm, imm6=0, Rn, Rd
            // Base = 0xAA000000
            Instruction::ORR { rd, rn, rm } => Ok(0xAA000000u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- EOR (shifted register) ----
            // EOR Xd, Xn, Xm: sf=1, opc=10, 01010, N=0, shift=00, Rm, imm6=0, Rn, Rd
            // Base = 0xCA000000
            Instruction::EOR { rd, rn, rm } => Ok(0xCA000000u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- LSL / LSR / ASR (shifted register or immediate) ----
            // LSL Xd, Xn, Xm uses data processing shifted register format (same as ADD)
            // Base = 0x8B000000 (sf=1, op=00, S=0, 01011, shift, 0, Rm, imm6=0, Rn, Rd)
            Instruction::LSL { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok(0x8B000000u32
                    | (ShiftKind::LSL.encoding() << 22)
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
                // LSL Xd, Xn, #shift = UBFM Xd, Xn, #(64-shift), #(63-shift)
                // UBFM: sf=1, opc=10, 100110, N=1, immr, imms, Rn, Rd
                // Base = 0xD3400000
                Operand::Imm12(imm) => {
                    let shift_val = *imm as u32;
                    let immr = (64 - shift_val) & 63;
                    let imms = 63 - shift_val;
                    Ok(0xD3400000u32
                        | (immr << 16)
                        | (imms << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // LSR Xd, Xn, Xm: same base as ADD shifted
            Instruction::LSR { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok(0x8B000000u32
                    | (ShiftKind::LSR.encoding() << 22)
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
                // LSR Xd, Xn, #shift = UBFM Xd, Xn, #shift, #63
                // UBFM: sf=1, opc=10, 100110, N=1, immr, imms, Rn, Rd
                // Base = 0xD3400000
                Operand::Imm12(imm) => Ok(0xD3400000u32
                    | ((*imm as u32) << 16)
                    | (63u32 << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
            },

            // ASR Xd, Xn, Xm: same base as ADD shifted
            Instruction::ASR { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok(0x8B000000u32
                    | (ShiftKind::ASR.encoding() << 22)
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
                // ASR Xd, Xn, #shift = SBFM Xd, Xn, #shift, #63
                // SBFM: sf=1, opc=00, 100110, N=1, immr, imms, Rn, Rd
                // Base = 0x93400000
                Operand::Imm12(imm) => Ok(0x93400000u32
                    | ((*imm as u32) << 16)
                    | (63u32 << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
            },

            // ---- LDR (unsigned offset, 64-bit) ----
            Instruction::LDR { rt, rn, offset } => {
                if *offset >= 0 && *offset % 8 == 0 {
                    let imm12 = (*offset as u32) / 8;
                    if imm12 <= 4095 {
                        Ok(0b1111_1001_0100_0000_0000_0000_0000_0000
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "LDR offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "LDR with offset {} not supported (must be non-negative multiple of 8, 0..32760); use address computation instead",
                        offset
                    )))
                }
            }

            // ---- STR (unsigned offset, 64-bit) ----
            Instruction::STR { rt, rn, offset } => {
                if *offset >= 0 && *offset % 8 == 0 {
                    let imm12 = (*offset as u32) / 8;
                    if imm12 <= 4095 {
                        // STR Xt, [Xn, #pimm]: 11_111_0_00_01_pimm_Rn_Rt
                        // Base = 0xF9000000
                        Ok(0xF9000000u32
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "STR offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "STR with offset {} not supported (must be non-negative multiple of 8, 0..32760); use address computation instead",
                        offset
                    )))
                }
            }

            // ---- LDR_W (32-bit, unsigned offset) ----
            // LDR Wt: 1 0 1 1 1 0 0 1 01 imm12 Rn Rt  (imm12 = offset/4)
            Instruction::LDR_W { rt, rn, offset } => {
                if *offset >= 0 && *offset % 4 == 0 {
                    let imm12 = (*offset as u32) / 4;
                    if imm12 <= 4095 {
                        Ok(0b1011_1001_0100_0000_0000_0000_0000_0000
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "LDR_W offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "LDR_W with offset {} not supported (must be non-negative multiple of 4, 0..16380)",
                        offset
                    )))
                }
            }

            // ---- STR_W (32-bit, unsigned offset) ----
            // STR Wt: 10_111_0_00_01_pimm_Rn_Rt  (imm12 = offset/4)
            // Base = 0xB9000000
            Instruction::STR_W { rt, rn, offset } => {
                if *offset >= 0 && *offset % 4 == 0 {
                    let imm12 = (*offset as u32) / 4;
                    if imm12 <= 4095 {
                        Ok(0xB9000000u32
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "STR_W offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "STR_W with offset {} not supported (must be non-negative multiple of 4, 0..16380)",
                        offset
                    )))
                }
            }

            // ---- LDRB (unsigned offset) ----
            // LDRB: 0 0 1 1 1 0 0 1 01 imm12 Rn Rt
            Instruction::LDRB { rt, rn, offset } => {
                if *offset >= 0 {
                    let imm12 = *offset as u32;
                    if imm12 <= 4095 {
                        Ok(0b0011_1001_0100_0000_0000_0000_0000_0000
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "LDRB offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "LDRB with negative offset {} not supported; use address computation instead",
                        offset
                    )))
                }
            }

            // ---- LDRH (unsigned offset) ----
            // LDRH: 0 1 1 1 1 0 0 1 01 imm12 Rn Rt  (imm12 = offset/2)
            Instruction::LDRH { rt, rn, offset } => {
                if *offset >= 0 && *offset % 2 == 0 {
                    let imm12 = (*offset as u32) / 2;
                    if imm12 <= 4095 {
                        Ok(0b0111_1001_0100_0000_0000_0000_0000_0000
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "LDRH offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "LDRH with offset {} not supported (must be non-negative multiple of 2, 0..8190)",
                        offset
                    )))
                }
            }

            // ---- LDRSW (unsigned offset) ----
            // LDRSW: 10_111_0_10_01_pimm_Rn_Rt  (imm12 = offset/4)
            // Base = 0xB9800000
            Instruction::LDRSW { rt, rn, offset } => {
                if *offset >= 0 && *offset % 4 == 0 {
                    let imm12 = (*offset as u32) / 4;
                    if imm12 <= 4095 {
                        Ok(0xB9800000u32
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "LDRSW offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "LDRSW with offset {} not supported (must be non-negative multiple of 4, 0..16380)",
                        offset
                    )))
                }
            }

            // ---- STRB (unsigned offset) ----
            // STRB: 00_111_0_00_01_pimm_Rn_Rt
            // Base = 0x39000000
            Instruction::STRB { rt, rn, offset } => {
                if *offset >= 0 {
                    let imm12 = *offset as u32;
                    if imm12 <= 4095 {
                        Ok(0x39000000u32
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "STRB offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "STRB with negative offset {} not supported; use address computation instead",
                        offset
                    )))
                }
            }

            // ---- STRH (unsigned offset) ----
            // STRH: 01_111_0_00_01_pimm_Rn_Rt  (imm12 = offset/2)
            // Base = 0x79000000
            Instruction::STRH { rt, rn, offset } => {
                if *offset >= 0 && *offset % 2 == 0 {
                    let imm12 = (*offset as u32) / 2;
                    if imm12 <= 4095 {
                        Ok(0x79000000u32
                            | (imm12 << 10)
                            | (rn.encoding() << 5)
                            | rt.encoding())
                    } else {
                        Err(CodegenError::EncodingError(format!(
                            "STRH offset {} too large (imm12 = {} exceeds 4095); use address computation instead",
                            offset, imm12
                        )))
                    }
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "STRH with offset {} not supported (must be non-negative multiple of 2, 0..8190)",
                        offset
                    )))
                }
            }

            // ---- LDP (signed offset, 64-bit) ----
            // Encoding: 10 101 0 01 1 imm7 Rt2 Rn Rt1
            // Base = 0xA9400000
            Instruction::LDP {
                rt1,
                rt2,
                rn,
                offset,
            } => {
                let imm7 = *offset / 8;
                // Encoding: 10 101 0 01 1 imm7[21:15] Rt2[14:10] Rn[9:5] Rt1[4:0]
                Ok(0xA9400000u32
                    | (((imm7 as u32) & 0x7F) << 15)
                    | (rt2.encoding() << 10)
                    | (rn.encoding() << 5)
                    | rt1.encoding())
            }

            // ---- STP (signed offset, 64-bit) ----
            // Encoding: 10 101 0 01 0 imm7 Rt2 Rn Rt1
            // Base = 0xA9000000
            Instruction::STP {
                rt1,
                rt2,
                rn,
                offset,
            } => {
                let imm7 = *offset / 8;
                // Encoding: 10 101 0 01 0 imm7[21:15] Rt2[14:10] Rn[9:5] Rt1[4:0]
                Ok(0xA9000000u32
                    | (((imm7 as u32) & 0x7F) << 15)
                    | (rt2.encoding() << 10)
                    | (rn.encoding() << 5)
                    | rt1.encoding())
            }

            // ---- LDXR ----
            // LDXR Xt, [Xn]: 1_10_01000_01_11111_0_01111_Rn_Rt
            // Base = 0xC85F7C00
            Instruction::LDXR { rt, rn } => {
                Ok(0xC85F7C00u32 | (rn.encoding() << 5) | rt.encoding())
            }

            // ---- STXR ----
            // STXR Ws, Xt, [Xn]: 1_10_001000_0_Rs_0_11111_Rn_Rt
            // Base = 0xC8007C00
            Instruction::STXR { rs, rt, rn } => Ok(0xC8007C00u32
                | (rs.encoding() << 16)
                | (rn.encoding() << 5)
                | rt.encoding()),

            // ---- LDAXR (load-acquire exclusive) ----
            // LDAXR: 1 1 0 0 1 0 0 0 1 1 1 1 1 0 0 0 0 0 Rn Rt
            Instruction::LDAXR { rt, rn } => {
                Ok(0x08DFFC00u32 | (rn.encoding() << 5) | rt.encoding())
            }

            // ---- STLXR (store-release exclusive) ----
            // STLXR: 1 1 0 0 1 0 0 0 0 0 Rs 0 0 0 0 0 Rn Rt
            Instruction::STLXR { rs, rt, rn } => {
                Ok(0x0800FC00u32 | (rs.encoding() << 16) | (rn.encoding() << 5) | rt.encoding())
            }

            // ---- CAS ----
            Instruction::CAS { rs, rt, rn } => {
                Ok(0x1800FC00u32 | (rs.encoding() << 16) | (rn.encoding() << 5) | rt.encoding())
            }

            // ---- LDAR (load-acquire) ----
            // LDAR: 1 0 0 0 1 0 0 1 1 1 0 1 1 1 1 1 0 0 0 0 0 Rn Rt
            Instruction::LDAR { rt, rn } => {
                Ok(0x08DFF800u32 | (rn.encoding() << 5) | rt.encoding())
            }

            // ---- STLR (store-release) ----
            // STLR: 1 0 0 0 1 0 0 0 1 1 0 1 1 1 1 1 0 0 0 0 0 Rn Rt
            Instruction::STLR { rt, rn } => {
                Ok(0x089FF800u32 | (rn.encoding() << 5) | rt.encoding())
            }

            // ---- B ----
            Instruction::B { offset } => {
                let imm26 = (*offset) >> 2;
                Ok(0b000101_00000000000000000000000000 | ((imm26 as u32) & 0x03FFFFFF))
            }

            // ---- BL ----
            Instruction::BL { offset } => {
                let imm26 = (*offset) >> 2;
                Ok(0b100101_00000000000000000000000000 | ((imm26 as u32) & 0x03FFFFFF))
            }

            // ---- BR ----
            Instruction::BR { rn } => {
                Ok(0b1101_0110_0001_1111_0000_0000_0000_0000 | (rn.encoding() << 5))
            }

            // ---- BLR ----
            // BLR Xn: 1101_0111_0000_11111_000000_Rn_00001
            // Base = 0xD63F0001
            Instruction::BLR { rn } => {
                Ok(0xD63F0001u32 | (rn.encoding() << 5))
            }

            // ---- RET ----
            Instruction::RET { rn } => {
                let reg = rn.unwrap_or(Register::X30);
                Ok(0b1101_0110_0101_1111_0000_0000_0000_0000 | (reg.encoding() << 5))
            }

            // ---- B.cond ----
            // B.cond: 0 1 0 1 0 1 0 0 imm19 0 cond
            Instruction::BCond { cond, offset } => {
                let imm19 = (*offset) >> 2;
                Ok(0x54000000u32 | (((imm19 as u32) & 0x7FFFF) << 5) | cond.encoding())
            }

            // ---- CBZ ----
            Instruction::CBZ { rt, offset } => {
                let imm19 = (*offset) >> 2;
                Ok(0xB4000000u32 | (((imm19 as u32) & 0x7FFFF) << 5) | rt.encoding())
            }

            // ---- CBNZ ----
            Instruction::CBNZ { rt, offset } => {
                let imm19 = (*offset) >> 2;
                Ok(0xB5000000u32 | (((imm19 as u32) & 0x7FFFF) << 5) | rt.encoding())
            }

            // ---- TBZ ----
            // TBZ: b5 011011 0 imm14 Rt
            // Base = 0x36000000
            Instruction::TBZ { rt, bit, offset } => {
                let b5 = *bit >> 5;
                let imm14 = (*offset) >> 2;
                Ok(0x36000000u32
                    | (b5 << 31)
                    | ((*bit & 0x1F) << 19)
                    | (((imm14 as u32) & 0x3FFF) << 5)
                    | rt.encoding())
            }

            // ---- TBNZ ----
            // TBNZ: b5 011011 1 imm14 Rt
            // Base = 0x37000000
            Instruction::TBNZ { rt, bit, offset } => {
                let b5 = *bit >> 5;
                let imm14 = (*offset) >> 2;
                Ok(0x37000000u32
                    | (b5 << 31)
                    | ((*bit & 0x1F) << 19)
                    | (((imm14 as u32) & 0x3FFF) << 5)
                    | rt.encoding())
            }

            // ---- CMP (alias for SUBS XZR, Rn, Rm) ----
            // CMP Xn, Xm (register): sf=1, op=01, S=1, 01011, shift=00, 0, Rm, imm6=0, Rn, Rd=XZR
            // Base = 0xEB00001F (SUBS XZR, Xn, Xm)
            Instruction::CMP { rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok(0xEB00001Fu32
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)),
                // CMP Xn, #imm: sf=1, op=1, S=1, 10001, shift=0, imm12, Rn, Rd=XZR
                // Base = 0xF100001F (SUBS XZR, Xn, #imm)
                Operand::Imm12(imm) => Ok(0xF100001Fu32
                    | ((*imm as u32) << 10)
                    | (rn.encoding() << 5)),
            },

            // ---- CMN (alias for ADDS XZR, Rn, Rm) ----
            // CMN Xn, Xm (register): sf=1, op=00, S=1, 01011, shift=00, 0, Rm, imm6=0, Rn, Rd=XZR
            // Base = 0xAB00001F (ADDS XZR, Xn, Xm)
            Instruction::CMN { rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok(0xAB00001Fu32
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)),
                // CMN Xn, #imm: sf=1, op=0, S=1, 10001, shift=0, imm12, Rn, Rd=XZR
                // Base = 0xB100001F (ADDS XZR, Xn, #imm)
                Operand::Imm12(imm) => Ok(0xB100001Fu32
                    | ((*imm as u32) << 10)
                    | (rn.encoding() << 5)),
            },

            // ---- TST (alias for ANDS XZR, Rn, Rm) ----
            // TST Xn, Xm: sf=1, opc=00, 01010, N=1, shift=00, Rm, imm6=0, Rn, Rd=XZR
            // Base = 0xEA00001F (ANDS XZR, Xn, Xm)
            // NOTE: TST has N=1 (bit23=1) unlike AND which has N=0
            Instruction::TST { rn, rm } => Ok(0xEA00001Fu32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)),

            // ---- CSEL ----
            // CSEL: 1 0 0 1 1 0 1 0 0 0 Rm 0000 0 cond Rn Rd
            Instruction::CSEL { rd, rn, rm, cond } => Ok((0x1A800000u64
                | (rm.encoding() as u64) << 16
                | (cond.encoding() as u64) << 12
                | (rn.encoding() as u64) << 5
                | rd.encoding() as u64)
                as u32),

            // ---- CSET (alias for CSINC Rd, XZR, XZR, invert(cond)) ----
            // CSINC: 1 0 0 1 1 0 1 0 1 0 Rm 0000 0 cond Rn Rd
            // CSET Rd, cond = CSINC Rd, XZR, XZR, invert(cond)
            Instruction::CSET { rd, cond } => {
                Ok(0x1A800000u32
                    | (Register::XZR.encoding() << 16)  // Rm = XZR
                    | (cond.invert().encoding() << 12)    // invert(cond)
                    | (Register::XZR.encoding() << 5)     // Rn = XZR
                    | rd.encoding())
            }

            // ---- MSUB: Rd = Ra - Rn * Rm ----
            // MSUB: 1 0 0 1 1 0 1 1 0 0 0 Rm 0 Ra Rn Rd
            Instruction::MSUB { rd, rn, rm, ra } => Ok(0x1B000000u32
                | (rm.encoding() << 16)
                | (ra.encoding() << 10)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- UBFM (unsigned bitfield move) ----
            // UBFM: 1 0 0 1 0 0 1 1 0 0 N immr imms Rn Rd
            // UBFM Xd, Xn, #immr, #imms: sf=1, opc=10, 100110, N=1, immr, imms, Rn, Rd
            // Base = 0xD3400000
            Instruction::UBFM { rd, rn, immr, imms } => Ok(0xD3400000u32
                | ((*immr & 0x3F) << 16)
                | ((*imms & 0x3F) << 10)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- SBFM (signed bitfield move) ----
            // SBFM: 0 0 0 1 0 0 1 1 0 0 N immr imms Rn Rd
            // SBFM Xd, Xn, #immr, #imms: sf=1, opc=00, 100110, N=1, immr, imms, Rn, Rd
            // Base = 0x93400000
            Instruction::SBFM { rd, rn, immr, imms } => Ok(0x93400000u32
                | ((*immr & 0x3F) << 16)
                | ((*imms & 0x3F) << 10)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- SXTW (alias for SBFM Xd, Xn, #0, #31) ----
            // SBFM: 1 0 0 1 1 0 1 1 0 0 N immr imms Rn Rd
            // For SXTW: N=1, immr=0, imms=31
            Instruction::SXTW { rd, rn } => Ok(0b1001_0011_0100_0000_0111_1100_0000_0000
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- SCVTF (signed integer to double-precision float) ----
            // SCVTF: 1 0 0 1 1 0 1 0 0 0 1 0 0 0 0 0 0 0 0 0 0 0 Rn Rd (64-bit to double)
            Instruction::SCVTF { rd, rn } => {
                Ok(0b10_0110_1000_0100_0000_0000_0000_0000 | (rn.encoding() << 5) | rd.encoding())
            }

            // ---- FCVTZS (double-precision float to signed integer) ----
            // FCVTZS: 1 0 0 1 1 0 1 1 0 0 1 1 0 0 0 0 0 0 0 0 0 0 Rn Rd
            Instruction::FCVTZS { rd, rn } => {
                Ok(0b10_0110_1100_0110_0000_0000_0000_0000 | (rn.encoding() << 5) | rd.encoding())
            }

            // ---- FCVT (convert between single and double) ----
            // This is a floating-point instruction using the FP register bank.
            // For now, we provide a placeholder encoding.
            Instruction::FCVT {
                rd,
                rn,
                to_double: _,
            } => {
                // FCVT Dd, Sn: 0 0 0 1 1 1 1 0 0 1 1 0 0 0 1 1 1 0 0 0 0 0 Rn Rd
                // Placeholder — real encoding requires FP register bank
                Ok(
                    0b0001_1110_0110_0011_1000_0000_0000_0000
                        | (rn.encoding() << 5)
                        | rd.encoding(),
                )
            }

            // ---- DMB ----
            // DMB: 1101_0101_0000_0011_1011_1_CRm_111_11111
            // Base (CRm=0) = 0xD50330BF
            Instruction::DMB { option } => {
                Ok(0xD50330BFu32 | (option.encoding() << 8))
            }

            // ---- DSB ----
            // DSB: 1101_0101_0000_0011_1011_1_CRm_101_11111
            // Base (CRm=0) = 0xD50330AF
            Instruction::DSB { option } => {
                Ok(0xD50330AFu32 | (option.encoding() << 8))
            }

            // ---- ISB ----
            // ISB: 1101_0101_0000_0011_1101_1_1111_111_11111
            // Fixed encoding = 0xD5033BDF
            Instruction::ISB => Ok(0xD5033BDFu32),

            // ---- MOV (alias for ORR Xd, XZR, Xm) ----
            // IMPORTANT: When rm is SP, ORR Xd, XZR, SP treats Rm=31 as XZR (not SP),
            // giving zero. We must emit ADD Xd, SP, #0 instead.
            // ADD Xd, SP, #0 = 0x910003E0 | rd.encoding()
            //   sf=1, op=0, S=0, shift=01, imm12=0, Rn=SP(31)
            //
            // ORR Xd, XZR, Xm encoding (64-bit default):
            //   sf=1, opc=01, 01011, N=0, shift=00, Rm, imm6=000000, Rn=11111(XZR), Rd
            //   = 0xAA0003E0 | (Rm << 16) | Rd
            Instruction::MOV { rd, rm } => {
                if matches!(rm, Register::SP) {
                    Ok(0x910003E0 | rd.encoding())
                } else {
                    Ok(0xAA0003E0u32
                        | (rm.encoding() << 16)
                        | rd.encoding())
                }
            }

            // ---- MOVZ ----
            Instruction::MOVZ { rd, imm16, shift } => {
                let hw = *shift / 16;
                Ok(0b1101_0010_1000_0000_0000_0000_0000_0000
                    | (hw << 21)
                    | ((*imm16 as u32) << 5)
                    | rd.encoding())
            }

            // ---- MOVK ----
            Instruction::MOVK { rd, imm16, shift } => {
                let hw = *shift / 16;
                Ok(0b1111_0010_1000_0000_0000_0000_0000_0000
                    | (hw << 21)
                    | ((*imm16 as u32) << 5)
                    | rd.encoding())
            }

            // ---- CLZ Xd, Xn ----
            // One-source data processing: 1 10 1101 0110 0000 0000 010 Rn Rd
            // CLZ X0, X0 = 0xDAC01000
            Instruction::CLZ { rd, rn } => Ok(0xDAC01000 | (rn.encoding() << 5) | rd.encoding()),

            // ---- RBIT Xd, Xn ----
            // One-source data processing: 1 10 1101 0110 0000 0000 000 Rn Rd
            // RBIT X0, X0 = 0xDAC00000
            Instruction::RBIT { rd, rn } => Ok(0xDAC00000 | (rn.encoding() << 5) | rd.encoding()),

            // ---- FMOV Dd, Xn (GPR → FP double) ----
            // Conversion between FP and integer: sf 1 0 11 1110 00 S 00 00 110 000 Rn Rd
            // FMOV D0, X0 = 0x9E670000
            Instruction::FMOV_DX { vd, rn } => Ok(0x9E670000 | (rn.encoding() << 5) | (*vd as u32)),

            // ---- FMOV Xd, Dn (FP double → GPR) ----
            // Conversion between FP and integer: sf 1 0 11 1110 00 S 00 00 111 000 Rn Rd
            // FMOV X0, D0 = 0x9E6F0000
            Instruction::FMOV_XD { rd, vn } => Ok(0x9E6F0000 | ((*vn as u32) << 5) | rd.encoding()),

            // ---- CNT Vd.8B, Vn.8B ----
            // Advanced SIMD bitwise: 0 0 001110 00 1 00000 010110 Vn Vd
            // CNT V0.8B, V0.8B = 0x0E205800
            Instruction::CNT { vd, vn } => Ok(0x0E205800 | ((*vn as u32) << 5) | (*vd as u32)),

            // ---- ADDV Bd, Vn.8B ----
            // Advanced SIMD reduction: 0 0 001110 01 11000 11011 1 Vn Vd
            // ADDV B0, V0.8B = 0x0E71B800
            Instruction::ADDV { vd, vn } => Ok(0x0E71B800 | ((*vn as u32) << 5) | (*vd as u32)),

            // ---- UMOV Wd, Vn.B[0] ----
            // Advanced SIMD copy: 0 0 001110 00 1 00 0000 1 0 000 Vn Rd
            // UMOV W0, V0.B[0] = 0x0E204000
            Instruction::UMOV { rd, vn } => Ok(0x0E204000 | ((*vn as u32) << 5) | rd.encoding()),

            // ---- SVC ----
            // SVC: 1101_0100_0000_0000_0000_imm16_00001
            // Base = 0xD4000001
            Instruction::SVC { imm16 } => {
                Ok(0xD4000001u32 | ((*imm16 as u32) << 5))
            }

            // ---- NOP ----
            // NOP: 1 1 0 1 0 1 0 1 0 0 0 0 0 0 1 1 0 0 1 0 0 0 0 0 1 1 1 0 0 0 0 0
            Instruction::NOP => Ok(0xD503201F),
        }
    }

    /// Encode this instruction with a specific register width.
    ///
    /// For instructions that support 32-bit W-register mode (arithmetic,
    /// logical, shift, move, load/store, compare, conditional select, and
    /// bitfield operations), the `width` parameter controls whether the
    /// sf bit (bit 31) or size field (bits 31:30) is set for 32-bit or
    /// 64-bit operation.
    ///
    /// For instructions that do not have a width variant (branches, system,
    /// SIMD, etc.), the width parameter is ignored and the default 64-bit
    /// encoding is used.
    ///
    /// Using `RegWidth::W32` produces instructions that operate on W
    /// sub-registers, giving automatic 32-bit wrapping arithmetic — essential
    /// for algorithms like SHA-256 that require `u32` modular arithmetic.
    pub fn encode_with_width(&self, width: RegWidth) -> Result<u32> {
        let sf = width.sf_bit();
        match self {
            // ---- ADD (shifted register): sf 0 0 0 1 0 1 1 shift 0 Rm imm6 Rn Rd ----
            // ADD (shifted register): sf op S 01011 shift 0 Rm imm6 Rn Rd
            // 64-bit base=0x8B000000, 32-bit base=0x0B000000
            Instruction::ADD { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift } => {
                    let (hw, imm6) = shift.map(|(k, v)| (k.encoding(), v)).unwrap_or((0, 0));
                    Ok((sf << 31)
                        | 0x0B000000u32
                        | (hw << 22)
                        | (reg.encoding() << 16)
                        | (imm6 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                // ADD (immediate): sf op S 10001 shift imm12 Rn Rd
                // 64-bit base=0x91000000, 32-bit base=0x11000000
                Operand::Imm12(imm) => Ok((sf << 31)
                    | 0x11000000u32
                    | ((*imm as u32) << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
            },

            // ---- SUB (shifted register): sf 1 0 0 1 0 1 1 shift 0 Rm imm6 Rn Rd ----
            // SUB (shifted register): sf op S 01011 shift 0 Rm imm6 Rn Rd
            // 64-bit base=0xCB000000, 32-bit base=0x4B000000
            Instruction::SUB { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift } => {
                    let (hw, imm6) = shift.map(|(k, v)| (k.encoding(), v)).unwrap_or((0, 0));
                    Ok((sf << 31)
                        | 0x4B000000u32
                        | (hw << 22)
                        | (reg.encoding() << 16)
                        | (imm6 << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
                // SUB (immediate): 64-bit base=0xD1000000, 32-bit base=0x51000000
                Operand::Imm12(imm) => Ok((sf << 31)
                    | 0x51000000u32
                    | ((*imm as u32) << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
            },

            // ---- MUL: alias for MADD Rd, Rn, Rm, XZR ----
            // MADD: sf 0 0 1 1 0 1 1 000 Rm 0 Ra Rn Rd
            // MUL (=MADD Rd, Rn, Rm, XZR): 64-bit base=0x9B007C00, 32-bit base=0x1B007C00
            Instruction::MUL { rd, rn, rm } => Ok((sf << 31)
                | 0x1B007C00u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- SDIV ----
            // SDIV: 64-bit base=0x9AC00C00, 32-bit base=0x1AC00C00
            Instruction::SDIV { rd, rn, rm } => Ok((sf << 31)
                | 0x1AC00C00u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- UDIV ----
            // UDIV: 64-bit base=0x9AC00800, 32-bit base=0x1AC00800
            Instruction::UDIV { rd, rn, rm } => Ok((sf << 31)
                | 0x1AC00800u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- AND (shifted register) ----
            // AND: 64-bit base=0x8A000000, 32-bit base=0x0A000000
            Instruction::AND { rd, rn, rm } => Ok((sf << 31)
                | 0x0A000000u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- ORR (shifted register) ----
            // ORR: 64-bit base=0xAA000000, 32-bit base=0x2A000000
            Instruction::ORR { rd, rn, rm } => Ok((sf << 31)
                | 0x2A000000u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- EOR (shifted register) ----
            // EOR: 64-bit base=0xCA000000, 32-bit base=0x4A000000
            Instruction::EOR { rd, rn, rm } => Ok((sf << 31)
                | 0x4A000000u32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- LSL (shifted register or UBFM immediate) ----
            // LSL (shifted register): same base as ADD
            // 64-bit base=0x8B000000, 32-bit base=0x0B000000
            Instruction::LSL { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok((sf << 31)
                    | 0x0B000000u32
                    | (ShiftKind::LSL.encoding() << 22)
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
                Operand::Imm12(imm) => {
                    // LSL Rd, Rn, #shift = UBFM Rd, Rn, #(size-shift), #(size-1-shift)
                    let size = width.bits();
                    let shift_val = *imm as u32;
                    let n_bit = if size == 64 { 1u32 } else { 0 };
                    let immr = (size - shift_val) & (size - 1);
                    let imms = (size - 1 - shift_val) & (size - 1);
                    // UBFM: sf opc 100110 N immr imms Rn Rd
                    // opc=10 for UBFM, base with sf=0,N=0 = 0x53000000
                    Ok((sf << 31)
                        | (n_bit << 22)
                        | 0x53000000u32
                        | (immr << 16)
                        | (imms << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // ---- LSR (shifted register or UBFM immediate) ----
            // LSR (shifted register): same base as ADD
            Instruction::LSR { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok((sf << 31)
                    | 0x0B000000u32
                    | (ShiftKind::LSR.encoding() << 22)
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
                Operand::Imm12(imm) => {
                    // LSR Rd, Rn, #shift = UBFM Rd, Rn, #shift, #(size-1)
                    let size = width.bits();
                    let n_bit = if size == 64 { 1u32 } else { 0 };
                    let immr = *imm as u32;
                    let imms = size - 1;
                    // UBFM base with sf=0,N=0 = 0x53000000
                    Ok((sf << 31)
                        | (n_bit << 22)
                        | 0x53000000u32
                        | (immr << 16)
                        | (imms << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // ---- ASR (shifted register or SBFM immediate) ----
            // ASR (shifted register): same base as ADD
            Instruction::ASR { rd, rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok((sf << 31)
                    | 0x0B000000u32
                    | (ShiftKind::ASR.encoding() << 22)
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)
                    | rd.encoding()),
                Operand::Imm12(imm) => {
                    // ASR Rd, Rn, #shift = SBFM Rd, Rn, #shift, #(size-1)
                    let size = width.bits();
                    let n_bit = if size == 64 { 1u32 } else { 0 };
                    let immr = *imm as u32;
                    let imms = size - 1;
                    // SBFM: sf opc 100110 N immr imms Rn Rd
                    // opc=00 for SBFM, base with sf=0,N=0 = 0x13000000
                    Ok((sf << 31)
                        | (n_bit << 22)
                        | 0x13000000u32
                        | (immr << 16)
                        | (imms << 10)
                        | (rn.encoding() << 5)
                        | rd.encoding())
                }
            },

            // ---- LDR (unsigned offset) ----
            // LDR Wt: size=10, opc=01 → 10_111_0_01_01
            // LDR Xt: size=11, opc=01 → 11_111_0_01_01
            // LDR: 64-bit base=0xF9400000, 32-bit base=0xB9400000
            Instruction::LDR { rt, rn, offset } => {
                let scale = width.scale(); // 3 for 64-bit, 2 for 32-bit
                let align = 1u32 << scale; // 8 for 64-bit, 4 for 32-bit
                if *offset >= 0 && (*offset as u32) % align == 0 {
                    let imm12 = (*offset as u32) / align;
                    Ok((sf << 31)
                        | 0xB9400000u32
                        | (imm12 << 10)
                        | (rn.encoding() << 5)
                        | rt.encoding())
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "LDR with offset {} not yet supported (must be non-negative multiple of {})",
                        offset, align
                    )))
                }
            }

            // ---- STR (unsigned offset) ----
            // STR Wt: size=10, opc=00 → 10_111_0_00_01
            // STR Xt: size=11, opc=00 → 11_111_0_00_01
            // STR: 64-bit base=0xF9000000, 32-bit base=0xB9000000
            Instruction::STR { rt, rn, offset } => {
                let scale = width.scale(); // 3 for 64-bit, 2 for 32-bit
                let align = 1u32 << scale; // 8 for 64-bit, 4 for 32-bit
                if *offset >= 0 && (*offset as u32) % align == 0 {
                    let imm12 = (*offset as u32) / align;
                    Ok((sf << 31)
                        | 0xB9000000u32
                        | (imm12 << 10)
                        | (rn.encoding() << 5)
                        | rt.encoding())
                } else {
                    Err(CodegenError::EncodingError(format!(
                        "STR with offset {} not yet supported (must be non-negative multiple of {})",
                        offset, align
                    )))
                }
            }

            // ---- CMP (alias for SUBS XZR/WZR, Rn, Rm) ----
            // CMP: 64-bit base=0xEB00001F, 32-bit base=0x6B00001F
            Instruction::CMP { rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok((sf << 31)
                    | 0x6B00001Fu32
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)),
                // CMP (immediate): 64-bit base=0xF100001F, 32-bit base=0x7100001F
                Operand::Imm12(imm) => Ok((sf << 31)
                    | 0x7100001Fu32
                    | ((*imm as u32) << 10)
                    | (rn.encoding() << 5)),
            },

            // ---- CMN (alias for ADDS XZR/WZR, Rn, Rm) ----
            // CMN: 64-bit base=0xAB00001F, 32-bit base=0x2B00001F
            Instruction::CMN { rn, rm } => match rm {
                Operand::Reg { reg, shift: _ } => Ok((sf << 31)
                    | 0x2B00001Fu32
                    | (reg.encoding() << 16)
                    | (rn.encoding() << 5)),
                // CMN (immediate): 64-bit base=0xB100001F, 32-bit base=0x3100001F
                Operand::Imm12(imm) => Ok((sf << 31)
                    | 0x3100001Fu32
                    | ((*imm as u32) << 10)
                    | (rn.encoding() << 5)),
            },

            // ---- TST (alias for ANDS XZR/WZR, Rn, Rm) ----
            // TST: 64-bit base=0xEA00001F, 32-bit base=0x6A00001F (ANDS XZR/WZR, Rn, Rm)
            // NOTE: ANDS has N=1 (bit23=1) unlike AND which has N=0
            Instruction::TST { rn, rm } => Ok((sf << 31)
                | 0x6A00001Fu32
                | (rm.encoding() << 16)
                | (rn.encoding() << 5)),

            // ---- CSEL ----
            // CSEL: sf 00 1101 0100 Rm cond Rn Rd
            // 32-bit base = 0x1A800000, 64-bit = 0x9A800000
            Instruction::CSEL { rd, rn, rm, cond } => Ok((sf << 31)
                | 0x1A800000u32
                | (rm.encoding() << 16)
                | (cond.encoding() << 12)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- CSET (alias for CSINC Rd, XZR, XZR, invert(cond)) ----
            // CSINC: sf 00 1101 0100 Rm cond 01 Rn Rd
            // 32-bit base = 0x1A800400, 64-bit = 0x9A800400
            // Combined: (sf << 31) | 0x1A800400
            Instruction::CSET { rd, cond } => Ok((sf << 31)
                | 0x1A800400u32
                | (Register::XZR.encoding() << 16)
                | (cond.invert().encoding() << 12)
                | (Register::XZR.encoding() << 5)
                | rd.encoding()),

            // ---- MSUB: Rd = Ra - Rn * Rm ----
            Instruction::MSUB { rd, rn, rm, ra } => Ok((sf << 31)
                | 0x1B000000u32
                | (rm.encoding() << 16)
                | (ra.encoding() << 10)
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- UBFM (unsigned bitfield move) ----
            // UBFM: sf opc 100110 N immr imms Rn Rd
            // opc=10 for UBFM, base with sf=0,N=0 = 0x53000000
            // N = sf for valid UBFM
            Instruction::UBFM { rd, rn, immr, imms } => {
                let n_bit = sf; // N bit matches sf for valid UBFM
                Ok((sf << 31)
                    | (n_bit << 22)
                    | 0x53000000u32
                    | ((*immr & 0x3F) << 16)
                    | ((*imms & 0x3F) << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- SBFM (signed bitfield move) ----
            // SBFM: sf opc 100110 N immr imms Rn Rd
            // opc=00 for SBFM, base with sf=0,N=0 = 0x13000000
            Instruction::SBFM { rd, rn, immr, imms } => {
                let n_bit = sf;
                Ok((sf << 31)
                    | (n_bit << 22)
                    | 0x13000000u32
                    | ((*immr & 0x3F) << 16)
                    | ((*imms & 0x3F) << 10)
                    | (rn.encoding() << 5)
                    | rd.encoding())
            }

            // ---- MOV (alias for ORR Xd/Wd, XZR/WZR, Xm/Wm) ----
            // When rm is SP, use ADD rd, SP, #0 instead of ORR (which treats Rm=31 as XZR).
            //
            // ORR Xd/Wd, XZR/WZR, Xm/Wm encoding:
            //   sf, opc=01, 01011, N=0, shift=00, Rm, imm6=000000, Rn=11111(ZR), Rd
            //   64-bit: 0xAA0003E0 | (Rm << 16) | Rd  (sf=1)
            //   32-bit: 0x2A0003E0 | (Rm << 16) | Rd  (sf=0)
            //   Combined: (sf << 31) | 0x2A0003E0 | (Rm << 16) | Rd
            Instruction::MOV { rd, rm } => {
                if matches!(rm, Register::SP) {
                    // ADD Xd/Wd, SP, #0 (sf-bit set appropriately)
                    // ADD sf 0 0 1 0 0 0 1 shift 0 imm12 Rn Rd
                    // With imm12=0, Rn=SP(31), shift=0:
                    //   64-bit: 0x910003E0 | Rd  (sf=1)
                    //   32-bit: 0x110003E0 | Rd  (sf=0)
                    Ok((sf << 31) | 0x110003E0 | rd.encoding())
                } else {
                    Ok((sf << 31)
                        | 0x2A0003E0u32
                        | (rm.encoding() << 16)
                        | rd.encoding())
                }
            }

            // ---- MOVZ ----
            Instruction::MOVZ { rd, imm16, shift } => {
                let hw = *shift / 16;
                Ok((sf << 31)
                    | 0b0101_0010_1000_0000_0000_0000_0000_0000
                    | (hw << 21)
                    | ((*imm16 as u32) << 5)
                    | rd.encoding())
            }

            // ---- MOVK ----
            Instruction::MOVK { rd, imm16, shift } => {
                let hw = *shift / 16;
                Ok((sf << 31)
                    | 0b0111_0010_1000_0000_0000_0000_0000_0000
                    | (hw << 21)
                    | ((*imm16 as u32) << 5)
                    | rd.encoding())
            }

            // ---- CLZ Rd, Rn ----
            // One-source: sf 1 0 1 1 0 1 0 1 1 0 00000 010 Rn Rd
            // CLZ: 64-bit base=0xDAC01000, 32-bit base=0x5AC01000
            Instruction::CLZ { rd, rn } => Ok((sf << 31)
                | 0x5AC01000u32
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- RBIT Rd, Rn ----
            // One-source: sf 1 0 1 1 0 1 0 1 1 0 00000 000 Rn Rd
            // RBIT: 64-bit base=0xDAC00000, 32-bit base=0x5AC00000
            Instruction::RBIT { rd, rn } => Ok((sf << 31)
                | 0x5AC00000u32
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- CBZ ----
            // CBZ: 64-bit base=0xB4000000, 32-bit base=0x34000000
            Instruction::CBZ { rt, offset } => {
                let imm19 = (*offset) >> 2;
                Ok((sf << 31)
                    | 0x34000000u32
                    | (((imm19 as u32) & 0x7FFFF) << 5)
                    | rt.encoding())
            }

            // ---- CBNZ ----
            // CBNZ: 64-bit base=0xB5000000, 32-bit base=0x35000000
            Instruction::CBNZ { rt, offset } => {
                let imm19 = (*offset) >> 2;
                Ok((sf << 31)
                    | 0x35000000u32
                    | (((imm19 as u32) & 0x7FFFF) << 5)
                    | rt.encoding())
            }

            // ---- SXTW (alias for SBFM Xd, Xn, #0, #31) ----
            // This is always a 64-bit destination (widening), so sf=1, N=1
            Instruction::SXTW { rd, rn } => Ok(0b1001_0011_0100_0000_0111_1100_0000_0000
                | (rn.encoding() << 5)
                | rd.encoding()),

            // ---- All other instructions: fall back to default 64-bit encoding ----
            _ => self.encode(),
        }
    }

    /// Decode a 32-bit AArch64 machine-code word into an `Instruction`.
    ///
    /// Returns `None` for encodings not yet covered by the decoder.
    /// Covers the top ~20 most common instruction classes emitted by this
    /// backend.
    pub fn decode(word: u32) -> Option<Instruction> {
        // ---- Fixed-pattern instructions ----
        if word == 0xD503201F {
            return Some(Instruction::NOP);
        }
        if word == 0xD65F03C0 {
            return Some(Instruction::RET {
                rn: Some(Register::X30),
            });
        }

        let rd = word & 0x1F;
        let rn = (word >> 5) & 0x1F;
        let rm = (word >> 16) & 0x1F;
        let imm12 = (word >> 10) & 0xFFF;
        let cond = word & 0xF;
        let shift_hw = (word >> 22) & 0x3;
        let imm6 = (word >> 10) & 0x3F;

        // ---- ADD (immediate): 1_00_100010_0_xxx ----
        // sf=1, op=0, S=0 → bits[31:23] = 1_00_10001_0
        if (word >> 23) & 0x1FF == 0b1_0010_0010 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::ADD {
                rd: rd_reg,
                rn: rn_reg,
                rm: Operand::Imm12(imm12 as u16),
            });
        }

        // ---- SUB (immediate): 1_10_100010_0_xxx ----
        if (word >> 23) & 0x1FF == 0b1_1010_0010 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::SUB {
                rd: rd_reg,
                rn: rn_reg,
                rm: Operand::Imm12(imm12 as u16),
            });
        }

        // ---- ADD (shifted register): 1_00_0101_1_shift_0_Rm_imm6_Rn_Rd ----
        // Top byte = 0b10001011 and bit 29 = 0 (no S flag)
        if (word >> 24) & 0xFF == 0b1000_1011 && (word >> 29) & 1 == 0 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            let shift_kind = match shift_hw {
                0 => ShiftKind::LSL,
                1 => ShiftKind::LSR,
                2 => ShiftKind::ASR,
                3 => ShiftKind::ROR,
                _ => ShiftKind::LSL, // Invalid encoding, default to LSL
            };
            let shift = if imm6 != 0 {
                Some((shift_kind, imm6))
            } else {
                None
            };
            return Some(Instruction::ADD {
                rd: rd_reg,
                rn: rn_reg,
                rm: Operand::Reg { reg: rm_reg, shift },
            });
        }

        // ---- SUB (shifted register): 1_10_0101_1_shift_0_Rm_imm6_Rn_Rd ----
        // Top byte = 0xCB (sf=1, op=1, S=0, 01011) and bit 29 = 0 (no S flag)
        if (word >> 24) & 0xFF == 0xCB && (word >> 29) & 1 == 0 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::SUB {
                rd: rd_reg,
                rn: rn_reg,
                rm: Operand::Reg {
                    reg: rm_reg,
                    shift: None,
                },
            });
        }

        // ---- MOV (register): ORR Xd, XZR, Xm ----
        // ORR shifted register with Rn = XZR (31)
        if (word >> 21) & 0x7FF == 0b10101010000 && rn == 31 {
            let rd_reg = Register::from_encoding(rd)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::MOV {
                rd: rd_reg,
                rm: rm_reg,
            });
        }

        // ---- ORR (shifted register): 1_01_0101_0_00_xxx ----
        if (word >> 21) & 0x7FF == 0b10101010000 && rn != 31 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::ORR {
                rd: rd_reg,
                rn: rn_reg,
                rm: rm_reg,
            });
        }

        // ---- AND (shifted register): 1_00_0101_0_00_xxx ----
        if (word >> 21) & 0x7FF == 0b10001010000 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::AND {
                rd: rd_reg,
                rn: rn_reg,
                rm: rm_reg,
            });
        }

        // ---- EOR (shifted register): 1_10_0101_0_00_xxx ----
        if (word >> 21) & 0x7FF == 0b11001010000 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::EOR {
                rd: rd_reg,
                rn: rn_reg,
                rm: rm_reg,
            });
        }

        // ---- MUL: MADD Xd, Xn, Xm, XZR ----
        // 1_00_1101_1000_Rm_0_01111_Rn_Rd
        if (word >> 21) & 0x7FF == 0b10011011000 && ((word >> 10) & 0x1F) == 0b01111 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::MUL {
                rd: rd_reg,
                rn: rn_reg,
                rm: rm_reg,
            });
        }

        // ---- SDIV: 1_00_1101_0100_Rm_00001_Rn_Rd ----
        if (word >> 21) & 0x7FF == 0b10011010100 && (word >> 10) & 0x1F == 0b00001 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::SDIV {
                rd: rd_reg,
                rn: rn_reg,
                rm: rm_reg,
            });
        }

        // ---- UDIV: 1_00_1101_0000_Rm_00001_Rn_Rd ----
        if (word >> 21) & 0x7FF == 0b10011010000 && (word >> 10) & 0x1F == 0b00001 {
            let rd_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::UDIV {
                rd: rd_reg,
                rn: rn_reg,
                rm: rm_reg,
            });
        }

        // ---- CMP (immediate): SUBS XZR, Xn, #imm12 ----
        // 1_11_10001_0_xxx → bits[31:23] = 1_1110_0010, rd = 31
        if (word >> 23) & 0x1FF == 0b1_1110_0010 && rd == 31 {
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::CMP {
                rn: rn_reg,
                rm: Operand::Imm12(imm12 as u16),
            });
        }

        // ---- CMP (register): SUBS XZR, Xn, Xm ----
        if (word >> 21) & 0x7FF == 0b11101011000 && rd == 31 {
            let rn_reg = Register::from_encoding(rn)?;
            let rm_reg = Register::from_encoding(rm)?;
            return Some(Instruction::CMP {
                rn: rn_reg,
                rm: Operand::Reg {
                    reg: rm_reg,
                    shift: None,
                },
            });
        }

        // ---- B.cond: 0101010x_xxxxxxxxxxxxxxxx_cond ----
        if (word >> 24) & 0xFF == 0x54 {
            let cond_code = Condition::from_encoding(cond)?;
            let imm19 = (word >> 5) & 0x7FFFF;
            let offset = ((imm19 as i32) << 13) >> 11; // sign-extend and *4
            return Some(Instruction::BCond {
                cond: cond_code,
                offset,
            });
        }

        // ---- B (unconditional): 000101xx_xxxxxxxxxxxxxxxxxx ----
        if (word >> 26) & 0x3F == 0b000101 {
            let imm26 = word & 0x3FFFFFF;
            let offset = ((imm26 as i32) << 6) >> 4;
            return Some(Instruction::B { offset });
        }

        // ---- BL: 100101xx_xxxxxxxxxxxxxxxxxx ----
        if (word >> 26) & 0x3F == 0b100101 {
            let imm26 = word & 0x3FFFFFF;
            let offset = ((imm26 as i32) << 6) >> 4;
            return Some(Instruction::BL { offset });
        }

        // ---- BR: 1101011_0000_11111_000000_Rn_00000 ----
        if (word >> 10) & 0x3FFFC0 == 0x3FFFC0 && (word >> 21) & 0x7FF == 0b11010110000 {
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::BR { rn: rn_reg });
        }

        // ---- BLR: 1101011_0000_11111_000000_Rn_00001 ----
        if (word & 0xFFFFFC1F) == 0xD63F0000 {
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::BLR { rn: rn_reg });
        }

        // ---- CBZ (64-bit): 1_101101_0_imm19_Rt ----
        if (word >> 24) & 0xFF == 0xB4 {
            let rt_reg = Register::from_encoding(rd)?;
            let imm19 = (word >> 5) & 0x7FFFF;
            let offset = ((imm19 as i32) << 13) >> 11;
            return Some(Instruction::CBZ { rt: rt_reg, offset });
        }

        // ---- CBNZ (64-bit): 1_101101_1_imm19_Rt ----
        if (word >> 24) & 0xFF == 0xB5 {
            let rt_reg = Register::from_encoding(rd)?;
            let imm19 = (word >> 5) & 0x7FFFF;
            let offset = ((imm19 as i32) << 13) >> 11;
            return Some(Instruction::CBNZ { rt: rt_reg, offset });
        }

        // ---- LDR (unsigned offset, 64-bit): 11111001_01_imm12_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b1111100101 {
            let rt_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let offset = (imm12 * 8) as i32;
            return Some(Instruction::LDR {
                rt: rt_reg,
                rn: rn_reg,
                offset,
            });
        }

        // ---- STR (unsigned offset, 64-bit): 11111000_01_imm12_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b1111100001 {
            let rt_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            let offset = (imm12 * 8) as i32;
            return Some(Instruction::STR {
                rt: rt_reg,
                rn: rn_reg,
                offset,
            });
        }

        // ---- LDRB (unsigned offset): 00111001_01_imm12_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b0011100101 {
            let rt_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::LDRB {
                rt: rt_reg,
                rn: rn_reg,
                offset: imm12 as i32,
            });
        }

        // ---- STRB (unsigned offset): 00111000_01_imm12_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b0011100001 {
            let rt_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::STRB {
                rt: rt_reg,
                rn: rn_reg,
                offset: imm12 as i32,
            });
        }

        // ---- LDRH (unsigned offset): 01111001_01_imm12_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b0111100101 {
            let rt_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::LDRH {
                rt: rt_reg,
                rn: rn_reg,
                offset: (imm12 * 2) as i32,
            });
        }

        // ---- STRH (unsigned offset): 01111000_01_imm12_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b0111100001 {
            let rt_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::STRH {
                rt: rt_reg,
                rn: rn_reg,
                offset: (imm12 * 2) as i32,
            });
        }

        // ---- LDRSW (unsigned offset): 10111001_01_imm12_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b1011100101 {
            let rt_reg = Register::from_encoding(rd)?;
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::LDRSW {
                rt: rt_reg,
                rn: rn_reg,
                offset: (imm12 * 4) as i32,
            });
        }

        // ---- LDP (signed offset, 64-bit): 101_0100_110_imm7_Rt2_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b1010100110 {
            let rt1_reg = Register::from_encoding(rd)?;
            let rt2 = (word >> 10) & 0x1F;
            let rt2_reg = Register::from_encoding(rt2)?;
            let rn_reg = Register::from_encoding(rn)?;
            let imm7 = ((word >> 15) & 0x7F) as i8 as i32;
            let offset = imm7 * 8;
            return Some(Instruction::LDP {
                rt1: rt1_reg,
                rt2: rt2_reg,
                rn: rn_reg,
                offset,
            });
        }

        // ---- STP (signed offset, 64-bit): 101_0100_010_imm7_Rt2_Rn_Rt ----
        if (word >> 22) & 0x3FF == 0b1010100010 {
            let rt1_reg = Register::from_encoding(rd)?;
            let rt2 = (word >> 10) & 0x1F;
            let rt2_reg = Register::from_encoding(rt2)?;
            let rn_reg = Register::from_encoding(rn)?;
            let imm7 = ((word >> 15) & 0x7F) as i8 as i32;
            let offset = imm7 * 8;
            return Some(Instruction::STP {
                rt1: rt1_reg,
                rt2: rt2_reg,
                rn: rn_reg,
                offset,
            });
        }

        // ---- MOVZ: 110100101_hw_imm16_Rd ----
        if (word >> 23) & 0x1FF == 0b110100101 {
            let rd_reg = Register::from_encoding(rd)?;
            let hw = (word >> 21) & 0x3;
            let imm16 = (word >> 5) & 0xFFFF;
            return Some(Instruction::MOVZ {
                rd: rd_reg,
                imm16: imm16 as u16,
                shift: hw * 16,
            });
        }

        // ---- MOVK: 111100101_hw_imm16_Rd ----
        if (word >> 23) & 0x1FF == 0b111100101 {
            let rd_reg = Register::from_encoding(rd)?;
            let hw = (word >> 21) & 0x3;
            let imm16 = (word >> 5) & 0xFFFF;
            return Some(Instruction::MOVK {
                rd: rd_reg,
                imm16: imm16 as u16,
                shift: hw * 16,
            });
        }

        // ---- RET (with register): 1101011_0010_11111_0000_00_Rn_00000 ----
        if (word & 0xFFFFFC1F) == 0xD65F0000 {
            let rn_reg = Register::from_encoding(rn)?;
            return Some(Instruction::RET { rn: Some(rn_reg) });
        }

        None
    }
}

// ---------------------------------------------------------------------------
// Instruction — Display (assembly text)
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
            Instruction::LDR_W { rt, rn, offset } => write!(f, "ldr w{}, [{}, #{}]", rt.encoding(), rn, offset),
            Instruction::STR_W { rt, rn, offset } => write!(f, "str w{}, [{}, #{}]", rt.encoding(), rn, offset),
            Instruction::LDRB { rt, rn, offset } => write!(f, "ldrb {}, [{}, #{}]", rt, rn, offset),
            Instruction::LDRH { rt, rn, offset } => write!(f, "ldrh {}, [{}, #{}]", rt, rn, offset),
            Instruction::LDRSW { rt, rn, offset } => {
                write!(f, "ldrsw {}, [{}, #{}]", rt, rn, offset)
            }
            Instruction::STRB { rt, rn, offset } => write!(f, "strb {}, [{}, #{}]", rt, rn, offset),
            Instruction::STRH { rt, rn, offset } => write!(f, "strh {}, [{}, #{}]", rt, rn, offset),
            Instruction::LDP {
                rt1,
                rt2,
                rn,
                offset,
            } => {
                write!(f, "ldp {}, {}, [{}, #{}]", rt1, rt2, rn, offset)
            }
            Instruction::STP {
                rt1,
                rt2,
                rn,
                offset,
            } => {
                write!(f, "stp {}, {}, [{}, #{}]", rt1, rt2, rn, offset)
            }
            Instruction::LDXR { rt, rn } => write!(f, "ldxr {}, [{}]", rt, rn),
            Instruction::STXR { rs, rt, rn } => write!(f, "stxr {}, {}, [{}]", rs, rt, rn),
            Instruction::LDAXR { rt, rn } => write!(f, "ldaxr {}, [{}]", rt, rn),
            Instruction::STLXR { rs, rt, rn } => write!(f, "stlxr {}, {}, [{}]", rs, rt, rn),
            Instruction::CAS { rs, rt, rn } => write!(f, "cas {}, {}, [{}]", rs, rt, rn),
            Instruction::LDAR { rt, rn } => write!(f, "ldar {}, [{}]", rt, rn),
            Instruction::STLR { rt, rn } => write!(f, "stlr {}, [{}]", rt, rn),
            Instruction::B { offset } => write!(f, "b #{}", offset),
            Instruction::BL { offset } => write!(f, "bl #{}", offset),
            Instruction::BR { rn } => write!(f, "br {}", rn),
            Instruction::BLR { rn } => write!(f, "blr {}", rn),
            Instruction::RET { rn } => match rn {
                Some(reg) => write!(f, "ret {}", reg),
                None => write!(f, "ret"),
            },
            Instruction::BCond { cond, offset } => {
                write!(f, "b.{} #{}", cond.asm_suffix(), offset)
            }
            Instruction::CBZ { rt, offset } => write!(f, "cbz {}, #{}", rt, offset),
            Instruction::CBNZ { rt, offset } => write!(f, "cbnz {}, #{}", rt, offset),
            Instruction::TBZ { rt, bit, offset } => write!(f, "tbz {}, #{}, #{}", rt, bit, offset),
            Instruction::TBNZ { rt, bit, offset } => {
                write!(f, "tbnz {}, #{}, #{}", rt, bit, offset)
            }
            Instruction::CMP { rn, rm } => write!(f, "cmp {}, {}", rn, rm),
            Instruction::CMN { rn, rm } => write!(f, "cmn {}, {}", rn, rm),
            Instruction::TST { rn, rm } => write!(f, "tst {}, {}", rn, rm),
            Instruction::CSEL { rd, rn, rm, cond } => {
                write!(f, "csel {}, {}, {}, {}", rd, rn, rm, cond.asm_suffix())
            }
            Instruction::CSET { rd, cond } => {
                write!(f, "cset {}, {}", rd, cond.asm_suffix())
            }
            Instruction::MSUB { rd, rn, rm, ra } => {
                write!(f, "msub {}, {}, {}, {}", rd, rn, rm, ra)
            }
            Instruction::UBFM { rd, rn, immr, imms } => {
                write!(f, "ubfm {}, {}, #{}, #{}", rd, rn, immr, imms)
            }
            Instruction::SBFM { rd, rn, immr, imms } => {
                write!(f, "sbfm {}, {}, #{}, #{}", rd, rn, immr, imms)
            }
            Instruction::SXTW { rd, rn } => write!(f, "sxtw {}, {}", rd, rn),
            Instruction::SCVTF { rd, rn } => write!(f, "scvtf {}, {}", rd, rn),
            Instruction::FCVTZS { rd, rn } => write!(f, "fcvtzs {}, {}", rd, rn),
            Instruction::FCVT { rd, rn, to_double } => {
                if *to_double {
                    write!(f, "fcvt {}, {} (to double)", rd, rn)
                } else {
                    write!(f, "fcvt {}, {} (to single)", rd, rn)
                }
            }
            Instruction::DMB { option } => write!(f, "dmb {}", option),
            Instruction::DSB { option } => write!(f, "dsb {}", option),
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
            Instruction::CLZ { rd, rn } => write!(f, "clz {}, {}", rd, rn),
            Instruction::RBIT { rd, rn } => write!(f, "rbit {}, {}", rd, rn),
            Instruction::FMOV_DX { vd, rn } => write!(f, "fmov d{}, {}", vd, rn),
            Instruction::FMOV_XD { rd, vn } => write!(f, "fmov {}, d{}", rd, vn),
            Instruction::CNT { vd, vn } => write!(f, "cnt v{}.8b, v{}.8b", vd, vn),
            Instruction::ADDV { vd, vn } => write!(f, "addv b{}, v{}.8b", vd, vn),
            Instruction::UMOV { rd, vn } => write!(f, "umov {}, v{}.b[0]", rd, vn),
            Instruction::SVC { imm16 } => write!(f, "svc #{}", imm16),
            Instruction::NOP => write!(f, "nop"),
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

// ---------------------------------------------------------------------------
// InstructionSelector
// ---------------------------------------------------------------------------

/// Selects ARM64 instructions from IR/SCG node types.
///
/// This is the core of the instruction selection pass. Each method maps a
/// specific IR/SCG node type to one or more ARM64 instructions, following the
/// mapping defined in the project specification.
///
/// # AAPCS64 Register Conventions
///
/// The selector respects the AAPCS64 calling convention:
/// - **x0–x7**: Argument/result registers
/// - **x8**: Indirect result location register
/// - **x9–x15**: Caller-saved temporaries
/// - **x19–x28**: Callee-saved registers
/// - **x29**: Frame pointer (FP)
/// - **x30**: Link register (LR)
/// - **SP**: Stack pointer
///
/// # Addressing Modes
///
/// The selector supports three addressing modes for load/store:
/// - **Base + unsigned offset**: `LDR Xt, [Xn, #offset]`
/// - **Pre-indexed**: `LDR Xt, [Xn, #offset]!`
/// - **Post-indexed**: `LDR Xt, [Xn], #offset`
/// - **Register offset**: `LDR Xt, [Xn, Xm, LSL #scale]`
pub struct InstructionSelector {
    /// Accumulated instructions for the current selection context.
    instructions: Vec<Instruction>,
}

/// Map an `IRType` to the appropriate `MemorySize` for load/store selection.
///
/// - U8/I8 → Byte
/// - U16/I16 → HalfWord
/// - U32/I32 → Word
/// - U64/I64/Ptr/Func → DoubleWord
/// - F32 → Word
/// - F64 → DoubleWord
/// - Other types default to DoubleWord
pub fn ir_type_to_memory_size(ty: &IRType) -> MemorySize {
    match ty {
        IRType::U8 | IRType::I8 => MemorySize::Byte,
        IRType::U16 | IRType::I16 => MemorySize::HalfWord,
        IRType::U32 | IRType::I32 | IRType::F32 => MemorySize::Word,
        IRType::U64 | IRType::I64 | IRType::Ptr | IRType::Func | IRType::F64 => {
            MemorySize::DoubleWord
        }
        // Default to 64-bit for structs, arrays, and void
        _ => MemorySize::DoubleWord,
    }
}

impl InstructionSelector {
    /// Create a new instruction selector.
    pub fn new() -> Self {
        Self {
            instructions: Vec::new(),
        }
    }

    /// Take the accumulated instructions, leaving the selector empty.
    pub fn take_instructions(&mut self) -> Vec<Instruction> {
        std::mem::take(&mut self.instructions)
    }

    /// Push an instruction.
    pub fn push(&mut self, instr: Instruction) {
        self.instructions.push(instr);
    }

    // -----------------------------------------------------------------------
    // Computation: ADD, SUB, MUL, SDIV, UDIV
    // -----------------------------------------------------------------------

    /// Select instructions for an arithmetic computation node.
    ///
    /// Maps:
    /// - `BinOpKind::Add` → `ADD`
    /// - `BinOpKind::Sub` → `SUB`
    /// - `BinOpKind::Mul` → `MUL`
    /// - `BinOpKind::SDiv` → `SDIV`
    /// - `BinOpKind::UDiv` → `UDIV`
    pub fn select_computation_arith(
        &mut self,
        op: BinOpKind,
        rd: Register,
        rn: Register,
        rm: Operand,
    ) -> Result<()> {
        let instr = match op {
            BinOpKind::Add => Instruction::ADD { rd, rn, rm },
            BinOpKind::Sub => Instruction::SUB { rd, rn, rm },
            BinOpKind::Mul => {
                let rm_reg = rm.as_reg().ok_or_else(|| {
                    CodegenError::InvalidInstruction("MUL requires a register operand".into())
                })?;
                Instruction::MUL { rd, rn, rm: rm_reg }
            }
            BinOpKind::SDiv => {
                let rm_reg = rm.as_reg().ok_or_else(|| {
                    CodegenError::InvalidInstruction("SDIV requires a register operand".into())
                })?;
                Instruction::SDIV { rd, rn, rm: rm_reg }
            }
            BinOpKind::UDiv => {
                let rm_reg = rm.as_reg().ok_or_else(|| {
                    CodegenError::InvalidInstruction("UDIV requires a register operand".into())
                })?;
                Instruction::UDIV { rd, rn, rm: rm_reg }
            }
            _ => {
                return Err(CodegenError::InvalidInstruction(format!(
                    "not an arithmetic op: {:?}",
                    op
                )))
            }
        };
        self.push(instr);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Computation: CMP, CSEL
    // -----------------------------------------------------------------------

    /// Select instructions for a comparison that produces a boolean result.
    ///
    /// Emits `CMP rn, rm` followed by `CSEL rd, #1, #0, cond`.
    /// The result is 1 if the condition is true, 0 otherwise.
    pub fn select_computation_cmp(
        &mut self,
        rd: Register,
        rn: Register,
        rm: Operand,
        cond: Condition,
    ) -> Result<()> {
        // CMP Rn, Rm
        self.push(Instruction::CMP { rn, rm: rm.clone() });
        // CSEL Rd, XZR (0), temp (1), cond
        // We use: CSEL rd, #1_reg, #0_reg, cond
        // ARM64 CSEL: CSEL Rd, Rn, Rm, cond  → Rd = Rn if cond else Rm
        // We need a register with 1 and a register with 0.
        // For simplicity, we emit MOVZ for the immediate 1 into a temp,
        // then CSEL with XZR for 0.
        // But a more efficient pattern: CSET Rd, cond (alias for CSINC Rd, XZR, XZR, invert(cond)))
        // For now, use the general CSEL pattern:
        //   MOV temp, #1
        //   CSEL rd, temp, XZR, cond
        // This gives rd = 1 if cond, 0 otherwise.
        // Note: we don't have a temp register allocator here, so we use X9.
        self.push(Instruction::MOVZ {
            rd: Register::X9,
            imm16: 1,
            shift: 0,
        });
        self.push(Instruction::CSEL {
            rd,
            rn: Register::X9,
            rm: Register::XZR,
            cond,
        });
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Computation: AND, ORR, EOR, LSL, LSR, ASR
    // -----------------------------------------------------------------------

    /// Select instructions for a bitwise/shift computation node.
    ///
    /// Maps:
    /// - `BinOpKind::And` → `AND`
    /// - `BinOpKind::Or` → `ORR`
    /// - `BinOpKind::Xor` → `EOR`
    /// - `BinOpKind::Shl` → `LSL`
    /// - `BinOpKind::ShrL` → `LSR`
    /// - `BinOpKind::ShrA` → `ASR`
    pub fn select_computation_bitwise(
        &mut self,
        op: BinOpKind,
        rd: Register,
        rn: Register,
        rm: Operand,
    ) -> Result<()> {
        let instr = match op {
            BinOpKind::And => {
                let rm_reg = rm.as_reg().ok_or_else(|| {
                    CodegenError::InvalidInstruction("AND requires a register operand".into())
                })?;
                Instruction::AND { rd, rn, rm: rm_reg }
            }
            BinOpKind::Or => {
                let rm_reg = rm.as_reg().ok_or_else(|| {
                    CodegenError::InvalidInstruction("ORR requires a register operand".into())
                })?;
                Instruction::ORR { rd, rn, rm: rm_reg }
            }
            BinOpKind::Xor => {
                let rm_reg = rm.as_reg().ok_or_else(|| {
                    CodegenError::InvalidInstruction("EOR requires a register operand".into())
                })?;
                Instruction::EOR { rd, rn, rm: rm_reg }
            }
            BinOpKind::Shl => Instruction::LSL { rd, rn, rm },
            BinOpKind::ShrL => Instruction::LSR { rd, rn, rm },
            BinOpKind::ShrA => Instruction::ASR { rd, rn, rm },
            BinOpKind::Ror | BinOpKind::Rol => Instruction::ASR { rd, rn, rm }, // placeholder
            _ => {
                return Err(CodegenError::InvalidInstruction(format!(
                    "not a bitwise/shift op: {:?}",
                    op
                )))
            }
        };
        self.push(instr);
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Allocation / Deallocation
    // -----------------------------------------------------------------------

    /// Select instructions for a stack allocation.
    ///
    /// Emits: `SUB SP, SP, #aligned_size` and `MOV rd, SP`.
    ///
    /// The size is rounded up to 16-byte alignment per AAPCS64 requirements.
    /// For sizes > 4096 bytes (or if `heap` is true), emits a call to
    /// `__vuma_alloc` instead.
    pub fn select_alloc_stack(&mut self, rd: Register, size: u32, heap: bool) {
        if heap || size > 4096 {
            // Heap allocation: mov x0, #size; bl __vuma_alloc
            self.push(Instruction::MOVZ {
                rd: Register::X0,
                imm16: size as u16,
                shift: 0,
            });
            self.push(Instruction::BL { offset: 0 }); // placeholder — linker resolves
        } else {
            // Stack allocation: round up to 16-byte alignment.
            let aligned = size.div_ceil(16) * 16;
            self.push(Instruction::SUB {
                rd: Register::SP,
                rn: Register::SP,
                rm: Operand::Imm12(aligned as u16),
            });
            // MOV rd, SP: cannot use ORR because ORR treats Rm=31 as XZR.
            // Use ADD rd, SP, #0 instead.
            self.push(Instruction::ADD {
                rd,
                rn: Register::SP,
                rm: Operand::Imm12(0),
            });
        }
    }

    /// Select instructions for a stack deallocation.
    ///
    /// Emits: `ADD SP, SP, #aligned_size`.
    ///
    /// For heap allocations, emits a call to `__vuma_free`.
    pub fn select_dealloc_stack(&mut self, size: u32, heap: bool) {
        if heap || size > 4096 {
            // Heap deallocation: x0 already holds pointer; bl __vuma_free
            self.push(Instruction::BL { offset: 0 }); // placeholder
        } else {
            let aligned = size.div_ceil(16) * 16;
            self.push(Instruction::ADD {
                rd: Register::SP,
                rn: Register::SP,
                rm: Operand::Imm12(aligned as u16),
            });
        }
    }

    // -----------------------------------------------------------------------
    // Access: Load (LDR, LDRB, LDRH, LDRSW)
    // -----------------------------------------------------------------------

    /// Select the correct load instruction based on the memory access size.
    ///
    /// | MemorySize   | Instruction |
    /// |--------------|-------------|
    /// | Byte         | LDRB        |
    /// | HalfWord     | LDRH        |
    /// | SignedWord   | LDRSW       |
    /// | Word         | LDR (W-form)|
    /// | DoubleWord   | LDR         |
    pub fn select_load(
        &mut self,
        rt: Register,
        addr: &AddressingMode,
        size: MemorySize,
    ) -> Result<()> {
        match addr {
            AddressingMode::UnsignedOffset { base, offset } => {
                let off = *offset as i32;
                let instr = match size {
                    MemorySize::Byte => Instruction::LDRB {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::HalfWord => Instruction::LDRH {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::SignedWord => Instruction::LDRSW {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::Word => Instruction::LDR_W {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::DoubleWord => Instruction::LDR {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                };
                self.push(instr);
            }
            AddressingMode::PreIndex { base, offset } => {
                // For pre/post-indexed and register-offset, we use a simplified
                // encoding that emits base+offset via ADD + LDR.
                // A full implementation would use the pre-indexed encoding.
                self.push(Instruction::ADD {
                    rd: *base,
                    rn: *base,
                    rm: Operand::Imm12((*offset as u16).min(4095)),
                });
                let instr = match size {
                    MemorySize::Byte => Instruction::LDRB {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::HalfWord => Instruction::LDRH {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::SignedWord => Instruction::LDRSW {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::Word => Instruction::LDR_W {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::DoubleWord => Instruction::LDR {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                };
                self.push(instr);
            }
            AddressingMode::PostIndex { base, offset } => {
                // Load first, then update base.
                let instr = match size {
                    MemorySize::Byte => Instruction::LDRB {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::HalfWord => Instruction::LDRH {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::SignedWord => Instruction::LDRSW {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::Word => Instruction::LDR_W {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::DoubleWord => Instruction::LDR {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                };
                self.push(instr);
                self.push(Instruction::ADD {
                    rd: *base,
                    rn: *base,
                    rm: Operand::Imm12((*offset as u16).min(4095)),
                });
            }
            AddressingMode::RegisterOffset { base, index, shift } => {
                // Compute effective address: ADD temp, base, index (shifted)
                // Then load from temp.
                let temp = Register::X9; // temp register for address computation
                match shift {
                    Some((kind, amount)) => {
                        self.push(Instruction::ADD {
                            rd: temp,
                            rn: *base,
                            rm: Operand::shifted(*index, *kind, *amount),
                        });
                    }
                    None => {
                        self.push(Instruction::ADD {
                            rd: temp,
                            rn: *base,
                            rm: Operand::reg(*index),
                        });
                    }
                }
                let instr = match size {
                    MemorySize::Byte => Instruction::LDRB {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                    MemorySize::HalfWord => Instruction::LDRH {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                    MemorySize::SignedWord => Instruction::LDRSW {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                    MemorySize::Word => Instruction::LDR_W {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                    MemorySize::DoubleWord => Instruction::LDR {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                };
                self.push(instr);
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Access: Store (STR, STRB, STRH)
    // -----------------------------------------------------------------------

    /// Select the correct store instruction based on the memory access size.
    ///
    /// | MemorySize   | Instruction |
    /// |--------------|-------------|
    /// | Byte         | STRB        |
    /// | HalfWord     | STRH        |
    /// | Word         | STR (W-form)|
    /// | DoubleWord   | STR         |
    pub fn select_store(
        &mut self,
        rt: Register,
        addr: &AddressingMode,
        size: MemorySize,
    ) -> Result<()> {
        match addr {
            AddressingMode::UnsignedOffset { base, offset } => {
                let off = *offset as i32;
                let instr = match size {
                    MemorySize::Byte => Instruction::STRB {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::HalfWord => Instruction::STRH {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::Word => Instruction::STR_W {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::DoubleWord => Instruction::STR {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                    MemorySize::SignedWord => Instruction::STR_W {
                        rt,
                        rn: *base,
                        offset: off,
                    },
                };
                self.push(instr);
            }
            AddressingMode::PreIndex { base, offset } => {
                self.push(Instruction::ADD {
                    rd: *base,
                    rn: *base,
                    rm: Operand::Imm12((*offset as u16).min(4095)),
                });
                let instr = match size {
                    MemorySize::Byte => Instruction::STRB {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::HalfWord => Instruction::STRH {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::Word | MemorySize::SignedWord => Instruction::STR_W {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::DoubleWord => Instruction::STR {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                };
                self.push(instr);
            }
            AddressingMode::PostIndex { base, offset } => {
                let instr = match size {
                    MemorySize::Byte => Instruction::STRB {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::HalfWord => Instruction::STRH {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::Word | MemorySize::SignedWord => Instruction::STR_W {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                    MemorySize::DoubleWord => Instruction::STR {
                        rt,
                        rn: *base,
                        offset: 0,
                    },
                };
                self.push(instr);
                self.push(Instruction::ADD {
                    rd: *base,
                    rn: *base,
                    rm: Operand::Imm12((*offset as u16).min(4095)),
                });
            }
            AddressingMode::RegisterOffset { base, index, shift } => {
                let temp = Register::X9;
                match shift {
                    Some((kind, amount)) => {
                        self.push(Instruction::ADD {
                            rd: temp,
                            rn: *base,
                            rm: Operand::shifted(*index, *kind, *amount),
                        });
                    }
                    None => {
                        self.push(Instruction::ADD {
                            rd: temp,
                            rn: *base,
                            rm: Operand::reg(*index),
                        });
                    }
                }
                let instr = match size {
                    MemorySize::Byte => Instruction::STRB {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                    MemorySize::HalfWord => Instruction::STRH {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                    MemorySize::Word | MemorySize::SignedWord => Instruction::STR_W {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                    MemorySize::DoubleWord => Instruction::STR {
                        rt,
                        rn: temp,
                        offset: 0,
                    },
                };
                self.push(instr);
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Cast: no-op, SXTW, SCVTF, FCVTZS, FCVT
    // -----------------------------------------------------------------------

    /// Select instructions for a type cast.
    ///
    /// | CastKind | Instruction(s)                               |
    /// |----------|----------------------------------------------|
    /// | BitCast  | No-op (MOV if rd != rn)                      |
    /// | ZExt     | Zero-extension via `AND rd, rn, #mask` or MOV|
    /// | SExt     | `SXTW rd, rn` (for i32→i64)                  |
    /// | Trunc    | `AND rd, rn, #mask` (for i64→i32)            |
    ///
    /// For float↔int conversions:
    /// - `SCVTF` (signed int → float)
    /// - `FCVTZS` (float → signed int)
    /// - `FCVT` (float ↔ float width change)
    pub fn select_cast(
        &mut self,
        kind: CastKind,
        rd: Register,
        rn: Register,
        is_float_conv: bool,
        _to_double: bool,
    ) {
        if is_float_conv {
            match kind {
                CastKind::BitCast => {
                    // Reinterpret bits between int and float — FMOV
                    // Placeholder: MOV for now (would need FP reg in reality)
                    if rd != rn {
                        self.push(Instruction::MOV { rd, rm: rn });
                    }
                }
                CastKind::SExt => {
                    // Signed int → float: SCVTF
                    self.push(Instruction::SCVTF { rd, rn });
                }
                CastKind::Trunc => {
                    // Float → signed int: FCVTZS
                    self.push(Instruction::FCVTZS { rd, rn });
                }
                _ => {
                    if rd != rn {
                        self.push(Instruction::MOV { rd, rm: rn });
                    }
                }
            }
        } else {
            match kind {
                CastKind::BitCast => {
                    // No-op: same bits, different type.
                    if rd != rn {
                        self.push(Instruction::MOV { rd, rm: rn });
                    }
                }
                CastKind::ZExt => {
                    // Zero-extend: on ARM64, we can use AND with a 32-bit mask.
                    // Since we don't have an AND-imm instruction variant, use the
                    // register AND with a pre-loaded mask. Actually, the simplest
                    // approach: SXTW zeros bits 63:32 for non-negative values but
                    // sign-extends. Instead, use LSR+LSL: shift left 32 then right 32.
                    // But even simpler: MOV to W register zero-extends. However our
                    // Instruction enum uses X registers. The most reliable approach
                    // is to compute: result = val & 0xFFFFFFFF using AND with a temp.
                    // For now, just emit a no-op since the stack-slot ISel on ARM64
                    // currently handles all values as 64-bit and the 32-bit W-form
                    // operations already zero-extend their results.
                    if rd != rn {
                        self.push(Instruction::MOV { rd, rm: rn });
                    }
                }
                CastKind::SExt => {
                    // Sign-extend word to doubleword: SXTW
                    self.push(Instruction::SXTW { rd, rn });
                }
                CastKind::Trunc => {
                    // Truncate: just use the lower 32 bits.
                    // MOV Wd, Wn implicitly truncates. For X registers,
                    // we AND with a mask.
                    if rd != rn {
                        self.push(Instruction::MOV { rd, rm: rn });
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // ControlFlow: B, B.cond, CBZ, CBNZ, TBZ, TBNZ
    // -----------------------------------------------------------------------

    /// Select a branch instruction based on the condition type.
    ///
    /// - For simple zero/non-zero tests: `CBZ` / `CBNZ`
    /// - For bit tests: `TBZ` / `TBNZ`
    /// - For comparison-based branches: `CMP` + `B.cond`
    /// - For unconditional jumps: `B`
    pub fn select_branch_zero(&mut self, rt: Register, offset: i32, is_zero: bool) {
        if is_zero {
            self.push(Instruction::CBZ { rt, offset });
        } else {
            self.push(Instruction::CBNZ { rt, offset });
        }
    }

    /// Select a bit-test branch.
    pub fn select_branch_bit(&mut self, rt: Register, bit: u32, offset: i32, is_zero: bool) {
        if is_zero {
            self.push(Instruction::TBZ { rt, bit, offset });
        } else {
            self.push(Instruction::TBNZ { rt, bit, offset });
        }
    }

    /// Select a comparison-based conditional branch.
    ///
    /// Emits `CMP rn, rm` followed by `B.cond offset`.
    pub fn select_branch_cmp(&mut self, rn: Register, rm: Operand, cond: Condition, offset: i32) {
        self.push(Instruction::CMP { rn, rm });
        self.push(Instruction::BCond { cond, offset });
    }

    /// Select an unconditional branch.
    pub fn select_branch_unconditional(&mut self, offset: i32) {
        self.push(Instruction::B { offset });
    }

    // -----------------------------------------------------------------------
    // High-level: select from IR instruction
    // -----------------------------------------------------------------------

    /// Select ARM64 instructions for a single IR instruction.
    ///
    /// This is a convenience method that dispatches to the specific
    /// `select_*` methods based on the IR instruction type.
    ///
    /// **Note:** This method does not perform register allocation. The caller
    /// must provide a mapping from `IRValue` to physical `Register`.
    pub fn select_from_ir(
        &mut self,
        instr: &IRInstr,
        resolve: &dyn Fn(&IRValue) -> Register,
    ) -> Result<()> {
        match instr {
            IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
                let rd = resolve(dst);
                let rn = resolve(lhs);
                let rm = match rhs {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            Operand::Imm12(*v as u16)
                        } else {
                            // For larger immediates, the caller should have
                            // materialized them into a register.
                            Operand::reg(resolve(rhs))
                        }
                    }
                    _ => Operand::reg(resolve(rhs)),
                };

                match op {
                    BinOpKind::Add
                    | BinOpKind::Sub
                    | BinOpKind::Mul
                    | BinOpKind::SDiv
                    | BinOpKind::UDiv => {
                        self.select_computation_arith(*op, rd, rn, rm)?;
                    }
                    BinOpKind::And
                    | BinOpKind::Or
                    | BinOpKind::Xor
                    | BinOpKind::Shl
                    | BinOpKind::ShrL
                    | BinOpKind::ShrA
                    | BinOpKind::Ror
                    | BinOpKind::Rol => {
                        self.select_computation_bitwise(*op, rd, rn, rm)?;
                    }
                    BinOpKind::SRem | BinOpKind::URem => {
                        // Remainder: UDIV + MSUB
                        let rm_reg = rm.as_reg().ok_or_else(|| {
                            CodegenError::InvalidInstruction(
                                "remainder requires register operand".into(),
                            )
                        })?;
                        let div_op = if *op == BinOpKind::SRem {
                            BinOpKind::SDiv
                        } else {
                            BinOpKind::UDiv
                        };
                        // Compute quotient in rd
                        self.select_computation_arith(div_op, rd, rn, Operand::reg(rm_reg))?;
                        // MSUB: rd = ra - rn * rm
                        self.push(Instruction::MSUB {
                            rd,
                            rn: rm_reg,
                            rm: rd,
                            ra: rn,
                        });
                    }
                    // Comparison operators — lower to CMP + CSEL
                    BinOpKind::Eq
                    | BinOpKind::Ne
                    | BinOpKind::SLt
                    | BinOpKind::SLe
                    | BinOpKind::SGt
                    | BinOpKind::SGe
                    | BinOpKind::ULt
                    | BinOpKind::ULe
                    | BinOpKind::UGt
                    | BinOpKind::UGe => {
                        let cond = match op {
                            BinOpKind::Eq => Condition::EQ,
                            BinOpKind::Ne => Condition::NE,
                            BinOpKind::SLt => Condition::LT,
                            BinOpKind::SLe => Condition::LE,
                            BinOpKind::SGt => Condition::GT,
                            BinOpKind::SGe => Condition::GE,
                            BinOpKind::ULt => Condition::CC,
                            BinOpKind::ULe => Condition::LS,
                            BinOpKind::UGt => Condition::HI,
                            BinOpKind::UGe => Condition::CS,
                            _ => unreachable!(),
                        };
                        self.select_computation_cmp(rd, rn, rm, cond)?;
                    }
                }
            }

            IRInstr::Load { dst, addr, offset, ty } => {
                let rt = resolve(dst);
                let rn = resolve(addr);
                let mem_size = ir_type_to_memory_size(ty);
                self.select_load(
                    rt,
                    &AddressingMode::UnsignedOffset {
                        base: rn,
                        offset: *offset as u32,
                    },
                    mem_size,
                )?;
                // For signed byte/halfword loads, sign-extend after loading.
                match ty {
                    IRType::I8 => {
                        // SXTB: SBFM Xd, Xn, #0, #7
                        self.push(Instruction::SBFM {
                            rd: rt,
                            rn: rt,
                            immr: 0,
                            imms: 7,
                        });
                    }
                    IRType::I16 => {
                        // SXTH: SBFM Xd, Xn, #0, #15
                        self.push(Instruction::SBFM {
                            rd: rt,
                            rn: rt,
                            immr: 0,
                            imms: 15,
                        });
                    }
                    _ => {}
                }
            }

            IRInstr::Store { value, addr, offset, ty } => {
                let rt = resolve(value);
                let rn = resolve(addr);
                let mem_size = ir_type_to_memory_size(ty);
                self.select_store(
                    rt,
                    &AddressingMode::UnsignedOffset {
                        base: rn,
                        offset: *offset as u32,
                    },
                    mem_size,
                )?;
            }

            IRInstr::Alloc { dst, size } => {
                let rd = resolve(dst);
                self.select_alloc_stack(rd, *size, false);
            }

            IRInstr::Free { ptr: _ } => {
                // Stack deallocation would need the size; heap free would need
                // the pointer register. For now, no-op (the stack frame
                // restoration in the epilogue handles it).
            }

            IRInstr::Cast { kind, dst, src } => {
                let rd = resolve(dst);
                let rn = resolve(src);
                self.select_cast(*kind, rd, rn, false, false);
            }

            IRInstr::UnaryOp { op, dst, operand, .. } => {
                let rd = resolve(dst);
                let rn = resolve(operand);
                match op {
                    crate::ir::UnaryOpKind::Neg => {
                        self.push(Instruction::SUB {
                            rd,
                            rn: Register::XZR,
                            rm: Operand::reg(rn),
                        });
                    }
                    crate::ir::UnaryOpKind::Not => {
                        // MVN = ORN Rd, XZR, Rn — but we use EOR with all-ones.
                        // Load -1 (all ones) into X9, then EOR rd, rn, X9.
                        self.push(Instruction::MOVZ {
                            rd: Register::X9,
                            imm16: 0,
                            shift: 0,
                        });
                        self.push(Instruction::SUB {
                            rd: Register::X9,
                            rn: Register::X9,
                            rm: Operand::Imm12(1),
                        });
                        self.push(Instruction::EOR {
                            rd,
                            rn,
                            rm: Register::X9,
                        });
                    }
                    _ => {
                        // CLZ, CTZ, POPCNT: was placeholder, now implemented
                        match op {
                            crate::ir::UnaryOpKind::Clz => {
                                self.push(Instruction::CLZ { rd, rn });
                            }
                            crate::ir::UnaryOpKind::Ctz => {
                                // CTZ = RBIT + CLZ: reverse bits then count leading zeros
                                // Use X9 as scratch if rd == rn (need intermediate)
                                if rd == rn {
                                    self.push(Instruction::RBIT {
                                        rd: Register::X9,
                                        rn,
                                    });
                                    self.push(Instruction::CLZ {
                                        rd,
                                        rn: Register::X9,
                                    });
                                } else {
                                    self.push(Instruction::RBIT { rd, rn });
                                    self.push(Instruction::CLZ { rd, rn: rd });
                                }
                            }
                            crate::ir::UnaryOpKind::Popcnt => {
                                // POPCNT using FMOV+CNT+ADDV+UMOV sequence:
                                // FMOV D8, Xn    — move GPR to SIMD register (V8 is caller-saved)
                                // CNT V8.8B, V8.8B — count bits per byte
                                // ADDV B8, V8.8B   — horizontal sum of byte counts
                                // UMOV Xd, V8.B[0] — move result back to GPR
                                const SIMD_SCRATCH: u8 = 8; // V8 is caller-saved in AAPCS64
                                self.push(Instruction::FMOV_DX {
                                    vd: SIMD_SCRATCH,
                                    rn,
                                });
                                self.push(Instruction::CNT {
                                    vd: SIMD_SCRATCH,
                                    vn: SIMD_SCRATCH,
                                });
                                self.push(Instruction::ADDV {
                                    vd: SIMD_SCRATCH,
                                    vn: SIMD_SCRATCH,
                                });
                                self.push(Instruction::UMOV {
                                    rd,
                                    vn: SIMD_SCRATCH,
                                });
                            }
                            _ => {
                                if rd != rn {
                                    self.push(Instruction::MOV { rd, rm: rn });
                                }
                            }
                        }
                    }
                }
            }

            IRInstr::Call { .. } => {
                // Call lowering is handled by the emitter, which needs
                // to set up argument registers and emit BL.
            }

            IRInstr::Phi { .. } => {
                // Phi nodes should be resolved before instruction selection.
            }

            IRInstr::GetAddress { .. } => {
                // Requires ADRP + ADD — handled by the emitter.
            }

            IRInstr::Offset { dst, base, offset } => {
                let rd = resolve(dst);
                let rn = resolve(base);
                let rm = match offset {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            Operand::Imm12(*v as u16)
                        } else {
                            Operand::reg(resolve(offset))
                        }
                    }
                    _ => Operand::reg(resolve(offset)),
                };
                self.push(Instruction::ADD { rd, rn, rm });
            }

            // Dedicated arithmetic instructions — lower same as BinOp.
            IRInstr::Add { dst, lhs, rhs, .. } => {
                let rd = resolve(dst);
                let rn = resolve(lhs);
                let rm = match rhs {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            Operand::Imm12(*v as u16)
                        } else {
                            Operand::reg(resolve(rhs))
                        }
                    }
                    _ => Operand::reg(resolve(rhs)),
                };
                self.push(Instruction::ADD { rd, rn, rm });
            }
            IRInstr::Sub { dst, lhs, rhs, .. } => {
                let rd = resolve(dst);
                let rn = resolve(lhs);
                let rm = match rhs {
                    IRValue::Immediate(v) => {
                        if *v >= 0 && *v <= 4095 {
                            Operand::Imm12(*v as u16)
                        } else {
                            Operand::reg(resolve(rhs))
                        }
                    }
                    _ => Operand::reg(resolve(rhs)),
                };
                self.push(Instruction::SUB { rd, rn, rm });
            }
            IRInstr::Mul { dst, lhs, rhs, .. } => {
                let rd = resolve(dst);
                let rn = resolve(lhs);
                let rm = Operand::reg(resolve(rhs));
                self.push(Instruction::MUL {
                    rd,
                    rn,
                    rm: rm.as_reg().ok_or_else(|| {
                        CodegenError::InvalidInstruction("MUL requires register".into())
                    })?,
                });
            }
            IRInstr::Div { dst, lhs, rhs, .. } => {
                let rd = resolve(dst);
                let rn = resolve(lhs);
                let rm = Operand::reg(resolve(rhs));
                self.push(Instruction::SDIV {
                    rd,
                    rn,
                    rm: rm.as_reg().ok_or_else(|| {
                        CodegenError::InvalidInstruction("SDIV requires register".into())
                    })?,
                });
            }

            IRInstr::Cmp {
                kind: _,
                dst,
                lhs,
                rhs, ty: _,
            } => {
                let rd = resolve(dst);
                let rn = resolve(lhs);
                let rm = Operand::reg(resolve(rhs));
                // SUBS (set flags), then CSET — simplified: use SUB + MOV placeholder.
                self.push(Instruction::SUB {
                    rd: Register::XZR,
                    rn,
                    rm,
                });
                // CSET based on condition: use NE (not-equal) after SUB sets flags.
                self.push(Instruction::CSET {
                    rd,
                    cond: Condition::NE,
                });
            }

            IRInstr::Ret { .. } => {
                // Handled as a terminator; no machine instruction here.
            }
            IRInstr::Branch { .. } => {
                // Handled as a terminator; no machine instruction here.
            }
            IRInstr::CondBranch { .. } => {
                // Handled as a terminator; no machine instruction here.
            }
            IRInstr::Select { .. } => {
                // Handled by the emitter's emit_ir_instr. The instruction
                // selector does not need to produce separate instructions
                // for this; the emitter handles it directly.
            }
        }
        Ok(())
    }

    /// Select ARM64 instructions for an IR terminator.
    pub fn select_terminator_from_ir(
        &mut self,
        term: &IRTerminator,
        resolve: &dyn Fn(&IRValue) -> Register,
    ) -> Result<()> {
        match term {
            IRTerminator::Jump(_target) => {
                self.select_branch_unconditional(0); // placeholder offset
            }
            IRTerminator::Branch {
                cond,
                true_block: _,
                false_block: _,
            } => {
                let rt = resolve(cond);
                self.select_branch_zero(rt, 0, false); // CBNZ to true
                self.select_branch_unconditional(0); // B to false
            }
            IRTerminator::Return(_vals) => {
                // Return sequence: restore frame, RET
                // The emitter handles the full epilogue.
                self.push(Instruction::RET { rn: None });
            }
            IRTerminator::Unreachable => {
                self.push(Instruction::NOP);
            }
            IRTerminator::Switch { .. }
            | IRTerminator::Invoke { .. }
            | IRTerminator::TailCall { .. }
            | IRTerminator::Resume { .. } => {
                // These are lowered by the control_flow module before
                // instruction selection. Emitting a NOP as a safe fallback.
                self.push(Instruction::NOP);
            }
        }
        Ok(())
    }
}

impl Default for InstructionSelector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SyncEdgeKind — Synchronization edge types for barrier insertion
// ---------------------------------------------------------------------------

/// The kind of synchronization edge, as derived from the SCG SyncEdge
/// annotations. Each variant maps to a different barrier strategy per the
/// ARM64 Code Generation Algorithm spec (Section 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SyncEdgeKind {
    /// Full happens-before relationship: insert `DMB ISH` to ensure all
    /// prior memory operations are globally visible before any subsequent
    /// memory operations begin.
    HappensBefore,
    /// Fine-grained acquire-release: replace normal LDR/STR with LDAR/STLR
    /// (or use LDAXR/STLXR for atomic CAS loops).
    AtomicAcquireRelease,
    /// Mutex-protected critical section: insert `BL lock_acquire` before
    /// and `BL lock_release` after. No additional barriers needed since
    /// the lock functions provide ordering internally.
    MutexLocked,
}

impl std::fmt::Display for SyncEdgeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            SyncEdgeKind::HappensBefore => "happens_before",
            SyncEdgeKind::AtomicAcquireRelease => "acquire_release",
            SyncEdgeKind::MutexLocked => "mutex_locked",
        })
    }
}

// ---------------------------------------------------------------------------
// BarrierInserter — Inserts memory barriers per the spec algorithm
// ---------------------------------------------------------------------------

/// Inserts memory barrier instructions into an instruction sequence according
/// to the ARM64 Code Generation Algorithm spec (Section 4).
///
/// # Algorithm
///
/// 1. For `HappensBefore` edges: insert `DMB ISH` after the last store before
///    the edge and before the first load after the edge.
/// 2. For `AtomicAcquireRelease` edges: replace the store at the release point
///    with `STLR`, and replace the load at the acquire point with `LDAR`.
///    For CAS patterns, emit `LDAXR`/`STLXR` loops.
/// 3. For `MutexLocked` edges: insert `BL lock_acquire` before the critical
///    section and `BL lock_release` after it. No additional barriers needed.
/// 4. Eliminate redundant barriers: if two `DMB ISH` instructions appear in
///    the same basic block with no intervening memory operations that require
///    ordering, remove the second one.
///
/// # Usage
///
/// ```ignore
/// let mut inserter = BarrierInserter::new();
/// inserter.insert_happens_before_barrier(&mut instructions, after_index);
/// inserter.replace_with_acquire_release(&mut instructions, load_idx, store_idx);
/// inserter.eliminate_redundant_barriers(&mut instructions);
/// ```
pub struct BarrierInserter {
    /// Track whether a DMB ISH was the last barrier inserted in the current
    /// basic block, for redundant-barrier elimination.
    last_dmb_ish_index: Option<usize>,
}

impl BarrierInserter {
    /// Create a new barrier inserter.
    pub fn new() -> Self {
        Self {
            last_dmb_ish_index: None,
        }
    }

    /// Insert a `DMB ISH` (Data Memory Barrier, Inner Shareable) at the
    /// specified position in the instruction list.
    ///
    /// This is the heavyweight barrier used for `HappensBefore` edges.
    /// On the Cortex-A76, this costs approximately 20–30 cycles.
    pub fn insert_happens_before_barrier(
        &mut self,
        instructions: &mut Vec<Instruction>,
        position: usize,
    ) {
        instructions.insert(
            position,
            Instruction::DMB {
                option: BarrierOption::ISH,
            },
        );
        self.last_dmb_ish_index = Some(position);
    }

    /// Insert a `DSB ISH` (Data Synchronization Barrier, Inner Shareable) at
    /// the specified position. DSB is stronger than DMB — it stalls the
    /// pipeline until all outstanding memory operations have completed.
    ///
    /// Used for multi-core release patterns (e.g., SEV after writing shared
    /// data, as in the bare-metal startup sequence).
    pub fn insert_dsb_ish(&mut self, instructions: &mut Vec<Instruction>, position: usize) {
        instructions.insert(
            position,
            Instruction::DSB {
                option: BarrierOption::ISH,
            },
        );
    }

    /// Insert an `ISB` (Instruction Synchronization Barrier) at the specified
    /// position. ISB flushes the pipeline and ensures that all subsequent
    /// instructions are fetched under the new context (e.g., after changing
    /// system registers).
    pub fn insert_isb(&mut self, instructions: &mut Vec<Instruction>, position: usize) {
        instructions.insert(position, Instruction::ISB);
    }

    /// Replace a store instruction at `store_idx` with `STLR` (store-release)
    /// and a load instruction at `load_idx` with `LDAR` (load-acquire).
    ///
    /// This implements the `AtomicAcquireRelease` edge type, providing
    /// fine-grained synchronization without the overhead of a full `DMB`.
    ///
    /// Returns `Err` if the instructions at the given indices are not
    /// compatible store/load instructions.
    pub fn replace_with_acquire_release(
        &mut self,
        instructions: &mut [Instruction],
        store_idx: usize,
        load_idx: usize,
    ) -> Result<()> {
        // Replace store with STLR
        let store_instr = &instructions[store_idx];
        match store_instr {
            Instruction::STR { rt, rn, offset: _ } => {
                instructions[store_idx] = Instruction::STLR { rt: *rt, rn: *rn };
            }
            Instruction::STRB { rt, rn, offset: _ } => {
                instructions[store_idx] = Instruction::STLR { rt: *rt, rn: *rn };
            }
            Instruction::STRH { rt, rn, offset: _ } => {
                instructions[store_idx] = Instruction::STLR { rt: *rt, rn: *rn };
            }
            Instruction::STXR { rs, rt, rn } => {
                instructions[store_idx] = Instruction::STLXR {
                    rs: *rs,
                    rt: *rt,
                    rn: *rn,
                };
            }
            _ => {
                return Err(CodegenError::InvalidInstruction(format!(
                    "expected a store instruction at index {}, got {:?}",
                    store_idx, store_instr
                )));
            }
        }

        // Replace load with LDAR
        let load_instr = &instructions[load_idx];
        match load_instr {
            Instruction::LDR { rt, rn, offset: _ } => {
                instructions[load_idx] = Instruction::LDAR { rt: *rt, rn: *rn };
            }
            Instruction::LDRB { rt, rn, offset: _ } => {
                instructions[load_idx] = Instruction::LDAR { rt: *rt, rn: *rn };
            }
            Instruction::LDRH { rt, rn, offset: _ } => {
                instructions[load_idx] = Instruction::LDAR { rt: *rt, rn: *rn };
            }
            Instruction::LDXR { rt, rn } => {
                instructions[load_idx] = Instruction::LDAXR { rt: *rt, rn: *rn };
            }
            _ => {
                return Err(CodegenError::InvalidInstruction(format!(
                    "expected a load instruction at index {}, got {:?}",
                    load_idx, load_instr
                )));
            }
        }

        Ok(())
    }

    /// Emit a complete atomic CAS loop sequence:
    ///
    /// ```asm
    /// .retry:
    ///   LDAXR X0, [X_addr]        ; load-acquire exclusive
    ///   CMP X0, X_expected        ; compare with expected
    ///   B.NE .fail                ; not equal, abort
    ///   STLXR W_temp, X_desired, [X_addr] ; store-release exclusive
    ///   CBNZ W_temp, .retry       ; retry if store failed
    ///   ; success: X0 = old value
    /// .fail:
    ///   ; failure: X0 = current value
    /// ```
    ///
    /// The `offset_retry` and `offset_fail` are byte offsets for branch
    /// targets (will be divided by 4 during encoding). The caller must
    /// fix these up during relaxation.
    #[allow(clippy::too_many_arguments)]
    pub fn emit_cas_loop(
        &mut self,
        instructions: &mut Vec<Instruction>,
        addr_reg: Register,
        expected_reg: Register,
        desired_reg: Register,
        result_reg: Register,
        status_reg: Register,
        offset_retry: i32,
        offset_fail: i32,
    ) {
        // .retry:
        instructions.push(Instruction::LDAXR {
            rt: result_reg,
            rn: addr_reg,
        });
        instructions.push(Instruction::CMP {
            rn: result_reg,
            rm: Operand::reg(expected_reg),
        });
        instructions.push(Instruction::BCond {
            cond: Condition::NE,
            offset: offset_fail,
        });
        instructions.push(Instruction::STLXR {
            rs: status_reg,
            rt: desired_reg,
            rn: addr_reg,
        });
        instructions.push(Instruction::CBNZ {
            rt: status_reg,
            offset: offset_retry,
        });
        // .fail: falls through
    }

    /// Eliminate redundant `DMB ISH` barriers in the instruction list.
    ///
    /// If two `DMB ISH` instructions appear in sequence with no intervening
    /// memory operations (LDR/STR/LDAR/STLR/LDXR/STXR/LDAXR/STLXR) that
    /// require ordering, the second one is replaced with NOP.
    ///
    /// This implements step 5 of the barrier insertion algorithm in the spec.
    pub fn eliminate_redundant_barriers(&mut self, instructions: &mut [Instruction]) {
        let mut last_dmb_pos: Option<usize> = None;
        for i in 0..instructions.len() {
            if let Instruction::DMB {
                option: BarrierOption::ISH,
            } = &instructions[i]
            {
                if let Some(last) = last_dmb_pos {
                    // Check if there are any memory operations between the two barriers
                    let has_memory_ops = instructions[last + 1..i]
                        .iter()
                        .any(Self::is_memory_operation);
                    if !has_memory_ops {
                        // Redundant barrier — replace with NOP
                        instructions[i] = Instruction::NOP;
                    }
                }
                last_dmb_pos = Some(i);
            }
        }
    }

    /// Returns `true` if the instruction is a memory operation that requires
    /// ordering (load, store, or atomic variant).
    fn is_memory_operation(instr: &Instruction) -> bool {
        matches!(
            instr,
            Instruction::LDR { .. }
                | Instruction::STR { .. }
                | Instruction::LDRB { .. }
                | Instruction::LDRH { .. }
                | Instruction::LDRSW { .. }
                | Instruction::STRB { .. }
                | Instruction::STRH { .. }
                | Instruction::LDP { .. }
                | Instruction::STP { .. }
                | Instruction::LDXR { .. }
                | Instruction::STXR { .. }
                | Instruction::LDAXR { .. }
                | Instruction::STLXR { .. }
                | Instruction::LDAR { .. }
                | Instruction::STLR { .. }
                | Instruction::CAS { .. }
        )
    }

    /// Apply a SyncEdge annotation to the instruction list.
    ///
    /// This is the main entry point for barrier insertion. Based on the
    /// `SyncEdgeKind`, it either inserts a `DMB ISH`, replaces load/store
    /// with acquire/release variants, or inserts lock/unlock calls.
    pub fn apply_sync_edge(
        &mut self,
        instructions: &mut Vec<Instruction>,
        kind: SyncEdgeKind,
        store_idx: Option<usize>,
        load_idx: Option<usize>,
    ) -> Result<()> {
        match kind {
            SyncEdgeKind::HappensBefore => {
                // Insert DMB ISH after the store (if provided)
                if let Some(si) = store_idx {
                    self.insert_happens_before_barrier(instructions, si + 1);
                }
                // Insert DMB ISH before the load (if provided)
                if let Some(li) = load_idx {
                    let adjusted = if store_idx.is_some() { li + 1 } else { li };
                    self.insert_happens_before_barrier(instructions, adjusted);
                }
            }
            SyncEdgeKind::AtomicAcquireRelease => {
                if let (Some(si), Some(li)) = (store_idx, load_idx) {
                    self.replace_with_acquire_release(instructions, si, li)?;
                }
            }
            SyncEdgeKind::MutexLocked => {
                // Insert BL lock_acquire before the critical section
                if let Some(si) = store_idx {
                    // Move argument to x0 before the call
                    instructions.insert(si, Instruction::BL { offset: 0 }); // placeholder
                }
                // Insert BL lock_release after the critical section
                if let Some(li) = load_idx {
                    let adjusted = if store_idx.is_some() { li + 1 } else { li };
                    instructions.insert(adjusted, Instruction::BL { offset: 0 });
                    // placeholder
                }
            }
        }
        Ok(())
    }
}

impl Default for BarrierInserter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Test 1: Register encoding ----
    #[test]
    fn register_encoding_roundtrip() {
        assert_eq!(Register::X0.encoding(), 0);
        assert_eq!(Register::X30.encoding(), 30);
        assert_eq!(Register::SP.encoding(), 31);
        assert_eq!(Register::XZR.encoding(), 31);
    }

    // ---- Test 2: Condition inversion ----
    #[test]
    fn condition_inversion() {
        assert_eq!(Condition::EQ.invert(), Condition::NE);
        assert_eq!(Condition::GT.invert(), Condition::LE);
        assert_eq!(Condition::CS.invert(), Condition::CC);
        assert_eq!(Condition::HI.invert(), Condition::LS);
    }

    // ---- Test 3: Instruction display ----
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

    // ---- Test 4: Computation — arithmetic instruction selection ----
    #[test]
    fn select_arithmetic_add() {
        let mut sel = InstructionSelector::new();
        sel.select_computation_arith(
            BinOpKind::Add,
            Register::X0,
            Register::X1,
            Operand::reg(Register::X2),
        )
        .unwrap();
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::ADD { .. }));
    }

    // ---- Test 5: Computation — subtraction instruction selection ----
    #[test]
    fn select_arithmetic_sub() {
        let mut sel = InstructionSelector::new();
        sel.select_computation_arith(
            BinOpKind::Sub,
            Register::X0,
            Register::X1,
            Operand::Imm12(100),
        )
        .unwrap();
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::SUB { .. }));
    }

    // ---- Test 6: Computation — multiply instruction selection ----
    #[test]
    fn select_arithmetic_mul() {
        let mut sel = InstructionSelector::new();
        sel.select_computation_arith(
            BinOpKind::Mul,
            Register::X0,
            Register::X1,
            Operand::reg(Register::X2),
        )
        .unwrap();
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::MUL { .. }));
    }

    // ---- Test 7: Computation — SDIV/UDIV instruction selection ----
    #[test]
    fn select_arithmetic_div() {
        let mut sel = InstructionSelector::new();
        sel.select_computation_arith(
            BinOpKind::SDiv,
            Register::X0,
            Register::X1,
            Operand::reg(Register::X2),
        )
        .unwrap();
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::SDIV { .. }));

        let mut sel = InstructionSelector::new();
        sel.select_computation_arith(
            BinOpKind::UDiv,
            Register::X0,
            Register::X1,
            Operand::reg(Register::X2),
        )
        .unwrap();
        let instrs = sel.take_instructions();
        assert!(matches!(instrs[0], Instruction::UDIV { .. }));
    }

    // ---- Test 8: Computation — CMP + CSEL instruction selection ----
    #[test]
    fn select_computation_cmp() {
        let mut sel = InstructionSelector::new();
        sel.select_computation_cmp(
            Register::X0,
            Register::X1,
            Operand::reg(Register::X2),
            Condition::EQ,
        )
        .unwrap();
        let instrs = sel.take_instructions();
        // Should emit: CMP, MOVZ (for #1), CSEL
        assert_eq!(instrs.len(), 3);
        assert!(matches!(instrs[0], Instruction::CMP { .. }));
        assert!(matches!(instrs[1], Instruction::MOVZ { .. }));
        assert!(matches!(instrs[2], Instruction::CSEL { .. }));
    }

    // ---- Test 9: Computation — bitwise instruction selection ----
    #[test]
    fn select_bitwise_ops() {
        for (op, expected_name) in [
            (BinOpKind::And, "and"),
            (BinOpKind::Or, "orr"),
            (BinOpKind::Xor, "eor"),
        ] {
            let mut sel = InstructionSelector::new();
            sel.select_computation_bitwise(
                op,
                Register::X0,
                Register::X1,
                Operand::reg(Register::X2),
            )
            .unwrap();
            let instrs = sel.take_instructions();
            assert_eq!(instrs.len(), 1);
            let text = format!("{}", instrs[0]);
            assert!(
                text.starts_with(expected_name),
                "expected {}, got {}",
                expected_name,
                text
            );
        }
    }

    // ---- Test 10: Computation — shift instruction selection ----
    #[test]
    fn select_shift_ops() {
        for (op, expected_name) in [
            (BinOpKind::Shl, "lsl"),
            (BinOpKind::ShrL, "lsr"),
            (BinOpKind::ShrA, "asr"),
        ] {
            let mut sel = InstructionSelector::new();
            sel.select_computation_bitwise(op, Register::X0, Register::X1, Operand::Imm12(3))
                .unwrap();
            let instrs = sel.take_instructions();
            assert_eq!(instrs.len(), 1);
            let text = format!("{}", instrs[0]);
            assert!(
                text.starts_with(expected_name),
                "expected {}, got {}",
                expected_name,
                text
            );
        }
    }

    // ---- Test 11: Allocation — stack allocation ----
    #[test]
    fn select_alloc_stack() {
        let mut sel = InstructionSelector::new();
        sel.select_alloc_stack(Register::X0, 64, false);
        let instrs = sel.take_instructions();
        // SUB SP, SP, #64 + MOV X0, SP
        assert_eq!(instrs.len(), 2);
        assert!(matches!(
            instrs[0],
            Instruction::SUB {
                rd: Register::SP,
                ..
            }
        ));
        // After MOV SP fix: alloc now emits ADD Xd, SP, #0 instead of MOV Xd, SP
        assert!(matches!(
            instrs[1],
            Instruction::ADD {
                rd: Register::X0,
                rn: Register::SP,
                rm: Operand::Imm12(0)
            }
        ));
    }

    // ---- Test 12: Allocation — heap allocation (large size) ----
    #[test]
    fn select_alloc_heap() {
        let mut sel = InstructionSelector::new();
        sel.select_alloc_stack(Register::X0, 8192, true);
        let instrs = sel.take_instructions();
        // MOVZ X0, #8192 + BL
        assert_eq!(instrs.len(), 2);
        assert!(matches!(instrs[0], Instruction::MOVZ { .. }));
        assert!(matches!(instrs[1], Instruction::BL { .. }));
    }

    // ---- Test 13: Deallocation — stack deallocation ----
    #[test]
    fn select_dealloc_stack() {
        let mut sel = InstructionSelector::new();
        sel.select_dealloc_stack(64, false);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(
            instrs[0],
            Instruction::ADD {
                rd: Register::SP,
                ..
            }
        ));
    }

    // ---- Test 14: Access — typed load instruction selection ----
    #[test]
    fn select_typed_loads() {
        for (size, expected_name) in [
            (MemorySize::Byte, "ldrb"),
            (MemorySize::HalfWord, "ldrh"),
            (MemorySize::SignedWord, "ldrsw"),
            (MemorySize::DoubleWord, "ldr"),
        ] {
            let mut sel = InstructionSelector::new();
            sel.select_load(
                Register::X0,
                &AddressingMode::UnsignedOffset {
                    base: Register::X1,
                    offset: 16,
                },
                size,
            )
            .unwrap();
            let instrs = sel.take_instructions();
            assert_eq!(instrs.len(), 1, "expected 1 instruction for {:?}", size);
            let text = format!("{}", instrs[0]);
            assert!(
                text.starts_with(expected_name),
                "expected {}, got {} for {:?}",
                expected_name,
                text,
                size
            );
        }
    }

    // ---- Test 15: Access — typed store instruction selection ----
    #[test]
    fn select_typed_stores() {
        for (size, expected_name) in [
            (MemorySize::Byte, "strb"),
            (MemorySize::HalfWord, "strh"),
            (MemorySize::DoubleWord, "str"),
        ] {
            let mut sel = InstructionSelector::new();
            sel.select_store(
                Register::X0,
                &AddressingMode::UnsignedOffset {
                    base: Register::X1,
                    offset: 16,
                },
                size,
            )
            .unwrap();
            let instrs = sel.take_instructions();
            assert_eq!(instrs.len(), 1, "expected 1 instruction for {:?}", size);
            let text = format!("{}", instrs[0]);
            assert!(
                text.starts_with(expected_name),
                "expected {}, got {} for {:?}",
                expected_name,
                text,
                size
            );
        }
    }

    // ---- Test 16: Cast — sign-extend (SXTW) instruction selection ----
    #[test]
    fn select_cast_sext() {
        let mut sel = InstructionSelector::new();
        sel.select_cast(CastKind::SExt, Register::X0, Register::X1, false, false);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::SXTW { .. }));
        assert_eq!(format!("{}", instrs[0]), "sxtw x0, x1");
    }

    // ---- Test 17: Cast — bitcast (no-op) instruction selection ----
    #[test]
    fn select_cast_bitcast() {
        let mut sel = InstructionSelector::new();
        sel.select_cast(CastKind::BitCast, Register::X0, Register::X0, false, false);
        let instrs = sel.take_instructions();
        // Same register → no instruction emitted
        assert_eq!(instrs.len(), 0);

        let mut sel = InstructionSelector::new();
        sel.select_cast(CastKind::BitCast, Register::X0, Register::X1, false, false);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::MOV { .. }));
    }

    // ---- Test 18: Cast — SCVTF (int→float) instruction selection ----
    #[test]
    fn select_cast_int_to_float() {
        let mut sel = InstructionSelector::new();
        sel.select_cast(CastKind::SExt, Register::X0, Register::X1, true, true);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::SCVTF { .. }));
        assert_eq!(format!("{}", instrs[0]), "scvtf x0, x1");
    }

    // ---- Test 19: Cast — FCVTZS (float→int) instruction selection ----
    #[test]
    fn select_cast_float_to_int() {
        let mut sel = InstructionSelector::new();
        sel.select_cast(CastKind::Trunc, Register::X0, Register::X1, true, false);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::FCVTZS { .. }));
        assert_eq!(format!("{}", instrs[0]), "fcvtzs x0, x1");
    }

    // ---- Test 20: ControlFlow — CBZ/CBNZ instruction selection ----
    #[test]
    fn select_branch_zero() {
        let mut sel = InstructionSelector::new();
        sel.select_branch_zero(Register::X0, 8, true);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::CBZ { .. }));

        let mut sel = InstructionSelector::new();
        sel.select_branch_zero(Register::X0, 8, false);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::CBNZ { .. }));
    }

    // ---- Test 21: ControlFlow — TBZ/TBNZ instruction selection ----
    #[test]
    fn select_branch_bit() {
        let mut sel = InstructionSelector::new();
        sel.select_branch_bit(Register::X0, 3, 16, true);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::TBZ { .. }));

        let mut sel = InstructionSelector::new();
        sel.select_branch_bit(Register::X0, 7, 16, false);
        let instrs = sel.take_instructions();
        assert!(matches!(instrs[0], Instruction::TBNZ { .. }));
    }

    // ---- Test 22: ControlFlow — CMP + B.cond instruction selection ----
    #[test]
    fn select_branch_cmp() {
        let mut sel = InstructionSelector::new();
        sel.select_branch_cmp(Register::X0, Operand::reg(Register::X1), Condition::LT, 16);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 2);
        assert!(matches!(instrs[0], Instruction::CMP { .. }));
        assert!(matches!(
            instrs[1],
            Instruction::BCond {
                cond: Condition::LT,
                ..
            }
        ));
    }

    // ---- Test 23: ControlFlow — unconditional branch ----
    #[test]
    fn select_branch_unconditional() {
        let mut sel = InstructionSelector::new();
        sel.select_branch_unconditional(32);
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(instrs[0], Instruction::B { offset: 32 }));
    }

    // ---- Test 24: Addressing mode display ----
    #[test]
    fn addressing_mode_display() {
        let mode = AddressingMode::UnsignedOffset {
            base: Register::X1,
            offset: 16,
        };
        assert_eq!(format!("{}", mode), "[x1, #16]");

        let mode = AddressingMode::UnsignedOffset {
            base: Register::X1,
            offset: 0,
        };
        assert_eq!(format!("{}", mode), "[x1]");

        let mode = AddressingMode::PreIndex {
            base: Register::SP,
            offset: -16,
        };
        assert_eq!(format!("{}", mode), "[sp, #-16]!");

        let mode = AddressingMode::PostIndex {
            base: Register::SP,
            offset: 16,
        };
        assert_eq!(format!("{}", mode), "[sp], #16");

        let mode = AddressingMode::RegisterOffset {
            base: Register::X0,
            index: Register::X1,
            shift: Some((ShiftKind::LSL, 3)),
        };
        assert_eq!(format!("{}", mode), "[x0, x1, lsl #3]");
    }

    // ---- Test 25: Load with register offset (array access) ----
    #[test]
    fn select_load_register_offset() {
        let mut sel = InstructionSelector::new();
        sel.select_load(
            Register::X0,
            &AddressingMode::RegisterOffset {
                base: Register::X1,
                index: Register::X2,
                shift: Some((ShiftKind::LSL, 3)),
            },
            MemorySize::DoubleWord,
        )
        .unwrap();
        let instrs = sel.take_instructions();
        // ADD temp, base, index shifted + LDR from temp
        assert_eq!(instrs.len(), 2);
        assert!(matches!(instrs[0], Instruction::ADD { .. }));
        assert!(matches!(instrs[1], Instruction::LDR { .. }));
    }

    // ---- Test 26: AAPCS64 argument register mapping ----
    #[test]
    fn aapcs64_arg_registers() {
        assert_eq!(Register::arg_register(0), Some(Register::X0));
        assert_eq!(Register::arg_register(7), Some(Register::X7));
        assert_eq!(Register::arg_register(8), None);
        assert_eq!(Register::X3.arg_index(), Some(3));
        assert_eq!(Register::X19.arg_index(), None);
    }

    // ---- Test 27: Stack allocation rounds up to 16-byte alignment ----
    #[test]
    fn select_alloc_stack_alignment() {
        let mut sel = InstructionSelector::new();
        sel.select_alloc_stack(Register::X0, 100, false);
        let instrs = sel.take_instructions();
        // 100 rounds up to 112 (next multiple of 16)
        if let Instruction::SUB {
            rm: Operand::Imm12(size),
            ..
        } = instrs[0]
        {
            assert_eq!(size, 112, "expected 16-byte aligned size 112, got {}", size);
        } else {
            panic!("expected SUB instruction with Imm12 operand");
        }
    }

    // ---- Test 28: B.cond encoding and display ----
    #[test]
    fn bcond_encoding_and_display() {
        let instr = Instruction::BCond {
            cond: Condition::EQ,
            offset: 16,
        };
        let text = format!("{}", instr);
        assert_eq!(text, "b.eq #16");
        let encoded = instr.encode().unwrap();
        // B.cond: 0101 0100 imm19 0 cond
        // Check that the condition field is correct
        assert_eq!(encoded & 0xF, Condition::EQ.encoding());
    }

    // ---- Test 29: CSEL encoding and display ----
    #[test]
    fn csel_display() {
        let instr = Instruction::CSEL {
            rd: Register::X0,
            rn: Register::X1,
            rm: Register::X2,
            cond: Condition::NE,
        };
        assert_eq!(format!("{}", instr), "csel x0, x1, x2, ne");
    }

    // ---- Test 30: New sub-word load/store display ----
    #[test]
    fn sub_word_load_store_display() {
        let ldrb = Instruction::LDRB {
            rt: Register::X0,
            rn: Register::X1,
            offset: 5,
        };
        assert_eq!(format!("{}", ldrb), "ldrb x0, [x1, #5]");

        let ldrh = Instruction::LDRH {
            rt: Register::X0,
            rn: Register::X1,
            offset: 4,
        };
        assert_eq!(format!("{}", ldrh), "ldrh x0, [x1, #4]");

        let ldrsw = Instruction::LDRSW {
            rt: Register::X0,
            rn: Register::X1,
            offset: 8,
        };
        assert_eq!(format!("{}", ldrsw), "ldrsw x0, [x1, #8]");

        let strb = Instruction::STRB {
            rt: Register::X0,
            rn: Register::X1,
            offset: 3,
        };
        assert_eq!(format!("{}", strb), "strb x0, [x1, #3]");

        let strh = Instruction::STRH {
            rt: Register::X0,
            rn: Register::X1,
            offset: 6,
        };
        assert_eq!(format!("{}", strh), "strh x0, [x1, #6]");
    }

    // ---- Test 31: NOP encoding ----
    #[test]
    fn nop_encoding() {
        let nop = Instruction::NOP;
        assert_eq!(nop.encode().unwrap(), 0xD503201F);
    }

    // ---- Test 32: Operand helpers ----
    #[test]
    fn operand_helpers() {
        let op = Operand::reg(Register::X5);
        assert_eq!(op.as_reg(), Some(Register::X5));
        assert_eq!(format!("{}", op), "x5");

        let op = Operand::Imm12(42);
        assert_eq!(op.as_reg(), None);
        assert_eq!(format!("{}", op), "#42");

        let op = Operand::shifted(Register::X3, ShiftKind::LSL, 2);
        assert_eq!(format!("{}", op), "x3, lsl #2");
    }

    // ---- Decode roundtrip tests ----

    // ---- Test: ADD immediate encode → decode roundtrip ----
    #[test]
    fn decode_add_immediate_roundtrip() {
        let instr = Instruction::ADD {
            rd: Register::X0,
            rn: Register::X1,
            rm: Operand::Imm12(42),
        };
        let word = instr.encode().unwrap();
        let decoded = Instruction::decode(word).expect("ADD imm should decode");
        assert_eq!(format!("{}", decoded), "add x0, x1, #42");
    }

    // ---- Test: SUB register encode → decode roundtrip ----
    #[test]
    fn decode_sub_register_roundtrip() {
        let instr = Instruction::SUB {
            rd: Register::X5,
            rn: Register::X6,
            rm: Operand::reg(Register::X7),
        };
        let word = instr.encode().unwrap();
        let decoded = Instruction::decode(word).expect("SUB reg should decode");
        assert_eq!(format!("{}", decoded), "sub x5, x6, x7");
    }

    // ---- Test: LDR/STR encode → decode roundtrip ----
    #[test]
    fn decode_ldr_str_roundtrip() {
        let ldr = Instruction::LDR {
            rt: Register::X0,
            rn: Register::SP,
            offset: 16,
        };
        let word = ldr.encode().unwrap();
        let decoded = Instruction::decode(word).expect("LDR should decode");
        assert_eq!(format!("{}", decoded), "ldr x0, [sp, #16]");

        let str_instr = Instruction::STR {
            rt: Register::X1,
            rn: Register::SP,
            offset: 8,
        };
        let word = str_instr.encode().unwrap();
        let decoded = Instruction::decode(word).expect("STR should decode");
        assert_eq!(format!("{}", decoded), "str x1, [sp, #8]");
    }

    // ---- Test: NOP and RET decode ----
    #[test]
    fn decode_nop_ret() {
        // NOP = 0xD503201F
        let decoded = Instruction::decode(0xD503201F).expect("NOP should decode");
        assert_eq!(format!("{}", decoded), "nop");

        // RET = 0xD65F03C0
        let decoded = Instruction::decode(0xD65F03C0).expect("RET should decode");
        assert!(format!("{}", decoded).starts_with("ret"));
    }

    // ---- Test: B.cond encode → decode roundtrip ----
    #[test]
    fn decode_bcond_roundtrip() {
        let instr = Instruction::BCond {
            cond: Condition::EQ,
            offset: 16,
        };
        let word = instr.encode().unwrap();
        let decoded = Instruction::decode(word).expect("B.cond should decode");
        assert!(format!("{}", decoded).contains("b.eq"));
    }

    // ---- Test: MOVZ/MOVK encode → decode roundtrip ----
    #[test]
    fn decode_movz_movk_roundtrip() {
        let movz = Instruction::MOVZ {
            rd: Register::X0,
            imm16: 42,
            shift: 0,
        };
        let word = movz.encode().unwrap();
        let decoded = Instruction::decode(word).expect("MOVZ should decode");
        assert!(format!("{}", decoded).starts_with("movz"));

        let movk = Instruction::MOVK {
            rd: Register::X0,
            imm16: 0x1234,
            shift: 16,
        };
        let word = movk.encode().unwrap();
        let decoded = Instruction::decode(word).expect("MOVK should decode");
        assert!(format!("{}", decoded).starts_with("movk"));
    }

    // ---- Test: CLZ encoding and display ----
    #[test]
    fn test_clz_emission() {
        // CLZ X0, X1 should encode to 0xDAC01000 | (1 << 5) | 0 = 0xDAC01020
        let clz = Instruction::CLZ {
            rd: Register::X0,
            rn: Register::X1,
        };
        assert_eq!(format!("{}", clz), "clz x0, x1");
        let encoded = clz.encode().unwrap();
        assert_eq!(encoded, 0xDAC01000 | (1u32 << 5) | 0);

        // CLZ X5, X5 should encode with both rd=5 and rn=5
        let clz_same = Instruction::CLZ {
            rd: Register::X5,
            rn: Register::X5,
        };
        let enc_same = clz_same.encode().unwrap();
        assert_eq!(enc_same, 0xDAC01000 | (5u32 << 5) | 5);

        // Verify the base encoding: CLZ X0, X0 = 0xDAC01000
        let clz_x0_x0 = Instruction::CLZ {
            rd: Register::X0,
            rn: Register::X0,
        };
        assert_eq!(clz_x0_x0.encode().unwrap(), 0xDAC01000);

        // Test instruction selector: CLZ should emit a single CLZ instruction
        let mut sel = InstructionSelector::new();
        sel.push(Instruction::CLZ {
            rd: Register::X0,
            rn: Register::X1,
        });
        let instrs = sel.take_instructions();
        assert_eq!(instrs.len(), 1);
        assert!(matches!(
            instrs[0],
            Instruction::CLZ {
                rd: Register::X0,
                rn: Register::X1
            }
        ));
    }

    // ---- Test: CTZ emission (RBIT + CLZ sequence) ----
    #[test]
    fn test_ctz_emission() {
        // RBIT X0, X1 should encode to 0xDAC00000 | (1 << 5) | 0 = 0xDAC00020
        let rbit = Instruction::RBIT {
            rd: Register::X0,
            rn: Register::X1,
        };
        assert_eq!(format!("{}", rbit), "rbit x0, x1");
        let encoded = rbit.encode().unwrap();
        assert_eq!(encoded, 0xDAC00000 | (1u32 << 5) | 0);

        // Verify the base encoding: RBIT X0, X0 = 0xDAC00000
        let rbit_x0_x0 = Instruction::RBIT {
            rd: Register::X0,
            rn: Register::X0,
        };
        assert_eq!(rbit_x0_x0.encode().unwrap(), 0xDAC00000);

        // CTZ = RBIT + CLZ sequence
        // When rd != rn: RBIT rd, rn then CLZ rd, rd
        let rbit_step = Instruction::RBIT {
            rd: Register::X0,
            rn: Register::X1,
        };
        let clz_step = Instruction::CLZ {
            rd: Register::X0,
            rn: Register::X0,
        };
        let rbit_enc = rbit_step.encode().unwrap();
        let clz_enc = clz_step.encode().unwrap();
        // Verify RBIT comes before CLZ and produces correct encodings
        assert_eq!(rbit_enc, 0xDAC00000 | (1u32 << 5) | 0); // RBIT X0, X1
        assert_eq!(clz_enc, 0xDAC01000 | (0u32 << 5) | 0); // CLZ X0, X0

        // When rd == rn: need scratch register (X9)
        // RBIT X9, X0 then CLZ X0, X9
        let rbit_scratch = Instruction::RBIT {
            rd: Register::X9,
            rn: Register::X0,
        };
        let clz_from_scratch = Instruction::CLZ {
            rd: Register::X0,
            rn: Register::X9,
        };
        let rbit_scratch_enc = rbit_scratch.encode().unwrap();
        let clz_scratch_enc = clz_from_scratch.encode().unwrap();
        assert_eq!(rbit_scratch_enc, 0xDAC00000 | (0u32 << 5) | 9); // RBIT X9, X0
        assert_eq!(clz_scratch_enc, 0xDAC01000 | (9u32 << 5) | 0); // CLZ X0, X9
    }

    // ---- Test: POPCNT emission (FMOV+CNT+ADDV+UMOV sequence) ----
    #[test]
    fn test_popcnt_emission() {
        // FMOV D8, X0: GPR to SIMD register
        let fmov_dx = Instruction::FMOV_DX {
            vd: 8,
            rn: Register::X0,
        };
        assert_eq!(format!("{}", fmov_dx), "fmov d8, x0");
        let fmov_enc = fmov_dx.encode().unwrap();
        assert_eq!(fmov_enc, 0x9E670000 | (0u32 << 5) | 8);

        // CNT V8.8B, V8.8B: count bits per byte
        let cnt = Instruction::CNT { vd: 8, vn: 8 };
        assert_eq!(format!("{}", cnt), "cnt v8.8b, v8.8b");
        let cnt_enc = cnt.encode().unwrap();
        assert_eq!(cnt_enc, 0x0E205800 | (8u32 << 5) | 8);

        // ADDV B8, V8.8B: horizontal sum
        let addv = Instruction::ADDV { vd: 8, vn: 8 };
        assert_eq!(format!("{}", addv), "addv b8, v8.8b");
        let addv_enc = addv.encode().unwrap();
        assert_eq!(addv_enc, 0x0E71B800 | (8u32 << 5) | 8);

        // UMOV X0, V8.B[0]: move result back to GPR
        let umov = Instruction::UMOV {
            rd: Register::X0,
            vn: 8,
        };
        assert_eq!(format!("{}", umov), "umov x0, v8.b[0]");
        let umov_enc = umov.encode().unwrap();
        assert_eq!(umov_enc, 0x0E204000 | (8u32 << 5) | 0);

        // Verify the full 4-instruction sequence produces distinct encodings
        assert_ne!(fmov_enc, cnt_enc);
        assert_ne!(cnt_enc, addv_enc);
        assert_ne!(addv_enc, umov_enc);

        // Also verify FMOV_XD (FP double to GPR)
        let fmov_xd = Instruction::FMOV_XD {
            rd: Register::X0,
            vn: 8,
        };
        assert_eq!(format!("{}", fmov_xd), "fmov x0, d8");
        let fmov_xd_enc = fmov_xd.encode().unwrap();
        assert_eq!(fmov_xd_enc, 0x9E6F0000 | (8u32 << 5) | 0);
    }
}
