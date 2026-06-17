//! # x86_64 Backend
//!
//! Implements the `Backend` trait for the x86_64 target (SystemV ABI).
//! This module provides:
//!
//! - `Gpr` — General-purpose register enum (RAX–R15)
//! - `Xmm` — SSE/SIMD register enum (XMM0–XMM15)
//! - REX prefix generation
//! - ModR/M + SIB byte encoding
//! - Instruction encoding for all key x86_64 instructions
//! - `X86_64Backend` — `Backend` implementation that lowers IR to x86_64 machine code
//!
//! ## x86_64 Register Convention (SystemV ABI)
//!
//! | Register | Role                    | Callee-saved |
//! |----------|-------------------------|-------------|
//! | RAX      | Return value / scratch  | No          |
//! | RCX      | 4th int arg / scratch   | No          |
//! | RDX      | 3rd int arg / scratch   | No          |
//! | RBX      | Callee-saved            | Yes         |
//! | RSP      | Stack pointer           | (special)   |
//! | RBP      | Frame pointer           | Yes         |
//! | RSI      | 2nd int arg / scratch   | No          |
//! | RDI      | 1st int arg / scratch   | No          |
//! | R8–R9    | 5th/6th int arg         | No          |
//! | R10–R11  | Scratch                 | No          |
//! | R12–R15  | Callee-saved            | Yes         |
//!
//! ## References
//!
//! - Intel 64 and IA-32 Architectures Software Developer's Manual, Volumes 2A/2B
//! - System V Application Binary Interface, AMD64 Architecture Processor Supplement

use crate::backend::{
    AllocatedFunction, AllocatedProgram, Backend, BackendError, TargetInfo, X86_64TargetInfo,
};
use crate::ir::{BinOpKind, CmpKind, IRFunction};
use std::collections::{HashMap, HashSet};
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// x86_64 general-purpose registers (RAX–R15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Gpr {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsp = 4,
    Rbp = 5,
    Rsi = 6,
    Rdi = 7,
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
    /// Returns the 4-bit encoding index for this register.
    pub fn encoding(&self) -> u8 {
        *self as u8
    }

    /// Returns `true` if this register requires a REX prefix bit (R8–R15).
    pub fn needs_rex(&self) -> bool {
        *self as u8 >= 8
    }

    /// Returns `true` if this register is callee-saved under SystemV ABI.
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Gpr::Rbx | Gpr::R12 | Gpr::R13 | Gpr::R14 | Gpr::R15 | Gpr::Rbp
        )
    }

    /// Returns `true` if this register is an integer argument register under SystemV ABI.
    pub fn is_arg_reg(&self) -> bool {
        matches!(
            self,
            Gpr::Rdi | Gpr::Rsi | Gpr::Rdx | Gpr::Rcx | Gpr::R8 | Gpr::R9
        )
    }

    /// Returns `true` if this register is available for register allocation.
    pub fn is_allocatable(&self) -> bool {
        !matches!(self, Gpr::Rsp)
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Gpr::Rax => "rax",
            Gpr::Rcx => "rcx",
            Gpr::Rdx => "rdx",
            Gpr::Rbx => "rbx",
            Gpr::Rsp => "rsp",
            Gpr::Rbp => "rbp",
            Gpr::Rsi => "rsi",
            Gpr::Rdi => "rdi",
            Gpr::R8 => "r8",
            Gpr::R9 => "r9",
            Gpr::R10 => "r10",
            Gpr::R11 => "r11",
            Gpr::R12 => "r12",
            Gpr::R13 => "r13",
            Gpr::R14 => "r14",
            Gpr::R15 => "r15",
        }
    }

    /// Returns the Gpr for a given SystemV integer argument index (0–5).
    pub fn arg_register(index: usize) -> Option<Gpr> {
        match index {
            0 => Some(Gpr::Rdi),
            1 => Some(Gpr::Rsi),
            2 => Some(Gpr::Rdx),
            3 => Some(Gpr::Rcx),
            4 => Some(Gpr::R8),
            5 => Some(Gpr::R9),
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
// XMM (SSE) Registers
// ===========================================================================

/// x86_64 SSE/SIMD registers (XMM0–XMM15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Xmm {
    Xmm0 = 0,
    Xmm1 = 1,
    Xmm2 = 2,
    Xmm3 = 3,
    Xmm4 = 4,
    Xmm5 = 5,
    Xmm6 = 6,
    Xmm7 = 7,
    Xmm8 = 8,
    Xmm9 = 9,
    Xmm10 = 10,
    Xmm11 = 11,
    Xmm12 = 12,
    Xmm13 = 13,
    Xmm14 = 14,
    Xmm15 = 15,
}

impl Xmm {
    /// Returns the 4-bit encoding index for this register.
    pub fn encoding(&self) -> u8 {
        *self as u8
    }

    /// Returns `true` if this register requires a REX prefix bit (XMM8–XMM15).
    pub fn needs_rex(&self) -> bool {
        *self as u8 >= 8
    }

    /// Returns the standard assembly name for this register.
    pub fn asm_name(&self) -> &'static str {
        match self {
            Xmm::Xmm0 => "xmm0",
            Xmm::Xmm1 => "xmm1",
            Xmm::Xmm2 => "xmm2",
            Xmm::Xmm3 => "xmm3",
            Xmm::Xmm4 => "xmm4",
            Xmm::Xmm5 => "xmm5",
            Xmm::Xmm6 => "xmm6",
            Xmm::Xmm7 => "xmm7",
            Xmm::Xmm8 => "xmm8",
            Xmm::Xmm9 => "xmm9",
            Xmm::Xmm10 => "xmm10",
            Xmm::Xmm11 => "xmm11",
            Xmm::Xmm12 => "xmm12",
            Xmm::Xmm13 => "xmm13",
            Xmm::Xmm14 => "xmm14",
            Xmm::Xmm15 => "xmm15",
        }
    }
}

impl fmt::Display for Xmm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.asm_name())
    }
}

// ===========================================================================
// Condition Codes
// ===========================================================================

/// x86_64 condition codes for SETcc, Jcc, and CMOVcc instructions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Cc {
    Overflow = 0x0,
    NoOverflow = 0x1,
    Below = 0x2,
    AboveEqual = 0x3,
    Equal = 0x4,
    NotEqual = 0x5,
    BelowEqual = 0x6,
    Above = 0x7,
    Sign = 0x8,
    NotSign = 0x9,
    Parity = 0xA,
    NotParity = 0xB,
    Less = 0xC,
    GreaterEqual = 0xD,
    LessEqual = 0xE,
    Greater = 0xF,
}

impl Cc {
    /// Returns the 4-bit condition code encoding.
    pub fn encoding(&self) -> u8 {
        *self as u8
    }
}

// ===========================================================================
// REX Prefix
// ===========================================================================

/// Generate a REX prefix byte.
///
/// - `w`: REX.W — 64-bit operand size
/// - `r`: Extension of the ModR/M `reg` field (for R8–R15 / XMM8–XMM15)
/// - `x`: Extension of the SIB `index` field
/// - `b`: Extension of the ModR/M `rm` field or SIB `base` field
///
/// Returns `None` if no REX byte is needed (all bits are 0).
fn rex_prefix(w: bool, r: bool, x: bool, b: bool) -> Option<u8> {
    let byte = 0x40 | (w as u8) << 3 | (r as u8) << 2 | (x as u8) << 1 | b as u8;
    if byte > 0x40 {
        Some(byte)
    } else {
        None
    }
}

// ===========================================================================
// ModR/M + SIB Encoding
// ===========================================================================

/// Encode a ModR/M byte.
///
/// - `mod_bits`: 2-bit mod field (0=mem, 1=mem+disp8, 2=mem+disp32, 3=reg)
/// - `reg`: 3-bit reg field (register or opcode extension)
/// - `rm`: 3-bit r/m field (register or memory operand)
fn modrm(mod_bits: u8, reg: u8, rm: u8) -> u8 {
    (mod_bits & 3) << 6 | (reg & 7) << 3 | (rm & 7)
}

/// Encode a SIB byte.
///
/// - `scale`: 2-bit scale factor (0=1, 1=2, 2=4, 3=8)
/// - `index`: 3-bit index register
/// - `base`: 3-bit base register
fn sib(scale: u8, index: u8, base: u8) -> u8 {
    (scale & 3) << 6 | (index & 7) << 3 | (base & 7)
}

// ===========================================================================
// Instruction Encoding Functions
// ===========================================================================

/// Emit a REX.W prefix plus opcode, then a ModR/M byte for reg-reg operations.
///
/// This is the common pattern for 64-bit ALU instructions: REX.W + opcode + ModR/M(mod=3, reg, rm).
fn emit_rexw_reg_reg(code: &mut Vec<u8>, opcode: u8, reg: Gpr, rm: Gpr) {
    let r = reg.needs_rex();
    let b = rm.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        // REX.W is still needed for 64-bit operations even if r and b are 0
        code.push(0x48);
    }
    code.push(opcode);
    code.push(modrm(3, reg.encoding() & 7, rm.encoding() & 7));
}

/// Emit a REX.W prefix (always), then opcode, then ModR/M for reg-reg with
/// specific reg field (opcode extension) and rm register.
fn emit_rexw_opext_reg(code: &mut Vec<u8>, opcode: u8, opext: u8, rm: Gpr) {
    let b = rm.needs_rex();
    // Always emit REX.W for 64-bit
    let rex = 0x48 | (b as u8);
    code.push(rex);
    code.push(opcode);
    code.push(modrm(3, opext & 7, rm.encoding() & 7));
}

/// Encode MOV r64, r64 (REX.W + 89 /r)
pub fn encode_mov_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x89, src, dst);
    code
}

/// Encode MOV r64, imm64 (REX.W + B8+rd + 8-byte imm)
pub fn encode_mov_reg_imm64(dst: Gpr, imm: u64) -> Vec<u8> {
    let mut code = Vec::with_capacity(10);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xB8 + (dst.encoding() & 7));
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode MOV r64, imm32 (REX.W + C7 /0 + 4-byte imm, sign-extended)
pub fn encode_mov_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xC7);
    code.push(modrm(3, 0, dst.encoding() & 7));
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode MOV r64, [r64+offset] (REX.W + 8B /r + displacement)
///
/// Handles special cases:
/// - RSP/R12 as base: SIB byte required
/// - RBP/R13 as base: mod=1 with disp8=0 even for zero offset
/// - Offset fits in i8: disp8; otherwise: disp32
pub fn encode_mov_reg_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(8);
    let r = dst.needs_rex();
    let b = base.needs_rex();

    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x8B);

    let needs_sib = base == Gpr::Rsp || base == Gpr::R12;
    let needs_disp8_for_zero = base == Gpr::Rbp || base == Gpr::R13;

    if offset == 0 && !needs_disp8_for_zero && !needs_sib {
        // mod=00, no displacement
        code.push(modrm(0, dst.encoding() & 7, base.encoding() & 7));
    } else if needs_sib {
        // SIB byte required: base = RSP(4), index = RSP(4) means "no index"
        if offset == 0 {
            code.push(modrm(0, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
        } else if (-128..=127).contains(&offset) {
            code.push(modrm(1, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if (-128..=127).contains(&offset) {
        // mod=01, disp8
        code.push(modrm(1, dst.encoding() & 7, base.encoding() & 7));
        code.push(offset as u8);
    } else {
        // mod=10, disp32
        code.push(modrm(2, dst.encoding() & 7, base.encoding() & 7));
        code.extend_from_slice(&offset.to_le_bytes());
    }

    code
}

/// Encode MOV [r64+offset], r64 (REX.W + 89 /r + displacement)
pub fn encode_mov_mem_reg(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(8);
    let r = src.needs_rex();
    let b = base.needs_rex();

    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x89);

    let needs_sib = base == Gpr::Rsp || base == Gpr::R12;
    let needs_disp8_for_zero = base == Gpr::Rbp || base == Gpr::R13;

    if offset == 0 && !needs_disp8_for_zero && !needs_sib {
        code.push(modrm(0, src.encoding() & 7, base.encoding() & 7));
    } else if needs_sib {
        if offset == 0 {
            code.push(modrm(0, src.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
        } else if (-128..=127).contains(&offset) {
            code.push(modrm(1, src.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, src.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if (-128..=127).contains(&offset) {
        code.push(modrm(1, src.encoding() & 7, base.encoding() & 7));
        code.push(offset as u8);
    } else {
        code.push(modrm(2, src.encoding() & 7, base.encoding() & 7));
        code.extend_from_slice(&offset.to_le_bytes());
    }

    code
}

/// Encode ADD r64, r64 (REX.W + 01 /r)
pub fn encode_add_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x01, src, dst);
    code
}

/// Encode SUB r64, r64 (REX.W + 29 /r)
pub fn encode_sub_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x29, src, dst);
    code
}

/// Encode IMUL r64, r64 (REX.W + 0F AF /r)
pub fn encode_imul_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xAF);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode IDIV r64 (REX.W + F7 /7)
pub fn encode_idiv_reg(src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xF7, 7, src);
    code
}

/// Encode CMP r64, r64 (REX.W + 39 /r)
pub fn encode_cmp_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x39, src, dst);
    code
}

/// Encode CMP r64, imm32 (REX.W + 81 /7 + imm)
pub fn encode_cmp_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x81);
    code.push(modrm(3, 7, dst.encoding() & 7));
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode TEST r64, r64 (REX.W + 85 /r)
pub fn encode_test_reg_reg(a: Gpr, b: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x85, a, b);
    code
}

/// Encode AND r64, r64 (REX.W + 21 /r)
pub fn encode_and_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x21, src, dst);
    code
}

/// Encode OR r64, r64 (REX.W + 09 /r)
pub fn encode_or_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x09, src, dst);
    code
}

/// Encode XOR r64, r64 (REX.W + 31 /r)
pub fn encode_xor_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x31, src, dst);
    code
}

/// Encode SHL r64, CL (REX.W + D3 /4)
pub fn encode_shl_reg_cl(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xD3, 4, dst);
    code
}

/// Encode SHR r64, CL (REX.W + D3 /5)
pub fn encode_shr_reg_cl(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xD3, 5, dst);
    code
}

/// Encode SAR r64, CL (REX.W + D3 /7)
pub fn encode_sar_reg_cl(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xD3, 7, dst);
    code
}

/// Encode JMP rel32 (E9 + 4-byte offset)
pub fn encode_jmp_rel32(offset: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    code.push(0xE9);
    code.extend_from_slice(&offset.to_le_bytes());
    code
}

/// Encode CALL rel32 (E8 + 4-byte offset)
pub fn encode_call_rel32(offset: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    code.push(0xE8);
    code.extend_from_slice(&offset.to_le_bytes());
    code
}

/// Encode RET (C3)
pub fn encode_ret() -> Vec<u8> {
    vec![0xC3]
}

/// Encode NOP (90)
pub fn encode_nop() -> Vec<u8> {
    vec![0x90]
}

/// Encode PUSH r64 (50+rd or REX.B+50+rd for R8–R15)
pub fn encode_push(src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(2);
    if src.needs_rex() {
        code.push(0x41); // REX.B
    }
    code.push(0x50 + (src.encoding() & 7));
    code
}

/// Encode POP r64 (58+rd or REX.B+58+rd for R8–R15)
pub fn encode_pop(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(2);
    if dst.needs_rex() {
        code.push(0x41); // REX.B
    }
    code.push(0x58 + (dst.encoding() & 7));
    code
}

/// Encode SETcc r/m8 (0F 9x /r)
pub fn encode_setcc(cc: Cc, dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    // SETcc always uses a byte register. For registers < 8, no REX needed
    // (unless we need to force REX to access SPL, BPL, SIL, DIL for RSP/RBP/RSI/RDI).
    // For R8B-R15B, we need REX.B.
    if dst.needs_rex() {
        code.push(0x41); // REX.B for R8B-R15B
    } else if matches!(dst, Gpr::Rsp | Gpr::Rbp | Gpr::Rsi | Gpr::Rdi) {
        // Accessing SPL, BPL, SIL, DIL requires a REX prefix
        code.push(0x40); // Bare REX
    }
    code.push(0x0F);
    code.push(0x90 + cc.encoding());
    code.push(modrm(3, 0, dst.encoding() & 7));
    code
}

/// Encode Jcc rel32 (0F 8x + 4-byte offset)
pub fn encode_jcc_rel32(cc: Cc, offset: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(6);
    code.push(0x0F);
    code.push(0x80 + cc.encoding());
    code.extend_from_slice(&offset.to_le_bytes());
    code
}

/// Encode CMOVcc r64, r64 (REX.W + 0F 4x /r)
pub fn encode_cmovcc_reg_reg(cc: Cc, dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0x40 + cc.encoding());
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode LEA r64, [r64+offset] (REX.W + 8D /r)
pub fn encode_lea_reg_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(8);
    let r = dst.needs_rex();
    let b = base.needs_rex();

    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x8D);

    let needs_sib = base == Gpr::Rsp || base == Gpr::R12;
    let needs_disp8_for_zero = base == Gpr::Rbp || base == Gpr::R13;

    if offset == 0 && !needs_disp8_for_zero && !needs_sib {
        code.push(modrm(0, dst.encoding() & 7, base.encoding() & 7));
    } else if needs_sib {
        if offset == 0 {
            code.push(modrm(0, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
        } else if (-128..=127).contains(&offset) {
            code.push(modrm(1, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if (-128..=127).contains(&offset) {
        code.push(modrm(1, dst.encoding() & 7, base.encoding() & 7));
        code.push(offset as u8);
    } else {
        code.push(modrm(2, dst.encoding() & 7, base.encoding() & 7));
        code.extend_from_slice(&offset.to_le_bytes());
    }

    code
}

/// Encode MOVZX r64, r8 (REX.W + 0F B6 /r) — zero-extend byte to 64 bits
pub fn encode_movzx_reg8(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xB6);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVZX r64, r16 (REX.W + 0F B7 /r) — zero-extend word to 64 bits
pub fn encode_movzx_reg16(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xB7);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVSX r64, r8 (REX.W + 0F BE /r) — sign-extend byte to 64 bits
pub fn encode_movsx_reg8(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xBE);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVSX r64, r16 (REX.W + 0F BF /r) — sign-extend word to 64 bits
pub fn encode_movsx_reg16(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xBF);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVSX r64, r32 (REX.W + 63 /r) — sign-extend dword to 64 bits
pub fn encode_movsxd(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x63);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode XCHG rax, r64 (REX.W + 90+rd)
pub fn encode_xchg_rax_reg(src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(2);
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x90 + (src.encoding() & 7));
    code
}

/// Encode SYSCALL (0F 05)
pub fn encode_syscall() -> Vec<u8> {
    vec![0x0F, 0x05]
}

/// Encode INT3 (CC)
pub fn encode_int3() -> Vec<u8> {
    vec![0xCC]
}

/// Encode NEG r64 (REX.W + F7 /3)
pub fn encode_neg_reg(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xF7, 3, dst);
    code
}

/// Encode NOT r64 (REX.W + F7 /2)
pub fn encode_not_reg(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xF7, 2, dst);
    code
}

/// Encode MUL r64 (REX.W + F7 /4) — unsigned multiply, result in RDX:RAX
pub fn encode_mul_reg(src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xF7, 4, src);
    code
}

/// Encode DIV r64 (REX.W + F7 /6) — unsigned divide
pub fn encode_div_reg(src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xF7, 6, src);
    code
}

/// Encode CQO (REX.W + 99) — sign-extend RAX into RDX:RAX
pub fn encode_cqo() -> Vec<u8> {
    vec![0x48, 0x99]
}

/// Encode SUB r64, imm32 (REX.W + 81 /5 + imm)
pub fn encode_sub_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x81);
    code.push(modrm(3, 5, dst.encoding() & 7));
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode ADD r64, imm32 (REX.W + 81 /0 + imm)
pub fn encode_add_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x81);
    code.push(modrm(3, 0, dst.encoding() & 7));
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode AND r64, imm32 (REX.W + 81 /4 + imm32)
pub fn encode_and_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x81);
    code.push(modrm(3, 4, dst.encoding() & 7)); // /4 is the AND extension
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode OR r64, imm32 (REX.W + 81 /1 + imm32)
pub fn encode_or_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x81);
    code.push(modrm(3, 1, dst.encoding() & 7)); // /1 is the OR extension
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode XOR r64, imm32 (REX.W + 81 /6 + imm32)
pub fn encode_xor_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, false, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x81);
    code.push(modrm(3, 6, dst.encoding() & 7)); // /6 is the XOR extension
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode ROR r64, CL (REX.W + D3 /1)
pub fn encode_ror_reg_cl(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xD3, 1, dst);
    code
}

/// Encode ROL r64, CL (REX.W + D3 /0)
pub fn encode_rol_reg_cl(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xD3, 0, dst);
    code
}

// ===========================================================================
// Memory Operand Helper
// ===========================================================================

/// Encode a memory operand (ModR/M + optional SIB + displacement) for [base + offset].
/// Appends the ModR/M byte, SIB byte (if needed), and displacement to `code`.
fn encode_mem_operand(code: &mut Vec<u8>, reg: u8, base: Gpr, offset: i32) {
    let needs_sib = base == Gpr::Rsp || base == Gpr::R12;
    let needs_disp8_for_zero = base == Gpr::Rbp || base == Gpr::R13;

    if offset == 0 && !needs_disp8_for_zero && !needs_sib {
        // mod=00, no displacement
        code.push(modrm(0, reg, base.encoding() & 7));
    } else if needs_sib {
        // SIB byte required: base = RSP(4), index = RSP(4) means "no index"
        if offset == 0 {
            code.push(modrm(0, reg, 4));
            code.push(sib(0, 4, base.encoding() & 7));
        } else if (-128..=127).contains(&offset) {
            code.push(modrm(1, reg, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, reg, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if (-128..=127).contains(&offset) {
        // mod=01, disp8
        code.push(modrm(1, reg, base.encoding() & 7));
        code.push(offset as u8);
    } else {
        // mod=10, disp32
        code.push(modrm(2, reg, base.encoding() & 7));
        code.extend_from_slice(&offset.to_le_bytes());
    }
}

/// Encode MOVZX r64, byte [r64 + offset] (REX.W + 0F B6 /r with memory operand)
pub fn encode_movzx_reg8_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    let r = dst.needs_rex();
    let b = base.needs_rex();
    // Always need REX.W for 64-bit dest
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xB6);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOV byte [r64 + offset], r8 (low byte of GPR) (88 /r with memory operand, no REX.W)
pub fn encode_mov_mem8_reg8(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    let r = src.needs_rex();
    let b = base.needs_rex();
    // We need REX prefix if r or b is extended register, but NOT REX.W
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0x88);
    encode_mem_operand(&mut code, src.encoding() & 7, base, offset);
    code
}

/// Encode MOV dword [r64 + offset], r32 (89 /r with no REX.W, 32-bit store that zero-extends)
pub fn encode_mov_mem32_reg32(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    let r = src.needs_rex();
    let b = base.needs_rex();
    // No REX.W for 32-bit operand size
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0x89);
    encode_mem_operand(&mut code, src.encoding() & 7, base, offset);
    code
}

/// Encode MOV r32, dword [r64 + offset] (8B /r with no REX.W, 32-bit load that zero-extends to 64)
pub fn encode_mov_reg32_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    let r = dst.needs_rex();
    let b = base.needs_rex();
    // No REX.W for 32-bit operand size
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0x8B);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOVSX r64, byte [r64 + offset] (REX.W + 0F BE /r with memory operand)
pub fn encode_movsx_reg8_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    let r = dst.needs_rex();
    let b = base.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xBE);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOVSX r64, word [r64 + offset] (REX.W + 0F BF /r with memory operand)
pub fn encode_movsx_reg16_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    let r = dst.needs_rex();
    let b = base.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xBF);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOV word [r64 + offset], r16 (66 89 /r with memory operand)
pub fn encode_mov_mem16_reg16(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    let r = src.needs_rex();
    let b = base.needs_rex();
    // 16-bit operand size prefix
    code.push(0x66);
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0x89);
    encode_mem_operand(&mut code, src.encoding() & 7, base, offset);
    code
}

/// Encode MOVZX r64, word [r64 + offset] (REX.W + 0F B7 /r with memory operand)
pub fn encode_movzx_reg16_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    let r = dst.needs_rex();
    let b = base.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x0F);
    code.push(0xB7);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

// ===========================================================================
// SSE / x87 FP Conversion & Move Encoding
// ===========================================================================

/// Encode MOVD xmm, r32 (66 0F 6E /r) — move 32-bit GPR low dword into XMM.
pub fn encode_movd_xmm_gpr(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0x66);
    code.push(0x0F);
    code.push(0x6E);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVD r32, xmm (66 0F 7E /r) — move low dword from XMM to GPR.
pub fn encode_movd_gpr_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0x66);
    code.push(0x0F);
    code.push(0x7E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MOVQ xmm, r64 (66 REX.W 0F 6E /r) — move 64-bit GPR into XMM.
pub fn encode_movq_xmm_gpr(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x66);
    code.push(0x0F);
    code.push(0x6E);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVQ r64, xmm (66 REX.W 0F 7E /r) — move 64-bit from XMM to GPR.
pub fn encode_movq_gpr_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0x66);
    code.push(0x0F);
    code.push(0x7E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSI2SD xmm, r32 (F2 0F 2A /r) — convert signed 32-bit int to f64.
pub fn encode_cvtsi2sd_xmm_r32(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSI2SD xmm, r64 (F2 REX.W 0F 2A /r) — convert signed 64-bit int to f64.
pub fn encode_cvtsi2sd_xmm_r64(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSI2SS xmm, r32 (F3 0F 2A /r) — convert signed 32-bit int to f32.
pub fn encode_cvtsi2ss_xmm_r32(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSI2SS xmm, r64 (F3 REX.W 0F 2A /r) — convert signed 64-bit int to f32.
pub fn encode_cvtsi2ss_xmm_r64(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = dst.needs_rex();
    let b = src.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSD2SI r32, xmm (F2 0F 2D /r) — convert f64 to signed 32-bit int.
pub fn encode_cvtsd2si_r32_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x2D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSD2SI r64, xmm (F2 REX.W 0F 2D /r) — convert f64 to signed 64-bit int.
pub fn encode_cvtsd2si_r64_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x2D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSS2SI r32, xmm (F3 0F 2D /r) — convert f32 to signed 32-bit int.
pub fn encode_cvtss2si_r32_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x2D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSS2SI r64, xmm (F3 REX.W 0F 2D /r) — convert f32 to signed 64-bit int.
pub fn encode_cvtss2si_r64_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x2D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTTSD2SI r32, xmm (F2 0F 2C /r) — convert f64 to signed 32-bit int
/// with truncation (toward zero).  This is the truncating variant of
/// `encode_cvtsd2si_r32_xmm`; it matches the C-style float->int cast
/// semantics represented by the IR's `FloatToInt` / `FloatToUInt`.
pub fn encode_cvttsd2si_r32_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x2C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTTSD2SI r64, xmm (F2 REX.W 0F 2C /r) — convert f64 to signed
/// 64-bit int with truncation (toward zero).
pub fn encode_cvttsd2si_r64_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x2C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTTSS2SI r32, xmm (F3 0F 2C /r) — convert f32 to signed 32-bit int
/// with truncation (toward zero).
pub fn encode_cvttss2si_r32_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x2C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTTSS2SI r64, xmm (F3 REX.W 0F 2C /r) — convert f32 to signed
/// 64-bit int with truncation (toward zero).
pub fn encode_cvttss2si_r64_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if let Some(rex) = rex_prefix(true, r, false, b) {
        code.push(rex);
    } else {
        code.push(0x48);
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x2C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSS2SD xmm, xmm (F3 0F 5A /r) — convert f32 to f64 (widen).
pub fn encode_cvtss2sd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x5A);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSD2SS xmm, xmm (F2 0F 5A /r) — convert f64 to f32 (narrow).
pub fn encode_cvtsd2ss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x5A);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode ADDSD xmm, xmm (F2 0F 58 /r) — add scalar double-precision floats.
pub fn encode_addsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x58);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode ADDSS xmm, xmm (F3 0F 58 /r) — add scalar single-precision floats.
pub fn encode_addss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b {
        if let Some(rex) = rex_prefix(false, r, false, b) {
            code.push(rex);
        }
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x58);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

// ===========================================================================
// x86_64 Mnemonic Disassembler
// ===========================================================================

/// Decode x86_64 bytes into mnemonic strings with (offset, mnemonic) pairs.
///
/// Handles the top 20+ most common x86_64 instructions including mov, add, sub,
/// push, pop, call, ret, jmp, cmp, test, lea, xor, and, or, shl, shr, nop,
/// mul, div, imul.
fn disassemble_x86_64_mnemonic(bytes: &[u8], addr: u64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    let mut pc = addr;

    while offset < bytes.len() {
        let start = offset;
        let start_pc = pc;
        let mut pos = offset;

        // Skip legacy prefixes
        while pos < bytes.len() && matches!(bytes[pos], 0x66 | 0x67 | 0xF2 | 0xF3) {
            pos += 1;
        }

        // REX prefix
        let mut rex = 0u8;
        let mut _rex_w = false;
        let mut rex_r = false;
        let mut rex_b = false;
        if pos < bytes.len() && bytes[pos] >= 0x40 && bytes[pos] <= 0x4F {
            rex = bytes[pos];
            _rex_w = (rex & 0x08) != 0;
            rex_r = (rex & 0x04) != 0;
            rex_b = (rex & 0x01) != 0;
            pos += 1;
        }

        if pos >= bytes.len() {
            let end = pos.min(bytes.len());
            let hex_bytes: Vec<String> = bytes[start..end]
                .iter()
                .map(|b| format!("{:02x}", b))
                .collect();
            lines.push(format!("{:#010x}:  {}", start_pc, hex_bytes.join(" ")));
            offset = end;
            pc = start_pc + (end - start) as u64;
            continue;
        }

        let opcode = bytes[pos];
        pos += 1;

        let mnemonic = match opcode {
            // NOP
            0x90 => "nop".to_string(),

            // RET
            0xC3 => "ret".to_string(),

            // INT3
            0xCC => "int3".to_string(),

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

            // MOV r64, imm64 (B8+rd)
            0xB8..=0xBF => {
                let reg_idx = (opcode - 0xB8) | (if rex_b { 8 } else { 0 });
                if pos + 8 <= bytes.len() {
                    let imm = u64::from_le_bytes(bytes[pos..pos + 8].try_into().unwrap_or([0; 8]));
                    pos += 8;
                    format!("mov {}, {:#x}", gpr_name_64(reg_idx), imm)
                } else {
                    pos = bytes.len();
                    format!("mov {}, ???", gpr_name_64(reg_idx))
                }
            }

            // JMP rel32
            0xE9 => {
                if pos + 4 <= bytes.len() {
                    let rel = i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                    pos += 4;
                    format!(
                        "jmp {:#x}",
                        (start_pc + (pos - start) as u64).wrapping_add(rel as u64)
                    )
                } else {
                    pos = bytes.len();
                    "jmp ???".to_string()
                }
            }

            // CALL rel32
            0xE8 => {
                if pos + 4 <= bytes.len() {
                    let rel = i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                    pos += 4;
                    format!(
                        "call {:#x}",
                        (start_pc + (pos - start) as u64).wrapping_add(rel as u64)
                    )
                } else {
                    pos = bytes.len();
                    "call ???".to_string()
                }
            }

            // Two-byte opcode (0F xx)
            0x0F => {
                if pos >= bytes.len() {
                    "0f ???".to_string()
                } else {
                    let op2 = bytes[pos];
                    pos += 1;
                    match op2 {
                        // SYSCALL
                        0x05 => "syscall".to_string(),
                        // IMUL r64, r64
                        0xAF => {
                            let (r, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                            pos = new_pos;
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
                                format!(
                                    "{} {:#x}",
                                    cc_name,
                                    (start_pc + (pos - start) as u64).wrapping_add(rel as u64)
                                )
                            } else {
                                pos = bytes.len();
                                format!("{} ???", cc_name)
                            }
                        }
                        // MOVZX r64, r8
                        0xB6 => {
                            let (r, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                            pos = new_pos;
                            format!("movzx {}, {}", gpr_name_64(r), gpr_name_8(rm, rex != 0))
                        }
                        // MOVZX r64, r16
                        0xB7 => {
                            let (r, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                            pos = new_pos;
                            format!("movzx {}, r16({})", gpr_name_64(r), gpr_name_64(rm))
                        }
                        // MOVSX r64, r8
                        0xBE => {
                            let (r, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                            pos = new_pos;
                            format!("movsx {}, {}", gpr_name_64(r), gpr_name_8(rm, rex != 0))
                        }
                        // MOVSX r64, r16
                        0xBF => {
                            let (r, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                            pos = new_pos;
                            format!("movsx {}, r16({})", gpr_name_64(r), gpr_name_64(rm))
                        }
                        // SETcc r/m8
                        0x90..=0x9F => {
                            let (_, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, false, rex_b);
                            pos = new_pos;
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
                            format!("{} {}", cc_name, gpr_name_8(rm, rex != 0))
                        }
                        // CMOVcc r64, r64
                        0x40..=0x4F => {
                            let (r, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                            pos = new_pos;
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
                            format!("{} {}, {}", cc_name, gpr_name_64(r), gpr_name_64(rm))
                        }
                        _ => format!("0f {:02x}", op2),
                    }
                }
            }

            // ALU reg-reg opcodes (with ModR/M byte)
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
            0x87 => {
                let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                pos = np;
                format!("xchg {}, {}", gpr_name_64(rm), gpr_name_64(r))
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
            0x8D => {
                let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                pos = np;
                format!("lea {}, [{}]", gpr_name_64(r), gpr_name_64(rm))
            }
            0x63 => {
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
                    _ => format!("f7 /{}, {}", r, gpr_name_64(rm)),
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
                    _ => format!("d3 /{}, {}", r, gpr_name_64(rm)),
                }
            }

            // C7 /0 + imm32 (MOV r/m64, imm32)
            0xC7 => {
                let (_, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                pos = np;
                if pos + 4 <= bytes.len() {
                    let imm = i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                    pos += 4;
                    format!("mov {}, {}", gpr_name_64(rm), imm)
                } else {
                    pos = bytes.len();
                    format!("mov {}, ???", gpr_name_64(rm))
                }
            }

            // 81 /x + imm32 (ADD/SUB/etc r/m64, imm32)
            0x81 => {
                let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                pos = np;
                if pos + 4 <= bytes.len() {
                    let imm = i32::from_le_bytes(bytes[pos..pos + 4].try_into().unwrap_or([0; 4]));
                    pos += 4;
                    let op_name = match r {
                        0 => "add",
                        1 => "or",
                        2 => "adc",
                        3 => "sbb",
                        4 => "and",
                        5 => "sub",
                        6 => "xor",
                        7 => "cmp",
                        _ => "???",
                    };
                    format!("{} {}, {}", op_name, gpr_name_64(rm), imm)
                } else {
                    pos = bytes.len();
                    format!("81 /{}, {}", r, gpr_name_64(rm))
                }
            }

            // 99 (CQO)
            0x99 => "cqo".to_string(),

            // XCHG rax, r64
            0x91..=0x97 => {
                let reg_idx = (opcode - 0x90) | (if rex_b { 8 } else { 0 });
                format!("xchg rax, {}", gpr_name_64(reg_idx))
            }

            _ => {
                // Unknown opcode — just show hex
                format!(".byte {:02x}", opcode)
            }
        };

        let end = pos.min(bytes.len());
        let hex_bytes: Vec<String> = bytes[start..end]
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        lines.push(format!(
            "{:#010x}:  {:20} {}",
            start_pc,
            hex_bytes.join(" "),
            mnemonic
        ));

        offset = end;
        pc = start_pc + (end - start) as u64;
    }

    lines
}

/// Helper: get 64-bit GPR name from index (0-15).
fn gpr_name_64(idx: u8) -> &'static str {
    match idx & 0xF {
        0 => "rax",
        1 => "rcx",
        2 => "rdx",
        3 => "rbx",
        4 => "rsp",
        5 => "rbp",
        6 => "rsi",
        7 => "rdi",
        8 => "r8",
        9 => "r9",
        10 => "r10",
        11 => "r11",
        12 => "r12",
        13 => "r13",
        14 => "r14",
        15 => "r15",
        _ => "r??",
    }
}

/// Helper: get 8-bit GPR name from index (0-15).
fn gpr_name_8(idx: u8, has_rex: bool) -> &'static str {
    match idx & 0xF {
        0 => {
            if has_rex {
                "r8b"
            } else {
                "al"
            }
        }
        1 => {
            if has_rex {
                "r9b"
            } else {
                "cl"
            }
        }
        2 => {
            if has_rex {
                "r10b"
            } else {
                "dl"
            }
        }
        3 => {
            if has_rex {
                "r11b"
            } else {
                "bl"
            }
        }
        4 => {
            if has_rex {
                "r12b"
            } else {
                "spl"
            }
        } // REX required for spl
        5 => {
            if has_rex {
                "r13b"
            } else {
                "bpl"
            }
        }
        6 => {
            if has_rex {
                "r14b"
            } else {
                "sil"
            }
        }
        7 => {
            if has_rex {
                "r15b"
            } else {
                "dil"
            }
        }
        8 => "r8b",
        9 => "r9b",
        10 => "r10b",
        11 => "r11b",
        12 => "r12b",
        13 => "r13b",
        14 => "r14b",
        15 => "r15b",
        _ => "??b",
    }
}

/// Decode a ModR/M byte, returning (reg, rm, new_pos).
/// Handles register-register (mod=3) only for simplicity.
fn decode_modrm_reg_rm(bytes: &[u8], pos: usize, rex_r: bool, rex_b: bool) -> (u8, u8, usize) {
    if pos >= bytes.len() {
        return (0, 0, pos);
    }
    let modrm = bytes[pos];
    let new_pos = pos + 1;
    let mod_bits = (modrm >> 6) & 3;
    let reg = ((modrm >> 3) & 7) | (if rex_r { 8 } else { 0 });
    let rm = (modrm & 7) | (if rex_b { 8 } else { 0 });

    if mod_bits == 3 {
        // Register-register
        (reg, rm, new_pos)
    } else {
        // For memory operands, just return the rm as-is (simplified)
        (reg, rm, new_pos)
    }
}

// ===========================================================================
// ELF64 Emission
// ===========================================================================

/// Build a minimal ELF64 binary for x86_64 from raw code bytes.
///
/// Produces a static executable with up to two LOAD segments:
/// 1. `.text` segment (PF_R | PF_X) — executable code
/// 2. `.bss` segment (PF_R | PF_W) — zero-initialized writable data (only if `bss_size > 0`)
///
/// Entry point is at `base_addr` + header offset.
///
/// The BSS segment is placed at the next page-aligned address after the text
/// segment. It has `p_filesz = 0` and `p_memsz = bss_size`, so the kernel
/// zero-fills it at load time. This provides writable memory for global
/// variables (e.g., those created by `allocate()` in VUMA source).
fn build_minimal_x86_64_elf(code: &[u8], base_addr: u64, bss_size: u64) -> Vec<u8> {
    const PAGE_SIZE: u64 = 0x1000; // 4 KB

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let num_phdrs: u64 = if bss_size > 0 { 2 } else { 1 };
    let phdr_end = elf_header_size + phdr_size * num_phdrs;
    // Page-align the text segment start for mmap compatibility (required by QEMU).
    let text_offset = ((phdr_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
    let text_size = code.len() as u64;
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity(text_offset as usize + code.len());

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(2); // ELFCLASS64
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0); // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields ---
    elf.extend_from_slice(&2u16.to_le_bytes()); // e_type = ET_EXEC
    elf.extend_from_slice(&62u16.to_le_bytes()); // e_machine = EM_X86_64
    elf.extend_from_slice(&1u32.to_le_bytes()); // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes()); // e_shoff (no section headers)
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&(num_phdrs as u16).to_le_bytes()); // e_phnum
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD segment for .text (PF_R | PF_X) ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD segment for .bss (PF_R | PF_W) ---
    // Only emitted when there is BSS data. The BSS segment starts at the
    // next page boundary after the text segment. p_filesz = 0 because BSS
    // has no file content; the kernel zero-fills p_memsz bytes at load time.
    if bss_size > 0 {
        let bss_vaddr = ((base_addr + text_offset + text_size + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
        elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
        elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
        elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset (no file content)
        elf.extend_from_slice(&bss_vaddr.to_le_bytes()); // p_vaddr
        elf.extend_from_slice(&bss_vaddr.to_le_bytes()); // p_paddr
        elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz (BSS is zero-filled)
        elf.extend_from_slice(&bss_size.to_le_bytes()); // p_memsz
        elf.extend_from_slice(&PAGE_SIZE.to_le_bytes()); // p_align
    }

    // --- Padding + Code section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    elf
}

// ===========================================================================
// Runtime Syscall Stubs
// ===========================================================================

/// Build runtime syscall stubs for x86_64 Linux.
///
/// These are tiny functions that use the `syscall` instruction to implement
/// POSIX operations without requiring libc. Each stub:
/// 1. Loads the syscall number into RAX
/// 2. Moves the 4th argument from RCX to R10 (for mmap, which has ≥4 args)
/// 3. Executes `syscall`
/// 4. Returns to the caller
///
/// # x86_64 Linux Syscall Numbers
///
/// | Function  | Syscall # | Name              |
/// |-----------|-----------|-------------------|
/// | read      | 0         | sys_read          |
/// | write     | 1         | sys_write         |
/// | open      | 2         | sys_open          |
/// | close     | 3         | sys_close         |
/// | sigaction | 13        | sys_rt_sigaction  |
/// | mmap      | 9         | sys_mmap          |
/// | munmap    | 11        | sys_munmap        |
/// | alarm     | 37        | sys_alarm         |
/// | exit      | 60        | sys_exit          |
/// | unlink    | 87        | sys_unlink        |
///
/// # Calling Convention Notes
///
/// - SystemV AMD64 ABI: args in RDI, RSI, RDX, RCX, R8, R9
/// - Linux syscall ABI: args in RDI, RSI, RDX, R10, R8, R9, number in RAX
/// - The only difference is the 4th arg: RCX (calling) vs R10 (syscall)
/// - For functions with ≤3 args, no register shuffling is needed
fn build_runtime_syscall_stubs() -> Vec<(String, Vec<u8>)> {
    let mut stubs = Vec::new();

    // write(fd, buf, count) → ssize_t  [syscall 1]
    // args: RDI=fd, RSI=buf, RDX=count → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));  // sys_write
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("write".to_string(), code));
    }

    // open(pathname, flags, mode) → int  [syscall 2]
    // args: RDI=pathname, RSI=flags, RDX=mode → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 2));  // sys_open
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("open".to_string(), code));
    }

    // close(fd) → int  [syscall 3]
    // args: RDI=fd → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));  // sys_close
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("close".to_string(), code));
    }

    // mmap(addr, length, prot, flags, fd, offset) → void*  [syscall 9]
    // args: RDI=addr, RSI=length, RDX=prot, RCX=flags, R8=fd, R9=offset
    // syscall: RDI=addr, RSI=length, RDX=prot, R10=flags, R8=fd, R9=offset
    // Need to move 4th arg from RCX → R10 before syscall
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 9));   // sys_mmap
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX → R10
        code.extend(encode_syscall());                      // syscall
        code.extend(encode_ret());                          // ret
        stubs.push(("mmap".to_string(), code));
    }

    // munmap(addr, length) → int  [syscall 11]
    // args: RDI=addr, RSI=length → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 11));  // sys_munmap
        code.extend(encode_syscall());                      // syscall
        code.extend(encode_ret());                          // ret
        stubs.push(("munmap".to_string(), code));
    }

    // unlink(pathname) → int  [syscall 87]
    // args: RDI=pathname → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 87));  // sys_unlink
        code.extend(encode_syscall());                      // syscall
        code.extend(encode_ret());                          // ret
        stubs.push(("unlink".to_string(), code));
    }

    // read(fd, buf, count) → ssize_t  [syscall 0]
    // args: RDI=fd, RSI=buf, RDX=count → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 0));   // sys_read
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("read".to_string(), code));
    }

    // exit(code) → void  [syscall 60]
    // args: RDI=code → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 60));  // sys_exit
        code.extend(encode_syscall());                     // syscall
        // No ret — exit never returns.  Include INT3 as safety guard.
        code.extend(encode_int3());
        stubs.push(("exit".to_string(), code));
    }

    // sigaction(signum, act, oldact) → long  [syscall 13 = rt_sigaction]
    // Kernel signature: rt_sigaction(int signum, const struct sigaction *act,
    //                                 struct sigaction *oldact, size_t sigsetsize)
    // VUMA declares 3 args; the 4th (sigsetsize) must be 8 on x86_64.
    // args: RDI=signum, RSI=act, RDX=oldact → same as syscall for first 3
    // R10 must be set to 8 (sigsetsize) before the syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 13));  // sys_rt_sigaction
        code.extend(encode_mov_reg_imm32(Gpr::R10, 8));   // sigsetsize = 8
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("sigaction".to_string(), code));
    }

    // alarm(seconds) → unsigned int  [syscall 37]
    // args: RDI=seconds → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 37));  // sys_alarm
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("alarm".to_string(), code));
    }

    // pipe(int pipefd[2]) → int  [syscall 22]
    // args: RDI=pipefd → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 22));  // sys_pipe
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("pipe".to_string(), code));
    }

    // dup2(int oldfd, int newfd) → int  [syscall 33]
    // args: RDI=oldfd, RSI=newfd → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 33));  // sys_dup2
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("dup2".to_string(), code));
    }

    // getpid() → pid_t  [syscall 39]
    // args: none
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 39));  // sys_getpid
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("getpid".to_string(), code));
    }

    // fork() → pid_t  [syscall 57]
    // args: none
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 57));  // sys_fork
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("fork".to_string(), code));
    }

    // execve(const char *pathname, char *const argv[], char *const envp[]) → int  [syscall 59]
    // args: RDI=pathname, RSI=argv, RDX=envp → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 59));  // sys_execve
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("execve".to_string(), code));
    }

    // wait4(pid_t pid, int *wstatus, int options, struct rusage *rusage) → pid_t  [syscall 61]
    // VUMA declares: fn waitpid(pid: i64, status: Address, options: i64) -> i64;
    // args: RDI=pid, RSI=wstatus, RDX=options → same as syscall for first 3
    // R10 must be 0 (NULL rusage) before the syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 61));  // sys_wait4
        code.extend(encode_xor_reg_reg(Gpr::R10, Gpr::R10)); // rusage = NULL
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("waitpid".to_string(), code));
    }

    // strcmp(const char *s1, const char *s2) → int
    // Not a syscall — implemented as a small assembly loop.
    // Register usage: AL = byte from s1, CL = byte from s2,
    // RDI and RSI are advanced each iteration.
    {
        // .loop:
        //   8A 07           mov al, [rdi]
        //   8A 0E           mov cl, [rsi]
        //   38 C8           cmp al, cl
        //   75 0C           jne .done (+12)
        //   84 C0           test al, al
        //   74 08           jz .done (+8)
        //   48 FF C7        inc rdi
        //   48 FF C6        inc rsi
        //   EB EC           jmp .loop (-20)
        // .done:
        //   0F B6 C0        movzx eax, al
        //   0F B6 C9        movzx ecx, cl
        //   29 C8           sub eax, ecx
        //   C3              ret
        let code: Vec<u8> = vec![
            0x8A, 0x07,                         // mov al, [rdi]
            0x8A, 0x0E,                         // mov cl, [rsi]
            0x38, 0xC8,                         // cmp al, cl
            0x75, 0x0C,                         // jne .done (+12)
            0x84, 0xC0,                         // test al, al
            0x74, 0x08,                         // jz .done (+8)
            0x48, 0xFF, 0xC7,                   // inc rdi
            0x48, 0xFF, 0xC6,                   // inc rsi
            0xEB, 0xEC,                         // jmp .loop (-20)
            0x0F, 0xB6, 0xC0,                   // movzx eax, al
            0x0F, 0xB6, 0xC9,                   // movzx ecx, cl
            0x29, 0xC8,                         // sub eax, ecx
            0xC3,                               // ret
        ];
        stubs.push(("strcmp".to_string(), code));
    }

    // ── Network / epoll syscall stubs ────────────────────────────────────
    // These are needed by programs that use socket, epoll, etc.
    // On x86_64 Linux, the 4th argument differs between the SystemV calling
    // convention (RCX) and the syscall convention (R10), so any stub with
    // ≥4 args must move RCX → R10 before the syscall instruction.

    // socket(domain, type, protocol) → int  [syscall 41]
    // args: RDI=domain, RSI=type, RDX=protocol → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 41));  // sys_socket
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("socket".to_string(), code));
    }

    // setsockopt(sockfd, level, optname, optval, optlen) → int  [syscall 54]
    // args: RDI=sockfd, RSI=level, RDX=optname, RCX=optval, R8=optlen
    // syscall: RDI=sockfd, RSI=level, RDX=optname, R10=optval, R8=optlen
    // Need to move 4th arg from RCX → R10
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 54));  // sys_setsockopt
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX → R10
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("setsockopt".to_string(), code));
    }

    // bind(sockfd, addr, addrlen) → int  [syscall 49]
    // args: RDI=sockfd, RSI=addr, RDX=addrlen → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 49));  // sys_bind
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("bind".to_string(), code));
    }

    // listen(sockfd, backlog) → int  [syscall 50]
    // args: RDI=sockfd, RSI=backlog → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 50));  // sys_listen
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("listen".to_string(), code));
    }

    // accept(sockfd, addr, addrlen) → int  [syscall 43]
    // args: RDI=sockfd, RSI=addr, RDX=addrlen → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 43));  // sys_accept
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("accept".to_string(), code));
    }

    // epoll_create1(flags) → int  [syscall 291]
    // args: RDI=flags → same as syscall convention
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 291)); // sys_epoll_create1
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("epoll_create1".to_string(), code));
    }

    // epoll_ctl(epfd, op, fd, event) → int  [syscall 233]
    // args: RDI=epfd, RSI=op, RDX=fd, RCX=event
    // syscall: RDI=epfd, RSI=op, RDX=fd, R10=event
    // Need to move 4th arg from RCX → R10
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 233)); // sys_epoll_ctl
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX → R10
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("epoll_ctl".to_string(), code));
    }

    // epoll_wait(epfd, events, maxevents, timeout) → int  [syscall 232]
    // args: RDI=epfd, RSI=events, RDX=maxevents, RCX=timeout
    // syscall: RDI=epfd, RSI=events, RDX=maxevents, R10=timeout
    // Need to move 4th arg from RCX → R10
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 232)); // sys_epoll_wait
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX → R10
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_ret());                         // ret
        stubs.push(("epoll_wait".to_string(), code));
    }

    stubs
}

// ===========================================================================
// X86_64Backend
// ===========================================================================

/// x86_64 code generation backend (SystemV ABI).
pub struct X86_64Backend {
    target_info: X86_64TargetInfo,
}

impl X86_64Backend {
    /// Create a new x86_64 backend.
    pub fn new() -> Self {
        Self {
            target_info: X86_64TargetInfo,
        }
    }
}

impl Default for X86_64Backend {
    fn default() -> Self {
        Self::new()
    }
}

// ── x86_64 ELF Relocation Types ─────────────────────────────────────────

/// R_X86_64_64 — S + A, 64-bit absolute relocation.
const R_X86_64_64: &str = "R_X86_64_64";
/// R_X86_64_PLT32 — L + A - P, 32-bit PC-relative PLT relocation for calls/jumps.
const R_X86_64_PLT32: &str = "R_X86_64_PLT32";

// ── ISel helpers ─────────────────────────────────────────────────────────

/// Map an IR CmpKind to an x86_64 condition code.
fn cmp_kind_to_cc(kind: &CmpKind) -> Cc {
    match kind {
        CmpKind::Eq => Cc::Equal,
        CmpKind::Ne => Cc::NotEqual,
        CmpKind::SLt => Cc::Less,
        CmpKind::SLe => Cc::LessEqual,
        CmpKind::SGt => Cc::Greater,
        CmpKind::SGe => Cc::GreaterEqual,
        CmpKind::ULt => Cc::Below,
        CmpKind::ULe => Cc::BelowEqual,
        CmpKind::UGt => Cc::Above,
        CmpKind::UGe => Cc::AboveEqual,
    }
}

/// Map an IR BinOpKind comparison to an x86_64 condition code.
fn binop_cmp_to_cc(op: &BinOpKind) -> Cc {
    match op {
        BinOpKind::Eq => Cc::Equal,
        BinOpKind::Ne => Cc::NotEqual,
        BinOpKind::SLt => Cc::Less,
        BinOpKind::SLe => Cc::LessEqual,
        BinOpKind::SGt => Cc::Greater,
        BinOpKind::SGe => Cc::GreaterEqual,
        BinOpKind::ULt => Cc::Below,
        BinOpKind::ULe => Cc::BelowEqual,
        BinOpKind::UGt => Cc::Above,
        BinOpKind::UGe => Cc::AboveEqual,
        _ => Cc::Equal, // fallback, shouldn't be reached
    }
}

impl Backend for X86_64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }


    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        stack_slot_isel::allocate_registers(func)
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
        // Build the _start stub:
        //   mov rdi, [rsp]          ; argc = *RSP       (4 bytes with SIB)
        //   lea rsi, [rsp + 8]      ; argv = RSP + 8    (5 bytes with SIB)
        //   E8 <rel32 to main>      ; call main         (5 bytes)
        //   48 89 C7                ; mov rdi, rax      (3 bytes)
        //   48 C7 C0 3C 00 00 00   ; mov rax, 60       (7 bytes)
        //   0F 05                   ; syscall            (2 bytes)
        // Total = 4 + 5 + 5 + 3 + 7 + 2 = 26 bytes
        //
        // On Linux x86_64, the process entry stack layout is:
        //   [RSP]     = argc (8 bytes)
        //   [RSP+8]   = argv[0] pointer
        //   [RSP+16]  = argv[1] pointer
        //   ...
        //   NULL
        //   envp[0], envp[1], ..., NULL
        //   auxv...

        let start_stub_size: usize = 26;

        // Build runtime syscall stubs for common POSIX operations.
        // These are small functions that use the `syscall` instruction
        // directly, avoiding the need for libc linkage.
        let runtime_stubs = build_runtime_syscall_stubs();
        let runtime_stubs_total_size: usize = runtime_stubs.iter()
            .map(|(_, code)| code.len())
            .sum();

        // Compute offsets: _start stub → runtime stubs → user functions
        let mut func_offsets: HashMap<String, usize> = HashMap::new();
        let mut current_offset: usize = start_stub_size;

        // Runtime stubs come right after _start
        for (name, code) in &runtime_stubs {
            func_offsets.insert(name.clone(), current_offset);
            current_offset += code.len();
        }

        // User functions follow the runtime stubs
        for func in &program.functions {
            func_offsets.insert(func.name.clone(), current_offset);
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            current_offset += func_size;
        }

        // Build _start stub
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // mov rdi, [rsp] — load argc from top of stack
        start_stub.extend(encode_mov_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));

        // lea rsi, [rsp + 8] — argv starts at RSP + 8
        start_stub.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 8));

        // call main (E8 + rel32 placeholder)
        start_stub.extend(encode_call_rel32(0));

        // mov rdi, rax
        start_stub.extend(encode_mov_reg_reg(Gpr::Rdi, Gpr::Rax));

        // mov rax, 60 (sys_exit)
        start_stub.extend(encode_mov_reg_imm32(Gpr::Rax, 60));

        // syscall
        start_stub.extend(encode_syscall());

        // Patch the call main rel32 offset in _start stub
        // The mov rdi,[rsp] is 4 bytes, lea rsi,[rsp+8] is 5 bytes, then E8 at offset 9.
        // The rel32 is at offset 10 (after the E8 opcode byte at offset 9)
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            let rel32_patch_offset = 10usize; // offset within start_stub
            // rel32 = target - (call_site + 5)
            // call_site = offset of the E8 byte = 9
            let rel32 = (main_offset as i64) - (9i64 + 5i64);
            start_stub[rel32_patch_offset..rel32_patch_offset + 4]
                .copy_from_slice(&(rel32 as i32).to_le_bytes());
        }

        // Concatenate: _start stub → runtime stubs → user functions
        let mut all_code = start_stub;
        for (_, code) in &runtime_stubs {
            all_code.extend_from_slice(code);
        }
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }

        // ── Collect data symbols from R_X86_64_64 relocations ─────────
        // Data symbols (global variables from `allocate()` in VUMA source)
        // are referenced via R_X86_64_64 absolute 64-bit relocations.
        // They need addresses in a writable BSS segment.  We assign each
        // unique symbol a slot of 8 bytes (pointer-sized) in BSS.
        const BSS_SLOT_SIZE: u64 = 8;
        let mut data_symbols: Vec<String> = Vec::new();
        {
            let mut seen: HashSet<String> = HashSet::new();
            for func in &program.functions {
                for reloc in &func.relocations {
                    if reloc.reloc_type == R_X86_64_64
                        && !func_offsets.contains_key(&reloc.symbol)
                        && seen.insert(reloc.symbol.clone())
                    {
                        data_symbols.push(reloc.symbol.clone());
                    }
                }
            }
        }
        let bss_size: u64 = data_symbols.len() as u64 * BSS_SLOT_SIZE;

        // ── Compute BSS virtual address ──────────────────────────────
        // The BSS segment starts at the next page boundary after the text
        // segment.  The text segment layout is computed inside
        // build_minimal_x86_64_elf, so we mirror the same calculation here.
        const ELF_HEADER_SIZE: u64 = 64;
        const PHDR_SIZE: u64 = 56;
        const PAGE_SIZE: u64 = 0x1000;
        const BASE_ADDR: u64 = 0x400000;
        let num_phdrs: u64 = if bss_size > 0 { 2 } else { 1 };
        let phdr_end = ELF_HEADER_SIZE + PHDR_SIZE * num_phdrs;
        let text_offset = ((phdr_end + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE;
        let text_size = all_code.len() as u64;
        let bss_vaddr: u64 = if bss_size > 0 {
            ((BASE_ADDR + text_offset + text_size + PAGE_SIZE - 1) / PAGE_SIZE) * PAGE_SIZE
        } else {
            0
        };

        // Build a map: data symbol name → BSS virtual address
        let data_symbol_addrs: HashMap<String, u64> = data_symbols
            .iter()
            .enumerate()
            .map(|(i, name)| (name.clone(), bss_vaddr + i as u64 * BSS_SLOT_SIZE))
            .collect();

        // ── Patch relocations for each function ──────────────────────
        // We need to adjust relocation offsets: they are relative to the start
        // of the function's code, but now all_code has the _start stub, runtime
        // stubs, and preceding functions prepended.
        let mut func_code_offset: usize = start_stub_size + runtime_stubs_total_size;
        for func in &program.functions {
            for reloc in &func.relocations {
                let abs_offset = func_code_offset + reloc.offset as usize;

                if reloc.reloc_type == R_X86_64_PLT32 {
                    if abs_offset + 4 > all_code.len() {
                        continue; // skip invalid relocations
                    }
                    // R_X86_64_PLT32 for x86_64 CALL/JMP rel32:
                    // rel32 = S + A - P - 4
                    // S = symbol value (target address)
                    // A = addend (current value at the relocation site)
                    // P = place (address of the relocation site)
                    if let Some(&target_offset) = func_offsets.get(&reloc.symbol) {
                        let current_val = i32::from_le_bytes([
                            all_code[abs_offset],
                            all_code[abs_offset + 1],
                            all_code[abs_offset + 2],
                            all_code[abs_offset + 3],
                        ]);
                        let s = target_offset as i64;
                        let a = current_val as i64;
                        let p = abs_offset as i64;
                        let resolved = (s + a - p - 4) as i32;
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&resolved.to_le_bytes());
                    } else {
                        // External symbol — defer to the system linker.
                        // When compiled with `vuma compile --format obj`, the linker
                        // will resolve this relocation against libc or the runtime.
                        log::debug!(
                            "unresolved relocation: symbol '{}' in '{}' at 0x{:X} (type: {}) — deferring to linker",
                            reloc.symbol, func.name, reloc.offset, reloc.reloc_type
                        );
                        continue;
                    }
                } else if reloc.reloc_type == R_X86_64_64 {
                    // R_X86_64_64 — absolute 64-bit address relocation.
                    // Used by GetAddress to load the address of a data symbol.
                    if abs_offset + 8 > all_code.len() {
                        continue; // skip invalid relocations
                    }
                    if let Some(&addr) = data_symbol_addrs.get(&reloc.symbol) {
                        all_code[abs_offset..abs_offset + 8]
                            .copy_from_slice(&addr.to_le_bytes());
                    } else if func_offsets.contains_key(&reloc.symbol) {
                        // Function symbol with absolute relocation — patch with
                        // the function's virtual address (text_offset + offset).
                        let func_addr = BASE_ADDR + text_offset + func_offsets[&reloc.symbol] as u64;
                        all_code[abs_offset..abs_offset + 8]
                            .copy_from_slice(&func_addr.to_le_bytes());
                    } else {
                        log::debug!(
                            "unresolved R_X86_64_64 relocation: symbol '{}' in '{}' at 0x{:X} — deferring to linker",
                            reloc.symbol, func.name, reloc.offset
                        );
                    }
                }
            }
            let func_size: usize = func.blocks.iter()
                .flat_map(|b| b.instructions.iter())
                .map(|i| i.encoded.len())
                .sum();
            func_code_offset += func_size;
        }

        Ok(build_minimal_x86_64_elf(&all_code, BASE_ADDR, bss_size))
    }

    fn return_stub(&self) -> Vec<u8> {
        // xor eax, eax; ret
        vec![0x31, 0xC0, 0xC3]
    }

    fn trampoline(&self, entry_addr: u64) -> Vec<u8> {
        // mov rax, imm64; jmp rax
        let mut code = vec![0x48, 0xB8]; // REX.W + MOV RAX, imm64
        code.extend_from_slice(&entry_addr.to_le_bytes());
        code.extend_from_slice(&[0xFF, 0xE0]); // JMP RAX
        code
    }

    fn disassemble(&self, bytes: &[u8], addr: u64) -> Vec<String> {
        disassemble_x86_64_mnemonic(bytes, addr)
    }

    fn name(&self) -> &'static str {
        "x86_64"
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CastKind, IRInstr, IRType, IRValue, UnaryOpKind};

    // ── REX Prefix Tests ────────────────────────────────────────────────

    #[test]
    fn test_rex_prefix_no_bits() {
        // No REX needed when all bits are 0
        assert_eq!(rex_prefix(false, false, false, false), None);
    }

    #[test]
    fn test_rex_prefix_w_only() {
        // REX.W only: 0x48
        assert_eq!(rex_prefix(true, false, false, false), Some(0x48));
    }

    #[test]
    fn test_rex_prefix_r_only() {
        // REX.R only: 0x44
        assert_eq!(rex_prefix(false, true, false, false), Some(0x44));
    }

    #[test]
    fn test_rex_prefix_x_only() {
        // REX.X only: 0x42
        assert_eq!(rex_prefix(false, false, true, false), Some(0x42));
    }

    #[test]
    fn test_rex_prefix_b_only() {
        // REX.B only: 0x41
        assert_eq!(rex_prefix(false, false, false, true), Some(0x41));
    }

    #[test]
    fn test_rex_prefix_wrb() {
        // REX.WRB: 0x4D
        assert_eq!(rex_prefix(true, true, false, true), Some(0x4D));
    }

    #[test]
    fn test_rex_prefix_all() {
        // All bits: 0x4F
        assert_eq!(rex_prefix(true, true, true, true), Some(0x4F));
    }

    // ── ModR/M Tests ────────────────────────────────────────────────────

    #[test]
    fn test_modrm_reg_reg() {
        // mod=3, reg=RAX(0), rm=RCX(1) => 0xC1
        assert_eq!(modrm(3, 0, 1), 0xC1);
    }

    #[test]
    fn test_modrm_mem_disp8() {
        // mod=1, reg=RAX(0), rm=RBX(3) => 0x43
        assert_eq!(modrm(1, 0, 3), 0x43);
    }

    #[test]
    fn test_modrm_mem_no_disp() {
        // mod=0, reg=RCX(1), rm=RDX(2) => 0x0A
        assert_eq!(modrm(0, 1, 2), 0x0A);
    }

    #[test]
    fn test_modrm_mem_disp32() {
        // mod=2, reg=RSI(6), rm=RDI(7) => 0xB7
        assert_eq!(modrm(2, 6, 7), 0xB7);
    }

    // ── SIB Tests ───────────────────────────────────────────────────────

    #[test]
    fn test_sib_basic() {
        // scale=0, index=RAX(0), base=RCX(1) => 0x01
        assert_eq!(sib(0, 0, 1), 0x01);
    }

    #[test]
    fn test_sib_scale2_index3_base5() {
        // scale=1, index=RBX(3), base=RBP(5): (1<<6)|(3<<3)|5 = 0x5D
        assert_eq!(sib(1, 3, 5), 0x5D);
    }

    // ── MOV Reg-Reg Tests ──────────────────────────────────────────────

    #[test]
    fn test_mov_rax_rcx() {
        // MOV RCX, RAX => REX.W + 89 /r with src=RAX, dst=RCX
        let code = encode_mov_reg_reg(Gpr::Rcx, Gpr::Rax);
        assert_eq!(code, vec![0x48, 0x89, 0xC1]);
    }

    #[test]
    fn test_mov_rax_r8() {
        // MOV R8, RAX => REX.WB + 89 /r (src=RAX in reg field, dst=R8 in rm field with REX.B)
        let code = encode_mov_reg_reg(Gpr::R8, Gpr::Rax);
        assert_eq!(code, vec![0x49, 0x89, 0xC0]);
    }

    #[test]
    fn test_mov_r9_r15() {
        // MOV R15, R9 => REX.WRB + 89 /r
        let code = encode_mov_reg_reg(Gpr::R15, Gpr::R9);
        assert_eq!(code, vec![0x4D, 0x89, 0xCF]);
    }

    // ── MOV Reg-Imm64 Tests ────────────────────────────────────────────

    #[test]
    fn test_mov_rax_imm64() {
        let code = encode_mov_reg_imm64(Gpr::Rax, 0xDEADBEEFCAFE0000);
        assert_eq!(code[0], 0x48); // REX.W
        assert_eq!(code[1], 0xB8); // MOV RAX, imm64
        assert_eq!(&code[2..10], 0xDEADBEEFCAFE0000u64.to_le_bytes());
    }

    #[test]
    fn test_mov_r8_imm64() {
        let code = encode_mov_reg_imm64(Gpr::R8, 0x1234);
        assert_eq!(code[0], 0x49); // REX.WB
        assert_eq!(code[1], 0xB8); // MOV R8, imm64
    }

    // ── MOV Reg-Imm32 Tests ────────────────────────────────────────────

    #[test]
    fn test_mov_rcx_imm32() {
        let code = encode_mov_reg_imm32(Gpr::Rcx, 42);
        assert_eq!(code, vec![0x48, 0xC7, 0xC1, 0x2A, 0x00, 0x00, 0x00]);
    }

    // ── ADD/SUB Tests ──────────────────────────────────────────────────

    #[test]
    fn test_add_rax_rcx() {
        let code = encode_add_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x01, 0xC8]);
    }

    #[test]
    fn test_sub_rdx_rsi() {
        let code = encode_sub_reg_reg(Gpr::Rdx, Gpr::Rsi);
        assert_eq!(code, vec![0x48, 0x29, 0xF2]);
    }

    #[test]
    fn test_add_r8_r9() {
        let code = encode_add_reg_reg(Gpr::R8, Gpr::R9);
        assert_eq!(code, vec![0x4D, 0x01, 0xC8]);
    }

    // ── IMUL Tests ─────────────────────────────────────────────────────

    #[test]
    fn test_imul_rax_rcx() {
        let code = encode_imul_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x0F, 0xAF, 0xC1]);
    }

    #[test]
    fn test_imul_r8_r15() {
        let code = encode_imul_reg_reg(Gpr::R8, Gpr::R15);
        assert_eq!(code, vec![0x4D, 0x0F, 0xAF, 0xC7]);
    }

    // ── IDIV Test ──────────────────────────────────────────────────────

    #[test]
    fn test_idiv_rcx() {
        let code = encode_idiv_reg(Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0xF7, 0xF9]);
    }

    // ── CMP Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_cmp_rax_rcx() {
        let code = encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x39, 0xC8]);
    }

    #[test]
    fn test_cmp_reg_imm32() {
        let code = encode_cmp_reg_imm32(Gpr::Rax, 100);
        assert_eq!(code[0], 0x48); // REX.W
        assert_eq!(code[1], 0x81); // CMP r/m64, imm32
        assert_eq!(code[2], 0xF8); // mod=3, reg=7(/7), rm=RAX(0)
        let imm = i32::from_le_bytes([code[3], code[4], code[5], code[6]]);
        assert_eq!(imm, 100);
    }

    // ── TEST Test ──────────────────────────────────────────────────────

    #[test]
    fn test_test_rax_rax() {
        let code = encode_test_reg_reg(Gpr::Rax, Gpr::Rax);
        assert_eq!(code, vec![0x48, 0x85, 0xC0]);
    }

    // ── AND/OR/XOR Tests ──────────────────────────────────────────────

    #[test]
    fn test_and_rax_rcx() {
        let code = encode_and_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x21, 0xC8]);
    }

    #[test]
    fn test_or_rdx_rsi() {
        let code = encode_or_reg_reg(Gpr::Rdx, Gpr::Rsi);
        assert_eq!(code, vec![0x48, 0x09, 0xF2]);
    }

    #[test]
    fn test_xor_rax_rax() {
        let code = encode_xor_reg_reg(Gpr::Rax, Gpr::Rax);
        assert_eq!(code, vec![0x48, 0x31, 0xC0]);
    }

    // ── Shift Tests ────────────────────────────────────────────────────

    #[test]
    fn test_shl_cl() {
        let code = encode_shl_reg_cl(Gpr::Rax);
        assert_eq!(code, vec![0x48, 0xD3, 0xE0]);
    }

    #[test]
    fn test_shr_cl() {
        // SHR RCX, CL => REX.W + D3 /5 + ModRM(3,5,1) = 0xE9
        let code = encode_shr_reg_cl(Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0xD3, 0xE9]);
    }

    #[test]
    fn test_sar_cl() {
        let code = encode_sar_reg_cl(Gpr::Rdx);
        assert_eq!(code, vec![0x48, 0xD3, 0xFA]);
    }

    // ── JMP/CALL/RET Tests ─────────────────────────────────────────────

    #[test]
    fn test_jmp_rel32() {
        let code = encode_jmp_rel32(0x100);
        assert_eq!(code[0], 0xE9);
        assert_eq!(&code[1..5], 0x100i32.to_le_bytes());
    }

    #[test]
    fn test_call_rel32() {
        let code = encode_call_rel32(-16);
        assert_eq!(code[0], 0xE8);
        assert_eq!(&code[1..5], (-16i32).to_le_bytes());
    }

    #[test]
    fn test_ret() {
        assert_eq!(encode_ret(), vec![0xC3]);
    }

    // ── NOP Test ───────────────────────────────────────────────────────

    #[test]
    fn test_nop() {
        assert_eq!(encode_nop(), vec![0x90]);
    }

    // ── PUSH/POP Tests ─────────────────────────────────────────────────

    #[test]
    fn test_push_rax() {
        assert_eq!(encode_push(Gpr::Rax), vec![0x50]);
    }

    #[test]
    fn test_push_r8() {
        assert_eq!(encode_push(Gpr::R8), vec![0x41, 0x50]);
    }

    #[test]
    fn test_pop_rbx() {
        assert_eq!(encode_pop(Gpr::Rbx), vec![0x5B]);
    }

    #[test]
    fn test_pop_r15() {
        assert_eq!(encode_pop(Gpr::R15), vec![0x41, 0x5F]);
    }

    // ── SETcc Tests ────────────────────────────────────────────────────

    #[test]
    fn test_sete_al() {
        let code = encode_setcc(Cc::Equal, Gpr::Rax);
        assert_eq!(code, vec![0x0F, 0x94, 0xC0]);
    }

    #[test]
    fn test_setl_r8b() {
        let code = encode_setcc(Cc::Less, Gpr::R8);
        assert_eq!(code, vec![0x41, 0x0F, 0x9C, 0xC0]);
    }

    // ── Jcc Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_je_rel32() {
        let code = encode_jcc_rel32(Cc::Equal, 0x20);
        assert_eq!(code[0], 0x0F);
        assert_eq!(code[1], 0x84);
        assert_eq!(&code[2..6], 0x20i32.to_le_bytes());
    }

    #[test]
    fn test_jl_rel32() {
        let code = encode_jcc_rel32(Cc::Less, -8);
        assert_eq!(code[0], 0x0F);
        assert_eq!(code[1], 0x8C);
        assert_eq!(&code[2..6], (-8i32).to_le_bytes());
    }

    // ── CMOVcc Tests ───────────────────────────────────────────────────

    #[test]
    fn test_cmove_rax_rcx() {
        let code = encode_cmovcc_reg_reg(Cc::Equal, Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x0F, 0x44, 0xC1]);
    }

    // ── LEA Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_lea_rax_rbp_offset8() {
        let code = encode_lea_reg_mem(Gpr::Rax, Gpr::Rbp, 8);
        assert_eq!(code, vec![0x48, 0x8D, 0x45, 0x08]);
    }

    #[test]
    fn test_lea_rax_rsp_offset0() {
        // RSP as base requires SIB byte
        let code = encode_lea_reg_mem(Gpr::Rax, Gpr::Rsp, 0);
        assert_eq!(code, vec![0x48, 0x8D, 0x04, 0x24]);
    }

    // ── MOVZX/MOVSX Tests ──────────────────────────────────────────────

    #[test]
    fn test_movzx_reg8() {
        let code = encode_movzx_reg8(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x0F, 0xB6, 0xC1]);
    }

    #[test]
    fn test_movsx_reg8() {
        let code = encode_movsx_reg8(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x0F, 0xBE, 0xC1]);
    }

    #[test]
    fn test_movsxd() {
        let code = encode_movsxd(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x63, 0xC1]);
    }

    // ── XCHG Test ──────────────────────────────────────────────────────

    #[test]
    fn test_xchg_rax_rcx() {
        let code = encode_xchg_rax_reg(Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x91]);
    }

    // ── SYSCALL/INT3 Tests ─────────────────────────────────────────────

    #[test]
    fn test_syscall() {
        assert_eq!(encode_syscall(), vec![0x0F, 0x05]);
    }

    #[test]
    fn test_int3() {
        assert_eq!(encode_int3(), vec![0xCC]);
    }

    // ── Gpr Properties Tests ───────────────────────────────────────────

    #[test]
    fn test_gpr_encoding() {
        assert_eq!(Gpr::Rax.encoding(), 0);
        assert_eq!(Gpr::Rdi.encoding(), 7);
        assert_eq!(Gpr::R8.encoding(), 8);
        assert_eq!(Gpr::R15.encoding(), 15);
    }

    #[test]
    fn test_gpr_needs_rex() {
        assert!(!Gpr::Rax.needs_rex());
        assert!(!Gpr::Rdi.needs_rex());
        assert!(Gpr::R8.needs_rex());
        assert!(Gpr::R15.needs_rex());
    }

    #[test]
    fn test_gpr_callee_saved() {
        assert!(Gpr::Rbx.is_callee_saved());
        assert!(Gpr::Rbp.is_callee_saved());
        assert!(Gpr::R12.is_callee_saved());
        assert!(!Gpr::Rax.is_callee_saved());
        assert!(!Gpr::Rdi.is_callee_saved());
    }

    #[test]
    fn test_gpr_arg_regs() {
        assert!(Gpr::Rdi.is_arg_reg());
        assert!(Gpr::R9.is_arg_reg());
        assert!(!Gpr::Rax.is_arg_reg());
        assert!(!Gpr::R10.is_arg_reg());
    }

    #[test]
    fn test_gpr_allocatable() {
        assert!(Gpr::Rax.is_allocatable());
        assert!(!Gpr::Rsp.is_allocatable());
    }

    #[test]
    fn test_gpr_arg_register() {
        assert_eq!(Gpr::arg_register(0), Some(Gpr::Rdi));
        assert_eq!(Gpr::arg_register(5), Some(Gpr::R9));
        assert_eq!(Gpr::arg_register(6), None);
    }

    // ── Return Stub Test ───────────────────────────────────────────────

    #[test]
    fn test_return_stub() {
        let backend = X86_64Backend::new();
        let stub = backend.return_stub();
        // xor eax, eax; ret
        assert_eq!(stub, vec![0x31, 0xC0, 0xC3]);
    }

    // ── Trampoline Test ────────────────────────────────────────────────

    #[test]
    fn test_trampoline() {
        let backend = X86_64Backend::new();
        let tramp = backend.trampoline(0x7FFFF7000000);
        // mov rax, imm64; jmp rax
        assert_eq!(tramp[0], 0x48); // REX.W
        assert_eq!(tramp[1], 0xB8); // MOV RAX, imm64
        assert_eq!(&tramp[2..10], 0x7FFFF7000000u64.to_le_bytes());
        assert_eq!(&tramp[10..12], &[0xFF, 0xE0]); // JMP RAX
    }

    // ── ELF Header Validation Test ─────────────────────────────────────

    #[test]
    fn test_elf_header() {
        let code = encode_ret();
        let elf = build_minimal_x86_64_elf(&code, 0x400000, 0);

        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // ELFCLASS64
        assert_eq!(elf[4], 2);
        // ELFDATA2LSB
        assert_eq!(elf[5], 1);
        // e_type = ET_EXEC (2)
        assert_eq!(u16::from_le_bytes([elf[16], elf[17]]), 2);
        // e_machine = EM_X86_64 (62)
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 62);
        // With 1 phdr (no BSS), e_phnum = 1
        assert_eq!(u16::from_le_bytes([elf[56], elf[57]]), 1);
        // entry = base + page_align(64 + 56) = 0x400000 + 0x1000 = 0x401000
        let entry = u64::from_le_bytes([
            elf[24], elf[25], elf[26], elf[27], elf[28], elf[29], elf[30], elf[31],
        ]);
        assert_eq!(entry, 0x401000);
    }

    #[test]
    fn test_elf_header_with_bss() {
        let code = encode_ret();
        let elf = build_minimal_x86_64_elf(&code, 0x400000, 16); // 16 bytes of BSS

        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // e_type = ET_EXEC
        assert_eq!(u16::from_le_bytes([elf[16], elf[17]]), 2);
        // e_machine = EM_X86_64
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 62);
        // With BSS, e_phnum = 2
        assert_eq!(u16::from_le_bytes([elf[56], elf[57]]), 2);
        // Entry point is still in text segment
        let entry = u64::from_le_bytes([
            elf[24], elf[25], elf[26], elf[27], elf[28], elf[29], elf[30], elf[31],
        ]);
        // With 2 phdrs, text_offset = page_align(64 + 2*56) = page_align(176) = 0x1000
        // entry = 0x400000 + 0x1000 = 0x401000
        assert_eq!(entry, 0x401000);

        // Second program header (BSS) starts at offset 64 + 56 = 120
        // Elf64_Phdr layout: p_type(4) p_flags(4) p_offset(8) p_vaddr(8) p_paddr(8) p_filesz(8) p_memsz(8) p_align(8)
        let ph2 = 64 + 56;
        let p_type = u32::from_le_bytes([elf[ph2], elf[ph2+1], elf[ph2+2], elf[ph2+3]]);
        assert_eq!(p_type, 1); // PT_LOAD
        let p_flags = u32::from_le_bytes([elf[ph2+4], elf[ph2+5], elf[ph2+6], elf[ph2+7]]);
        assert_eq!(p_flags, 6); // PF_R | PF_W
        let p_filesz = u64::from_le_bytes([
            elf[ph2+32], elf[ph2+33], elf[ph2+34], elf[ph2+35],
            elf[ph2+36], elf[ph2+37], elf[ph2+38], elf[ph2+39],
        ]);
        assert_eq!(p_filesz, 0); // BSS has no file content
        let p_memsz = u64::from_le_bytes([
            elf[ph2+40], elf[ph2+41], elf[ph2+42], elf[ph2+43],
            elf[ph2+44], elf[ph2+45], elf[ph2+46], elf[ph2+47],
        ]);
        assert_eq!(p_memsz, 16);
        let bss_vaddr = u64::from_le_bytes([
            elf[ph2+16], elf[ph2+17], elf[ph2+18], elf[ph2+19],
            elf[ph2+20], elf[ph2+21], elf[ph2+22], elf[ph2+23],
        ]);
        // BSS vaddr should be page-aligned and after the text segment
        assert_eq!(bss_vaddr % 0x1000, 0, "BSS vaddr should be page-aligned");
        assert!(bss_vaddr > 0x401000, "BSS should be after text segment");
    }

    // ── Backend Trait Dispatch Test ─────────────────────────────────────

    #[test]
    fn test_backend_trait_dispatch() {
        let backend: Box<dyn Backend> = Box::new(X86_64Backend::new());
        assert_eq!(backend.name(), "x86_64");
        assert_eq!(backend.target_info().isa_name(), "x86_64");
        assert_eq!(backend.target_info().elf_machine_type(), 62);
        assert_eq!(backend.target_info().calling_convention_name(), "systemv");
    }

    // ── Backend TargetInfo Consistency Test ─────────────────────────────

    #[test]
    fn test_target_info_consistency() {
        let backend = X86_64Backend::new();
        let info = backend.target_info();
        assert_eq!(info.pointer_width(), 8);
        assert_eq!(info.num_gp_regs(), 16);
        assert_eq!(info.num_simd_fp_regs(), 16);
        assert!(!info.has_hardwired_zero());
        assert!(!info.has_link_register());
        assert_eq!(info.stack_alignment(), 16);
        assert_eq!(info.instruction_alignment(), 1);
        assert_eq!(info.instruction_width_range(), (1, 15));
        assert_eq!(info.num_int_arg_regs(), 6);
        assert_eq!(info.num_fp_arg_regs(), 8);
    }

    // ── MOV [mem] Tests ────────────────────────────────────────────────

    #[test]
    fn test_mov_reg_mem_offset8() {
        let code = encode_mov_reg_mem(Gpr::Rax, Gpr::Rbx, 8);
        assert_eq!(code, vec![0x48, 0x8B, 0x43, 0x08]);
    }

    #[test]
    fn test_mov_reg_mem_offset0_rbp() {
        // RBP with offset 0 requires mod=01 with disp8=0
        let code = encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, 0);
        assert_eq!(code, vec![0x48, 0x8B, 0x45, 0x00]);
    }

    #[test]
    fn test_mov_reg_mem_rsp_sib() {
        // RSP as base requires SIB byte
        let code = encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 0);
        assert_eq!(code, vec![0x48, 0x8B, 0x04, 0x24]);
    }

    #[test]
    fn test_mov_mem_reg_offset8() {
        let code = encode_mov_mem_reg(Gpr::Rbx, 8, Gpr::Rax);
        assert_eq!(code, vec![0x48, 0x89, 0x43, 0x08]);
    }

    // ── CQO Test ───────────────────────────────────────────────────────

    #[test]
    fn test_cqo() {
        assert_eq!(encode_cqo(), vec![0x48, 0x99]);
    }

    // ── NEG/NOT Tests ──────────────────────────────────────────────────

    #[test]
    fn test_neg_rax() {
        let code = encode_neg_reg(Gpr::Rax);
        assert_eq!(code, vec![0x48, 0xF7, 0xD8]);
    }

    #[test]
    fn test_not_rcx() {
        let code = encode_not_reg(Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0xF7, 0xD1]);
    }

    // ── MOVZX r16 Test ─────────────────────────────────────────────────

    #[test]
    fn test_movzx_reg16() {
        let code = encode_movzx_reg16(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x0F, 0xB7, 0xC1]);
    }

    // ── ADD/SUB imm32 Tests ────────────────────────────────────────────

    #[test]
    fn test_sub_reg_imm32() {
        let code = encode_sub_reg_imm32(Gpr::Rsp, 32);
        assert_eq!(code[0], 0x48); // REX.W
        assert_eq!(code[1], 0x81); // SUB r/m64, imm32
        assert_eq!(code[2], 0xEC); // mod=3, /5, rm=RSP(4)
    }

    // ── Disassemble Test ───────────────────────────────────────────────

    #[test]
    fn test_disassemble_ret() {
        let backend = X86_64Backend::new();
        let bytes = encode_ret();
        let lines = backend.disassemble(&bytes, 0x400000);
        assert!(!lines.is_empty());
        assert!(lines[0].contains("400000"));
        assert!(lines[0].contains("c3"));
    }

    // ── MOVSX r16 Test ─────────────────────────────────────────────────

    #[test]
    fn test_movsx_reg16_rax_rcx() {
        let code = encode_movsx_reg16(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x48, 0x0F, 0xBF, 0xC1]);
    }

    #[test]
    fn test_movsx_reg16_r8_r9() {
        let code = encode_movsx_reg16(Gpr::R8, Gpr::R9);
        assert_eq!(code, vec![0x4D, 0x0F, 0xBF, 0xC1]);
    }

    // ── ISel Tests (full allocate_registers pipeline) ──────────────────

    /// Helper: build a minimal IR function with a single instruction and
    /// a Ret, then run allocate_registers and return the encoded bytes
    /// for the instruction (skipping prologue).
    fn isel_single_instr(instr: IRInstr) -> Vec<u8> {
        let mut func = IRFunction::new("test");
        // vreg 0 = dst, vreg 1 = lhs (if any), vreg 2 = rhs (if any)
        func.current_block().instructions.push(instr);
        func.current_block().instructions.push(IRInstr::Ret {
            values: vec![IRValue::Register(0)],
        });
        let backend = X86_64Backend::new();
        let allocated = backend.allocate_registers(&func).unwrap();
        // The encoded bytes include prologue + instruction(s) + epilogue.
        // Concatenate all instructions and return the full encoded output.
        let mut bytes = Vec::new();
        for block in &allocated.blocks {
            for instr in &block.instructions {
                bytes.extend_from_slice(&instr.encoded);
            }
        }
        bytes
    }

    #[test]
    fn test_isel_add_reg_reg() {
        let code = isel_single_instr(IRInstr::Add {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: None,
        });
        // Should contain an ADD r64, r64 instruction (opcode 0x01)
        assert!(
            code.iter().any(|&b| b == 0x01),
            "ADD opcode 0x01 not found in encoded output"
        );
    }

    #[test]
    fn test_isel_add_imm32() {
        let code = isel_single_instr(IRInstr::Add {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(42),
            ty: None,
        });
        // With immediate rhs, should use ADD r64, imm32 (opcode 0x81 /0)
        let has_add_imm = code
            .windows(2)
            .any(|w| w[0] == 0x81 && (w[1] & 0xC0) == 0xC0 && (w[1] & 0x38) == 0x00);
        assert!(has_add_imm, "ADD r64, imm32 not found in encoded output");
    }

    #[test]
    fn test_isel_sub_reg_reg() {
        let code = isel_single_instr(IRInstr::Sub {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: None,
        });
        // Should contain SUB r64, r64 (opcode 0x29)
        assert!(code.iter().any(|&b| b == 0x29), "SUB opcode 0x29 not found");
    }

    #[test]
    fn test_isel_sub_imm32() {
        let code = isel_single_instr(IRInstr::Sub {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(10),
            ty: None,
        });
        // With immediate, should use SUB r64, imm32 (0x81 /5)
        let has_sub_imm = code
            .windows(2)
            .any(|w| w[0] == 0x81 && (w[1] & 0xC0) == 0xC0 && (w[1] & 0x38) == 0x28);
        assert!(has_sub_imm, "SUB r64, imm32 not found");
    }

    #[test]
    fn test_isel_binop_and() {
        let code = isel_single_instr(IRInstr::BinOp {
            op: BinOpKind::And,
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: None,
        });
        // AND r64, r64 (opcode 0x21)
        assert!(code.iter().any(|&b| b == 0x21), "AND opcode 0x21 not found");
    }

    #[test]
    fn test_isel_binop_xor() {
        let code = isel_single_instr(IRInstr::BinOp {
            op: BinOpKind::Xor,
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: None,
        });
        // XOR r64, r64 (opcode 0x31)
        assert!(code.iter().any(|&b| b == 0x31), "XOR opcode 0x31 not found");
    }

    #[test]
    fn test_isel_binop_sdiv() {
        let code = isel_single_instr(IRInstr::BinOp {
            op: BinOpKind::SDiv,
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
            ty: None,
        });
        // SDiv uses CQO (0x48 0x99) + IDIV (0xF7 /7)
        assert!(
            code.windows(2).any(|w| w[0] == 0x48 && w[1] == 0x99),
            "CQO not found for SDiv"
        );
        assert!(
            code.iter().any(|&b| b == 0xF7),
            "IDIV opcode not found for SDiv"
        );
    }

    #[test]
    fn test_isel_unaryop_neg() {
        let code = isel_single_instr(IRInstr::UnaryOp {
            op: UnaryOpKind::Neg,
            dst: IRValue::Register(0),
            operand: IRValue::Register(1),
            ty: None,
        });
        // NEG r64 (0xF7 /3)
        assert!(code.iter().any(|&b| b == 0xF7), "NEG opcode 0xF7 not found");
    }

    #[test]
    fn test_isel_unaryop_not() {
        let code = isel_single_instr(IRInstr::UnaryOp {
            op: UnaryOpKind::Not,
            dst: IRValue::Register(0),
            operand: IRValue::Register(1),
            ty: None,
        });
        // NOT r64 (0xF7 /2)
        assert!(code.iter().any(|&b| b == 0xF7), "NOT opcode 0xF7 not found");
    }

    #[test]
    fn test_isel_cmp_imm32() {
        let code = isel_single_instr(IRInstr::Cmp {
            kind: CmpKind::Eq,
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(5),
            ty: None,
        });
        // CMP r64, imm32 (0x81 /7)
        let has_cmp_imm = code
            .windows(2)
            .any(|w| w[0] == 0x81 && (w[1] & 0xC0) == 0xC0 && (w[1] & 0x38) == 0x38);
        assert!(has_cmp_imm, "CMP r64, imm32 not found");
        // Should also have SETcc (0F 9x) and MOVZX (0F B6)
        assert!(
            code.windows(2)
                .any(|w| w[0] == 0x0F && w[1] >= 0x90 && w[1] <= 0x9F),
            "SETcc not found"
        );
    }

    #[test]
    fn test_isel_cast_zext() {
        let code = isel_single_instr(IRInstr::Cast {
            kind: CastKind::ZExt,
            dst: IRValue::Register(0),
            src: IRValue::Register(1),
            from_ty: None,
            to_ty: None,
        });
        // ZExt of a register uses MOVZX r8→r64 (0F B6)
        assert!(
            code.windows(2).any(|w| w[0] == 0x0F && w[1] == 0xB6),
            "MOVZX r8 not found for ZExt"
        );
    }

    #[test]
    fn test_isel_cast_sext() {
        let code = isel_single_instr(IRInstr::Cast {
            kind: CastKind::SExt,
            dst: IRValue::Register(0),
            src: IRValue::Register(1),
            from_ty: None,
            to_ty: None,
        });
        // SExt of a register uses MOVSX r8→r64 (0F BE)
        assert!(
            code.windows(2).any(|w| w[0] == 0x0F && w[1] == 0xBE),
            "MOVSX r8 not found for SExt"
        );
    }

    #[test]
    fn test_isel_select() {
        let code = isel_single_instr(IRInstr::Select {
            dst: IRValue::Register(0),
            cond: IRValue::Register(1),
            true_val: IRValue::Register(2),
            false_val: IRValue::Register(3),
            ty: None,
        });
        // Select uses TEST + CMOVcc.
        //
        // The stack-slot isel (src/codegen/src/x86_64/stack_slot_isel.rs:640)
        // lowers Select as: load false_val->RAX, true_val->R10, cond->R11;
        // `TEST R11, R11` then `CMOVNZ RAX, R10`.
        //
        // R11 is in the high register file (R8-R15), so its encoding requires
        // REX.R and REX.B extensions on top of REX.W. The resulting REX prefix
        // for `TEST R11, R11` is therefore 0x4D (REX.WRB), not 0x48 (REX.W
        // only). The CMOVcc opcode byte (0x0F 0x45 for CMOVNZ) is unaffected.
        //
        // Accept any REX.W+TEST encoding (REX byte 0x48..=0x4F followed by the
        // TEST r/m64, r64 opcode 0x85) so the assertion matches the actual
        // isel output regardless of which scratch register holds `cond`.
        assert!(
            code.windows(2)
                .any(|w| (w[0] >= 0x48 && w[0] <= 0x4F) && w[1] == 0x85),
            "TEST (REX.W + 0x85) not found for Select"
        );
        assert!(
            code.windows(2)
                .any(|w| w[0] == 0x0F && w[1] >= 0x40 && w[1] <= 0x4F),
            "CMOVcc not found for Select"
        );
    }

    // ── Disassembler Tests ───────────────────────────────────────────

    #[test]
    fn test_x86_64_disassemble_nop() {
        let backend = X86_64Backend::new();
        let bytes = encode_nop();
        let lines = backend.disassemble(&bytes, 0x1000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("nop"), "Expected nop, got: {}", lines[0]);
    }

    #[test]
    fn test_x86_64_disassemble_ret() {
        let backend = X86_64Backend::new();
        let bytes = encode_ret();
        let lines = backend.disassemble(&bytes, 0x1000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("ret"), "Expected ret, got: {}", lines[0]);
    }

    #[test]
    fn test_x86_64_disassemble_push_pop() {
        let backend = X86_64Backend::new();
        let mut bytes = Vec::new();
        bytes.extend(encode_push(Gpr::Rbp));
        bytes.extend(encode_pop(Gpr::Rbp));
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("push"),
            "Expected push, got: {}",
            lines[0]
        );
        assert!(lines[1].contains("pop"), "Expected pop, got: {}", lines[1]);
    }

    #[test]
    fn test_x86_64_disassemble_mov_reg_reg() {
        let backend = X86_64Backend::new();
        let bytes = encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp);
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("mov"), "Expected mov, got: {}", lines[0]);
    }

    #[test]
    fn test_x86_64_disassemble_add_sub() {
        let backend = X86_64Backend::new();
        let mut bytes = Vec::new();
        bytes.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rcx));
        bytes.extend(encode_sub_reg_reg(Gpr::Rax, Gpr::Rcx));
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("add"), "Expected add, got: {}", lines[0]);
        assert!(lines[1].contains("sub"), "Expected sub, got: {}", lines[1]);
    }
}
pub mod disasm;
pub mod stack_slot_isel;
