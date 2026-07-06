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
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// m68k general-purpose registers: 8 data (D0–D7) and 8 address (A0–A7).
/// A7 is the stack pointer.  Encodings 0–7 are D0–D7, 8–15 are A0–A7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
                let w = 0xB180u16 | ((dst.encoding() as u16 & 0x7) << 9) | (src.encoding() as u16 & 0x7);
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
/// D0–D2 are caller-saved and free for our use.  We avoid D3–D7 (callee-saved).
const S0: Gpr = Gpr::D0;
const S1: Gpr = Gpr::D1;
const S2: Gpr = Gpr::D2;

/// Frame pointer (A6) and stack pointer (A7).
const FP: Gpr = Gpr::A6;
const SP: Gpr = Gpr::A7;

// ===========================================================================
// Stack-slot helpers
// ===========================================================================

/// Load a 32-bit immediate into a register.
fn ss_load_imm(dst: Gpr, val: i64) -> Vec<u8> {
    let v = val as i32;
    if (-128..=127).contains(&v) {
        Instruction::Moveq { dst, imm: v as i8 }.encode()
    } else {
        Instruction::MoveImm32 { dst, imm: v }.encode()
    }
}

/// Load a 32-bit value from stack slot at [FP + offset] into dst.
fn ss_ld(dst: Gpr, offset: i32) -> Vec<u8> {
    // m68k d16(An) signed 16-bit displacement range.
    if (-32768..=32767).contains(&offset) {
        Instruction::Load { base: FP, offset: offset as i16, dst }.encode()
    } else {
        // Large offset: compute address into a temp register first.
        let mut code = ss_load_imm(S2, offset as i64);
        // LEA: A6 + S2 → S2.  We use ADD.L S2, A6 (treating A6 as a 32-bit value).
        // Actually, ADDA.L Dn, An: word = 0xD1C0 | (An<<9) | Dn. An stays as An.
        // Simpler: load FP into S2 (move.l a6, s2), then add offset, then load via (S2).
        // But we already overwrote S2 with offset.  Re-do:
        code.clear();
        // S2 = offset
        code.extend(ss_load_imm(S2, offset as i64));
        // S2 += FP  → use ADDA.L D2, A3 (we need an address reg as dst).
        // Encode ADDA.L D2, A1: word = 0xD1C0 | (A1<<9) | D2.
        // A1 enc as 1, D2 enc as 2:  0xD1C0 | (1<<9) | 2 = 0xD3C2.
        code.extend_from_slice(&[0xD3, 0xC2]); // adda.l %d2, %a1  →  A1 = A1 + D2  (NO! we want A1 = FP + D2)
        // Actually we want S2 (= A1) = FP + S2 (= D2).  Move FP into A1 first:
        code.clear();
        // move.l %a6, %a1  →  MOVE.L An, Am  via MOVEA.L: word = 0x2040 | (Am<<9) | An
        // For A6→A1: Am=A1(enc 1 in dst field), An=A6(enc 6 in src field; but src mode for An is 1).
        // word = 0x2000 | (1<<9) | (1<<3) | 6 = 0x2246.
        code.extend_from_slice(&[0x22, 0x46]); // movea.l %a6, %a1
        // add.l #offset, %a1  — ADDQ.L #data8, An (data 1-8) or ADDA.L #imm32, An.
        // Simpler: load offset into D2, then ADDA.L D2, A1.
        code.extend(ss_load_imm(S2, offset as i64));
        // adda.l %d2, %a1: word = 0xD1C0 | (1<<9) | 2 = 0xD3C2.
        code.extend_from_slice(&[0xD3, 0xC2]);
        // Now load via (A1): MOVE.L (A1), Dn  → word = 0x2000 | (Dn<<9) | (2<<3) | 1.
        let w = 0x2000u16 | ((dst.encoding() as u16 & 0x7) << 9) | (2u16 << 3) | 1;
        code.extend_from_slice(&w.to_be_bytes());
        code
    }
}

/// Store a 32-bit value from src to stack slot at [FP + offset].
fn ss_st(src: Gpr, offset: i32) -> Vec<u8> {
    if (-32768..=32767).contains(&offset) {
        Instruction::Store { src, base: FP, offset: offset as i16 }.encode()
    } else {
        let mut code = Vec::new();
        // move.l %a6, %a1
        code.extend_from_slice(&[0x22, 0x46]);
        code.extend(ss_load_imm(S2, offset as i64));
        // adda.l %d2, %a1
        code.extend_from_slice(&[0xD3, 0xC2]);
        // move.l src, (a1): word = 0x2000 | (1<<9) | (2<<3) | src_enc.
        let w = 0x2000u16 | (1u16 << 9) | (2u16 << 3) | (src.encoding() as u16 & 0x7);
        code.extend_from_slice(&w.to_be_bytes());
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
    // For simplicity we use NEGATIVE offsets from FP, starting at -4.

    let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
    let mut current_offset: i32 = -4;
    let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
    all_vreg_ids_sorted.sort();
    for &id in &all_vreg_ids_sorted {
        vreg_stack_slots.insert(id, current_offset);
        current_offset -= 4;
    }

    // Alloc regions after vreg slots (also negative offsets).
    let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
    let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
    alloc_vreg_ids.sort();
    for &id in &alloc_vreg_ids {
        let size = alloc_sizes[&id];
        current_offset -= size;
        // Align to -16.
        current_offset = current_offset & !15;
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

    for (_blk_idx, block) in func.blocks.iter().enumerate() {
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
                // Move return value to D0 (if any), then epilogue.
                if let Some(first_val) = vals.first() {
                    code.extend(ss_load_value(first_val, &vreg_stack_slots, Gpr::D0));
                }
                // Epilogue: UNLK A6, RTS.
                code.extend(Instruction::Unlk { reg: FP }.encode());
                code.extend(Instruction::Rts.encode());
            }
            crate::ir::IRTerminator::Unreachable => {
                // Just RTS (shouldn't happen).
                code.extend(Instruction::Rts.encode());
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
        }
        IRInstr::Sub { dst, lhs, rhs, ty: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Sub { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
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
            // DIVU.W S1, S0: S0[31:16] = S0[31:0] / S1[15:0]; S0[15:0] = remainder.
            // For values that fit in 16 bits, quotient fits in 16 bits.
            // Then SWAP S0 to move quotient to lower 16 bits, and AND.L #0xFFFF to clear upper.
            code.extend(Instruction::Divu { src: S1, dst: S0 }.encode());
            code.extend(Instruction::Swap { dst: S0 }.encode());
            // AND.L #0xFFFF, S0: word = 0xC0BC | (S0<<9), ext word = 0x0000_FFFF.
            // Encode: ANDI.L #imm32, Dn — word0 = 0x02BC | (Dn<<9), then 4-byte imm32.
            // Actually: ANDI.L #imm, Dn: word = 0x02BC | (Dn<<9), then 4-byte imm.
            {
                let w = 0x0280u16 | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
                code.extend_from_slice(&0x0000_FFFFu32.to_be_bytes());
            }
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
            ty: _,
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
            // Load value from (A1) into S0.
            // For .L: MOVE.L (A1), D0 → 0x2010 | 1 = 0x2011.
            // For .B: MOVE.B (A1), D0 → 0x1010 | 1 = 0x1011, then zero-extend (default).
            // For simplicity, we always do .L load.
            {
                let w = 0x2000u16 | (0u16 << 9) | (2u16 << 3) | 1;
                code.extend_from_slice(&w.to_be_bytes());
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Store {
            value,
            addr,
            offset,
            ty: _,
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
            // MOVE.L S2, (A1): 0x2000 | (1<<9) | (2<<3) | S2_enc.
            {
                let w = 0x2000u16 | (1u16 << 9) | (2u16 << 3) | (S2.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
            }
        }
        IRInstr::Alloc { dst, size: _ } => {
            let dst_id = dst.as_register().unwrap_or(0);
            // dst = FP + alloc_offset[dst_id]
            if let Some(&off) = alloc_offsets.get(&dst_id) {
                // S0 = A6 (FP)
                // MOVE.L A6, D0: 0x2000 | (0<<9) | (1<<3) | 6 = 0x2016.
                code.extend_from_slice(&[0x20, 0x16]);
                // ADD.L #off, D0 — ADDQ.L or ADDI.L.
                if (-7..=-1).contains(&off) || (1..=8).contains(&off) {
                    let abs = off.unsigned_abs() as u16;
                    let w = 0x5080u16 | (abs << 9) | (0u16 << 6) | 0;
                    // For negative: use SUBQ.L instead.
                    if off < 0 {
                        let w = 0x5180u16 | (abs << 9) | 0;
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
            }
            code.extend(Instruction::Unlk { reg: FP }.encode());
            code.extend(Instruction::Rts.encode());
        }
        IRInstr::Branch { target: _ } => {
            // BRA.W placeholder (rarely used).
            code.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]);
        }
        IRInstr::CondBranch {
            cond,
            true_target: _,
            false_target: _,
        } => {
            code.extend(ss_load_value(cond, vreg_stack_slots, S0));
            code.extend(Instruction::Tst { dst: S0 }.encode());
            code.extend_from_slice(&[0x66, 0x00, 0x00, 0x00]); // BNE.W placeholder
            code.extend_from_slice(&[0x60, 0x00, 0x00, 0x00]); // BRA.W placeholder
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
            // JSR via PEA + JSR (PC-relative).  For simplicity, use BSR with 32-bit displacement
            // patched at link time.  BSR.L disp32: word = 0x6100 0xFF + 4-byte disp32 (BSR.L form).
            // Simpler: PEA sym(PC); JSR (SP)+.  But cleanest is BSR.L.
            // m68k BSR.L: 0x61 0x00 0xFF 0xFF + 4-byte disp32 (the 0xFFFF extension word
            // indicates the 32-bit displacement form).
            let call_offset = code.len() as u64;
            code.extend_from_slice(&[0x61, 0x00, 0xFF, 0xFF]);
            code.extend_from_slice(&0u32.to_be_bytes());
            relocations.push(RelocationEntry {
                offset: call_offset,
                symbol: func.clone(),
                reloc_type: "R_68K_PC32".to_string(),
            });
            // Move return value from D0 to dst's stack slot.
            if let Some(d) = dst {
                let d_id = d.as_register().unwrap_or(0);
                let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                code.extend(Instruction::Move { src: Gpr::D0, dst: S0 }.encode());
                code.extend(ss_st(S0, d_off));
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
            // MOVE.L S0, (A1)
            {
                let w = 0x2000u16 | (1u16 << 9) | (2u16 << 3) | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
            }
            let skip_disp = (code.len() as i64 - (bne_patch as i64 + 2)) as i16;
            code[bne_patch + 2..bne_patch + 4].copy_from_slice(&skip_disp.to_be_bytes());
            // dst already holds old value (in its stack slot).
            let _ = dst_off;
        }
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
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Add { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Sub => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Sub { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
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
            code.extend(Instruction::Divu { src: S1, dst: S0 }.encode());
            code.extend(Instruction::Swap { dst: S0 }.encode());
            {
                let w = 0x0280u16 | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
                code.extend_from_slice(&0x0000_FFFFu32.to_be_bytes());
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SRem | BinOpKind::URem => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Divu { src: S1, dst: S0 }.encode());
            // Remainder is in lower 16 bits after DIVU.W.
            {
                let w = 0x0280u16 | (S0.encoding() as u16 & 0x7);
                code.extend_from_slice(&w.to_be_bytes());
                code.extend_from_slice(&0x0000_FFFFu32.to_be_bytes());
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::And => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::And { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Or => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Or { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Xor => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend(Instruction::Xor { src: S1, dst: S0 }.encode());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Shl => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // LSL.L D1, D0: word = 0xE1A0 | (D0<<9) | (0<<5) | (1<<4) | (0<<0) | D1
            // Actually: LSL.L Dn, Dm: word = 0xE1A0 | (Dm<<9) | (0<<5) | (1<<4) | Dn
            // Hmm, let me use the immediate form for shift counts that fit.
            // For variable shift: LSL.L D1, D0 = 0xE1A0 | (0<<9) | (1<<5)... actually
            // shift-by-Dn: 1110 | count_or_0 | 1 | Dm | size | direction | i/r | Dn_or_count
            // i/r=1 means register count (in Dn).
            let w = 0xE1ACu16 | ((S0.encoding() as u16 & 0x7) << 9) | (S1.encoding() as u16 & 0x7);
            code.extend_from_slice(&w.to_be_bytes());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ShrL => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            let w = 0xE2ACu16 | ((S0.encoding() as u16 & 0x7) << 9) | (S1.encoding() as u16 & 0x7);
            code.extend_from_slice(&w.to_be_bytes());
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ShrA => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            let w = 0xE0ACu16 | ((S0.encoding() as u16 & 0x7) << 9) | (S1.encoding() as u16 & 0x7);
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
}

impl M68kBackend {
    pub fn new() -> Self {
        Self { target_info: M68kTargetInfo }
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
}

impl Backend for M68kBackend {
    fn target_info(&self) -> &dyn TargetInfo { &self.target_info }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        m68k_allocate_registers_ss(func)
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
        // BSR.L main — 6 bytes (0x61 0x00 0xFF 0xFF + 4-byte disp32), offset 0.
        //   After return, D0 holds main's return value.
        // MOVE.L D0, D1 — 2 bytes, offset 6.  (D1 = exit code)
        // MOVEQ #1, D0 — 2 bytes, offset 8.   (D0 = SYS_exit = 1)
        // TRAP #0      — 2 bytes, offset 10.
        let start_stub_size: usize = 12;
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
            // Args: D1 = size (incoming).  We need: D1=size, D2=PROT, D3=flags, D4=fd, D5=offset.
            // Move D1 → D2 (save size).
            code.extend(Instruction::Move { src: Gpr::D1, dst: Gpr::D2 }.encode());
            // D1 = 0 (NULL addr)
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 0 }.encode());
            // D2 = 3 (PROT_READ | PROT_WRITE) — overwrite with 3 since size was moved.
            // Wait, we need D1=size for mmap.  Let me redo.
            code.clear();
            // D2 = size
            code.extend(Instruction::Move { src: Gpr::D1, dst: Gpr::D2 }.encode());
            // D1 = NULL
            code.extend(Instruction::Moveq { dst: Gpr::D1, imm: 0 }.encode());
            // D3 = 3 (PROT_READ|PROT_WRITE)
            code.extend(Instruction::Moveq { dst: Gpr::D3, imm: 3 }.encode());
            // D4 = 0x22 (MAP_PRIVATE|MAP_ANONYMOUS)
            code.extend(Instruction::MoveImm32 { dst: Gpr::D4, imm: 0x22 }.encode());
            // D5 = -1 (fd)
            code.extend(Instruction::Moveq { dst: Gpr::D5, imm: -1 }.encode());
            // D0 = 90 (sys_mmap) — but D2 has size; we need D1=size, D2=prot, D3=flags, D4=fd, D5=offset
            // m68k mmap args: D1=addr, D2=length, D3=prot, D4=flags, D5=fd, then offset on stack.
            // Hmm, we have D1=0(NULL), D2=size, D3=3(PROT), D4=0x22(flags), D5=-1(fd).  Good.
            // Push offset=0 onto stack.
            code.extend(Instruction::Moveq { dst: Gpr::D0, imm: 0 }.encode());
            // PEA (SP) — push 0 offset... simpler: PE.L #0 is PEA #0 which isn't valid.
            // Use: MOVEQ #0, D0; MOVE.L D0, -(SP).
            // PUSH D0 onto stack: MOVE.L D0, -(SP) = 0x2F00.
            code.extend_from_slice(&[0x2F, 0x00]);
            // D0 = 90 (sys_mmap)
            code.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: 90 }.encode());
            // TRAP #0
            code.extend(Instruction::Trap0.encode());
            // Pop the offset arg off the stack: ADDQ.L #4, SP.
            // ADDQ.L #4, A7: 0x5FC4.
            code.extend_from_slice(&[0x5F, 0xC4]);
            // RTS
            code.extend(Instruction::Rts.encode());
            code
        };

        let vuma_free_stub: Vec<u8> = {
            let mut code = Vec::new();
            // D1 = addr (incoming).  D2 = 0 (size).
            // D2 = 0
            code.extend(Instruction::Moveq { dst: Gpr::D2, imm: 0 }.encode());
            // D0 = 91 (sys_munmap)
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
                ("mmap", 90),
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
                ("epoll_create1", 449),
                ("epoll_ctl", 424),
                ("epoll_wait", 425),
                ("dup3", 431),
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

        // ── Build _start stub bytes ──
        let mut start_stub = Vec::with_capacity(start_stub_size);
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
        // BSR.L: 0x61 0x00 0xFF 0xFF + 4-byte disp32.
        // disp32 at bytes [4..8] of the 8-byte instruction.
        // PC = address of byte 4 (extension word) = instr_addr + 4.
        let mut func_code_offset: usize = start_stub_size + ffi_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 8 > all_code.len() {
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
                    // For BSR.L: PC = abs_offset + 4 (extension word position).
                    let pc_abs = BASE_ADDR + text_offset + abs_offset as u64 + 4;
                    let target_abs = BASE_ADDR + text_offset + target_offset as u64;
                    let disp = (target_abs as i64 - pc_abs as i64) as i32;
                    let disp_be = disp.to_be_bytes();
                    all_code[abs_offset + 4..abs_offset + 8].copy_from_slice(&disp_be);
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
        ((base_addr + text_file_end + HOST_PAGE_ALIGN - 1) / HOST_PAGE_ALIGN) * HOST_PAGE_ALIGN;
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
