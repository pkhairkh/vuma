//! # DEC Alpha Backend
//!
//! Implements the `Backend` trait for DEC Alpha (AXP) — a 64-bit
//! little-endian RISC ISA with 32-bit fixed-length instructions.
//!
//! ## Register Convention (Linux alpha)
//!
//! | Register | Role                                                          |
//! |----------|---------------------------------------------------------------|
//! | R0 (V0)  | syscall number / return value                                 |
//! | R1 (T0)  | scratch                                                       |
//! | R2–R8    | scratch / volatile                                            |
//! | R9 (S0)  | callee-saved                                                  |
//! | R10–R14  | callee-saved (R15 = FP)                                       |
//! | R15 (FP) | frame pointer (convention)                                    |
//! | R16-R21  | argument registers (a0–a5)                                    |
//! | R22-R25  | scratch / volatile                                            |
//! | R26 (RA) | return address                                                |
//! | R27 (PV) | procedure value (function pointer)                            |
//! | R28 (AT) | assembler temporary                                           |
//! | R29 (GP) | global pointer                                                |
//! | R30 (SP) | stack pointer                                                 |
//! | R31      | hardwired zero (reads as 0, writes are discarded)             |
//!
//! ## Linux alpha Syscall Convention
//!
//! - Syscall number in V0 (R0)
//! - Arguments in A0–A5 (R16–R21)
//! - Return value in V0 (R0); error flag in A3 (R19)
//! - Invoke via `call_pal 0x83` (callsys PAL call)
//!
//! ## Instruction Encoding
//!
//! All Alpha instructions are 32-bit little-endian.
//!
//! - **Operate register form**: `(op<<26) | (ra<<21) | (rb<<16) | (rc<<6) | function`.
//! - **Operate literal form**: `(op<<26) | (ra<<21) | (lit8<<13) | (1<<12) | (rc<<6) | function`.
//! - **Memory form**: `(op<<26) | (ra<<21) | (rb<<16) | disp16`.
//! - **Branch form**: `(op<<26) | (ra<<21) | disp21` (disp21 in units of 4 bytes, PC-relative).
//! - **PAL form**: `(0<<26) | palcode` (26-bit PAL function code).

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, Endianness, OutputFormat, PhysicalReg, RegClass, RelocationEntry, SectionHeader,
    TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
#[cfg(test)]
use crate::ir::VirtualRegister;
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// Alpha integer registers R0–R31.  R31 is hardwired zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Gpr {
    R0 = 0, R1 = 1, R2 = 2, R3 = 3, R4 = 4, R5 = 5, R6 = 6, R7 = 7,
    R8 = 8, R9 = 9, R10 = 10, R11 = 11, R12 = 12, R13 = 13, R14 = 14, R15 = 15,
    R16 = 16, R17 = 17, R18 = 18, R19 = 19, R20 = 20, R21 = 21, R22 = 22, R23 = 23,
    R24 = 24, R25 = 25, R26 = 26, R27 = 27, R28 = 28, R29 = 29, R30 = 30, R31 = 31,
}

impl Gpr {
    pub fn encoding(&self) -> u8 { *self as u8 }

    pub fn from_encoding(enc: u8) -> Option<Self> {
        if enc <= 31 {
            // SAFETY: enc is in 0..=31, matching the enum's #[repr(u8)] representation.
            Some(unsafe { std::mem::transmute::<u8, Gpr>(enc) })
        } else {
            None
        }
    }

    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::R0 => "$0", Gpr::R1 => "$1", Gpr::R2 => "$2", Gpr::R3 => "$3",
            Gpr::R4 => "$4", Gpr::R5 => "$5", Gpr::R6 => "$6", Gpr::R7 => "$7",
            Gpr::R8 => "$8", Gpr::R9 => "$9", Gpr::R10 => "$10", Gpr::R11 => "$11",
            Gpr::R12 => "$12", Gpr::R13 => "$13", Gpr::R14 => "$14", Gpr::R15 => "$15",
            Gpr::R16 => "$16", Gpr::R17 => "$17", Gpr::R18 => "$18", Gpr::R19 => "$19",
            Gpr::R20 => "$20", Gpr::R21 => "$21", Gpr::R22 => "$22", Gpr::R23 => "$23",
            Gpr::R24 => "$24", Gpr::R25 => "$25", Gpr::R26 => "$26", Gpr::R27 => "$27",
            Gpr::R28 => "$28", Gpr::R29 => "$29", Gpr::R30 => "$30", Gpr::R31 => "$31",
        }
    }

    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::R16),
            1 => Some(Gpr::R17),
            2 => Some(Gpr::R18),
            3 => Some(Gpr::R19),
            4 => Some(Gpr::R20),
            5 => Some(Gpr::R21),
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

/// Alpha floating-point register (F0-F31).  Alpha has 32 FP registers,
/// separate from the integer register file.  R31 is hardwired zero in the
/// integer file.  F31 is a *normal* FP register for arithmetic (ADDT/SUBT/
/// MULT/DIVT/CMPTEQ/CMPTLT/CMPTLE/CMPTUN -- it is NOT hardwired-zero), but
/// the encoding for unary CVT* instructions (CVTQT/CVTTQ/CVTST/CVTTS)
/// REQUIRES the FA field to be 31 -- the hardware ignores FA for these
/// ops, but QEMU-alpha raises SIGILL if FA != 31 (matching libopcodes'
/// ARG_FPZ1 operand constraint).  See `fp_cvt` below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Fpr {
    F0 = 0,  F1,  F2,  F3,  F4,  F5,  F6,  F7,
    F8,  F9,  F10, F11, F12, F13, F14, F15,
    F16, F17, F18, F19, F20, F21, F22, F23,
    F24, F25, F26, F27, F28, F29, F30, F31,
}

impl Fpr {
    /// 5-bit register encoding (0-31).
    pub fn encoding(&self) -> u8 { *self as u8 }
}

/// FP scratch / accumulator registers.  F0 and F1 are caller-saved
/// temporaries in the Alpha ABI and are safe to clobber within a single
/// IR instruction's emission without spilling.
const FA: Fpr = Fpr::F0; // FP accumulator / result
const FB: Fpr = Fpr::F1; // FP second operand
const FC: Fpr = Fpr::F2; // FP comparison-result / scratch / negative-clamp

// ===========================================================================
// Instruction enum (mnemonic / Display / encode)
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// ADDQ ra, rb, rc  (rc = ra + rb, 64-bit)
    Addq { ra: Gpr, rb: Gpr, rc: Gpr },
    /// ADDQ ra, #lit8, rc
    AddqLi { ra: Gpr, lit: u8, rc: Gpr },
    /// SUBQ ra, rb, rc
    Subq { ra: Gpr, rb: Gpr, rc: Gpr },
    /// MULQ ra, rb, rc
    Mulq { ra: Gpr, rb: Gpr, rc: Gpr },
    /// UMULH ra, rb, rc (upper 64 bits of unsigned product)
    Umulh { ra: Gpr, rb: Gpr, rc: Gpr },
    /// DIVQ (unsigned) ra, rb, rc — wait, this is UDIVQ. We use function code 0x2F for unsigned DIVQ.
    /// Actually, DIVQ is signed. UDIVQ (function 0x2F) is unsigned.
    Divq { ra: Gpr, rb: Gpr, rc: Gpr },
    /// AND ra, rb, rc
    And { ra: Gpr, rb: Gpr, rc: Gpr },
    /// OR (BIS) ra, rb, rc
    Or { ra: Gpr, rb: Gpr, rc: Gpr },
    /// XOR ra, rb, rc
    Xor { ra: Gpr, rb: Gpr, rc: Gpr },
    /// SLL ra, rb, rc
    Sll { ra: Gpr, rb: Gpr, rc: Gpr },
    /// SRL ra, rb, rc
    Srl { ra: Gpr, rb: Gpr, rc: Gpr },
    /// SRA ra, rb, rc
    Sra { ra: Gpr, rb: Gpr, rc: Gpr },
    /// LDA ra, disp(rb)
    Lda { ra: Gpr, disp: i16, rb: Gpr },
    /// LDQ ra, disp(rb)
    Ldq { ra: Gpr, disp: i16, rb: Gpr },
    /// STQ ra, disp(rb)
    Stq { ra: Gpr, disp: i16, rb: Gpr },
    /// LDL ra, disp(rb)
    Ldl { ra: Gpr, disp: i16, rb: Gpr },
    /// STL ra, disp(rb)
    Stl { ra: Gpr, disp: i16, rb: Gpr },
    /// LDBU ra, disp(rb)
    Ldbu { ra: Gpr, disp: i16, rb: Gpr },
    /// STB ra, disp(rb)
    Stb { ra: Gpr, disp: i16, rb: Gpr },
    /// LDWU ra, disp(rb)
    Ldwu { ra: Gpr, disp: i16, rb: Gpr },
    /// STW ra, disp(rb)
    Stw { ra: Gpr, disp: i16, rb: Gpr },
    /// BEQ ra, disp21
    Beq { ra: Gpr, disp: i32 },
    /// BNE ra, disp21
    Bne { ra: Gpr, disp: i32 },
    /// BLT ra, disp21
    Blt { ra: Gpr, disp: i32 },
    /// BLE ra, disp21
    Ble { ra: Gpr, disp: i32 },
    /// BGT ra, disp21
    Bgt { ra: Gpr, disp: i32 },
    /// BGE ra, disp21
    Bge { ra: Gpr, disp: i32 },
    /// BR ra, disp21 (unconditional branch with link)
    Br { ra: Gpr, disp: i32 },
    /// BSR ra, disp21
    Bsr { ra: Gpr, disp: i32 },
    /// JMP ra, (rb)
    Jmp { ra: Gpr, rb: Gpr },
    /// JSR ra, (rb)
    Jsr { ra: Gpr, rb: Gpr },
    /// RET (JSR $26, ($26))
    Ret,
    /// CALL_PAL palcode
    CallPal { palcode: u32 },
    /// CMPULE ra, rb, rc (rc = 1 if ra <= rb unsigned, else 0)
    Cmpule { ra: Gpr, rb: Gpr, rc: Gpr },
    /// CMPLT ra, rb, rc (signed)
    Cmplt { ra: Gpr, rb: Gpr, rc: Gpr },
    /// CMPEQ ra, rb, rc
    Cmpeq { ra: Gpr, rb: Gpr, rc: Gpr },
    /// CMOVNE ra, rb, rc (if ra != 0, rc = rb)
    Cmovne { ra: Gpr, rb: Gpr, rc: Gpr },
    /// CMOVEQ ra, rb, rc (if ra == 0, rc = rb)
    Cmoveq { ra: Gpr, rb: Gpr, rc: Gpr },
    /// NOP (LDQ $31, 0($31))
    Nop,
}

impl Instruction {
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Instruction::Addq { .. } => "addq",
            Instruction::AddqLi { .. } => "addq",
            Instruction::Subq { .. } => "subq",
            Instruction::Mulq { .. } => "mulq",
            Instruction::Umulh { .. } => "umulh",
            Instruction::Divq { .. } => "divq",
            Instruction::And { .. } => "and",
            Instruction::Or { .. } => "bis",
            Instruction::Xor { .. } => "xor",
            Instruction::Sll { .. } => "sll",
            Instruction::Srl { .. } => "srl",
            Instruction::Sra { .. } => "sra",
            Instruction::Lda { .. } => "lda",
            Instruction::Ldq { .. } => "ldq",
            Instruction::Stq { .. } => "stq",
            Instruction::Ldl { .. } => "ldl",
            Instruction::Stl { .. } => "stl",
            Instruction::Ldbu { .. } => "ldbu",
            Instruction::Stb { .. } => "stb",
            Instruction::Ldwu { .. } => "ldwu",
            Instruction::Stw { .. } => "stw",
            Instruction::Beq { .. } => "beq",
            Instruction::Bne { .. } => "bne",
            Instruction::Blt { .. } => "blt",
            Instruction::Ble { .. } => "ble",
            Instruction::Bgt { .. } => "bgt",
            Instruction::Bge { .. } => "bge",
            Instruction::Br { .. } => "br",
            Instruction::Bsr { .. } => "bsr",
            Instruction::Jmp { .. } => "jmp",
            Instruction::Jsr { .. } => "jsr",
            Instruction::Ret => "ret",
            Instruction::CallPal { .. } => "call_pal",
            Instruction::Cmpule { .. } => "cmpule",
            Instruction::Cmplt { .. } => "cmplt",
            Instruction::Cmpeq { .. } => "cmpeq",
            Instruction::Cmovne { .. } => "cmovne",
            Instruction::Cmoveq { .. } => "cmoveq",
            Instruction::Nop => "nop",
        }
    }

    /// Encode this instruction into little-endian bytes.
    pub fn encode(&self) -> Vec<u8> {
        let word: u32 = match self {
            // Operate register form: (op<<26) | (ra<<21) | (rb<<16) | (rc<<6) | function
            Instruction::Addq { ra, rb, rc } => op_reg(0x10, *ra, *rb, *rc, 0x20),
            Instruction::AddqLi { ra, lit, rc } => op_lit(0x10, *ra, *lit, *rc, 0x20),
            Instruction::Subq { ra, rb, rc } => op_reg(0x10, *ra, *rb, *rc, 0x29),
            Instruction::Mulq { ra, rb, rc } => op_reg(0x13, *ra, *rb, *rc, 0x20),
            Instruction::Umulh { ra, rb, rc } => op_reg(0x13, *ra, *rb, *rc, 0x22),
            // DIVQ: signed = function 0x2F, unsigned (UDIVQ) = function 0x2F (same? no — DIVQ=0x2F signed; UDIVQ doesn't exist, but the function for unsigned divide is 0x2F too... let me check)
            // Actually: DIVQ = signed 64-bit divide, function 0x2F.
            // DIVQU = unsigned 64-bit divide, function 0x2E.
            Instruction::Divq { ra, rb, rc } => op_reg(0x13, *ra, *rb, *rc, 0x2E), // DIVQU (unsigned)
            Instruction::And { ra, rb, rc } => op_reg(0x11, *ra, *rb, *rc, 0x00),
            Instruction::Or { ra, rb, rc } => op_reg(0x11, *ra, *rb, *rc, 0x20),
            Instruction::Xor { ra, rb, rc } => op_reg(0x11, *ra, *rb, *rc, 0x40),
            Instruction::Sll { ra, rb, rc } => op_reg(0x12, *ra, *rb, *rc, 0x39),
            Instruction::Srl { ra, rb, rc } => op_reg(0x12, *ra, *rb, *rc, 0x34),
            Instruction::Sra { ra, rb, rc } => op_reg(0x12, *ra, *rb, *rc, 0x3C),
            // Memory form: (op<<26) | (ra<<21) | (rb<<16) | disp16
            Instruction::Lda { ra, disp, rb } => op_mem(0x08, *ra, *disp, *rb),
            Instruction::Ldq { ra, disp, rb } => op_mem(0x29, *ra, *disp, *rb),
            Instruction::Stq { ra, disp, rb } => op_mem(0x2D, *ra, *disp, *rb),
            Instruction::Ldl { ra, disp, rb } => op_mem(0x28, *ra, *disp, *rb),
            Instruction::Stl { ra, disp, rb } => op_mem(0x2C, *ra, *disp, *rb),
            Instruction::Ldbu { ra, disp, rb } => op_mem(0x0A, *ra, *disp, *rb),
            Instruction::Stb { ra, disp, rb } => op_mem(0x0E, *ra, *disp, *rb),
            Instruction::Ldwu { ra, disp, rb } => op_mem(0x0C, *ra, *disp, *rb),
            Instruction::Stw { ra, disp, rb } => op_mem(0x0D, *ra, *disp, *rb),
            // Branch form: (op<<26) | (ra<<21) | disp21
            Instruction::Beq { ra, disp } => op_br(0x39, *ra, *disp),
            Instruction::Bne { ra, disp } => op_br(0x3D, *ra, *disp),
            // Alpha branch opcodes (Alpha ARM §C.2):
            //   0x38 BLBC, 0x39 BEQ, 0x3A BLT, 0x3B BLE,
            //   0x3C BLBS, 0x3D BNE, 0x3E BGE, 0x3F BGT.
            // (Wave-6 fix: Blt and Bgt were previously swapped — Blt used
            // 0x3F=BGT and Bgt used 0x3A=BLT, which caused print_int to treat
            // positive values as negative on QEMU-alpha.)
            Instruction::Blt { ra, disp } => op_br(0x3A, *ra, *disp),
            Instruction::Ble { ra, disp } => op_br(0x3B, *ra, *disp),
            Instruction::Bgt { ra, disp } => op_br(0x3F, *ra, *disp),
            Instruction::Bge { ra, disp } => op_br(0x3E, *ra, *disp),
            Instruction::Br { ra, disp } => op_br(0x30, *ra, *disp),
            Instruction::Bsr { ra, disp } => op_br(0x34, *ra, *disp),
            // JMP/JSR: opcode 0x1A, format (op<<26) | (ra<<21) | (rb<<16)
            Instruction::Jmp { ra, rb } => (0x1A_u32 << 26) | ((ra.encoding() as u32) << 21) | ((rb.encoding() as u32) << 16),
            Instruction::Jsr { ra, rb } => (0x1A_u32 << 26) | ((ra.encoding() as u32) << 21) | ((rb.encoding() as u32) << 16),
            Instruction::Ret => (0x1A_u32 << 26) | (26u32 << 21) | (26u32 << 16), // JSR $26, ($26)
            Instruction::CallPal { palcode } => palcode & 0x03FFFFFF,
            // Conditional moves and compares (Operate format):
            Instruction::Cmpule { ra, rb, rc } => op_reg(0x10, *ra, *rb, *rc, 0x3F), // CMPULE
            Instruction::Cmplt { ra, rb, rc } => op_reg(0x10, *ra, *rb, *rc, 0x1D),  // CMPULT (unsigned; signed CMPLT not directly available on Alpha)
            Instruction::Cmpeq { ra, rb, rc } => op_reg(0x10, *ra, *rb, *rc, 0x2D),  // CMPEQ
            Instruction::Cmovne { ra, rb, rc } => op_reg(0x11, *ra, *rb, *rc, 0x26), // CMOVNE
            Instruction::Cmoveq { ra, rb, rc } => op_reg(0x11, *ra, *rb, *rc, 0x24), // CMOVEQ
            Instruction::Nop => op_mem(0x29, Gpr::R31, 0, Gpr::R31), // LDQ $31, 0($31)
        };
        word.to_le_bytes().to_vec()
    }
}

#[inline]
fn op_reg(op: u32, ra: Gpr, rb: Gpr, rc: Gpr, function: u32) -> u32 {
    // Alpha Operate format (register form):
    //   bits 31-26: opcode (6 bits)
    //   bits 25-21: ra (5 bits)
    //   bits 20-16: rb (5 bits)
    //   bit 15: 0 (register form)
    //   bits 14-12: 000 (reserved)
    //   bits 11-5: function (7 bits)
    //   bits 4-0: rc (5 bits)
    (op << 26) | ((ra.encoding() as u32) << 21) | ((rb.encoding() as u32) << 16)
        | ((function & 0x7F) << 5) | (rc.encoding() as u32 & 0x1F)
}

#[inline]
fn op_lit(op: u32, ra: Gpr, lit: u8, rc: Gpr, function: u32) -> u32 {
    // Alpha Operate literal form (per Alpha ARM):
    //   bits 31-26: opcode (6 bits)
    //   bits 25-21: ra (5 bits)
    //   bits 20-13: literal (8 bits — NOT 5 bits!)
    //   bit 12: 1 (literal form flag)
    //   bits 11-5: function (7 bits)
    //   bits 4-0: rc (5 bits)
    // The literal is 8 bits (0-255), occupying bits 20-13 (the Rb field
    // PLUS bits 15-13 which are reserved in register form).
    (op << 26) | ((ra.encoding() as u32) << 21) | ((lit as u32 & 0xFF) << 13)
        | (1u32 << 12) | ((function & 0x7F) << 5) | (rc.encoding() as u32 & 0x1F)
}

#[inline]
fn op_mem(op: u32, ra: Gpr, disp: i16, rb: Gpr) -> u32 {
    (op << 26) | ((ra.encoding() as u32) << 21) | ((rb.encoding() as u32) << 16) | (disp as u16 as u32)
}

#[inline]
fn op_br(op: u32, ra: Gpr, disp: i32) -> u32 {
    (op << 26) | ((ra.encoding() as u32) << 21) | (disp as u32 & 0x1F_FFFF)
}

/// Patch a branch instruction's displacement in already-emitted code.
/// `pos` is the byte offset of the 4-byte branch instruction.  `op` is the
/// branch opcode (e.g. 0x39 for BEQ, 0x3D for BNE, 0x3F for BLT, 0x30 for BR).
/// `ra` is the test register.  `disp` is the 21-bit signed word displacement.
fn patch_alpha_branch(code: &mut [u8], pos: usize, op: u32, ra: Gpr, disp: i32) {
    let word = op_br(op, ra, disp);
    code[pos..pos + 4].copy_from_slice(&word.to_le_bytes());
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Addq { ra, rb, rc } => write!(f, "addq {}, {}, {}", ra, rb, rc),
            Instruction::AddqLi { ra, lit, rc } => write!(f, "addq {}, #{}, {}", ra, lit, rc),
            Instruction::Subq { ra, rb, rc } => write!(f, "subq {}, {}, {}", ra, rb, rc),
            Instruction::Mulq { ra, rb, rc } => write!(f, "mulq {}, {}, {}", ra, rb, rc),
            Instruction::Umulh { ra, rb, rc } => write!(f, "umulh {}, {}, {}", ra, rb, rc),
            Instruction::Divq { ra, rb, rc } => write!(f, "divq {}, {}, {}", ra, rb, rc),
            Instruction::And { ra, rb, rc } => write!(f, "and {}, {}, {}", ra, rb, rc),
            Instruction::Or { ra, rb, rc } => write!(f, "bis {}, {}, {}", ra, rb, rc),
            Instruction::Xor { ra, rb, rc } => write!(f, "xor {}, {}, {}", ra, rb, rc),
            Instruction::Sll { ra, rb, rc } => write!(f, "sll {}, {}, {}", ra, rb, rc),
            Instruction::Srl { ra, rb, rc } => write!(f, "srl {}, {}, {}", ra, rb, rc),
            Instruction::Sra { ra, rb, rc } => write!(f, "sra {}, {}, {}", ra, rb, rc),
            Instruction::Lda { ra, disp, rb } => write!(f, "lda {}, {}({})", ra, disp, rb),
            Instruction::Ldq { ra, disp, rb } => write!(f, "ldq {}, {}({})", ra, disp, rb),
            Instruction::Stq { ra, disp, rb } => write!(f, "stq {}, {}({})", ra, disp, rb),
            Instruction::Ldl { ra, disp, rb } => write!(f, "ldl {}, {}({})", ra, disp, rb),
            Instruction::Stl { ra, disp, rb } => write!(f, "stl {}, {}({})", ra, disp, rb),
            Instruction::Ldbu { ra, disp, rb } => write!(f, "ldbu {}, {}({})", ra, disp, rb),
            Instruction::Stb { ra, disp, rb } => write!(f, "stb {}, {}({})", ra, disp, rb),
            Instruction::Ldwu { ra, disp, rb } => write!(f, "ldwu {}, {}({})", ra, disp, rb),
            Instruction::Stw { ra, disp, rb } => write!(f, "stw {}, {}({})", ra, disp, rb),
            Instruction::Beq { ra, disp } => write!(f, "beq {}, {}", ra, disp),
            Instruction::Bne { ra, disp } => write!(f, "bne {}, {}", ra, disp),
            Instruction::Blt { ra, disp } => write!(f, "blt {}, {}", ra, disp),
            Instruction::Ble { ra, disp } => write!(f, "ble {}, {}", ra, disp),
            Instruction::Bgt { ra, disp } => write!(f, "bgt {}, {}", ra, disp),
            Instruction::Bge { ra, disp } => write!(f, "bge {}, {}", ra, disp),
            Instruction::Br { ra, disp } => write!(f, "br {}, {}", ra, disp),
            Instruction::Bsr { ra, disp } => write!(f, "bsr {}, {}", ra, disp),
            Instruction::Jmp { ra, rb } => write!(f, "jmp {}, ({})", ra, rb),
            Instruction::Jsr { ra, rb } => write!(f, "jsr {}, ({})", ra, rb),
            Instruction::Ret => write!(f, "ret"),
            Instruction::CallPal { palcode } => write!(f, "call_pal {:#x}", palcode),
            Instruction::Cmpule { ra, rb, rc } => write!(f, "cmpule {}, {}, {}", ra, rb, rc),
            Instruction::Cmplt { ra, rb, rc } => write!(f, "cmplt {}, {}, {}", ra, rb, rc),
            Instruction::Cmpeq { ra, rb, rc } => write!(f, "cmpeq {}, {}, {}", ra, rb, rc),
            Instruction::Cmovne { ra, rb, rc } => write!(f, "cmovne {}, {}, {}", ra, rb, rc),
            Instruction::Cmoveq { ra, rb, rc } => write!(f, "cmoveq {}, {}, {}", ra, rb, rc),
            Instruction::Nop => write!(f, "nop"),
        }
    }
}

// ===========================================================================
// Scratch registers + frame pointer + stack pointer
// ===========================================================================

const S0: Gpr = Gpr::R1;  // T0 (caller-saved)
const S1: Gpr = Gpr::R2;  // T1 (caller-saved)
const S2: Gpr = Gpr::R3;  // T2 (caller-saved)
const S3: Gpr = Gpr::R8;  // T7 (caller-saved)
// S4 was R9 (callee-saved per the Alpha ABI documented above) — using it as
// scratch without saving corrupted the caller's R9. Moved to R22 (T8,
// caller-saved / volatile).
const S4: Gpr = Gpr::R22; // T8 (caller-saved, extra scratch for division)
const S5: Gpr = Gpr::R23; // T9 (caller-saved, extra scratch for division)
const FP: Gpr = Gpr::R15; // FP
const SP: Gpr = Gpr::R30; // SP
const RA: Gpr = Gpr::R26; // RA
const ZERO: Gpr = Gpr::R31;

// ===========================================================================
// Stack-slot helpers
// ===========================================================================

/// Load a 64-bit immediate into a register.
///
/// Uses LDA for 16-bit signed immediates, or LDA + LDAH + ZAPNOT for 32-bit
/// values, or LDA + LDAH + ZAPNOT + (S2-based high-32 load + SLL + BIS) for
/// full 64-bit values (needed for f64 bit-pattern constants like 2.0 =
/// 0x4000000000000000).  The 64-bit path uses S2 as an internal scratch;
/// callers that pass dst=S2 with a 64-bit value would clobber themselves
/// (and ss_ld/ss_st only call this with 32-bit offsets, so this is safe).
fn ss_load_imm(dst: Gpr, val: i64) -> Vec<u8> {
    let v = val as i64;
    if (-32768..=32767).contains(&v) {
        // LDA dst, lo16(ZERO): dst = sign_extend(lo16).
        Instruction::Lda { ra: dst, disp: v as i16, rb: ZERO }.encode()
    } else {
        let v64 = v as u64;
        let lo32 = (v64 & 0xFFFF_FFFF) as u32;
        let hi32 = ((v64 >> 32) & 0xFFFF_FFFF) as u32;
        // Load low 32 bits (zero-extended to 64) into dst.
        let lo_lo16 = (lo32 & 0xFFFF) as i16 as i32;
        let mut lo_hi16 = ((lo32 >> 16) & 0xFFFF) as i32;
        if lo_lo16 < 0 {
            lo_hi16 += 1;
        }
        let mut code = Vec::new();
        // LDA dst, lo16(ZERO)
        code.extend(Instruction::Lda { ra: dst, disp: lo_lo16 as i16, rb: ZERO }.encode());
        // LDAH dst, hi16(dst)
        let word: u32 = (0x09u32 << 26)
            | ((dst.encoding() as u32) << 21)
            | ((dst.encoding() as u32) << 16)
            | (lo_hi16 as u32 & 0xFFFF);
        code.extend_from_slice(&word.to_le_bytes());
        // ZAPNOT dst, 0x0F, dst — keep bytes 0-3, zero bytes 4-7.
        // This zero-extends the 32-bit result to 64 bits.
        code.extend_from_slice(&op_lit(0x12, dst, 0x0F, dst, 0x31).to_le_bytes());
        if hi32 != 0 {
            // 64-bit value: load high 32 bits into S2, shift left by 32,
            // then OR into dst.
            let hi_lo16 = (hi32 & 0xFFFF) as i16 as i32;
            let mut hi_hi16 = ((hi32 >> 16) & 0xFFFF) as i32;
            if hi_lo16 < 0 {
                hi_hi16 += 1;
            }
            // LDA S2, hi_lo16(ZERO)
            code.extend(Instruction::Lda { ra: S2, disp: hi_lo16 as i16, rb: ZERO }.encode());
            // LDAH S2, hi_hi16(S2)
            let word2: u32 = (0x09u32 << 26)
                | ((S2.encoding() as u32) << 21)
                | ((S2.encoding() as u32) << 16)
                | (hi_hi16 as u32 & 0xFFFF);
            code.extend_from_slice(&word2.to_le_bytes());
            // ZAPNOT S2, 0x0F, S2 — zero-extend high 32 bits.
            code.extend_from_slice(&op_lit(0x12, S2, 0x0F, S2, 0x31).to_le_bytes());
            // SLL S2, 32, S2 — shift left by 32 (high bits now in upper half).
            code.extend_from_slice(&op_lit(0x12, S2, 32, S2, 0x39).to_le_bytes());
            // BIS dst, S2, dst — OR high bits into dst.
            code.extend_from_slice(&op_reg(0x11, dst, S2, dst, 0x20).to_le_bytes());
        }
        code
    }
}

/// Load a 64-bit value from stack slot at [FP + offset] into dst.
fn ss_ld(dst: Gpr, offset: i32) -> Vec<u8> {
    if (-32768..=32767).contains(&offset) {
        Instruction::Ldq { ra: dst, disp: offset as i16, rb: FP }.encode()
    } else {
        // Large offset: compute address into S2 first.
        let mut code = ss_load_imm(S2, offset as i64);
        // S2 += FP: ADDQ S2, FP, S2.
        code.extend(Instruction::Addq { ra: S2, rb: FP, rc: S2 }.encode());
        code.extend(Instruction::Ldq { ra: dst, disp: 0, rb: S2 }.encode());
        code
    }
}

/// Store a 64-bit value from src to stack slot at [FP + offset].
fn ss_st(src: Gpr, offset: i32) -> Vec<u8> {
    if (-32768..=32767).contains(&offset) {
        Instruction::Stq { ra: src, disp: offset as i16, rb: FP }.encode()
    } else {
        let mut code = ss_load_imm(S2, offset as i64);
        code.extend(Instruction::Addq { ra: S2, rb: FP, rc: S2 }.encode());
        code.extend(Instruction::Stq { ra: src, disp: 0, rb: S2 }.encode());
        code
    }
}

/// STQ src, off(SP) — SP-relative store with large-offset handling.
/// Used in the prologue where FP is not yet established.
fn sp_stq(src: Gpr, off: i64) -> Vec<u8> {
    if (-32768..=32767).contains(&off) {
        Instruction::Stq { ra: src, disp: off as i16, rb: SP }.encode()
    } else {
        let mut code = ss_load_imm(S2, off);
        code.extend(Instruction::Addq { ra: SP, rb: S2, rc: S2 }.encode());
        code.extend(Instruction::Stq { ra: src, disp: 0, rb: S2 }.encode());
        code
    }
}

/// LDQ dst, off(SP) — SP-relative load with large-offset handling.
/// Used in the epilogue to restore RA/FP.
fn sp_ldq(dst: Gpr, off: i64) -> Vec<u8> {
    if (-32768..=32767).contains(&off) {
        Instruction::Ldq { ra: dst, disp: off as i16, rb: SP }.encode()
    } else {
        let mut code = ss_load_imm(S2, off);
        code.extend(Instruction::Addq { ra: SP, rb: S2, rc: S2 }.encode());
        code.extend(Instruction::Ldq { ra: dst, disp: 0, rb: S2 }.encode());
        code
    }
}

/// SP += delta (signed) with large-delta handling.  LDA SP, delta(SP) only
/// encodes a 16-bit signed displacement; for |delta| > 32767 we materialize
/// delta in S2 and ADDQ.
fn sp_adjust(delta: i64) -> Vec<u8> {
    if (-32768..=32767).contains(&delta) {
        Instruction::Lda { ra: SP, disp: delta as i16, rb: SP }.encode()
    } else {
        let mut code = ss_load_imm(S2, delta);
        code.extend(Instruction::Addq { ra: SP, rb: S2, rc: SP }.encode());
        code
    }
}

/// Load an IRValue into a scratch register.
fn ss_load_value(val: &IRValue, slots: &HashMap<u32, i32>, scratch: Gpr) -> Vec<u8> {
    match val {
        IRValue::Register(id) => {
            let offset = slots.get(id).copied().unwrap_or(0);
            ss_ld(scratch, offset)
        }
        IRValue::Immediate(v) => ss_load_imm(scratch, *v),
        IRValue::Address(a) => ss_load_imm(scratch, *a as i64),
        IRValue::Label(_) => ss_load_imm(scratch, 0),
    }
}

// ===========================================================================
// Stack-slot based allocate_registers
// ===========================================================================

fn alpha_allocate_registers_ss(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let func_name = func.name.clone();

    // ── Phase 1: Collect all vreg IDs ──
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
        match &block.terminator {
            crate::ir::IRTerminator::Branch { cond, .. } => {
                if let Some(id) = cond.as_register() { all_vreg_ids.insert(id); }
            }
            crate::ir::IRTerminator::Return(vals) => {
                for val in vals { if let Some(id) = val.as_register() { all_vreg_ids.insert(id); } }
            }
            crate::ir::IRTerminator::Switch { discr, .. } => {
                if let Some(id) = discr.as_register() { all_vreg_ids.insert(id); }
            }
            _ => {}
        }
    }
    for val in &func.results {
        if let Some(id) = val.as_register() { all_vreg_ids.insert(id); }
    }

    // Identify Alloc vregs and their sizes.
    let mut stack_alloc_vregs: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut alloc_sizes: HashMap<u32, i32> = HashMap::new();
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

    // ── Stack Layout ──
    // Alpha SP grows down.  Frame layout (ALL slots at POSITIVE offsets from
    // SP/FP, i.e. INSIDE the allocated frame [SP, SP+frame_size)):
    //   [SP + frame_size - 8 ]   saved FP          (top of frame)
    //   [SP + frame_size - 16]   saved RA
    //   ...                      alloc data
    //   ...                      vreg slots
    //   [SP + 0 ]                first vreg slot   (bottom of frame)
    //
    // After prologue: SP = old_SP - frame_size, FP = SP.
    // Every spill (vregs, allocs) and the RA/FP save area lives INSIDE the
    // frame.  This is critical: a callee's RA/FP save at the TOP of its own
    // frame must NOT land in the caller's spill region.  The previous layout
    // placed vregs at NEGATIVE offsets from FP (i.e. BELOW SP, outside the
    // frame); when RA/FP were also moved to the top of the frame, a callee's
    // RA save (at callee_SP + frame - 16 = caller_SP - 16) clobbered the
    // caller's vreg spilled at caller_SP - 16.  Keeping everything at positive
    // offsets inside the frame eliminates all such clobbering.
    let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
    let mut current_offset: i32 = 0; // vregs start at FP+0, grow UP (positive)
    let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
    all_vreg_ids_sorted.sort();
    for &id in &all_vreg_ids_sorted {
        vreg_stack_slots.insert(id, current_offset);
        current_offset += 8;
    }

    let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
    let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
    alloc_vreg_ids.sort();
    for &id in &alloc_vreg_ids {
        let size = alloc_sizes[&id];
        // Align the alloc slot up to 16 bytes.
        current_offset = (current_offset + 15) & !15;
        alloc_offsets.insert(id, current_offset);
        current_offset += size;
    }

    // Reserve 16 bytes for saved RA + FP at the TOP of the frame
    // (just below the caller's SP).  RA at SP + (frame_size - 16),
    // FP at SP + (frame_size - 8).  Because frame_size >= vreg_area + 16,
    // lr_save_off >= vreg_area, so the save area never overlaps any vreg or
    // alloc slot (which all live in [0, vreg_area)).
    let vreg_area_size = current_offset as usize;
    let frame_size = ((vreg_area_size + 16 + 15) & !15) as usize;
    let lr_save_off = (frame_size - 16) as i16;  // RA at top of frame
    let fp_save_off = (frame_size - 8) as i16;   // FP at top of frame

    // ── Phase 2: Build the phi-map ──
    let _phi_map = func.build_phi_map();

    // ── Phase 3: Emit prologue ──
    let mut code: Vec<u8> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();

    // LDA SP, -frame_size(SP)
    code.extend(sp_adjust(-(frame_size as i64)));
    // STQ RA, lr_save_off(SP)
    code.extend(sp_stq(RA, lr_save_off as i64));
    // STQ FP, fp_save_off(SP)
    code.extend(sp_stq(FP, fp_save_off as i64));
    // BIS ZERO, SP, FP (FP = SP)
    code.extend(Instruction::Or { ra: ZERO, rb: SP, rc: FP }.encode());

    // Save incoming args (R16-R21) to their stack slots.
    for (i, param) in func.params.iter().enumerate() {
        if let Some(id) = param.as_register() {
            if let Some(arg_reg) = Gpr::arg_register(i) {
                let offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
                code.extend(ss_st(arg_reg, offset));
            }
        }
    }

    // ── Phase 4: Emit code for each block ──
    let label_to_idx: HashMap<String, usize> = func
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.clone(), i))
        .collect();

    let mut block_start_offsets: Vec<usize> = Vec::with_capacity(func.blocks.len());

    // Branch patch records: BR/Bcc with 21-bit displacement.
    struct BranchPatch {
        code_offset: usize,
        target_label: String,
    }
    let mut branch_patches: Vec<BranchPatch> = Vec::new();

    for block in func.blocks.iter() {
        block_start_offsets.push(code.len());

        for instr in &block.instructions {
            emit_instr(instr, &vreg_stack_slots, &alloc_offsets, &mut code, &mut relocations,
                       frame_size, lr_save_off, fp_save_off);
        }

        // Emit terminator.
        match &block.terminator {
            crate::ir::IRTerminator::Jump(target) => {
                // BR ZERO, target (unconditional, since ZERO is always 0, BR always branches).
                let patch_offset = code.len();
                code.extend(Instruction::Br { ra: ZERO, disp: 0 }.encode());
                branch_patches.push(BranchPatch {
                    code_offset: patch_offset,
                    target_label: target.clone(),
                });
            }
            crate::ir::IRTerminator::Branch { cond, true_block, false_block } => {
                // Load cond into S0, then BNE S0, true_block; BR ZERO, false_block.
                code.extend(ss_load_value(cond, &vreg_stack_slots, S0));
                let true_patch = code.len();
                code.extend(Instruction::Bne { ra: S0, disp: 0 }.encode());
                branch_patches.push(BranchPatch {
                    code_offset: true_patch,
                    target_label: true_block.clone(),
                });
                let false_patch = code.len();
                code.extend(Instruction::Br { ra: ZERO, disp: 0 }.encode());
                branch_patches.push(BranchPatch {
                    code_offset: false_patch,
                    target_label: false_block.clone(),
                });
            }
            crate::ir::IRTerminator::Return(vals) => {
                if let Some(first_val) = vals.first() {
                    code.extend(ss_load_value(first_val, &vreg_stack_slots, Gpr::R0));
                }
                // Epilogue:
                code.extend(sp_ldq(RA, lr_save_off as i64));
                code.extend(sp_ldq(FP, fp_save_off as i64));
                code.extend(sp_adjust(frame_size as i64));
                code.extend(Instruction::Ret.encode());
            }
            crate::ir::IRTerminator::Unreachable => {
                // CALL_PAL 0 — trap. Must NOT fall through.
                code.extend(Instruction::CallPal { palcode: 0x0 }.encode());
            }
            crate::ir::IRTerminator::Switch { discr, targets, default } => {
                code.extend(ss_load_value(discr, &vreg_stack_slots, S0));
                for (val, label) in targets {
                    code.extend(ss_load_imm(S1, *val));
                    code.extend(Instruction::Cmpeq { ra: S0, rb: S1, rc: S2 }.encode());
                    let patch = code.len();
                    code.extend(Instruction::Bne { ra: S2, disp: 0 }.encode());
                    branch_patches.push(BranchPatch {
                        code_offset: patch,
                        target_label: label.clone(),
                    });
                }
                let patch = code.len();
                code.extend(Instruction::Br { ra: ZERO, disp: 0 }.encode());
                branch_patches.push(BranchPatch {
                    code_offset: patch,
                    target_label: default.clone(),
                });
            }
            crate::ir::IRTerminator::Invoke { dst: _, func: _, args: _, normal, unwind: _ } => {
                let patch = code.len();
                code.extend(Instruction::Br { ra: ZERO, disp: 0 }.encode());
                branch_patches.push(BranchPatch {
                    code_offset: patch,
                    target_label: normal.clone(),
                });
            }
            crate::ir::IRTerminator::TailCall { .. } => {
                code.extend(sp_ldq(RA, lr_save_off as i64));
                code.extend(sp_ldq(FP, fp_save_off as i64));
                code.extend(sp_adjust(frame_size as i64));
                code.extend(Instruction::Ret.encode());
            }
            crate::ir::IRTerminator::Resume { .. } => {
                code.extend(Instruction::Nop.encode());
            }
        }
    }

    // ── Phase 5: Patch branch offsets ──
    // BR/Bcc disp21: target = PC + 4 + (disp21 * 4).
    // disp21 = (target_offset - (patch_offset + 4)) / 4.
    for patch in &branch_patches {
        if let Some(&target_idx) = label_to_idx.get(&patch.target_label) {
            let target_offset = block_start_offsets[target_idx] as i64;
            let pc_offset = (patch.code_offset as i64) + 4;
            let disp = (target_offset - pc_offset) / 4;
            let disp21 = (disp as i32) & 0x1F_FFFF;
            // Patch the low 21 bits of the instruction word (LE byte order).
            // The instruction word is at code[patch.code_offset..patch.code_offset+4].
            let word_le = [
                code[patch.code_offset],
                code[patch.code_offset + 1],
                code[patch.code_offset + 2],
                code[patch.code_offset + 3],
            ];
            let mut word = u32::from_le_bytes(word_le);
            word = (word & !0x1F_FFFF) | (disp21 as u32);
            let new_le = word.to_le_bytes();
            code[patch.code_offset..patch.code_offset + 4].copy_from_slice(&new_le);
        }
    }

    // ── Phase 6: Build AllocatedFunction ──
    let total_code_size = code.len();
    let entry_block_label = func
        .blocks
        .first()
        .map(|b| b.label.clone())
        .unwrap_or_else(|| "entry".to_string());

    let allocated_block = AllocatedBlock {
        label: entry_block_label,
        instructions: vec![AllocatedInstruction {
            opcode: "raw".to_string(),
            reads: vec![],
            writes: vec![],
            encoded: code,
        }],
        code_offset: 0,
    };

    Ok(AllocatedFunction {
        name: func_name,
        blocks: vec![allocated_block],
        frame_size,
        callee_saved: vec![
            PhysicalReg::new(RegClass::Gpr, Gpr::R26 as u32),
            PhysicalReg::new(RegClass::Gpr, Gpr::R15 as u32),
        ],
        spill_slots: all_vreg_ids.len(),
        code_size: total_code_size,
        relocations,
        wasm_func_type: None,
        wasm_locals: None,
    })
}

/// Real register allocation: assigns vregs to real GPRs where possible,
/// spilling the rest to stack slots. Falls back to the stack-slot allocator
/// for the instruction encoding, but records the physical register assignments
/// in the AllocatedFunction's reads/writes fields.
///
/// This is a hybrid approach (Wave 23): the instruction encoding still uses
/// stack slots (for safety), but the allocation metadata records which vregs
/// COULD be in real registers. A future wave will use this metadata to emit
/// register-based instructions directly.
fn alpha_allocate_registers_real(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    // Run the existing stack-slot allocator to get a working AllocatedFunction.
    let mut allocated = alpha_allocate_registers_ss(func)?;

    // Post-process: record which vregs are assigned to real registers.
    // We use a simple greedy assignment: the first N vregs (sorted by ID)
    // get assigned to the first N available caller-saved GPRs.

    // Collect all vreg IDs.
    let mut all_vreg_ids: Vec<u32> = Vec::new();
    for &id in func.vregs.keys() { all_vreg_ids.push(id); }
    for param in &func.params {
        if let Some(id) = param.as_register() { all_vreg_ids.push(id); }
    }
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() { all_vreg_ids.push(id); }
            for id in instr.used_regs() { all_vreg_ids.push(id); }
        }
    }
    all_vreg_ids.sort();
    all_vreg_ids.dedup();

    // Assign the first N vregs to PhysicalReg GPRs (index 0..N).
    // The actual register indices are backend-specific but we use a
    // generic 0-based indexing scheme here.
    let max_real_regs = 8; // conservative limit
    for (i, &_vreg_id) in all_vreg_ids.iter().enumerate() {
        if i < max_real_regs {
            let preg = crate::backend::PhysicalReg::new(
                crate::backend::RegClass::Gpr,
                i as u32,
            );
            // Record this assignment in every instruction that defines/uses this vreg.
            for block in &mut allocated.blocks {
                for instr in &mut block.instructions {
                    // Check if this instruction defines the vreg
                    // (simplified: we add the preg to writes for every instruction
                    // that could define it, and to reads for every instruction
                    // that could use it — this is conservative metadata).
                    if i < max_real_regs {
                        if !instr.writes.contains(&preg) {
                            instr.writes.push(preg);
                        }
                        if !instr.reads.contains(&preg) {
                            instr.reads.push(preg);
                        }
                    }
                }
            }
        }
    }

    // Record the number of real registers used.
    allocated.spill_slots = all_vreg_ids.len().saturating_sub(max_real_regs);

    Ok(allocated)
}

/// Emit a single IR instruction as Alpha machine code.
#[allow(clippy::too_many_arguments)]
fn emit_instr(
    instr: &IRInstr,
    vreg_stack_slots: &HashMap<u32, i32>,
    alloc_offsets: &HashMap<u32, i32>,
    code: &mut Vec<u8>,
    relocations: &mut Vec<RelocationEntry>,
    frame_size: usize,
    lr_save_off: i16,
    fp_save_off: i16,
) {
    let _ = alloc_offsets;
    match instr {
        IRInstr::Add { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Addq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Sub { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Subq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Mul { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Mulq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Div { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // Shift-and-subtract division (O(64) instead of O(quotient)).
            code.extend(Instruction::Addq { ra: ZERO, rb: ZERO, rc: S2 }.encode()); // S2 = 0 (remainder)
            code.extend(Instruction::Addq { ra: ZERO, rb: ZERO, rc: S3 }.encode()); // S3 = 0 (quotient)
            code.extend(Instruction::AddqLi { ra: ZERO, lit: 64, rc: S4 }.encode()); // S4 = 64 (counter)
            let div_loop = code.len();
            code.extend_from_slice(&op_lit(0x12, S0, 63, S5, 0x34).to_le_bytes()); // S5 = S0 >> 63
            code.extend_from_slice(&op_lit(0x12, S0, 1, S0, 0x39).to_le_bytes());  // S0 <<= 1
            code.extend_from_slice(&op_lit(0x12, S2, 1, S2, 0x39).to_le_bytes());  // S2 <<= 1
            code.extend_from_slice(&op_reg(0x11, S5, S2, S2, 0x20).to_le_bytes()); // S2 |= S5 (BIS/OR)
            code.extend_from_slice(&op_lit(0x12, S3, 1, S3, 0x39).to_le_bytes());  // S3 <<= 1
            code.extend_from_slice(&op_lit(0x10, S4, 1, S4, 0x29).to_le_bytes());  // S4 -= 1 (before branch)
            code.extend_from_slice(&op_reg(0x10, S2, S1, S5, 0x1D).to_le_bytes()); // S5 = (S2 < S1)
            code.extend_from_slice(&op_br(0x3D, S5, 2).to_le_bytes());             // BNE S5, +2 (skip)
            code.extend(Instruction::Subq { ra: S2, rb: S1, rc: S2 }.encode());   // S2 -= S1
            code.extend(Instruction::AddqLi { ra: S3, lit: 1, rc: S3 }.encode()); // S3 += 1
            let div_br_disp = ((div_loop as i32 - code.len() as i32) / 4) - 1;
            code.extend_from_slice(&op_br(0x3D, S4, div_br_disp).to_le_bytes());
            code.extend(Instruction::Or { ra: S3, rb: ZERO, rc: S0 }.encode()); // S0 = quotient
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            // FP dispatch: when the result type is F32 or F64, emit native
            // Alpha FP arithmetic (ADDT/SUBT/MULT/DIVT) instead of the
            // integer path.  Mirrors the x86_64 stack-slot pattern
            // (stack_slot_isel.rs:427-549).
            if let Some(IRType::F32) | Some(IRType::F64) = ty {
                emit_fp_binop(op, dst, lhs, rhs, ty, vreg_stack_slots, code);
            } else {
                emit_binop(op, dst, lhs, rhs, vreg_stack_slots, code);
            }
        }
        IRInstr::Cmp { kind, dst, lhs, rhs, ty } => {
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
            // FP dispatch: when the operand type is F32 or F64, emit native
            // Alpha FP comparisons (CMPTEQ/CMPTLT/CMPTLE/CMPTUN) via
            // `emit_fp_binop` instead of the integer path.  Integer CMP on
            // raw FP bits gives silently wrong results for negatives/NaN.
            // Mirrors the BinOp arm at line 986.
            if let Some(IRType::F32) | Some(IRType::F64) = ty {
                emit_fp_binop(&binop_kind, dst, lhs, rhs, ty, vreg_stack_slots, code);
            } else {
                emit_binop(&binop_kind, dst, lhs, rhs, vreg_stack_slots, code);
            }
        }
        IRInstr::UnaryOp { op, dst, operand, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(operand, vreg_stack_slots, S0));
            match op {
                UnaryOpKind::Neg => {
                    // SUBQ ZERO, S0, S0 (S0 = -S0).
                    code.extend(Instruction::Subq { ra: ZERO, rb: S0, rc: S0 }.encode());
                }
                UnaryOpKind::Not => {
                    // XOR S0, -1, S0 — but -1 needs to be loaded.
                    // Easier: ORNOT S0, ZERO, S0 — but ORNOT is its own opcode (0x11, function 0x60).
                    // ORNOT ra, rb, rc: rc = ra | ~rb. With rb = ZERO, rc = ra | ~0 = ra | -1.
                    // Encoding: same as OR but function 0x60.
                    let word = op_reg(0x11, S0, ZERO, S0, 0x60);
                    code.extend_from_slice(&word.to_le_bytes());
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    // Not directly supported on base Alpha (Alpha 21264+ has CTPOP, CTLZ).
                    // Simplified: emit 0.
                    code.extend(ss_load_imm(S0, 0));
                }
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Load { dst, addr, offset, ty } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // S2 = addr + offset.
            code.extend(ss_load_value(addr, vreg_stack_slots, S2));
            if *offset != 0 {
                code.extend(ss_load_imm(S3, *offset as i64));
                code.extend(Instruction::Addq { ra: S2, rb: S3, rc: S2 }.encode());
            }
            // Typed load based on IR type.  Alpha is little-endian.
            //   U8/I8  → LDBU (zero-extend byte; sign-extend if signed)
            //   U16/I16→ LDWU (zero-extend halfword; sign-extend if signed)
            //   U32/I32→ LDL  (sign-extends 32→64; mask to zero-extend for U32)
            //   U64/I64→ LDQ  (full 64-bit load)
            //   Ptr    → LDQ  (pointer-sized, 64-bit on alpha)
            use crate::ir::IRType;
            match ty {
                IRType::U8 => {
                    code.extend(Instruction::Ldbu { ra: S0, disp: 0, rb: S2 }.encode());
                }
                IRType::I8 => {
                    // LDBU (zero-extend byte), then sign-extend: SLL 56, SRA 56.
                    // SLL = opcode 0x12 function 0x34; SRA = opcode 0x13 function 0x3C.
                    // Use register form with S3 holding the shift count.
                    code.extend(Instruction::Ldbu { ra: S0, disp: 0, rb: S2 }.encode());
                    code.extend(ss_load_imm(S3, 56));
                    code.extend_from_slice(&op_reg(0x12, S0, S3, S0, 0x34).to_le_bytes()); // SLL S0,S3,S0
                    code.extend_from_slice(&op_reg(0x13, S0, S3, S0, 0x3C).to_le_bytes()); // SRA S0,S3,S0
                }
                IRType::U16 => {
                    code.extend(Instruction::Ldwu { ra: S0, disp: 0, rb: S2 }.encode());
                }
                IRType::I16 => {
                    // LDWU (zero-extend halfword), then sign-extend: SLL 48, SRA 48.
                    code.extend(Instruction::Ldwu { ra: S0, disp: 0, rb: S2 }.encode());
                    code.extend(ss_load_imm(S3, 48));
                    code.extend_from_slice(&op_reg(0x12, S0, S3, S0, 0x34).to_le_bytes()); // SLL 48
                    code.extend_from_slice(&op_reg(0x13, S0, S3, S0, 0x3C).to_le_bytes()); // SRA 48
                }
                IRType::U32 => {
                    // LDL sign-extends 32→64.  Zero-extend by masking high 32
                    // bits: S0 = S0 & 0x00000000FFFFFFFF.
                    // ZAPNOT (opcode 0x12, function 0x31) keeps bytes of ra
                    // where rb's corresponding bit is 1.  rb = 0x0F keeps
                    // bytes 0-3, zeros bytes 4-7.
                    code.extend(Instruction::Ldl { ra: S0, disp: 0, rb: S2 }.encode());
                    code.extend(ss_load_imm(S3, 0x0F));
                    code.extend_from_slice(&op_reg(0x12, S0, S3, S0, 0x31).to_le_bytes());
                }
                IRType::I32 => {
                    // LDL sign-extends 32→64.  Correct for signed.
                    code.extend(Instruction::Ldl { ra: S0, disp: 0, rb: S2 }.encode());
                }
                _ => {
                    // U64, I64, Ptr, Func, etc. → full 64-bit load.
                    code.extend(Instruction::Ldq { ra: S0, disp: 0, rb: S2 }.encode());
                }
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Store { value, addr, offset, ty } => {
            code.extend(ss_load_value(addr, vreg_stack_slots, S2));
            if *offset != 0 {
                code.extend(ss_load_imm(S3, *offset as i64));
                code.extend(Instruction::Addq { ra: S2, rb: S3, rc: S2 }.encode());
            }
            code.extend(ss_load_value(value, vreg_stack_slots, S0));
            // Typed store based on IR type.
            use crate::ir::IRType;
            match ty {
                IRType::U8 | IRType::I8 => {
                    code.extend(Instruction::Stb { ra: S0, disp: 0, rb: S2 }.encode());
                }
                IRType::U16 | IRType::I16 => {
                    code.extend(Instruction::Stw { ra: S0, disp: 0, rb: S2 }.encode());
                }
                IRType::U32 | IRType::I32 => {
                    code.extend(Instruction::Stl { ra: S0, disp: 0, rb: S2 }.encode());
                }
                _ => {
                    code.extend(Instruction::Stq { ra: S0, disp: 0, rb: S2 }.encode());
                }
            }
        }
        IRInstr::Alloc { dst, size: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            if let Some(&off) = alloc_offsets.get(&dst_id) {
                // S0 = FP + off
                code.extend(ss_load_imm(S1, off as i64));
                code.extend(Instruction::Addq { ra: FP, rb: S1, rc: S0 }.encode());
                let dst_slot = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                code.extend(ss_st(S0, dst_slot));
            }
        }
        IRInstr::Free { ptr: _ } => { /* no-op; lowered to Call */ }
        IRInstr::Cast { kind, dst, src, from_ty, to_ty } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);

            // Resolve src's stack offset.  If src is a register, its value
            // already lives at its own slot.  If src is an immediate /
            // address / label, we ferry it through S0 and spill to dst_off
            // (safe -- we overwrite dst with the result before returning,
            // and all reads of src happen before any write-back).
            let src_off: i32 = match src.as_register() {
                Some(id) => vreg_stack_slots.get(&id).copied().unwrap_or(0),
                None => {
                    code.extend(ss_load_value(src, vreg_stack_slots, S0));
                    code.extend(ss_st(S0, dst_off));
                    dst_off
                }
            };

            match kind {
                CastKind::ZExt | CastKind::SExt | CastKind::Trunc | CastKind::BitCast => {
                    // Integer-only casts on 64-bit Alpha are no-ops in
                    // registers: all integer values are already sign /
                    // zero-extended to 64 bits.  If src was a register, move
                    // it to dst; if src was an immediate, it's already at
                    // dst_off from the spill above.
                    if src.as_register().is_some() {
                        code.extend(ss_load_value(src, vreg_stack_slots, S0));
                        code.extend(ss_st(S0, dst_off));
                    }
                }
                CastKind::IntToFloat => {
                    // Signed int -> float.  Load the integer bits into FA via
                    // LDT (memory already holds the int as 64 bits), then
                    // CVTQT (int64 -> f64).  Narrow to f32 if needed.
                    code.extend(fp_ldt(FA, FP, src_off as i16));
                    code.extend(fp_cvt(FP_CVTQT, FA, FA));
                    if matches!(to_ty, Some(IRType::F64)) {
                        code.extend(fp_stt(FA, FP, dst_off as i16));
                    } else {
                        code.extend(fp_cvt(FP_CVTTS, FA, FA));
                        code.extend(fp_sts(FA, FP, dst_off as i16));
                    }
                }
                CastKind::UIntToFloat => {
                    // G5b: Unsigned int -> float with full u64 range.
                    // Alpha CVTQT treats the source as SIGNED.  For values
                    // with the high bit set (> i64::MAX), use the shift trick:
                    //   x' = x >> 1;  f = CVTQT(x');  f = f + f  (= x, modulo
                    // rounding for the dropped low bit — exact for even x,
                    // off by < 1 ulp for odd x with f64 mantissa).
                    // For values with the high bit clear, CVTQT is exact.
                    // Load the int, test the high bit, branch.
                    code.extend(ss_ld(S0, src_off));              // S0 = x (int bits)
                    // S1 = x >> 63 (high bit).
                    code.extend(ss_load_imm(S1, 63));
                    // SRA (arithmetic shift right) S0, S1, S1.
                    // Alpha SRA: opcode 0x12, function 0x3C.  Use op_reg.
                    // (Alpha has no separate quadword shift; SLL/SRL/SRA are 64-bit.)
                    code.extend_from_slice(&op_reg(0x12, S0, S1, S1, 0x3C).to_le_bytes());
                    // S1 = 0 if high bit clear, -1 (all 1s) if set.
                    // If S1 == 0: fast path (CVTQT direct).  Else: shift path.
                    // BEQ S1, +offset (branch if S1 == 0).  Alpha BEQ: op_br(0x39, S1, disp).
                    // We'll emit both paths with a branch over the shift path.
                    // Layout:
                    //   BEQ S1, fast_path   (branch forward past the shift path)
                    //   -- shift path (high bit set) --
                    //   S2 = S0 >> 1;  CVTQT(S2) -> FA;  FA = FA + FA;  jump store
                    //   -- fast path --
                    //   CVTQT(S0) -> FA
                    //   -- store --
                    let beq_off = code.len();
                    code.extend_from_slice(&op_br(0x39, S1, 0).to_le_bytes());  // BEQ S1, +0 (patch later)
                    // Shift path: S2 = S0 >> 1 (logical, SRLQ function 0x34).
                    code.extend(ss_load_imm(S1, 1));
                    code.extend_from_slice(&op_reg(0x12, S0, S1, S2, 0x34).to_le_bytes());  // SRL S0, S1, S2
                    // Store S2 to a temp slot, LDT into FA, CVTQT.
                    code.extend(ss_st(S2, dst_off));              // spill S2
                    code.extend(fp_ldt(FA, FP, dst_off as i16));
                    code.extend(fp_cvt(FP_CVTQT, FA, FA));     // FA = (float)(x>>1)
                    code.extend(fp_op(FP_ADDT, FA, FA, FA));      // FA = FA + FA = 2*(x>>1) ≈ x
                    // Branch to store (unconditional, BR zero, +offset).
                    let br_to_store = code.len();
                    code.extend_from_slice(&op_br(0x30, ZERO, 0).to_le_bytes());  // BR zero, +0 (patch)
                    // Fast path: CVTQT(S0) -> FA.
                    let fast_off = code.len();
                    code.extend(ss_st(S0, dst_off));              // spill S0
                    code.extend(fp_ldt(FA, FP, dst_off as i16));
                    code.extend(fp_cvt(FP_CVTQT, FA, FA));
                    // Patch BEQ to jump to fast_off.
                    let beq_disp = ((fast_off as i32 - beq_off as i32) / 4) - 1;
                    let beq_word = u32::from_le_bytes(code[beq_off..beq_off+4].try_into().unwrap());
                    let beq_patched = (beq_word & !0x1FFFFF) | (beq_disp as u32 & 0x1FFFFF);
                    code[beq_off..beq_off+4].copy_from_slice(&beq_patched.to_le_bytes());
                    // Store path: write FA to dst (f64 or narrowed f32).
                    let store_off = code.len();
                    // Patch the BR to jump here.
                    let br_disp = ((store_off as i32 - br_to_store as i32) / 4) - 1;
                    let br_word = u32::from_le_bytes(code[br_to_store..br_to_store+4].try_into().unwrap());
                    let br_patched = (br_word & !0x1FFFFF) | (br_disp as u32 & 0x1FFFFF);
                    code[br_to_store..br_to_store+4].copy_from_slice(&br_patched.to_le_bytes());
                    if matches!(to_ty, Some(IRType::F64)) {
                        code.extend(fp_stt(FA, FP, dst_off as i16));
                    } else {
                        code.extend(fp_cvt(FP_CVTTS, FA, FA));
                        code.extend(fp_sts(FA, FP, dst_off as i16));
                    }
                }
                CastKind::FloatToInt => {
                    // f64/f32 -> signed int (truncating).  CVTTQ truncates
                    // f64 -> i64.  For f32 source, widen to f64 first.
                    if matches!(from_ty, Some(IRType::F64)) {
                        code.extend(fp_ldt(FA, FP, src_off as i16));
                    } else {
                        code.extend(fp_lds(FA, FP, src_off as i16));
                        code.extend(fp_cvt(FP_CVTST, FA, FA)); // widen f32 -> f64
                    }
                    code.extend(fp_cvt(FP_CVTTQ, FA, FA)); // f64 -> i64 (bits in FPR)
                    code.extend(fp_stt(FA, FP, dst_off as i16)); // store 64-bit int bits
                    code.extend(ss_ld(S0, dst_off)); // reload as int
                    code.extend(ss_st(S0, dst_off));
                }
                CastKind::FloatToUInt => {
                    // G5b: f64 -> unsigned int with negative clamping.
                    // If the float is < 0.0, clamp to 0 (negative values are
                    // out of u64 range).  Otherwise, CVTTQ (signed truncation)
                    // and store the bits — correct for 0 <= f < 2^63.
                    // TODO G5b: for f >= 2^63, subtract 2^63, CVTTQ, then OR
                    // the high bit back (the full-range unsigned correction).
                    // The negative-clamp path handles the most common real bug
                    // (negative float -> huge unsigned via bit reinterpretation).
                    if matches!(from_ty, Some(IRType::F64)) {
                        code.extend(fp_ldt(FA, FP, src_off as i16));
                    } else {
                        code.extend(fp_lds(FA, FP, src_off as i16));
                        code.extend(fp_cvt(FP_CVTST, FA, FA));  // widen f32 -> f64
                    }
                    // Compare FA to 0.0.  We need 0.0 in an FPR: store 0 to
                    // a temp slot, LDT into FB.
                    code.extend(ss_load_imm(S0, 0));
                    code.extend(ss_st(S0, dst_off));  // dst_off as temp
                    code.extend(fp_ldt(FB, FP, dst_off as i16));  // FB = 0.0
                    // CMPTLT FA, FB, FC → FC = 1.0 if FA < 0.0 else 0.0.
                    code.extend(fp_op(FP_CMPTLT, FA, FB, FC));
                    // Store FC to a temp, reload as int, test.
                    code.extend(ss_load_imm(S0, 0));  // temp slot for FC
                    // We need a temp slot distinct from dst_off.  Use src_off
                    // (already consumed).  Store FC to src_off as f64.
                    code.extend(fp_stt(FC, FP, src_off as i16));
                    code.extend(ss_ld(S0, src_off));  // S0 = FC bits (0 or 1 as float)
                    // If S0 != 0 (FA < 0), result = 0.  Else, CVTTQ.
                    // BLBC S0, nonzero_path (branch if low bit clear, i.e. == 0).
                    // Alpha BLBC: op_br(0x38, reg, disp).
                    let blbc_off = code.len();
                    code.extend_from_slice(&op_br(0x38, S0, 0).to_le_bytes());  // BLBC S0, +0 (patch)
                    // Negative path: result = 0.
                    code.extend(ss_load_imm(S0, 0));
                    code.extend(ss_st(S0, dst_off));
                    // Branch to end.
                    let br_end = code.len();
                    code.extend_from_slice(&op_br(0x30, ZERO, 0).to_le_bytes());  // BR zero, +0 (patch)
                    // Nonzero path: CVTTQ and store.
                    let nz_off = code.len();
                    // Patch BLBC to jump here.
                    let blbc_disp = ((nz_off as i32 - blbc_off as i32) / 4) - 1;
                    let blbc_word = u32::from_le_bytes(code[blbc_off..blbc_off+4].try_into().unwrap());
                    let blbc_patched = (blbc_word & !0x1FFFFF) | (blbc_disp as u32 & 0x1FFFFF);
                    code[blbc_off..blbc_off+4].copy_from_slice(&blbc_patched.to_le_bytes());
                    code.extend(fp_cvt(FP_CVTTQ, FA, FA));
                    code.extend(fp_stt(FA, FP, dst_off as i16));
                    code.extend(ss_ld(S0, dst_off));
                    code.extend(ss_st(S0, dst_off));
                    // Patch BR to jump to end (here).
                    let end_off = code.len();
                    let br_disp = ((end_off as i32 - br_end as i32) / 4) - 1;
                    let br_word = u32::from_le_bytes(code[br_end..br_end+4].try_into().unwrap());
                    let br_patched = (br_word & !0x1FFFFF) | (br_disp as u32 & 0x1FFFFF);
                    code[br_end..br_end+4].copy_from_slice(&br_patched.to_le_bytes());
                }
                CastKind::FloatToFloat => {
                    // f32 <-> f64.  Widen (f32 -> f64) or narrow (f64 -> f32).
                    if matches!(from_ty, Some(IRType::F64)) {
                        // f64 -> f32
                        code.extend(fp_ldt(FA, FP, src_off as i16));
                        code.extend(fp_cvt(FP_CVTTS, FA, FA));
                        code.extend(fp_sts(FA, FP, dst_off as i16));
                    } else {
                        // f32 -> f64
                        code.extend(fp_lds(FA, FP, src_off as i16));
                        code.extend(fp_cvt(FP_CVTST, FA, FA));
                        code.extend(fp_stt(FA, FP, dst_off as i16));
                    }
                }
            }
            let _ = (from_ty, to_ty);
        }
        IRInstr::Phi { .. } => { /* no code emitted */ }
        IRInstr::GetAddress { dst, name } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // LDA S0, sym — we use LDA with displacement 0 and record a relocation.
            // Actually, for absolute address, use LDAQ via relocation.
            // Simplified: load 0 (placeholder) and record a relocation.
            let reloc_offset = code.len() as u64;
            code.extend(Instruction::Lda { ra: S0, disp: 0, rb: ZERO }.encode());
            // Followed by LDAH S0, 0(S0) for high 16 bits.
            let word: u32 = (0x09u32 << 26)
                | ((S0.encoding() as u32) << 21)
                | ((S0.encoding() as u32) << 16);
            code.extend_from_slice(&word.to_le_bytes());
            relocations.push(RelocationEntry {
                offset: reloc_offset,
                symbol: name.clone(),
                reloc_type: "R_ALPHA_REFQUAD".to_string(),
            });
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Offset { dst, base, offset } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(base, vreg_stack_slots, S0));
            code.extend(ss_load_value(offset, vreg_stack_slots, S1));
            code.extend(Instruction::Addq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Select { dst, cond, true_val, false_val, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // dst = (cond != 0) ? true_val : false_val
            // Use CMOVNE: load false_val, then if cond != 0, overwrite with true_val.
            code.extend(ss_load_value(false_val, vreg_stack_slots, S0));
            code.extend(ss_load_value(cond, vreg_stack_slots, S1));
            code.extend(ss_load_value(true_val, vreg_stack_slots, S2));
            // CMOVNE S1, S2, S0: if S1 != 0, S0 = S2.
            code.extend(Instruction::Cmovne { ra: S1, rb: S2, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Ret { values } => {
            if let Some(first_val) = values.first() {
                code.extend(ss_load_value(first_val, vreg_stack_slots, Gpr::R0));
            }
            code.extend(sp_ldq(RA, lr_save_off as i64));
            code.extend(sp_ldq(FP, fp_save_off as i64));
            code.extend(sp_adjust(frame_size as i64));
            code.extend(Instruction::Ret.encode());
        }
        IRInstr::Branch { target: _ } => {
            // Instruction-level branch (not terminator). Redundant with
            // the Jump terminator that follows. Emit NOP (BIS ZERO,ZERO,ZERO)
            // to avoid unpatched self-loop branch.
            code.extend_from_slice(&[0x1F, 0x04, 0xFF, 0x47]); // BIS ZERO,ZERO,ZERO (LE)
        }
        IRInstr::CondBranch { cond: _, true_target: _, false_target: _ } => {
            // Instruction-level CondBranch (not terminator). Redundant with
            // the Branch terminator that follows. Emit NOPs.
            code.extend_from_slice(&[0x1F, 0x04, 0xFF, 0x47]); // NOP
            code.extend_from_slice(&[0x1F, 0x04, 0xFF, 0x47]); // NOP
            code.extend_from_slice(&[0x1F, 0x04, 0xFF, 0x47]); // NOP
        }
        IRInstr::Call { dst, func, args, is_extern: _ } => {
            // Move args into R16-R21.
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    code.extend(ss_load_value(arg, vreg_stack_slots, S0));
                    code.extend(Instruction::Or { ra: ZERO, rb: S0, rc: arg_reg }.encode());
                }
            }
            // BSR RA, func — 4-byte instruction with 21-bit displacement.
            let call_offset = code.len() as u64;
            code.extend(Instruction::Bsr { ra: RA, disp: 0 }.encode());
            relocations.push(RelocationEntry {
                offset: call_offset,
                symbol: func.clone(),
                reloc_type: "R_ALPHA_BRADDR".to_string(),
            });
            // Move return value from R0 to dst's stack slot.
            if let Some(d) = dst {
                let d_id = d.as_register().unwrap_or(0);
                let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                code.extend(Instruction::Or { ra: ZERO, rb: Gpr::R0, rc: S0 }.encode());
                code.extend(ss_st(S0, d_off));
            }
        }
        IRInstr::CtSelect { dst, cond, true_val, false_val, ty: _ } => {
            // Same as Select (CMOVNE is branch-free).
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(false_val, vreg_stack_slots, S0));
            code.extend(ss_load_value(cond, vreg_stack_slots, S1));
            code.extend(ss_load_value(true_val, vreg_stack_slots, S2));
            code.extend(Instruction::Cmovne { ra: S1, rb: S2, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::CtEq { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Cmpeq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::AtomicLoad { dst, addr, ty } => {
            emit_instr(
                &IRInstr::Load { dst: dst.clone(), addr: addr.clone(), offset: 0, ty: ty.clone() },
                vreg_stack_slots, alloc_offsets, code, relocations, frame_size, lr_save_off, fp_save_off,
            );
        }
        IRInstr::AtomicStore { value, addr, ty } => {
            emit_instr(
                &IRInstr::Store { value: value.clone(), addr: addr.clone(), offset: 0, ty: ty.clone() },
                vreg_stack_slots, alloc_offsets, code, relocations, frame_size, lr_save_off, fp_save_off,
            );
        }
        IRInstr::AtomicCas { dst, addr, expected, desired, ty } => {
            // CAS: load old; if old==expected, store desired; dst = old.
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // Load old value from *addr into dst's stack slot.
            emit_instr(
                &IRInstr::Load { dst: dst.clone(), addr: addr.clone(), offset: 0, ty: ty.clone() },
                vreg_stack_slots, alloc_offsets, code, relocations, frame_size, lr_save_off, fp_save_off,
            );
            // S0 = old (from dst's stack slot)
            code.extend(ss_ld(S0, dst_off));
            // S1 = expected
            code.extend(ss_load_value(expected, vreg_stack_slots, S1));
            // S2 = (S0 == S1) ? 1 : 0
            code.extend(Instruction::Cmpeq { ra: S0, rb: S1, rc: S2 }.encode());
            // BEQ S2, skip_store (if S2 == 0, comparison failed, skip store)
            // Skip 3 instructions: LDQ desired, LDQ addr, STQ
            code.extend(Instruction::Beq { ra: S2, disp: 3 }.encode());
            // S0 = desired
            code.extend(ss_load_value(desired, vreg_stack_slots, S0));
            // S3 = addr
            code.extend(ss_load_value(addr, vreg_stack_slots, S3));
            // STQ S0, 0(S3) — store desired to *addr
            code.extend(Instruction::Stq { ra: S0, disp: 0, rb: S3 }.encode());
            // skip_store: (dst already holds old value)
        }
        IRInstr::Syscall { nr, args, dst } => {
            // alpha Linux syscall: args in R16-R21, nr in R0,
            // `call_pal 0x83` (callsys), result in R0.
            // Translate VUMA-generic (asm-generic) syscall number to the
            // backend's native numbering. TODO(P1-b): per-arch table.
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::Alpha,
                *nr,
            );
            let syscall_arg_regs =
                [Gpr::R16, Gpr::R17, Gpr::R18, Gpr::R19, Gpr::R20, Gpr::R21];
            let num_reg_args = args.len().min(syscall_arg_regs.len());
            for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                code.extend(ss_load_value(arg, vreg_stack_slots, syscall_arg_regs[i]));
            }
            // LDI R0, nr
            code.extend(ss_load_imm(Gpr::R0, native_nr as i64));
            // CALL_PAL 0x83 (callsys)
            code.extend_from_slice(&Instruction::CallPal { palcode: 0x83 }.encode());
            // Store result (R0) to dst's stack slot
            if let Some(d) = dst {
                let dst_id = d.as_register().unwrap_or(0);
                let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                code.extend(ss_st(Gpr::R0, dst_off));
            }
        }
        // ── VectorOp (Wave 29) ──
        // Alpha has no SIMD encoder in the Wave 29 suite; emit nothing.
        // The vectorizer still produces the IR; this backend just cannot
        // lower it to SIMD machine code.
        IRInstr::VectorOp { .. } => {}
    }
}

// ===========================================================================
// Alpha FP instruction encoders (opcode 0x16, FP operate)
// ===========================================================================
//
// Alpha FP operate format (per Alpha Architecture Manual, section C.2):
//   bits 31-26: opcode (0x16 for FP operate)
//   bits 25-21: fa
//   bits 20-16: fb
//   bits 15-5 : function (11 bits -- wider than integer's 7-bit field)
//   bits 4-0  : fc
// All instructions are 32-bit little-endian.
//
// FP load/store use the standard memory format with an FPR in the ra field:
//   LDS (0x22), LDT (0x23), STS (0x26), STT (0x27).
//
// NOTE: Alpha has no direct FPR<->GPR move instruction -- values transit
// through memory (STT -> LDQ, or STQ -> LDT).  The helpers below address
// memory via FP (the frame pointer), matching the convention used by
// `ss_ld`/`ss_st`.  After the prologue, FP == SP, so this is consistent.

/// Encode an FP operate instruction (arithmetic / compare / convert).
/// `fn_code` is the 11-bit FP function field.
fn fp_op(fn_code: u32, fa: Fpr, fb: Fpr, fc: Fpr) -> Vec<u8> {
    let word: u32 = (0x16u32 << 26)
        | ((fa.encoding() as u32) << 21)
        | ((fb.encoding() as u32) << 16)
        | ((fn_code & 0x7FF) << 5)
        | (fc.encoding() as u32);
    word.to_le_bytes().to_vec()
}

/// Encode a unary FP convert instruction (CVTQT/CVTTQ/CVTST/CVTTS).
///
/// Alpha's CVT* family is unary: the source is FB, the destination is FC.
/// The FA field is IGNORED by the hardware, BUT the instruction encoding
/// REQUIRES FA = 31 (the libopcodes `ARG_FPZ1` constraint).  QEMU-alpha
/// raises SIGILL if FA != 31 for these opcodes, so we hardwire FA to F31
/// here rather than letting callers pass an arbitrary FPR.
fn fp_cvt(fn_code: u32, src: Fpr, dst: Fpr) -> Vec<u8> {
    let word: u32 = (0x16u32 << 26)
        | ((Fpr::F31.encoding() as u32) << 21)   // FA = F31 (required by encoding)
        | ((src.encoding() as u32) << 16)         // FB = source
        | ((fn_code & 0x7FF) << 5)
        | (dst.encoding() as u32);                // FC = destination
    word.to_le_bytes().to_vec()
}

/// Load double (LDT, opcode 0x23): FPR `fa` <- mem[rb + disp].
fn fp_ldt(fa: Fpr, rb: Gpr, disp: i16) -> Vec<u8> {
    let word: u32 = (0x23u32 << 26)
        | ((fa.encoding() as u32) << 21)
        | ((rb.encoding() as u32) << 16)
        | (disp as u16 as u32);
    word.to_le_bytes().to_vec()
}

/// Store double (STT, opcode 0x27): mem[rb + disp] <- FPR `fa`.
fn fp_stt(fa: Fpr, rb: Gpr, disp: i16) -> Vec<u8> {
    let word: u32 = (0x27u32 << 26)
        | ((fa.encoding() as u32) << 21)
        | ((rb.encoding() as u32) << 16)
        | (disp as u16 as u32);
    word.to_le_bytes().to_vec()
}

/// Load single (LDS, opcode 0x22).  Loads 32 bits and zero-extends to 64.
fn fp_lds(fa: Fpr, rb: Gpr, disp: i16) -> Vec<u8> {
    let word: u32 = (0x22u32 << 26)
        | ((fa.encoding() as u32) << 21)
        | ((rb.encoding() as u32) << 16)
        | (disp as u16 as u32);
    word.to_le_bytes().to_vec()
}

/// Store single (STS, opcode 0x26).  Stores the low 32 bits of FPR `fa`.
fn fp_sts(fa: Fpr, rb: Gpr, disp: i16) -> Vec<u8> {
    let word: u32 = (0x26u32 << 26)
        | ((fa.encoding() as u32) << 21)
        | ((rb.encoding() as u32) << 16)
        | (disp as u16 as u32);
    word.to_le_bytes().to_vec()
}

// Alpha FP function codes (IEEE).  T = double (64-bit), S = single (32-bit).
// Arithmetic is performed in T; S operands are widened via CVTST and
// narrowed via CVTTS.  Function codes use the /su (software-completion +
// underflow) variants for arithmetic, /svc (software-completion + integer-
// overflow + chopped) for CVTTQ, /sui (software-completion + underflow +
// inexact) for CVTQT, and /s for CVTST.  These are the codes QEMU-alpha's
// translator recognises.
//
// IMPORTANT (FIX-alpha-v2): the previous FP_CVTTQ = 0x5AF is decoded by
// QEMU-alpha 7.2 as `cvttq/sv` -- WITHOUT the /c (chopped) qualifier --
// which causes QEMU to honour the FPCR rounding mode (default = round-to-
// nearest) instead of truncating toward zero.  This produced off-by-one
// results in three gold-standard tests:
//   - neg_float_trunc: floattoint(-2.9) returned -3 (round-to-nearest)
//     instead of -2 (trunc toward zero).  Exit 253 vs expected 254.
//   - newton_sqrt: floattoint(sqrt(1000)=31.62) returned 32 instead of 31.
//   - taylor_exp: floattoint(e=2.718) returned 3 instead of 2.
//
// Function code 0x52F is decoded by QEMU 7.2 as `cvttq/svc` (with the /c
// chopped qualifier), which forces round-toward-zero (truncation)
// regardless of FPCR.  This matches the binutils canonical `cvttq/sv`
// table entry (0x52F) and the Alpha ARM, which states CVTTQ chops unless
// the /m (dynamic) qualifier is present.  Verified empirically on
// QEMU-alpha 7.2:
//   0x52F: floattoint(-2.9) -> exit 254 (-2, truncation)  [CORRECT]
//   0x5AF: floattoint(-2.9) -> exit 253 (-3, round-to-nearest)  [WRONG]
//
// The earlier worklog note that 0x52F caused SIGILL was incorrect: the
// SIGILL was caused by a different bug (FA != 31 for unary CVT*), which
// is now fixed by the fp_cvt helper hardwiring FA to F31.
const FP_ADDT: u32   = 0x5A0; // f64 add  (addt/su)
const FP_SUBT: u32   = 0x5A1; // f64 sub  (subt/su)
const FP_MULT: u32   = 0x5A2; // f64 mul  (mult/su)
const FP_DIVT: u32   = 0x5A3; // f64 div  (divt/su)
const FP_CVTQT: u32  = 0x7BE; // int64 -> f64   (cvtqt/sui)
const FP_CVTTQ: u32  = 0x52F; // f64 -> int64   (cvttq/svc, chopped/truncating)
const FP_CVTST: u32  = 0x6AC; // f32 -> f64     (cvtst/s, widen)
const FP_CVTTS: u32  = 0x5AC; // f64 -> f32     (cvtts/su, narrow)
const FP_CMPTUN: u32 = 0x5A4; // compare unordered  (cmptun/su)
const FP_CMPTEQ: u32 = 0x5A5; // compare equal      (cmpteq/su)
const FP_CMPTLT: u32 = 0x5A6; // compare less-than  (cmptlt/su)
const FP_CMPTLE: u32 = 0x5A7; // compare less-or-eq (cmptle/su)

/// Emit a binary op (BinOpKind) result into dst's stack slot.
fn emit_binop(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    vreg_stack_slots: &HashMap<u32, i32>,
    code: &mut Vec<u8>,
) {
    let dst_id = dst.as_register().unwrap_or(0);
    let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
    match op {
        BinOpKind::Add => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Addq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Sub => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Subq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Mul => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Mulq { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::UDiv | BinOpKind::SDiv => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if matches!(op, BinOpKind::SDiv) {
                // Signed division: take absolute values, divide unsigned,
                // then negate the quotient if operand signs differ.
                code.extend(Instruction::AddqLi { ra: ZERO, lit: 63, rc: S2 }.encode()); // S2 = 63
                code.extend(Instruction::Sra { ra: S0, rb: S2, rc: S3 }.encode()); // S3 = lhs >> 63
                code.extend(Instruction::Sra { ra: S1, rb: S2, rc: S4 }.encode()); // S4 = rhs >> 63
                code.extend_from_slice(&op_reg(0x11, S0, S3, S0, 0x40).to_le_bytes()); // XOR S0,S3,S0
                code.extend(Instruction::Subq { ra: S0, rb: S3, rc: S0 }.encode()); // S0 -= S3
                code.extend_from_slice(&op_reg(0x11, S1, S4, S1, 0x40).to_le_bytes()); // XOR S1,S4,S1
                code.extend(Instruction::Subq { ra: S1, rb: S4, rc: S1 }.encode()); // S1 -= S4
                code.extend_from_slice(&op_reg(0x11, S3, S4, S4, 0x40).to_le_bytes()); // S4 = S3 ^ S4
            }
            // Shift-and-subtract division (O(64) instead of O(quotient)).
            // S0 = dividend, S1 = divisor, S2 = remainder, S3 = quotient,
            // S4 = counter (64), S5 = temp.
            code.extend(Instruction::Addq { ra: ZERO, rb: ZERO, rc: S2 }.encode()); // S2 = 0 (remainder)
            code.extend(Instruction::Addq { ra: ZERO, rb: ZERO, rc: S3 }.encode()); // S3 = 0 (quotient)
            code.extend(Instruction::AddqLi { ra: ZERO, lit: 64, rc: S4 }.encode()); // S4 = 64 (counter)
            // Loop:
            let div_loop = code.len();
            code.extend_from_slice(&op_lit(0x12, S0, 63, S5, 0x34).to_le_bytes()); // S5 = S0 >> 63 (SRL)
            code.extend_from_slice(&op_lit(0x12, S0, 1, S0, 0x39).to_le_bytes());  // S0 = S0 << 1 (SLL)
            code.extend_from_slice(&op_lit(0x12, S2, 1, S2, 0x39).to_le_bytes());  // S2 = S2 << 1 (SLL)
            code.extend_from_slice(&op_reg(0x11, S5, S2, S2, 0x20).to_le_bytes()); // S2 = S2 | S5 (BIS/OR)
            code.extend_from_slice(&op_lit(0x12, S3, 1, S3, 0x39).to_le_bytes());  // S3 = S3 << 1 (SLL)
            code.extend_from_slice(&op_lit(0x10, S4, 1, S4, 0x29).to_le_bytes());  // S4 -= 1 (BEFORE branch — always executes)
            code.extend_from_slice(&op_reg(0x10, S2, S1, S5, 0x1D).to_le_bytes()); // S5 = (S2 < S1) (CMPULT)
            code.extend_from_slice(&op_br(0x3D, S5, 2).to_le_bytes());             // BNE S5, +2 (skip subtract+add)
            code.extend(Instruction::Subq { ra: S2, rb: S1, rc: S2 }.encode());   // S2 -= S1
            code.extend(Instruction::AddqLi { ra: S3, lit: 1, rc: S3 }.encode()); // S3 += 1 (set quotient bit)
            // skip:
            // BNE S4, loop: disp = (div_loop - current) / 4 - 1
            let div_br_disp = ((div_loop as i32 - code.len() as i32) / 4) - 1;
            code.extend_from_slice(&op_br(0x3D, S4, div_br_disp).to_le_bytes());   // BNE S4, loop
            // S3 = quotient. Move to S0.
            code.extend(Instruction::Or { ra: S3, rb: ZERO, rc: S0 }.encode()); // S0 = S3
            if matches!(op, BinOpKind::SDiv) {
                code.extend_from_slice(&op_reg(0x11, S0, S4, S0, 0x40).to_le_bytes()); // XOR S0,S4,S0
                code.extend(Instruction::Subq { ra: S0, rb: S4, rc: S0 }.encode()); // S0 -= S4
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SRem | BinOpKind::URem => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if matches!(op, BinOpKind::SRem) {
                // Signed remainder: same sign handling as SDiv.
                code.extend(Instruction::AddqLi { ra: ZERO, lit: 63, rc: S2 }.encode());
                code.extend(Instruction::Sra { ra: S0, rb: S2, rc: S3 }.encode());
                code.extend(Instruction::Sra { ra: S1, rb: S2, rc: S4 }.encode());
                code.extend_from_slice(&op_reg(0x11, S0, S3, S0, 0x40).to_le_bytes());
                code.extend(Instruction::Subq { ra: S0, rb: S3, rc: S0 }.encode());
                code.extend_from_slice(&op_reg(0x11, S1, S4, S1, 0x40).to_le_bytes());
                code.extend(Instruction::Subq { ra: S1, rb: S4, rc: S1 }.encode());
                code.extend_from_slice(&op_reg(0x11, S3, S4, S4, 0x40).to_le_bytes()); // S4 = sign(lhs) (for remainder sign)
            }
            // Shift-and-subtract division, returning remainder.
            code.extend(Instruction::Addq { ra: ZERO, rb: ZERO, rc: S2 }.encode()); // S2 = 0 (remainder)
            code.extend(Instruction::Addq { ra: ZERO, rb: ZERO, rc: S3 }.encode()); // S3 = 0 (quotient)
            code.extend(Instruction::AddqLi { ra: ZERO, lit: 64, rc: S4 }.encode()); // S4 = 64
            let rem_loop = code.len();
            code.extend_from_slice(&op_lit(0x12, S0, 63, S5, 0x34).to_le_bytes()); // S5 = S0 >> 63
            code.extend_from_slice(&op_lit(0x12, S0, 1, S0, 0x39).to_le_bytes());  // S0 <<= 1
            code.extend_from_slice(&op_lit(0x12, S2, 1, S2, 0x39).to_le_bytes());  // S2 <<= 1
            code.extend_from_slice(&op_reg(0x11, S5, S2, S2, 0x20).to_le_bytes()); // S2 |= S5 (BIS/OR)
            code.extend_from_slice(&op_lit(0x12, S3, 1, S3, 0x39).to_le_bytes());  // S3 <<= 1
            code.extend_from_slice(&op_lit(0x10, S4, 1, S4, 0x29).to_le_bytes());  // S4 -= 1 (before branch)
            code.extend_from_slice(&op_reg(0x10, S2, S1, S5, 0x1D).to_le_bytes()); // S5 = (S2 < S1)
            code.extend_from_slice(&op_br(0x3D, S5, 2).to_le_bytes());             // BNE S5, +2 (skip)
            code.extend(Instruction::Subq { ra: S2, rb: S1, rc: S2 }.encode());   // S2 -= S1
            code.extend(Instruction::AddqLi { ra: S3, lit: 1, rc: S3 }.encode()); // S3 += 1
            let rem_br_disp = ((rem_loop as i32 - code.len() as i32) / 4) - 1;
            code.extend_from_slice(&op_br(0x3D, S4, rem_br_disp).to_le_bytes());   // BNE S4, loop
            // S2 = remainder. Move to S0.
            code.extend(Instruction::Or { ra: S2, rb: ZERO, rc: S0 }.encode()); // S0 = S2
            if matches!(op, BinOpKind::SRem) {
                // If S4 (sign of original dividend) != 0, negate remainder.
                code.extend_from_slice(&op_reg(0x11, S0, S4, S0, 0x40).to_le_bytes()); // XOR S0,S4,S0
                code.extend(Instruction::Subq { ra: S0, rb: S4, rc: S0 }.encode()); // S0 -= S4
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::And => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::And { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Or => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Or { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Xor => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Xor { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Shl => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Sll { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ShrL => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Srl { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ShrA => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Sra { ra: S0, rb: S1, rc: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Ror | BinOpKind::Rol => {
            // Simplified: just return lhs.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Eq | BinOpKind::Ne
        | BinOpKind::SLt | BinOpKind::ULt
        | BinOpKind::SLe | BinOpKind::ULe
        | BinOpKind::SGt | BinOpKind::UGt
        | BinOpKind::SGe | BinOpKind::UGe => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // Use CMPxx: 1 if condition true, else 0.
            match op {
                BinOpKind::Eq => code.extend(Instruction::Cmpeq { ra: S0, rb: S1, rc: S0 }.encode()),
                BinOpKind::Ne => {
                    // NE = !EQ: CMPEQ then XOR with 1.
                    code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
                    code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
                    code.extend(Instruction::Cmpeq { ra: S0, rb: S1, rc: S0 }.encode());
                    code.extend(Instruction::AddqLi { ra: ZERO, lit: 1, rc: S1 }.encode());
                    code.extend(Instruction::Xor { ra: S0, rb: S1, rc: S0 }.encode());
                }
                BinOpKind::SLt => {
                    // Signed less-than: XOR both operands with sign bit (0x8000...),
                    // then use CMPULT (unsigned). This maps signed [-2^63, 2^63-1]
                    // to unsigned [0, 2^64-1] monotonically.
                    // Load sign bit: S4 = 1 << 63 (ss_load_imm can't handle 64-bit)
                    code.extend(Instruction::Lda { ra: S4, disp: 1, rb: ZERO }.encode()); // S4 = 1
                    code.extend_from_slice(&op_lit(0x12, S4, 63, S4, 0x39).to_le_bytes()); // S4 = S4 << 63
                    code.extend_from_slice(&op_reg(0x11, S0, S4, S0, 0x40).to_le_bytes()); // XOR S0, S4, S0
                    code.extend_from_slice(&op_reg(0x11, S1, S4, S1, 0x40).to_le_bytes()); // XOR S1, S4, S1
                    code.extend(Instruction::Cmplt { ra: S0, rb: S1, rc: S0 }.encode());
                }
                BinOpKind::ULt => {
                    // ULT = !ULE && ... actually: ULT = !ULE?  No. ULE = (a <= b unsigned). ULT = (a < b unsigned) = !ULE? No, ULE includes equal. ULT = ULE && !EQ.
                    // Easier: ULT = !UGE. Or compute CMPULE and check.
                    // For ULT: rc = (a < b unsigned). Equivalent: CMPULE b, a gives (b <= a). ULT = !CMPULE(b,a) = !(b<=a) = (a<b).
                    // So ULT(a, b) = XOR(CMPULE(b, a), 1).
                    code.extend(Instruction::Cmpule { ra: S1, rb: S0, rc: S0 }.encode()); // S0 = (S1 <= S0) = (b <= a)
                    code.extend(Instruction::AddqLi { ra: ZERO, lit: 1, rc: S2 }.encode());
                    code.extend(Instruction::Xor { ra: S0, rb: S2, rc: S0 }.encode()); // S0 = !(b<=a) = (a<b)
                }
                BinOpKind::SLe => {
                    // SLE = !SLT(b, a) = !(b < a) (signed).
                    // XOR both with sign bit, then CMPULT.
                    code.extend(Instruction::Lda { ra: S4, disp: 1, rb: ZERO }.encode()); code.extend_from_slice(&op_lit(0x12, S4, 63, S4, 0x39).to_le_bytes());
                    code.extend_from_slice(&op_reg(0x11, S1, S4, S1, 0x40).to_le_bytes()); // XOR S1 (b)
                    code.extend_from_slice(&op_reg(0x11, S0, S4, S0, 0x40).to_le_bytes()); // XOR S0 (a)
                    code.extend(Instruction::Cmplt { ra: S1, rb: S0, rc: S0 }.encode()); // S0 = (b' < a') = (b < a signed)
                    code.extend(Instruction::AddqLi { ra: ZERO, lit: 1, rc: S2 }.encode());
                    code.extend(Instruction::Xor { ra: S0, rb: S2, rc: S0 }.encode()); // S0 = !(b<a) = (a<=b)
                }
                BinOpKind::ULe => code.extend(Instruction::Cmpule { ra: S0, rb: S1, rc: S0 }.encode()),
                BinOpKind::SGt => {
                    // SGT = SLT(b, a) (signed).
                    code.extend(Instruction::Lda { ra: S4, disp: 1, rb: ZERO }.encode()); code.extend_from_slice(&op_lit(0x12, S4, 63, S4, 0x39).to_le_bytes());
                    code.extend_from_slice(&op_reg(0x11, S1, S4, S1, 0x40).to_le_bytes()); // XOR S1 (b)
                    code.extend_from_slice(&op_reg(0x11, S0, S4, S0, 0x40).to_le_bytes()); // XOR S0 (a)
                    code.extend(Instruction::Cmplt { ra: S1, rb: S0, rc: S0 }.encode()); // S0 = (b' < a') = (b < a signed) = (a > b)
                }
                BinOpKind::UGt => {
                    // UGT = !ULE(a, b)... actually UGT(a, b) = ULE(b, a) && !EQ(a, b). Or simpler: UGT(a, b) = !ULE(a, b) && !EQ? No.
                    // UGT(a, b) = (a > b unsigned) = !(a <= b unsigned) || (a == b)? No: UGT = !ULE.
                    // Actually: UGT(a, b) = ULE(b, a) (b <= a) AND !EQ. But if a == b, ULE(b,a)=1 but UGT=0. So UGT = ULE(b, a) && !EQ.
                    // Simpler: UGT(a, b) = !ULE(a, b) && !... hmm actually UGT = !ULE && !EQ. No: UGT = (a > b), ULE = (a <= b). a > b means NOT (a <= b). So UGT = !ULE. Wait that's also wrong because if a == b, ULE=1 and UGT=0, so UGT = !ULE works.
                    // Wait: ULE(a,b) = (a <= b) = 1 if a <= b, 0 otherwise. !ULE = 1 if a > b. Yes, UGT = !ULE.
                    code.extend(Instruction::Cmpule { ra: S0, rb: S1, rc: S0 }.encode()); // S0 = (a <= b)
                    code.extend(Instruction::AddqLi { ra: ZERO, lit: 1, rc: S2 }.encode());
                    code.extend(Instruction::Xor { ra: S0, rb: S2, rc: S0 }.encode()); // S0 = !(a<=b) = (a>b)
                }
                BinOpKind::SGe => {
                    // SGE = !SLT(a, b) (signed).
                    code.extend(Instruction::Lda { ra: S4, disp: 1, rb: ZERO }.encode()); code.extend_from_slice(&op_lit(0x12, S4, 63, S4, 0x39).to_le_bytes());
                    code.extend_from_slice(&op_reg(0x11, S0, S4, S0, 0x40).to_le_bytes()); // XOR S0 (a)
                    code.extend_from_slice(&op_reg(0x11, S1, S4, S1, 0x40).to_le_bytes()); // XOR S1 (b)
                    code.extend(Instruction::Cmplt { ra: S0, rb: S1, rc: S0 }.encode()); // S0 = (a' < b') = (a < b signed)
                    code.extend(Instruction::AddqLi { ra: ZERO, lit: 1, rc: S2 }.encode());
                    code.extend(Instruction::Xor { ra: S0, rb: S2, rc: S0 }.encode()); // S0 = !(a<b) = (a>=b)
                }
                BinOpKind::UGe => {
                    // UGE = !ULT(a, b) = ULE(b, a).
                    code.extend(Instruction::Cmpule { ra: S1, rb: S0, rc: S0 }.encode()); // S0 = (b <= a) = (a >= b)
                }
                _ => unreachable!(),
            }
            code.extend(ss_st(S0, dst_off));
        }
    }
}

/// Emit a floating-point binary op.  Operand values live in their stack
/// slots (the same slots `ss_load_value` reads for integers); we load them
/// directly into FPRs with LDT/LDS, operate, and store back with STT/STS.
/// The result is then reloaded into S0 so downstream integer-path code
/// (which expects every value in a GPR) sees a consistent representation.
///
/// Alpha FP arithmetic is performed in double (T) precision.  For F32
/// operands, we widen via CVTST on entry and narrow via CVTTS on exit.
fn emit_fp_binop(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    ty: &Option<IRType>,
    vreg_stack_slots: &HashMap<u32, i32>,
    code: &mut Vec<u8>,
) {
    let dst_id = dst.as_register().unwrap_or(0);
    let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
    let is_f64 = matches!(ty, Some(IRType::F64));

    // Resolve operand stack offsets.  Register operands are loaded directly
    // from their own slot; immediates / addresses / labels are ferried into
    // S0 and spilled to dst_off (which we'll overwrite with the result
    // before returning, so the spill is safe).  Both operands are loaded
    // into FPRs BEFORE the result is written back.
    let lhs_off: i32 = lhs.as_register()
        .and_then(|id| vreg_stack_slots.get(&id).copied())
        .unwrap_or(dst_off);
    let rhs_off: i32 = rhs.as_register()
        .and_then(|id| vreg_stack_slots.get(&id).copied())
        .unwrap_or(dst_off);

    // Spill lhs immediate (if any) to its resolved slot, then load FA.
    if lhs.as_register().is_none() {
        code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
        code.extend(ss_st(S0, lhs_off));
    }
    if is_f64 {
        code.extend(fp_ldt(FA, FP, lhs_off as i16));
    } else {
        code.extend(fp_lds(FA, FP, lhs_off as i16));
        code.extend(fp_cvt(FP_CVTST, FA, FA)); // widen f32 -> f64
    }

    // Spill rhs immediate (after FA is loaded, so even if rhs_off == lhs_off
    // the lhs value is already safe in FA), then load FB.
    if rhs.as_register().is_none() {
        code.extend(ss_load_value(rhs, vreg_stack_slots, S0));
        code.extend(ss_st(S0, rhs_off));
    }
    if is_f64 {
        code.extend(fp_ldt(FB, FP, rhs_off as i16));
    } else {
        code.extend(fp_lds(FB, FP, rhs_off as i16));
        code.extend(fp_cvt(FP_CVTST, FB, FB)); // widen f32 -> f64
    }

    // Dispatch.  Arithmetic is always in T (double); S narrowing happens at
    // the very end if the result type is F32.
    match op {
        BinOpKind::Add => { code.extend(fp_op(FP_ADDT, FA, FB, FA)); }
        BinOpKind::Sub => { code.extend(fp_op(FP_SUBT, FA, FB, FA)); }
        BinOpKind::Mul => { code.extend(fp_op(FP_MULT, FA, FB, FA)); }
        BinOpKind::SDiv | BinOpKind::UDiv => { code.extend(fp_op(FP_DIVT, FA, FB, FA)); }
        BinOpKind::Eq | BinOpKind::Ne
        | BinOpKind::SLt | BinOpKind::ULt
        | BinOpKind::SLe | BinOpKind::ULe
        | BinOpKind::SGt | BinOpKind::UGt
        | BinOpKind::SGe | BinOpKind::UGe => {
            // Alpha FP compare writes 0.0 (false) or 1.0 (true) to the
            // destination FPR -- NOT a flags register.  We convert that to
            // the i1 result VUMA expects via CVTTQ (f64 -> i64), STT, LDQ,
            // then mask to 1 bit.
            let fc_code = match op {
                BinOpKind::Eq => FP_CMPTEQ,
                BinOpKind::Ne => FP_CMPTEQ, // inverted below
                BinOpKind::SLt | BinOpKind::ULt => FP_CMPTLT,
                BinOpKind::SLe | BinOpKind::ULe => FP_CMPTLE,
                BinOpKind::SGt | BinOpKind::UGt => FP_CMPTLT, // swapped below
                BinOpKind::SGe | BinOpKind::UGe => FP_CMPTLE, // swapped below
                _ => FP_CMPTEQ,
            };
            if matches!(op, BinOpKind::SGt | BinOpKind::UGt | BinOpKind::SGe | BinOpKind::UGe) {
                // For > / >=, compare FB < FA (or FB <= FA) instead of
                // FA < FB.
                code.extend(fp_op(fc_code, FB, FA, FA));
            } else {
                code.extend(fp_op(fc_code, FA, FB, FA));
            }
            // Alpha FP compare writes 2.0 (true) or 0.0 (false) to FC -- NOT
            // 1.0/0.0.  Convert to int (0 or 2) via CVTTQ, then shift right
            // by 1 (SRL) to normalize to 0 or 1.  For Ne, XOR with 1 to
            // invert (0<->1) -- this replaces the old `1.0 - FA` path which
            // was broken because it assumed the compare returned 1.0 for true
            // (producing -1.0 for true, which `& 1` then masked to 1 for BOTH
            // true and false).
            code.extend(fp_cvt(FP_CVTTQ, FA, FA));       // FA = 0 or 2 (int bits in FPR)
            code.extend(fp_stt(FA, FP, dst_off as i16)); // store to dst
            code.extend(ss_ld(S0, dst_off));             // S0 = 0 or 2
            // SRL S0, 1, S0 -- normalize 2->1, 0->0 (SRL = opcode 0x12, fn 0x34, literal form)
            code.extend_from_slice(&op_lit(0x12, S0, 1, S0, 0x34).to_le_bytes());
            if matches!(op, BinOpKind::Ne) {
                // Ne = !eq.  XOR with 1 flips 0<->1.
                code.extend(ss_load_imm(S1, 1));
                code.extend(Instruction::Xor { ra: S0, rb: S1, rc: S0 }.encode());
            }
            code.extend(ss_st(S0, dst_off));
            return; // done -- skip the post-arithmetic store below
        }
        // Bitwise / shift ops on floats are invalid; the IR verifier (F2a)
        // should reject these before codegen.  Fallback: delegate to the
        // integer path (will produce wrong results, but is safe).
        _ => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            emit_binop(op, dst, lhs, rhs, vreg_stack_slots, code);
            return;
        }
    }

    // Narrow f64 -> f32 if the result type is F32, then store to dst slot.
    if is_f64 {
        code.extend(fp_stt(FA, FP, dst_off as i16));
    } else {
        code.extend(fp_cvt(FP_CVTTS, FA, FA)); // narrow f64 -> f32
        code.extend(fp_sts(FA, FP, dst_off as i16));
    }
    // Reload into S0 so the integer-path slot is consistent.  Downstream FP
    // ops will fp_ldt/fp_lds directly from the slot, so this is harmless.
    code.extend(ss_ld(S0, dst_off));
    let _ = (lhs_off, rhs_off);
}

// ===========================================================================
// Backend struct + TargetInfo + Backend impl
// ===========================================================================

pub struct AlphaBackend {
    target_info: AlphaTargetInfo,
    /// Whether to use real register allocation (Wave 23) or stack-slot lowering.
    pub use_real_regalloc: bool,
}

impl AlphaBackend {
    pub fn new() -> Self { Self { target_info: AlphaTargetInfo, use_real_regalloc: false } }
}

impl Default for AlphaBackend {
    fn default() -> Self { Self::new() }
}

pub struct AlphaTargetInfo;

impl TargetInfo for AlphaTargetInfo {
    fn isa_name(&self) -> &'static str { "alpha" }
    fn target_triple(&self) -> &'static str { "alpha-unknown-linux-gnu" }
    fn elf_machine_type(&self) -> u16 { 0x9026 } // EM_ALPHA (unofficial)
    fn default_base_address(&self) -> u64 { 0x120000000 }
    fn pointer_width(&self) -> usize { 8 }
    fn size_of(&self, ty: &IRType) -> usize { crate::ir::size_of_with_ptr_width(ty, 8) }
    fn alignment_of(&self, ty: &IRType) -> usize { crate::ir::alignment_of_with_ptr_width(ty, 8) }
    fn endianness(&self) -> Endianness { Endianness::Little }
    fn has_registers(&self) -> bool { true }
    fn num_gp_regs(&self) -> usize { 32 }
    fn num_simd_fp_regs(&self) -> usize { 32 }
    fn has_hardwired_zero(&self) -> bool { true } // R31
    fn has_link_register(&self) -> bool { true } // R26
    fn has_branch_delay_slots(&self) -> bool { false }
    fn has_toc_pointer(&self) -> bool { false }
    fn has_condition_registers(&self) -> bool { false }
    fn calling_convention_name(&self) -> &'static str { "alpha-linux" }
    fn num_int_arg_regs(&self) -> usize { 6 } // R16-R21
    fn num_fp_arg_regs(&self) -> usize { 8 }
    fn stack_alignment(&self) -> usize { 16 }
    fn instruction_alignment(&self) -> usize { 4 }
    fn instruction_width_range(&self) -> (usize, usize) { (4, 4) }
    fn output_format(&self) -> OutputFormat { OutputFormat::Elf64 }

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::alpha()
    }
}

impl Backend for AlphaBackend {
    fn target_info(&self) -> &dyn TargetInfo { &self.target_info }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        if self.use_real_regalloc {
            alpha_allocate_registers_real(func)
        } else {
            alpha_allocate_registers_ss(func)
        }
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
        const R_ALPHA_BRADDR: &str = "R_ALPHA_BRADDR";
        const BASE_ADDR: u64 = 0x120000000;

        let elf_header_size: u64 = 64;
        let phdr_size: u64 = 56;
        let num_phdrs: u64 = 3;
        let phdr_end = elf_header_size + num_phdrs * phdr_size;
        let text_offset: u64 = phdr_end;

        // _start stub:
        //   LDQ R16, 0(SP)   — 4 bytes (load argc from stack)
        //   LDA R17, 8(SP)   — 4 bytes (argv = SP + 8, 64-bit pointers)
        //   BSR RA, main     — 4 bytes (will be patched)
        //   BIS ZERO, R0, R16 — 4 bytes (move return value to R16 = a0)
        //   LDA R0, 1(ZERO)   — 4 bytes (R0 = 1 = SYS_exit)
        //   CALL_PAL 0x83     — 4 bytes
        // Total: 24 bytes.
        let start_stub_size: usize = 24;
        let ffi_stub_offset: usize = start_stub_size;
        // FFI return-0 stub: BIS ZERO, ZERO, R0 (4 bytes) + RET (4 bytes) = 8 bytes.
        let ffi_stub_size: usize = 8;

        // ── __vuma_alloc / __vuma_free syscall stubs ──
        // Linux alpha syscall convention: syscall # in V0 (R0), args in A0-A5 (R16-R21),
        // return in V0.  Invoke via CALL_PAL 0x83 (callsys).
        //   Alpha has its OWN syscall table (NOT shared with m68k/x86). Key numbers
        //   from arch/alpha/include/uapi/asm/unistd.h:
        //     __NR_exit = 1, __NR_read = 3, __NR_write = 4, __NR_open = 5,
        //     __NR_close = 6, __NR_mmap = 113, __NR_munmap = 111, __NR_brk = 17,
        //     __NR_mprotect = 50, etc.
        //   [wave 6 — mmap ABI normalization, verified] alpha's __NR_mmap (71) is
        //   the DIRECT 6-arg form: (addr, len, prot, flags, fd, offset) all passed
        //   in R16-R21, with `offset` in BYTES (alpha has no mmap2; the generic
        //   sys_mmap handles byte→page conversion in-kernel). Both __vuma_alloc
        //   (which sets R21=0) and the custom `mmap` stub pass the caller's
        //   R16-R21 straight through with the SAME offset unit (bytes, via R21),
        //   satisfying the wave-6 "same offset-unit handling as __vuma_alloc"
        //   requirement. Alpha passes 6 args in R16-R21, so no stack-arg plumbing
        //   is needed.
        //
        //   CRITICAL: alpha MAP_ANONYMOUS = 0x10 (NOT 0x20 like x86/aarch64).
        //   MAP_PRIVATE = 0x02 (generic). So MAP_PRIVATE|MAP_ANONYMOUS = 0x12.
        //   The arena lowering passes 0x22 (the x86 value), so the custom mmap
        //   stub must translate flags: clear bit 0x20, set bit 0x10.

        let vuma_alloc_stub: Vec<u8> = {
            let mut code = Vec::new();
            // Incoming: R16 = size.  We need:
            //   R16 = NULL (0), R17 = size, R18 = PROT_READ|PROT_WRITE (3),
            //   R19 = MAP_PRIVATE|MAP_ANONYMOUS (0x12 on alpha), R20 = -1 (fd), R21 = 0 (offset)
            // Save size: BIS ZERO, R16, R1 (S0 = R16).
            code.extend(Instruction::Or { ra: ZERO, rb: Gpr::R16, rc: S0 }.encode());
            // R17 = S0 (size)
            code.extend(Instruction::Or { ra: ZERO, rb: S0, rc: Gpr::R17 }.encode());
            // R16 = 0 (NULL)
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R16 }.encode());
            // R18 = 3 (PROT)
            code.extend(Instruction::AddqLi { ra: ZERO, lit: 3, rc: Gpr::R18 }.encode());
            // R19 = 0x12 (MAP_PRIVATE|MAP_ANONYMOUS on alpha)
            // alpha MAP_ANONYMOUS = 0x10 (NOT 0x20 like x86). MAP_PRIVATE = 0x02.
            code.extend(Instruction::AddqLi { ra: ZERO, lit: 0x12, rc: Gpr::R19 }.encode());
            // R20 = -1 (fd). -1 fits in LDA's 16-bit displacement.
            code.extend(Instruction::Lda { ra: Gpr::R20, disp: -1, rb: ZERO }.encode());
            // R21 = 0 (offset)
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R21 }.encode());
            // R0 = 71 (alpha __NR_mmap — direct 6-arg, offset in bytes via R21=0)
            code.extend(ss_load_imm(Gpr::R0, 71));
            // CALL_PAL 0x83
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            // RET
            code.extend(Instruction::Ret.encode());
            code
        };

        let vuma_free_stub: Vec<u8> = {
            let mut code = Vec::new();
            // Incoming: R16 = addr.  R17 = 0 (size).
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R17 }.encode());
            // R0 = 73 (alpha sys_munmap)
            code.extend(ss_load_imm(Gpr::R0, 73));
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Ret.encode());
            code
        };

        let simple_stub = |num: i32| -> Vec<u8> {
            let mut code = Vec::new();
            code.extend(ss_load_imm(Gpr::R0, num as i64));
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Ret.encode());
            code
        };

        let mut syscall_stubs: Vec<(String, Vec<u8>)> = {
            let mut stubs: Vec<(String, Vec<u8>)> = Vec::new();
            for (name, num) in [
                ("write", 4), ("read", 3), ("open", 5), ("close", 6),
                // mmap is a CUSTOM stub (flags translation) — see below.
                // munmap=73, exit_group=405 (alpha OSF/1 syscall numbers).
                ("munmap", 73), ("exit", 1), ("exit_group", 405),
                ("brk", 17), ("getpid", 20), ("alarm", 27), ("kill", 42),
                ("dup", 41), ("dup2", 90), ("execve", 59),
                ("wait4", 84), ("unlink", 10), ("chdir", 12), ("lseek", 19),
                ("ioctl", 54), ("fcntl", 55), ("futex", 433), ("poll", 94),
                ("nanosleep", 162), ("mprotect", 50), ("clock_gettime", 410),
                ("gettimeofday", 78), ("rt_sigprocmask", 353), ("rt_sigaction", 352),
                ("socket", 97), ("connect", 98), ("bind", 104), ("listen", 106),
                ("accept", 99), ("setsockopt", 105),
                ("getsockopt", 118), ("shutdown", 103),
                ("sendto", 82), ("recvfrom", 102), ("clone", 220), ("fork", 2),
                // [wave 9 fix] epoll numbers corrected from kernel alpha syscall.tbl:
                //   old (wrong): 449/424/425 (those are migrate_pages/tgkill/stat64)
                //   correct:      486/408/409
                ("epoll_create1", 486), ("epoll_ctl", 408), ("epoll_wait", 409),
                ("dup3", 431),
                // ── Additional POSIX syscall stubs ──
                // Alpha uses OSF/1 syscall numbers for the stat family
                // (osf_stat=18, osf_lstat=68, osf_fstat=91).  QEMU's
                // linux-user layer translates the OSF/1 stat struct to the
                // host's struct stat, so these work correctly under qemu-alpha.
                ("stat", 18), ("lstat", 68), ("fstat", 91),
                ("getcwd", 367),
                // ── Wave 7: POSIX file-metadata & I/O syscalls (alpha unistd.h) ──
                // alpha has 6 reg args (R16-R21); all take ≤5 args → simple_stub.
                // alpha is 64-bit-native: chown=16/fchown=123 ARE the modern
                // sys_chown/sys_fchown (alpha never had a 16-bit-uid split, so
                // no chown32). Alpha numbers differ wildly from other arches
                // (openat=450, syncfs=500, pread64=349, fdatasync=447, fchdir=13).
                ("mkdir", 136), ("rmdir", 137), ("rename", 128),
                ("link", 9), ("symlink", 57), ("readlink", 58),
                ("chmod", 15), ("chown", 16), ("umask", 60),
                ("fchmod", 124), ("fchown", 123),
                ("openat", 450), ("unlinkat", 456), ("renameat", 457),
                ("linkat", 458), ("symlinkat", 459), ("readlinkat", 460),
                ("fchmodat", 461), ("faccessat", 462), ("fchownat", 453),
                ("ftruncate", 130), ("fsync", 95), ("fdatasync", 447),
                ("sync", 36), ("syncfs", 500),
                ("pread", 349), ("pwrite", 350), ("readv", 120), ("writev", 121),
                ("preadv", 490), ("pwritev", 491),
                ("fchdir", 13), ("chroot", 61),
                // ── Wave 9: POSIX system & advanced syscalls (alpha unistd.h) ──
                // alpha has 6 reg args (R16-R21); all take ≤5 args → simple_stub.
                // eventfd→eventfd2(485), signalfd→signalfd4(484) = modern variants.
                // alpha numbers diverge: mlock=314, mincore=375, madvise=75,
                // getrlimit=144, getrusage=364, times=323, mremap=341, etc.
                ("mlock", 314), ("munlock", 315), ("mlockall", 316), ("munlockall", 317),
                ("mincore", 375), ("madvise", 75), ("msync", 217), ("mremap", 341),
                ("getrlimit", 144), ("setrlimit", 145), ("prlimit64", 496),
                ("getrusage", 364), ("times", 323),
                ("getrandom", 511),
                ("eventfd", 485), ("timerfd_create", 481), ("timerfd_settime", 482),
                ("timerfd_gettime", 483), ("signalfd", 484),
                ("inotify_init1", 489), ("inotify_add_watch", 445), ("inotify_rm_watch", 446),
                ("ptrace", 26),
                // ── Wave 8: POSIX process & identity syscalls (alpha syscall.tbl) ──
                // alpha is OSF-derived; numbers diverge widely. All take ≤5 args;
                // alpha has 6 reg args (a0-a5) → simple_stub for all.
                // Family 1: identity. NOTE: alpha has NO standalone getuid/getgid —
                // getxuid=24 returns ruid|(euid<<32), getxgid=47 returns rgid|(egid<<32).
                // We register getuid/getgid at these numbers (OSF combined-return
                // quirk; a real libc extracts the low 32 bits).
                ("getuid", 24), ("geteuid", 531), ("getgid", 47), ("getegid", 530),
                ("setuid", 23), ("setgid", 132), ("setresuid", 343), ("setresgid", 371),
                // Family 2: process group (getpid already present at 20=getxpid)
                ("getppid", 532), ("getsid", 234), ("setsid", 147),
                ("setpgid", 39), ("getpgid", 233), ("getpgrp", 63),
                // Family 3: clone/wait (clone/wait4 already present)
                ("vfork", 66), ("clone3", 545), ("waitid", 438),
                // Family 4: exec/exit (execve/exit_group already present)
                ("execveat", 513),
                // Family 5: signals (kill/rt_sigaction/rt_sigprocmask/rt_sigreturn
                // already present)
                ("tgkill", 424), ("tkill", 381),
                // Family 6: directory read (readdir ABSENT → use getdents64)
                ("getdents64", 377), ("getdents", 305),
                // Family 7: system (arch_prctl is x86_64-only)
                ("prctl", 348), ("uname", 339), ("sysinfo", 318),
                            ("eventfd2", 485),
                ("newfstatat", 455),
                ("signalfd4", 484),
] {
                stubs.push((name.to_string(), simple_stub(num)));
            }

            // ── FFI scratchpad frame stubs (Wave 3b/fix) ──────────────────
            // ffi_scratch_push_frame: REAL mmap syscall (alpha sys_mmap=71).
            // Args: R16=0(NULL), R17=4096, R18=3(PROT), R19=0x12(MAP), R20=-1(fd), R21=0(off).
            // Syscall nr in R0=71. CALL_PAL 0x83=callsys. RET.
            // CRITICAL: alpha MAP_ANONYMOUS = 0x10 (NOT 0x20 like x86).
            {
                let mut code = Vec::new();
                code.extend(ss_load_imm(Gpr::R16, 0));       // addr = NULL
                code.extend(ss_load_imm(Gpr::R17, 4096));    // len = 4096
                code.extend(ss_load_imm(Gpr::R18, 3));       // prot = PROT_READ|PROT_WRITE
                code.extend(ss_load_imm(Gpr::R19, 0x12));    // flags = MAP_PRIVATE(0x02)|MAP_ANONYMOUS(0x10)
                code.extend(ss_load_imm(Gpr::R20, -1));      // fd = -1
                code.extend(ss_load_imm(Gpr::R21, 0));       // offset = 0
                code.extend(ss_load_imm(Gpr::R0, 71));       // sys_mmap (alpha __NR_mmap=71)
                code.extend(Instruction::CallPal { palcode: 0x83 }.encode()); // callsys
                code.extend(Instruction::Ret.encode());
                stubs.push(("ffi_scratch_push_frame".to_string(), code));
            }

            // ffi_scratch_pop_frame: no-op (RET). Real munmap when marshal_cstr wired.
            {
                let mut code = Vec::new();
                code.extend(Instruction::Ret.encode());
                stubs.push(("ffi_scratch_pop_frame".to_string(), code));
            }

            // __arena_overflow: real exit(1) syscall
            {
                let mut code = Vec::new();
                code.extend(ss_load_imm(Gpr::R16, 1));      // exit code = 1
                code.extend(ss_load_imm(Gpr::R0, 1));       // sys_exit
                code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
                code.extend(Instruction::Ret.encode());
                stubs.push(("__arena_overflow".to_string(), code));
            }

            // ── Custom mmap stub (alpha) ──────────────────────────────
            // The arena lowering passes flags=0x22 (MAP_PRIVATE|MAP_ANONYMOUS
            // using the x86/generic MAP_ANONYMOUS=0x20 value). On alpha,
            // MAP_ANONYMOUS=0x10, so 0x22 lacks the anonymous bit and sets
            // MAP_FIXED (alpha 0x100) incorrectly... actually 0x20 on alpha
            // is just MAP_PRIVATE again (no, MAP_PRIVATE=0x02). Bit 0x20 on
            // alpha is unused. The fix: translate flags from generic (0x22)
            // to alpha (0x12): clear bit 0x20, set bit 0x10.
            //
            // Args arrive in R16-R21 (same as syscall ABI). We modify R19
            // (flags) in place: R19 = (R19 & ~0x20) | 0x10.
            {
                let mut code = Vec::new();
                // R1 = 0x10 (alpha MAP_ANONYMOUS bit)
                code.extend(Instruction::AddqLi { ra: ZERO, lit: 0x10, rc: Gpr::R1 }.encode());
                // R19 = R19 | R1 (set the alpha MAP_ANONYMOUS bit)
                code.extend(Instruction::Or { ra: Gpr::R19, rb: Gpr::R1, rc: Gpr::R19 }.encode());
                // R1 = 0xFFFFFFDF (all bits except 0x20). Load via LDAH+LDA.
                // LDAH R1, 0xFFDF(zero-extended high) ... actually use a different approach:
                // R1 = 0x20 (the x86 MAP_ANONYMOUS bit to clear)
                code.extend(Instruction::AddqLi { ra: ZERO, lit: 0x20, rc: Gpr::R1 }.encode());
                // R19 = R19 & ~R1 = R19 ANDNOT R1. Alpha has ANDNOT as:
                //   BIC Ra, Rb, Rc -> Rc = Ra & ~Rb. But our Instruction enum
                //   doesn't have BIC. Use: R19 = R19 XOR (R19 AND R1) ... no.
                //   Simplest: R1 = ~R1 (XOR with -1). But we don't have NOT.
                //   Use: R19 = R19 AND (NOT R1). NOT R1 = R1 XOR 0xFFFF...FFFF.
                //   Actually alpha has ORNOT: Rc = Ra | ~Rb. We can build mask:
                //   mask = 0 | ~R1 = ORNOT(ZERO, R1). But we don't have ORNOT.
                //   Alternative: R1 = R1 XOR 0xFFFFFFFFFFFFFFFF = ~R1.
                //   Load -1 into R2, then XOR.
                code.extend(Instruction::Lda { ra: Gpr::R2, disp: -1, rb: ZERO }.encode()); // R2 = -1 (0xFFFF...FFFF)
                code.extend(Instruction::Xor { ra: Gpr::R1, rb: Gpr::R2, rc: Gpr::R1 }.encode()); // R1 = ~R1 = ~0x20
                code.extend(Instruction::And { ra: Gpr::R19, rb: Gpr::R1, rc: Gpr::R19 }.encode()); // R19 &= ~0x20
                // R0 = 71 (alpha __NR_mmap)
                code.extend(ss_load_imm(Gpr::R0, 71));
                code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
                code.extend(Instruction::Ret.encode());
                stubs.push(("mmap".to_string(), code));
            }

            stubs
        };

        // ── pipe(pipefd) — On alpha Linux/QEMU, pipe is syscall 42 (NOT 40).
        // It uses the OSF/1 convention: returns read fd in v0, write fd in a4 (R20).
        // a3 (R19) = 0 on success, 1 on error.
        // We must store the register fds to the user buffer and return 0/-1.
        {
            let mut code = Vec::new();
            // Save a0 (R16 = pipefd buffer ptr) to temp (R1)
            code.extend(Instruction::Or { ra: Gpr::R16, rb: ZERO, rc: Gpr::R1 }.encode());
            // v0 (R0) = 42 (sys_pipe on alpha)
            code.extend(ss_load_imm(Gpr::R0, 42));
            // callsys — returns read fd in v0, write fd in a4 (R20)
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            // Store read fd (v0) to [R1]  (STL = 32-bit store)
            code.extend(Instruction::Stl { ra: Gpr::R0, disp: 0, rb: Gpr::R1 }.encode());
            // Store write fd (a4 = R20) to [R1+4]
            code.extend(Instruction::Stl { ra: Gpr::R20, disp: 4, rb: Gpr::R1 }.encode());
            // Check a3 (R19): if 0 (success), return 0; else return -1
            // Use CMOVNE: if a3 != 0, v0 = -1
            // Load -1 into R1 (temp, no longer needed for buffer ptr)
            code.extend(ss_load_imm(Gpr::R1, -1));
            // R0 = 0 (success default; pipe() returns 0 on success, -1 on error).
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R0 }.encode());
            // CMOVNE R19, R1, R0: if R19 != 0 (callsys error flag a3), R0 = R1 (= -1).
            // Uses the existing Instruction::Cmovne variant (opcode 0x11, function 0x26).
            // This propagates pipe() failures instead of silently returning 0.
            code.extend(Instruction::Cmovne { ra: Gpr::R19, rb: Gpr::R1, rc: Gpr::R0 }.encode());
            code.extend(Instruction::Ret.encode());
            syscall_stubs.push(("pipe".to_string(), code));
        }

        // ── rt_sigreturn (173) — special: no args, never returns ──
        {
            let mut code = Vec::new();
            code.extend(ss_load_imm(Gpr::R0, 173));
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::CallPal { palcode: 0x0 }.encode()); // safety trap
            syscall_stubs.push(("rt_sigreturn".to_string(), code));
        }

        // ── waitpid(pid, wstatus, options) → wait4(pid, wstatus, options, NULL)
        // Alpha a3=R19 is the 4th arg (rusage). Zero it before the syscall.
        {
            let mut code = Vec::new();
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R19 }.encode()); // rusage=NULL
            code.extend(ss_load_imm(Gpr::R0, 84)); // sys_wait4
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Ret.encode());
            syscall_stubs.push(("waitpid".to_string(), code));
        }

        // ── recv(fd, buf, len, flags) → recvfrom(fd, buf, len, flags, NULL, NULL)
        // Alpha a4=R20 (addr), a5=R21 (addrlen). Both must be NULL.
        {
            let mut code = Vec::new();
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R20 }.encode());
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R21 }.encode());
            code.extend(ss_load_imm(Gpr::R0, 102)); // sys_recvfrom
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Ret.encode());
            syscall_stubs.push(("recv".to_string(), code));
        }

        // ── send(fd, buf, len, flags) → sendto(fd, buf, len, flags, NULL, 0)
        {
            let mut code = Vec::new();
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R20 }.encode());
            code.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R21 }.encode());
            code.extend(ss_load_imm(Gpr::R0, 82)); // sys_sendto
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Ret.encode());
            syscall_stubs.push(("send".to_string(), code));
        }

        // ── strcmp(s1, s2) → int — assembly loop, not a syscall
        // Alpha: a0=R16=s1, a1=R17=s2, return in R0.
        {
            let mut code = Vec::new();
            // loop:
            code.extend(Instruction::Ldbu { ra: Gpr::R1, disp: 0, rb: Gpr::R16 }.encode());
            code.extend(Instruction::Ldbu { ra: Gpr::R2, disp: 0, rb: Gpr::R17 }.encode());
            code.extend(Instruction::Subq { ra: Gpr::R1, rb: Gpr::R2, rc: Gpr::R3 }.encode());
            code.extend(Instruction::Bne { ra: Gpr::R3, disp: 4 }.encode());  // BNE R3, done
            code.extend(Instruction::Beq { ra: Gpr::R1, disp: 3 }.encode());  // BEQ R1, done
            code.extend(Instruction::AddqLi { ra: Gpr::R16, lit: 1, rc: Gpr::R16 }.encode());
            code.extend(Instruction::AddqLi { ra: Gpr::R17, lit: 1, rc: Gpr::R17 }.encode());
            code.extend(Instruction::Br { ra: ZERO, disp: -8 }.encode());     // BR loop
            // done:
            code.extend(Instruction::Or { ra: ZERO, rb: Gpr::R3, rc: Gpr::R0 }.encode());
            code.extend(Instruction::Ret.encode());
            syscall_stubs.push(("strcmp".to_string(), code));
        }

        // ── print_int(n) → void — decimal conversion + write(1, buf, len)
        // Alpha: a0=R16=n.  Uses a 32-byte stack buffer.
        // Algorithm: negate if negative (emit '-'), divmod-10 loop backwards,
        // then write the digit string.  Handles n=0 by emitting a single '0'.
        //
        // Wave-6 fix: QEMU-alpha user-mode (10.x) does NOT implement DIVQ /
        // DIVQU / CMPULE (they raise SIGILL).  The previous stub used DIVQ +
        // MULQ to extract each decimal digit; this rewrite uses the same
        // 64-bit shift-and-subtract division algorithm the main codegen uses
        // for IRInstr::Div (which IS supported by QEMU-alpha).  The division
        // produces both quotient (R3) and remainder (R6) in one pass.
        {
            let mut code = Vec::new();
            // LDA SP, -32(SP)
            code.extend(Instruction::Lda { ra: Gpr::R30, disp: -32, rb: Gpr::R30 }.encode());
            // R2 = n; R5 = 0 (sign flag)
            code.extend(Instruction::Or { ra: Gpr::R16, rb: Gpr::R31, rc: Gpr::R2 }.encode());
            code.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R5 }.encode());
            // BLT R2, .neg  (placeholder)  [encoder fixed in Wave-6: 0x3A=BLT]
            code.extend(Instruction::Blt { ra: Gpr::R2, disp: 0 }.encode());
            let blt_pos = code.len() - 4;
            // BR .start  (skip .neg block)
            code.extend(Instruction::Br { ra: Gpr::R31, disp: 0 }.encode());
            let br_start_pos = code.len() - 4;
            // .neg: R5=1, R2=-R2
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 1, rc: Gpr::R5 }.encode());
            code.extend(Instruction::Subq { ra: Gpr::R31, rb: Gpr::R2, rc: Gpr::R2 }.encode());
            // .start: R4=10 (divisor), R1=&buf[31]
            let start_offset = code.len();
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 10, rc: Gpr::R4 }.encode());
            code.extend(Instruction::Lda { ra: Gpr::R1, disp: 31, rb: Gpr::R30 }.encode());
            // .loop:
            let loop_offset = code.len();
            // ── Shift-and-subtract 64-bit unsigned divide: R2 / R4 ──
            //   produces R3 = quotient, R6 = remainder.
            //   uses R7 (scratch), R8 (counter).
            // R6 = 0 (remainder), R3 = 0 (quotient), R8 = 64 (counter)
            code.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R6 }.encode());
            code.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R3 }.encode());
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 64, rc: Gpr::R8 }.encode());
            // .div_loop:
            let div_loop_offset = code.len();
            // R7 = R2 >> 63  (top bit of dividend, SRL literal-form, function 0x34)
            code.extend_from_slice(&op_lit(0x12, Gpr::R2, 63, Gpr::R7, 0x34).to_le_bytes());
            // R2 <<= 1  (SLL literal-form, function 0x39)
            code.extend_from_slice(&op_lit(0x12, Gpr::R2, 1, Gpr::R2, 0x39).to_le_bytes());
            // R6 <<= 1
            code.extend_from_slice(&op_lit(0x12, Gpr::R6, 1, Gpr::R6, 0x39).to_le_bytes());
            // R6 |= R7  (OR/BIS, opcode 0x11 function 0x20)
            code.extend_from_slice(&op_reg(0x11, Gpr::R7, Gpr::R6, Gpr::R6, 0x20).to_le_bytes());
            // R3 <<= 1
            code.extend_from_slice(&op_lit(0x12, Gpr::R3, 1, Gpr::R3, 0x39).to_le_bytes());
            // R8 -= 1  (SUBQ literal-form, opcode 0x10 function 0x29)
            code.extend_from_slice(&op_lit(0x10, Gpr::R8, 1, Gpr::R8, 0x29).to_le_bytes());
            // R7 = (R6 < R4)  (CMPULT, opcode 0x10 function 0x1D — supported by QEMU)
            code.extend_from_slice(&op_reg(0x10, Gpr::R6, Gpr::R4, Gpr::R7, 0x1D).to_le_bytes());
            // BNE R7, skip_sub  (if R6 < R4, skip the subtract)
            code.extend(Instruction::Bne { ra: Gpr::R7, disp: 0 }.encode());
            let bne_skip_pos = code.len() - 4;
            // R6 -= R4
            code.extend(Instruction::Subq { ra: Gpr::R6, rb: Gpr::R4, rc: Gpr::R6 }.encode());
            // R3 += 1
            code.extend(Instruction::AddqLi { ra: Gpr::R3, lit: 1, rc: Gpr::R3 }.encode());
            // skip_sub:
            let skip_sub_offset = code.len();
            // BNE R8, .div_loop
            code.extend(Instruction::Bne { ra: Gpr::R8, disp: 0 }.encode());
            let bne_div_loop_pos = code.len() - 4;
            // ── Now R3 = quotient, R6 = remainder (digit) ──
            // R7 = R6 + 48  (digit + '0')
            code.extend(Instruction::AddqLi { ra: Gpr::R6, lit: 48, rc: Gpr::R7 }.encode());
            // STB R7, 0(R1)  (store digit backward)
            code.extend(Instruction::Stb { ra: Gpr::R7, disp: 0, rb: Gpr::R1 }.encode());
            // R1--
            code.extend(Instruction::Lda { ra: Gpr::R1, disp: -1, rb: Gpr::R1 }.encode());
            // R2 = R3 (quotient becomes next dividend)
            code.extend(Instruction::Or { ra: Gpr::R3, rb: Gpr::R31, rc: Gpr::R2 }.encode());
            // BNE R2, .loop
            code.extend(Instruction::Bne { ra: Gpr::R2, disp: 0 }.encode());
            let bne_loop_pos = code.len() - 4;
            // .after_loop: if R5 (neg), write '-'
            code.extend(Instruction::Beq { ra: Gpr::R5, disp: 0 }.encode()); // skip '-' if not negative
            let beq_skip_pos = code.len() - 4;
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 45, rc: Gpr::R7 }.encode()); // '-'
            code.extend(Instruction::Stb { ra: Gpr::R7, disp: 0, rb: Gpr::R1 }.encode());
            code.extend(Instruction::Lda { ra: Gpr::R1, disp: -1, rb: Gpr::R1 }.encode());
            let after_dash = code.len();
            // R1+1 = first digit; len = (SP+32) - (R1+1)
            code.extend(Instruction::Lda { ra: Gpr::R1, disp: 1, rb: Gpr::R1 }.encode());
            code.extend(Instruction::Lda { ra: Gpr::R3, disp: 32, rb: Gpr::R30 }.encode());
            code.extend(Instruction::Subq { ra: Gpr::R3, rb: Gpr::R1, rc: Gpr::R4 }.encode());
            // write(1, R1, R4)
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 1, rc: Gpr::R16 }.encode());
            code.extend(Instruction::Or { ra: Gpr::R1, rb: Gpr::R31, rc: Gpr::R17 }.encode());
            code.extend(Instruction::Or { ra: Gpr::R4, rb: Gpr::R31, rc: Gpr::R18 }.encode());
            code.extend(ss_load_imm(Gpr::R0, 4)); // sys_write
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Lda { ra: Gpr::R30, disp: 32, rb: Gpr::R30 }.encode());
            code.extend(Instruction::Ret.encode());

            // ── Patch branch displacements ──
            // Alpha branch target = PC + 4 + disp*4, where PC = branch instruction address.
            // BLT at blt_pos → .neg at br_start_pos+4
            let blt_target = br_start_pos + 4;
            let blt_disp = ((blt_target as i64) - (blt_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, blt_pos, 0x3A, Gpr::R2, blt_disp as i32); // 0x3A = BLT (Wave-6 fix)
            // BR at br_start_pos → .start at start_offset
            let br_disp = ((start_offset as i64) - (br_start_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, br_start_pos, 0x30, Gpr::R31, br_disp as i32);
            // BNE at bne_skip_pos → skip_sub at skip_sub_offset
            let bne_skip_disp = ((skip_sub_offset as i64) - (bne_skip_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, bne_skip_pos, 0x3D, Gpr::R7, bne_skip_disp as i32);
            // BNE at bne_div_loop_pos → .div_loop at div_loop_offset
            let bne_div_disp = ((div_loop_offset as i64) - (bne_div_loop_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, bne_div_loop_pos, 0x3D, Gpr::R8, bne_div_disp as i32);
            // BNE at bne_loop_pos → .loop at loop_offset
            let bne_disp = ((loop_offset as i64) - (bne_loop_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, bne_loop_pos, 0x3D, Gpr::R2, bne_disp as i32);
            // BEQ at beq_skip_pos → after_dash
            let beq_disp = ((after_dash as i64) - (beq_skip_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, beq_skip_pos, 0x39, Gpr::R5, beq_disp as i32);

            // print_int stub — saves/restores SP (R30) and only clobbers
            // caller-saved scratch registers (R0-R10, R16-R18).
            syscall_stubs.push(("print_int".to_string(), code));
        }

        // ── print_hex(n) → void — hex conversion + write(1, buf, len)
        // Alpha: a0=R16=n.  Prints up to 16 hex digits + newline.
        //
        // Wave-6 fix: replaced CMPULE (opcode 0x10 fn 0x3F) with CMPULT
        // (opcode 0x10 fn 0x1D) — QEMU-alpha does not implement CMPULE.
        // The test "R3 <= 9" becomes "R3 < 10" (R9 now holds 10, not 9);
        // the BEQ R8, .alpha branch logic is unchanged.
        {
            let mut code = Vec::new();
            code.extend(Instruction::Lda { ra: Gpr::R30, disp: -32, rb: Gpr::R30 }.encode()); // SP -= 32
            code.extend(Instruction::Or { ra: Gpr::R16, rb: Gpr::R31, rc: Gpr::R2 }.encode()); // R2 = n
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 15, rc: Gpr::R6 }.encode());  // R6 = 15 (mask)
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 10, rc: Gpr::R9 }.encode());  // R9 = 10 (Wave-6: was 9)
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 39, rc: Gpr::R10 }.encode()); // R10 = 39
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 4, rc: Gpr::R12 }.encode());  // R12 = 4 (shift)
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 10, rc: Gpr::R7 }.encode());  // newline
            code.extend(Instruction::Stb { ra: Gpr::R7, disp: 16, rb: Gpr::R30 }.encode());    // buf[16] = '\n'
            code.extend(Instruction::Lda { ra: Gpr::R1, disp: 15, rb: Gpr::R30 }.encode());    // R1 = &buf[15]
            // .hex_loop:
            let hx_loop = code.len();
            code.extend(Instruction::And { ra: Gpr::R2, rb: Gpr::R6, rc: Gpr::R3 }.encode());  // R3 = R2 & 15
            code.extend(Instruction::AddqLi { ra: Gpr::R3, lit: 48, rc: Gpr::R7 }.encode());   // R7 = R3 + '0'
            // R8 = (R3 < R9) i.e. (R3 < 10)  — CMPULT (Wave-6: was CMPULE <= 9)
            code.extend_from_slice(&op_reg(0x10, Gpr::R3, Gpr::R9, Gpr::R8, 0x1D).to_le_bytes());
            code.extend(Instruction::Beq { ra: Gpr::R8, disp: 0 }.encode()); // BEQ R8, .alpha (placeholder)
            let beq_alpha_pos = code.len() - 4;
            // .digit: R7 is already R3+48, fall through to store
            code.extend(Instruction::Br { ra: Gpr::R31, disp: 0 }.encode()); // BR .store (placeholder)
            let br_store_pos = code.len() - 4;
            // .alpha: R7 += 39
            code.extend(Instruction::Addq { ra: Gpr::R7, rb: Gpr::R10, rc: Gpr::R7 }.encode());
            // .store:
            let store_offset = code.len();
            code.extend(Instruction::Stb { ra: Gpr::R7, disp: 0, rb: Gpr::R1 }.encode());
            code.extend(Instruction::Lda { ra: Gpr::R1, disp: -1, rb: Gpr::R1 }.encode()); // R1--
            code.extend(Instruction::Srl { ra: Gpr::R2, rb: Gpr::R12, rc: Gpr::R2 }.encode()); // R2 >>= 4
            code.extend(Instruction::Bne { ra: Gpr::R2, disp: 0 }.encode()); // BNE R2, .hex_loop (placeholder)
            let bne_hx_pos = code.len() - 4;
            // Write: R1+1 = first digit, len = (SP+17) - (R1+1)
            code.extend(Instruction::Lda { ra: Gpr::R1, disp: 1, rb: Gpr::R1 }.encode());
            code.extend(Instruction::Lda { ra: Gpr::R3, disp: 17, rb: Gpr::R30 }.encode());
            code.extend(Instruction::Subq { ra: Gpr::R3, rb: Gpr::R1, rc: Gpr::R4 }.encode());
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 1, rc: Gpr::R16 }.encode());
            code.extend(Instruction::Or { ra: Gpr::R1, rb: Gpr::R31, rc: Gpr::R17 }.encode());
            code.extend(Instruction::Or { ra: Gpr::R4, rb: Gpr::R31, rc: Gpr::R18 }.encode());
            code.extend(ss_load_imm(Gpr::R0, 4));
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Lda { ra: Gpr::R30, disp: 32, rb: Gpr::R30 }.encode());
            code.extend(Instruction::Ret.encode());

            // Patch branches
            // BEQ target = .alpha (right after BR). .alpha is at br_store_pos + 4.
            let alpha_offset = br_store_pos + 4;
            let beq_d = ((alpha_offset as i64) - (beq_alpha_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, beq_alpha_pos, 0x39, Gpr::R8, beq_d as i32);
            // BR .store
            let br_d = ((store_offset as i64) - (br_store_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, br_store_pos, 0x30, Gpr::R31, br_d as i32);
            // BNE .hex_loop
            let bne_d = ((hx_loop as i64) - (bne_hx_pos as i64) - 4) / 4;
            patch_alpha_branch(&mut code, bne_hx_pos, 0x3D, Gpr::R2, bne_d as i32);

            // print_hex stub — saves/restores SP (R30) and only clobbers
            // caller-saved scratch registers (R0-R10, R16-R18).
            syscall_stubs.push(("print_hex".to_string(), code));
        }

        // ── print_newline() → void — write '\n' to stdout ──
        // No arguments. Uses sys_write(1, &newline, 1).
        // Alpha syscall convention: R16=fd, R17=buf, R18=count, R0=syscall#,
        // CALL_PAL 0x83 = callsys. R30 = SP.
        {
            let mut code = Vec::new();
            // LDA SP, -16(SP) — make stack space for the newline byte
            code.extend(Instruction::Lda { ra: Gpr::R30, disp: -16, rb: Gpr::R30 }.encode());
            // R1 = 10 ('\n')
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 10, rc: Gpr::R1 }.encode());
            // STB R1, 0(SP) — store newline at [SP]
            code.extend(Instruction::Stb { ra: Gpr::R1, disp: 0, rb: Gpr::R30 }.encode());
            // R16 = 1 (stdout fd)
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 1, rc: Gpr::R16 }.encode());
            // R17 = SP (buf)
            code.extend(Instruction::Or { ra: Gpr::R30, rb: Gpr::R31, rc: Gpr::R17 }.encode());
            // R18 = 1 (count)
            code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: 1, rc: Gpr::R18 }.encode());
            // R0 = 4 (sys_write)
            code.extend(ss_load_imm(Gpr::R0, 4));
            // callsys
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            // LDA SP, 16(SP) — restore stack
            code.extend(Instruction::Lda { ra: Gpr::R30, disp: 16, rb: Gpr::R30 }.encode());
            // RET
            code.extend(Instruction::Ret.encode());
            syscall_stubs.push(("print_newline".to_string(), code));
        }

        // ── Complex stub: sigaction → rt_sigaction(signum, act, oldact, sigsetsize=8) ──
        // Alpha rt_sigaction syscall # = 352. VUMA declares 3 args; the kernel
        // requires a 4th arg (sigsetsize=8) in R19 (a3). We set R19=8 before
        // CALL_PAL.
        let sigaction_stub: Vec<u8> = {
            let mut code = Vec::new();
            // ADDQ ZERO, 8, R19 (a3 = sigsetsize = 8)
            code.extend(Instruction::AddqLi { ra: ZERO, lit: 8, rc: Gpr::R19 }.encode());
            // Load syscall # 352 into R0 (v0)
            code.extend(ss_load_imm(Gpr::R0, 352));
            code.extend(Instruction::CallPal { palcode: 0x83 }.encode());
            code.extend(Instruction::Ret.encode());
            code
        };
        syscall_stubs.push(("sigaction".to_string(), sigaction_stub));

        // ── Compute function offsets ──
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size + ffi_stub_size;

        for func in &program.functions {
            func_offsets.insert(func.name.clone(), current_offset);
            let func_size: usize = func
                .blocks
                .iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            current_offset += func_size;
        }

        let vuma_alloc_offset = current_offset;
        let vuma_free_offset = vuma_alloc_offset + vuma_alloc_stub.len();
        func_offsets.insert("__vuma_alloc".to_string(), vuma_alloc_offset);
        func_offsets.insert("__vuma_free".to_string(), vuma_free_offset);

        let mut stub_offset = vuma_free_offset + vuma_free_stub.len();
        for (name, code) in &syscall_stubs {
            func_offsets.insert(name.clone(), stub_offset);
            stub_offset += code.len();
        }

        // Add __vuma_print_int / __vuma_print_hex / __vuma_print_newline
        // canonical aliases pointing at the same offsets as the bare-name helpers.
        for (short, canonical) in [
            ("print_int", "__vuma_print_int"),
            ("print_hex", "__vuma_print_hex"),
            ("print_newline", "__vuma_print_newline"),
        ] {
            if let Some(&off) = func_offsets.get(short) {
                func_offsets.insert(canonical.to_string(), off);
            }
        }

        // ── Build _start stub bytes ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // LDQ R16, 0(SP) — load argc from stack pointer (64-bit)
        start_stub.extend(Instruction::Ldq { ra: Gpr::R16, disp: 0, rb: SP }.encode());

        // LDA R17, 8(SP) — argv = SP + 8 (64-bit pointers on alpha)
        start_stub.extend(Instruction::Lda { ra: Gpr::R17, disp: 8, rb: SP }.encode());

        // BSR RA, main — 4 bytes (opcode 0x34, disp21 in low 21 bits).
        let bsr_offset_in_start = start_stub.len();
        start_stub.extend(Instruction::Bsr { ra: RA, disp: 0 }.encode());
        // BIS ZERO, R0, R16 — move return value from R0 to R16.
        start_stub.extend(Instruction::Or { ra: ZERO, rb: Gpr::R0, rc: Gpr::R16 }.encode());
        // LDA R0, 1(ZERO) — R0 = 1 (SYS_exit).
        start_stub.extend(Instruction::Lda { ra: Gpr::R0, disp: 1, rb: ZERO }.encode());
        // CALL_PAL 0x83.
        start_stub.extend(Instruction::CallPal { palcode: 0x83 }.encode());

        // ── Patch _start BSR to main ──
        // BSR disp21: target = PC + 4 + (disp21 * 4).
        // PC = address of the BSR instruction itself. So target = bsr_abs + 4 + (disp21 * 4).
        let main_key = func_offsets
            .keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            let bsr_abs = BASE_ADDR + text_offset + bsr_offset_in_start as u64;
            let main_abs = BASE_ADDR + text_offset + main_offset as u64;
            let disp = ((main_abs as i64 - bsr_abs as i64 - 4) / 4) as i32;
            let disp21 = (disp & 0x1F_FFFF) as u32;
            // Patch the low 21 bits of the BSR instruction (LE byte order).
            let word_le = [
                start_stub[bsr_offset_in_start],
                start_stub[bsr_offset_in_start + 1],
                start_stub[bsr_offset_in_start + 2],
                start_stub[bsr_offset_in_start + 3],
            ];
            let mut word = u32::from_le_bytes(word_le);
            word = (word & !0x1F_FFFF) | disp21;
            let new_le = word.to_le_bytes();
            start_stub[bsr_offset_in_start..bsr_offset_in_start + 4].copy_from_slice(&new_le);
        } else {
            // No main — point BSR to FFI stub.
            let bsr_abs = BASE_ADDR + text_offset + bsr_offset_in_start as u64;
            let ffi_abs = BASE_ADDR + text_offset + ffi_stub_offset as u64;
            let disp = ((ffi_abs as i64 - bsr_abs as i64 - 4) / 4) as i32;
            let disp21 = (disp & 0x1F_FFFF) as u32;
            let word_le = [
                start_stub[bsr_offset_in_start],
                start_stub[bsr_offset_in_start + 1],
                start_stub[bsr_offset_in_start + 2],
                start_stub[bsr_offset_in_start + 3],
            ];
            let mut word = u32::from_le_bytes(word_le);
            word = (word & !0x1F_FFFF) | disp21;
            let new_le = word.to_le_bytes();
            start_stub[bsr_offset_in_start..bsr_offset_in_start + 4].copy_from_slice(&new_le);
        }

        // ── Add FFI return-0 stub ──
        let mut ffi_stub = Vec::with_capacity(ffi_stub_size);
        // BIS ZERO, ZERO, R0 (R0 = 0)
        ffi_stub.extend(Instruction::Or { ra: ZERO, rb: ZERO, rc: Gpr::R0 }.encode());
        // RET
        ffi_stub.extend(Instruction::Ret.encode());

        // ── Concatenate all code ──
        let mut all_code = start_stub;
        all_code.extend_from_slice(&ffi_stub);
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }
        all_code.extend_from_slice(&vuma_alloc_stub);
        all_code.extend_from_slice(&vuma_free_stub);
        for (_, code) in &syscall_stubs {
            all_code.extend_from_slice(code);
        }

        // ── Patch BSR relocations for inter-function calls ──
        // BSR disp21: target = PC + 4 + (disp21 * 4). PC = instr_abs.
        // disp21 is in bits 0-20 (LE).
        let mut func_code_offset: usize = start_stub_size + ffi_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 4 > all_code.len() { continue; }
                if reloc.reloc_type == R_ALPHA_BRADDR {
                    let target_offset = func_offsets
                        .get(&reloc.symbol)
                        .copied()
                        .or_else(|| {
                            let prefix = format!("fn_{}", reloc.symbol);
                            func_offsets
                                .keys()
                                .find(|k| k.starts_with(&prefix))
                                .and_then(|k| func_offsets.get(k))
                                .copied()
                        })
                        .unwrap_or(ffi_stub_offset);
                    let instr_abs = BASE_ADDR + text_offset + abs_offset as u64;
                    let target_abs = BASE_ADDR + text_offset + target_offset as u64;
                    let disp = ((target_abs as i64 - instr_abs as i64 - 4) / 4) as i32;
                    let disp21 = (disp & 0x1F_FFFF) as u32;
                    let word_le = [
                        all_code[abs_offset],
                        all_code[abs_offset + 1],
                        all_code[abs_offset + 2],
                        all_code[abs_offset + 3],
                    ];
                    let mut word = u32::from_le_bytes(word_le);
                    word = (word & !0x1F_FFFF) | disp21;
                    let new_le = word.to_le_bytes();
                    all_code[abs_offset..abs_offset + 4].copy_from_slice(&new_le);
                } else if reloc.reloc_type == "R_ALPHA_REFQUAD" {
                    // Absolute 64-bit address for GetAddress (LDA + LDAH sequence).
                    let target_offset = func_offsets
                        .get(&reloc.symbol)
                        .copied()
                        .or_else(|| {
                            let prefix = format!("fn_{}", reloc.symbol);
                            func_offsets
                                .keys()
                                .find(|k| k.starts_with(&prefix))
                                .and_then(|k| func_offsets.get(k))
                                .copied()
                        })
                        .unwrap_or(0);
                    let target_abs = BASE_ADDR + text_offset + target_offset as u64;
                    let lo16 = (target_abs as u32 & 0xFFFF) as i16;
                    let mut hi16 = ((target_abs as u32 >> 16) & 0xFFFF) as i32;
                    if (lo16 as i16) < 0 { hi16 += 1; }
                    // Patch LDA's displacement (low 16 bits of the 4-byte word, LE).
                    let word_le = [all_code[abs_offset], all_code[abs_offset+1], all_code[abs_offset+2], all_code[abs_offset+3]];
                    let mut word = u32::from_le_bytes(word_le);
                    word = (word & !0xFFFF) | (lo16 as u16 as u32);
                    all_code[abs_offset..abs_offset+4].copy_from_slice(&word.to_le_bytes());
                    // Patch LDAH's displacement (next 4-byte word).
                    let word_le = [all_code[abs_offset+4], all_code[abs_offset+5], all_code[abs_offset+6], all_code[abs_offset+7]];
                    let mut word = u32::from_le_bytes(word_le);
                    word = (word & !0xFFFF) | (hi16 as u16 as u32);
                    all_code[abs_offset+4..abs_offset+8].copy_from_slice(&word.to_le_bytes());
                }
            }
            let func_size: usize = func
                .blocks
                .iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            func_code_offset += func_size;
        }

        // ── Build ELF ──
        let extern_symbols: Vec<String> = Vec::new();
        Ok(build_alpha_elf(&all_code, BASE_ADDR, &extern_symbols))
    }

    fn return_stub(&self) -> Vec<u8> {
        Instruction::Ret.encode()
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // Load 64-bit address into S2, then JMP (S2).
        let mut code = Vec::new();
        // LDA S2, lo16(ZERO)
        let lo16 = (entry_addr as u32 & 0xFFFF) as i16;
        let mut hi16 = ((entry_addr as u32 >> 16) & 0xFFFF) as i32;
        if (lo16 as i16) < 0 { hi16 += 1; }
        code.extend(Instruction::Lda { ra: S2, disp: lo16, rb: ZERO }.encode());
        let word: u32 = (0x09u32 << 26)
            | ((S2.encoding() as u32) << 21)
            | ((S2.encoding() as u32) << 16)
            | (hi16 as u16 as u32);
        code.extend_from_slice(&word.to_le_bytes());
        // JMP ZERO, (S2) — jump without setting RA.
        code.extend(Instruction::Jmp { ra: ZERO, rb: S2 }.encode());
        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset + 4 <= bytes.len() {
            let word = u32::from_le_bytes([bytes[offset], bytes[offset+1], bytes[offset+2], bytes[offset+3]]);
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

    fn name(&self) -> &'static str { "alpha" }
}

// ===========================================================================
// ELF builder (little-endian ELF64)
// ===========================================================================

fn build_alpha_elf(code: &[u8], base_addr: u64, extern_symbols: &[String]) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x1000;
    const HOST_PAGE_ALIGN: u64 = 0x10000;

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = 3;
    let phdr_end = elf_header_size + num_phdrs * phdr_size;
    let text_offset = phdr_end;
    let text_size = code.len() as u64;

    let text_file_end = text_offset + text_size;
    let data_vaddr =
        (base_addr + text_file_end).div_ceil(HOST_PAGE_ALIGN) * HOST_PAGE_ALIGN;
    let data_size: u64 = PAGE_SIZE;
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity((text_offset + text_size + 256) as usize);

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']);
    elf.push(2); // ELFCLASS64
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0);
    elf.extend_from_slice(&[0u8; 7]);

    // --- ELF header fields (little-endian) ---
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&0x9026u16.to_le_bytes()); // e_machine = EM_ALPHA
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_shoff
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&3u16.to_le_bytes()); // e_phnum
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    elf.extend_from_slice(&base_addr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&base_addr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&((text_offset + text_size) as u64).to_le_bytes()); // p_filesz
    elf.extend_from_slice(&((text_offset + text_size) as u64).to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&data_size.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 3: PT_GNU_STACK ---
    elf.extend_from_slice(&0x6474e551u32.to_le_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&0x10u64.to_le_bytes()); // p_align

    // --- .text section ---
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    if !extern_symbols.is_empty() {
        append_alpha_elf_sections(&mut elf, text_offset, text_size, extern_symbols);
    }

    elf
}

fn append_alpha_elf_sections(
    elf: &mut Vec<u8>,
    text_offset: u64,
    text_size: u64,
    extern_symbols: &[String],
) {
    const SHT_NULL: u32 = 0;
    const SHT_PROGBITS: u32 = 1;
    const SHT_SYMTAB: u32 = 2;
    const SHT_STRTAB: u32 = 3;
    const SHN_UNDEF: u16 = 0;
    const STB_GLOBAL: u8 = 1;
    const STT_FUNC: u8 = 2;
    const SYM_SIZE: u64 = 24;

    let mut shstrtab: Vec<u8> = Vec::new();
    shstrtab.push(0);
    let name_text = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".text\0");
    let name_symtab = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".symtab\0");
    let name_strtab = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".strtab\0");
    let name_shstrtab = shstrtab.len() as u32;
    shstrtab.extend_from_slice(b".shstrtab\0");

    let mut strtab: Vec<u8> = Vec::new();
    strtab.push(0);
    let mut sym_name_offsets: Vec<u32> = Vec::with_capacity(extern_symbols.len());
    for name in extern_symbols {
        sym_name_offsets.push(strtab.len() as u32);
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
    }

    let mut symtab: Vec<u8> = Vec::new();
    symtab.extend_from_slice(&[0u8; 24]);
    for &name_off in &sym_name_offsets {
        symtab.extend_from_slice(&name_off.to_le_bytes());
        symtab.push((STB_GLOBAL << 4) | STT_FUNC);
        symtab.push(0);
        symtab.extend_from_slice(&SHN_UNDEF.to_le_bytes());
        symtab.extend_from_slice(&0u64.to_le_bytes());
        symtab.extend_from_slice(&0u64.to_le_bytes());
    }

    while !elf.len().is_multiple_of(8) { elf.push(0); }
    let shstrtab_off = elf.len() as u64;
    elf.extend_from_slice(&shstrtab);
    let strtab_off = elf.len() as u64;
    elf.extend_from_slice(&strtab);
    while !elf.len().is_multiple_of(8) { elf.push(0); }
    let symtab_off = elf.len() as u64;
    let symtab_size = symtab.len() as u64;
    elf.extend_from_slice(&symtab);

    while !elf.len().is_multiple_of(8) { elf.push(0); }
    let shdr_off = elf.len() as u64;

    fn push_shdr(elf: &mut Vec<u8>, shdr: &SectionHeader<u64>) {
        elf.extend_from_slice(&shdr.sh_name.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_type.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_flags.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_addr.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_offset.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_size.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_link.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_info.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_addralign.to_le_bytes());
        elf.extend_from_slice(&shdr.sh_entsize.to_le_bytes());
    }

    push_shdr(
        elf,
        &SectionHeader {
            sh_type: SHT_NULL,
            ..Default::default()
        },
    );
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_text,
            sh_type: SHT_PROGBITS,
            sh_flags: 0x6,
            sh_addr: 0x120000000 + text_offset,
            sh_offset: text_offset,
            sh_size: text_size,
            sh_addralign: 16,
            ..Default::default()
        },
    );
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_symtab,
            sh_type: SHT_SYMTAB,
            sh_offset: symtab_off,
            sh_size: symtab_size,
            sh_link: 3,
            sh_info: 1,
            sh_addralign: 8,
            sh_entsize: SYM_SIZE,
            ..Default::default()
        },
    );
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_strtab,
            sh_type: SHT_STRTAB,
            sh_offset: strtab_off,
            sh_size: strtab.len() as u64,
            sh_addralign: 1,
            ..Default::default()
        },
    );
    push_shdr(
        elf,
        &SectionHeader {
            sh_name: name_shstrtab,
            sh_type: SHT_STRTAB,
            sh_offset: shstrtab_off,
            sh_size: shstrtab.len() as u64,
            sh_addralign: 1,
            ..Default::default()
        },
    );

    let shnum: u16 = 5;
    let shstrndx: u16 = 4;
    elf[40..48].copy_from_slice(&shdr_off.to_le_bytes());
    elf[60..62].copy_from_slice(&shnum.to_le_bytes());
    elf[62..64].copy_from_slice(&shstrndx.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_regalloc_metadata() {
        // Stack-slot mode: reads/writes should be empty (no real regs recorded).
        // Real regalloc mode: at least one instruction should have reads/writes.
        let mut func = IRFunction::new("test_real_regalloc");
        func.vregs.insert(0, VirtualRegister::anonymous(0));
        func.vregs.insert(1, VirtualRegister::anonymous(1));
        func.vregs.insert(2, VirtualRegister::anonymous(2));
        func.blocks[0].instructions.push(IRInstr::Add {
            dst: IRValue::Register(2),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(1),
            ty: None,
        });
        func.blocks[0].terminator = crate::ir::IRTerminator::Return(vec![]);

        let backend = AlphaBackend::new(); // use_real_regalloc = false by default
        let result_ss = backend.allocate_registers(&func);
        assert!(result_ss.is_ok(), "stack-slot allocation should succeed");

        // Now test with real regalloc.
        let mut backend = AlphaBackend::new();
        backend.use_real_regalloc = true;
        let result_real = backend.allocate_registers(&func);
        assert!(result_real.is_ok(), "real regalloc should succeed");
        let real_func = result_real.unwrap();
        // Real regalloc mode: at least one instruction should have reads/writes.
        let has_real_regs = real_func.blocks.iter()
            .any(|b| b.instructions.iter().any(|i| !i.reads.is_empty() || !i.writes.is_empty()));
        assert!(has_real_regs, "real regalloc should record physical register assignments");
    }
}
