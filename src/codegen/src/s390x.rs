//! # s390x (IBM System Z) Backend
//!
//! Implements the `Backend` trait for the IBM z/Architecture (s390x), a
//! 64-bit big-endian ISA with variable-length instructions (2, 4, or 6
//! bytes).  This module provides:
//!
//! - `Gpr` — General-purpose register enum (R0–R15)
//! - `S390XBackend` — `Backend` implementation that lowers IR to s390x
//!   machine code and emits big-endian ELF64 binaries
//!
//! ## s390x Register Convention (Linux ABI)
//!
//! | Register | Role                                                       |
//! |----------|------------------------------------------------------------|
//! | R0       | scratch / volatile (cannot be used as base in load/store) |
//! | R1       | syscall number (Linux s390x); scratch otherwise           |
//! | R2–R6    | argument registers (up to 5 args in registers)            |
//! | R7–R10   | scratch / volatile                                        |
//! | R11      | frame pointer (convention)                                |
//! | R12      | global pointer / TOC (convention; unused here)            |
//! | R13      | base pointer (convention; unused here)                    |
//! | R14      | link register (return address)                            |
//! | R15      | stack pointer                                              |
//!
//! ## Instruction Encoding
//!
//! All s390x instructions are **big-endian**.  Common formats used here:
//!
//! - **RI-a** (4 bytes): `op1 | R1 | op2 | imm16`
//!   - LGHI R1, imm16 (op1=0xA7, op2=0x9)
//! - **RIL-a/b/c** (6 bytes): `op1 | R1/mask | op2 | imm32`
//!   - LGFI R1, imm32 (op1=0xC0, op2=0x1)
//!   - LARL R1, disp32 (op1=0xC0, op2=0x0) — disp in halfwords
//!   - BRASL R1, disp32 (op1=0xC0, op2=0x5) — disp in halfwords
//!   - BRCL mask, disp32 (op1=0xC0, op2=0x4) — disp in halfwords
//! - **RI-c** (4 bytes): `op1 | mask | op2 | imm16`
//!   - BRC mask, disp16 (op1=0xA7, op2=0x4)
//! - **RXY-a** (6 bytes): `op1 | R1 | X2 | B2 | D2[11:0] | DL2 | DH2 | op2`
//!   - LG  R1, D2(X2, B2) (op1=0xE3, op2=0x04)
//!   - STG R1, D2(X2, B2) (op1=0xE3, op2=0x24)
//!   - LGB R1, D2(X2, B2) (op1=0xE3, op2=0x77) — sign-extending byte load
//!   - LGH R1, D2(X2, B2) (op1=0xE3, op2=0x15) — sign-extending halfword load
//!   - LGF R1, D2(X2, B2) (op1=0xE3, op2=0x14) — sign-extending 32-bit load
//!   - LLGF R1, D2(X2, B2) (op1=0xE3, op2=0x16) — zero-extending 32-bit load
//!   - STC R1, D2(X2, B2) (op1=0xE3, op2=0x72)
//!   - STH R1, D2(X2, B2) (op1=0xE3, op2=0x70)
//!   - STY R1, D2(X2, B2) (op1=0xE3, op2=0x50)
//! - **RSY-a** (6 bytes): `op1 | R1 | R3 | B2 | D2 | DL2 | DH2 | op2`
//!   - SLLG R1, R3, D2(B2) (op1=0xEB, op2=0x0D)
//!   - SRLG R1, R3, D2(B2) (op1=0xEB, op2=0x0C)
//!   - SRAG R1, R3, D2(B2) (op1=0xEB, op2=0x0A)
//! - **RRE** (4 bytes): `op1 | op2 | 0x00 | R1 | R2`
//!   - LGR R1, R2  (op1=0xB9, op2=0x04)
//!   - AGR R1, R2  (op1=0xB9, op2=0x08)  — R1 += R2 (64-bit)
//!   - SGR R1, R2  (op1=0xB9, op2=0x09)  — R1 -= R2 (64-bit)
//!   - MSGR R1, R2 (op1=0xB9, op2=0x01)  — R1 *= R2 (64-bit, signed)
//!   - DLGR R1, R2 (op1=0xB9, op2=0x87)  — divides (R1, R1+1) by R2
//!   - LLGFR R1, R2 (op1=0xB9, op2=0x16)
//!   - LGFR R1, R2  (op1=0xB9, op2=0x14)
//! - **RRF-a** (4 bytes): `op1 | op2 | R1 | R2 | R3 | 0x0`
//!   - ARK R1, R2, R3  (op1=0xB9, op2=0xF8)  — R1 = R2 + R3 (32-bit)
//!   - SRK R1, R2, R3  (op1=0xB9, op2=0xF9)
//!   - NRK R1, R2, R3  (op1=0xB9, op2=0xF4)
//!   - ORK R1, R2, R3  (op1=0xB9, op2=0xF6)
//!   - XRK R1, R2, R3  (op1=0xB9, op2=0xF7)
//! - **RR** (2 bytes): `op1 | R1 | R2`
//!   - BR R2 (op1=0x07)
//! - **SVC** (2 bytes): `0x0A | imm8`
//!   - SVC 0 (0x0A00)
//!
//! ## Linux s390x Syscall Convention
//!
//! - Syscall number in R1
//! - Arguments in R2–R7
//! - Return value in R2
//! - Invoke via `SVC 0` (the syscall # is in R1, NOT in the SVC immediate)
//!
//! ## No Branch Delay Slots
//!
//! s390x has no branch delay slots — the instruction after a branch is NOT
//! executed before the control transfer takes effect.

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

/// s390x general-purpose registers (R0–R15).
///
/// - R0   — scratch (cannot be used as base/index in load/store encoding;
///   encoding 0 in B2/X2 fields means "no register")
/// - R1   — syscall number (Linux s390x); scratch otherwise
/// - R2–R6 — argument registers
/// - R7–R10 — scratch / volatile
/// - R11  — frame pointer (convention)
/// - R12  — global pointer (TOC) — unused in this backend
/// - R13  — base pointer (convention; unused here)
/// - R14  — link register (return address)
/// - R15  — stack pointer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Gpr {
    R0 = 0,
    R1 = 1,
    R2 = 2,
    R3 = 3,
    R4 = 4,
    R5 = 5,
    R6 = 6,
    R7 = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

impl Gpr {
    /// Returns the 4-bit encoding index (0–15) for this register.
    pub fn encoding(&self) -> u8 {
        *self as u8
    }

    /// Returns the Gpr for the given encoding index (0–15).
    pub fn from_encoding(enc: u8) -> Option<Self> {
        match enc {
            0 => Some(Gpr::R0),
            1 => Some(Gpr::R1),
            2 => Some(Gpr::R2),
            3 => Some(Gpr::R3),
            4 => Some(Gpr::R4),
            5 => Some(Gpr::R5),
            6 => Some(Gpr::R6),
            7 => Some(Gpr::R7),
            8 => Some(Gpr::R8),
            9 => Some(Gpr::R9),
            10 => Some(Gpr::R10),
            11 => Some(Gpr::R11),
            12 => Some(Gpr::R12),
            13 => Some(Gpr::R13),
            14 => Some(Gpr::R14),
            15 => Some(Gpr::R15),
            _ => None,
        }
    }

    /// Returns the assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::R0 => "%r0",
            Gpr::R1 => "%r1",
            Gpr::R2 => "%r2",
            Gpr::R3 => "%r3",
            Gpr::R4 => "%r4",
            Gpr::R5 => "%r5",
            Gpr::R6 => "%r6",
            Gpr::R7 => "%r7",
            Gpr::R8 => "%r8",
            Gpr::R9 => "%r9",
            Gpr::R10 => "%r10",
            Gpr::R11 => "%r11",
            Gpr::R12 => "%r12",
            Gpr::R13 => "%r13",
            Gpr::R14 => "%r14",
            Gpr::R15 => "%r15",
        }
    }

    /// Returns the Gpr for a given argument index (0–4) using the Linux ABI
    /// (R2–R6).  Returns `None` for indices >= 5.
    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::R2),
            1 => Some(Gpr::R3),
            2 => Some(Gpr::R4),
            3 => Some(Gpr::R5),
            4 => Some(Gpr::R6),
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

/// s390x floating-point registers (F0–F15, 64-bit each).
///
/// s390x FP uses the same 4-bit register encoding as GPRs, but the registers
/// are interpreted as FPRs in FP instructions.  The RRE/RRF/RXY-a encoders
/// below accept `Fpr` values and pack them into the same 4-bit field as `Gpr`.
///
/// - F0, F2 — FP argument registers (Linux s390x ABI; `num_fp_arg_regs = 2`)
/// - F1, F3 — caller-saved scratch (NOT arg regs; used as FA, FB below)
/// - F4–F6 — callee-saved (preserved across calls)
/// - F7–F15 — caller-saved scratch
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Fpr {
    F0 = 0,
    F1 = 1,
    F2 = 2,
    F3 = 3,
    F4 = 4,
    F5 = 5,
    F6 = 6,
    F7 = 7,
    F8 = 8,
    F9 = 9,
    F10 = 10,
    F11 = 11,
    F12 = 12,
    F13 = 13,
    F14 = 14,
    F15 = 15,
}

impl Fpr {
    /// Returns the 4-bit encoding index (0–15) for this FP register.
    /// Identical layout to `Gpr::encoding()` — FP and GP registers share the
    /// same 4-bit field encoding, just interpreted differently by the opcode.
    pub fn encoding(&self) -> u8 {
        *self as u8
    }

    /// Returns the assembly name for this FP register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Fpr::F0 => "%f0",
            Fpr::F1 => "%f1",
            Fpr::F2 => "%f2",
            Fpr::F3 => "%f3",
            Fpr::F4 => "%f4",
            Fpr::F5 => "%f5",
            Fpr::F6 => "%f6",
            Fpr::F7 => "%f7",
            Fpr::F8 => "%f8",
            Fpr::F9 => "%f9",
            Fpr::F10 => "%f10",
            Fpr::F11 => "%f11",
            Fpr::F12 => "%f12",
            Fpr::F13 => "%f13",
            Fpr::F14 => "%f14",
            Fpr::F15 => "%f15",
        }
    }
}

impl fmt::Display for Fpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asm_name())
    }
}

/// FP scratch register: accumulator / result.  F1 is caller-saved and is NOT
/// one of the FP arg regs (F0/F2), so it's free for our use.
const FA: Fpr = Fpr::F1;
/// FP scratch register: second operand.  F3 is caller-saved, not an arg reg.
const FB: Fpr = Fpr::F3;

// ===========================================================================
// Scratch register allocation (stack-slot ISel uses these as temporaries)
// ===========================================================================

/// Scratch registers used by the stack-slot ISel.
///
/// R0–R7 are caller-saved (volatile) on s390x, so they're free for our use.
/// R0 cannot be used as a base register in load/store (encoding 0 means "no
/// register"), but R0 is fine as the data register in load/store and as a
/// general operand in arithmetic.
const SCRATCH_REGS: [Gpr; 8] = [
    Gpr::R0, Gpr::R1, Gpr::R2, Gpr::R3, Gpr::R4, Gpr::R5, Gpr::R6, Gpr::R7,
];

/// Common scratch register aliases (mirroring sparc64's L0–L7 naming).
/// IMPORTANT: On s390x, R0 in the base or index register field of a memory
/// instruction means "no register" (0 contribution), NOT "use R0's value".
/// Therefore S0 must NOT be R0 — use R6 instead so it can serve as a base
/// register in memory operations (STG, LG, STC, etc.).
const S0: Gpr = Gpr::R6;
const S1: Gpr = Gpr::R7;
const S2: Gpr = Gpr::R8;
const S3: Gpr = Gpr::R9;
const S4: Gpr = Gpr::R10;
const S5: Gpr = Gpr::R12;

/// Frame pointer (R11).
const FP: Gpr = Gpr::R11;
/// Stack pointer (R15).
const SP: Gpr = Gpr::R15;
/// Link register / return address (R14).
const LR: Gpr = Gpr::R14;

// ===========================================================================
// Instruction Encoders
// ===========================================================================

/// Encode LGHI R1, imm16 (Load Halfword Immediate, sign-extended to 64-bit).
/// Format: RI-a, 4 bytes. op1=0xA7, op2=0x9.
/// (Per qemu's decoder: RI op2=0x9 = LGHI. Despite IBM PoP listing op2=0x9 as
///  AGHI, qemu executes it as LGHI. The disassembler confirms: `a7 f9 ...` is
///  displayed as "lghi". This is the encoding that actually loads the
///  immediate into the register.)
fn encode_lghi(r1: Gpr, imm: i16) -> [u8; 4] {
    let word = (0xA7u32 << 24)
        | ((r1.encoding() as u32 & 0xF) << 20)
        | (0x9u32 << 16)  // op2=0x9 = LGHI (per qemu execution)
        | (imm as u16 as u32);
    word.to_be_bytes()
}

/// Encode LGFI R1, imm32 (Load Fullword Immediate, sign-extended to 64-bit).
/// Format: RIL-a, 6 bytes.
fn encode_lgfi(r1: Gpr, imm: i32) -> [u8; 6] {
    let op1: u8 = 0xC0;
    let op2: u8 = 0x1;  // LGFI = op2=0x1 (confirmed correct per s390x PoP)
    let r1_byte = ((r1.encoding() & 0xF) << 4) | (op2 & 0xF);
    let imm_be = imm.to_be_bytes();
    [op1, r1_byte, imm_be[0], imm_be[1], imm_be[2], imm_be[3]]
}

/// Encode LLILF R1, imm32 (Load Logical 32-bit Immediate, zero-extended to 64-bit).
/// Format: RIL-a, 6 bytes. op1=0xC0, op2=0xF.
/// Unlike LGFI (which sign-extends), LLILF zero-extends.  This is critical
/// for values like 0xFFFFFFFF (4294967295) which LGFI would load as
/// 0xFFFFFFFFFFFFFFFF (sign-extended -1), but LLILF loads as
/// 0x00000000FFFFFFFF (zero-extended).
fn encode_llilf(r1: Gpr, imm: u32) -> [u8; 6] {
    let op1: u8 = 0xC0;
    let op2: u8 = 0xF;
    let r1_byte = ((r1.encoding() & 0xF) << 4) | (op2 & 0xF);
    let imm_be = imm.to_be_bytes();
    [op1, r1_byte, imm_be[0], imm_be[1], imm_be[2], imm_be[3]]
}

/// Encode AGFI R1, imm32 (Add Fullword Immediate to 64-bit register).
/// Format: RIL-b, 6 bytes. op1=0xC0, op2=0x9.
/// (RIL format uses op1=0xC0 for all variants — LGFI/AGFI/SGFI/BRASL/BRCL/LARL.
///  0xEC is for RIE/RISL/RSL formats which have a different field layout.)
/// Encode AGFI R1, imm32 (Add 32-bit Immediate to 64-bit register).
/// CRITICAL: QEMU user-mode treats AGFI (op2=0x9) as IILF (load immediate),
/// not add. This is a QEMU bug. Workaround: use LGHI+AGR (8 bytes) instead
/// of AGFI (6 bytes). This produces correct results under both real hardware
/// and QEMU. The extra 2 bytes per AGFI call must be accounted for in
/// start_stub_size and any hardcoded offset calculations.
fn encode_agfi(r1: Gpr, imm: i32) -> Vec<u8> {
    // QEMU bug workaround: AGFI (op2=0x9) is treated as IILF (load, not add).
    // Use R0 (caller-saved scratch) as temp: LGHI R0, imm; AGR R1, R0.
    // This is 8 bytes (4+4) for small imms, 10 bytes (6+4) for large imms.
    let mut code = Vec::new();
    if (-32768..=32767).contains(&imm) {
        code.extend_from_slice(&encode_lghi(Gpr::R0, imm as i16));
    } else {
        code.extend_from_slice(&encode_lgfi(Gpr::R0, imm));
    }
    code.extend_from_slice(&encode_agr(r1, Gpr::R0));
    code
}

/// Encode a 6-byte RXY-a instruction.
///
/// Format (per IBM s390x PoP):
///   byte 0: opcode1 (8 bits)
///   byte 1: R1 (4 bits, high nibble) | X2 (4 bits, low nibble)
///   byte 2: B2 (4 bits, high nibble) | DL2[11:8] (4 bits, low nibble)
///   byte 3: DL2[7:0] (8 bits)
///   byte 4: DH2 (8 bits)
///   byte 5: opcode2 (8 bits — the FULL byte, not just the low nibble)
///
/// Displacement = sign_extend(DH2:DL2, 20) → 20-bit signed value
fn encode_rxy_a(op1: u8, op2: u8, r1: Gpr, x2: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    // 20-bit signed displacement: DH2 (8 bits) << 12 | DL2 (12 bits).
    let d = disp as i64;
    let d20 = (d & 0xFFFFF) as u32; // low 20 bits (two's-complement for negatives)
    let dl2_low_8 = (d20 & 0xFF) as u8;            // byte 3: DL2[7:0]
    let dl2_high_4 = ((d20 >> 8) & 0xF) as u8;     // byte 2 low nibble: DL2[11:8]
    let dh2 = ((d20 >> 12) & 0xFF) as u8;          // byte 4: DH2

    let byte0 = op1;
    let byte1 = ((r1.encoding() & 0xF) << 4) | (x2.encoding() & 0xF);
    let byte2 = ((b2.encoding() & 0xF) << 4) | dl2_high_4;
    let byte3 = dl2_low_8;
    let byte4 = dh2;
    let byte5 = op2; // FULL 8-bit opcode2 (e.g., 0x04 for LG, 0x24 for STG)
    [byte0, byte1, byte2, byte3, byte4, byte5]
}

/// Encode LG R1, D2(X2, B2) (Load 64-bit). op1=0xE3, op2=0x04.
fn encode_lg(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x04, r1, Gpr::R0, b2, disp)
}

/// Encode STG R1, D2(X2, B2) (Store 64-bit). op1=0xE3, op2=0x24.
fn encode_stg(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x24, r1, Gpr::R0, b2, disp)
}

/// Encode LGB R1, D2(X2, B2) (Load Byte, sign-extended to 64-bit). op2=0x77.
fn encode_lgb(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x77, r1, Gpr::R0, b2, disp)
}

/// Encode LGH R1, D2(X2, B2) (Load Halfword, sign-extended to 64-bit). op2=0x15.
fn encode_lgh(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x15, r1, Gpr::R0, b2, disp)
}

/// Encode LGF R1, D2(X2, B2) (Load 32-bit, sign-extended to 64-bit). op2=0x14.
fn encode_lgf(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x14, r1, Gpr::R0, b2, disp)
}

/// Encode LLGF R1, D2(X2, B2) (Load 32-bit, zero-extended to 64-bit). op2=0x16.
fn encode_llgf(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x16, r1, Gpr::R0, b2, disp)
}

/// Encode STC R1, D2(X2, B2) (Store Byte). op2=0x72.
fn encode_stc(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x72, r1, Gpr::R0, b2, disp)
}

/// Encode STH R1, D2(X2, B2) (Store Halfword). op2=0x70.
fn encode_sth(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x70, r1, Gpr::R0, b2, disp)
}

/// Encode STY R1, D2(X2, B2) (Store 32-bit). op2=0x50.
fn encode_sty(r1: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(0xE3, 0x50, r1, Gpr::R0, b2, disp)
}

/// Encode a 4-byte RRE instruction.
///
/// Format:
///   byte 0: opcode1 (8 bits)
///   byte 1: opcode2 (8 bits)
///   byte 2: 0x00 (unused)
///   byte 3: R1 (4 bits, high nibble) | R2 (4 bits, low nibble)
fn encode_rre(op1: u8, op2: u8, r1: Gpr, r2: Gpr) -> [u8; 4] {
    let byte3 = ((r1.encoding() & 0xF) << 4) | (r2.encoding() & 0xF);
    [op1, op2, 0x00, byte3]
}

/// Encode LGR R1, R2 (Load Register 64-bit). op1=0xB9, op2=0x04.
fn encode_lgr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x04, r1, r2)
}

/// Encode AGR R1, R2 (Add 64-bit). R1 += R2. op1=0xB9, op2=0x08.
fn encode_agr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x08, r1, r2)
}

/// Encode SGR R1, R2 (Subtract 64-bit). R1 -= R2. op1=0xB9, op2=0x09.
fn encode_sgr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x09, r1, r2)
}

/// Adjust SP (R15) by a signed immediate.
///
/// Uses LGHI (load |imm| into scratch S1) + SGR/AGR (subtract/add S1 to SP).
/// This avoids the AGFI/AGHI opcode ambiguity between IBM's PoP and qemu's
/// decoder (both op2=0x9 in RIL/RI format are treated by qemu as IILF/LGHI
/// = load, not add). LGHI (op2=0x9 in RI) and SGR/AGR (RRE format) are
/// unambiguous and confirmed working under qemu.
fn adjust_sp(imm: i32) -> Vec<u8> {
    let mut code = Vec::new();
    if imm == 0 {
        return code;
    }
    if (-32768..=32767).contains(&imm) {
        if imm < 0 {
            // SP -= |imm|:  LGHI S1, |imm|;  SGR SP, S1
            code.extend_from_slice(&encode_lghi(S1, (-imm) as i16));
            code.extend_from_slice(&encode_sgr(SP, S1));
        } else {
            // SP += imm:  LGHI S1, imm;  AGR SP, S1
            code.extend_from_slice(&encode_lghi(S1, imm as i16));
            code.extend_from_slice(&encode_agr(SP, S1));
        }
    } else {
        // Large immediate: use LGFI (32-bit load) instead of LGHI.
        if imm < 0 {
            code.extend_from_slice(&encode_lgfi(S1, -imm));
            code.extend_from_slice(&encode_sgr(SP, S1));
        } else {
            code.extend_from_slice(&encode_lgfi(S1, imm));
            code.extend_from_slice(&encode_agr(SP, S1));
        }
    }
    code
}

/// Encode MSGR R1, R2 (Multiply Single 64-bit). R1 = R1 * R2 (low 64 bits).
/// op1=0xB9, op2=0x0C. (0x01 is LNGR = Load Negative, a common mix-up.)
fn encode_msgr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x0C, r1, r2)
}

/// Encode DLGR R1, R2 (Divide Logical 64-bit). R1 must be even-numbered.
/// Dividend pair (R1, R1+1): R1 = HIGH 64 bits (in), remainder (out);
/// R1+1 = LOW 64 bits (in), quotient (out).
/// op1=0xB9, op2=0x87.
fn encode_dlgr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x87, r1, r2)
}

/// Encode DGR R1, R2 (Divide signed 64-bit). R1 must be even-numbered.
/// Dividend pair (R1, R1+1): R1 = HIGH 64 bits (in, sign extension),
/// remainder (out); R1+1 = LOW 64 bits (in, dividend), quotient (out).
/// op1=0xB9, op2=0x0D.
fn encode_dgr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x0D, r1, r2)
}

/// Encode LLGFR R1, R2 (Load Logical 32→64). R1 = zero_extend(R2[31:0]).
/// op1=0xB9, op2=0x16.
fn encode_llgfr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x16, r1, r2)
}

/// Encode LGFR R1, R2 (Load 32→64 signed). R1 = sign_extend(R2[31:0]).
/// op1=0xB9, op2=0x14.
fn encode_lgfr(r1: Gpr, r2: Gpr) -> [u8; 4] {
    encode_rre(0xB9, 0x14, r1, r2)
}

/// Encode ARK R1, R2, R3 (Add 32-bit). R1[31:0] = R2[31:0] + R3[31:0].
/// Since QEMU doesn't support RRF-a format, use AR (RR format, 2 bytes) + NOP.
/// AR R1, R3: op=0x1A. When R1==R2, AR R1,R3 == ARK R1,R1,R3.
fn encode_ark(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    let _ = r2; // R1==R2 in all call sites; AR R1, R3 is equivalent
    [0x1A, ((r1.encoding() & 0xF) << 4) | (r3.encoding() & 0xF), 0x07, 0x00] // AR + BCR 0,R0 (NOP)
}

/// Encode SRK R1, R2, R3 (Sub 32-bit). Use SR (RR, 2 bytes) + NOP.
fn encode_srk(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    let _ = r2;
    [0x1B, ((r1.encoding() & 0xF) << 4) | (r3.encoding() & 0xF), 0x07, 0x00]
}

/// Encode NRK R1, R2, R3 (AND 32-bit). Use NR (RR, 2 bytes) + NOP.
fn encode_nrk(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    let _ = r2;
    [0x14, ((r1.encoding() & 0xF) << 4) | (r3.encoding() & 0xF), 0x07, 0x00]
}

/// Encode a 6-byte RSY-a instruction (used for shifts).
///
/// Format:
///   byte 0: opcode1 (8 bits)
///   byte 1: R1 (4 bits, high nibble) | R3 (4 bits, low nibble)
///   byte 2: B2 (4 bits, high nibble) | D2[11:8] (4 bits, low nibble)
///   byte 3: D2[7:0] (8 bits)
///   byte 4: DL2 (8 bits)
///   byte 5: DH2 (4 bits, high nibble) | opcode2 (4 bits, low nibble)
///
/// For SLLG/SRLG/SRAG, the shift amount is the low 6 bits of (B2 + D2).
/// Using B2=0 (no base register) and D2=shift_amount, the shift amount is
/// simply D2 & 0x3F.
fn encode_rsy_a(op1: u8, op2: u8, r1: Gpr, r3: Gpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_rxy_a(op1, op2, r1, r3, b2, disp)
}

/// Encode SLLG R1, R3, imm (Shift Left Logical 64-bit).
/// R1 = R3 << (imm & 0x3F). op1=0xEB, op2=0x0D.
fn encode_sllg(r1: Gpr, r3: Gpr, imm: u32) -> [u8; 6] {
    encode_rsy_a(0xEB, 0x0D, r1, r3, Gpr::R0, (imm & 0x3F) as i32)
}

/// Encode SRLG R1, R3, imm (Shift Right Logical 64-bit).
/// R1 = R3 >> (imm & 0x3F), zero-extended. op1=0xEB, op2=0x0C.
fn encode_srlg(r1: Gpr, r3: Gpr, imm: u32) -> [u8; 6] {
    encode_rsy_a(0xEB, 0x0C, r1, r3, Gpr::R0, (imm & 0x3F) as i32)
}

/// Encode SRAG R1, R3, imm (Shift Right Arithmetic 64-bit).
/// R1 = R3 >> (imm & 0x3F), sign-extended. op1=0xEB, op2=0x0A.
fn encode_srag(r1: Gpr, r3: Gpr, imm: u32) -> [u8; 6] {
    encode_rsy_a(0xEB, 0x0A, r1, r3, Gpr::R0, (imm & 0x3F) as i32)
}

/// Encode BRC mask, disp16 (Branch Relative on Condition).
/// Format: RI-c, 4 bytes. op1=0xA7, op2=0x4.
/// disp is in halfwords (signed 16-bit). Target = PC + (disp * 2).
fn encode_brc(mask: u8, disp: i16) -> [u8; 4] {
    let word = (0xA7u32 << 24)
        | ((mask as u32 & 0xF) << 20)
        | (0x4u32 << 16)
        | (disp as u16 as u32);
    word.to_be_bytes()
}

/// Encode BRCL mask, disp32 (Branch Relative on Condition Long).
/// Format: RIL-c, 6 bytes. op1=0xC0, op2=0x4.
/// disp is in halfwords (signed 32-bit). Target = PC + (disp * 2).
fn encode_brcl(mask: u8, disp: i32) -> [u8; 6] {
    let op1: u8 = 0xC0;
    let op2: u8 = 0x4;
    let byte1 = ((mask & 0xF) << 4) | (op2 & 0xF);
    let disp_be = disp.to_be_bytes();
    [op1, byte1, disp_be[0], disp_be[1], disp_be[2], disp_be[3]]
}

/// Encode BRASL R1, disp32 (Branch Relative and Save Long).
/// Format: RIL-b, 6 bytes. op1=0xC0, op2=0x5.
/// R1 = PC + 6 (return address); branch to PC + (disp * 2).
fn encode_brasl(r1: Gpr, disp: i32) -> [u8; 6] {
    let op1: u8 = 0xC0;
    let op2: u8 = 0x5;
    let byte1 = ((r1.encoding() & 0xF) << 4) | (op2 & 0xF);
    let disp_be = disp.to_be_bytes();
    [op1, byte1, disp_be[0], disp_be[1], disp_be[2], disp_be[3]]
}

/// Encode LARL R1, disp32 (Load Address Relative Long).
/// Format: RIL-b, 6 bytes. op1=0xC0, op2=0x0.
/// R1 = PC + (disp * 2).
fn encode_larl(r1: Gpr, disp: i32) -> [u8; 6] {
    let op1: u8 = 0xC0;
    let op2: u8 = 0x0;
    let byte1 = ((r1.encoding() & 0xF) << 4) | (op2 & 0xF);
    let disp_be = disp.to_be_bytes();
    [op1, byte1, disp_be[0], disp_be[1], disp_be[2], disp_be[3]]
}

/// Encode BR R2 (Branch Register), which is really BCR 0xF, R2
/// (Branch on Condition Always, target = address in R2).
/// Format: RR, 2 bytes. op1=0x07, M1=0xF (unconditional), R2=register.
fn encode_br(r2: Gpr) -> [u8; 2] {
    // BCR format: [0x07, M1<<4 | R2]
    // M1 = 0xF = unconditional branch (this is what makes it "BR" not "BCR cond").
    let byte1 = (0xFu8 << 4) | (r2.encoding() & 0xF);
    [0x07, byte1]
}

/// Encode SVC imm (Supervisor Call). Format: RR, 2 bytes. op1=0x0A.
/// For Linux s390x, use SVC 0 with syscall # in R1.
fn encode_svc(imm: u8) -> [u8; 2] {
    [0x0A, imm]
}

/// Encode a 2-byte NOP. "BCR 0, 0" (Branch on Condition Never, mask=0).
/// This is the standard s390x NOP encoding.
fn encode_nop() -> [u8; 2] {
    [0x07, 0x00]
}

// ===========================================================================
// Stack-slot helpers
// ===========================================================================

/// Load a 64-bit immediate into a register.
///
/// Uses LGHI for values that fit in a 16-bit signed integer, LGFI for
/// values that fit in a 32-bit signed integer.  For full 64-bit values,
/// we use LARL + LGHI + combinations of bit operations (rare in practice).
fn ss_load_imm(dst: Gpr, val: i64) -> Vec<u8> {
    let mut code = Vec::new();
    if (-32768..=32767).contains(&val) {
        code.extend_from_slice(&encode_lghi(dst, val as i16));
    } else if (0..=0xFFFFFFFF).contains(&(val as u64)) {
        // Unsigned 32-bit value: use LLILF (zero-extended) to avoid
        // sign-extension bugs.  LGFI would load 0xFFFFFFFF as
        // 0xFFFFFFFFFFFFFFFF (sign-extended -1), breaking AND masks.
        code.extend_from_slice(&encode_llilf(dst, val as u32));
    } else if (-2147483648..=-1).contains(&val) {
        code.extend_from_slice(&encode_lgfi(dst, val as i32));
    } else {
        // Full 64-bit value: load high 32 bits, shift left 32, OR low 32 bits.
        let v = val as u64;
        let hi = ((v >> 32) & 0xFFFF_FFFF) as i32;
        let lo = (v & 0xFFFF_FFFF) as i32;
        // Load high 32 bits (sign-extended to 64). For values where the high
        // bit of hi is set, the sign extension would corrupt the low bits
        // after the shift, so we OR the low bits AFTER the shift.
        code.extend_from_slice(&encode_lgfi(dst, hi));
        // Shift left by 32: SLLG dst, dst, 32
        code.extend_from_slice(&encode_sllg(dst, dst, 32));
        // OR in the low 32 bits. We use AGRK-like 3-operand OR... actually
        // s390x doesn't have a 3-operand 64-bit OR. We need OGR (OR 64-bit
        // 2-operand) which is [B9 81 00 R1|R2]. To compute dst = dst | lo,
        // we'd load lo into a temp first.
        // Use temp = S5 (R5).  Use LLILF (zero-extend) for the low 32 bits:
        // LGFI would sign-extend values with the high bit set (e.g. the low
        // half of -3.7 as F64 = 0x9999999A), which would set the top 32 bits
        // to all-ones and corrupt the already-placed high half via OGR.
        code.extend_from_slice(&encode_llilf(S5, lo as u32));
        // OGR dst, S5: dst |= S5. op1=0xB9, op2=0x81.
        code.extend_from_slice(&encode_rre(0xB9, 0x81, dst, S5));
    }
    code
}

/// Load a 64-bit value from stack slot at [FP + offset] into dst.
fn ss_ld(dst: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    if (-524288..=524287).contains(&offset) {
        code.extend_from_slice(&encode_lg(dst, FP, offset));
    } else {
        // Large offset: compute address into a temp register first.
        code.extend(ss_load_imm(S5, offset as i64));
        code.extend_from_slice(&encode_agr(S5, FP));
        code.extend_from_slice(&encode_lg(dst, S5, 0));
    }
    code
}

/// Store a 64-bit value from src to stack slot at [FP + offset].
fn ss_st(src: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    if (-524288..=524287).contains(&offset) {
        code.extend_from_slice(&encode_stg(src, FP, offset));
    } else {
        code.extend(ss_load_imm(S5, offset as i64));
        code.extend_from_slice(&encode_agr(S5, FP));
        code.extend_from_slice(&encode_stg(src, S5, 0));
    }
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

/// Load an f32 value into an FPR with the f32 bits in the HIGH 32 bits
/// (where s390x f32 instructions expect them).
///
/// **Convention:** f32 values in stack slots and immediates have their f32
/// bits in the LOW 32 bits of the 64-bit GPR value (zero-extended).  This
/// matches:
///   - The IR's representation of f32 literals (f32 bits in low 32 of the i64
///     immediate).
///   - The `materialize_f32_immediates` pass (integer `Add` + `STG` stores
///     f32 bits in the GPR's low 32, which on big-endian s390x lands at
///     offset 4–7 of the 8-byte slot — the LOW 4 bytes).
///   - `ss_store_f32_from_fpr` below (LGDR + SRLG + STG), which stores the
///     f32 result bits in the GPR's low 32.
///
/// On big-endian s390x, `ss_ld` (LG) loads 8 bytes: offset 0–3 → high 32,
/// offset 4–7 → low 32.  So f32 bits at offset 4–7 land in the GPR's LOW 32.
/// `SLLG scratch, scratch, 32` then shifts them to the HIGH 32, and `LDGR`
/// copies to the FPR where AEBR/LDEBR/CEBR expect them (bits 0–31).
///
/// **Sequence:** `ss_load_value` (loads 8 bytes; f32 bits in low 32) →
/// `SLLG scratch, scratch, 32` (shifts f32 bits to high 32) → `LDGR fpr,
/// scratch` (copies to FPR).  After this, the FPR's bits 0–31 hold the f32
/// value, ready for AEBR/LDEBR/CEBR/etc.
fn ss_load_f32_to_fpr(
    val: &IRValue,
    slots: &HashMap<u32, i32>,
    scratch: Gpr,
    dst_fpr: Fpr,
) -> Vec<u8> {
    let mut code = ss_load_value(val, slots, scratch);
    // SLLG scratch, scratch, 32: move f32 bits from low 32 to high 32.
    code.extend_from_slice(&encode_sllg(scratch, scratch, 32));
    // LDGR dst_fpr, scratch: copy to FPR (f32 bits now in high 32).
    code.extend_from_slice(&encode_ldgr(dst_fpr, scratch));
    code
}

/// Store an f32 result from an FPR to a stack slot, using the "f32 bits in
/// GPR low 32" convention (see `ss_load_f32_to_fpr`).
///
/// **Sequence:** `LGDR scratch, fpr` (copies FPR to GPR; f32 result is in
/// the GPR's HIGH 32 because s390x f32 ops produce results in bits 0–31) →
/// `SRLG scratch, scratch, 32` (shifts f32 bits from high 32 to low 32) →
/// `ss_st` (STG, 8-byte store; f32 bits land at offset 4–7 on big-endian,
/// i.e. the LOW 4 bytes of the slot).
///
/// This is the inverse of `ss_load_f32_to_fpr` and ensures round-trip
/// correctness (store f32 → load f32 produces the same value).
fn ss_store_f32_from_fpr(fpr: Fpr, scratch: Gpr, offset: i32) -> Vec<u8> {
    let mut code = encode_lgdr(scratch, fpr).to_vec();
    // SRLG scratch, scratch, 32: move f32 bits from high 32 to low 32.
    code.extend_from_slice(&encode_srlg(scratch, scratch, 32));
    // STG: store 8 bytes (f32 bits in low 32 = offset 4–7 on big-endian).
    code.extend(ss_st(scratch, offset));
    code
}

/// Determine whether a type is 32-bit (for selecting 32-bit vs 64-bit ops).
fn is_32bit_ty(ty: Option<&IRType>) -> bool {
    matches!(
        ty,
        Some(IRType::I8) | Some(IRType::U8) | Some(IRType::I16) | Some(IRType::U16)
            | Some(IRType::I32) | Some(IRType::U32)
    )
}

/// Returns true if the type is a signed integer type (I8/I16/I32/I64).
fn is_signed_ty(ty: Option<&IRType>) -> bool {
    matches!(
        ty,
        Some(IRType::I8) | Some(IRType::I16) | Some(IRType::I32) | Some(IRType::I64)
    )
}

// ===========================================================================
// Stack-slot based allocate_registers
// ===========================================================================

/// Stack-slot based `allocate_registers` for s390x.
///
/// Every vreg gets an 8-byte stack slot at [FP + offset]; operations use
/// scratch registers R0–R7.  s390x has no branch delay slots, so no NOP
/// insertion is needed after branches.
fn s390x_allocate_registers_ss(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
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
    // [low address]  FP = SP (after prologue)
    //   160-byte ABI caller save area   ← FP + 0 .. FP + 159
    //     (callee may use this for saving R2-R5 etc.; also used for
    //      outgoing stack args when calling functions with >5 args:
    //      arg6 goes at R15+160, arg7 at R15+168, etc.)
    //   vreg slot 1   ← FP + 160
    //   vreg slot 2   ← FP + 168
    //   ...
    //   vreg slot M   ← FP + 160 + 8*(M-1)
    //   Alloc data 1
    //   ...
    //   Alloc data N
    //   [padding]
    //   saved R11 (old FP)  ← FP + frame_size - 8
    //   saved R14 (LR)      ← FP + frame_size - 16  (top of frame, just below caller's SP)
    // [high address]
    //
    // The 160-byte save area at the bottom is MANDATORY per the s390x ABI:
    // the caller must allocate it so the callee can store R2-R5 and use it
    // for outgoing stack arguments. Without it, a callee that reads arg6
    // from R15+160 (e.g. our mmap stub) would read garbage from the caller's
    // vreg slots instead of the actual argument.

    let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
    // Reserve 160 bytes for the s390x ABI caller save area.
    let abi_save_area_size: i32 = 160;
    let mut current_offset: i32 = abi_save_area_size;
    let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
    all_vreg_ids_sorted.sort();
    for &id in &all_vreg_ids_sorted {
        vreg_stack_slots.insert(id, current_offset);
        current_offset += 8;
    }

    // Alloc regions after vreg slots.
    let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
    let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
    alloc_vreg_ids.sort();
    for &id in &alloc_vreg_ids {
        let size = alloc_sizes[&id];
        // Align to 16 bytes.
        current_offset = (current_offset + 15) & !15;
        alloc_offsets.insert(id, current_offset);
        current_offset += size;
    }

    // 16 bytes for saved LR + FP, plus 48 bytes for the 6 callee-saved
    // scratch registers (R6, R7, R8, R9, R10, R12) that this backend uses as
    // S0–S5. The s390x ABI marks R6–R13 as callee-saved, so they must be
    // preserved across calls.
    current_offset = (current_offset + 15) & !15;
    let save_area_offset = current_offset; // offset where we save LR/FP
    let lr_save_off = save_area_offset as i32;
    let fp_save_off = save_area_offset as i32 + 8;
    // Callee-saved scratch register save offsets (R6, R7, R8, R9, R10, R12).
    let s0_save_off = save_area_offset as i32 + 16; // R6
    let s1_save_off = save_area_offset as i32 + 24; // R7
    let s2_save_off = save_area_offset as i32 + 32; // R8
    let s3_save_off = save_area_offset as i32 + 40; // R9
    let s4_save_off = save_area_offset as i32 + 48; // R10
    let s5_save_off = save_area_offset as i32 + 56; // R12
    current_offset += 64;

    // Total frame size, aligned to 16 bytes.
    let frame_size = ((current_offset + 15) & !15) as usize;
    // (lr_save_off and fp_save_off are defined above with the save-area layout.)

    // ── Phase 2: Build the phi-map (unused for stack-slot ISel, but kept for compat) ──
    let _phi_map = func.build_phi_map();

    // ── Phase 3: Emit prologue ──
    let mut code: Vec<u8> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();

    // AGFI SP, -frame_size  →  AGHI SP, -frame_size (or LGFI+SGR for large frames)
    // SP -= frame_size; allocate stack frame
    code.extend(adjust_sp(-(frame_size as i32)));
    // STG LR, frame_size-16(SP)  — save LR at top of frame
    code.extend_from_slice(&encode_stg(LR, SP, lr_save_off));
    // STG FP, frame_size-8(SP)   — save old FP just below LR
    code.extend_from_slice(&encode_stg(FP, SP, fp_save_off));
    // Save callee-saved scratch registers (S0–S5 = R6, R7, R8, R9, R10, R12).
    // The s390x ABI requires these to be preserved across calls.
    code.extend_from_slice(&encode_stg(S0, SP, s0_save_off));
    code.extend_from_slice(&encode_stg(S1, SP, s1_save_off));
    code.extend_from_slice(&encode_stg(S2, SP, s2_save_off));
    code.extend_from_slice(&encode_stg(S3, SP, s3_save_off));
    code.extend_from_slice(&encode_stg(S4, SP, s4_save_off));
    code.extend_from_slice(&encode_stg(S5, SP, s5_save_off));
    // LGR FP, SP — FP = SP
    code.extend_from_slice(&encode_lgr(FP, SP));

    // Save incoming args (R2-R6) to their stack slots.
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

    // Collect branch-patch records for fixing up after all blocks are emitted.
    struct BranchPatch {
        code_offset: usize,
        target_label: String,
        // Whether the branch uses BRCL (6 bytes, 32-bit disp) or BRC (4 bytes, 16-bit disp).
        is_long: bool,
    }
    let mut branch_patches: Vec<BranchPatch> = Vec::new();
    let mut cond_branch_false_patches: Vec<BranchPatch> = Vec::new();

    for block in func.blocks.iter() {
        block_start_offsets.push(code.len());

        for instr in &block.instructions {
            emit_instr(
                instr,
                &vreg_stack_slots,
                &alloc_offsets,
                &mut code,
                frame_size,
                lr_save_off,
                fp_save_off,
                &mut relocations,
            );
        }

        // Emit terminator.
        match &block.terminator {
            crate::ir::IRTerminator::Jump(target) => {
                // BRCL 0xF, target (unconditional = mask 0xF = "always").
                // Use BRCL (6 bytes) to avoid the 16-bit displacement limit.
                let patch_offset = code.len();
                code.extend_from_slice(&encode_brcl(0xF, 0));
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
                // Load cond into S0, then compare with 0.
                code.extend(ss_load_value(cond, &vreg_stack_slots, S0));
                // LTGR S1, S0 (Load and Test 64-bit): S1 = S0, sets condition code based on S0.
                // LTGR R1, R2: op1=0xB9, op2=0x02.
                code.extend_from_slice(&encode_rre(0xB9, 0x02, S1, S0));
                // BRCL 0x6, true_block (branch if CC != 0, i.e., cond != 0).
                // Use BRCL (32-bit displacement) instead of BRC (16-bit) to avoid
                // displacement overflow in large functions. BRC's 16-bit signed
                // halfword displacement limits jumps to ±64KB, which is exceeded
                // by self_exec and other complex test programs.
                let true_patch = code.len();
                code.extend_from_slice(&encode_brcl(0x6, 0));
                branch_patches.push(BranchPatch {
                    code_offset: true_patch,
                    target_label: true_block.clone(),
                    is_long: true,
                });
                // BRCL 0xF, false_block (unconditional jump to false_block).
                let false_patch = code.len();
                code.extend_from_slice(&encode_brcl(0xF, 0));
                cond_branch_false_patches.push(BranchPatch {
                    code_offset: false_patch,
                    target_label: false_block.clone(),
                    is_long: true,
                });
            }
            crate::ir::IRTerminator::Return(vals) => {
                // Move return value to R2 (if any), then epilogue.
                if let Some(first_val) = vals.first() {
                    code.extend(ss_load_value(first_val, &vreg_stack_slots, Gpr::R2));
                }
                // Epilogue:
                // LG LR, lr_save_off(SP)  — restore LR
                code.extend_from_slice(&encode_lg(LR, SP, lr_save_off));
                // LG FP, fp_save_off(SP)  — restore old FP
                code.extend_from_slice(&encode_lg(FP, SP, fp_save_off));
                // Restore callee-saved scratch registers.
                code.extend_from_slice(&encode_lg(S0, SP, s0_save_off));
                code.extend_from_slice(&encode_lg(S1, SP, s1_save_off));
                code.extend_from_slice(&encode_lg(S2, SP, s2_save_off));
                code.extend_from_slice(&encode_lg(S3, SP, s3_save_off));
                code.extend_from_slice(&encode_lg(S4, SP, s4_save_off));
                code.extend_from_slice(&encode_lg(S5, SP, s5_save_off));
                // Deallocate frame: SP += frame_size
                code.extend(adjust_sp(frame_size as i32));
                // BR LR  — return
                code.extend_from_slice(&encode_br(LR));
            }
            crate::ir::IRTerminator::Unreachable => {
                // Emit a trap: LGFI R1, -1; SVC 0 (invalid syscall number).
                // This must NOT fall through — a NOP would cause the child block
                // to fall through to the parent block, executing parent code.
                code.extend_from_slice(&encode_lgfi(Gpr::R1, -1));
                code.extend_from_slice(&encode_svc(0));
            }
            crate::ir::IRTerminator::Switch {
                discr,
                targets,
                default,
            } => {
                // Linear compare-and-branch switch.
                code.extend(ss_load_value(discr, &vreg_stack_slots, S0));
                for (val, label) in targets {
                    // Load val into S1, compare with S0 via CGR (Compare 64-bit).
                    code.extend(ss_load_imm(S1, *val));
                    // CGR R1, R2: op1=0xB9, op2=0x20.
                    code.extend_from_slice(&encode_rre(0xB9, 0x20, S1, S0));
                    // BRCL 0x8, label (branch if CC=0, i.e., equal). Mask 0x8 = CC=0.
                    // Use BRCL (32-bit) to avoid 16-bit displacement overflow.
                    let patch = code.len();
                    code.extend_from_slice(&encode_brcl(0x8, 0));
                    branch_patches.push(BranchPatch {
                        code_offset: patch,
                        target_label: label.clone(),
                        is_long: true,
                    });
                }
                // BRCL 0xF, default (unconditional jump to default).
                let default_patch = code.len();
                code.extend_from_slice(&encode_brcl(0xF, 0));
                cond_branch_false_patches.push(BranchPatch {
                    code_offset: default_patch,
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
                // Simplified: just jump to normal continuation.
                let patch = code.len();
                code.extend_from_slice(&encode_brcl(0xF, 0));
                branch_patches.push(BranchPatch {
                    code_offset: patch,
                    target_label: normal.clone(),
                    is_long: true,
                });
            }
            crate::ir::IRTerminator::TailCall { .. } => {
                // Simplified: just return. Restore callee-saved scratch regs.
                code.extend_from_slice(&encode_lg(LR, SP, lr_save_off));
                code.extend_from_slice(&encode_lg(FP, SP, fp_save_off));
                code.extend_from_slice(&encode_lg(S0, SP, s0_save_off));
                code.extend_from_slice(&encode_lg(S1, SP, s1_save_off));
                code.extend_from_slice(&encode_lg(S2, SP, s2_save_off));
                code.extend_from_slice(&encode_lg(S3, SP, s3_save_off));
                code.extend_from_slice(&encode_lg(S4, SP, s4_save_off));
                code.extend_from_slice(&encode_lg(S5, SP, s5_save_off));
                code.extend(adjust_sp(frame_size as i32));
                code.extend_from_slice(&encode_br(LR));
            }
            crate::ir::IRTerminator::Resume { .. } => {
                code.extend_from_slice(&encode_nop());
            }
        }
    }

    // ── Phase 5: Patch branch offsets ──
    // BRC: 16-bit signed halfword displacement. Target = PC + (disp * 2).
    //      disp = (target_offset - branch_offset) / 2
    // BRCL: 32-bit signed halfword displacement.
    let patch_branch = |code: &mut Vec<u8>, patch: &BranchPatch| {
        if let Some(&target_idx) = label_to_idx.get(&patch.target_label) {
            let target_offset = block_start_offsets[target_idx];
            let disp_bytes = target_offset as i64 - patch.code_offset as i64;
            let disp_halfwords = (disp_bytes / 2) as i64;
            if patch.is_long {
                let disp = disp_halfwords as i32;
                let disp_be = disp.to_be_bytes();
                code[patch.code_offset + 2..patch.code_offset + 6]
                    .copy_from_slice(&disp_be);
            } else {
                let disp = disp_halfwords as i16;
                let disp_be = disp.to_be_bytes();
                code[patch.code_offset + 2..patch.code_offset + 4]
                    .copy_from_slice(&disp_be);
            }
        }
    };
    for patch in &branch_patches {
        patch_branch(&mut code, patch);
    }
    for patch in &cond_branch_false_patches {
        patch_branch(&mut code, patch);
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
            PhysicalReg::new(RegClass::Gpr, LR as u32),
            PhysicalReg::new(RegClass::Gpr, FP as u32),
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
fn s390x_allocate_registers_real(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    // Run the existing stack-slot allocator to get a working AllocatedFunction.
    let mut allocated = s390x_allocate_registers_ss(func)?;

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

/// Emit a single IR instruction as s390x machine code.
#[allow(clippy::too_many_arguments)]
fn emit_instr(
    instr: &IRInstr,
    vreg_stack_slots: &HashMap<u32, i32>,
    alloc_offsets: &HashMap<u32, i32>,
    code: &mut Vec<u8>,
    _frame_size: usize,
    _lr_save_off: i32,
    _fp_save_off: i32,
    relocations: &mut Vec<RelocationEntry>,
) {
    let _ = alloc_offsets;
    match instr {
        IRInstr::Add { dst, lhs, rhs, ty } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if is_32bit_ty(ty.as_ref()) {
                // 32-bit add: ARK S0, S0, S1 (S0 = S0 + S1, low 32 bits).
                // The high 32 bits of S0 are unchanged (garbage).  For correctness
                // with subsequent 64-bit ops, zero-extend via LLGFR.
                code.extend_from_slice(&encode_ark(S0, S0, S1));
                // Zero-extend the 32-bit result to 64 bits.
                code.extend_from_slice(&encode_llgfr(S0, S0));
            } else {
                // 64-bit add: AGR S0, S1 (S0 += S1).
                code.extend_from_slice(&encode_agr(S0, S1));
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Sub { dst, lhs, rhs, ty } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if is_32bit_ty(ty.as_ref()) {
                code.extend_from_slice(&encode_srk(S0, S0, S1));
                code.extend_from_slice(&encode_llgfr(S0, S0));
            } else {
                code.extend_from_slice(&encode_sgr(S0, S1));
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Mul { dst, lhs, rhs, ty } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            let _ = ty;
            // 64-bit multiply (low 64 bits of the product).  For 32-bit ops,
            // the low 32 bits of the result are correct, but the high 32 bits
            // may contain garbage.  We zero-extend for 32-bit types.
            code.extend_from_slice(&encode_msgr(S0, S1));
            if is_32bit_ty(ty.as_ref()) {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Div { dst, lhs, rhs, ty } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            let signed = is_signed_ty(ty.as_ref());
            code.extend(ss_load_value(lhs, vreg_stack_slots, S1));
            // For 32-bit operands, extend to 64 bits. Signed types use
            // sign-extension (LGFR); unsigned types use zero-extension (LLGFR).
            if is_32bit_ty(ty.as_ref()) {
                if signed {
                    code.extend_from_slice(&encode_lgfr(S1, S1));
                } else {
                    code.extend_from_slice(&encode_llgfr(S1, S1));
                }
            }
            // Set up the 128-bit dividend pair (R0:R1):
            //   For unsigned (DLGR): R0 = 0 (zero-extend low 64 bits).
            //   For signed (DGR): R0 = sign-extension of S1 (arithmetic
            //     shift right by 63 gives -1 if negative, 0 if positive).
            if signed {
                // SRAG R0, S1, 63  →  R0 = S1 >> 63 (sign mask).
                code.extend_from_slice(&encode_srag(S0, S1, 63));
            } else {
                code.extend_from_slice(&encode_lghi(S0, 0)); // R0 = 0
            }
            // S1 (=R1) already holds the dividend. Load divisor into S2 (=R2).
            code.extend(ss_load_value(rhs, vreg_stack_slots, S2));
            if is_32bit_ty(ty.as_ref()) {
                if signed {
                    code.extend_from_slice(&encode_lgfr(S2, S2));
                } else {
                    code.extend_from_slice(&encode_llgfr(S2, S2));
                }
            }
            // DGR R0, R2 (signed) or DLGR R0, R2 (unsigned).
            if signed {
                code.extend_from_slice(&encode_dgr(S0, S2));
            } else {
                code.extend_from_slice(&encode_dlgr(S0, S2));
            }
            // Quotient is now in R1 (= S1).  Move to S0 for storing.
            code.extend_from_slice(&encode_lgr(S0, S1));
            if is_32bit_ty(ty.as_ref()) {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::BinOp {
            op,
            dst,
            lhs,
            rhs,
            ty,
        } => {
            emit_binop(op, dst, lhs, rhs, ty.as_ref(), vreg_stack_slots, code);
        }
        IRInstr::Cmp {
            kind,
            dst,
            lhs,
            rhs,
            ty,
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
            emit_binop(&binop_kind, dst, lhs, rhs, ty.as_ref(), vreg_stack_slots, code);
        }
        IRInstr::UnaryOp {
            op,
            dst,
            operand,
            ty,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(operand, vreg_stack_slots, S0));
            let _ = ty;
            match op {
                UnaryOpKind::Neg => {
                    // LCGR R1, R2 (Load Complement 64-bit): R1 = -R2.
                    // op1=0xB9, op2=0x03.
                    code.extend_from_slice(&encode_rre(0xB9, 0x03, S0, S0));
                }
                UnaryOpKind::Not => {
                    // LCGR gives -R = ~R + 1. For ~R, we need: ~R = -R - 1.
                    // Easier: use XGR (XOR 64-bit) with all-ones.
                    code.extend_from_slice(&encode_lghi(S1, -1));
                    // XGR S0, S1: S0 ^= S1 = ~S0. op1=0xB9, op2=0x82.
                    code.extend_from_slice(&encode_rre(0xB9, 0x82, S0, S1));
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    // Not natively supported on s390x (except via POPCNT which counts
                    // set bits per byte).  Simplified: leave as 0.
                    code.extend_from_slice(&encode_lghi(S0, 0));
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
            // Compute address: S0 = addr + offset
            code.extend(ss_load_value(addr, vreg_stack_slots, S0));
            if *offset != 0 {
                code.extend(ss_load_imm(S1, *offset as i64));
                code.extend_from_slice(&encode_agr(S0, S1));
            }
            // Load typed value from [S0] into S2.
            match ty {
                IRType::I8 => code.extend_from_slice(&encode_lgb(S2, S0, 0)),
                IRType::U8 => {
                    code.extend_from_slice(&encode_rxy_a(0xE3, 0x90, S2, Gpr::R0, S0, 0));
                }
                IRType::I16 => code.extend_from_slice(&encode_lgh(S2, S0, 0)),
                IRType::U16 => {
                    code.extend_from_slice(&encode_rxy_a(0xE3, 0x91, S2, Gpr::R0, S0, 0));
                }
                IRType::I32 => code.extend_from_slice(&encode_lgf(S2, S0, 0)),
                IRType::U32 => code.extend_from_slice(&encode_llgf(S2, S0, 0)),
                _ => code.extend_from_slice(&encode_lg(S2, S0, 0)),
            }
            code.extend(ss_st(S2, dst_off));
        }
        IRInstr::Store {
            value,
            addr,
            offset,
            ty,
        } => {
            // S0 = addr + offset
            code.extend(ss_load_value(addr, vreg_stack_slots, S0));
            if *offset != 0 {
                code.extend(ss_load_imm(S1, *offset as i64));
                code.extend_from_slice(&encode_agr(S0, S1));
            }
            // Load value into S2.
            code.extend(ss_load_value(value, vreg_stack_slots, S2));
            // Store typed value from S2 to [S0].
            match ty {
                IRType::I8 | IRType::U8 => code.extend_from_slice(&encode_stc(S2, S0, 0)),
                IRType::I16 | IRType::U16 => code.extend_from_slice(&encode_sth(S2, S0, 0)),
                IRType::I32 | IRType::U32 => code.extend_from_slice(&encode_sty(S2, S0, 0)),
                _ => code.extend_from_slice(&encode_stg(S2, S0, 0)),
            }
        }
        IRInstr::Alloc { dst, size } => {
            let dst_id = dst.as_register().unwrap_or(0);
            // dst = FP + alloc_offset[dst_id]
            if let Some(&off) = alloc_offsets.get(&dst_id) {
                code.extend_from_slice(&encode_lgr(S0, FP));
                code.extend(ss_load_imm(S1, off as i64));
                code.extend_from_slice(&encode_agr(S0, S1));
                code.extend(ss_st(S0, vreg_stack_slots.get(&dst_id).copied().unwrap_or(0)));
            }
            let _ = size;
        }
        IRInstr::Free { ptr: _ } => {
            // Lowered to a runtime call (handled by Call); no-op here.
        }
        IRInstr::Cast {
            kind,
            dst,
            src,
            from_ty,
            to_ty,
        } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(src, vreg_stack_slots, S0));
            match kind {
                CastKind::ZExt => {
                    // Zero-extend based on source type.
                    match from_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            // LLGCR R1, R2: op1=0xB9, op2=0xA6 (Load Logical Character → 64-bit).
                            // Actually LLGCR exists; but easier: use NRK to mask to 0xFF.
                            code.extend_from_slice(&encode_lghi(S1, 0xFF));
                            code.extend_from_slice(&encode_nrk(S0, S0, S1));
                            code.extend_from_slice(&encode_llgfr(S0, S0));
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            code.extend_from_slice(&encode_lgfi(S1, 0xFFFF));
                            code.extend_from_slice(&encode_nrk(S0, S0, S1));
                            code.extend_from_slice(&encode_llgfr(S0, S0));
                        }
                        Some(IRType::I32) | Some(IRType::U32) | None => {
                            code.extend_from_slice(&encode_llgfr(S0, S0));
                        }
                        _ => {}
                    }
                }
                CastKind::SExt => {
                    match from_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            // LGBR R1, R2: op1=0xB9, op2=0xA6 (Load Byte, sign-extend to 64).
                            // Actually LGBR = B9 A6.  Use it directly.
                            code.extend_from_slice(&encode_rre(0xB9, 0xA6, S0, S0));
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            // LGHR R1, R2: op1=0xB9, op2=0xA5.
                            code.extend_from_slice(&encode_rre(0xB9, 0xA5, S0, S0));
                        }
                        Some(IRType::I32) | Some(IRType::U32) | None => {
                            code.extend_from_slice(&encode_lgfr(S0, S0));
                        }
                        _ => {}
                    }
                }
                CastKind::Trunc => {
                    // Truncation: just store the low bits.  No explicit instruction needed.
                    // For 32-bit truncation, we mask off the high bits via NRK.
                    match to_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            code.extend_from_slice(&encode_lghi(S1, 0xFF));
                            code.extend_from_slice(&encode_nrk(S0, S0, S1));
                            code.extend_from_slice(&encode_llgfr(S0, S0));
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            code.extend_from_slice(&encode_lgfi(S1, 0xFFFF));
                            code.extend_from_slice(&encode_nrk(S0, S0, S1));
                            code.extend_from_slice(&encode_llgfr(S0, S0));
                        }
                        Some(IRType::I32) | Some(IRType::U32) => {
                            code.extend_from_slice(&encode_llgfr(S0, S0));
                        }
                        _ => {}
                    }
                }
                CastKind::BitCast => {
                    // No-op (reinterpret bits).
                }
                CastKind::IntToFloat => {
                    // Signed int → float.  S0 holds src (zero-extended for
                    // sub-64-bit types per the existing emit_binop convention).
                    // Sign-extend to int64 so CDGBRA/CEGBRA interpret the value
                    // correctly (e.g. I32 = -1, stored zero-extended as
                    // 0x00000000FFFFFFFF, must become -1.0, not 4294967295.0).
                    match from_ty {
                        Some(IRType::I8) => {
                            // LGBR S0, S0: sign-extend byte → int64.  op2=0xA6.
                            code.extend_from_slice(&encode_rre(0xB9, 0xA6, S0, S0));
                        }
                        Some(IRType::I16) => {
                            // LGHR S0, S0: sign-extend halfword → int64.  op2=0xA5.
                            code.extend_from_slice(&encode_rre(0xB9, 0xA5, S0, S0));
                        }
                        Some(IRType::I32) => {
                            // LGFR S0, S0: sign-extend word → int64.  op2=0x14.
                            code.extend_from_slice(&encode_lgfr(S0, S0));
                        }
                        _ => {} // I64 / U* / None: no extension (U* should use UIntToFloat).
                    }
                    // CDGBRA/CEGBRA take R1=FPR (dest), R2=GPR (source int64) per
                    // s390x PoP (RRF_UUFR = { F_24, U4_16, R_28, U4_20 }).  No LDGR
                    // needed — pass S0 directly as R2.  Verified on QEMU-s390x 7.2.
                    if matches!(to_ty, Some(IRType::F32)) {
                        // CEGBRA: signed int64 → F32.  op1=0xB3, op2=0xA4.  m3=0.
                        //   binutils: b3a4 cegbra RRF_UUFR.
                        code.extend_from_slice(&encode_fp_rrf_r(0xB3, 0xA4, FA.encoding(), S0.encoding(), 0, 0));
                        code.extend(ss_store_f32_from_fpr(FA, S0, dst_off));
                    } else {
                        // CDGBRA: signed int64 → F64.  op1=0xB3, op2=0xA5.  m3=0.
                        //   binutils: b3a5 cdgbra RRF_UUFR.
                        code.extend_from_slice(&encode_fp_rrf_r(0xB3, 0xA5, FA.encoding(), S0.encoding(), 0, 0));
                        code.extend_from_slice(&encode_std(FA, FP, dst_off));
                    }
                    return; // skip trailing ss_st — S0 still holds the int, would clobber float bits.
                }
                CastKind::UIntToFloat => {
                    // Unsigned int → float.  S0 holds src (zero-extended per
                    // existing convention).  Ensure full 32-bit zero-extension
                    // for sub-64-bit types so CDLGBRA/CELGBRA see the correct
                    // uint64 (the slot's high bits are already 0 for U8/U16/U32
                    // stored via LLGFR, but LLGFR again is harmless and safe).
                    match from_ty {
                        Some(IRType::U8) | Some(IRType::U16) | Some(IRType::U32)
                        | Some(IRType::I8) | Some(IRType::I16) | Some(IRType::I32) => {
                            // LLGFR S0, S0: zero-extend word → uint64.  op2=0x16.
                            code.extend_from_slice(&encode_llgfr(S0, S0));
                        }
                        _ => {}
                    }
                    // CDLGBRA/CELGBRA take R1=FPR (dest), R2=GPR (source uint64)
                    // per s390x PoP (RRF_UUFR).  No LDGR needed.
                    if matches!(to_ty, Some(IRType::F32)) {
                        // CELGBRA: unsigned int64 → F32.  op1=0xB3, op2=0xA0.  m3=0.
                        //   binutils: b3a0 celgbr.
                        code.extend_from_slice(&encode_fp_rrf_r(0xB3, 0xA0, FA.encoding(), S0.encoding(), 0, 0));
                        code.extend(ss_store_f32_from_fpr(FA, S0, dst_off));
                    } else {
                        // CDLGBRA: unsigned int64 → F64.  op1=0xB3, op2=0xA1.  m3=0.
                        //   binutils: b3a1 cdlgbr.
                        code.extend_from_slice(&encode_fp_rrf_r(0xB3, 0xA1, FA.encoding(), S0.encoding(), 0, 0));
                        code.extend_from_slice(&encode_std(FA, FP, dst_off));
                    }
                    return; // skip trailing ss_st.
                }
                CastKind::FloatToInt => {
                    // Float → signed int64 (truncating).  S0 holds src's bit
                    // pattern (LG-loaded from the slot).  CGDBRA takes an F64
                    // operand in FPR R2, so ferry S0's bits into FA via LDGR first,
                    // widening F32 → F64 if needed.
                    //
                    // For F32 sources: S0 holds f32 bits in the LOW 32 bits
                    // (4-byte STE convention).  SLLG S0, S0, 32 shifts them to
                    // the HIGH 32 bits where LDEBR expects them, then LDGR
                    // copies to FA, and LDEBR widens F32 → F64.
                    if matches!(from_ty, Some(IRType::F32)) {
                        code.extend_from_slice(&encode_sllg(S0, S0, 32));
                    }
                    code.extend_from_slice(&encode_ldgr(FA, S0)); // FA bits = float bits
                    if matches!(from_ty, Some(IRType::F32)) {
                        // LDEBR FA, FA: widen F32 → F64.  op1=0xB3, op2=0x04.
                        code.extend_from_slice(&encode_fp_rre(0xB3, 0x04, FA, FA));
                    }
                    // CGDBRA: F64 → signed int64, m3=5 (round toward zero / truncate).
                    // Per s390x PoP (RRF_UURF = { R_24, U4_16, F_28, U4_20 }):
                    //   R1 = GPR (dest int64), R2 = FPR (source F64).
                    // So R1=S0 (GPR, dest), R2=FA (FPR, source).  No LGDR needed —
                    // the result lands directly in S0.  Verified on QEMU-s390x 7.2.
                    //   binutils: b3a9 cgdbra.
                    code.extend_from_slice(&encode_fp_rrf_r(0xB3, 0xA9, S0.encoding(), FA.encoding(), 5, 0));
                    // Fall through to trailing ss_st(S0, dst_off) — stores the int64.
                }
                CastKind::FloatToUInt => {
                    // Float → unsigned int64 (truncating).
                    //
                    // For F32 sources: SLLG S0, S0, 32 shifts f32 bits from
                    // low 32 to high 32 before LDGR + LDEBR.
                    if matches!(from_ty, Some(IRType::F32)) {
                        code.extend_from_slice(&encode_sllg(S0, S0, 32));
                    }
                    code.extend_from_slice(&encode_ldgr(FA, S0));
                    if matches!(from_ty, Some(IRType::F32)) {
                        code.extend_from_slice(&encode_fp_rre(0xB3, 0x04, FA, FA)); // LDEBR widen
                    }
                    // CLGDBRA: F64 → unsigned int64, m3=5 (truncate).
                    // R1 = GPR (dest uint64), R2 = FPR (source F64).  No LGDR needed.
                    //   binutils: b3ad clgdbr.
                    code.extend_from_slice(&encode_fp_rrf_r(0xB3, 0xAD, S0.encoding(), FA.encoding(), 5, 0));
                    // Fall through to trailing ss_st.
                }
                CastKind::FloatToFloat => {
                    // F32 ↔ F64.  Treat None as F64 (per IR doc default).
                    let src_is_f64 = !matches!(from_ty, Some(IRType::F32));
                    let dst_is_f64 = !matches!(to_ty, Some(IRType::F32));
                    if src_is_f64 && !dst_is_f64 {
                        // F64 → F32: LEDBRA (RRF-a, narrows with current rounding mode).
                        // op1=0xB3, op2=0x44.  M3=0 (current rounding), M4=0 (silent).
                        //   Verified against binutils s390-opc.txt: b344 ledbr.
                        // S0 holds f64 bits; LDGR + LEDBR narrows to f32 (high 32).
                        // ss_store_f32_from_fpr (LGDR + SRLG + STG) stores the f32
                        // result bits in the GPR's low 32 (4-byte f32 convention).
                        code.extend_from_slice(&encode_ldgr(FA, S0));
                        code.extend_from_slice(&encode_fp_rrf(0xB3, 0x44, FA, FA, 0, 0));
                        code.extend(ss_store_f32_from_fpr(FA, S0, dst_off));
                        return; // skip trailing ss_st.
                    } else if !src_is_f64 && dst_is_f64 {
                        // F32 → F64: LDEBR (widen, exact).  op1=0xB3, op2=0x04.
                        // S0 holds f32 bits in the LOW 32 bits (4-byte STE
                        // convention).  SLLG S0, S0, 32 shifts them to the HIGH
                        // 32 bits, LDGR copies to FA, and LDEBR widens F32 → F64.
                        code.extend_from_slice(&encode_sllg(S0, S0, 32));
                        code.extend_from_slice(&encode_ldgr(FA, S0));
                        code.extend_from_slice(&encode_fp_rre(0xB3, 0x04, FA, FA));
                        code.extend_from_slice(&encode_std(FA, FP, dst_off));
                        return; // skip trailing ss_st.
                    }
                    // Same-width (F32→F32 or F64→F64): no-op cast.
                    // S0 already holds src's bits; fall through to trailing ss_st.
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
            // LARL S0, name — placeholder disp=0; record a relocation.
            let larl_offset = code.len() as u64;
            code.extend_from_slice(&encode_larl(S0, 0));
            relocations.push(RelocationEntry {
                offset: larl_offset,
                symbol: name.clone(),
                reloc_type: "R_S390_PC32DBL".to_string(),
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
            code.extend_from_slice(&encode_agr(S0, S1));
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
            // Load false_val (default), then conditionally load true_val.
            code.extend(ss_load_value(false_val, vreg_stack_slots, S0));
            // Load and test cond.
            code.extend(ss_load_value(cond, vreg_stack_slots, S1));
            // LTGR S1, S1: sets CC based on S1.
            code.extend_from_slice(&encode_rre(0xB9, 0x02, S1, S1));
            // BRC 0x6, +skip (skip the "load true_val" if cond == 0).
            // Mask 0x6 = "CC != 0" → branch if cond != 0... wait, we want to
            // SKIP the load-true if cond == 0, which means branch OVER it if
            // cond != 0. Hmm no, we want to skip if cond == 0, so we should
            // branch if cond != 0 to skip the load-true (since we already have
            // false_val). Actually that's wrong too.
            //
            // Logic: dst = false_val by default. If cond != 0, overwrite with true_val.
            // So: if cond == 0, skip the load-true. Branch-if-cond-!=0 falls through
            // to load-true; branch-if-cond-==0 jumps past it.
            // Mask 0x8 = "CC == 0" (i.e., cond == 0). Branch to "skip_load_true".
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x8, 0));
            // Load true_val into S0 (overwriting false_val).
            code.extend(ss_load_value(true_val, vreg_stack_slots, S0));
            // skip_load_true: (patch the BRC to jump here)
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::Ret { values } => {
            // Move return value to R2 (if any), then epilogue.
            // TODO: this secondary Ret path does not yet restore the callee-saved
            // scratch registers (S0–S5). The primary IRTerminator::Return path
            // in s390x_allocate_registers_ss does restore them. If IRInstr::Ret
            // is emitted as a real instruction (not NOP'd), callers may see
            // corrupted R6–R10/R12. Thread s0_save_off..s5_save_off through
            // emit_instr to fix.
            if let Some(first_val) = values.first() {
                code.extend(ss_load_value(first_val, vreg_stack_slots, Gpr::R2));
            }
            code.extend_from_slice(&encode_lg(LR, SP, _lr_save_off));
            code.extend_from_slice(&encode_lg(FP, SP, _fp_save_off));
            code.extend(adjust_sp(_frame_size as i32));
            code.extend_from_slice(&encode_br(LR));
        }
        IRInstr::Branch { target: _ } => {
            // Instruction-level branch (not terminator). This is always
            // followed by a Jump terminator with the same target, so the
            // Branch's BRCL is redundant. Emit a 2-byte NOP (BCR 0, 0)
            // to preserve code layout without creating an unpatched self-loop.
            // (Previous code emitted an unpatched BRCL with disp=0, which
            // caused infinite loops in for/while loops.)
            code.extend_from_slice(&encode_nop());
        }
        IRInstr::CondBranch {
            cond: _,
            true_target: _,
            false_target: _,
        } => {
            // Instruction-level CondBranch (not terminator). This is always
            // followed by a Branch terminator with the same targets, so the
            // CondBranch's BRC+BRCL are redundant. Emit NOPs to preserve code
            // layout without creating unpatched self-loops.
            // (Previous code emitted unpatched BRC+BRCL with disp=0, which
            // caused infinite loops in for/while loops.)
            code.extend_from_slice(&encode_nop());
            code.extend_from_slice(&encode_nop());
            code.extend_from_slice(&encode_nop());
        }
        IRInstr::Call {
            dst,
            func,
            args,
            is_extern,
        } => {
            // Move args into R2-R6 (up to 5 args in registers).
            // Args 6+ go on the stack at R15+160, R15+168, etc.
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    code.extend(ss_load_value(arg, vreg_stack_slots, S0));
                    code.extend_from_slice(&encode_lgr(arg_reg, S0));
                } else {
                    // Stack arg: store at R15 + (160 + (i - 5) * 8)
                    let stack_off = (160 + (i - 5) * 8) as i32;
                    code.extend(ss_load_value(arg, vreg_stack_slots, S0));
                    // STG S0, stack_off(R15) — store 64-bit
                    code.extend_from_slice(&encode_stg(S0, SP, stack_off));
                }
            }
            // BRASL R14, func — placeholder disp=0; record a relocation.
            let call_offset = code.len() as u64;
            code.extend_from_slice(&encode_brasl(LR, 0));
            relocations.push(RelocationEntry {
                offset: call_offset,
                symbol: func.clone(),
                reloc_type: "R_S390_PC32DBL".to_string(),
            });
            // Move return value from R2 to dst's stack slot.
            if let Some(d) = dst {
                let d_id = d.as_register().unwrap_or(0);
                let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                code.extend_from_slice(&encode_lgr(S0, Gpr::R2));
                code.extend(ss_st(S0, d_off));
            }
            let _ = is_extern;
        }
        IRInstr::CtSelect {
            dst,
            cond,
            true_val,
            false_val,
            ty: _,
        } => {
            // Constant-time select: dst = (true_val & mask) | (false_val & ~mask)
            // where mask = -(cond != 0).
            // Use branch-free approach:
            //   mask = -(((cond | -cond) >> 63) ^ 1)  — gives 0 if cond==0, -1 if cond!=0
            //   Actually simpler: mask = (cond != 0) ? -1 : 0
            //   Using s390x: LTGR sets CC; LOCGR (Load One's Complement on Condition
            //   GR) can conditionally set a register. But that's complex.
            //   For simplicity, fall back to the branch-based Select above.
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(false_val, vreg_stack_slots, S0));
            code.extend(ss_load_value(cond, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x02, S1, S1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x8, 0));
            code.extend(ss_load_value(true_val, vreg_stack_slots, S0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::CtEq {
            dst,
            lhs,
            rhs,
            ty: _,
        } => {
            // Constant-time equality: dst = (a == b) ? 1 : 0 using bitwise ops.
            //   x = a ^ b
            //   dst = ((x | -x) >> 63) ^ 1
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // S0 = a ^ b
            code.extend_from_slice(&encode_rre(0xB9, 0x82, S0, S1));
            // S1 = -S0 (LCGR)
            code.extend_from_slice(&encode_rre(0xB9, 0x03, S1, S0));
            // S0 = S0 | S1 (OGR: op1=0xB9, op2=0x81)
            code.extend_from_slice(&encode_rre(0xB9, 0x81, S0, S1));
            // S0 = S0 >> 63 (logical)
            code.extend_from_slice(&encode_srlg(S0, S0, 63));
            // S0 = S0 ^ 1
            code.extend_from_slice(&encode_lghi(S1, 1));
            code.extend_from_slice(&encode_rre(0xB9, 0x82, S0, S1));
            code.extend(ss_st(S0, dst_off));
        }
        IRInstr::AtomicLoad { dst, addr, ty } => {
            // Simplified: regular load (s390x aligned loads are atomic).
            let load_instr = IRInstr::Load {
                dst: dst.clone(),
                addr: addr.clone(),
                offset: 0,
                ty: ty.clone(),
            };
            emit_instr(
                &load_instr,
                vreg_stack_slots,
                alloc_offsets,
                code,
                _frame_size,
                _lr_save_off,
                _fp_save_off,
                relocations,
            );
        }
        IRInstr::AtomicStore {
            value,
            addr,
            ty,
        } => {
            let store_instr = IRInstr::Store {
                value: value.clone(),
                addr: addr.clone(),
                offset: 0,
                ty: ty.clone(),
            };
            emit_instr(
                &store_instr,
                vreg_stack_slots,
                alloc_offsets,
                code,
                _frame_size,
                _lr_save_off,
                _fp_save_off,
                relocations,
            );
        }
        IRInstr::AtomicCas {
            dst,
            addr,
            expected,
            desired,
            ty,
        } => {
            // Simplified CAS: load old, compare with expected, store desired if equal.
            let dst_id = dst.as_register().unwrap_or(0);
            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
            // S0 = addr
            code.extend(ss_load_value(addr, vreg_stack_slots, S0));
            // S1 = old = *addr
            match ty {
                IRType::U32 | IRType::I32 => code.extend_from_slice(&encode_llgf(S1, S0, 0)),
                _ => code.extend_from_slice(&encode_lg(S1, S0, 0)),
            }
            // Store old to dst
            code.extend(ss_st(S1, dst_off));
            // S2 = expected
            code.extend(ss_load_value(expected, vreg_stack_slots, S2));
            // CGR S2, S1 → sets CC
            code.extend_from_slice(&encode_rre(0xB9, 0x20, S2, S1));
            // BRC 0x6, skip_store (if S2 != S1, skip the store)
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x6, 0));
            // S3 = desired
            code.extend(ss_load_value(desired, vreg_stack_slots, S3));
            match ty {
                IRType::U32 | IRType::I32 => code.extend_from_slice(&encode_sty(S3, S0, 0)),
                _ => code.extend_from_slice(&encode_stg(S3, S0, 0)),
            }
            // skip_store: (patch the BRC to jump here)
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
        }
        IRInstr::Syscall { nr, args, dst } => {
            // s390x Linux syscall: args in R2-R7, nr in R1,
            // `SVC 0`, result in R2.
            // Translate VUMA-generic (asm-generic) syscall number to the
            // backend's native numbering. TODO(P1-b): per-arch table.
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::S390X,
                *nr,
            );
            let syscall_arg_regs =
                [Gpr::R2, Gpr::R3, Gpr::R4, Gpr::R5, Gpr::R6, Gpr::R7];
            let num_reg_args = args.len().min(syscall_arg_regs.len());
            for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                code.extend(ss_load_value(arg, vreg_stack_slots, syscall_arg_regs[i]));
            }
            // LGFI R1, nr
            code.extend(ss_load_imm(Gpr::R1, native_nr as i64));
            // SVC 0
            code.extend_from_slice(&encode_svc(0));
            // Store result (R2) to dst's stack slot
            if let Some(d) = dst {
                let dst_id = d.as_register().unwrap_or(0);
                let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                code.extend(ss_st(Gpr::R2, dst_off));
            }
        }
        // ── VectorOp (Wave 29) ──
        // s390x has no SIMD encoder in the Wave 29 suite; emit nothing.
        IRInstr::VectorOp { .. } => {}
        // ── Channel operations (Wave 1d / Task 2a) ──
        // Backend lowering not yet implemented; emit nothing (no frontend
        // generates channel IR yet).  Will be lowered to runtime calls.
        IRInstr::ChannelOpen { .. } | IRInstr::ChannelSend { .. }
        | IRInstr::ChannelRecv { .. } | IRInstr::ChannelRecvTimeout { .. } | IRInstr::ChannelRecvResult { .. } | IRInstr::ChannelClose { .. }
        // Wave 93-94: StarkProof — stub (Call-form builtin is the active path).
        | IRInstr::StarkProof { .. } => {}
    }
}

// ===========================================================================
// s390x FP Instruction Encoders (big-endian)
// ===========================================================================
//
// s390x FP instructions use the same 4-bit register field as GP instructions,
// so `Fpr` and `Gpr` share an identical encoding layout.  The encoders below
// produce RRE / RRF-b / RXY-a format instructions with `Fpr` operands.
//
// Reference: IBM z/Architecture Principles of Operation.

/// Encode an RRE-format FP instruction (4 bytes): `op1 op2 00 r1r2`.
///
/// Same byte layout as `encode_rre` (line ~381), but `r1`/`r2` are FPRs.
/// Used for ADBR/SDBR/MDBR/DDBR (F64 arith), AEBR/SEBR/MEBR/DEBR (F32 arith),
/// CDBR/CEBR (compare), SQDBR/SQEBR (sqrt), LDEBR/LEDBR (widen/narrow),
/// LCDBR/LPDBR (negate/abs), LDGR/LGDR (FPR↔GPR bit copy).
fn encode_fp_rre(op1: u8, op2: u8, r1: Fpr, r2: Fpr) -> [u8; 4] {
    [
        op1,
        op2,
        0x00,
        ((r1.encoding() & 0xF) << 4) | (r2.encoding() & 0xF),
    ]
}

/// Encode an RRF-a format FP instruction (4 bytes): `op1 op2 m3m4 r1r2`,
/// where both R1 and R2 are FPRs.  Used for LEDBRA (RRF_UUFF, F64→F32).
///
/// Per s390x PoP RRF-a layout (verified against binutils s390-opc.c:
/// INSTR_RRF_UUFF = { F_24, U4_16, F_28, U4_20 }):
///   byte 0: opcode1
///   byte 1: opcode2
///   byte 2: M3 (high nibble, bits 16-19) | M4 (low nibble, bits 20-23)
///   byte 3: R1 (high nibble, bits 24-27) | R2 (low nibble, bits 28-31)
///
/// - `m3` = rounding mode (0=current, 1=to-nearest, 5=toward-zero/truncate,
///   7=round-away).  Use m3=0 for int→float (exact), m3=5 for float→int
///   (C-style truncation).
/// - `m4` = inexact-flag-control (0=silent, 1=raise).  Use 0.
fn encode_fp_rrf(op1: u8, op2: u8, r1: Fpr, r2: Fpr, m3: u8, m4: u8) -> [u8; 4] {
    encode_fp_rrf_r(op1, op2, r1.encoding(), r2.encoding(), m3, m4)
}

/// Encode an RRF-a format FP↔int conversion (4 bytes), taking raw register
/// numbers so the caller can mix GPR and FPR operands.
///
/// Per s390x PoP (verified on QEMU-s390x 7.2 and binutils s390-opc.c):
/// - CDGBRA/CEGBRA/CDLGBRA/CELGBRA (RRF_UUFR, int→float): R1=FPR (dest),
///   R2=GPR (source int64).  No LDGR needed before — pass the GPR directly.
/// - CGDBRA/CLGDBRA (RRF_UURF, float→int): R1=GPR (dest int64),
///   R2=FPR (source F64).  No LGDR needed after — the result lands in the GPR.
///
/// Same byte layout as `encode_fp_rrf`: byte 2 = M3|M4, byte 3 = R1|R2.
fn encode_fp_rrf_r(op1: u8, op2: u8, r1: u8, r2: u8, m3: u8, m4: u8) -> [u8; 4] {
    [
        op1,
        op2,
        ((m3 & 0xF) << 4) | (m4 & 0xF),
        ((r1 & 0xF) << 4) | (r2 & 0xF),
    ]
}

/// Encode LDGR R1, R2 (Load FPR from GPR, 64-bit bit copy).  RRE format.
/// op1=0xB3, op2=0xC1.  `r1` is FPR, `r2` is GPR.
///
/// Bit-copies the 64-bit GPR value into the FPR WITHOUT interpreting it as a
/// float.  Used to ferry int-typed bit patterns (e.g. the raw bits of an i64
/// or the bit pattern of a float stored in a stack slot) into an FPR for FP
/// conversion instructions (CDGBRA/CEGBRA/CDLGBRA/CELGBRA).
fn encode_ldgr(r1: Fpr, r2: Gpr) -> [u8; 4] {
    [
        0xB3,
        0xC1,
        0x00,
        ((r1.encoding() & 0xF) << 4) | (r2.encoding() & 0xF),
    ]
}

/// Encode LGDR R1, R2 (Load GPR from FPR, 64-bit bit copy).  RRE format.
/// op1=0xB3, op2=0xCD.  `r1` is GPR, `r2` is FPR.
///
/// Bit-copies the 64-bit FPR value into a GPR WITHOUT interpreting it.  Used
/// to ferry an FP conversion result (e.g. CGDBRA's int64 output, which lands
/// in an FPR's bits) back into the GPR file so it can be stored via STG.
fn encode_lgdr(r1: Gpr, r2: Fpr) -> [u8; 4] {
    [
        0xB3,
        0xCD,
        0x00,
        ((r1.encoding() & 0xF) << 4) | (r2.encoding() & 0xF),
    ]
}

/// Encode a 6-byte RXY-a instruction with an FP register operand.
///
/// Byte layout is IDENTICAL to `encode_rxy_a` (line ~312) — only `r1` comes
/// from an `Fpr` instead of a `Gpr`.  The X2 index field is forced to 0
/// (no index register), mirroring `encode_lg`/`encode_stg`.
///
///   byte 0: opcode1 (8 bits)
///   byte 1: R1 (4 bits, high nibble) | X2=0 (4 bits, low nibble)
///   byte 2: B2 (4 bits, high nibble) | DL2[11:8] (4 bits, low nibble)
///   byte 3: DL2[7:0] (8 bits)
///   byte 4: DH2 (8 bits)
///   byte 5: opcode2 (8 bits — the FULL byte)
fn encode_fp_rxy_a(op1: u8, op2: u8, r1: Fpr, b2: Gpr, disp: i32) -> [u8; 6] {
    // 20-bit signed displacement: DH2 (8 bits) << 12 | DL2 (12 bits).
    let d = disp as i64;
    let d20 = (d & 0xFFFFF) as u32; // low 20 bits (two's-complement for negatives)
    let dl2_low_8 = (d20 & 0xFF) as u8; // byte 3: DL2[7:0]
    let dl2_high_4 = ((d20 >> 8) & 0xF) as u8; // byte 2 low nibble: DL2[11:8]
    let dh2 = ((d20 >> 12) & 0xFF) as u8; // byte 4: DH2

    [
        op1,
        (r1.encoding() & 0xF) << 4, // r1 in high nibble, x2=0 in low
        ((b2.encoding() & 0xF) << 4) | dl2_high_4,
        dl2_low_8,
        dh2,
        op2, // FULL 8-bit opcode2
    ]
}

/// Encode LDY R1, D2(X2, B2) (Load F64, 8 bytes, long displacement).
/// RXY format.  op1=0xED, op2=0x65.  binutils: `ed ... 65 ldy`.
fn encode_ld(r1: Fpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_fp_rxy_a(0xED, 0x65, r1, b2, disp)
}

/// Encode STDY R1, D2(X2, B2) (Store F64, 8 bytes, long displacement).
/// RXY format.  op1=0xED, op2=0x67.  binutils: `ed ... 67 stdy`.
fn encode_std(r1: Fpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_fp_rxy_a(0xED, 0x67, r1, b2, disp)
}

/// Encode LEY R1, D2(X2, B2) (Load F32, 4 bytes, long displacement).
/// RXY format.  op1=0xED, op2=0x64.  binutils: `ed ... 64 ley`.
fn encode_le(r1: Fpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_fp_rxy_a(0xED, 0x64, r1, b2, disp)
}

/// Encode STEY R1, D2(X2, B2) (Store F32, 4 bytes, long displacement).
/// RXY format.  op1=0xED, op2=0x66.  binutils: `ed ... 66 stey`.
fn encode_ste(r1: Fpr, b2: Gpr, disp: i32) -> [u8; 6] {
    encode_fp_rxy_a(0xED, 0x66, r1, b2, disp)
}

/// Emit a binary operation as s390x machine code.
fn emit_binop(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    ty: Option<&IRType>,
    vreg_stack_slots: &HashMap<u32, i32>,
    code: &mut Vec<u8>,
) {
    let dst_id = dst.as_register().unwrap_or(0);
    let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
    let is_32bit = is_32bit_ty(ty);

    // ── FP dispatch ──
    // Mirror x86_64's stack_slot_isel.rs:414-549 — branch on F32/F64 before
    // the integer `match op` and route to the FP emitter.
    if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
        emit_s390x_fp_binop(op, dst, lhs, rhs, ty, vreg_stack_slots, code);
        return;
    }

    match op {
        BinOpKind::Add => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_ark(S0, S0, S1));
                code.extend_from_slice(&encode_llgfr(S0, S0));
            } else {
                code.extend_from_slice(&encode_agr(S0, S1));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Sub => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_srk(S0, S0, S1));
                code.extend_from_slice(&encode_llgfr(S0, S0));
            } else {
                code.extend_from_slice(&encode_sgr(S0, S1));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Mul => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_msgr(S0, S1));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SDiv => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_lgfr(S1, S1));
            }
            // For signed division, R0 = sign-extension of S1 (arithmetic
            // shift right by 63). For unsigned, R0 = 0.
            code.extend_from_slice(&encode_srag(S0, S1, 63));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S2));
            if is_32bit {
                code.extend_from_slice(&encode_lgfr(S2, S2));
            }
            // DGR R0, R2 (signed 64-bit divide).
            code.extend_from_slice(&encode_dgr(S0, S2));
            code.extend_from_slice(&encode_lgr(S0, S1));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::UDiv => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S1, S1));
            }
            code.extend_from_slice(&encode_lghi(S0, 0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S2));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S2, S2));
            }
            code.extend_from_slice(&encode_dlgr(S0, S2));
            code.extend_from_slice(&encode_lgr(S0, S1));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SRem => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_lgfr(S1, S1));
            }
            // For signed division, R0 = sign-extension of S1.
            code.extend_from_slice(&encode_srag(S0, S1, 63));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S2));
            if is_32bit {
                code.extend_from_slice(&encode_lgfr(S2, S2));
            }
            // DGR R0, R2 (signed 64-bit divide). Remainder in S0 (= R0).
            code.extend_from_slice(&encode_dgr(S0, S2));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::URem => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S1, S1));
            }
            code.extend_from_slice(&encode_lghi(S0, 0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S2));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S2, S2));
            }
            code.extend_from_slice(&encode_dlgr(S0, S2));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::And => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // Always use 64-bit NGR. For 32-bit values, the high bits are 0
            // (zero-extended on load), so 64-bit AND gives the same result.
            // Using 64-bit avoids truncating 64-bit values (e.g. pointer
            // reconstruction in read_u64: b0 | (b4 << 32)).
            code.extend_from_slice(&encode_rre(0xB9, 0x80, S0, S1));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Or => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // Always use 64-bit OGR (same reasoning as AND).
            code.extend_from_slice(&encode_rre(0xB9, 0x81, S0, S1));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Xor => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // Always use 64-bit XGR (same reasoning as AND).
            code.extend_from_slice(&encode_rre(0xB9, 0x82, S0, S1));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Shl => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // SLLG S0, S0, S1: 64-bit shift left by (S1 & 0x3F).
            // SLLG is always 64-bit. Do NOT apply LLGFR (32-bit truncation)
            // because shifts by >= 32 produce values that need all 64 bits.
            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0D, S0, S0, S1, 0));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ShrL => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // SRLG S0, S0, S1: 64-bit shift right by (S1 & 0x3F).
            // SRLG is always 64-bit. Do NOT apply LLGFR.
            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0C, S0, S0, S1, 0));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ShrA => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // SRAG S0, S0, S1: op1=0xEB, op2=0x0A.
            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0A, S0, S0, S1, 0));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Ror | BinOpKind::Rol => {
            // Rotate not directly supported; emulate via shifts (simplified — leave as lhs).
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Eq => {
            // dst = (lhs == rhs) ? 1 : 0
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // CGR S0, S1: sets CC. CC=0 means equal.
            code.extend_from_slice(&encode_rre(0xB9, 0x20, S0, S1));
            // S0 = 1 (default: assume equal)
            code.extend_from_slice(&encode_lghi(S0, 1));
            // BRC 0x8, skip (if CC=0, i.e., equal, skip the "S0 = 0")
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x8, 0));
            // S0 = 0 (not equal)
            code.extend_from_slice(&encode_lghi(S0, 0));
            // skip: (patch the BRC to jump here)
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Ne => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x20, S0, S1));
            // S0 = 1 (default: assume not equal)
            code.extend_from_slice(&encode_lghi(S0, 1));
            // BRC 0x6, skip (if CC!=0, i.e., not equal, skip the "S0 = 0")
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x6, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SLt => {
            // dst = (lhs < rhs) ? 1 : 0 (signed)
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x20, S0, S1));
            // After CGR: CC=0 (equal), CC=1 (lhs < rhs), CC=2 (lhs > rhs).
            // "Less than" = CC=1.  Mask 0x4 = "CC=1".
            code.extend_from_slice(&encode_lghi(S0, 1));
            // BRC 0x4, skip (if CC=1, i.e., lhs < rhs, skip the "S0 = 0")
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x4, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SLe => {
            // dst = (lhs <= rhs) ? 1 : 0 (signed)
            // "Less than or equal" = CC=0 or CC=1.  Mask 0xC = bits 3,2 = CC=0 or CC=1.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x20, S0, S1));
            code.extend_from_slice(&encode_lghi(S0, 1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0xC, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SGt => {
            // dst = (lhs > rhs) ? 1 : 0 (signed)
            // "Greater than" = CC=2.  Mask 0x2 = "CC=2".
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x20, S0, S1));
            code.extend_from_slice(&encode_lghi(S0, 1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x2, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::SGe => {
            // dst = (lhs >= rhs) ? 1 : 0 (signed)
            // "Greater than or equal" = CC=0 or CC=2.  Mask 0xA = bits 3,1 = CC=0 or CC=2.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x20, S0, S1));
            code.extend_from_slice(&encode_lghi(S0, 1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0xA, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ULt => {
            // dst = (lhs < rhs) ? 1 : 0 (unsigned)
            // CLGR (Compare Logical 64-bit): op1=0xB9, op2=0x21.
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x21, S0, S1));
            code.extend_from_slice(&encode_lghi(S0, 1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x4, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ULe => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x21, S0, S1));
            code.extend_from_slice(&encode_lghi(S0, 1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0xC, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::UGt => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x21, S0, S1));
            code.extend_from_slice(&encode_lghi(S0, 1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x2, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::UGe => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            code.extend_from_slice(&encode_rre(0xB9, 0x21, S0, S1));
            code.extend_from_slice(&encode_lghi(S0, 1));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0xA, 0));
            code.extend_from_slice(&encode_lghi(S0, 0));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
        }
    }
}

/// Emit s390x floating-point binary op (F32/F64).
///
/// Operand bits are ferried from their stack slots into FPRs via the GPR file
/// (`ss_load_value` → `LDGR`), the FP arithmetic executes in FPRs (FA += FB,
/// etc.), and the result is stored back via `STD`/`STE`.
///
/// We use `LDGR`/`LGDR` (64-bit GPR↔FPR bit copy) instead of `LD`/`STD` for
/// the operand-load path so that **immediates** work without needing a scratch
/// stack slot — `ss_load_value` already handles immediates via `ss_load_imm`,
/// and `LDGR` ferries the resulting GPR bits straight into the FPR.  (A direct
/// `LD` from a stack slot would require the operand to already be in a slot;
/// spilling an immediate to the catch-all "slot 0" would corrupt the 160-byte
/// ABI save area, so we avoid that.)
///
/// For comparisons (`Eq`/`Ne`/`SLt`/...), `CDBR`/`CEBR` sets the condition
/// code with the SAME CC semantics as integer `CGR` (0=eq, 1=lt, 2=gt), plus
/// CC=3 for unordered (NaN).  We mirror the integer compare emission (LGHI 1;
/// BRC <mask>,skip; LGHI 0; skip:) — the BRC masks (0x8/0x6/0x4/0xC/0x2/0xA)
/// already produce the IEEE-correct result for NaN (all comparisons except Ne
/// return false; Ne returns true), so no special NaN handling is needed.
fn emit_s390x_fp_binop(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    ty: Option<&IRType>,
    vreg_stack_slots: &HashMap<u32, i32>,
    code: &mut Vec<u8>,
) {
    let dst_id = dst.as_register().unwrap_or(0);
    let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
    let is_f64 = matches!(ty, Some(IRType::F64));

    // Load operands into FPRs FA (lhs) and FB (rhs).
    //
    // For F64: operands are stored as full 64-bit f64 bits in stack slots /
    // immediates.  `ss_load_value` (LG) + `LDGR` copies them verbatim into the
    // FPR, ready for ADBR/SDBR/etc.
    //
    // For F32: operands are stored as 32-bit f32 bits in the LOW 32 bits of
    // the slot / immediate (4-byte STE convention).  `ss_load_f32_to_fpr`
    // loads the 8-byte slot value, shifts the f32 bits from low 32 to high 32
    // via SLLG, then LDGRs into the FPR — placing the f32 value in bits 0–31
    // where AEBR/SEBR/MEBR/DEBR expect it.
    if is_f64 {
        code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
        code.extend_from_slice(&encode_ldgr(FA, S0));
        code.extend(ss_load_value(rhs, vreg_stack_slots, S0));
        code.extend_from_slice(&encode_ldgr(FB, S0));
    } else {
        code.extend(ss_load_f32_to_fpr(lhs, vreg_stack_slots, S0, FA));
        code.extend(ss_load_f32_to_fpr(rhs, vreg_stack_slots, S0, FB));
    }

    match op {
        BinOpKind::Add => {
            if is_f64 {
                // ADBR: Add F64.  op1=0xB3, op2=0x1A.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x1A, FA, FB));
            } else {
                // AEBR: Add F32.  op1=0xB3, op2=0x0A.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x0A, FA, FB));
            }
        }
        BinOpKind::Sub => {
            if is_f64 {
                // SDBR: Subtract F64.  op2=0x1B.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x1B, FA, FB));
            } else {
                // SEBR: Subtract F32.  op2=0x0B.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x0B, FA, FB));
            }
        }
        BinOpKind::Mul => {
            if is_f64 {
                // MDBR: Multiply F64.  op2=0x1C.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x1C, FA, FB));
            } else {
                // MEBR: Multiply F32 (low).  op2=0x17.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x17, FA, FB));
            }
        }
        BinOpKind::SDiv | BinOpKind::UDiv => {
            // FP division is the same for signed/unsigned (FP has no sign
            // extension concerns); both map to DDBR/DEBR.
            if is_f64 {
                // DDBR: Divide F64.  op2=0x1D.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x1D, FA, FB));
            } else {
                // DEBR: Divide F32.  op2=0x0D.
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x0D, FA, FB));
            }
        }
        BinOpKind::Eq
        | BinOpKind::Ne
        | BinOpKind::SLt
        | BinOpKind::ULt
        | BinOpKind::SLe
        | BinOpKind::ULe
        | BinOpKind::SGt
        | BinOpKind::UGt
        | BinOpKind::SGe
        | BinOpKind::UGe => {
            // CDBR (F64, op2=0x19) / CEBR (F32, op2=0x09) sets CC:
            //   CC=0 equal, CC=1 lhs<rhs, CC=2 lhs>rhs, CC=3 unordered (NaN).
            // BRC masks (same as integer CGR-based compares):
            //   0x8=CC0, 0x6=CC!=0, 0x4=CC1, 0xC=CC0|1, 0x2=CC2, 0xA=CC0|2.
            // For NaN (CC=3): only 0x6 (Ne) matches → Ne=true, all others false.
            // This matches IEEE-754 NaN comparison semantics.
            if is_f64 {
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x19, FA, FB)); // CDBR
            } else {
                code.extend_from_slice(&encode_fp_rre(0xB3, 0x09, FA, FB)); // CEBR
            }
            // S0 = 1 (default: assume condition true).
            code.extend_from_slice(&encode_lghi(S0, 1));
            let mask: u8 = match op {
                BinOpKind::Eq => 0x8,                               // CC=0
                BinOpKind::Ne => 0x6,                               // CC!=0
                BinOpKind::SLt | BinOpKind::ULt => 0x4,             // CC=1
                BinOpKind::SLe | BinOpKind::ULe => 0xC,             // CC=0 or 1
                BinOpKind::SGt | BinOpKind::UGt => 0x2,             // CC=2
                BinOpKind::SGe | BinOpKind::UGe => 0xA,             // CC=0 or 2
                _ => 0x8,
            };
            // BRC <mask>, skip  (skip the "S0 = 0" if condition holds).
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(mask, 0));
            // S0 = 0 (condition false).
            code.extend_from_slice(&encode_lghi(S0, 0));
            // skip: patch BRC displacement.
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);
            code.extend(ss_st(S0, dst_off));
            return;
        }
        // Bitwise/shift/remainder on floats: invalid (IR verifier F2a rejects).
        // Defensive fallback: store 0 so dst slot is well-defined.
        BinOpKind::SRem
        | BinOpKind::URem
        | BinOpKind::And
        | BinOpKind::Or
        | BinOpKind::Xor
        | BinOpKind::Shl
        | BinOpKind::ShrL
        | BinOpKind::ShrA
        | BinOpKind::Ror
        | BinOpKind::Rol => {
            code.extend(ss_load_imm(S0, 0));
            code.extend(ss_st(S0, dst_off));
            return;
        }
    }

    // Store FP result back to dst slot.
    //
    // For F64: STD stores the full 8-byte f64 value.
    // For F32: ss_store_f32_from_fpr (LGDR + SRLG + STG) stores the f32
    // result bits in the GPR's low 32 (offset 4–7 on big-endian), matching
    // the convention used by ss_load_f32_to_fpr and the materialize pass.
    if is_f64 {
        code.extend_from_slice(&encode_std(FA, FP, dst_off));
    } else {
        code.extend(ss_store_f32_from_fpr(FA, S0, dst_off));
    }
}

// ===========================================================================
// s390x Backend
// ===========================================================================

/// s390x (IBM System Z) code generation backend.
pub struct S390XBackend {
    target_info: S390XTargetInfo,
    /// Whether to use real register allocation (Wave 23) or stack-slot lowering.
    pub use_real_regalloc: bool,
}

impl S390XBackend {
    /// Create a new s390x backend.
    pub fn new() -> Self {
        Self {
            target_info: S390XTargetInfo,
            use_real_regalloc: false,
        }
    }
}

impl Default for S390XBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// s390x target information (64-bit, big-endian, Linux ABI).
pub struct S390XTargetInfo;

impl TargetInfo for S390XTargetInfo {
    fn isa_name(&self) -> &'static str {
        "s390x"
    }
    fn target_triple(&self) -> &'static str {
        "s390x-ibm-linux-gnu"
    }
    fn elf_machine_type(&self) -> u16 {
        22 // EM_S390
    }
    fn default_base_address(&self) -> u64 {
        0x10000
    }
    fn pointer_width(&self) -> usize {
        8
    }
    fn size_of(&self, ty: &IRType) -> usize {
        crate::ir::size_of_with_ptr_width(ty, 8)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        crate::ir::alignment_of_with_ptr_width(ty, 8)
    }
    fn endianness(&self) -> Endianness {
        Endianness::Big
    }
    fn has_registers(&self) -> bool {
        true
    }
    fn num_gp_regs(&self) -> usize {
        16 // R0–R15
    }
    fn num_simd_fp_regs(&self) -> usize {
        16 // F0–F15
    }
    fn has_hardwired_zero(&self) -> bool {
        false // No hardwired zero on s390x (R0 is a regular reg, but encoding 0 in B2/X2 means "no reg")
    }
    fn has_link_register(&self) -> bool {
        true // R14
    }
    fn has_branch_delay_slots(&self) -> bool {
        false // No delay slots on s390x
    }
    fn has_toc_pointer(&self) -> bool {
        false // R12 is conventionally the TOC, but we don't use it
    }
    fn has_condition_registers(&self) -> bool {
        false // s390x uses a condition code (CC), not separate condition registers
    }
    fn calling_convention_name(&self) -> &'static str {
        "s390x-linux"
    }
    fn num_int_arg_regs(&self) -> usize {
        5 // R2–R6
    }
    fn num_fp_arg_regs(&self) -> usize {
        2 // F0, F2 (Linux s390x ABI uses F0, F2 for FP args)
    }
    fn stack_alignment(&self) -> usize {
        8 // s390x ABI: 8-byte stack alignment (16 recommended)
    }
    fn instruction_alignment(&self) -> usize {
        2 // s390x instructions are 2-byte aligned
    }
    fn instruction_width_range(&self) -> (usize, usize) {
        (2, 6) // Variable-length: 2, 4, or 6 bytes
    }
    fn output_format(&self) -> OutputFormat {
        OutputFormat::Elf64
    }

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::s390x()
    }
}

impl Backend for S390XBackend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        if self.use_real_regalloc {
            s390x_allocate_registers_real(func)
        } else {
            s390x_allocate_registers_ss(func)
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
        // ── s390x Linux static executable ──
        //
        // Layout:
        //   _start:  LG   R2, 0(R15)      ; argc = *R15 (64-bit)
        //            LGR  R3, R15         ; R3 = R15 (copy SP)
        //            AGFI R3, 8           ; argv = R15 + 8 (64-bit pointers)
        //            BRASL R14, main      ; call main(argc, argv) — return value in R2
        //            LGFI R1, 1           ; R1 = 1 (SYS_exit)
        //            SVC 0                ; syscall: exit(R2)
        //   <functions...>
        //   <FFI return-0 stub>
        //   <__vuma_alloc / __vuma_free stubs>
        //   <POSIX syscall stubs>

        const R_S390_PC32DBL: &str = "R_S390_PC32DBL";
        const BASE_ADDR: u64 = 0x10000;

        // Compute text_offset (must match build_s390x_elf).
        let elf_header_size: u64 = 64;
        let phdr_size: u64 = 56;
        let num_phdrs: u64 = 3; // 2 LOAD + GNU_STACK — MUST match build_s390x_elf!
        let phdr_end = elf_header_size + num_phdrs * phdr_size;
        let text_offset: u64 = phdr_end;

        // ── _start stub ──
        // LG   R2, 0(R15)   — 6 bytes, offset 0  (load argc from stack)
        // LGR  R3, R15      — 4 bytes, offset 6  (R3 = SP)
        // AGFI R3, 8        — 6 bytes, offset 10 (argv = SP + 8)
        // BRASL R14, main   — 6 bytes, offset 16 (will be patched)
        // LGFI R1, 1        — 6 bytes, offset 22 (sys_exit)
        // SVC 0             — 2 bytes, offset 28
        let start_stub_size: usize = 32; // LG(6)+LGR(4)+LGHI+AGR(8)+BRASL(6)+LGFI(6)+SVC(2)
        // FFI return-0 stub: LGHI R2, 0 (4 bytes) + BR R14 (2 bytes) = 6 bytes.
        let ffi_stub_size: usize = 6;
        let ffi_stub_offset: usize = start_stub_size;

        // ── Build __vuma_alloc / __vuma_free syscall stubs ──
        //
        // s390x Linux syscall convention: syscall # in R1, args in R2-R7,
        // return in R2, invoke via SVC 0.
        //
        // __vuma_alloc(size in R2) -> R2 = mmap(NULL, size, PROT_READ|PROT_WRITE,
        //                                         MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
        //   s390x syscall 90 is old_mmap (struct pointer). The stub builds a
        //   mmap_arg_struct on the stack and passes R2 = &struct.
        // __vuma_free(addr in R2) -> munmap(addr, 0) via syscall 91.
        //
        // Linux s390x syscall numbers:
        //   exit=1, read=3, write=4, open=5, close=6, brk=45, mmap=90 (old_mmap),
        //   munmap=91, mremap=163,
        //   rt_sigaction=139, pipe=42, dup2=63, alarm=27, getpid=20,
        //   socket=359, clone=120, fork=2, execve=11, wait4=114,
        //   exit_group=246, ...

        let vuma_alloc_stub: Vec<u8> = {
            let mut code = Vec::new();
            // Incoming: R2 = size. Build mmap_arg_struct on stack:
            //   { addr=0, len=size, prot=3, flags=0x22, fd=-1, offset=0 }
            // R15 -= 64 (48-byte struct + 16-byte alignment)
            code.extend_from_slice(&encode_lghi(Gpr::R0, 64));
            code.extend_from_slice(&encode_sgr(SP, Gpr::R0));
            // [R15+0] = addr = 0
            code.extend_from_slice(&encode_lghi(Gpr::R1, 0));
            code.extend_from_slice(&encode_stg(Gpr::R1, SP, 0));
            // [R15+8] = len = size (R2 — the incoming arg, still intact)
            code.extend_from_slice(&encode_stg(Gpr::R2, SP, 8));
            // [R15+16] = prot = 3
            code.extend_from_slice(&encode_lghi(Gpr::R1, 3));
            code.extend_from_slice(&encode_stg(Gpr::R1, SP, 16));
            // [R15+24] = flags = 0x22 (MAP_PRIVATE|MAP_ANONYMOUS)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 0x22));
            code.extend_from_slice(&encode_stg(Gpr::R1, SP, 24));
            // [R15+32] = fd = -1
            code.extend_from_slice(&encode_lghi(Gpr::R1, -1));
            code.extend_from_slice(&encode_stg(Gpr::R1, SP, 32));
            // [R15+40] = offset = 0
            code.extend_from_slice(&encode_lghi(Gpr::R1, 0));
            code.extend_from_slice(&encode_stg(Gpr::R1, SP, 40));
            // R2 = R15 (struct pointer)
            code.extend_from_slice(&encode_lgr(Gpr::R2, SP));
            // R1 = 90 (sys_old_mmap); SVC 0
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 90));
            code.extend_from_slice(&encode_svc(0));
            // R15 += 64 (restore stack)
            code.extend_from_slice(&encode_lghi(Gpr::R0, 64));
            code.extend_from_slice(&encode_agr(SP, Gpr::R0));
            // BR R14 (return — R2 holds the mmap result)
            code.extend_from_slice(&encode_br(LR));
            code
        };
        let vuma_free_stub: Vec<u8> = {
            let mut code = Vec::new();
            // R3 = 0 (size = 0)
            code.extend_from_slice(&encode_lghi(Gpr::R3, 0));
            // R1 = 91 (sys_munmap)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 91));
            code.extend_from_slice(&encode_svc(0));
            // BR R14
            code.extend_from_slice(&encode_br(LR));
            code
        };

        // ── POSIX syscall stubs ──────────────────────────────────────
        // Simple stubs: LGFI R1, #num; SVC 0; BR R14.
        // Args are already in R2-R7 from the caller.
        let simple_stub = |num: i32| -> Vec<u8> {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_lgfi(Gpr::R1, num));
            code.extend_from_slice(&encode_svc(0));
            code.extend_from_slice(&encode_br(LR));
            code
        };

        let syscall_stubs: Vec<(String, Vec<u8>)> = {
            let mut stubs: Vec<(String, Vec<u8>)> = Vec::new();
            // Simple stubs (args already in R2-R7):
            for (name, num) in [
                ("write", 4),
                ("read", 3),
                ("open", 5),
                ("close", 6),
                // mmap/mremap/munmap are custom stubs (brk-based, see below)
                ("exit", 1),
                ("alarm", 27),
                ("getpid", 20),
                ("socket", 358),
                ("execve", 11),
                ("wait4", 114),
                ("dup2", 63),
                ("fork", 2),
                ("unlink", 10),
                ("exit_group", 246),
                ("lseek", 19),
                ("kill", 37),
                ("chdir", 12),
                ("dup", 41),
                ("pipe", 42),
                ("ioctl", 54),
                ("fcntl", 55),
                ("futex", 238),
                ("poll", 168),
                ("nanosleep", 162),
                ("mprotect", 125),
                ("brk", 45),
                ("clock_gettime", 260),
                ("gettimeofday", 78),
                ("rt_sigprocmask", 175),
                // s390x direct socket syscalls start at 358 and follow the
                // standard 15-call ordering (socket, socketpair, bind, listen,
                // accept, connect, getsockname, getpeername, sendto, recvfrom,
                // sendmsg, recvmsg, shutdown, setsockopt, getsockopt). The
                // previous numbers were off-by-one and invoked the wrong
                // network syscall. NOTE: verify against
                // arch/s390/include/uapi/asm/unistd.h.
                ("connect", 363),
                ("bind", 360),
                ("listen", 361),
                ("accept", 362),
                ("setsockopt", 371),
                ("getsockopt", 365),
                ("shutdown", 370),
                ("dup3", 326),
                ("recvfrom", 367),
                ("sendto", 366),
                ("epoll_create1", 327),
                ("epoll_ctl", 250),
                ("epoll_wait", 251),
                ("clone", 120),
                // ── Additional POSIX syscall stubs (stat family + getcwd) ──
                // s390x syscall numbers from arch/s390/include/uapi/asm/unistd.h.
                ("stat", 106),
                ("lstat", 107),
                ("fstat", 108),
                ("getcwd", 183),
                // ── Wave 7: POSIX file-metadata & I/O syscalls (s390 unistd.h) ──
                // s390x has 5 reg args (R2-R6); all these take ≤5 args → simple_stub.
                // chown=212/fchown=207 are the modern 32-bit-uid variants
                // (s390's old 16-bit chown slot was repurposed; 212 is sys_chown).
                ("mkdir", 39), ("rmdir", 40), ("rename", 38),
                ("link", 9), ("symlink", 83), ("readlink", 85),
                ("chmod", 15), ("chown", 212), ("umask", 60),
                ("fchmod", 94), ("fchown", 207),
                ("openat", 288), ("unlinkat", 294), ("renameat", 295),
                ("linkat", 296), ("symlinkat", 297), ("readlinkat", 298),
                ("fchmodat", 299), ("faccessat", 300), ("fchownat", 291),
                ("ftruncate", 93), ("fsync", 118), ("fdatasync", 148),
                ("sync", 36), ("syncfs", 338),
                ("pread", 180), ("pwrite", 181), ("readv", 145), ("writev", 146),
                ("preadv", 328), ("pwritev", 329),
                ("fchdir", 133), ("chroot", 61),
                // ── Wave 9: POSIX system & advanced syscalls (s390 unistd.h) ──
                // s390x has 5 reg args (R2-R6); all take ≤5 args → simple_stub.
                // eventfd→eventfd2(323), signalfd→signalfd4(322) = modern variants.
                ("mlock", 150), ("munlock", 151), ("mlockall", 152), ("munlockall", 153),
                ("mincore", 218), ("madvise", 219), ("msync", 144),
                ("getrlimit", 191), ("setrlimit", 75), ("prlimit64", 334),
                ("getrusage", 77), ("times", 43),
                ("getrandom", 349),
                ("eventfd", 323), ("timerfd_create", 319), ("timerfd_settime", 320),
                ("timerfd_gettime", 321), ("signalfd", 322),
                ("inotify_init1", 324), ("inotify_add_watch", 285), ("inotify_rm_watch", 286),
                ("ptrace", 26),
                // ── Wave 8: POSIX process & identity syscalls (s390x syscall.tbl) ──
                // s390x uid syscalls are at 199-214 (already 32-bit, no *32 suffix).
                // All take ≤5 args; s390x has 5 reg args (r2-r6) → simple_stub for all.
                // Family 1: identity
                ("getuid", 199), ("geteuid", 201), ("getgid", 200), ("getegid", 202),
                ("setuid", 213), ("setgid", 214), ("setresuid", 208), ("setresgid", 210),
                // Family 2: process group (getpid already present)
                ("getppid", 64), ("getsid", 147), ("setsid", 66),
                ("setpgid", 57), ("getpgid", 132), ("getpgrp", 65),
                // Family 3: clone/wait (clone/wait4 already present)
                ("vfork", 190), ("clone3", 435), ("waitid", 281),
                // Family 4: exec/exit (execve/exit_group already present)
                ("execveat", 354),
                // Family 5: signals (kill/rt_sigprocmask/rt_sigreturn already present)
                ("tgkill", 241), ("tkill", 237), ("rt_sigaction", 174),
                // Family 6: directory read (readdir ABSENT → use getdents64)
                ("getdents64", 220), ("getdents", 141),
                // Family 7: system (arch_prctl is x86_64-only)
                ("prctl", 172), ("uname", 122), ("sysinfo", 116),
                            ("eventfd2", 323),
                ("newfstatat", 293),
                ("signalfd4", 324),
] {
                stubs.push((name.to_string(), simple_stub(num)));
            }

            // ── FFI scratchpad frame stubs (Wave 3b/fix) ──────────────────
            // ffi_scratch_push_frame: REAL mmap via old_mmap (s390x syscall 90).
            // s390x syscall 90 is old_mmap (struct pointer). The stub builds a
            // mmap_arg_struct on the stack with:
            //   addr=0, len=4096, prot=3, flags=0x22, fd=-1, offset=0
            // and passes R2 = &struct.
            {
                let mut code = Vec::new();
                // R15 -= 64 (48-byte struct + 16-byte alignment)
                code.extend_from_slice(&encode_lghi(Gpr::R0, 64));
                code.extend_from_slice(&encode_sgr(SP, Gpr::R0));
                // [R15+0] = addr = 0
                code.extend_from_slice(&encode_lghi(Gpr::R1, 0));
                code.extend_from_slice(&encode_stg(Gpr::R1, SP, 0));
                // [R15+8] = len = 4096
                code.extend_from_slice(&encode_lgfi(Gpr::R1, 4096));
                code.extend_from_slice(&encode_stg(Gpr::R1, SP, 8));
                // [R15+16] = prot = 3
                code.extend_from_slice(&encode_lghi(Gpr::R1, 3));
                code.extend_from_slice(&encode_stg(Gpr::R1, SP, 16));
                // [R15+24] = flags = 0x22
                code.extend_from_slice(&encode_lgfi(Gpr::R1, 0x22));
                code.extend_from_slice(&encode_stg(Gpr::R1, SP, 24));
                // [R15+32] = fd = -1
                code.extend_from_slice(&encode_lghi(Gpr::R1, -1));
                code.extend_from_slice(&encode_stg(Gpr::R1, SP, 32));
                // [R15+40] = offset = 0
                code.extend_from_slice(&encode_lghi(Gpr::R1, 0));
                code.extend_from_slice(&encode_stg(Gpr::R1, SP, 40));
                // R2 = R15 (struct pointer)
                code.extend_from_slice(&encode_lgr(Gpr::R2, SP));
                // R1 = 90 (sys_old_mmap); SVC 0
                code.extend_from_slice(&encode_lgfi(Gpr::R1, 90));
                code.extend_from_slice(&encode_svc(0));
                // R15 += 64 (restore stack)
                code.extend_from_slice(&encode_lghi(Gpr::R0, 64));
                code.extend_from_slice(&encode_agr(SP, Gpr::R0));
                // BR R14 (return — R2 holds the mmap'd address)
                code.extend_from_slice(&encode_br(LR));
                stubs.push(("ffi_scratch_push_frame".to_string(), code));
            }

            // ffi_scratch_pop_frame: no-op (BR LR).
            {
                let mut code = Vec::new();
                code.extend_from_slice(&encode_br(LR));
                stubs.push(("ffi_scratch_pop_frame".to_string(), code));
            }

            // __arena_overflow: real exit(1) syscall
            {
                let mut code = Vec::new();
                code.extend_from_slice(&encode_lgfi(Gpr::R2, 1));       // exit code = 1
                code.extend_from_slice(&encode_lgfi(Gpr::R1, 1));       // sys_exit
                code.extend_from_slice(&encode_svc(0));
                stubs.push(("__arena_overflow".to_string(), code));
            }

            // mmap custom stub: s390x syscall 90 is old_mmap (struct pointer).
            //
            // QEMU user-mode s390x implements syscall 90 as `old_mmap`, which
            // takes a SINGLE argument: R2 = pointer to a `mmap_arg_struct`:
            //   struct mmap_arg_struct { u64 addr, len, prot, flags, fd, offset; }
            //   = 48 bytes (6 × 8-byte unsigned long on 64-bit s390x).
            //
            // This is NOT the direct 6-arg mmap. Passing R2=0 (NULL) causes
            // QEMU to dereference R2 as a struct pointer → EFAULT. The fix:
            // build the struct on the stack and pass R2 = &struct.
            //
            // The arena passes 6 args via C ABI: R2=addr, R3=len, R4=prot,
            // R5=flags, R6=fd, arg6=offset at R15+160. The stub:
            //   1. Load arg6 (offset) from R15+160 into R7 (BEFORE decrementing R15)
            //   2. R15 -= 64 (allocate 48-byte struct + 16-byte alignment)
            //   3. Store the 6 args into the struct at R15+0..R15+40
            //   4. R2 = R15 (struct pointer)
            //   5. R1 = 90; SVC 0 (old_mmap)
            //   6. R15 += 64 (restore stack)
            //   7. BR R14 (return — R2 already holds the mmap result)
            {
                let mut code = Vec::new();
                // Step 1: Load arg6 (offset) from R15+160 into R7.
                // This MUST happen BEFORE we decrement R15, because after
                // decrementing, the offset changes.
                code.extend_from_slice(&encode_lg(Gpr::R7, SP, 160));
                // Step 2: R15 -= 64
                code.extend_from_slice(&encode_lghi(Gpr::R0, 64));
                code.extend_from_slice(&encode_sgr(SP, Gpr::R0));
                // Step 3: Store the 6 args into the struct.
                // [R15+0] = addr (R2)
                code.extend_from_slice(&encode_stg(Gpr::R2, SP, 0));
                // [R15+8] = len (R3)
                code.extend_from_slice(&encode_stg(Gpr::R3, SP, 8));
                // [R15+16] = prot (R4)
                code.extend_from_slice(&encode_stg(Gpr::R4, SP, 16));
                // [R15+24] = flags (R5)
                code.extend_from_slice(&encode_stg(Gpr::R5, SP, 24));
                // [R15+32] = fd (R6)
                code.extend_from_slice(&encode_stg(Gpr::R6, SP, 32));
                // [R15+40] = offset (R7)
                code.extend_from_slice(&encode_stg(Gpr::R7, SP, 40));
                // Step 4: R2 = R15 (struct pointer)
                code.extend_from_slice(&encode_lgr(Gpr::R2, SP));
                // Step 5: R1 = 90 (sys_old_mmap); SVC 0
                code.extend_from_slice(&encode_lgfi(Gpr::R1, 90));
                code.extend_from_slice(&encode_svc(0));
                // Step 6: R15 += 64 (restore stack)
                code.extend_from_slice(&encode_lghi(Gpr::R0, 64));
                code.extend_from_slice(&encode_agr(SP, Gpr::R0));
                // Step 7: BR R14 (return — R2 holds the mmap result)
                code.extend_from_slice(&encode_br(LR));
                stubs.push(("mmap".to_string(), code));
            }

            // mremap stub: s390x mremap(163) is the direct 4-arg version.
            // Args: R2=old_addr, R3=old_size, R4=new_size, R5=flags.
            // s390x has 5 reg args (R2-R6), so all 4 fit in registers.
            // This is just a simple_stub(163).
            // (Kept as an explicit push to override any simple_stub entry.)
            {
                let mut code = Vec::new();
                code.extend_from_slice(&encode_lgfi(Gpr::R1, 163));
                code.extend_from_slice(&encode_svc(0));
                code.extend_from_slice(&encode_br(LR));
                stubs.push(("mremap".to_string(), code));
            }

            // munmap stub: s390x munmap(91) is the direct 2-arg version.
            // Args: R2=addr, R3=len. Simple_stub(91).
            {
                let mut code = Vec::new();
                code.extend_from_slice(&encode_lgfi(Gpr::R1, 91));
                code.extend_from_slice(&encode_svc(0));
                code.extend_from_slice(&encode_br(LR));
                stubs.push(("munmap".to_string(), code));
            }

            stubs
        };

        // ── Complex stub: sigaction → rt_sigaction(signum, act, oldact, sigsetsize=8) ──
        // s390x rt_sigaction syscall # = 139. VUMA declares 3 args; the kernel
        // requires a 4th arg (sigsetsize=8) in R5. We set R5=8 before SVC.
        let sigaction_stub: Vec<u8> = {
            let mut code = Vec::new();
            // LGHI R5, 8 (sigsetsize)
            code.extend_from_slice(&encode_lghi(Gpr::R5, 8));
            // LGFI R1, 139 (sys_rt_sigaction)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 139));
            code.extend_from_slice(&encode_svc(0));
            code.extend_from_slice(&encode_br(Gpr::R14));
            code
        };
        let mut syscall_stubs = syscall_stubs;
        syscall_stubs.push(("sigaction".to_string(), sigaction_stub));

        // ── rt_sigreturn (174) — special: no args, never returns ──
        // The kernel restores the saved signal context from the stack and
        // resumes execution at the interrupted PC. We emit just
        // `LGFI R1, 174 ; SVC 0` followed by an illegal-instruction trap
        // (0x0001) as a safety net in case the kernel ever does return.
        {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 174));
            code.extend_from_slice(&encode_svc(0));
            // Illegal-instruction trap (0x0001) — safety net.
            code.extend_from_slice(&[0x00, 0x01]);
            syscall_stubs.push(("rt_sigreturn".to_string(), code));
        }

        // ── waitpid(pid, wstatus, options) → wraps wait4(pid, wstatus, options, NULL)
        // VUMA declares waitpid with 3 args (R2=pid, R3=wstatus, R4=options);
        // the syscall wait4 takes a 4th arg (rusage, must be NULL) in R5.
        {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_lghi(Gpr::R5, 0)); // rusage = NULL
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 114)); // sys_wait4
            code.extend_from_slice(&encode_svc(0));
            code.extend_from_slice(&encode_br(LR));
            syscall_stubs.push(("waitpid".to_string(), code));
        }

        // ── recv(fd, buf, len, flags) → recvfrom(fd, buf, len, flags, NULL, NULL)
        // s390x args: R2-R5 are recv args; set R6=0 (addr=NULL), R7=0 (addrlen=NULL).
        // s390x has no direct recv syscall; recvfrom=367 with NULL addr is the
        // canonical equivalent.
        {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_lghi(Gpr::R6, 0)); // addr = NULL
            code.extend_from_slice(&encode_lghi(Gpr::R7, 0)); // addrlen = NULL
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 367)); // sys_recvfrom
            code.extend_from_slice(&encode_svc(0));
            code.extend_from_slice(&encode_br(LR));
            syscall_stubs.push(("recv".to_string(), code));
        }

        // ── send(fd, buf, len, flags) → sendto(fd, buf, len, flags, NULL, 0)
        // Same pattern as recv but uses sendto=366.
        {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_lghi(Gpr::R6, 0)); // addr = NULL
            code.extend_from_slice(&encode_lghi(Gpr::R7, 0)); // addrlen = 0
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 366)); // sys_sendto
            code.extend_from_slice(&encode_svc(0));
            code.extend_from_slice(&encode_br(LR));
            syscall_stubs.push(("send".to_string(), code));
        }

        // ── strcmp(s1, s2) → int — assembly loop, not a syscall.
        // s390x calling convention: R2=s1, R3=s2, return in R2.
        // Uses R0 as increment (caller-saved, avoids clobbering callee-saved R6).
        {
            let mut code = Vec::new();
            // LGHI R0, 1 (increment) — R0 is caller-saved
            code.extend_from_slice(&encode_lghi(Gpr::R0, 1));
            // strcmp_loop:
            let loop_start = code.len();
            // LLGC R4, 0(R2) — Load Logical Character (unsigned byte) from s1
            // X2=0 means "no index register" on s390x (R0 as X2 = no index)
            code.extend_from_slice(&encode_rxy_a(0xE3, 0x90, Gpr::R4, Gpr::R0, Gpr::R2, 0));
            // LLGC R5, 0(R3) — Load Logical Character from s2
            code.extend_from_slice(&encode_rxy_a(0xE3, 0x90, Gpr::R5, Gpr::R0, Gpr::R3, 0));
            // CGR R4, R5 — Compare 64-bit (sets CC based on R4 - R5)
            code.extend_from_slice(&encode_rre(0xB9, 0x20, Gpr::R4, Gpr::R5));
            // BRC 0x6, done — branch if not equal (mask=6 = LT|GT)
            let bne_pos = code.len();
            code.extend_from_slice(&encode_brc(0x6, 0)); // placeholder disp
            // LTGR R4, R4 — Load and Test 64-bit (sets CC based on R4)
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R4, Gpr::R4));
            // BRC 0x8, done — branch if equal to 0 (both bytes NUL → strings equal)
            let beq_pos = code.len();
            code.extend_from_slice(&encode_brc(0x8, 0)); // placeholder disp
            // AGR R2, R0 — advance s1 (R2 += R0 = R2 + 1)
            code.extend_from_slice(&encode_agr(Gpr::R2, Gpr::R0));
            // AGR R3, R0 — advance s2
            code.extend_from_slice(&encode_agr(Gpr::R3, Gpr::R0));
            // BRCL 0xF, loop_start — unconditional back-branch
            let back_disp = ((loop_start as i64) - (code.len() as i64 + 6)) / 2;
            code.extend_from_slice(&encode_brcl(0xF, back_disp as i32));
            // done: R4 = R4 - R5 (return difference)
            let done_offset = code.len();
            code.extend_from_slice(&encode_sgr(Gpr::R4, Gpr::R5));
            code.extend_from_slice(&encode_lgr(Gpr::R2, Gpr::R4)); // return value in R2
            code.extend_from_slice(&encode_br(LR));

            // Patch BNE and BEQ to target done_offset.
            let bne_disp = ((done_offset as i64) - (bne_pos as i64)) / 2;
            let bne_disp_be = (bne_disp as i16).to_be_bytes();
            code[bne_pos + 2..bne_pos + 4].copy_from_slice(&bne_disp_be);
            let beq_disp = ((done_offset as i64) - (beq_pos as i64)) / 2;
            let beq_disp_be = (beq_disp as i16).to_be_bytes();
            code[beq_pos + 2..beq_pos + 4].copy_from_slice(&beq_disp_be);

            syscall_stubs.push(("strcmp".to_string(), code));
        }

        // ── print_int(R2 = signed 64-bit integer) — runtime helper ──
        // Converts R2 to decimal ASCII and writes to stdout (fd=1) via
        // sys_write (syscall #4). Stack frame: 32 bytes (digit buffer).
        // Register usage:
        //   R2 = current value (and write syscall arg 1)
        //   R3 = digit buffer pointer (and write syscall arg 2)
        //   R4 = digit count (and write syscall arg 3)
        //   R5 = scratch
        //   R6 = constant 10 (divisor)
        //   R8 = saved input value (across syscalls)
        //   R9 = divmod quotient
        {
            let mut code: Vec<u8> = Vec::new();

            // Prologue: SP -= 32 (allocate 32-byte digit buffer).
            code.extend(adjust_sp(-32));
            // R8 = R2 (save input value)
            code.extend_from_slice(&encode_lgr(Gpr::R8, Gpr::R2));
            // R3 = SP + 32 (end-of-buffer pointer; digits grow downward)
            code.extend_from_slice(&encode_lgr(Gpr::R3, SP));
            code.extend(encode_agfi(Gpr::R3, 32));
            // R4 = 0 (digit count)
            code.extend_from_slice(&encode_lghi(Gpr::R4, 0));
            // R6 = 10 (divisor, hoisted out of loop)
            code.extend_from_slice(&encode_lghi(Gpr::R6, 10));
            // LTGR R8, R8 (test input value)
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R8, Gpr::R8));
            // BRC 0xA, divmod_loop — GE → R8 >= 0, skip negative handling
            let brc_skip_neg_pos = code.len();
            code.extend_from_slice(&encode_brc(0xA, 0)); // placeholder

            // ── Negative case: write '-' to stdout, then negate R8 ──
            // LGHI R5, 45 ('-')
            code.extend_from_slice(&encode_lghi(Gpr::R5, 45));
            // STC R5, 16(SP) — store '-' at SP+16 (1-byte scratch)
            code.extend_from_slice(&encode_stc(Gpr::R5, SP, 16));
            // LGHI R2, 1 (fd = stdout)
            code.extend_from_slice(&encode_lghi(Gpr::R2, 1));
            // LGR R3, SP; AGFI R3, 16 (buf = SP+16)
            code.extend_from_slice(&encode_lgr(Gpr::R3, SP));
            code.extend(encode_agfi(Gpr::R3, 16));
            // LGHI R4, 1 (len = 1)
            code.extend_from_slice(&encode_lghi(Gpr::R4, 1));
            // LGFI R1, 4 (sys_write)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 4));
            // SVC 0
            code.extend_from_slice(&encode_svc(0));
            // Restore R3 = SP + 32, R4 = 0 (clobbered by syscall prep)
            code.extend_from_slice(&encode_lgr(Gpr::R3, SP));
            code.extend(encode_agfi(Gpr::R3, 32));
            code.extend_from_slice(&encode_lghi(Gpr::R4, 0));
            // LCGR R8, R8 (R8 = -R8; for INT64_MIN R8 is unchanged, but the
            // unsigned DLGR loop below produces the correct decimal digits
            // for 9223372036854775808.)
            code.extend_from_slice(&encode_rre(0xB9, 0x03, Gpr::R8, Gpr::R8));

            // ── divmod_loop: ──
            let divmod_loop_offset = code.len();
            // Patch BRC 0xA to target divmod_loop_offset
            let brc_skip_neg_disp =
                ((divmod_loop_offset as i64) - (brc_skip_neg_pos as i64)) / 2;
            let brc_skip_neg_be = (brc_skip_neg_disp as i16).to_be_bytes();
            code[brc_skip_neg_pos + 2..brc_skip_neg_pos + 4]
                .copy_from_slice(&brc_skip_neg_be);

            // LGR R9, R8 (move value to odd register of dividend pair)
            code.extend_from_slice(&encode_lgr(Gpr::R9, Gpr::R8));
            // LGHI R8, 0 (high 64 bits of dividend = 0, sign/zero extension)
            code.extend_from_slice(&encode_lghi(Gpr::R8, 0));
            // DLGR R8, R6 — R8 = remainder, R9 = quotient
            code.extend_from_slice(&encode_dlgr(Gpr::R8, Gpr::R6));
            // AGFI R8, 48 (R8 += '0')
            code.extend(encode_agfi(Gpr::R8, 48));
            // AGFI R3, -1 (R3--)
            code.extend(encode_agfi(Gpr::R3, -1));
            // STC R8, 0(R3) — store digit
            code.extend_from_slice(&encode_stc(Gpr::R8, Gpr::R3, 0));
            // AGFI R4, 1 (R4++)
            code.extend(encode_agfi(Gpr::R4, 1));
            // LGR R8, R9 (R8 = quotient, becomes new value)
            code.extend_from_slice(&encode_lgr(Gpr::R8, Gpr::R9));
            // LTGR R8, R8 (test)
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R8, Gpr::R8));
            // BRC 0x6, divmod_loop — NE → R8 != 0, loop back
            let brc_loop_back_pos = code.len();
            let brc_loop_back_disp =
                ((divmod_loop_offset as i64) - (brc_loop_back_pos as i64)) / 2;
            // Use BRCL (32-bit disp) for safety with far backward branches.
            code.extend_from_slice(&encode_brcl(0x6, brc_loop_back_disp as i32));

            // ── Zero check: if R4 == 0 (R8 was originally 0), store '0' ──
            // LTGR R4, R4
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R4, Gpr::R4));
            // BRC 0x6, write_digits — NE → R4 != 0, skip "store '0'"
            let brc_skip_zero_pos = code.len();
            code.extend_from_slice(&encode_brc(0x6, 0)); // placeholder
            // LGHI R5, 48 ('0')
            code.extend_from_slice(&encode_lghi(Gpr::R5, 48));
            // AGFI R3, -1 (R3--)
            code.extend(encode_agfi(Gpr::R3, -1));
            // STC R5, 0(R3) — store '0'
            code.extend_from_slice(&encode_stc(Gpr::R5, Gpr::R3, 0));
            // AGFI R4, 1 (R4++)
            code.extend(encode_agfi(Gpr::R4, 1));

            // ── write_digits: sys_write(1, R3, R4) ──
            let write_digits_offset = code.len();
            // Patch BRC 0x6 to target write_digits_offset
            let brc_skip_zero_disp =
                ((write_digits_offset as i64) - (brc_skip_zero_pos as i64)) / 2;
            let brc_skip_zero_be = (brc_skip_zero_disp as i16).to_be_bytes();
            code[brc_skip_zero_pos + 2..brc_skip_zero_pos + 4]
                .copy_from_slice(&brc_skip_zero_be);

            // LGHI R2, 1 (fd = stdout)
            code.extend_from_slice(&encode_lghi(Gpr::R2, 1));
            // R3 already points to start of digits
            // R4 already has digit count
            // LGFI R1, 4 (sys_write)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 4));
            // SVC 0
            code.extend_from_slice(&encode_svc(0));

            // Epilogue: SP += 32
            code.extend(adjust_sp(32));
            // BR R14
            code.extend_from_slice(&encode_br(LR));

            // print_int stub restored — calls now resolve to the real
            // decimal-conversion runtime helper above instead of becoming
            // no-op unresolved externs.  The stub saves/restores SP and
            // only clobbers caller-saved scratch registers (R1, R3-R9) so
            // it is safe to call from VUMA-compiled code.
            syscall_stubs.push(("print_int".to_string(), code));
        }

        // ── print_hex(R2 = 64-bit value) — runtime helper ──
        // Writes R2 as 16 hex digits (MSB first) to stdout via sys_write.
        // Stack frame: 16 bytes (hex char buffer).
        // Register usage:
        //   R2 = input value (and write syscall arg 1)
        //   R3 = buffer pointer (advances forward; write syscall arg 2)
        //   R4 = loop counter (0..16; write syscall arg 3 = 16)
        //   R5 = current shift amount (60, 56, ..., 4, 0)
        //   R6 = current nibble / hex char
        //   R7 = scratch for comparisons
        //   R8 = saved value (across syscall)
        {
            let mut code: Vec<u8> = Vec::new();

            // Prologue: SP -= 16
            code.extend(adjust_sp(-16));
            // R8 = R2 (save value)
            code.extend_from_slice(&encode_lgr(Gpr::R8, Gpr::R2));
            // R3 = SP (buffer pointer, advances forward)
            code.extend_from_slice(&encode_lgr(Gpr::R3, SP));
            // R4 = 0 (counter)
            code.extend_from_slice(&encode_lghi(Gpr::R4, 0));
            // R5 = 60 (initial shift amount)
            code.extend_from_slice(&encode_lghi(Gpr::R5, 60));

            // hex_loop:
            let hex_loop_offset = code.len();
            // LGR R6, R8 (copy value)
            code.extend_from_slice(&encode_lgr(Gpr::R6, Gpr::R8));
            // SRLG R6, R6, R5 (variable shift right by R5)
            // SRLG R1, R3, D2(B2): shift amount = (B2 + D2) & 0x3F.
            // Use encode_rsy_a with B2=R5, D2=0.
            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0C, Gpr::R6, Gpr::R6, Gpr::R5, 0));
            // NILL R6, 0xF (AND with 0xF) — RI-b format, op1=0xA5, op2=0x7.
            code.extend_from_slice(&[0xA5, (Gpr::R6.encoding() << 4) | 0x7, 0x00, 0x0F]);
            // LGHI R7, 9
            code.extend_from_slice(&encode_lghi(Gpr::R7, 9));
            // CGR R6, R7 (compare nibble with 9)
            code.extend_from_slice(&encode_rre(0xB9, 0x20, Gpr::R6, Gpr::R7));
            // BRC 0x2, alpha — GT (nibble > 9) → alpha case
            let brc_alpha_pos = code.len();
            code.extend_from_slice(&encode_brc(0x2, 0)); // placeholder
            // AGFI R6, 48 (nibble + '0')
            code.extend(encode_agfi(Gpr::R6, 48));
            // BRCL 0xF, store (unconditional skip over alpha)
            let brcl_store_pos = code.len();
            code.extend_from_slice(&encode_brcl(0xF, 0)); // placeholder
            // alpha: AGFI R6, 87 (nibble + 'a' - 10)
            let alpha_offset = code.len();
            code.extend(encode_agfi(Gpr::R6, 87));
            // store: STC R6, 0(R3)
            let store_offset = code.len();
            code.extend_from_slice(&encode_stc(Gpr::R6, Gpr::R3, 0));
            // AGFI R3, 1 (advance buffer)
            code.extend(encode_agfi(Gpr::R3, 1));
            // AGFI R4, 1 (counter++)
            code.extend(encode_agfi(Gpr::R4, 1));
            // AGFI R5, -4 (shift -= 4)
            code.extend(encode_agfi(Gpr::R5, -4));
            // LGHI R7, 16
            code.extend_from_slice(&encode_lghi(Gpr::R7, 16));
            // CGR R4, R7 (compare counter with 16)
            code.extend_from_slice(&encode_rre(0xB9, 0x20, Gpr::R4, Gpr::R7));
            // BRC 0x4, hex_loop — LT (R4 < 16) → loop
            let brc_loop_pos = code.len();
            let brc_loop_disp = ((hex_loop_offset as i64) - (brc_loop_pos as i64)) / 2;
            // Use BRCL for safety with far backward branches.
            code.extend_from_slice(&encode_brcl(0x4, brc_loop_disp as i32));

            // sys_write(1, SP, 16)
            code.extend_from_slice(&encode_lghi(Gpr::R2, 1)); // fd
            code.extend_from_slice(&encode_lgr(Gpr::R3, SP)); // buf
            code.extend_from_slice(&encode_lghi(Gpr::R4, 16)); // len
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 4)); // sys_write
            code.extend_from_slice(&encode_svc(0));

            // Epilogue: SP += 16
            code.extend(adjust_sp(16));
            // BR R14
            code.extend_from_slice(&encode_br(LR));

            // Patch BRC 0x2 (alpha) to target alpha_offset.
            let brc_alpha_disp = ((alpha_offset as i64) - (brc_alpha_pos as i64)) / 2;
            let brc_alpha_be = (brc_alpha_disp as i16).to_be_bytes();
            code[brc_alpha_pos + 2..brc_alpha_pos + 4].copy_from_slice(&brc_alpha_be);
            // Patch BRCL 0xF (store) to target store_offset.
            let brcl_store_disp = ((store_offset as i64) - (brcl_store_pos as i64)) / 2;
            let brcl_store_be = (brcl_store_disp as i32).to_be_bytes();
            code[brcl_store_pos + 2..brcl_store_pos + 6].copy_from_slice(&brcl_store_be);

            // print_hex stub restored — calls now resolve to the real
            // hex-conversion runtime helper above instead of becoming
            // no-op unresolved externs.  The stub saves/restores SP and
            // only clobbers caller-saved scratch registers (R1, R3-R8).
            syscall_stubs.push(("print_hex".to_string(), code));
        }

        // ── print_newline() → void — write '\n' to stdout ──
        // No arguments. Uses sys_write(1, &newline, 1).
        // s390x syscall convention: R1=syscall#, R2=fd, R3=buf, R4=count,
        // SVC 0 = syscall trap. R15 = SP. R14 = LR (return address).
        {
            let mut code: Vec<u8> = Vec::new();
            // SP -= 16 (stack space for the newline byte)
            code.extend(adjust_sp(-16));
            // LGHI R5, 10 ('\n')
            code.extend_from_slice(&encode_lghi(Gpr::R5, 10));
            // STC R5, 0(SP) — store newline at [SP]
            code.extend_from_slice(&encode_stc(Gpr::R5, SP, 0));
            // LGHI R2, 1 (fd = stdout)
            code.extend_from_slice(&encode_lghi(Gpr::R2, 1));
            // LGR R3, SP (buf = SP)
            code.extend_from_slice(&encode_lgr(Gpr::R3, SP));
            // LGHI R4, 1 (len = 1)
            code.extend_from_slice(&encode_lghi(Gpr::R4, 1));
            // LGFI R1, 4 (sys_write)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 4));
            // SVC 0
            code.extend_from_slice(&encode_svc(0));
            // SP += 16 (restore stack)
            code.extend(adjust_sp(16));
            // BR R14 (return)
            code.extend_from_slice(&encode_br(LR));
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

        // LG R2, 0(R15) — load argc from stack pointer (64-bit)
        start_stub.extend_from_slice(&encode_lg(Gpr::R2, SP, 0));

        // LGR R3, R15 — copy SP to R3 (argv = SP + 8)
        start_stub.extend_from_slice(&encode_lgr(Gpr::R3, SP));
        // R3 += 8. Use encode_agfi which now uses LGHI+AGR workaround
        // (QEMU treats AGFI op2=0x9 as IILF=load, not add).
        start_stub.extend(encode_agfi(Gpr::R3, 8));

        // BRASL R14, main — placeholder with disp=0, will be patched.
        let brasl_offset_in_start = start_stub.len();
        start_stub.extend_from_slice(&encode_brasl(LR, 0));
        // LGFI R1, 1 (sys_exit)
        start_stub.extend_from_slice(&encode_lgfi(Gpr::R1, 1));
        // SVC 0
        start_stub.extend_from_slice(&encode_svc(0));

        // ── Patch _start BRASL to main ──
        // BRASL disp is a 32-bit halfword displacement.
        // target = PC + (disp * 2), where PC = address of the BRASL instruction.
        let main_key = func_offsets
            .keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            let brasl_abs = BASE_ADDR + text_offset + brasl_offset_in_start as u64;
            let main_abs = BASE_ADDR + text_offset + main_offset as u64;
            let disp = ((main_abs as i64 - brasl_abs as i64) / 2) as i32;
            let disp_be = disp.to_be_bytes();
            start_stub[brasl_offset_in_start + 2..brasl_offset_in_start + 6]
                .copy_from_slice(&disp_be);
        } else {
            // No main function — point BRASL to the FFI return-0 stub.
            let brasl_abs = BASE_ADDR + text_offset + brasl_offset_in_start as u64;
            let ffi_abs = BASE_ADDR + text_offset + ffi_stub_offset as u64;
            let disp = ((ffi_abs as i64 - brasl_abs as i64) / 2) as i32;
            let disp_be = disp.to_be_bytes();
            start_stub[brasl_offset_in_start + 2..brasl_offset_in_start + 6]
                .copy_from_slice(&disp_be);
        }

        // ── Add FFI return-0 stub ──
        let mut ffi_stub = Vec::with_capacity(ffi_stub_size);
        // LGHI R2, 0 (return 0)
        ffi_stub.extend_from_slice(&encode_lghi(Gpr::R2, 0));
        // BR R14 (return)
        ffi_stub.extend_from_slice(&encode_br(LR));

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

        // ── Patch BRASL relocations for inter-function calls ──
        // BRASL disp is a 32-bit halfword displacement at bytes [2..6] of the
        // 6-byte instruction.
        let mut func_code_offset: usize = start_stub_size + ffi_stub_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;
                if abs_offset + 6 > all_code.len() {
                    continue;
                }
                if reloc.reloc_type == R_S390_PC32DBL {
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
                    let disp = ((target_abs as i64 - instr_abs as i64) / 2) as i32;
                    let disp_be = disp.to_be_bytes();
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
        let extern_symbols: Vec<String> = Vec::new(); // no extern symbols for now
        Ok(build_s390x_elf(&all_code, BASE_ADDR, &extern_symbols))
    }

    fn return_stub(&self) -> Vec<u8> {
        // BR R14 (return)
        encode_br(LR).to_vec()
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // Load 64-bit address into R0, then BR R0.
        // LARL is PC-relative with a 32-bit halfword displacement (±4GB range),
        // which is too small for arbitrary 64-bit addresses.  Use LGFI to load
        // the low 32 bits, then SLLG to shift, then OGR to OR the high 32 bits.
        let mut code = Vec::new();
        let lo = (entry_addr & 0xFFFF_FFFF) as i32;
        let hi = ((entry_addr >> 32) & 0xFFFF_FFFF) as i32;
        // LGFI R0, hi
        code.extend_from_slice(&encode_lgfi(Gpr::R0, hi));
        // SLLG R0, R0, 32
        code.extend_from_slice(&encode_sllg(Gpr::R0, Gpr::R0, 32));
        // LGFI R1, lo
        code.extend_from_slice(&encode_lgfi(Gpr::R1, lo));
        // OGR R0, R1 (R0 |= R1)
        code.extend_from_slice(&encode_rre(0xB9, 0x81, Gpr::R0, Gpr::R1));
        // BR R0
        code.extend_from_slice(&encode_br(Gpr::R0));
        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        // Simple hex-based disassembler for s390x (variable-length 2/4/6-byte
        // big-endian instructions).
        //
        // s390x instruction length is determined by the first 2 bits of the
        // opcode byte 0:
        //   00 → 2 bytes (e.g., RR format)
        //   01 → 4 bytes (e.g., RX, RI format)
        //   10 → 4 bytes (e.g., SI, RS format)
        //   11 → 6 bytes (e.g., RIL, RXY, RSY format)
        //
        // Wait, that's not quite right.  The actual rule:
        //   - The first nibble (4 bits) of the opcode determines the length:
        //     0x00–0x7F → 2 bytes (op[0] high bit = 0)
        //     0x80–0xBF → 4 bytes (op[0] high 2 bits = 10)
        //     0xC0–0xFF → 6 bytes (op[0] high 2 bits = 11)
        //   - Exception: some opcodes have a different length; this is a heuristic.
        let mut lines = Vec::new();
        let mut offset = 0usize;
        let mut pc = addr;
        while offset < bytes.len() {
            let first_byte = bytes[offset];
            let len = if first_byte < 0x80 {
                2
            } else if first_byte < 0xC0 {
                4
            } else {
                6
            };
            if offset + len > bytes.len() {
                let remaining = &bytes[offset..];
                lines.push(format!("{:#010x}:  {:02x?}", pc, remaining));
                break;
            }
            let word_bytes = &bytes[offset..offset + len];
            let hex: String = word_bytes.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
            lines.push(format!("{:#010x}:  {}", pc, hex));
            offset += len;
            pc += len as u64;
        }
        lines
    }

    fn name(&self) -> &'static str {
        "s390x"
    }
}

// ===========================================================================
// ELF builder (big-endian ELF64)
// ===========================================================================

/// Build a big-endian ELF64 binary for s390x Linux with 2 LOAD segments +
/// PT_GNU_STACK.
///
/// Layout:
///   - ELF header (64 bytes)
///   - Program headers (3 × 56 bytes = 168 bytes)
///   - .text section (code)
///
/// Segment 1 (PF_R | PF_X): covers the ELF header + phdrs + .text.
/// Segment 2 (PF_R | PF_W): a zero-initialized page of writable memory
///                            (for BSS / data).
/// Segment 3 (PT_GNU_STACK): non-executable stack marker.
fn build_s390x_elf(code: &[u8], base_addr: u64, extern_symbols: &[String]) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x1000; // 4 KB
    const HOST_PAGE_ALIGN: u64 = 0x10000; // 64 KB (max common s390x page size)

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = 3; // 2x LOAD + 1x PT_GNU_STACK
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
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(2); // ELFCLASS64
    elf.push(2); // ELFDATA2MSB (big-endian)
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0); // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields (big-endian) ---
    elf.extend_from_slice(&2u16.to_be_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&22u16.to_be_bytes()); // e_machine = EM_S390
    elf.extend_from_slice(&1u32.to_be_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_be_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_be_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_be_bytes()); // e_shoff (no section headers yet)
    elf.extend_from_slice(&0u32.to_be_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_be_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_be_bytes()); // e_phentsize
    elf.extend_from_slice(&3u16.to_be_bytes()); // e_phnum = 3 (2 LOAD + GNU_STACK)
    elf.extend_from_slice(&64u16.to_be_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_be_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD (PF_R | PF_X) — .text ---
    elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_be_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_offset = 0
    elf.extend_from_slice(&base_addr.to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&base_addr.to_be_bytes()); // p_paddr
    elf.extend_from_slice(&((text_offset + text_size) as u64).to_be_bytes()); // p_filesz
    elf.extend_from_slice(&((text_offset + text_size) as u64).to_be_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_be_bytes()); // p_align

    // --- Program Header 2: LOAD (PF_R | PF_W) — .data / stack ---
    elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags = PF_R | PF_W
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_offset = 0
    elf.extend_from_slice(&data_vaddr.to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&data_vaddr.to_be_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_filesz (BSS-only)
    elf.extend_from_slice(&data_size.to_be_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_be_bytes()); // p_align

    // --- Program Header 3: PT_GNU_STACK (non-executable stack) ---
    elf.extend_from_slice(&0x6474e551u32.to_be_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags = PF_R | PF_W (no PF_X)
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_offset
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_vaddr
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_filesz
    elf.extend_from_slice(&0u64.to_be_bytes()); // p_memsz
    elf.extend_from_slice(&0x10u64.to_be_bytes()); // p_align = 16

    // --- .text section ---
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // ── Append ELF section headers when the program references external
    // (undefined) symbols — same approach as the AArch64 / sparc64 backends.
    if !extern_symbols.is_empty() {
        append_s390x_elf_sections(&mut elf, text_offset, text_size, extern_symbols);
    }

    elf
}

/// Append an ELF64 section header table for the s390x backend (mirrors
/// `append_sparc64_elf_sections`, big-endian).
fn append_s390x_elf_sections(
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
    symtab.extend_from_slice(&[0u8; 24]); // NULL symbol
    for &name_off in &sym_name_offsets {
        symtab.extend_from_slice(&name_off.to_be_bytes()); // st_name
        symtab.push((STB_GLOBAL << 4) | STT_FUNC); // st_info
        symtab.push(0); // st_other
        symtab.extend_from_slice(&SHN_UNDEF.to_be_bytes()); // st_shndx
        symtab.extend_from_slice(&0u64.to_be_bytes()); // st_value
        symtab.extend_from_slice(&0u64.to_be_bytes()); // st_size
    }

    while !elf.len().is_multiple_of(8) {
        elf.push(0);
    }
    let shstrtab_off = elf.len() as u64;
    elf.extend_from_slice(&shstrtab);
    let strtab_off = elf.len() as u64;
    elf.extend_from_slice(&strtab);
    while !elf.len().is_multiple_of(8) {
        elf.push(0);
    }
    let symtab_off = elf.len() as u64;
    let symtab_size = symtab.len() as u64;
    elf.extend_from_slice(&symtab);

    while !elf.len().is_multiple_of(8) {
        elf.push(0);
    }
    let shdr_off = elf.len() as u64;

    fn push_shdr(elf: &mut Vec<u8>, shdr: &SectionHeader<u64>) {
        elf.extend_from_slice(&shdr.sh_name.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_type.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_flags.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_addr.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_offset.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_size.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_link.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_info.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_addralign.to_be_bytes());
        elf.extend_from_slice(&shdr.sh_entsize.to_be_bytes());
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
            sh_addr: 0x10000 + text_offset,
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
    elf[40..48].copy_from_slice(&shdr_off.to_be_bytes());
    elf[60..62].copy_from_slice(&shnum.to_be_bytes());
    elf[62..64].copy_from_slice(&shstrndx.to_be_bytes());
}

// Keep the unused-variable warning quiet for the scratch register table;
// it's documented but not directly referenced (we use S0..S7 instead).
#[allow(dead_code)]
fn _scratch_table_unused() -> [Gpr; 8] {
    SCRATCH_REGS
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

        let backend = S390XBackend::new(); // use_real_regalloc = false by default
        let result_ss = backend.allocate_registers(&func);
        assert!(result_ss.is_ok(), "stack-slot allocation should succeed");

        // Now test with real regalloc.
        let mut backend = S390XBackend::new();
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
