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
    BackendError, Endianness, OutputFormat, PhysicalReg, RegClass, RelocationEntry, TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
use std::collections::HashMap;
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// s390x general-purpose registers (R0–R15).
///
/// - R0   — scratch (cannot be used as base/index in load/store encoding;
///           encoding 0 in B2/X2 fields means "no register")
/// - R1   — syscall number (Linux s390x); scratch otherwise
/// - R2–R6 — argument registers
/// - R7–R10 — scratch / volatile
/// - R11  — frame pointer (convention)
/// - R12  — global pointer (TOC) — unused in this backend
/// - R13  — base pointer (convention; unused here)
/// - R14  — link register (return address)
/// - R15  — stack pointer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
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
const S6: Gpr = Gpr::R0;  // R0 can be used for non-base-register purposes
const S7: Gpr = Gpr::R1;

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
    let op2: u8 = 0x1;
    let r1_byte = ((r1.encoding() & 0xF) << 4) | (op2 & 0xF);
    let imm_be = imm.to_be_bytes();
    [op1, r1_byte, imm_be[0], imm_be[1], imm_be[2], imm_be[3]]
}

/// Encode AGFI R1, imm32 (Add Fullword Immediate to 64-bit register).
/// Format: RIL-b, 6 bytes. op1=0xC0, op2=0x9.
/// (RIL format uses op1=0xC0 for all variants — LGFI/AGFI/SGFI/BRASL/BRCL/LARL.
///  0xEC is for RIE/RISL/RSL formats which have a different field layout.)
fn encode_agfi(r1: Gpr, imm: i32) -> [u8; 6] {
    let op1: u8 = 0xC0;
    let op2: u8 = 0x9;
    let r1_byte = ((r1.encoding() & 0xF) << 4) | (op2 & 0xF);
    let imm_be = imm.to_be_bytes();
    [op1, r1_byte, imm_be[0], imm_be[1], imm_be[2], imm_be[3]]
}

/// Encode SGFI R1, imm32 (Subtract Fullword Immediate from 64-bit register).
/// Format: RIL-b, 6 bytes. op1=0xC0, op2=0xA.
fn encode_sgfi(r1: Gpr, imm: i32) -> [u8; 6] {
    let op1: u8 = 0xC0;
    let op2: u8 = 0xA;
    let r1_byte = ((r1.encoding() & 0xF) << 4) | (op2 & 0xF);
    let imm_be = imm.to_be_bytes();
    [op1, r1_byte, imm_be[0], imm_be[1], imm_be[2], imm_be[3]]
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

/// Encode a 4-byte RRF-a instruction (3-register, with M4=0).
///
/// Format:
///   byte 0: opcode1 (8 bits)
///   byte 1: opcode2 (8 bits)
///   byte 2: R1 (4 bits, high nibble) | R2 (4 bits, low nibble)
///   byte 3: R3 (4 bits, high nibble) | M4=0 (4 bits, low nibble)
fn encode_rrf_a(op1: u8, op2: u8, r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    let byte2 = ((r1.encoding() & 0xF) << 4) | (r2.encoding() & 0xF);
    let byte3 = (r3.encoding() & 0xF) << 4;
    [op1, op2, byte2, byte3]
}

/// Encode ARK R1, R2, R3 (Add 32-bit). R1[31:0] = R2[31:0] + R3[31:0].
/// op1=0xB9, op2=0xF8.
fn encode_ark(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    encode_rrf_a(0xB9, 0xF8, r1, r2, r3)
}

/// Encode SRK R1, R2, R3 (Subtract 32-bit). R1[31:0] = R2[31:0] - R3[31:0].
/// op1=0xB9, op2=0xF9.
fn encode_srk(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    encode_rrf_a(0xB9, 0xF9, r1, r2, r3)
}

/// Encode NRK R1, R2, R3 (AND 32-bit). op1=0xB9, op2=0xF4.
fn encode_nrk(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    encode_rrf_a(0xB9, 0xF4, r1, r2, r3)
}

/// Encode ORK R1, R2, R3 (OR 32-bit). op1=0xB9, op2=0xF6.
fn encode_ork(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    encode_rrf_a(0xB9, 0xF6, r1, r2, r3)
}

/// Encode XRK R1, R2, R3 (XOR 32-bit). op1=0xB9, op2=0xF7.
fn encode_xrk(r1: Gpr, r2: Gpr, r3: Gpr) -> [u8; 4] {
    encode_rrf_a(0xB9, 0xF7, r1, r2, r3)
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
    } else if (-2147483648..=2147483647).contains(&val) {
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
        // Use temp = S5 (R5).
        code.extend_from_slice(&encode_lgfi(S5, lo));
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

/// Load a typed value from stack slot at [FP + offset] into dst.
fn ss_ld_typed(dst: Gpr, offset: i32, ty: &IRType) -> Vec<u8> {
    let mut code = Vec::new();
    if (-524288..=524287).contains(&offset) {
        match ty {
            IRType::I8 => code.extend_from_slice(&encode_lgb(dst, FP, offset)),
            IRType::U8 => {
                // No zero-extending byte load; use LGB then clear high bits via NRK.
                code.extend_from_slice(&encode_lgb(dst, FP, offset));
                // (LGB sign-extends; for u8, values 0-127 are fine. For 128-255,
                // the sign-extension would set the high bits. We need LLGC instead.)
                // LLGC R1, D2(X2, B2): op1=0xE3, op2=0x90 — Load Logical Character.
                code.clear();
                code.extend_from_slice(&encode_rxy_a(0xE3, 0x90, dst, Gpr::R0, FP, offset));
            }
            IRType::I16 => code.extend_from_slice(&encode_lgh(dst, FP, offset)),
            IRType::U16 => {
                // LLGH R1, D2(X2, B2): op1=0xE3, op2=0x91 — Load Logical Halfword.
                code.extend_from_slice(&encode_rxy_a(0xE3, 0x91, dst, Gpr::R0, FP, offset));
            }
            IRType::I32 => code.extend_from_slice(&encode_lgf(dst, FP, offset)),
            IRType::U32 => code.extend_from_slice(&encode_llgf(dst, FP, offset)),
            _ => code.extend_from_slice(&encode_lg(dst, FP, offset)),
        }
    } else {
        code.extend(ss_load_imm(S5, offset as i64));
        code.extend_from_slice(&encode_agr(S5, FP));
        match ty {
            IRType::I8 => code.extend_from_slice(&encode_lgb(dst, S5, 0)),
            IRType::U8 => {
                code.extend_from_slice(&encode_rxy_a(0xE3, 0x90, dst, Gpr::R0, S5, 0));
            }
            IRType::I16 => code.extend_from_slice(&encode_lgh(dst, S5, 0)),
            IRType::U16 => {
                code.extend_from_slice(&encode_rxy_a(0xE3, 0x91, dst, Gpr::R0, S5, 0));
            }
            IRType::I32 => code.extend_from_slice(&encode_lgf(dst, S5, 0)),
            IRType::U32 => code.extend_from_slice(&encode_llgf(dst, S5, 0)),
            _ => code.extend_from_slice(&encode_lg(dst, S5, 0)),
        }
    }
    code
}

/// Store a typed value from src to stack slot at [FP + offset].
fn ss_st_typed(src: Gpr, offset: i32, ty: &IRType) -> Vec<u8> {
    let mut code = Vec::new();
    if (-524288..=524287).contains(&offset) {
        match ty {
            IRType::I8 | IRType::U8 => code.extend_from_slice(&encode_stc(src, FP, offset)),
            IRType::I16 | IRType::U16 => code.extend_from_slice(&encode_sth(src, FP, offset)),
            IRType::I32 | IRType::U32 => code.extend_from_slice(&encode_sty(src, FP, offset)),
            _ => code.extend_from_slice(&encode_stg(src, FP, offset)),
        }
    } else {
        code.extend(ss_load_imm(S5, offset as i64));
        code.extend_from_slice(&encode_agr(S5, FP));
        match ty {
            IRType::I8 | IRType::U8 => code.extend_from_slice(&encode_stc(src, S5, 0)),
            IRType::I16 | IRType::U16 => code.extend_from_slice(&encode_sth(src, S5, 0)),
            IRType::I32 | IRType::U32 => code.extend_from_slice(&encode_sty(src, S5, 0)),
            _ => code.extend_from_slice(&encode_stg(src, S5, 0)),
        }
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
    //   vreg slot 1   ← FP + 0
    //   vreg slot 2   ← FP + 8
    //   ...
    //   vreg slot M   ← FP + 8*(M-1)
    //   Alloc data 1
    //   ...
    //   Alloc data N
    //   [padding]
    //   saved R11 (old FP)  ← FP + frame_size - 8
    //   saved R14 (LR)      ← FP + frame_size - 16  (top of frame, just below caller's SP)
    // [high address]

    let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
    let mut current_offset: i32 = 0;
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

    for (_blk_idx, block) in func.blocks.iter().enumerate() {
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
                // BRC 0x6, true_block (branch if CC != 0, i.e., cond != 0).
                // Mask 0x6 = "CC != 0" (i.e., not equal to zero).
                // Mask encoding: 0x6 = bits 1,2 set = CC=1 or CC=2 = "not zero".
                // Actually mask is 4 bits: each bit enables a CC: bit 3=CC=0, bit 2=CC=1,
                // bit 1=CC=2, bit 0=CC=3. "Not equal" (cond != 0) means CC=1 or CC=2,
                // so mask = 0b0110 = 0x6.
                let true_patch = code.len();
                code.extend_from_slice(&encode_brc(0x6, 0));
                branch_patches.push(BranchPatch {
                    code_offset: true_patch,
                    target_label: true_block.clone(),
                    is_long: false,
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
                // Emit a trap: SVC 0 with R1 = 0 (invalid syscall).  Or just a NOP.
                code.extend_from_slice(&encode_nop());
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
                    // BRC 0x8, label (branch if CC=0, i.e., equal). Mask 0x8 = CC=0.
                    let patch = code.len();
                    code.extend_from_slice(&encode_brc(0x8, 0));
                    branch_patches.push(BranchPatch {
                        code_offset: patch,
                        target_label: label.clone(),
                        is_long: false,
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
            // Move args into R2-R6 (up to 5 args).
            for (i, arg) in args.iter().enumerate() {
                if let Some(arg_reg) = Gpr::arg_register(i) {
                    code.extend(ss_load_value(arg, vreg_stack_slots, S0));
                    code.extend_from_slice(&encode_lgr(arg_reg, S0));
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
    }
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
            if is_32bit {
                code.extend_from_slice(&encode_nrk(S0, S0, S1));
                code.extend_from_slice(&encode_llgfr(S0, S0));
            } else {
                // NGR: op1=0xB9, op2=0x80.
                code.extend_from_slice(&encode_rre(0xB9, 0x80, S0, S1));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Or => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_ork(S0, S0, S1));
                code.extend_from_slice(&encode_llgfr(S0, S0));
            } else {
                // OGR: op1=0xB9, op2=0x81.
                code.extend_from_slice(&encode_rre(0xB9, 0x81, S0, S1));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Xor => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            if is_32bit {
                code.extend_from_slice(&encode_xrk(S0, S0, S1));
                code.extend_from_slice(&encode_llgfr(S0, S0));
            } else {
                // XGR: op1=0xB9, op2=0x82.
                code.extend_from_slice(&encode_rre(0xB9, 0x82, S0, S1));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::Shl => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // SLLG S0, S0, S1: shift S0 left by (S1 & 0x3F).
            // For register-based shift, use SLLG with B2=S1, D2=0.
            // SLLG R1, R3, 0(B2): op1=0xEB, op2=0x0D.
            // byte 1 = (R1<<4) | R3, byte 2 = (B2<<4) | 0, byte 3 = 0, byte 5 = 0x0D.
            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0D, S0, S0, S1, 0));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
            code.extend(ss_st(S0, dst_off));
        }
        BinOpKind::ShrL => {
            code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
            code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
            // SRLG S0, S0, S1: op1=0xEB, op2=0x0C.
            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0C, S0, S0, S1, 0));
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(S0, S0));
            }
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

// ===========================================================================
// s390x Backend
// ===========================================================================

/// s390x (IBM System Z) code generation backend.
pub struct S390XBackend {
    target_info: S390XTargetInfo,
}

impl S390XBackend {
    /// Create a new s390x backend.
    pub fn new() -> Self {
        Self {
            target_info: S390XTargetInfo,
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
}

impl Backend for S390XBackend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        s390x_allocate_registers_ss(func)
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
        //   _start:  BRASL R14, main    ; call main (return value in R2)
        //            LGFI R1, 1          ; R1 = 1 (SYS_exit)
        //            SVC 0               ; syscall: exit(R2)
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
        // BRASL R14, main — 6 bytes, offset 0 (will be patched)
        // LGFI R1, 1      — 6 bytes, offset 6
        // SVC 0           — 2 bytes, offset 12
        let start_stub_size: usize = 14;
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
        //   __NR_mmap (s390x) = 90
        // __vuma_free(addr in R2) -> munmap(addr, 0)
        //   __NR_munmap (s390x) = 91
        //
        // Linux s390x syscall numbers:
        //   exit=1, read=3, write=4, open=5, close=6, mmap=90, munmap=91,
        //   rt_sigaction=139, pipe=42, dup2=63, alarm=27, getpid=20,
        //   socket=359, clone=120, fork=2, execve=11, wait4=114,
        //   exit_group=246, ...

        let vuma_alloc_stub: Vec<u8> = {
            let mut code = Vec::new();
            // Move args: R2=size → R3; R2=NULL (0)
            // LGR R3, R2 (R3 = size)
            code.extend_from_slice(&encode_lgr(Gpr::R3, Gpr::R2));
            // LGHI R2, 0 (R2 = NULL)
            code.extend_from_slice(&encode_lghi(Gpr::R2, 0));
            // LGHI R4, 3 (PROT_READ | PROT_WRITE)
            code.extend_from_slice(&encode_lghi(Gpr::R4, 3));
            // LGFI R5, 0x22 (MAP_PRIVATE | MAP_ANONYMOUS)
            code.extend_from_slice(&encode_lgfi(Gpr::R5, 0x22));
            // LGHI R6, -1 (fd = -1)
            code.extend_from_slice(&encode_lghi(Gpr::R6, -1));
            // LGHI R7, 0 (offset = 0)
            code.extend_from_slice(&encode_lghi(Gpr::R7, 0));
            // LGFI R1, 90 (sys_mmap)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 90));
            // SVC 0
            code.extend_from_slice(&encode_svc(0));
            // BR R14 (return)
            code.extend_from_slice(&encode_br(LR));
            code
        };
        let vuma_free_stub: Vec<u8> = {
            let mut code = Vec::new();
            // LGHI R3, 0 (size = 0)
            code.extend_from_slice(&encode_lghi(Gpr::R3, 0));
            // LGFI R1, 91 (sys_munmap)
            code.extend_from_slice(&encode_lgfi(Gpr::R1, 91));
            // SVC 0
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
                ("mmap", 90),
                ("munmap", 91),
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
                ("shutdown", 370),
                ("dup3", 326),
                ("recvfrom", 367),
                ("sendto", 366),
                ("epoll_create1", 327),
                ("epoll_ctl", 250),
                ("epoll_wait", 251),
                ("clone", 120),
            ] {
                stubs.push((name.to_string(), simple_stub(num)));
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
        ((base_addr + text_file_end + HOST_PAGE_ALIGN - 1) / HOST_PAGE_ALIGN) * HOST_PAGE_ALIGN;
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

    while (elf.len() % 8) != 0 {
        elf.push(0);
    }
    let shstrtab_off = elf.len() as u64;
    elf.extend_from_slice(&shstrtab);
    let strtab_off = elf.len() as u64;
    elf.extend_from_slice(&strtab);
    while (elf.len() % 8) != 0 {
        elf.push(0);
    }
    let symtab_off = elf.len() as u64;
    let symtab_size = symtab.len() as u64;
    elf.extend_from_slice(&symtab);

    while (elf.len() % 8) != 0 {
        elf.push(0);
    }
    let shdr_off = elf.len() as u64;

    fn push_shdr(
        elf: &mut Vec<u8>,
        sh_name: u32,
        sh_type: u32,
        sh_flags: u64,
        sh_addr: u64,
        sh_offset: u64,
        sh_size: u64,
        sh_link: u32,
        sh_info: u32,
        sh_addralign: u64,
        sh_entsize: u64,
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
    push_shdr(
        elf,
        name_text,
        SHT_PROGBITS,
        0x6,
        0x10000 + text_offset,
        text_offset,
        text_size,
        0,
        0,
        16,
        0,
    );
    push_shdr(
        elf,
        name_symtab,
        SHT_SYMTAB,
        0,
        0,
        symtab_off,
        symtab_size,
        3,
        1,
        8,
        SYM_SIZE,
    );
    push_shdr(
        elf,
        name_strtab,
        SHT_STRTAB,
        0,
        0,
        strtab_off,
        strtab.len() as u64,
        0,
        0,
        1,
        0,
    );
    push_shdr(
        elf,
        name_shstrtab,
        SHT_STRTAB,
        0,
        0,
        shstrtab_off,
        shstrtab.len() as u64,
        0,
        0,
        1,
        0,
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
