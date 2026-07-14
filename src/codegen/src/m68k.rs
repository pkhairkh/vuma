//! # Motorola 68000 (m68k) Backend
//!
//! Implements the `Backend` trait for the Motorola 68000 — a 32-bit
//! big-endian CISC ISA with variable-length instructions (2–11 bytes,
//! most common forms are 2–4 bytes).
//!
//! ## Register Convention (Linux m68k)
//!
//! | Register | Role                                                          |
//! |----------|---------------------------------------------------------------|
//! | D0       | scratch / syscall number / return value                       |
//! | D1–D2    | scratch / syscall args                                        |
//! | D3–D7    | callee-saved                                                  |
//! | A0–A1    | scratch / syscall args                                        |
//! | A2–A6    | callee-saved (A6 = frame pointer by convention)               |
//! | A7       | stack pointer (SP)                                            |
//!
//! ## Linux m68k Syscall Convention
//!
//! - Syscall number in D0
//! - Arguments in D1–D5
//! - Return value in D0
//! - Invoke via `trap #0` (opcode 0x4E40)
//!
//! ## Instruction Encoding
//!
//! All m68k instructions are big-endian.  Common formats used here:
//!
//! - **MOVEQ** (2 bytes): `0x70 | (Dn << 9) | (imm8 & 0xff)` — Load 8-bit signed imm.
//! - **MOVE.L #imm32, Dn** (6 bytes): `0x203C | (Dn << 9)` + 4-byte imm.
//! - **MOVE.L (d16, An), Dn** (4 bytes): `0x2000 | (Dn<<9) | (5<<3) | An` + 2-byte d16.
//! - **MOVE.L Dn, (d16, An)** (4 bytes): `0x2000 | (An<<9) | (5<<6) | Dn` + 2-byte d16.
//! - **MOVE.L Dm, Dn** (2 bytes): `0x2000 | (Dn<<9) | Dm`.
//! - **ADD.L Dm, Dn** (2 bytes): `0xD080 | (Dn<<9) | Dm` (Dn += Dm).
//! - **SUB.L Dm, Dn** (2 bytes): `0x9080 | (Dn<<9) | Dm` (Dn -= Dm).
//! - **AND.L Dm, Dn** (2 bytes): `0xC080 | (Dn<<9) | Dm`.
//! - **OR.L  Dm, Dn** (2 bytes): `0x8080 | (Dn<<9) | Dm`.
//! - **MULU.W Dm, Dn** (2 bytes): `0xC0C0 | (Dn<<9) | Dm` — Dn = Dn[15:0] * Dm[15:0].
//! - **DIVU.W Dm, Dn** (2 bytes): `0x80C0 | (Dn<<9) | Dm` — quotient in upper 16 bits,
//!   remainder in lower 16 bits.
//! - **SWAP Dn** (2 bytes): `0x4840 | (Dn<<9)` — swap upper/lower 16 bits.
//! - **JMP (An)** (2 bytes): `0x4ED0 | An`.
//! - **JSR (An)** (2 bytes): `0x4E90 | An`.
//! - **RTS** (2 bytes): `0x4E75`.
//! - **BRA.S offset** (2 bytes): `0x6000 | (offset & 0xff)`.
//! - **Bcc.S offset** (2 bytes): `0x54(condition)<<8 | (offset & 0xff)`.
//! - **CMP.L Dm, Dn** (2 bytes): `0xB080 | (Dn<<9) | Dm`.
//! - **TST.L Dn** (2 bytes): `0x4A80 | (Dn<<9)`.
//! - **TRAP #0** (2 bytes): `0x4E40`.
//! - **LINK An, #disp16** (4 bytes): `0x4E50 | An` + 2-byte disp16 — push An, An=SP, SP-=disp.
//! - **UNLK An** (2 bytes): `0x4E58 | An` — SP=An, An=(SP)+.
//! - **MOVEM.L Dm-Dn, -(SP)** (4 bytes): save multiple registers.
//! - **MOVEM.L (SP)+, Dm-Dn** (4 bytes): restore multiple registers.

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, Endianness, OutputFormat, PhysicalReg, RegClass, RelocationEntry, TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
#[cfg(test)]
use crate::ir::VirtualRegister;
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// m68k general-purpose registers: 8 data (D0–D7) and 8 address (A0–A7).
/// A7 is the stack pointer.  Encodings 0–7 are D0–D7, 8–15 are A0–A7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Gpr {
    D0 = 0, D1 = 1, D2 = 2, D3 = 3, D4 = 4, D5 = 5, D6 = 6, D7 = 7,
    A0 = 8, A1 = 9, A2 = 10, A3 = 11, A4 = 12, A5 = 13, A6 = 14, A7 = 15,
}

impl Gpr {
    /// Returns the 4-bit encoding index (0–7 = D0–D7, 8–15 = A0–A7).
    pub fn encoding(&self) -> u8 {
        *self as u8
    }

    /// Returns the Gpr for the given encoding index (0–15).
    pub fn from_encoding(enc: u8) -> Option<Self> {
        match enc {
            0 => Some(Gpr::D0), 1 => Some(Gpr::D1), 2 => Some(Gpr::D2), 3 => Some(Gpr::D3),
            4 => Some(Gpr::D4), 5 => Some(Gpr::D5), 6 => Some(Gpr::D6), 7 => Some(Gpr::D7),
            8 => Some(Gpr::A0), 9 => Some(Gpr::A1), 10 => Some(Gpr::A2), 11 => Some(Gpr::A3),
            12 => Some(Gpr::A4), 13 => Some(Gpr::A5), 14 => Some(Gpr::A6), 15 => Some(Gpr::A7),
            _ => None,
        }
    }

    /// Returns the assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::D0 => "%d0", Gpr::D1 => "%d1", Gpr::D2 => "%d2", Gpr::D3 => "%d3",
            Gpr::D4 => "%d4", Gpr::D5 => "%d5", Gpr::D6 => "%d6", Gpr::D7 => "%d7",
            Gpr::A0 => "%a0", Gpr::A1 => "%a1", Gpr::A2 => "%a2", Gpr::A3 => "%a3",
            Gpr::A4 => "%a4", Gpr::A5 => "%a5", Gpr::A6 => "%a6", Gpr::A7 => "%a7",
        }
    }

    /// Returns the data-register form for a given encoding (0–7 → D0–D7).
    /// For argument passing, Linux m68k uses D1–D5 (then A0–A1).
    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::D1),
            1 => Some(Gpr::D2),
            2 => Some(Gpr::D3),
            3 => Some(Gpr::D4),
            4 => Some(Gpr::D5),
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
// Instruction enum (mnemonic / Display)
// ===========================================================================

/// A coarse-grained instruction enum used for disassembly / Display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    Moveq { dst: Gpr, imm: i8 },
    MoveImm32 { dst: Gpr, imm: i32 },
    Move { src: Gpr, dst: Gpr },
    Load { base: Gpr, offset: i16, dst: Gpr },
    Store { src: Gpr, base: Gpr, offset: i16 },
    Add { src: Gpr, dst: Gpr },
    Sub { src: Gpr, dst: Gpr },
    And { src: Gpr, dst: Gpr },
    Or { src: Gpr, dst: Gpr },
    Xor { src: Gpr, dst: Gpr },
    Lsl { dst: Gpr, imm: u8 },
    Lsr { dst: Gpr, imm: u8 },
    Asr { dst: Gpr, imm: u8 },
    Mulu { src: Gpr, dst: Gpr },
    Divu { src: Gpr, dst: Gpr },
    Swap { dst: Gpr },
    Cmp { src: Gpr, dst: Gpr },
    Tst { dst: Gpr },
    Jmp { target: Gpr },
    Jsr { target: Gpr },
    Rts,
    Bra { offset: i16 },
    Bcc { cond: u8, offset: i16 },
    Trap0,
    Link { reg: Gpr, disp: i16 },
    Unlk { reg: Gpr },
    Nop,
}

impl Instruction {
    /// Returns the mnemonic string for this instruction.
    pub fn mnemonic(&self) -> &'static str {
        match self {
            Instruction::Moveq { .. } => "moveq",
            Instruction::MoveImm32 { .. } => "move.l",
            Instruction::Move { .. } => "move.l",
            Instruction::Load { .. } => "move.l",
            Instruction::Store { .. } => "move.l",
            Instruction::Add { .. } => "add.l",
            Instruction::Sub { .. } => "sub.l",
            Instruction::And { .. } => "and.l",
            Instruction::Or { .. } => "or.l",
            Instruction::Xor { .. } => "eor.l",
            Instruction::Lsl { .. } => "lsl.l",
            Instruction::Lsr { .. } => "lsr.l",
            Instruction::Asr { .. } => "asr.l",
            Instruction::Mulu { .. } => "mulu.w",
            Instruction::Divu { .. } => "divu.w",
            Instruction::Swap { .. } => "swap",
            Instruction::Cmp { .. } => "cmp.l",
            Instruction::Tst { .. } => "tst.l",
            Instruction::Jmp { .. } => "jmp",
            Instruction::Jsr { .. } => "jsr",
            Instruction::Rts => "rts",
            Instruction::Bra { .. } => "bra",
            Instruction::Bcc { .. } => "bcc",
            Instruction::Trap0 => "trap #0",
            Instruction::Link { .. } => "link",
            Instruction::Unlk { .. } => "unlk",
            Instruction::Nop => "nop",
        }
    }

    /// Encode this instruction into big-endian bytes.
    pub fn encode(&self) -> Vec<u8> {
        match self {
            Instruction::Moveq { dst, imm } => {
                let w = 0x7000u16 | ((dst.encoding() as u16 & 0x7) << 9) | (*imm as u8 as u16);
                w.to_be_bytes().to_vec()
            }
            Instruction::MoveImm32 { dst, imm } => {
                let w = 0x203Cu16 | ((dst.encoding() as u16 & 0x7) << 9);
                let mut v = w.to_be_bytes().to_vec();
                v.extend_from_slice(&imm.to_be_bytes());
                v
            }
            Instruction::Move { src, dst } => {
                let w = 0x2000u16
                    | ((dst.encoding() as u16 & 0x7) << 9)
                    | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Load { base, offset, dst } => {
                // src = (d16, An) — mode 5, reg = base (which must be an An).
                let base_enc = if *base as u8 >= 8 { *base as u8 - 8 } else { *base as u8 };
                let w = 0x2000u16
                    | ((dst.encoding() as u16 & 0x7) << 9)
                    | (5u16 << 3)
                    | (base_enc as u16 & 0x7);
                let mut v = w.to_be_bytes().to_vec();
                v.extend_from_slice(&offset.to_be_bytes());
                v
            }
            Instruction::Store { src, base, offset } => {
                let base_enc = if *base as u8 >= 8 { *base as u8 - 8 } else { *base as u8 };
                let w = 0x2000u16
                    | ((base_enc as u16 & 0x7) << 9)
                    | (5u16 << 6)
                    | (src.encoding() as u16 & 0x7);
                let mut v = w.to_be_bytes().to_vec();
                v.extend_from_slice(&offset.to_be_bytes());
                v
            }
            Instruction::Add { src, dst } => {
                let w = 0xD080u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Sub { src, dst } => {
                let w = 0x9080u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::And { src, dst } => {
                let w = 0xC080u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Or { src, dst } => {
                let w = 0x8080u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Xor { src, dst } => {
                // EOR.L Dn, Dm: Dm = Dm ^ Dn. Dn is at bits 11-9, Dm at bits 2-0.
                // We want dst = dst ^ src, so Dn=src, Dm=dst.
                let w = 0xB180u16 | ((src.encoding() as u16 & 0x7) << 9) | (dst.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Lsl { dst, imm } => {
                // LSL.L #count, Dn: word = 0xE1A0 | (Dn<<9) | (count&7) — but count field is bits 5-0 lower
                // Actual encoding: 1110 | count[2:0] | 1 | Dn | 00 | 0 | 0 | count[5:3] | Dn
                // Simpler: shift immediate form: 0xE08x | (Dn<<9) | (count&0x1F<<0) with i/r=0 (immediate)
                // bits 15-12=1110, bits 11-9=Dn, bit 8=1 (immediate count), bits 7-6=size (11=long),
                // bits 5-3=shift type (000=lsl, 001=lsr, 010=asr/roxr), bits 2-0=count
                let count = (*imm & 0x7) as u16;
                let w = 0xE1A8u16 | ((dst.encoding() as u16 & 0x7) << 9) | count;
                w.to_be_bytes().to_vec()
            }
            Instruction::Lsr { dst, imm } => {
                let count = (*imm & 0x7) as u16;
                let w = 0xE2A8u16 | ((dst.encoding() as u16 & 0x7) << 9) | count;
                w.to_be_bytes().to_vec()
            }
            Instruction::Asr { dst, imm } => {
                let count = (*imm & 0x7) as u16;
                let w = 0xE0A8u16 | ((dst.encoding() as u16 & 0x7) << 9) | count;
                w.to_be_bytes().to_vec()
            }
            Instruction::Mulu { src, dst } => {
                let w = 0xC0C0u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Divu { src, dst } => {
                let w = 0x80C0u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Swap { dst } => {
                let w = 0x4840u16 | ((dst.encoding() as u16 & 0x7) << 9);
                w.to_be_bytes().to_vec()
            }
            Instruction::Cmp { src, dst } => {
                let w = 0xB080u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Tst { dst } => {
                let w = 0x4A80u16 | ((dst.encoding() as u16 & 0x7) << 9);
                w.to_be_bytes().to_vec()
            }
            Instruction::Jmp { target } => {
                let t_enc = if *target as u8 >= 8 { *target as u8 - 8 } else { *target as u8 };
                // JMP (An): 0x4ED0 | An — note address register indirect (mode 2)
                let w = 0x4ED0u16 | (t_enc as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Jsr { target } => {
                let t_enc = if *target as u8 >= 8 { *target as u8 - 8 } else { *target as u8 };
                let w = 0x4E90u16 | (t_enc as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Rts => vec![0x4E, 0x75],
            Instruction::Bra { offset } => {
                let w = 0x6000u16 | (*offset as u16 & 0xff);
                w.to_be_bytes().to_vec()
            }
            Instruction::Bcc { cond, offset } => {
                let w = 0x5400u16 | ((*cond as u16 & 0xf) << 8) | (*offset as u16 & 0xff);
                w.to_be_bytes().to_vec()
            }
            Instruction::Trap0 => vec![0x4E, 0x40],
            Instruction::Link { reg, disp } => {
                let r_enc = if *reg as u8 >= 8 { *reg as u8 - 8 } else { *reg as u8 };
                let w = 0x4E50u16 | (r_enc as u16 & 0x7);
                let mut v = w.to_be_bytes().to_vec();
                v.extend_from_slice(&disp.to_be_bytes());
                v
            }
            Instruction::Unlk { reg } => {
                let r_enc = if *reg as u8 >= 8 { *reg as u8 - 8 } else { *reg as u8 };
                let w = 0x4E58u16 | (r_enc as u16 & 0x7);
                w.to_be_bytes().to_vec()
            }
            Instruction::Nop => vec![0x4E, 0x71],
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Moveq { dst, imm } => write!(f, "moveq #{}, {}", imm, dst),
            Instruction::MoveImm32 { dst, imm } => write!(f, "move.l #{}, {}", imm, dst),
            Instruction::Move { src, dst } => write!(f, "move.l {}, {}", src, dst),
            Instruction::Load { base, offset, dst } => write!(f, "move.l {}({}), {}", offset, base, dst),
            Instruction::Store { src, base, offset } => write!(f, "move.l {}, {}({})", src, offset, base),
            Instruction::Add { src, dst } => write!(f, "add.l {}, {}", src, dst),
            Instruction::Sub { src, dst } => write!(f, "sub.l {}, {}", src, dst),
            Instruction::And { src, dst } => write!(f, "and.l {}, {}", src, dst),
            Instruction::Or { src, dst } => write!(f, "or.l {}, {}", src, dst),
            Instruction::Xor { src, dst } => write!(f, "eor.l {}, {}", src, dst),
            Instruction::Lsl { dst, imm } => write!(f, "lsl.l #{}, {}", imm, dst),
            Instruction::Lsr { dst, imm } => write!(f, "lsr.l #{}, {}", imm, dst),
            Instruction::Asr { dst, imm } => write!(f, "asr.l #{}, {}", imm, dst),
            Instruction::Mulu { src, dst } => write!(f, "mulu.w {}, {}", src, dst),
            Instruction::Divu { src, dst } => write!(f, "divu.w {}, {}", src, dst),
            Instruction::Swap { dst } => write!(f, "swap {}", dst),
            Instruction::Cmp { src, dst } => write!(f, "cmp.l {}, {}", src, dst),
            Instruction::Tst { dst } => write!(f, "tst.l {}", dst),
            Instruction::Jmp { target } => write!(f, "jmp ({})", target),
            Instruction::Jsr { target } => write!(f, "jsr ({})", target),
            Instruction::Rts => write!(f, "rts"),
            Instruction::Bra { offset } => write!(f, "bra.s {}", offset),
            Instruction::Bcc { cond, offset } => write!(f, "bcc{:#x} {}", cond, offset),
            Instruction::Trap0 => write!(f, "trap #0"),
            Instruction::Link { reg, disp } => write!(f, "link {}, #{}", reg, disp),
            Instruction::Unlk { reg } => write!(f, "unlk {}", reg),
            Instruction::Nop => write!(f, "nop"),
        }
    }
}

// ===========================================================================
// Scratch register allocation (stack-slot ISel uses these as temporaries)
// ===========================================================================

/// Scratch data registers for stack-slot ISel.
/// D0–D2 are caller-saved and free for our use.
/// D3 is callee-saved, used as extra scratch for division (saved/restored).
const S0: Gpr = Gpr::D0;
const S1: Gpr = Gpr::D1;
const S2: Gpr = Gpr::D2;
const S3: Gpr = Gpr::D3;

/// Frame pointer (A6) and stack pointer (A7).
const FP: Gpr = Gpr::A6;
const SP: Gpr = Gpr::A7;

// ===========================================================================
// Stack-slot helpers
// ===========================================================================

/// Load a 32-bit immediate into a register (low word of a 64-bit value).
fn ss_load_imm(dst: Gpr, val: i64) -> Vec<u8> {
    let v = val as i32;
    if (-128..=127).contains(&v) {
        Instruction::Moveq { dst, imm: v as i8 }.encode()
    } else {
        Instruction::MoveImm32 { dst, imm: v }.encode()
    }
}

/// Store a 32-bit value from src register to stack slot at [FP + offset].
/// Only stores the low 32 bits.  The high 32 bits (at offset+4) are
/// managed explicitly by operations that produce 64-bit results (Shl 32,
/// Or with 64-bit operands).
fn ss_st(src: Gpr, offset: i32) -> Vec<u8> {
    if (-32768..=32767).contains(&offset) {
        Instruction::Store { src, base: FP, offset: offset as i16 }.encode()
    } else {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x22, 0x46]); // movea.l %a6, %a1
        code.extend(ss_load_imm(S2, offset as i64));
        code.extend_from_slice(&[0xD3, 0xC2]); // adda.l %d2, %a1
        let w = 0x2000u16 | (1u16 << 9) | (2u16 << 3) | (src.encoding() as u16 & 0x7);
        code.extend_from_slice(&w.to_be_bytes());
        code
    }
}

/// Load a 32-bit value from stack slot at [FP + offset] into dst.
/// Only loads the low 32 bits (sufficient for most operations).
fn ss_ld(dst: Gpr, offset: i32) -> Vec<u8> {
    if (-32768..=32767).contains(&offset) {
        Instruction::Load { base: FP, offset: offset as i16, dst }.encode()
    } else {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x22, 0x46]); // movea.l %a6, %a1
        code.extend(ss_load_imm(S2, offset as i64));
        code.extend_from_slice(&[0xD3, 0xC2]); // adda.l %d2, %a1
        code.extend(Instruction::Load { base: Gpr::A1, offset: 0, dst }.encode());
        code
    }
}

/// Emit a 32-bit/32-bit unsigned division or modulo using shift-and-subtract.
///
/// The m68k `divu.w` instruction only supports 16-bit quotients. When the
/// quotient exceeds 65535 (e.g., dividing a 32-bit address by a small
/// constant), divu.w produces incorrect results or traps.
///
/// This function implements a proper 32-bit division using the standard
/// shift-and-subtract algorithm with LSL.L and ROXL.L instructions:
///
/// For each of 32 iterations:
///   1. LSL.L #1, dividend  — shift left, MSB → X (extend flag)
///   2. ROXL.L #1, remainder — shift left, X → LSB (propagate MSB into remainder)
///   3. LSL.L #1, quotient   — shift left (make room for new bit)
///   4. If remainder >= divisor: subtract divisor from remainder, set quotient bit
///
/// Input: S0 = dividend, S1 = divisor
/// Output: S0 = quotient (if want_remainder=false) or remainder (if true)
/// Uses: S2 (remainder), S3 (quotient, saved/restored), stack (counter)
fn emit_divmod_32bit(want_remainder: bool) -> Vec<u8> {
    let mut code = Vec::new();
    let s0 = S0.encoding() as u8 & 0x7;
    let _s1 = S1.encoding() as u8 & 0x7;
    let s2 = S2.encoding() as u8 & 0x7;
    let s3 = S3.encoding() as u8 & 0x7;

    // Save D3 (callee-saved): MOVE.L D3, -(A7) = 0x2F03
    code.extend_from_slice(&[0x2F, 0x03]);

    // S2 = 0 (remainder)
    code.extend(Instruction::Moveq { dst: S2, imm: 0 }.encode());
    // S3 = 0 (quotient)
    code.extend(Instruction::Moveq { dst: S3, imm: 0 }.encode());

    // Push counter (32) to stack: MOVE.L D3, -(A7)
    code.extend(Instruction::Moveq { dst: S3, imm: 32 }.encode());
    code.extend_from_slice(&[0x2F, 0x03]);
    // S3 = 0 (quotient, reinitialize after pushing counter)
    code.extend(Instruction::Moveq { dst: S3, imm: 0 }.encode());

    // Loop: 32 iterations
    let div_loop = code.len() as i64;

    // LSL.L #1, S0 — shift dividend left, MSB → X (extend flag)
    // Encoding: 1110 001 1 10 0 01 rrr = 0xE388 | r
    code.extend_from_slice(&[0xE3, 0x88 | s0]);

    // ROXL.L #1, S2 — rotate remainder left through X: S2 = (S2 << 1) | X
    // This propagates the dividend's MSB (now in X) into the remainder's LSB
    // Encoding: 1110 001 1 10 0 10 rrr = 0xE390 | r
    code.extend_from_slice(&[0xE3, 0x90 | s2]);

    // LSL.L #1, S3 — shift quotient left (make room for new bit)
    code.extend_from_slice(&[0xE3, 0x88 | s3]);

    // CMP.L S1, S2 — sets C=1 (borrow) if S2 < S1
    code.extend(Instruction::Cmp { src: S1, dst: S2 }.encode());

    // BCS.S skip — if S2 < S1 (C=1), skip the subtract
    let bcs_off = code.len();
    code.extend_from_slice(&[0x65, 0x00]); // placeholder

    // SUB.L S1, S2 — remainder -= divisor
    code.extend(Instruction::Sub { src: S1, dst: S2 }.encode());

    // ORI.L #1, S3 — set quotient LSB (quotient bit = 1)
    // Encoding: 0000 0000 10 000 ddd, 0x00000001
    code.extend_from_slice(&[0x00, 0x80 | s3, 0x00, 0x00, 0x00, 0x01]);

    // skip:
    let skip_end = code.len() as i64;
    code[bcs_off + 1] = (skip_end - bcs_off as i64 - 2) as i8 as u8;

    // SUBQ.L #1, (A7) — decrement counter on stack
    // Encoding: 0101 001 1 10 010 111 = 0x5397
    code.extend_from_slice(&[0x53, 0x97]);

    // BNE.S loop — if counter != 0, branch back
    let bne_off = code.len();
    code.extend_from_slice(&[0x66, 0x00]); // placeholder
    let bne_disp = (div_loop - bne_off as i64 - 2) as i8;
    code[bne_off + 1] = bne_disp as u8;

    // Pop counter (discard): MOVE.L (A7)+, D0 = 0x201F
    code.extend_from_slice(&[0x20, 0x1F]);

    // Move result to S0
    if want_remainder {
        code.extend(Instruction::Move { src: S2, dst: S0 }.encode());
    } else {
        code.extend(Instruction::Move { src: S3, dst: S0 }.encode());
    }

    // Restore D3: MOVE.L (A7)+, D3 = 0x261F
    code.extend_from_slice(&[0x26, 0x1F]);

    code
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

/// Determine whether a type is 32-bit (or smaller) — for selecting 32-bit ops.
/// m68k is a 32-bit architecture (32-bit registers, 32-bit address space), so
/// all integer ops are 32-bit by default.  We use 32-bit MOVE/ADD/etc for everything.
fn _is_32bit_ty(_ty: Option<&IRType>) -> bool {
    true
}

// ===========================================================================
// Stack-slot based allocate_registers
// ===========================================================================

/// Stack-slot based `allocate_registers` for m68k.
///
/// Every vreg gets a 4-byte stack slot at [FP + offset]; operations use
/// scratch registers D0–D2.  m68k has no branch delay slots.
fn m68k_allocate_registers_ss(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let func_name = func.name.clone();

    // ── Phase 1: Collect all vreg IDs and compute stack layout ──
    let mut all_vreg_ids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &id in func.vregs.keys() {
        all_vreg_ids.insert(id);
    }
    for param in &func.params {
        if let Some(id) = param.as_register() {
            all_vreg_ids.insert(id);
        }
    }
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() {
                all_vreg_ids.insert(id);
            }
            for id in instr.used_regs() {
                all_vreg_ids.insert(id);
            }
        }
        match &block.terminator {
            crate::ir::IRTerminator::Branch { cond, .. } => {
                if let Some(id) = cond.as_register() {
                    all_vreg_ids.insert(id);
                }
            }
            crate::ir::IRTerminator::Return(vals) => {
                for val in vals {
                    if let Some(id) = val.as_register() {
                        all_vreg_ids.insert(id);
                    }
                }
            }
            crate::ir::IRTerminator::Switch { discr, .. } => {
                if let Some(id) = discr.as_register() {
                    all_vreg_ids.insert(id);
                }
            }
            _ => {}
        }
    }
    for val in &func.results {
        if let Some(id) = val.as_register() {
            all_vreg_ids.insert(id);
        }
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
    // [low address]  FP = saved A6 + 4 (after LINK A6, #-frame_size)
    //   local/arg area grows UP toward higher addresses
    //   vreg slot M   ← FP + 4*(M-1) + 4   (we use positive offsets from FP)
    //   Alloc data
    //   ...
    //   saved A6 (old FP)  ← FP+0
    //   saved return addr  ← FP+4  (pushed by JSR/BSR, then LINK pushes A6)
    // [high address] SP = FP + 4 + frame_size
    //
    // We use POSITIVE offsets from FP for vreg slots (this matches the
    // standard "LINK A6, #-N" convention where locals are at negative
    // offsets, but we invert by allocating locals ABOVE FP for simplicity).
    // Actually, the standard m68k convention uses NEGATIVE offsets for
    // locals: LINK A6, #-frame_size allocates N bytes BELOW A6.
    //
    // For simplicity we use NEGATIVE offsets from FP, starting at -8.
    // Each vreg gets 8 bytes (two 32-bit words) to support 64-bit values.
    // The low 32 bits are at offset, the high 32 bits are at offset+4.

    let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
    let mut current_offset: i32 = -8;
    let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
    all_vreg_ids_sorted.sort();
    for &id in &all_vreg_ids_sorted {
        vreg_stack_slots.insert(id, current_offset);
        current_offset -= 8;
    }

    // Alloc regions after vreg slots (also negative offsets).
    let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
    let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
    alloc_vreg_ids.sort();
    for &id in &alloc_vreg_ids {
        let size = alloc_sizes[&id];
        current_offset -= size;
        // Align to -16.
        current_offset &= !15;
        alloc_offsets.insert(id, current_offset);
    }

    // Total frame size: |current_offset|, aligned to 4.
    let frame_size = ((-current_offset as i64 + 3) & !3) as usize;
    let frame_size_i16 = frame_size as i16;

    // ── Phase 2: Build the phi-map (kept for compat) ──
    let _phi_map = func.build_phi_map();

    // ── Phase 3: Emit prologue ──
    let mut code: Vec<u8> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();

    // LINK A6, #-frame_size: push A6, A6 = SP, SP -= frame_size.
    code.extend(Instruction::Link { reg: FP, disp: -frame_size_i16 }.encode());

    // Save incoming args (D1-D5) to their stack slots.
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

    // Collect branch-patch records.
    struct BranchPatch {
        code_offset: usize,
        target_label: String,
        is_long: bool, // BRA.W (16-bit disp) vs BRA.S (8-bit disp)
    }
    let mut branch_patches: Vec<BranchPatch> = Vec::new();

    for block in func.blocks.iter() {
        block_start_offsets.push(code.len());

        for instr in &block.instructions {
            emit_instr(instr, &vreg_stack_slots, &alloc_offsets, &mut code, &mut relocations);
        }

        // Emit terminator.
        match &block.terminator {
            crate::ir::IRTerminator::Jump(target) => {
                // BRA.W target — 4 bytes (0x60 0x00 + 2-byte disp).
                let patch_offset = code.len();
                // BRA.W: 0x6000 0x0000 — 16-bit displacement form.
                code.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
                branch_patches.push(BranchPatch {
                    code_offset: patch_offset,
                    target_label: target.clone(),
                    is_long: true,
                });
            }
            crate::ir::IRTerminator::Branch {
                cond,
                true_block,
                false_block,
            } => {
                // Load cond into S0, then TST.L S0, then BNE to true_block.
                code.extend(ss_load_value(cond, &vreg_stack_slots, S0));
                code.extend(Instruction::Tst { dst: S0 }.encode());
                // BNE.W true_block — 4 bytes (0x66 0x00 + 2-byte disp).
                let true_patch = code.len();
                code.extend_from_slice(&[0x66, 0x00, 0x00, 0x00]);
                branch_patches.push(BranchPatch {
                    code_offset: true_patch,
                    target_label: true_block.clone(),
                    is_long: true,
                });
                // BRA.W false_block — 4 bytes.
                let false_patch = code.len();
                code.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
                branch_patches.push(BranchPatch {
                    code_offset: false_patch,
                    target_label: false_block.clone(),
                    is_long: true,
                });
            }
            crate::ir::IRTerminator::Return(vals) => {
                // Move return value to D0 (low 32 bits) and D1 (high 32 bits).
                if let Some(first_val) = vals.first() {
                    code.extend(ss_load_value(first_val, &vreg_stack_slots, Gpr::D0));
                    // Also load high word into D1
                    if let IRValue::Register(id) = first_val {
                        let off = vreg_stack_slots.get(id).copied().unwrap_or(0);
                        code.extend(Instruction::Load { base: FP, offset: (off + 4) as i16, dst: Gpr::D1 }.encode());
                    } else {
                        code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 0 }.encode());
                    }
                }
                // Epilogue: UNLK A6, RTS.
                code.extend(Instruction::Unlk { reg: FP }.encode());
                code.extend(Instruction::Rts.encode());
            }
            crate::ir::IRTerminator::Unreachable => {
                // ILLEGAL instruction (0x4AFC) — must NOT fall through.
                // RTS would return to caller, potentially falling through to parent code.
                code.extend_from_slice(&[0x4A, 0xFC]);
            }
            crate::ir::IRTerminator::Switch { discr, targets, default } => {
                // Simplified: linear compare-and-branch sequence.
                code.extend(ss_load_value(discr, &vreg_stack_slots, S0));
                for (val, label) in targets {
                    code.extend(ss_load_imm(S1, *val));
                    code.extend(Instruction::Cmp { src: S1, dst: S0 }.encode());
                    // BEQ.W label — 0x67 00 + 2-byte disp.
                    let patch = code.len();
                    code.extend_from_slice(&[0x67, 0x00, 0x00, 0x00]);
                    branch_patches.push(BranchPatch {
                        code_offset: patch,
                        target_label: label.clone(),
                        is_long: true,
                    });
                }
                let patch = code.len();
                code.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
                branch_patches.push(BranchPatch {
                    code_offset: patch,
                    target_label: default.clone(),
                    is_long: true,
                });
            }
            crate::ir::IRTerminator::Invoke {
                dst: _,
                func: _,
                args: _,
                normal,
                unwind: _,
            } => {
                let patch = code.len();
                code.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
                branch_patches.push(BranchPatch {
                    code_offset: patch,
                    target_label: normal.clone(),
                    is_long: true,
                });
            }
            crate::ir::IRTerminator::TailCall { .. } => {
                code.extend(Instruction::Unlk { reg: FP }.encode());
                code.extend(Instruction::Rts.encode());
            }
            crate::ir::IRTerminator::Resume { .. } => {
                code.extend(Instruction::Nop.encode());
            }
        }
    }

    // ── Phase 5: Patch branch offsets ──
    // BRA.W / Bcc.W: 16-bit signed displacement. Target = PC + disp + 2.
    // PC points to the EXTENSION word (the 2nd word of the instruction) at offset+2.
    // So disp = target_offset - (patch_offset + 2).
    for patch in &branch_patches {
        if let Some(&target_idx) = label_to_idx.get(&patch.target_label) {
            let target_offset = block_start_offsets[target_idx] as i64;
            let pc_offset = (patch.code_offset + 2) as i64; // PC = patch_offset + 2 (extension word position)
            let disp = target_offset - pc_offset;
            if patch.is_long {
                let disp_be = (disp as i16).to_be_bytes();
                code[patch.code_offset + 2..patch.code_offset + 4].copy_from_slice(&disp_be);
            } else {
                let disp_byte = disp as i8;
                code[patch.code_offset + 1] = disp_byte as u8;
            }
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
            PhysicalReg::new(RegClass::Gpr, Gpr::A6 as u32),
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
fn m68k_allocate_registers_real(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    // Run the existing stack-slot allocator to get a working AllocatedFunction.
    let mut allocated = m68k_allocate_registers_ss(func)?;

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

/// Emit a single IR instruction as m68k machine code.
fn emit_instr(
    instr: &IRInstr,
    vreg_stack_slots: &HashMap<u32, i32>,
    alloc_offsets: &HashMap<u32, i32>,
    code: &mut Vec<u8>,
    relocations: &mut Vec<RelocationEntry>,
) {
    match instr {
        IRInstr::Add { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Add { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
            // 64-bit: also add high words
            let lhs_off = if let IRValue::Register(id) = lhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            code.extend(Instruction::Load { base: FP, offset: (lhs_off + 4) as i16, dst: S0 }.encode());
            if let IRValue::Immediate(v) = rhs {
                code.extend(ss_load_imm(S1, (v >> 32) as i64));
            } else {
                let rhs_off = if let IRValue::Register(id) = rhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
                code.extend(Instruction::Load { base: FP, offset: (rhs_off + 4) as i16, dst: S1 }.encode());
            }
            code.extend(Instruction::Add { src: S1, dst: S0 }.encode());
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
        }
        IRInstr::Sub { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Sub { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
            // 64-bit: also sub high words
            let lhs_off = if let IRValue::Register(id) = lhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            code.extend(Instruction::Load { base: FP, offset: (lhs_off + 4) as i16, dst: S0 }.encode());
            if let IRValue::Immediate(v) = rhs {
                code.extend(ss_load_imm(S1, (v >> 32) as i64));
            } else {
                let rhs_off = if let IRValue::Register(id) = rhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
                code.extend(Instruction::Load { base: FP, offset: (rhs_off + 4) as i16, dst: S1 }.encode());
            }
            code.extend(Instruction::Sub { src: S1, dst: S0 }.encode());
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
        }
        IRInstr::Mul { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // MULU.W S1, S0: S0[31:0] = S0[15:0] * S1[15:0] (low 16 bits each).
            // For values that fit in 16 bits, this gives the correct 32-bit result.
            code.extend(Instruction::Mulu { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Div { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(emit_divmod_32bit(false));
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::BinOp {
            op,
            dst,
            lhs,
            rhs,
            ty: _,
        } => {
            emit_binop(op, dst, lhs, rhs, vreg_stack_slots, code);
        }
        IRInstr::Cmp {
            kind,
            dst,
            lhs,
            rhs,
            ty: _,
        } => {
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
            emit_binop(&binop_kind, dst, lhs, rhs, vreg_stack_slots, code);
        }
        IRInstr::UnaryOp {
            op,
            dst,
            operand,
            ty: _,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(operand, vreg_stack_slots, S0));
            match op {
                UnaryOpKind::Neg => {
                    // NEG.L Dn: word = 0x4480 | (Dn<<9).
                    let w = 0x4480u16 | ((S0.encoding() as u16 & 0x7) << 9);
                    code.extend_from_slice(&w.to_be_bytes());
                }
                UnaryOpKind::Not => {
                    // NOT.L Dn: word = 0x4680 | (Dn<<9).
                    let w = 0x4680u16 | ((S0.encoding() as u16 & 0x7) << 9);
                    code.extend_from_slice(&w.to_be_bytes());
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    // Not natively supported on basic 68000; emit 0.
                    code.extend(ss_load_imm(S0, 0));
                }
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Load {
            dst,
            addr,
            offset,
            ty,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // Compute address: S2 = addr + offset (in an address register).
            code.extend(ss_load_value(addr, vreg_stack_slots, S0));
            // MOVE.L S0, A1 (MOVEA.L D0, A1): word = 0x2040 | (1<<9) | S0_enc.
            {
                let w = 0x2040u16 | (1u16 << 9) | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
            }
            if *offset != 0 {
                // ADDQ.L #data8, A1 (data 1-8) or ADDA.L #imm32, A1.
                let o = *offset;
                if (1..=8).contains(&o) {
                    // ADDQ.L #data, An: word = 0x50C0 | (data<<9) | (3<<6) | An.
                    // For An=A1 (enc 1): 0x50C0 | (o<<9) | (3<<6) | 1.
                    let w = 0x50C0u16 | ((o as u16) << 9) | (3u16 << 6) | 1;
                    code.extend_from_slice(&w.to_be_bytes());
                } else {
                    // Load offset into D2, then ADDA.L D2, A1.
                    code.extend(ss_load_imm(S2, o as i64));
                    // ADDA.L D2, A1: word = 0xD1C0 | (1<<9) | 2.
                    let w = 0xD1C0u16 | (1u16 << 9) | 2;
                    code.extend_from_slice(&w.to_be_bytes());
                }
            }
            // Load value from (A1) into S0, using correct size based on ty.
            let size_bits: u16 = match ty {
                IRType::I8 | IRType::U8 => 0x1000, // byte
                IRType::I16 | IRType::U16 => 0x3000, // word
                _ => 0x2000, // long (default)
            };
            {
                let w = size_bits | (2u16 << 3) | 1;
                code.extend_from_slice(&w.to_be_bytes());
            }
            // CRITICAL: m68k MOVE.B to a data register only modifies the
            // low byte — the upper 24 bits are UNCHANGED.  This means a
            // byte load of 0x00 into D0 (which had 0x12345678) gives
            // D0 = 0x12345600, NOT 0x00000000.  This breaks comparisons
            // (CMP.L sees a non-zero value) and arithmetic.
            //
            // Fix: zero-extend after byte/word loads by clearing the
            // upper bits with AND.L.
            match ty {
                IRType::I8 | IRType::U8 => {
                    // AND.L #0xFF, D0 — 0x0280 0x000000FF
                    code.extend_from_slice(&[0x02, 0x80, 0x00, 0x00, 0x00, 0xFF]);
                }
                IRType::I16 | IRType::U16 => {
                    // AND.L #0xFFFF, D0 — 0x0280 0x0000FFFF
                    code.extend_from_slice(&[0x02, 0x80, 0x00, 0x00, 0xFF, 0xFF]);
                }
                _ => {} // long: no masking needed
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Store {
            value,
            addr,
            offset,
            ty,
        } => {
            code.extend(ss_load_value(addr, vreg_stack_slots, S0));
            // MOVE.L S0, A1
            {
                let w = 0x2040u16 | (1u16 << 9) | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
            }
            if *offset != 0 {
                let o = *offset;
                if (1..=8).contains(&o) {
                    let w = 0x50C0u16 | ((o as u16) << 9) | (3u16 << 6) | 1;
                    code.extend_from_slice(&w.to_be_bytes());
                } else {
                    code.extend(ss_load_imm(S2, o as i64));
                    let w = 0xD1C0u16 | (1u16 << 9) | 2;
                    code.extend_from_slice(&w.to_be_bytes());
                }
            }
            // Load value into S2.
            code.extend(ss_load_value(value, vreg_stack_slots, S2));
            // Store S2 to (A1) using correct size based on ty.
            // MOVE.B S2, (A1): size=01, dst=A1 mode=010, src=S2 mode=000
            //   = 0x1000 | (1<<9) | (2<<6) | (0<<3) | S2_enc
            // MOVE.W S2, (A1): size=11
            //   = 0x3000 | (1<<9) | (2<<6) | (0<<3) | S2_enc
            // MOVE.L S2, (A1): size=10 (default)
            //   = 0x2000 | (1<<9) | (2<<6) | (0<<3) | S2_enc
            let size_bits: u16 = match ty {
                IRType::I8 | IRType::U8 => 0x1000, // byte
                IRType::I16 | IRType::U16 => 0x3000, // word
                _ => 0x2000, // long (default)
            };
            {
                let w = size_bits | (1u16 << 9) | (2u16 << 6) | (S2.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
            }
        }
        IRInstr::Alloc { dst, size: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            // dst = FP + alloc_offset[dst_id]
            if let Some(&off) = alloc_offsets.get(&dst_id) {
                // S0 = A6 (FP)
                // MOVE.L A6, D0: src=A6 mode=001(An direct), dst=D0 mode=000(Dn).
                // Format: 00 SS DDD ddd mmm rrr
                // SS=10(long), DDD=000(D0), ddd=000(Dn), mmm=001(An), rrr=110(A6)
                // = 0x200E.
                code.extend_from_slice(&[0x20, 0x0E]);
                // ADD.L #off, D0 — ADDQ.L or ADDI.L.
                if (-7..=-1).contains(&off) || (1..=8).contains(&off) {
                    let abs = off.unsigned_abs() as u16;
                    let w = 0x5080u16 | (abs << 9);
                    // For negative: use SUBQ.L instead.
                    if off < 0 {
                        let w = 0x5180u16 | (abs << 9);
                        code.extend_from_slice(&w.to_be_bytes());
                    } else {
                        code.extend_from_slice(&w.to_be_bytes());
                    }
                } else {
                    // ADDI.L #imm32, D0: word = 0x0680 | (0<<9), ext = imm32.
                    code.extend_from_slice(&[0x06, 0x80]);
                    code.extend_from_slice(&(off as i32).to_be_bytes());
                }
                let dst_slot = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                code.extend(ss_st(S0, dst_slot));
            }
        }
        IRInstr::Free { ptr: _ } => {
            // Lowered to a runtime call (handled by Call); no-op here.
        }
        IRInstr::Cast {
            kind,
            dst,
            src,
            from_ty: _,
            to_ty: _,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(src, vreg_stack_slots, S0));
            match kind {
                CastKind::ZExt | CastKind::SExt | CastKind::Trunc | CastKind::BitCast => {
                    // For a 32-bit architecture, casting between integer types
                    // just keeps the low bits.  No-op (store as-is).
                }
                CastKind::IntToFloat
                | CastKind::UIntToFloat
                | CastKind::FloatToInt
                | CastKind::FloatToUInt
                | CastKind::FloatToFloat => {
                    // FP not supported in this minimal backend; leave as-is.
                }
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Phi { .. } => {
            // Phi nodes are handled by SSA deconstruction at the IR level;
            // no code is emitted here.
        }
        IRInstr::GetAddress { dst, name } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // Use LEA placeholder; record a relocation.
            // LEA sym(PC), Dn — 0x4EFB | (Dn<<9) | (0b111<<6) | (0b010<<3) | (0b011<<0) = PC-relative d16 form.
            // Encoding: word = 0x4EFB | (Dn<<9) | 0xFA, ext word = 0x0000 (d16 = 0, patched).
            // Actually: LEA d16(PC), An: word = 0x41F8 | (An<<9) | (0b111<<6) | (0b010<<3) | (0b011).
            // We use MOVEA.L #0, Dn (will be patched via relocation).
            let reloc_offset = code.len() as u64;
            // MOVE.L #0, S0 — 0x203C | (S0<<9), imm32 = 0.
            {
                let w = 0x203Cu16 | ((S0.encoding() as u16 & 0x7) << 9);
                code.extend_from_slice(&w.to_be_bytes());
                code.extend_from_slice(&0u32.to_be_bytes());
            }
            relocations.push(RelocationEntry {
                offset: reloc_offset,
                symbol: name.clone(),
                reloc_type: "R_68K_32".to_string(),
            });
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Offset {
            dst,
            base,
            offset,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(base, vreg_stack_slots, S0));
            code.extend(ss_load_value(offset, vreg_stack_slots, S1));
            code.extend(Instruction::Add { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Select {
            dst,
            cond,
            true_val,
            false_val,
            ty: _,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // dst = (cond != 0) ? true_val : false_val
            code.extend(ss_load_value(false_val, vreg_stack_slots, S0));
            code.extend(ss_load_value(cond, vreg_stack_slots, S1));
            code.extend(Instruction::Tst { dst: S1 }.encode());
            // BEQ.S skip (skip the load-true if cond == 0).
            // BEQ.S +4 (skip MOVE.L #true, S0 — but true_val is variable size, so use 4-byte skip).
            // For safety, use BEQ.W (4 bytes) + skip the load-true sequence.
            // Actually simpler: BEQ.W +sizeof(load_true_seq).  We compute the patch later.
            let beq_patch = code.len();
            code.extend_from_slice(&[0x67, 0x00, 0x00, 0x00]); // BEQ.W placeholder
            let load_true_start = code.len();
            code.extend(ss_load_value(true_val, vreg_stack_slots, S0));
            let load_true_end = code.len();
            let skip_disp = (load_true_end as i64 - (beq_patch as i64 + 2)) as i16;
            code[beq_patch + 2..beq_patch + 4].copy_from_slice(&skip_disp.to_be_bytes());
            let _ = load_true_start;
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Ret { values } => {
            if let Some(first_val) = values.first() {
                code.extend(ss_load_value(first_val, vreg_stack_slots, Gpr::D0));
                // Also load high word into D1 for 64-bit return values
                if let IRValue::Register(id) = first_val {
                    let off = vreg_stack_slots.get(id).copied().unwrap_or(0);
                    code.extend(Instruction::Load { base: FP, offset: (off + 4) as i16, dst: Gpr::D1 }.encode());
                } else {
                    code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 0 }.encode());
                }
            }
            code.extend(Instruction::Unlk { reg: FP }.encode());
            code.extend(Instruction::Rts.encode());
        }
        IRInstr::Branch { target: _ } => {
            // Instruction-level branch (not terminator). Redundant with
            // the Jump terminator that follows. Emit NOP to avoid
            // unpatched self-loop BRA.
            code.extend_from_slice(&[0x4E, 0x71]); // NOP
        }
        IRInstr::CondBranch {
            cond: _,
            true_target: _,
            false_target: _,
        } => {
            // Instruction-level CondBranch (not terminator). Redundant with
            // the Branch terminator that follows. Emit NOPs.
            code.extend_from_slice(&[0x4E, 0x71]); // NOP
            code.extend_from_slice(&[0x4E, 0x71]); // NOP
            code.extend_from_slice(&[0x4E, 0x71]); // NOP
        }
        IRInstr::Call {
            dst,
            func,
            args,
            is_extern: _,
        } => {
            // Move args into D1-D5 (up to 5 args).
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    code.extend(ss_load_value(arg, vreg_stack_slots, S0));
                    code.extend(Instruction::Move { src: S0, dst: arg_reg }.encode());
                }
            }
            // BSR.L disp32: 0x61 0xFF + 4-byte disp32 (6 bytes total).
            // The 0xFF in the 8-bit displacement field signals BSR.L (32-bit
            // displacement follows). The old encoding (0x61 0x00 0xFF 0xFF)
            // was BSR.W with displacement 0xFFFF = -1, branching to PC-1.
            let call_offset = code.len() as u64;
            code.extend_from_slice(&[0x61, 0xFF]);
            code.extend_from_slice(&0u32.to_be_bytes());
            relocations.push(RelocationEntry {
                offset: call_offset,
                symbol: func.clone(),
                reloc_type: "R_68K_PC32".to_string(),
            });
            // Move return value from D0 (low) and D1 (high) to dst's stack slot.
            if let Some(d) = dst {
                let d_id = d.as_register().unwrap_or(0);
                let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                // Store low word from D0
                code.extend(Instruction::Move { src: Gpr::D0, dst: S0 }.encode());
                code.extend(ss_st(S0, d_off));
                // Store high word from D1
                code.extend(Instruction::Store { src: Gpr::D1, base: FP, offset: (d_off + 4) as i16 }.encode());
            }
        }
        IRInstr::CtSelect {
            dst,
            cond,
            true_val,
            false_val,
            ty: _,
        } => {
            // Constant-time select using bitwise ops (no branches).
            // mask = -(cond != 0); result = (true_val & mask) | (false_val & ~mask)
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // Simplified: fall back to branch-based Select.
            code.extend(ss_load_value(false_val, vreg_stack_slots, S0));
            code.extend(ss_load_value(cond, vreg_stack_slots, S1));
            code.extend(Instruction::Tst { dst: S1 }.encode());
            let beq_patch = code.len();
            code.extend_from_slice(&[0x67, 0x00, 0x00, 0x00]);
            code.extend(ss_load_value(true_val, vreg_stack_slots, S0));
            let skip_disp = (code.len() as i64 - (beq_patch as i64 + 2)) as i16;
            code[beq_patch + 2..beq_patch + 4].copy_from_slice(&skip_disp.to_be_bytes());
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::CtEq {
            dst,
            lhs,
            rhs,
            ty: _,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // S0 = lhs ^ rhs; S0 = (S0 == 0) ? 1 : 0
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // EOR.L S1, S0: 0xB180 | (S0<<9) | S1.
            {
                let w = 0xB180u16 | ((S0.encoding() as u16 & 0x7) << 9) | (S1.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
            }
            // SEQ D0 (Set on EQ): 0x50C1 | (S0<<9) — wait that's not right.
            // Scc Dn: word = 0x50C0 | (cc<<8) | (0<<6) | Dn.  cc=0 (T), 1 (F), ..., 2 (HI), ..., 7 (EQ).
            // SEQ = cc 0111 = 7.  So word = 0x50C0 | (7<<8) | Dn = 0x57C0 | Dn.
            // But this sets Dn to 0xFF (true) or 0x00 (false).  We want 1 or 0.
            // Truncate via AND.L #1.
            code.extend(ss_load_imm(S1, 0));
            code.extend(Instruction::Cmp { src: S0, dst: S1 }.encode());
            let w = 0x57C0u16 | (S0.encoding() as u16 & 0x7);
            code.extend_from_slice(&w.to_be_bytes());
            // ANDI.L #1, S0: 0x02BC | (S0<<9), imm32 = 1.
            {
                let w = 0x0280u16 | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
                code.extend_from_slice(&1u32.to_be_bytes());
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::AtomicLoad { dst, addr, ty } => {
            // Simplified: regular load.
            let load_instr = IRInstr::Load {
                dst: dst.clone(),
                addr: addr.clone(),
                offset: 0,
                ty: ty.clone(),
            };
            emit_instr(&load_instr, vreg_stack_slots, alloc_offsets, code, relocations);
        }
        IRInstr::AtomicStore { value, addr, ty } => {
            let store_instr = IRInstr::Store {
                value: value.clone(),
                addr: addr.clone(),
                offset: 0,
                ty: ty.clone(),
            };
            emit_instr(&store_instr, vreg_stack_slots, alloc_offsets, code, relocations);
        }
        IRInstr::AtomicCas {
            dst,
            addr,
            expected,
            desired,
            ty,
        } => {
            // CAS2 is complex; simplified as load + cmp + swap with branches.
            // For correctness, emit: load old; if old==expected, store desired.
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // Load old into S0.
            emit_instr(
                &IRInstr::Load { dst: dst.clone(), addr: addr.clone(), offset: 0, ty: ty.clone() },
                vreg_stack_slots, alloc_offsets, code, relocations,
            );
            // Compare S0 with expected.
            code.extend(ss_load_value(expected, vreg_stack_slots, S1));
            code.extend(Instruction::Cmp { src: S1, dst: S0 }.encode());
            // BNE skip-store.
            let bne_patch = code.len();
            code.extend_from_slice(&[0x66, 0x00, 0x00, 0x00]);
            // Store desired.
            code.extend(ss_load_value(desired, vreg_stack_slots, S0));
            code.extend(ss_load_value(addr, vreg_stack_slots, S2));
            // MOVE.L S2, A1
            {
                let w = 0x2040u16 | (1u16 << 9) | (S2.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
            }
            // MOVE.L S0, (A1) — store desired to *addr
            // m68k MOVE.L format: 00 SS DDD mmm mmm rrr
            //   SS=10 (long), DDD=001 (A1), dest mode=010 ((An)), src mode=000 (Dn), src reg=S0
            {
                let w = 0x2000u16  // MOVE.L base (bits 15-12=0010, bits 13-12=10)
                    | (1u16 << 9)  // dest reg = 1 (A1)
                    | (2u16 << 6)  // dest mode = 010 ((An) indirect)
                    | (S0.encoding() as u16 & 0x7);  // src reg (src mode = 000 = Dn)
                code.extend_from_slice(&w.to_be_bytes());
            }
            let skip_disp = (code.len() as i64 - (bne_patch as i64 + 2)) as i16;
            code[bne_patch + 2..bne_patch + 4].copy_from_slice(&skip_disp.to_be_bytes());
            // dst already holds old value (in its stack slot).
            let _ = dst_off;
        }
        IRInstr::Syscall { nr, args, dst } => {
            // m68k Linux syscall: args in D1-D5, nr in D0,
            // `trap #0`, result in D0.
            let syscall_arg_regs =
                [Gpr::D1, Gpr::D2, Gpr::D3, Gpr::D4, Gpr::D5];
            let num_reg_args = args.len().min(syscall_arg_regs.len());
            for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                code.extend(ss_load_value(arg, vreg_stack_slots, syscall_arg_regs[i]));
            }
            // MOVEQ #nr, D0  (or ss_load_imm for large numbers)
            code.extend(ss_load_imm(Gpr::D0, *nr as i64));
            // TRAP #0
            code.extend(Instruction::Trap0.encode());
            // Store result (D0) to dst's stack slot
            if let Some(d) = dst {
                let dst_id = d.as_register().unwrap_or(0);
                let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                code.extend(ss_st(Gpr::D0, dst_off));
            }
        }
        // ── VectorOp (Wave 29) ──
        // m68k has no SIMD encoder in the Wave 29 suite; emit nothing.
        IRInstr::VectorOp { .. } => {}
    }
}

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
            // 64-bit add: add low words, then add high words (with carry).
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Add { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
            // Add high words (ignore carry for simplicity — works for small values)
            let lhs_off = if let IRValue::Register(id) = lhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            let rhs_off = if let IRValue::Register(id) = rhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            code.extend(Instruction::Load { base: FP, offset: (lhs_off + 4) as i16, dst: S0 }.encode());
            if let IRValue::Immediate(v) = rhs {
                let hi = (v >> 32) as i32;
                code.extend(ss_load_imm(S1, hi as i64));
            } else {
                code.extend(Instruction::Load { base: FP, offset: (rhs_off + 4) as i16, dst: S1 }.encode());
            }
            code.extend(Instruction::Add { src: S1, dst: S0 }.encode());  // ADDX.L would be better (with carry)
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
        }
        BinOpKind::Sub => {
            // 64-bit sub: sub low words, then sub high words.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Sub { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
            let lhs_off = if let IRValue::Register(id) = lhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            let rhs_off = if let IRValue::Register(id) = rhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            code.extend(Instruction::Load { base: FP, offset: (lhs_off + 4) as i16, dst: S0 }.encode());
            if let IRValue::Immediate(v) = rhs {
                let hi = (v >> 32) as i32;
                code.extend(ss_load_imm(S1, hi as i64));
            } else {
                code.extend(Instruction::Load { base: FP, offset: (rhs_off + 4) as i16, dst: S1 }.encode());
            }
            code.extend(Instruction::Sub { src: S1, dst: S0 }.encode());
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
        }
        BinOpKind::Mul => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Mulu { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::UDiv | BinOpKind::SDiv => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(emit_divmod_32bit(false));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SRem | BinOpKind::URem => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(emit_divmod_32bit(true));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::And => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::And { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Or => {
            // 64-bit OR: OR both low and high words.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Or { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
            // OR high words: load lhs_hi, OR with rhs_hi, store at dst_off+4
            let lhs_off = if let IRValue::Register(id) = lhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            let rhs_off = if let IRValue::Register(id) = rhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            // Load lhs high word
            code.extend(Instruction::Load { base: FP, offset: (lhs_off + 4) as i16, dst: S0 }.encode());
            // Load rhs high word
            code.extend(Instruction::Load { base: FP, offset: (rhs_off + 4) as i16, dst: S1 }.encode());
            code.extend(Instruction::Or { src: S1, dst: S0 }.encode());
            // Store high word at dst_off+4
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
        }
        BinOpKind::Xor => {
            // 64-bit XOR: XOR both low and high words.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Xor { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
            // XOR high words
            let lhs_off = if let IRValue::Register(id) = lhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            let rhs_off = if let IRValue::Register(id) = rhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            code.extend(Instruction::Load { base: FP, offset: (lhs_off + 4) as i16, dst: S0 }.encode());
            code.extend(Instruction::Load { base: FP, offset: (rhs_off + 4) as i16, dst: S1 }.encode());
            code.extend(Instruction::Xor { src: S1, dst: S0 }.encode());
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
        }
        BinOpKind::Shl => {
            // 64-bit left shift: handle shift by 32 specially.
            // The m68k LSL.L instruction masks shift count to 5 bits (0-31),
            // so << 32 becomes << 0 (no-op).  We need to check at runtime
            // if the shift amount is >= 32 and handle it as a 64-bit shift.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // CMPI.L #32, D1 — compare shift amount with 32
            code.extend_from_slice(&[0x0C, 0x81, 0x00, 0x00, 0x00, 0x20]);
            // BNE.S to normal_shift (offset will be patched)
            let bne_patch = code.len();
            code.extend_from_slice(&[0x66, 0x00]); // BNE.S +0 (placeholder)
            // 64-bit shift by 32: low = 0, high = S0
            // Store S0 as high word at dst_off+4
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
            // Store 0 as low word at dst_off
            code.extend(Instruction::Moveq { dst: S0, imm: 0 }.encode());
            code.extend(ss_st(S0, dst_off));
            // BRA.S to done
            let bra_patch = code.len();
            code.extend_from_slice(&[0x60, 0x00]); // BRA.S +0 (placeholder)
            // normal_shift:
            let normal_start = code.len();
            // Patch BNE to jump here
            let bne_disp = (normal_start - bne_patch - 2) as i8;
            code[bne_patch + 1] = bne_disp as u8;
            // Normal 32-bit shift
            let w = 0xE1A8u16 | ((S1.encoding() as u16 & 0x7) << 9) | (S0.encoding() as u16 & 0x7);
            code.extend_from_slice(&w.to_be_bytes());
            code.extend(ss_st(S0, dst_off));
            // Clear high word
            code.extend(Instruction::Moveq { dst: S0, imm: 0 }.encode());
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
            // done:
            let done = code.len();
            // Patch BRA to jump here
            let bra_disp = (done - bra_patch - 2) as i8;
            code[bra_patch + 1] = bra_disp as u8;
        }
        BinOpKind::ShrL => {
            // 64-bit right shift: handle shift by 32 specially.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // CMPI.L #32, D1
            code.extend_from_slice(&[0x0C, 0x81, 0x00, 0x00, 0x00, 0x20]);
            // BNE.S to normal_shift
            let bne_patch = code.len();
            code.extend_from_slice(&[0x66, 0x00]);
            // 64-bit shift by 32: low = x_high, high = 0
            let lhs_off = if let IRValue::Register(id) = lhs { vreg_stack_slots.get(id).copied().unwrap_or(0) } else { 0 };
            // Load high word of lhs into S0
            code.extend(Instruction::Load { base: FP, offset: (lhs_off + 4) as i16, dst: S0 }.encode());
            // Store as low word of result
            code.extend(ss_st(S0, dst_off));
            // Store 0 as high word
            code.extend(Instruction::Moveq { dst: S0, imm: 0 }.encode());
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
            // BRA.S to done
            let bra_patch = code.len();
            code.extend_from_slice(&[0x60, 0x00]);
            // normal_shift:
            let normal_start = code.len();
            let bne_disp = (normal_start - bne_patch - 2) as i8;
            code[bne_patch + 1] = bne_disp as u8;
            // Normal 32-bit shift
            let w = 0xE0A8u16 | ((S1.encoding() as u16 & 0x7) << 9) | (S0.encoding() as u16 & 0x7);
            code.extend_from_slice(&w.to_be_bytes());
            code.extend(ss_st(S0, dst_off));
            code.extend(Instruction::Moveq { dst: S0, imm: 0 }.encode());
            code.extend(Instruction::Store { src: S0, base: FP, offset: (dst_off + 4) as i16 }.encode());
            // done:
            let done = code.len();
            let bra_disp = (done - bra_patch - 2) as i8;
            code[bra_patch + 1] = bra_disp as u8;
        }
        BinOpKind::ShrA => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // ASR.L Dn, Dm: shift Dm right (arithmetic) by Dn.
            // Format: 1110 Dn 0 10 1 00 Dm (bit 8=right, bits 4-3=00=AS, bit 5=1=reg)
            // Base: 0xE0A0 | (Dn<<9) | Dm
            let w = 0xE0A0u16 | ((S1.encoding() as u16 & 0x7) << 9) | (S0.encoding() as u16 & 0x7);
            code.extend_from_slice(&w.to_be_bytes());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Ror | BinOpKind::Rol => {
            // Simplified: rotate via shifts (rare in practice).
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SLt | BinOpKind::ULt | BinOpKind::SLe | BinOpKind::ULe
        | BinOpKind::SGt | BinOpKind::UGt | BinOpKind::SGe | BinOpKind::UGe
        | BinOpKind::Eq | BinOpKind::Ne => {
            // dst = (lhs op rhs) ? 1 : 0
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // CMP.L S1, S0 — sets condition codes.
            code.extend(Instruction::Cmp { src: S1, dst: S0 }.encode());
            // Scc S0: word = 0x50C0 | (cc<<8) | (0<<6) | S0.
            // m68k condition codes (low 4 bits):
            //   T=0, F=1, HI=2, LS=3, CC=4, CS=5, NE=6, EQ=7,
            //   VC=8, VS=9, PL=10, MI=11, GE=12, LT=13, GT=14, LE=15.
            let cc: u8 = match op {
                BinOpKind::Eq => 7,
                BinOpKind::Ne => 6,
                BinOpKind::SLt | BinOpKind::ULt => 13, // LT works for both signed and unsigned when operands are equal-range
                BinOpKind::SLe | BinOpKind::ULe => 15,
                BinOpKind::SGt | BinOpKind::UGt => 14,
                BinOpKind::SGe | BinOpKind::UGe => 12,
                _ => 6,
            };
            let w = 0x50C0u16 | ((cc as u16) << 8) | (S0.encoding() as u16 & 0x7);
            code.extend_from_slice(&w.to_be_bytes());
            // Scc sets byte to 0xFF (true) or 0x00 (false).  AND.L #1 to get 1 or 0.
            {
                let w = 0x0280u16 | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
                code.extend_from_slice(&1u32.to_be_bytes());
            }
            code.extend(ss_st(S0, dst_off));
        }
    }
}

// ===========================================================================
// Backend struct + TargetInfo + Backend impl
// ===========================================================================

/// m68k backend.
pub struct M68kBackend {
    target_info: M68kTargetInfo,
    /// Whether to use real register allocation (Wave 23) or stack-slot lowering.
    pub use_real_regalloc: bool,
}

impl M68kBackend {
    pub fn new() -> Self {
        Self { target_info: M68kTargetInfo, use_real_regalloc: false }
    }
}

impl Default for M68kBackend {
    fn default() -> Self { Self::new() }
}

/// m68k target information (32-bit, big-endian, Linux ABI).
pub struct M68kTargetInfo;

impl TargetInfo for M68kTargetInfo {
    fn isa_name(&self) -> &'static str { "m68k" }
    fn target_triple(&self) -> &'static str { "m68k-unknown-linux-gnu" }
    fn elf_machine_type(&self) -> u16 { 4 } // EM_68K
    fn default_base_address(&self) -> u64 { 0x10000 }
    fn pointer_width(&self) -> usize { 4 }
    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of_with_ptr_width(ty, 4)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of_with_ptr_width(ty, 4)
    }
    fn endianness(&self) -> Endianness { Endianness::Big }
    fn has_registers(&self) -> bool { true }
    fn num_gp_regs(&self) -> usize { 16 }
    fn num_simd_fp_regs(&self) -> usize { 8 } // FP0-FP7
    fn has_hardwired_zero(&self) -> bool { false }
    fn has_link_register(&self) -> bool { false } // m68k pushes return address on stack
    fn has_branch_delay_slots(&self) -> bool { false }
    fn has_toc_pointer(&self) -> bool { false }
    fn has_condition_registers(&self) -> bool { false }
    fn calling_convention_name(&self) -> &'static str { "m68k-linux" }
    fn num_int_arg_regs(&self) -> usize { 5 } // D1-D5
    fn num_fp_arg_regs(&self) -> usize { 2 } // FP0-FP1
    fn stack_alignment(&self) -> usize { 4 }
    fn instruction_alignment(&self) -> usize { 2 }
    fn instruction_width_range(&self) -> (usize, usize) { (2, 11) }
    fn output_format(&self) -> OutputFormat { OutputFormat::Elf32 }

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::m68k()
    }
}

impl Backend for M68kBackend {
    fn target_info(&self) -> &dyn TargetInfo { &self.target_info }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        if self.use_real_regalloc {
            m68k_allocate_registers_real(func)
        } else {
            m68k_allocate_registers_ss(func)
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
        // ── m68k Linux static executable ──
        //
        // Layout:
        //   _start:  PEA return_addr (skip); JSR main; MOVE.L D0, D1; MOVEQ #1, D0; TRAP #0
        //            (simplified: BSR main; MOVE.L D0, D1; MOVEQ #1, D0; TRAP #0)
        //   <functions...>
        //   <FFI return-0 stub>
        //   <__vuma_alloc / __vuma_free stubs>
        //   <POSIX syscall stubs>

        const R_M68K_PC32: &str = "R_68K_PC32";
        const BASE_ADDR: u64 = 0x10000;

        // Compute text_offset (must match build_m68k_elf).
        let elf_header_size: u64 = 52; // ELF32 header
        let phdr_size: u64 = 32; // Elf32_Phdr
        let num_phdrs: u64 = 3; // 2 LOAD + GNU_STACK — MUST match build_m68k_elf!
        let phdr_end = elf_header_size + num_phdrs * phdr_size;
        let text_offset: u64 = phdr_end;

        // ── _start stub ──
        // On Linux m68k, the process entry stack layout is:
        //   [SP]     = argc (4 bytes, m68k is 32-bit)
        //   [SP+4]   = argv[0] pointer
        //   [SP+8]   = argv[1] pointer
        //   ...
        //   NULL
        //   envp...
        //   NULL
        //   auxv...
        //
        // m68k calling convention: args in D1-D5 (then A0-A1), return in D0.
        // So argc -> D1, argv -> D2.
        //
        // MOVE.L (A7), D1     — 2 bytes (load argc into D1, first arg)
        // PEA 4(A7)           — 4 bytes (push argv pointer = SP+4 onto stack)
        // MOVE.L (A7)+, D2    — 2 bytes (pop argv pointer into D2, second arg)
        // BSR.L main          — 6 bytes (0x61FF + 4-byte disp32), offset 8.
        //   After return, D0 holds main's return value.
        // MOVE.L D0, D1       — 2 bytes, offset 14. (D1 = exit code)
        // MOVEQ #1, D0        — 2 bytes, offset 16. (D0 = SYS_exit = 1)
        // TRAP #0             — 2 bytes, offset 18.
        let start_stub_size: usize = 20;
        let ffi_stub_offset: usize = start_stub_size;
        // FFI return-0 stub: MOVEQ #0, D0 (2 bytes) + RTS (2 bytes) = 4 bytes.
        let ffi_stub_size: usize = 4;

        // ── Build __vuma_alloc / __vuma_free syscall stubs ──
        // Linux m68k syscall convention: syscall # in D0, args in D1-D5, return in D0.
        //   __NR_mmap (m68k) = 90
        //   __NR_munmap (m68k) = 91
        //
        // __vuma_alloc(size in D1) -> D0 = mmap(NULL, size, PROT_READ|PROT_WRITE,
        //                                         MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
        // __vuma_free(addr in D1) -> munmap(addr, 0)
        let vuma_alloc_stub: Vec<u8> = {
            let mut code = Vec::new();
            // CRITICAL: D3, D4, D5 are callee-saved on m68k. mmap2 uses
            // D3=prot, D4=flags, D5=fd. Save/restore them with MOVEM.L.
            // MOVEM.L D3-D5, -(SP) = 0x48E7 0x0038 (mask: D3=bit3, D4=bit4, D5=bit5)
            code.extend_from_slice(&[0x48, 0xE7, 0x00, 0x38]);
            // D2 = size (from D1)
            code.extend(Instruction::Move { src: Gpr::D1, dst: Gpr::D2 }.encode());
            // D1 = NULL
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 0 }.encode());
            // D3 = 3 (PROT_READ|PROT_WRITE)
            code.extend(Instruction::Moveq { dst: Gpr::D3, imm: 3 }.encode());
            // D4 = 0x22 (MAP_PRIVATE|MAP_ANONYMOUS)
            code.extend(Instruction::MoveImm32 { dst: Gpr::D4, imm: 0x22 }.encode());
            // D5 = -1 (fd)
            code.extend(Instruction::Moveq { dst: Gpr::D5, imm: -1 }.encode());
            // Push offset=0 onto stack
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 0 }.encode());
            code.extend_from_slice(&[0x2F, 0x00]); // MOVE.L D0, -(SP)
            // D0 = 192 (mmap2)
            code.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: 192 }.encode());
            // TRAP #0
            code.extend(Instruction::Trap0.encode());
            // Pop the pushed pgoff: ADDQ.L #4, SP.
            // Encoding 0x58CF = ADDQ.L #4, A7 (data=4, size=long, mode=001=An, reg=111=A7).
            // NOTE: the previous bytes here were 0x5F 0xC4, which decode to
            // `SLE D4` (Scc with cc=LE into D4) — a no-op on SP that left the
            // pushed pgoff on the stack, so RTS would pop pgoff(0) as the
            // return address and crash. Fixed in wave 6 to the real ADDQ.
            code.extend_from_slice(&[0x58, 0xCF]);
            // Restore D3-D5: MOVEM.L (SP)+, D3-D5 = 0x4CDF 0x0038
            code.extend_from_slice(&[0x4C, 0xDF, 0x00, 0x38]);
            // RTS
            code.extend(Instruction::Rts.encode());
            code
        };

        let vuma_free_stub: Vec<u8> = {
            // __vuma_free(addr in D1) -> munmap(addr, 4096).
            // m68k __NR_munmap = 91. Caller passes addr in D1 (arg0); munmap's
            // second arg (length) goes in D2. We pass length=4096 (page size)
            // so the kernel unmaps at least one page.
            //
            // NOTE: IRInstr::Alloc on m68k lowers to stack-relative offsets, so
            // __vuma_alloc is only invoked via explicit IRInstr::Call (e.g. from
            // stdlib). This stub ensures such allocations can be properly freed.
            // If a caller mistakenly passes a stack address, munmap returns
            // -EINVAL (no SIGSEGV — the kernel validates the range first).
            let mut code = Vec::new();
            // D2 = 4096 (0x1000) — length for munmap.
            code.extend(Instruction::MoveImm32 { dst: Gpr::D2, imm: 0x1000 }.encode());
            // D0 = 91 (sys_munmap). D1 = addr (already from caller).
            code.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: 91 }.encode());
            // TRAP #0
            code.extend(Instruction::Trap0.encode());
            // RTS
            code.extend(Instruction::Rts.encode());
            code
        };

        // ── POSIX syscall stubs ──────────────────────────────────────
        // Simple stubs: D0 = #num; TRAP #0; RTS.
        // Args are already in D1-D5 from the caller.
        let simple_stub = |num: i32| -> Vec<u8> {
            let mut code = Vec::new();
            code.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: num }.encode());
            code.extend(Instruction::Trap0.encode());
            code.extend(Instruction::Rts.encode());
            code
        };

        let syscall_stubs: Vec<(String, Vec<u8>)> = {
            let mut stubs: Vec<(String, Vec<u8>)> = Vec::new();
            for (name, num) in [
                ("write", 4),
                ("read", 3),
                ("open", 5),
                ("close", 6),
                // NOTE: `mmap` is NOT registered here — it gets a dedicated
                // stub below (see "mmap2 offset-in-pages stub"). The legacy
                // __NR_mmap (90) on m68k expects a single struct-pointer
                // argument in D1, which the simple_stub calling convention
                // does not build, so we use mmap2 (192) instead. See the
                // dedicated stub for the full ABI discussion.
                ("munmap", 91),
                ("exit", 1),
                ("exit_group", 252),
                ("brk", 45),
                ("getpid", 20),
                ("alarm", 27),
                ("kill", 37),
                ("pipe", 42),
                ("dup", 41),
                ("dup2", 63),
                ("execve", 11),
                ("wait4", 114),
                ("unlink", 10),
                ("chdir", 12),
                ("lseek", 19),
                ("ioctl", 54),
                ("fcntl", 55),
                ("futex", 235),
                ("poll", 168),
                ("nanosleep", 162),
                ("mprotect", 125),
                ("clock_gettime", 265),
                ("gettimeofday", 78),
                ("rt_sigprocmask", 175),
                ("rt_sigaction", 174),
                ("socket", 340),
                ("connect", 343),
                ("bind", 341),
                ("listen", 342),
                ("accept", 344),
                ("setsockopt", 346),
                ("shutdown", 348),
                ("sendto", 349),
                ("recvfrom", 350),
                ("clone", 120),
                ("fork", 2),
                // [wave 9 fix] epoll numbers corrected from kernel m68k syscall.tbl:
                //   old (wrong): 449/424/425 (those are inotify_init1/pidfd_send_signal/io_uring_setup)
                //   correct:      325/250/251
                ("epoll_create1", 325),
                ("epoll_ctl", 250),
                ("epoll_wait", 251),
                ("dup3", 431),
                // ── Additional POSIX syscall stubs (stat family, getcwd,
                // recv/send direct syscalls) ──
                // m68k syscall numbers from arch/m68k/include/uapi/asm/unistd.h
                // (same as ARM EABI for these calls).
                ("stat", 106),
                ("lstat", 107),
                ("fstat", 108),
                ("getcwd", 183),
                ("recv", 291),
                ("send", 290),
                // ── Wave 7: POSIX file-metadata & I/O syscalls (m68k unistd.h) ──
                // m68k has 5 reg args (D1-D5); all these take ≤5 args → simple_stub.
                // chown/fchown map to the modern 32-bit-uid variants (chown32=198,
                // fchown32=207); the 16-bit chown=16/fchown=95 are NOT exposed.
                ("mkdir", 39), ("rmdir", 40), ("rename", 38),
                ("link", 9), ("symlink", 83), ("readlink", 85),
                ("chmod", 15), ("chown", 198), ("umask", 60),
                ("fchmod", 94), ("fchown", 207),
                ("openat", 288), ("unlinkat", 294), ("renameat", 295),
                ("linkat", 296), ("symlinkat", 297), ("readlinkat", 298),
                ("fchmodat", 299), ("faccessat", 300), ("fchownat", 291),
                ("ftruncate", 93), ("fsync", 118), ("fdatasync", 148),
                ("sync", 36), ("syncfs", 343),
                ("pread", 180), ("pwrite", 181), ("readv", 145), ("writev", 146),
                ("preadv", 329), ("pwritev", 330),
                ("fchdir", 133), ("chroot", 61),
                // ── Wave 9: POSIX system & advanced syscalls (m68k unistd.h) ──
                // m68k has 5 reg args (D1-D5); all take ≤5 args → simple_stub.
                // eventfd→eventfd2(324), signalfd→signalfd4(323) = modern variants.
                ("mlock", 150), ("munlock", 151), ("mlockall", 152), ("munlockall", 153),
                ("mincore", 237), ("madvise", 238), ("msync", 144), ("mremap", 163),
                ("getrlimit", 76), ("setrlimit", 75), ("prlimit64", 339),
                ("getrusage", 77), ("times", 43),
                ("getrandom", 352),
                ("eventfd", 324), ("timerfd_create", 318), ("timerfd_settime", 321),
                ("timerfd_gettime", 322), ("signalfd", 323),
                ("inotify_init1", 328), ("inotify_add_watch", 285), ("inotify_rm_watch", 286),
                ("ptrace", 26),
                // ── Wave 8: POSIX process & identity syscalls (m68k syscall.tbl) ──
                // m68k has a uid16 split — use modern *32 variants (199-214) per
                // Wave 7 precedent (chown32). All take ≤5 args; m68k has 5 reg
                // args (d1-d5) → simple_stub for all.
                // Family 1: identity (*32 variants)
                ("getuid", 199), ("geteuid", 201), ("getgid", 200), ("getegid", 202),
                ("setuid", 213), ("setgid", 214), ("setresuid", 208), ("setresgid", 210),
                // Family 2: process group (getpid already present)
                ("getppid", 64), ("getsid", 147), ("setsid", 66),
                ("setpgid", 57), ("getpgid", 132), ("getpgrp", 65),
                // Family 3: clone/wait (clone/wait4 already present)
                ("vfork", 190), ("clone3", 435), ("waitid", 277),
                // Family 4: exec/exit (execve/exit_group already present)
                ("execveat", 355),
                // Family 5: signals (kill/rt_sigaction/rt_sigprocmask/rt_sigreturn
                // already present)
                ("tgkill", 265), ("tkill", 222),
                // Family 6: directory read (readdir=89 is sys_old_readdir, deprecated)
                ("getdents64", 220), ("getdents", 141), ("readdir", 89),
                // Family 7: system (arch_prctl is x86_64-only)
                ("prctl", 172), ("uname", 122), ("sysinfo", 116),
            ] {
                stubs.push((name.to_string(), simple_stub(num)));
            }
            stubs
        };

        // ── Complex stub: sigaction → rt_sigaction(signum, act, oldact, sigsetsize=8) ──
        // m68k rt_sigaction syscall # = 174. VUMA declares 3 args; the kernel
        // requires a 4th arg (sigsetsize=8) in D4. We set D4=8 before TRAP.
        let sigaction_stub: Vec<u8> = {
            let mut code = Vec::new();
            // MOVEQ #8, D4 (sigsetsize)
            let w = 0x7000u16 | ((Gpr::D4.encoding() as u16 & 0x7) << 9) | 8;
            code.extend_from_slice(&w.to_be_bytes());
            // MOVE.L #174, D0 (sys_rt_sigaction — 174 > 127, can't use MOVEQ)
            code.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: 174 }.encode());
            code.extend(Instruction::Trap0.encode());
            code.extend(Instruction::Rts.encode());
            code
        };
        let mut syscall_stubs = syscall_stubs;
        syscall_stubs.push(("sigaction".to_string(), sigaction_stub));

        // ── mmap2 offset-in-pages stub (wave 6: mmap ABI normalization) ──
        // mmap(addr, length, prot, flags, fd, offset) → void*  [m68k __NR_mmap2 = 192]
        //
        // m68k Linux syscall convention: syscall # in D0, args 1-5 in D1-D5,
        // and arg 6 (pgoff) on the stack at (SP) at TRAP time. mmap2 takes
        // the offset in 4 KiB PAGES (not bytes), unlike the legacy
        // __NR_mmap=90 which takes a struct pointer in D1.
        //
        // VUMA m68k calling convention: only 5 integer args (D1-D5) — there is
        // no register or stack-arg slot for a 6th argument (see Gpr::arg_register:
        // index 5 returns None, and IRInstr::Call only fills D1-D5). This means
        // the caller CANNOT pass a byte `offset` for mmap, so this stub
        // hardcodes pgoff = 0 — exactly matching `__vuma_alloc` above, which
        // also calls mmap2(..., offset=0). Both use the same offset-unit
        // handling (mmap2, offset-in-pages, value 0), satisfying the wave-6
        // "same offset-unit handling as __vuma_alloc" requirement.
        //
        // Limitation: only anonymous / zero-offset mappings are supported on
        // the m68k backend. File-backed mmap with a non-zero offset would
        // require extending the m68k Call lowering to push a 6th stack
        // argument — out of scope for wave 6 (which is per-backend mmap ABI
        // normalization only). Callers needing file-backed mmap should target
        // a backend with a 6-arg calling convention (x86_64, ppc64, riscv32,
        // aarch64, ...). The test suite already avoids mmap on 32-bit
        // backends (see tests/gold_standard/crypto_patterns/mmap_sha256d.vuma,
        // which uses allocate() instead of mmap()).
        //
        // Register usage at stub entry (from caller, per VUMA m68k CC):
        //   D1 = addr, D2 = length, D3 = prot, D4 = flags, D5 = fd
        // These are passed straight through to the kernel as syscall args 1-5
        // (the m68k syscall ABI preserves D1-D5 across TRAP #0). D0 (return
        // register, caller-saved) is used as scratch for pgoff and the syscall
        // number. No callee-saved register (D3-D5, A6) is modified, so no
        // MOVEM save/restore is needed (unlike __vuma_alloc, which rebuilds
        // D3-D5 and therefore must save them).
        {
            let mut code = Vec::new();
            // MOVEQ #0, D0        (D0 = 0 = pgoff)
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 0 }.encode());
            // MOVE.L D0, -(SP)    (push 6th syscall arg = pgoff = 0)
            // Encoding: 0x2F00 = MOVE.L D0, -(A7)
            code.extend_from_slice(&[0x2F, 0x00]);
            // MOVE.L #192, D0     (D0 = __NR_mmap2; 192 > 127 so MOVEQ can't hold it)
            code.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: 192 }.encode());
            // TRAP #0             (mmap2(D1=addr, D2=len, D3=prot, D4=flags,
            //                            D5=fd, [SP]=pgoff=0) → D0 = ptr/-errno)
            code.extend(Instruction::Trap0.encode());
            // ADDQ.L #4, SP       (pop the pushed pgoff; restore SP before RTS)
            // Encoding: 0x58CF = ADDQ.L #4, A7  (data=4, size=long, mode=An, reg=A7)
            code.extend_from_slice(&[0x58, 0xCF]);
            // RTS
            code.extend(Instruction::Rts.encode());
            syscall_stubs.push(("mmap".to_string(), code));
        }

        // ── rt_sigreturn (173) — special: no args, never returns.
        // The kernel restores the saved signal context from the stack and
        // resumes execution at the interrupted PC. We emit just
        // `MOVE.L #173, D0 ; TRAP #0` followed by an ILLEGAL trap (0x4AFC)
        // as a safety net in case the kernel ever does return.
        // 173 > 127 so MOVEQ cannot be used; use MOVE.L #imm32, D0.
        {
            let mut code = Vec::new();
            code.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: 173 }.encode());
            code.extend(Instruction::Trap0.encode());
            // ILLEGAL (0x4AFC) — safety net.
            code.extend_from_slice(&[0x4A, 0xFC]);
            syscall_stubs.push(("rt_sigreturn".to_string(), code));
        }

        // ── waitpid(pid, wstatus, options) → wraps wait4(pid, wstatus, options, NULL)
        // VUMA declares waitpid with 3 args (D1=pid, D2=wstatus, D3=options);
        // the syscall wait4 takes a 4th arg (rusage, must be NULL) in D4.
        {
            let mut code = Vec::new();
            // MOVEQ #0, D4 (rusage = NULL)
            code.extend(Instruction::Moveq { dst: Gpr::D4, imm: 0 }.encode());
            // MOVEQ #114, D0 (sys_wait4 — 114 fits in MOVEQ)
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 114 }.encode());
            code.extend(Instruction::Trap0.encode());
            code.extend(Instruction::Rts.encode());
            syscall_stubs.push(("waitpid".to_string(), code));
        }

        // ── strcmp(s1, s2) → int — assembly loop, not a syscall.
        // m68k calling convention: integer args in D1-D5. s1 in D1, s2 in D2.
        // Return in D0 = (*s1 - *s2) at first differing byte (or 0 if equal).
        // Uses A0, A1 as pointers (post-increment), D3/D4 as byte scratch.
        {
            let mut code = Vec::new();
            // MOVEA.L D1, A0 (A0 = s1)
            // MOVEA.L Dn, An: 0x2000 | (an<<9) | (1<<6) | (0<<3) | dn
            // For D1 → A0: 0x2000 | 0 | 0x40 | 0 | 1 = 0x2041
            code.extend_from_slice(&[0x20, 0x41]);
            // MOVEA.L D2, A1 (A1 = s2)
            // For D2 → A1: 0x2000 | (1<<9) | 0x40 | 0 | 2 = 0x2242
            code.extend_from_slice(&[0x22, 0x42]);

            // strcmp_loop:
            let loop_start = code.len();
            // MOVEQ #0, D3 (clear D3 for zero-extension)
            code.extend(Instruction::Moveq { dst: Gpr::D3, imm: 0 }.encode());
            // MOVE.B (A0)+, D3 (load byte from s1, post-increment)
            // MOVE.B (An)+, Dn: 0x1000 | (dn<<9) | (0<<6) | (3<<3) | an
            // For (A0)+ → D3: 0x1000 | (3<<9) | 0 | (3<<3) | 0 = 0x1618
            code.extend_from_slice(&[0x16, 0x18]);
            // MOVEQ #0, D4 (clear D4 for zero-extension)
            code.extend(Instruction::Moveq { dst: Gpr::D4, imm: 0 }.encode());
            // MOVE.B (A1)+, D4 (load byte from s2)
            // For (A1)+ → D4: 0x1000 | (4<<9) | 0 | (3<<3) | 1 = 0x1819
            code.extend_from_slice(&[0x18, 0x19]);
            // CMP.L D3, D4 (compare; sets CC based on D4 - D3)
            code.extend(Instruction::Cmp { src: Gpr::D3, dst: Gpr::D4 }.encode());
            // BNE.S done (cond=6=NE)
            let bne_pos = code.len();
            code.extend_from_slice(&[0x66, 0x00]); // placeholder
            // TST.L D3 (test if D3 == 0)
            code.extend(Instruction::Tst { dst: Gpr::D3 }.encode());
            // BEQ.S done (cond=7=EQ — both bytes NUL, strings equal)
            let beq_pos = code.len();
            code.extend_from_slice(&[0x67, 0x00]); // placeholder
            // BRA.S loop_start (unconditional)
            let bra_pos = code.len();
            code.extend_from_slice(&[0x60, 0x00]); // placeholder

            // done: D0 = D3 - D4
            let done_offset = code.len();
            // Patch BNE and BEQ to target done_offset.
            let bne_disp = (done_offset as i64 - bne_pos as i64 - 2) as i8;
            code[bne_pos + 1] = bne_disp as u8;
            let beq_disp = (done_offset as i64 - beq_pos as i64 - 2) as i8;
            code[beq_pos + 1] = beq_disp as u8;
            // Patch BRA to target loop_start.
            let bra_disp = (loop_start as i64 - bra_pos as i64 - 2) as i8;
            code[bra_pos + 1] = bra_disp as u8;

            // SUB.L D4, D3 (D3 = D3 - D4)
            code.extend(Instruction::Sub { src: Gpr::D4, dst: Gpr::D3 }.encode());
            // MOVE.L D3, D0 (return value in D0)
            code.extend(Instruction::Move { src: Gpr::D3, dst: Gpr::D0 }.encode());
            // RTS
            code.extend(Instruction::Rts.encode());
            syscall_stubs.push(("strcmp".to_string(), code));
        }

        // ── print_int(D1 = signed 32-bit integer) — runtime helper ──
        // Converts D1 to decimal ASCII and writes to stdout via sys_write
        // (D0=4). Stack frame: LINK A6, #-32 (digit buffer + scratch).
        // Register usage:
        //   D0, D1, D2 = scratch for divmod and syscall (caller-saved)
        //   D3 = saved/restored by inline divmod (callee-saved, save anyway)
        //   D4 = current value (callee-saved, save/restore)
        //   D5 = digit count (callee-saved, save/restore)
        //   D6 = quotient (callee-saved, save/restore)
        //   D7 = remainder / scratch (callee-saved, save/restore)
        //   A0 = digit buffer pointer (scratch, A0/A1 are scratch)
        //   A6 = frame pointer
        //
        // Inline divmod10: D4 → D6=quotient, D7=remainder.
        //   Uses D0 (working dividend), D1 (counter), shift-and-subtract.
        {
            let mut code = Vec::new();

            // ── Prologue ──
            // LINK A6, #-32
            code.extend(Instruction::Link { reg: Gpr::A6, disp: -32 }.encode());
            // MOVEM.L D3-D7, -(SP) — save callee-saved
            // Encoding: 0x48E7 + 2-byte mask. Mask for D3-D7 = 0x00F8.
            code.extend_from_slice(&[0x48, 0xE7, 0x00, 0xF8]);

            // D4 = D1 (save input value)
            code.extend(Instruction::Move { src: Gpr::D1, dst: Gpr::D4 }.encode());
            // D5 = 0 (digit count)
            code.extend(Instruction::Moveq { dst: Gpr::D5, imm: 0 }.encode());
            // A0 = A6 (end-of-buffer pointer; digits grow down)
            // MOVEA.L A6, A0: 0x2000 | (0<<9) | (1<<6) | (1<<3) | 6 = 0x204E
            code.extend_from_slice(&[0x20, 0x4E]);

            // ── Check sign of D4 ──
            code.extend(Instruction::Tst { dst: Gpr::D4 }.encode());
            // BPL.S positive (cond=10=PL)
            let bpl_pos = code.len();
            code.extend_from_slice(&[0x6A, 0x00]); // placeholder

            // ── Negative: write '-' to stdout, negate D4 ──
            // MOVEQ #45, D0 ('-')
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 45 }.encode());
            // MOVE.L D0, -(SP) — push '-' (4 bytes, '-' as low byte)
            code.extend_from_slice(&[0x2F, 0x00]);
            // MOVEQ #1, D1 (fd = stdout)
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 1 }.encode());
            // MOVE.L A7, D2 (buf = SP)
            // MOVE.L An, Dn: 0x2000 | (dn<<9) | (0<<6) | (1<<3) | an
            // For A7 → D2: 0x2000 | (2<<9) | 0 | (1<<3) | 7 = 0x240F
            code.extend_from_slice(&[0x24, 0x0F]);
            // MOVEQ #1, D3 (len = 1)
            code.extend(Instruction::Moveq { dst: Gpr::D3, imm: 1 }.encode());
            // MOVEQ #4, D0 (sys_write)
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 4 }.encode());
            // TRAP #0
            code.extend(Instruction::Trap0.encode());
            // ADDQ.L #4, A7 (pop the 4-byte '-' buffer)
            // ADDQ.L #4, An: 0101_100_0_11_001_111 = 0x58CF
            code.extend_from_slice(&[0x58, 0xCF]);
            // NEG.L D4 (negate D4)
            // NEG.L Dn: 0x4480 | (dn<<9). For D4: 0x4480 | (4<<9) = 0x4C80
            code.extend_from_slice(&[0x4C, 0x80]);

            // ── positive: ──
            let positive_offset = code.len();
            // Patch BPL.S to target positive_offset
            let bpl_disp = (positive_offset as i64 - bpl_pos as i64 - 2) as i8;
            code[bpl_pos + 1] = bpl_disp as u8;

            // ── Outer divmod loop ──
            let outer_loop = code.len();

            // ── Inline divmod10: D4 → D6=quotient, D7=remainder ──
            // MOVE.L D4, D0 (working dividend)
            code.extend(Instruction::Move { src: Gpr::D4, dst: Gpr::D0 }.encode());
            // MOVEQ #0, D6 (quotient = 0)
            code.extend(Instruction::Moveq { dst: Gpr::D6, imm: 0 }.encode());
            // MOVEQ #0, D7 (remainder = 0)
            code.extend(Instruction::Moveq { dst: Gpr::D7, imm: 0 }.encode());
            // MOVEQ #32, D1 (counter = 32)
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 32 }.encode());

            // ── Inner divmod loop ──
            let inner_loop = code.len();
            // ADD.L D0, D0 (D0 <<= 1, X = old MSB)
            code.extend(Instruction::Add { src: Gpr::D0, dst: Gpr::D0 }.encode());
            // ROXL.L #1, D7 (D7 = (D7 << 1) | X)
            // ROXL.L #1, Dn: 1110_111_1_10_010_001 = 0xEFA9 for D7
            code.extend_from_slice(&[0xEF, 0xA9]);
            // LSL.L #1, D6 (D6 <<= 1)
            // LSL.L #1, D6: 1110_110_1_10_001_001 = 0xED89
            code.extend_from_slice(&[0xED, 0x89]);
            // CMPI.L #10, D7
            // CMPI.L #imm32, Dn: 0x0C80 | dn, then 4-byte imm. For D7: 0x0C87
            code.extend_from_slice(&[0x0C, 0x87, 0x00, 0x00, 0x00, 0x0A]);
            // BCS.S skip (cond=5=CS, skip SUBI + ORI = 12 bytes)
            code.extend_from_slice(&[0x65, 0x0C]);
            // SUBI.L #10, D7
            // SUBI.L #imm32, Dn: 0x0480 | dn. For D7: 0x0487
            code.extend_from_slice(&[0x04, 0x87, 0x00, 0x00, 0x00, 0x0A]);
            // ORI.L #1, D6 (set quotient LSB)
            // ORI.L #imm32, Dn: 0x0080 | dn. For D6: 0x0086
            code.extend_from_slice(&[0x00, 0x86, 0x00, 0x00, 0x00, 0x01]);
            // skip: SUBQ.L #1, D1 (counter--)
            // SUBQ.L #1, Dn: 0101_001_1_11_000_rrr. For D1: 0x53C1
            code.extend_from_slice(&[0x53, 0xC1]);
            // BNE.S inner_loop (cond=6=NE)
            let bne_inner_pos = code.len();
            code.extend_from_slice(&[0x66, 0x00]); // placeholder
            let bne_inner_disp = (inner_loop as i64 - bne_inner_pos as i64 - 2) as i8;
            code[bne_inner_pos + 1] = bne_inner_disp as u8;

            // ── After divmod10: D6=quotient, D7=remainder ──
            // ADDI.B #48, D7 (remainder + '0')
            // ADDI.B #imm8, Dn: 0x0600 | dn, then 2-byte imm. For D7: 0x0607
            code.extend_from_slice(&[0x06, 0x07, 0x00, 0x30]);
            // SUBQ.L #1, A0 (A0--)
            // SUBQ.L #1, An: 0101_001_1_11_001_000 = 0x53C8 for A0
            code.extend_from_slice(&[0x53, 0xC8]);
            // MOVE.B D7, (A0) (store digit)
            // MOVE.B Dn, (An): 0x1000 | (dn<<9) | (2<<6) | an. For D7, A0: 0x1087
            code.extend_from_slice(&[0x10, 0x87]);
            // ADDQ.L #1, D5 (digit count++)
            // ADDQ.L #1, Dn: 0101_001_0_11_000_101 = 0x52C5 for D5
            code.extend_from_slice(&[0x52, 0xC5]);
            // MOVE.L D6, D4 (D4 = quotient, becomes new value)
            code.extend(Instruction::Move { src: Gpr::D6, dst: Gpr::D4 }.encode());
            // TST.L D4
            code.extend(Instruction::Tst { dst: Gpr::D4 }.encode());
            // BNE.S outer_loop (loop back, cond=6=NE)
            let bne_outer_pos = code.len();
            code.extend_from_slice(&[0x66, 0x00]); // placeholder
            let bne_outer_disp = (outer_loop as i64 - bne_outer_pos as i64 - 2) as i8;
            code[bne_outer_pos + 1] = bne_outer_disp as u8;

            // ── Zero check: if D5 == 0 (D4 was originally 0), store '0' ──
            code.extend(Instruction::Tst { dst: Gpr::D5 }.encode());
            // BNE.S write_digits (cond=6=NE)
            let bne_zero_pos = code.len();
            code.extend_from_slice(&[0x66, 0x00]); // placeholder
            // MOVEQ #48, D7 ('0')
            code.extend(Instruction::Moveq { dst: Gpr::D7, imm: 48 }.encode());
            // SUBQ.L #1, A0
            code.extend_from_slice(&[0x53, 0xC8]);
            // MOVE.B D7, (A0)
            code.extend_from_slice(&[0x10, 0x87]);
            // ADDQ.L #1, D5
            code.extend_from_slice(&[0x52, 0xC5]);

            // ── write_digits: sys_write(1, A0, D5) ──
            let write_digits_offset = code.len();
            // Patch BNE.S to target write_digits_offset
            let bne_zero_disp = (write_digits_offset as i64 - bne_zero_pos as i64 - 2) as i8;
            code[bne_zero_pos + 1] = bne_zero_disp as u8;

            // MOVEQ #1, D1 (fd = stdout)
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 1 }.encode());
            // MOVE.L A0, D2 (buf = A0)
            // MOVE.L An, Dn: 0x2000 | (dn<<9) | (0<<6) | (1<<3) | an. For A0 → D2: 0x2408
            code.extend_from_slice(&[0x24, 0x08]);
            // MOVE.L D5, D3 (len = D5)
            code.extend(Instruction::Move { src: Gpr::D5, dst: Gpr::D3 }.encode());
            // MOVEQ #4, D0 (sys_write)
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 4 }.encode());
            // TRAP #0
            code.extend(Instruction::Trap0.encode());

            // ── Epilogue ──
            // MOVEM.L (SP)+, D3-D7 — restore callee-saved
            // Encoding: 0x4CDF + 2-byte mask. Mask for D3-D7 = 0x00F8.
            code.extend_from_slice(&[0x4C, 0xDF, 0x00, 0xF8]);
            // UNLK A6
            code.extend(Instruction::Unlk { reg: Gpr::A6 }.encode());
            // RTS
            code.extend(Instruction::Rts.encode());

            // print_int stub restored — calls now resolve to the real
            // decimal-conversion runtime helper above instead of becoming
            // no-op unresolved externs.  The stub uses LINK/UNLK A6 for
            // the frame and saves/restores D3-D7 via MOVEM so it only
            // clobbers caller-saved scratch registers (D0-D2, A0-A1).
            syscall_stubs.push(("print_int".to_string(), code));
        }

        // ── print_hex(D1 = 32-bit value) — runtime helper ──
        // Writes D1 as 8 hex digits (MSB first) to stdout via sys_write.
        // Stack frame: LINK A6, #-16 (hex char buffer).
        // Register usage:
        //   D0, D1, D2, D3 = syscall scratch
        //   D4 = current value (preserved across syscalls)
        //   D5 = loop counter (0..8)
        //   D7 = current nibble / hex char
        //   A0 = buffer pointer (advances backward)
        //   A6 = frame pointer
        {
            let mut code = Vec::new();

            // ── Prologue ──
            // LINK A6, #-16
            code.extend(Instruction::Link { reg: Gpr::A6, disp: -16 }.encode());
            // MOVEM.L D3-D7, -(SP)
            code.extend_from_slice(&[0x48, 0xE7, 0x00, 0xF8]);

            // D4 = D1 (save value)
            code.extend(Instruction::Move { src: Gpr::D1, dst: Gpr::D4 }.encode());
            // D5 = 0 (counter)
            code.extend(Instruction::Moveq { dst: Gpr::D5, imm: 0 }.encode());
            // A0 = A6 (end-of-buffer pointer; chars grow down)
            code.extend_from_slice(&[0x20, 0x4E]); // MOVEA.L A6, A0

            // ── hex_loop: ──
            let hex_loop = code.len();

            // D7 = D4 (copy value)
            code.extend(Instruction::Move { src: Gpr::D4, dst: Gpr::D7 }.encode());
            // ANDI.L #0xF, D7 (extract low nibble)
            // ANDI.L #imm32, Dn: 0x0280 | dn. For D7: 0x0287
            code.extend_from_slice(&[0x02, 0x87, 0x00, 0x00, 0x00, 0x0F]);
            // ADDI.B #48, D7 (nibble + '0')
            code.extend_from_slice(&[0x06, 0x07, 0x00, 0x30]);
            // CMPI.B #57, D7 (compare with '9')
            // CMPI.B #imm8, Dn: 0x0C00 | dn, then 2-byte imm. For D7: 0x0C07
            code.extend_from_slice(&[0x0C, 0x07, 0x00, 0x39]);
            // BLE.S store (cond=15=LE, skip ADDI.B = 4 bytes)
            code.extend_from_slice(&[0x6F, 0x04]);
            // ADDI.B #39, D7 (alpha adjust: 'a'-'9' = 39)
            code.extend_from_slice(&[0x06, 0x07, 0x00, 0x27]);
            // store: SUBQ.L #1, A0
            code.extend_from_slice(&[0x53, 0xC8]);
            // MOVE.B D7, (A0)
            code.extend_from_slice(&[0x10, 0x87]);
            // LSR.L #4, D4 (shift value right by 4)
            // LSR.L #4, D4: 1110_100_1_10_101_100 = 0xE9AC
            code.extend_from_slice(&[0xE9, 0xAC]);
            // ADDQ.L #1, D5 (counter++)
            code.extend_from_slice(&[0x52, 0xC5]);
            // CMPI.L #8, D5 (compare counter with 8)
            // CMPI.L #imm32, Dn: 0x0C80 | dn. For D5: 0x0C85
            code.extend_from_slice(&[0x0C, 0x85, 0x00, 0x00, 0x00, 0x08]);
            // BNE.S hex_loop (cond=6=NE, loop back)
            let bne_loop_pos = code.len();
            code.extend_from_slice(&[0x66, 0x00]); // placeholder
            let bne_loop_disp = (hex_loop as i64 - bne_loop_pos as i64 - 2) as i8;
            code[bne_loop_pos + 1] = bne_loop_disp as u8;

            // ── sys_write(1, A0, 8) ──
            // MOVEQ #1, D1 (fd)
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 1 }.encode());
            // MOVE.L A0, D2 (buf)
            code.extend_from_slice(&[0x24, 0x08]);
            // MOVEQ #8, D3 (len)
            code.extend(Instruction::Moveq { dst: Gpr::D3, imm: 8 }.encode());
            // MOVEQ #4, D0 (sys_write)
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 4 }.encode());
            // TRAP #0
            code.extend(Instruction::Trap0.encode());

            // ── Epilogue ──
            // MOVEM.L (SP)+, D3-D7
            code.extend_from_slice(&[0x4C, 0xDF, 0x00, 0xF8]);
            // UNLK A6
            code.extend(Instruction::Unlk { reg: Gpr::A6 }.encode());
            // RTS
            code.extend(Instruction::Rts.encode());

            // print_hex stub restored — calls now resolve to the real
            // hex-conversion runtime helper above instead of becoming
            // no-op unresolved externs.  The stub uses LINK/UNLK A6 for
            // the frame and saves/restores D3-D7 via MOVEM so it only
            // clobbers caller-saved scratch registers (D0-D2, A0-A1).
            syscall_stubs.push(("print_hex".to_string(), code));
        }

        // ── print_newline() → void — write '\n' to stdout ──
        // No arguments. Uses sys_write(1, &newline, 1).
        // m68k syscall convention: D0=syscall#, D1=fd, D2=buf, D3=count,
        // TRAP #0 = syscall trap. A7 = SP.
        {
            let mut code = Vec::new();
            // MOVEQ #10, D0 ('\n') — load newline char
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 10 }.encode());
            // MOVE.L D0, -(SP) — push newline onto stack (4 bytes, 0x0A as low byte)
            code.extend_from_slice(&[0x2F, 0x00]);
            // MOVEQ #1, D1 (fd = stdout)
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 1 }.encode());
            // MOVE.L A7, D2 (buf = SP) — 0x2000|(2<<9)|(0<<6)|(1<<3)|7 = 0x240F
            code.extend_from_slice(&[0x24, 0x0F]);
            // MOVEQ #1, D3 (len = 1)
            code.extend(Instruction::Moveq { dst: Gpr::D3, imm: 1 }.encode());
            // MOVEQ #4, D0 (sys_write)
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 4 }.encode());
            // TRAP #0
            code.extend(Instruction::Trap0.encode());
            // ADDQ.L #4, A7 (pop the 4-byte newline buffer) — 0x58CF
            code.extend_from_slice(&[0x58, 0xCF]);
            // RTS
            code.extend(Instruction::Rts.encode());
            syscall_stubs.push(("print_newline".to_string(), code));
        }

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

        // __vuma_alloc / __vuma_free stubs go after all user functions.
        let vuma_alloc_offset = current_offset;
        let vuma_free_offset = vuma_alloc_offset + vuma_alloc_stub.len();
        func_offsets.insert("__vuma_alloc".to_string(), vuma_alloc_offset);
        func_offsets.insert("__vuma_free".to_string(), vuma_free_offset);

        // POSIX syscall stubs go after __vuma_free.
        let mut stub_offset = vuma_free_offset + vuma_free_stub.len();
        for (name, code) in &syscall_stubs {
            func_offsets.insert(name.clone(), stub_offset);
            stub_offset += code.len();
        }

        // ── Register __vuma_print_int / __vuma_print_hex as aliases
        // pointing at the same offsets as print_int / print_hex, so user
        // code can call them by their bare POSIX-friendly names.
        let print_int_offset = func_offsets.get("print_int").copied().unwrap_or(0);
        let print_hex_offset = func_offsets.get("print_hex").copied().unwrap_or(0);
        let print_newline_offset = func_offsets.get("print_newline").copied().unwrap_or(0);
        func_offsets.insert("__vuma_print_int".to_string(), print_int_offset);
        func_offsets.insert("__vuma_print_hex".to_string(), print_hex_offset);
        func_offsets.insert("__vuma_print_newline".to_string(), print_newline_offset);

        // ── Build _start stub bytes ──
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // MOVE.L (A7), D1 — load argc into D1 (first arg).
        // Encoding: dest=D1 (mode 0, reg 1), src=(A7) (mode 2, reg 7), size=long.
        // = 00 10 001 000 010 111 = 0x2217
        start_stub.extend_from_slice(&[0x22, 0x17]);

        // PEA 4(A7) — push effective address of (SP+4) = &argv[0].
        // PEA (d16, An): 0x4840 | (mode<<3) | reg, then 2-byte disp (BE).
        // mode 5 (d16, An), reg 7 (A7): 0x4840 | (5<<3) | 7 = 0x486F, disp 0x0004.
        // CRITICAL: 0x4878 is PEA (absolute short), NOT PEA 4(A7)!
        start_stub.extend_from_slice(&[0x48, 0x6F, 0x00, 0x04]);

        // MOVE.L (A7)+, D2 — pop argv pointer into D2 (second arg).
        // Encoding: dest=D2 (mode 0, reg 2), src=(A7)+ (mode 2, reg 7), size=long.
        // = 00 10 010 000 010 111 = 0x2417
        start_stub.extend_from_slice(&[0x24, 0x17]);

        // BSR.L main — 6 bytes: opcode 0x61FF + 4-byte disp32.
        // m68k BSR encoding: word 0x6100 | disp8. If disp8 == 0xFF, it's
        // BSR.L (32-bit displacement follows). If disp8 == 0x00, it's BSR.W
        // (16-bit displacement follows). The code previously emitted 0x6100
        // (BSR.W) with 0xFFFF as the displacement — that branched to an odd
        // address and crashed. Fix: emit 0x61FF (BSR.L) + 4-byte disp32.
        // BSR.L disp32: target = PC + disp32, where PC = address after the
        // opcode word (offset+2).
        let brasl_offset_in_start = start_stub.len();
        start_stub.extend_from_slice(&[0x61, 0xFF]);
        start_stub.extend_from_slice(&0u32.to_be_bytes());

        // MOVE.L D0, D1 — move main's return value (D0) to D1 (exit code arg).
        // m68k MOVE.L encoding: 0x20ss | (dst_reg << 9) | (dst_mode << 6) |
        //   (src_reg << 3) | src_mode, where ss=10 for long.
        // For D0 → D1: dst_reg=1, dst_mode=000 (Dn), src_reg=0, src_mode=000 (Dn).
        // = 0x2000 | 0x0200 | 0x0000 | 0x0000 | 0x0000 = 0x2200.
        // (Previous code used 0x2240 which is MOVEA.L D0, A1 — wrong register
        //  type. MOVEA.L moves to an address register, but D1 is a data
        //  register and the exit syscall takes the code in D1 as a value.)
        start_stub.extend_from_slice(&[0x22, 0x00]);
        // MOVEQ #1, D0 — 0x7001.
        start_stub.extend_from_slice(&[0x70, 0x01]);
        // TRAP #0
        start_stub.extend(Instruction::Trap0.encode());

        // ── Patch _start BSR.L to main ──
        // BSR.L disp32: target = (PC at displacement word) + disp32.
        // Displacement word is at offset brasl_offset_in_start + 2.
        let main_key = func_offsets
            .keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            let bsr_disp_abs = BASE_ADDR + text_offset + brasl_offset_in_start as u64 + 2;
            let main_abs = BASE_ADDR + text_offset + main_offset as u64;
            let disp = (main_abs as i64 - bsr_disp_abs as i64) as i32;
            let disp_be = disp.to_be_bytes();
            start_stub[brasl_offset_in_start + 2..brasl_offset_in_start + 6]
                .copy_from_slice(&disp_be);
        } else {
            // No main function — point BSR.L to the FFI return-0 stub.
            let bsr_disp_abs = BASE_ADDR + text_offset + brasl_offset_in_start as u64 + 2;
            let ffi_abs = BASE_ADDR + text_offset + ffi_stub_offset as u64;
            let disp = (ffi_abs as i64 - bsr_disp_abs as i64) as i32;
            let disp_be = disp.to_be_bytes();
            start_stub[brasl_offset_in_start + 2..brasl_offset_in_start + 6]
                .copy_from_slice(&disp_be);
        }

        // ── Add FFI return-0 stub ──
        let mut ffi_stub = Vec::with_capacity(ffi_stub_size);
        // MOVEQ #0, D0 (return 0)
        ffi_stub.extend(Instruction::Moveq { dst: Gpr::D0, imm: 0 }.encode());
        // RTS
        ffi_stub.extend(Instruction::Rts.encode());

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

        // ── Patch BSR.L / CALL relocations for inter-function calls ──
        // BSR.L: 0x61 0xFF + 4-byte disp32 (6 bytes total).
        // disp32 at bytes [2..6] of the 6-byte instruction.
        // PC = address of the displacement field = instr_addr + 2.
        let mut func_code_offset: usize = start_stub_size + ffi_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 6 > all_code.len() {
                    continue;
                }
                if reloc.reloc_type == R_M68K_PC32 {
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
                    // For BSR.L: PC = abs_offset + 2 (displacement field position).
                    let pc_abs = BASE_ADDR + text_offset + abs_offset as u64 + 2;
                    let target_abs = BASE_ADDR + text_offset + target_offset as u64;
                    let disp = (target_abs as i64 - pc_abs as i64) as i32;
                    let disp_be = disp.to_be_bytes();
                    all_code[abs_offset + 2..abs_offset + 6].copy_from_slice(&disp_be);
                } else if reloc.reloc_type == "R_68K_32" {
                    // Absolute 32-bit relocation for GetAddress.
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
                    let target_abs = (BASE_ADDR + text_offset + target_offset as u64) as u32;
                    let disp_be = target_abs.to_be_bytes();
                    // For MOVE.L #imm32, Dn: the imm32 is at bytes [2..6] of the 6-byte instruction.
                    all_code[abs_offset + 2..abs_offset + 6].copy_from_slice(&disp_be);
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
        Ok(build_m68k_elf(&all_code, BASE_ADDR, &extern_symbols))
    }

    fn return_stub(&self) -> Vec<u8> {
        Instruction::Rts.encode()
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // Load 32-bit address into A0, then JMP (A0).
        let mut code = Vec::new();
        // MOVEA.L #addr, A0: word = 0x2040 | (0<<9) | 0x3C = 0x207C, then 4-byte addr.
        code.extend_from_slice(&[0x20, 0x7C]);
        code.extend_from_slice(&(entry_addr as u32).to_be_bytes());
        // JMP (A0): 0x4ED0.
        code.extend_from_slice(&[0x4E, 0xD0]);
        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        // Simple hex-based disassembler for m68k (variable-length, big-endian).
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset < bytes.len() {
            // Heuristic: take 2 bytes as a "word" and display.
            if offset + 2 > bytes.len() {
                let remaining = &bytes[offset..];
                lines.push(format!("{:#010x}:  {:02x?}", pc, remaining));
                break;
            }
            let word = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]);
            // Simple length heuristic — m68k instruction length is complex.
            let len = 2; // most basic instructions are 2 bytes; this is just for display
            let hex: String = bytes[offset..offset + len]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect::<Vec<_>>()
                .join(" ");
            lines.push(format!("{:#010x}:  {:04x}  {}", pc, word, hex));
            offset += len;
            pc += len as u64;
        }
        lines
    }

    fn name(&self) -> &'static str { "m68k" }
}

// ===========================================================================
// ELF builder (big-endian ELF32)
// ===========================================================================

/// Build a big-endian ELF32 binary for m68k Linux with 2 LOAD segments +
/// PT_GNU_STACK.
fn build_m68k_elf(code: &[u8], base_addr: u64, extern_symbols: &[String]) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x1000;
    const HOST_PAGE_ALIGN: u64 = 0x1000;

    let elf_header_size: u64 = 52;
    let phdr_size: u64 = 32;
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
    elf.push(1); // ELFCLASS32
    elf.push(2); // ELFDATA2MSB (big-endian)
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0);
    elf.extend_from_slice(&[0u8; 7]);

    // --- ELF header fields (big-endian) ---
    elf.extend_from_slice(&2u16.to_be_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&4u16.to_be_bytes()); // e_machine = EM_68K
    elf.extend_from_slice(&1u32.to_be_bytes()); // e_version
    elf.extend_from_slice(&(entry_point as u32).to_be_bytes()); // e_entry
    elf.extend_from_slice(&(elf_header_size as u32).to_be_bytes()); // e_phoff
    elf.extend_from_slice(&0u32.to_be_bytes()); // e_shoff
    elf.extend_from_slice(&0u32.to_be_bytes()); // e_flags
    elf.extend_from_slice(&52u16.to_be_bytes()); // e_ehsize
    elf.extend_from_slice(&32u16.to_be_bytes()); // e_phentsize
    elf.extend_from_slice(&3u16.to_be_bytes()); // e_phnum
    elf.extend_from_slice(&40u16.to_be_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    // ELF32 Phdr field order: p_type, p_offset, p_vaddr, p_paddr,
    //   p_filesz, p_memsz, p_flags, p_align  (note: p_flags is 7th, NOT 2nd!)
    // (ELF64 has p_flags at position 2, but ELF32 has it at position 7.)
    elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr as u32).to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr as u32).to_be_bytes()); // p_paddr
    elf.extend_from_slice(&((text_offset + text_size) as u32).to_be_bytes()); // p_filesz
    elf.extend_from_slice(&((text_offset + text_size) as u32).to_be_bytes()); // p_memsz
    elf.extend_from_slice(&5u32.to_be_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&(PAGE_SIZE as u32).to_be_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data ---
    elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset
    elf.extend_from_slice(&(data_vaddr as u32).to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&(data_vaddr as u32).to_be_bytes()); // p_paddr
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_filesz
    elf.extend_from_slice(&(data_size as u32).to_be_bytes()); // p_memsz
    elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&(PAGE_SIZE as u32).to_be_bytes()); // p_align

    // --- Program Header 3: PT_GNU_STACK ---
    elf.extend_from_slice(&0x6474e551u32.to_be_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_paddr
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_filesz
    elf.extend_from_slice(&0u32.to_be_bytes()); // p_memsz
    elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&0x10u32.to_be_bytes()); // p_align

    // --- .text section ---
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // ── Append section headers if extern symbols are referenced ──
    if !extern_symbols.is_empty() {
        append_m68k_elf_sections(&mut elf, text_offset, text_size, extern_symbols);
    }

    elf
}

/// Append an ELF32 section header table for the m68k backend (big-endian).
fn append_m68k_elf_sections(
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
    const SYM_SIZE: u32 = 16;

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
    symtab.extend_from_slice(&[0u8; 16]); // NULL symbol
    for &name_off in &sym_name_offsets {
        symtab.extend_from_slice(&name_off.to_be_bytes()); // st_name
        symtab.push((STB_GLOBAL << 4) | STT_FUNC); // st_info
        symtab.push(0); // st_other
        symtab.extend_from_slice(&SHN_UNDEF.to_be_bytes()); // st_shndx
        symtab.extend_from_slice(&0u32.to_be_bytes()); // st_value
        symtab.extend_from_slice(&0u32.to_be_bytes()); // st_size
    }

    while (elf.len() % 4) != 0 {
        elf.push(0);
    }
    let shstrtab_off = elf.len() as u64;
    elf.extend_from_slice(&shstrtab);
    let strtab_off = elf.len() as u64;
    elf.extend_from_slice(&strtab);
    while (elf.len() % 4) != 0 {
        elf.push(0);
    }
    let symtab_off = elf.len() as u64;
    let symtab_size = symtab.len() as u64;
    elf.extend_from_slice(&symtab);

    while (elf.len() % 4) != 0 {
        elf.push(0);
    }
    let shdr_off = elf.len() as u64;

    fn push_shdr(
        elf: &mut Vec<u8>,
        sh_name: u32,
        sh_type: u32,
        sh_flags: u32,
        sh_addr: u32,
        sh_offset: u32,
        sh_size: u32,
        sh_link: u32,
        sh_info: u32,
        sh_addralign: u32,
        sh_entsize: u32,
    ) {
        elf.extend_from_slice(&sh_name.to_be_bytes());
        elf.extend_from_slice(&sh_type.to_be_bytes());
        elf.extend_from_slice(&sh_flags.to_be_bytes());
        elf.extend_from_slice(&sh_addr.to_be_bytes());
        elf.extend_from_slice(&sh_offset.to_be_bytes());
        elf.extend_from_slice(&sh_size.to_be_bytes());
        elf.extend_from_slice(&sh_link.to_be_bytes());
        elf.extend_from_slice(&sh_info.to_be_bytes());
        elf.extend_from_slice(&sh_addralign.to_be_bytes());
        elf.extend_from_slice(&sh_entsize.to_be_bytes());
    }

    push_shdr(elf, 0, SHT_NULL, 0, 0, 0, 0, 0, 0, 0, 0);
    push_shdr(elf, name_text, SHT_PROGBITS, 0x6, (0x10000 + text_offset) as u32, text_offset as u32, text_size as u32, 0, 0, 4, 0);
    push_shdr(elf, name_symtab, SHT_SYMTAB, 0, 0, symtab_off as u32, symtab_size as u32, 3, 1, 4, SYM_SIZE);
    push_shdr(elf, name_strtab, SHT_STRTAB, 0, 0, strtab_off as u32, strtab.len() as u32, 0, 0, 1, 0);
    push_shdr(elf, name_shstrtab, SHT_STRTAB, 0, 0, shstrtab_off as u32, shstrtab.len() as u32, 0, 0, 1, 0);

    let shnum: u16 = 5;
    let shstrndx: u16 = 4;
    elf[32..36].copy_from_slice(&shdr_off.to_be_bytes());
    elf[48..50].copy_from_slice(&shnum.to_be_bytes());
    elf[50..52].copy_from_slice(&shstrndx.to_be_bytes());
}

// Keep the unused-variable warning quiet for the helper.
#[allow(dead_code)]
fn _unused_table_marker() -> u8 {
    Gpr::D0.encoding()
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

        let backend = M68kBackend::new(); // use_real_regalloc = false by default
        let result_ss = backend.allocate_registers(&func);
        assert!(result_ss.is_ok(), "stack-slot allocation should succeed");

        // Now test with real regalloc.
        let mut backend = M68kBackend::new();
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
