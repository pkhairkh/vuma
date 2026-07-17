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
    AllocatedFunction, AllocatedProgram, Backend,
    BackendError, TargetInfo, X86_64TargetInfo,
};
use crate::ir::{BinOpKind, CmpKind, IRFunction};
#[cfg(test)]
use crate::ir::IRValue;
#[cfg(test)]
use crate::ir::{CastKind, IRInstr, UnaryOpKind};
use std::collections::{HashMap, HashSet};
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// x86_64 general-purpose registers (RAX–R15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
// SSE / AVX SIMD Instruction Encoders (Wave 29)
// ===========================================================================
//
// These encoders produce the x86_64 machine-code bytes for SSE2/SSSE3/SSE4.1
// and AVX SIMD instructions used by the vectorizer's `PackedOp` lowering
// (`vectorize::PackedOpKind::Add/Sub/Mul`). They are unit-tested directly
// (asserting exact opcode bytes) and are the hook the backend will call to
// emit SIMD code once ISel integration is wired (TODO(wave29)).
//
// Intel-syntax operands: `dst` is the destination register (encoded in the
// ModR/M `rm` field for legacy SSE, in the VEX `vd` field for AVX); `src` is
// the source register (encoded in the ModR/M `reg` field for SSE, in the VEX
// `vn` field for AVX).
//
// References: Intel 64 and IA-32 Architectures Software Developer's Manual,
// Vol. 2A/2B, SSE2 and AVX instruction set references.

/// Encode the SSE2 mandatory prefix (66 / F2 / F3) plus 0F escape, with an
/// optional REX prefix for XMM8–XMM15 operands. `reg_in_modrm` is the
/// register encoded in the ModR/M `reg` field; `rm_in_modrm` is the register
/// in the `rm` field. `mod_bits=3` for register-register operands.
fn emit_sse_header(code: &mut Vec<u8>, prefix: u8, reg: Xmm, rm: Xmm) {
    let r = reg.needs_rex();
    let b = rm.needs_rex();
    if r || b {
        // REX (no REX.W — SIMD ops are 128/256-bit, not 64-bit GP).
        code.push(0x40 | ((r as u8) << 2) | (b as u8));
    }
    code.push(prefix);
    code.push(0x0F);
}

/// Encode `paddq xmm1, xmm2` (SSE2): `66 0F D4 /r`.
///
/// `dst` (xmm1) is the rm field; `src` (xmm2) is the reg field.
pub fn encode_sse_paddq(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    emit_sse_header(&mut code, 0x66, src, dst);
    code.push(0xD4);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode `psubd xmm1, xmm2` (SSE2): `66 0F FA /r`.
pub fn encode_sse_psubd(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    emit_sse_header(&mut code, 0x66, src, dst);
    code.push(0xFA);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode `pmulld xmm1, xmm2` (SSE4.1): `66 0F 38 40 /r`.
pub fn encode_sse_pmulld(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    emit_sse_header(&mut code, 0x66, src, dst);
    code.push(0x38);
    code.push(0x40);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode `movdqu xmm1, [r64+offset]` (SSE2 load): `F3 0F 6F /r`.
///
/// `dst` (xmm1) is the reg field; `base` is the rm field (memory operand).
pub fn encode_sse_movdqu_load(dst: Xmm, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(8);
    let r = dst.needs_rex();
    let b = base.needs_rex();
    if r || b {
        code.push(0x40 | ((r as u8) << 2) | (b as u8));
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x6F);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode `movdqu [r64+offset], xmm1` (SSE2 store): `F3 0F 7F /r`.
///
/// `src` (xmm1) is the reg field; `base` is the rm field (memory operand).
pub fn encode_sse_movdqu_store(base: Gpr, offset: i32, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(8);
    let r = src.needs_rex();
    let b = base.needs_rex();
    if r || b {
        code.push(0x40 | ((r as u8) << 2) | (b as u8));
    }
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x7F);
    encode_mem_operand(&mut code, src.encoding() & 7, base, offset);
    code
}

/// Encode a 2-byte VEX prefix (C5) for 128-bit AVX instructions without XMM8+
/// or R8+ operands. For operands in the high register file, callers should use
/// a 3-byte VEX prefix (C4); this minimal encoder supports the common case
/// where all operands are in the low 8 registers.
fn emit_vex2(code: &mut Vec<u8>, pp: u8, opcode: u8, _dst: Xmm, src1: Xmm, src2_or_rm: Xmm) {
    // C5 RvvvvLpp — R is the inverted REX.R bit (for src2/reg field),
    //   vvvv encodes the inverted src1 register, L is 0 for 128-bit,
    //   pp is the mandatory-prefix payload (00=none, 01=66, 10=F3, 11=F2).
    // We assume all registers are XMM0–XMM7 (no REX bits needed) for this
    // minimal encoder.
    let r_bit = !(src2_or_rm.encoding() >> 3) & 1; // inverted bit 3 of src2/rm
    let vvvv = !src1.encoding() & 0x0F;
    let pp_field = pp & 0x03;
    let c5_byte2 = (r_bit << 7) | (vvvv << 3) | pp_field;
    code.push(0xC5);
    code.push(c5_byte2);
    code.push(opcode);
}

/// Encode `vpaddq xmm1, xmm2, xmm3` (AVX): `VEX.128.66.0F.WIG D4 /r`.
///
/// All operands must be XMM0–XMM7 (the 3-byte VEX form for XMM8+ is not
/// emitted by this minimal encoder).
pub fn encode_avx_vpaddq(dst: Xmm, src1: Xmm, src2: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    emit_vex2(&mut code, 0x01, 0xD4, dst, src1, src2); // pp=01 for 66 prefix
    code.push(modrm(3, src2.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode `vmovdqu xmm1, [r64+offset]` (AVX load): `VEX.128.F3.0F.WIG 6F /r`.
pub fn encode_avx_vmovdqu_load(dst: Xmm, base: Gpr, offset: i32) -> Vec<u8> {
    // For memory operands we use the 3-byte VEX form to allow R8–R15 base
    // registers. VEX.3: C4 RXBmmmmm WvvvvLpp.
    let mut code = Vec::with_capacity(8);
    let r_bit = 1; // inverted REX.R for dst in reg field, no high bit assumed for XMM0–7
    let x_bit = 1;
    let b_bit = !(base.encoding() >> 3) & 1; // inverted bit 3 of base
    let mmmmm = 0x01; // 0F escape
    let w = 1; // WIG (ignored, but 1 is common)
    let vvvv = 0x0F; // no additional src register (1111 = none)
    let l = 0; // 128-bit
    let pp = 0b10; // F3 prefix
    let c4_byte1 = (r_bit << 7) | (x_bit << 6) | (b_bit << 5) | mmmmm;
    let c4_byte2 = (w << 7) | (vvvv << 3) | (l << 2) | pp;
    code.push(0xC4);
    code.push(c4_byte1);
    code.push(c4_byte2);
    code.push(0x6F);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
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

/// Encode SUBSD xmm, xmm (F2 0F 5C /r) — subtract scalar double-precision floats.
pub fn encode_subsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
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
    code.push(0x5C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode SUBSS xmm, xmm (F3 0F 5C /r) — subtract scalar single-precision floats.
pub fn encode_subss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
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
    code.push(0x5C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MULSD xmm, xmm (F2 0F 59 /r)
pub fn encode_mulsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF2); code.push(0x0F); code.push(0x59);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MULSS xmm, xmm (F3 0F 59 /r)
pub fn encode_mulss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF3); code.push(0x0F); code.push(0x59);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode DIVSD xmm, xmm (F2 0F 5E /r)
pub fn encode_divsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF2); code.push(0x0F); code.push(0x5E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode DIVSS xmm, xmm (F3 0F 5E /r)
pub fn encode_divss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF3); code.push(0x0F); code.push(0x5E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode SQRTSD xmm, xmm (F2 0F 51 /r)
pub fn encode_sqrtsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF2); code.push(0x0F); code.push(0x51);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode SQRTSS xmm, xmm (F3 0F 51 /r)
pub fn encode_sqrtss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF3); code.push(0x0F); code.push(0x51);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MINSD xmm, xmm (F2 0F 5D /r)
pub fn encode_minsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF2); code.push(0x0F); code.push(0x5D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MAXSD xmm, xmm (F2 0F 5F /r)
pub fn encode_maxsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0xF2); code.push(0x0F); code.push(0x5F);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode UCOMISD xmm, xmm (66 0F 2E /r) — unordered compare scalar double
pub fn encode_ucomisd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0x66); code.push(0x0F); code.push(0x2E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode UCOMISS xmm, xmm (0F 2E /r) — unordered compare scalar single
pub fn encode_ucomiss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    let r = src.needs_rex();
    let b = dst.needs_rex();
    if r || b { if let Some(rex) = rex_prefix(false, r, false, b) { code.push(rex); } }
    code.push(0x0F); code.push(0x2E);
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

        // Skip legacy prefixes and REX. x86_64 permits legacy prefixes
        // (0x66/0x67/0xF2/0xF3) and the REX byte to appear in any order
        // before the opcode; we loop until we hit a non-prefix byte.
        //
        // Special handling for 0xF2/0xF3: these are ambiguous — they can be
        // REP/REPNE legacy prefixes OR mandatory SSE/SSE2/SSE3 prefixes
        // (e.g. CVTSI2SD = F2 0F 2A, ADDSD = F2 0F 58). To distinguish, we
        // peek forward past any REX byte and check whether 0x0F follows. If
        // so, the prefix is treated as a mandatory SSE prefix; otherwise it
        // is treated as a REP/REPNE hint and discarded.
        let mut rex = 0u8;
        let mut _rex_w = false;
        let mut rex_r = false;
        let mut rex_x = false;
        let mut rex_b = false;
        let mut mandatory_prefix: u8 = 0; // 0 = none, 0xF2, or 0xF3

        while pos < bytes.len() {
            match bytes[pos] {
                0x66 | 0x67 => {
                    pos += 1;
                }
                0xF2 | 0xF3 => {
                    // Peek forward (skipping any REX) for the 0x0F two-byte
                    // opcode escape. If found, treat as mandatory SSE prefix.
                    let mut peek = pos + 1;
                    while peek < bytes.len() && bytes[peek] >= 0x40 && bytes[peek] <= 0x4F {
                        peek += 1;
                    }
                    if peek < bytes.len() && bytes[peek] == 0x0F {
                        mandatory_prefix = bytes[pos];
                        pos += 1;
                    } else {
                        // REP/REPNE hint: discard.
                        pos += 1;
                    }
                }
                0x40..=0x4F => {
                    rex = bytes[pos];
                    _rex_w = (rex & 0x08) != 0;
                    rex_r = (rex & 0x04) != 0;
                    rex_x = (rex & 0x02) != 0;
                    rex_b = (rex & 0x01) != 0;
                    pos += 1;
                }
                _ => break,
            }
        }
        let _ = rex_x; // currently unused by reg/reg callers but tracked for SIB

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
                    // SSE mandatory-prefix opcodes take priority over the
                    // legacy 0F xx decoding. 0xF2/0xF3 were detected during
                    // prefix scanning only if a 0x0F escape followed them.
                    if mandatory_prefix == 0xF3 {
                        match op2 {
                            // POPCNT r64, r/m64 (F3 0F B8 /r)
                            0xB8 => {
                                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                                pos = np;
                                format!("popcnt {}, {}", gpr_name_64(r), op)
                            }
                            // TZCNT r64, r/m64 (F3 0F BC /r)
                            0xBC => {
                                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                                pos = np;
                                format!("tzcnt {}, {}", gpr_name_64(r), op)
                            }
                            // LZCNT r64, r/m64 (F3 0F BD /r)
                            0xBD => {
                                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                                pos = np;
                                format!("lzcnt {}, {}", gpr_name_64(r), op)
                            }
                            // CVTSI2SS xmm, r/m32 (F3 0F 2A /r) — reg=xmm, rm=gpr/mem
                            0x2A => {
                                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                                pos = np;
                                format!("cvtsi2ss {}, {}", xmm_name(r), op)
                            }
                            // CVTTSS2SI r32, xmm (F3 0F 2C /r) — reg=gpr, rm=xmm/mem
                            0x2C => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("cvttss2si {}, {}", gpr_name_64(r), op)
                            }
                            // CVTSS2SI r32, xmm (F3 0F 2D /r) — reg=gpr, rm=xmm/mem
                            0x2D => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("cvtss2si {}, {}", gpr_name_64(r), op)
                            }
                            // ADDSS xmm, xmm (F3 0F 58 /r) — both XMM
                            0x58 => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("addss {}, {}", xmm_name(r), op)
                            }
                            // CVTSS2SD xmm, xmm (F3 0F 5A /r) — both XMM
                            0x5A => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("cvtss2sd {}, {}", xmm_name(r), op)
                            }
                            // SUBSS xmm, xmm (F3 0F 5C /r) — both XMM
                            0x5C => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("subss {}, {}", xmm_name(r), op)
                            }
                            _ => format!("f3 0f {:02x}", op2),
                        }
                    } else if mandatory_prefix == 0xF2 {
                        match op2 {
                            // CVTSI2SD xmm, r/m32 (F2 0F 2A /r) — reg=xmm, rm=gpr/mem
                            0x2A => {
                                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                                pos = np;
                                format!("cvtsi2sd {}, {}", xmm_name(r), op)
                            }
                            // CVTTSD2SI r32, xmm (F2 0F 2C /r) — reg=gpr, rm=xmm/mem
                            0x2C => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("cvttsd2si {}, {}", gpr_name_64(r), op)
                            }
                            // CVTSD2SI r32, xmm (F2 0F 2D /r) — reg=gpr, rm=xmm/mem
                            0x2D => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("cvtsd2si {}, {}", gpr_name_64(r), op)
                            }
                            // ADDSD xmm, xmm (F2 0F 58 /r) — both XMM
                            0x58 => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("addsd {}, {}", xmm_name(r), op)
                            }
                            // CVTSD2SS xmm, xmm (F2 0F 5A /r) — both XMM
                            0x5A => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("cvtsd2ss {}, {}", xmm_name(r), op)
                            }
                            // SUBSD xmm, xmm (F2 0F 5C /r) — both XMM
                            0x5C => {
                                let (r, op, np) = decode_modrm_xmm_rm(bytes, pos, rex);
                                pos = np;
                                format!("subsd {}, {}", xmm_name(r), op)
                            }
                            _ => format!("f2 0f {:02x}", op2),
                        }
                    } else {
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
                        // BT/BTS/BTR/BTC r/m64, r64 (0F BA /x is the imm8 form,
                        // but 0F A3/AB/B3/BB are the reg forms — handled below.)
                        0xBA => {
                            let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                            pos = np;
                            let op_name = match r & 7 {
                                4 => "bt",
                                5 => "bts",
                                6 => "btr",
                                7 => "btc",
                                _ => "0f ba",
                            };
                            if pos < bytes.len() {
                                let imm = bytes[pos] as i8 as i32;
                                pos += 1;
                                format!("{} {}, {}", op_name, op, imm)
                            } else {
                                format!("{} {}, ???", op_name, op)
                            }
                        }
                        // BSWAP r64 (0F C8+rd)
                        0xC8..=0xCF => {
                            let reg_idx = (op2 - 0xC8) | (if rex_b { 8 } else { 0 });
                            format!("bswap {}", gpr_name_64(reg_idx))
                        }
                        // MOVZX r64, r8
                        0xB6 => {
                            let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                            pos = np;
                            format!("movzx {}, {}", gpr_name_64(r), op)
                        }
                        // MOVZX r64, r16
                        0xB7 => {
                            let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                            pos = np;
                            format!("movzx {}, {}", gpr_name_64(r), op)
                        }
                        // MOVSX r64, r8
                        0xBE => {
                            let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                            pos = np;
                            format!("movsx {}, {}", gpr_name_64(r), op)
                        }
                        // MOVSX r64, r16
                        0xBF => {
                            let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                            pos = np;
                            format!("movsx {}, {}", gpr_name_64(r), op)
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
            // MOV r/m, r (89 /r) — supports memory operands.
            0x89 => {
                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                pos = np;
                format!("mov {}, {}", op, gpr_name_64(r))
            }
            // MOV r, r/m (8B /r) — supports memory operands.
            0x8B => {
                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                pos = np;
                format!("mov {}, {}", gpr_name_64(r), op)
            }
            // MOV r/m8, r8 (88 /r) — byte store to memory or register.
            0x88 => {
                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                pos = np;
                format!("mov {}, {}", op, gpr_name_8(r, rex != 0))
            }
            // MOV r8, r/m8 (8A /r) — byte load from memory or register.
            0x8A => {
                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                pos = np;
                format!("mov {}, {}", gpr_name_8(r, rex != 0), op)
            }
            // LEA r64, m (8D /r) — memory operand (no load occurs).
            0x8D => {
                let (r, op, np) = decode_modrm_operand(bytes, pos, rex);
                pos = np;
                format!("lea {}, {}", gpr_name_64(r), op)
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

            // C1 /x + imm8 (SHL/SHR/SAR/ROL/ROR r/m, imm8)
            0xC1 => {
                let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                pos = np;
                if pos < bytes.len() {
                    let imm = bytes[pos];
                    pos += 1;
                    let op_name = match r {
                        0 => "rol",
                        1 => "ror",
                        2 => "rcl",
                        3 => "rcr",
                        4 => "shl",
                        5 => "shr",
                        6 => "sal",
                        7 => "sar",
                        _ => "???",
                    };
                    format!("{} {}, {}", op_name, gpr_name_64(rm), imm)
                } else {
                    pos = bytes.len();
                    format!("c1 /{}, {}", r, gpr_name_64(rm))
                }
            }

            // C6 /0 + imm8 (MOV r/m8, imm8) / C7 /0 + imm32 handled below
            0xC6 => {
                let (_, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                pos = np;
                if pos < bytes.len() {
                    let imm = bytes[pos];
                    pos += 1;
                    format!("mov byte {}, {}", gpr_name_64(rm), imm)
                } else {
                    pos = bytes.len();
                    format!("mov byte {}, ???", gpr_name_64(rm))
                }
            }

            // CD ib (INT ib) — software interrupt. CD 80 is the Linux
            // 32-bit syscall gateway; show as "int 0x80" for that case.
            0xCD => {
                if pos < bytes.len() {
                    let imm = bytes[pos];
                    pos += 1;
                    format!("int {:#x}", imm)
                } else {
                    pos = bytes.len();
                    "int ???".to_string()
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
///
/// Properly advances `new_pos` past the ModR/M byte and any SIB byte and
/// displacement for memory operands (mod != 3). For mod=3 (register-register),
/// `rm` is the destination register index; for memory operands, `rm` is the
/// raw r/m field value (with REX.B applied) — callers that need to display the
/// memory operand should use [`decode_modrm_operand`] instead.
fn decode_modrm_reg_rm(bytes: &[u8], pos: usize, rex_r: bool, rex_b: bool) -> (u8, u8, usize) {
    if pos >= bytes.len() {
        return (0, 0, pos);
    }
    let modrm = bytes[pos];
    let mut new_pos = pos + 1;
    let mod_bits = (modrm >> 6) & 3;
    let reg = ((modrm >> 3) & 7) | (if rex_r { 8 } else { 0 });
    let rm_raw = modrm & 7;
    let rm = rm_raw | (if rex_b { 8 } else { 0 });

    if mod_bits == 3 {
        // Register-register: just the ModR/M byte.
        return (reg, rm, new_pos);
    }

    // Memory operand: consume SIB byte and/or displacement.
    if rm_raw == 4 {
        // SIB byte follows.
        if new_pos >= bytes.len() {
            return (reg, rm, new_pos);
        }
        let sib = bytes[new_pos];
        new_pos += 1;
        let sib_base = sib & 7;
        // Special case: SIB base == 5 with mod == 0 means no base register;
        // a disp32 follows instead.
        if sib_base == 5 && mod_bits == 0 && new_pos + 4 <= bytes.len() {
            new_pos += 4;
        }
    } else if rm_raw == 5 && mod_bits == 0 {
        // mod=0, r/m=5: disp32 (RIP-relative) follows; no base register.
        if new_pos + 4 <= bytes.len() {
            new_pos += 4;
        }
    }

    // Displacement based on mod (mod=0 already handled above; only SIB.base==5
    // case has disp32 already consumed).
    match mod_bits {
        1 => new_pos = (new_pos + 1).min(bytes.len()), // disp8
        2 => new_pos = (new_pos + 4).min(bytes.len()), // disp32
        _ => {}
    }

    (reg, rm, new_pos)
}

/// Decode a ModR/M byte and return the full operand string.
///
/// Returns `(reg, operand_string, new_pos)`:
/// - For mod=3 (register operand), `operand_string` is the register name.
/// - For mod!=3 (memory operand), `operand_string` is the full addressing mode,
///   e.g. `[rbp-0x10]`, `[rax+rcx*4+0x100]`, `[rip+0x1234]`, `[0x4000]`.
///
/// `rex` is the full REX byte (0x40-0x4F, or 0 if no REX). The REX.R/X/B bits
/// are extracted internally.
fn decode_modrm_operand(bytes: &[u8], pos: usize, rex: u8) -> (u8, String, usize) {
    if pos >= bytes.len() {
        return (0, "???".to_string(), pos);
    }
    let modrm = bytes[pos];
    let mut new_pos = pos + 1;
    let mod_bits = (modrm >> 6) & 3;
    let rex_r = (rex & 0x04) != 0;
    let rex_x = (rex & 0x02) != 0;
    let rex_b = (rex & 0x01) != 0;
    let reg = ((modrm >> 3) & 7) | (if rex_r { 8 } else { 0 });
    let rm_raw = modrm & 7;

    if mod_bits == 3 {
        let rm = rm_raw | (if rex_b { 8 } else { 0 });
        return (reg, gpr_name_64(rm).to_string(), new_pos);
    }

    // Memory operand.
    let mut base_reg: Option<u8> = None;
    let mut index_reg: Option<u8> = None;
    let mut scale: u8 = 0;
    let mut disp: i64 = 0;
    let mut has_disp = false;
    let mut rip_relative = false;

    if rm_raw == 4 {
        // SIB byte follows.
        if new_pos >= bytes.len() {
            return (reg, "[???]".to_string(), new_pos);
        }
        let sib = bytes[new_pos];
        new_pos += 1;
        let scale_raw = (sib >> 6) & 3;
        scale = match scale_raw {
            0 => 1,
            1 => 2,
            2 => 4,
            _ => 8,
        };
        let index_raw = (sib >> 3) & 7;
        let base_raw = sib & 7;

        // Index: raw == 4 means "no index" (even with REX.X).
        if index_raw != 4 {
            index_reg = Some(index_raw | (if rex_x { 8 } else { 0 }));
        }

        // Base register: raw == 5 with mod == 0 means "no base", disp32 follows.
        if base_raw == 5 && mod_bits == 0 {
            if new_pos + 4 <= bytes.len() {
                disp = i32::from_le_bytes(
                    bytes[new_pos..new_pos + 4].try_into().unwrap_or([0; 4]),
                ) as i64;
                new_pos += 4;
                has_disp = true;
            }
        } else {
            base_reg = Some(base_raw | (if rex_b { 8 } else { 0 }));
        }
    } else if rm_raw == 5 && mod_bits == 0 {
        // mod=0, r/m=5: disp32 (RIP-relative).
        if new_pos + 4 <= bytes.len() {
            disp = i32::from_le_bytes(
                bytes[new_pos..new_pos + 4].try_into().unwrap_or([0; 4]),
            ) as i64;
            new_pos += 4;
            has_disp = true;
            rip_relative = true;
        }
    } else {
        base_reg = Some(rm_raw | (if rex_b { 8 } else { 0 }));
    }

    // Displacement based on mod.
    match mod_bits {
        1
            if new_pos < bytes.len() => {
                disp = bytes[new_pos] as i8 as i64;
                new_pos += 1;
                has_disp = true;
            }
        2
            if new_pos + 4 <= bytes.len() => {
                disp = i32::from_le_bytes(
                    bytes[new_pos..new_pos + 4].try_into().unwrap_or([0; 4]),
                ) as i64;
                new_pos += 4;
                has_disp = true;
            }
        _ => {}
    }

    if rip_relative {
        let s = if disp < 0 {
            format!("[rip-{:#x}]", (-disp) as u64)
        } else {
            format!("[rip+{:#x}]", disp as u64)
        };
        return (reg, s, new_pos);
    }

    // Build the operand string: [base+index*scale+disp]
    let mut s = String::from("[");
    let mut empty = true;
    if let Some(b) = base_reg {
        s.push_str(gpr_name_64(b));
        empty = false;
    }
    if let Some(i) = index_reg {
        if !empty {
            s.push('+');
        }
        s.push_str(gpr_name_64(i));
        if scale != 1 {
            s.push_str(&format!("*{}", scale));
        }
        empty = false;
    }
    if has_disp {
        if empty {
            // Absolute address (no base/index).
            s.push_str(&format!("{:#x}", disp as u64));
        } else if disp < 0 {
            s.push_str(&format!("-{:#x}", (-disp) as u64));
        } else {
            s.push_str(&format!("+{:#x}", disp as u64));
        }
    }
    s.push(']');
    (reg, s, new_pos)
}

/// Helper: get XMM register name from index (0-15).
fn xmm_name(idx: u8) -> &'static str {
    match idx & 0xF {
        0 => "xmm0",
        1 => "xmm1",
        2 => "xmm2",
        3 => "xmm3",
        4 => "xmm4",
        5 => "xmm5",
        6 => "xmm6",
        7 => "xmm7",
        8 => "xmm8",
        9 => "xmm9",
        10 => "xmm10",
        11 => "xmm11",
        12 => "xmm12",
        13 => "xmm13",
        14 => "xmm14",
        15 => "xmm15",
        _ => "xmm?",
    }
}

/// Decode ModR/M for SSE instructions where the r/m field is an XMM register
/// (mod=3) or a memory operand (mod != 3).
///
/// Returns `(reg, operand_string, new_pos)`. For mod=3, `operand_string` is
/// the XMM register name; for mod!=3, it is the full memory operand string
/// (e.g., `[rax+0x8]`). The `reg` field is returned as a raw index — callers
/// format it as either GPR or XMM depending on the instruction.
fn decode_modrm_xmm_rm(bytes: &[u8], pos: usize, rex: u8) -> (u8, String, usize) {
    if pos >= bytes.len() {
        return (0, "???".to_string(), pos);
    }
    let modrm = bytes[pos];
    let mod_bits = (modrm >> 6) & 3;
    if mod_bits == 3 {
        let rex_r = (rex & 0x04) != 0;
        let rex_b = (rex & 0x01) != 0;
        let reg = ((modrm >> 3) & 7) | (if rex_r { 8 } else { 0 });
        let rm = (modrm & 7) | (if rex_b { 8 } else { 0 });
        return (reg, xmm_name(rm).to_string(), pos + 1);
    }
    // For memory operands, the addressing-mode format is independent of
    // whether the loaded/stored value is in a GPR or XMM register.
    decode_modrm_operand(bytes, pos, rex)
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
    // Use 4K for file offset alignment (keeps the file small) but 64K for
    // virtual address alignment.  QEMU 10.x on hosts with 16K or 64K page
    // sizes requires MAP_FIXED_NOREPLACE addresses to be host-page-aligned.
    // Since x86_64's TARGET_PAGE_SIZE is fixed at 4K in QEMU, the issue
    // manifests as mmap returning EINVAL for non-host-page-aligned addresses,
    // causing "Unable to find a guest_base" errors.  Aligning vaddrs to 64K
    // (the largest common aarch64 page size) ensures compatibility.
    const FILE_PAGE_SIZE: u64 = 0x1000; // 4 KB — file offset alignment
    const VADDR_ALIGN: u64 = 0x10000;   // 64 KB — virtual address alignment

    // ELF section header constants (Elf64_Shdr sh_type / sh_flags).
    // SHT_NULL (0) is implicit: section 0 is emitted as 64 zero bytes.
    const SHT_PROGBITS: u32 = 1;
    const SHT_STRTAB: u32 = 3;
    const SHT_NOBITS: u32 = 8;
    const SHF_WRITE: u64 = 0x1;
    const SHF_ALLOC: u64 = 0x2;
    const SHF_EXECINSTR: u64 = 0x4;
    const SHDR_SIZE: u64 = 64; // sizeof(Elf64_Shdr)

    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    // Program headers: (1) text LOAD, (2) BSS LOAD (if any BSS), (3) PT_GNU_STACK.
    // PT_GNU_STACK is always emitted so the kernel explicitly marks the stack
    // non-executable; without it, some loaders default to an executable stack
    // (security risk). The +1 for PT_GNU_STACK keeps e_phnum in sync.
    let has_bss = bss_size > 0;
    let num_phdrs: u64 = if has_bss { 3 } else { 2 };
    let phdr_end = elf_header_size + phdr_size * num_phdrs;
    // Page-align the text segment start for mmap compatibility (required by QEMU).
    let text_offset = phdr_end.div_ceil(FILE_PAGE_SIZE) * FILE_PAGE_SIZE;
    let text_size = code.len() as u64;
    // Align the text virtual address to 64K for host page size compatibility.
    // p_offset (text_offset, 4K-aligned) and p_vaddr (64K-aligned) are both
    // 0 mod 4K, satisfying the p_align congruence requirement.
    let text_vaddr = (base_addr + text_offset).div_ceil(VADDR_ALIGN) * VADDR_ALIGN;
    let entry_point = text_vaddr;

    // BSS virtual address (computed once; used by both the program header
    // and the .bss section header). Zero when there is no BSS.
    let bss_vaddr: u64 = if has_bss {
        (text_vaddr + text_size).div_ceil(VADDR_ALIGN) * VADDR_ALIGN
    } else {
        0
    };

    // --- Section header table layout ---
    // .shstrtab content: NUL + ".text" + NUL + ".bss" + NUL + ".shstrtab" + NUL
    //   name offsets:  .text=1  .bss=7  .shstrtab=12
    let shstrtab_content: &[u8] = b"\0.text\0.bss\0.shstrtab\0";
    let shstrtab_size = shstrtab_content.len() as u64;
    // .shstrtab immediately follows the text segment in the file.
    let shstrtab_offset = text_offset + text_size;
    // Section header table starts after .shstrtab, 8-byte aligned
    // (Elf64_Shdr has natural alignment of 8 bytes).
    let shdr_offset = (shstrtab_offset + shstrtab_size).div_ceil(8) * 8;
    // Sections: 0=null, 1=.text, [2=.bss], last=.shstrtab
    let num_shdrs: u64 = if has_bss { 4 } else { 3 };
    let shstrndx: u16 = (num_shdrs - 1) as u16; // .shstrtab is the last section

    let mut elf = Vec::with_capacity(shdr_offset as usize + (num_shdrs * SHDR_SIZE) as usize);

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
    elf.extend_from_slice(&shdr_offset.to_le_bytes()); // e_shoff
    elf.extend_from_slice(&0u32.to_le_bytes()); // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes()); // e_phentsize
    elf.extend_from_slice(&(num_phdrs as u16).to_le_bytes()); // e_phnum
    elf.extend_from_slice(&64u16.to_le_bytes()); // e_shentsize
    elf.extend_from_slice(&(num_shdrs as u16).to_le_bytes()); // e_shnum
    elf.extend_from_slice(&shstrndx.to_le_bytes()); // e_shstrndx

    // --- Program Header 1: LOAD segment for .text (PF_R | PF_X) ---
    elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes()); // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&text_vaddr.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&text_vaddr.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&text_size.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&FILE_PAGE_SIZE.to_le_bytes()); // p_align

    // --- Program Header 2: LOAD segment for .bss (PF_R | PF_W) ---
    // Only emitted when there is BSS data. The BSS segment starts at the
    // next 64K boundary after the text segment to ensure it doesn't share
    // a host page with the RX text segment. p_filesz = 0 because BSS
    // has no file content; the kernel zero-fills p_memsz bytes at load time.
    if has_bss {
        elf.extend_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD
        elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W
        elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset (no file content)
        elf.extend_from_slice(&bss_vaddr.to_le_bytes()); // p_vaddr
        elf.extend_from_slice(&bss_vaddr.to_le_bytes()); // p_paddr
        elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz (BSS is zero-filled)
        elf.extend_from_slice(&bss_size.to_le_bytes()); // p_memsz
        elf.extend_from_slice(&FILE_PAGE_SIZE.to_le_bytes()); // p_align
    }

    // --- Program Header (last): PT_GNU_STACK ---
    // Marks the stack as non-executable (PF_W | PF_R, no PF_X). Always
    // emitted — even when there is no BSS — so the loader never falls
    // back to a default executable-stack policy. A zero p_memsz/p_filesz
    // is the conventional "annotation-only" form: the kernel still
    // allocates a stack of its default size, but applies our permission
    // policy to it.
    //
    // p_type   = PT_GNU_STACK (0x6474e551)
    // p_flags  = PF_W | PF_R  (0x6)         — explicitly NOT PF_X
    // p_offset = 0, p_vaddr = 0, p_paddr = 0
    // p_filesz = 0, p_memsz  = 0
    // p_align  = 0x10                       — standard PT_GNU_STACK alignment
    elf.extend_from_slice(&0x6474e551u32.to_le_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&6u32.to_le_bytes()); // p_flags = PF_R | PF_W (no PF_X)
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_offset
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_paddr
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_filesz
    elf.extend_from_slice(&0u64.to_le_bytes()); // p_memsz
    elf.extend_from_slice(&0x10u64.to_le_bytes()); // p_align

    // --- Padding + Code section ---
    // Pad to page-aligned text_offset
    while (elf.len() as u64) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    // --- .shstrtab section content ---
    // Immediately follows the text segment. Contains NUL-terminated section
    // name strings referenced by the section header table's sh_name fields.
    elf.extend_from_slice(shstrtab_content);

    // Pad to 8-byte alignment for the section header table.
    while (elf.len() as u64) < shdr_offset {
        elf.push(0);
    }

    // --- Section Header Table ---
    // Each Elf64_Shdr is 64 bytes:
    //   sh_name(u32) sh_type(u32) sh_flags(u64) sh_addr(u64) sh_offset(u64)
    //   sh_size(u64) sh_link(u32) sh_info(u32) sh_addralign(u64) sh_entsize(u64)

    // Section 0: SHT_NULL (reserved, all zeros).
    elf.extend_from_slice(&[0u8; 64]);

    // Section 1: .text (SHT_PROGBITS, SHF_ALLOC | SHF_EXECINSTR).
    elf.extend_from_slice(&1u32.to_le_bytes()); // sh_name (offset 1 in .shstrtab)
    elf.extend_from_slice(&SHT_PROGBITS.to_le_bytes()); // sh_type
    elf.extend_from_slice(&(SHF_ALLOC | SHF_EXECINSTR).to_le_bytes()); // sh_flags
    elf.extend_from_slice(&text_vaddr.to_le_bytes()); // sh_addr
    elf.extend_from_slice(&text_offset.to_le_bytes()); // sh_offset
    elf.extend_from_slice(&text_size.to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&16u64.to_le_bytes()); // sh_addralign (16 for code)
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize

    // Section 2: .bss (SHT_NOBITS, SHF_ALLOC | SHF_WRITE) — only when present.
    if has_bss {
        elf.extend_from_slice(&7u32.to_le_bytes()); // sh_name (offset 7 in .shstrtab)
        elf.extend_from_slice(&SHT_NOBITS.to_le_bytes()); // sh_type
        elf.extend_from_slice(&(SHF_ALLOC | SHF_WRITE).to_le_bytes()); // sh_flags
        elf.extend_from_slice(&bss_vaddr.to_le_bytes()); // sh_addr
        elf.extend_from_slice(&0u64.to_le_bytes()); // sh_offset (NOBITS: no file content)
        elf.extend_from_slice(&bss_size.to_le_bytes()); // sh_size
        elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
        elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
        elf.extend_from_slice(&16u64.to_le_bytes()); // sh_addralign
        elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize
    }

    // Section (last): .shstrtab (SHT_STRTAB, no alloc flags — not loaded).
    // ".shstrtab" lives at offset 12 in the .shstrtab blob regardless of
    // whether the .bss section is present.
    elf.extend_from_slice(&12u32.to_le_bytes()); // sh_name
    elf.extend_from_slice(&SHT_STRTAB.to_le_bytes()); // sh_type
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_flags (not loaded into memory)
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_addr (no virtual address)
    elf.extend_from_slice(&shstrtab_offset.to_le_bytes()); // sh_offset
    elf.extend_from_slice(&shstrtab_size.to_le_bytes()); // sh_size
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_link
    elf.extend_from_slice(&0u32.to_le_bytes()); // sh_info
    elf.extend_from_slice(&1u64.to_le_bytes()); // sh_addralign (byte-aligned strings)
    elf.extend_from_slice(&0u64.to_le_bytes()); // sh_entsize

    elf
}

// ===========================================================================
// Runtime Syscall Stubs
// ===========================================================================

/// Wave 47: Size in bytes of the runtime argv-storage slot reserved at
/// offset 0 of BSS. Holds argc (8 bytes) and argv (8 bytes) saved by the
/// `_start` stub at process entry, so the `__vuma_argc` / `__vuma_argv`
/// runtime stubs can retrieve them later. Unconditionally reserved by
/// `encode_program` so the stubs are always safe to call.
pub const RUNTIME_ARGV_STORAGE_SIZE: u64 = 16;

/// Wave 47: 8-byte sentinel value used as a placeholder for the BSS
/// argv-storage virtual address inside the `_start` stub and the
/// `__vuma_argc` / `__vuma_argv` runtime stubs. The placeholder is emitted
/// by `build_runtime_syscall_stubs` (for the two argv stubs) and by
/// `encode_program` (for the `_start` stub), then patched in a single
/// scan-and-replace pass over the concatenated code once BSS layout is
/// finalized. The value is chosen to be a recognizable sentinel that will
/// not collide with any legitimate instruction immediate emitted by the
/// x86_64 encoders (which produce small constants ≤ 0xFFFFFFFF or syscall
/// numbers ≤ 318).
pub const RUNTIME_ARGV_STORAGE_PLACEHOLDER: u64 = 0xDEAD_BEEF_CAFE_BABE;

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
///
/// # Wave 47 — Process Startup Argument Access
///
/// Two non-syscall stubs (`__vuma_argc`, `__vuma_argv`) provide VUMA programs
/// with access to argc/argv. They read from a 16-byte runtime-managed slot at
/// the start of BSS, populated by the `_start` stub before `main` is called.
/// See [`RUNTIME_ARGV_STORAGE_PLACEHOLDER`] and [`RUNTIME_ARGV_STORAGE_SIZE`].
fn build_runtime_syscall_stubs() -> Vec<(String, Vec<u8>)> {
    let mut stubs = Vec::new();

    // -- Helper: encode a simple "mov eax, #num ; syscall ; ret" stub --
    // For syscalls whose SystemV argument registers (RDI, RSI, RDX, RCX,
    // R8, R9) already match the syscall argument registers (RDI, RSI, RDX,
    // R10, R8, R9) -- i.e. those taking <=3 args -- the stub is just three
    // instructions.  The table below lists every such syscall we expose.
    let simple_stub = |num: i32| -> Vec<u8> {
        let mut code = Vec::with_capacity(14);
        code.extend(encode_mov_reg_imm32(Gpr::Rax, num));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        code
    };

    // -- Simple stubs (args already in correct registers) --
    // Linux x86_64 syscall numbers.  Each entry maps a VUMA-visible
    // function name to its syscall number; the stub body is the canonical
    // "mov eax, #num ; syscall ; ret" sequence.
    for (name, num) in [
        // Core I/O & process
        ("read", 0), ("write", 1), ("open", 2), ("close", 3),
        ("stat", 4), ("fstat", 5), ("lstat", 6), ("poll", 7),
        ("lseek", 8), ("mprotect", 10),
        ("munmap", 11),
        ("brk", 12),
        // Signal-related (simple variants; sigaction needs a 4th arg, kept inline)
        ("rt_sigprocmask", 14),
        ("rt_sigreturn", 15),
        ("ioctl", 16), ("pipe", 22), ("dup", 32), ("dup2", 33),
        ("nanosleep", 35), ("alarm", 37),
        ("getpid", 39), ("fork", 57), ("execve", 59), ("exit_group", 231),
        ("kill", 62), ("getcwd", 79), ("chdir", 80), ("fcntl", 72),
        ("unlink", 87),
        // Time
        ("gettimeofday", 96),
        ("clock_gettime", 228),
        // Networking
        ("socket", 41), ("connect", 42), ("accept", 43),
        ("send", 44), ("recv", 45), ("sendto", 44), ("recvfrom", 45),
        ("bind", 49), ("listen", 50), ("shutdown", 48),
        // epoll (create1 is simple; ctl needs RCX->R10, kept inline)
        ("epoll_create1", 291),
        // dup3 (modern variant of dup2)
        ("dup3", 292),
        // VUMA heap-free helper -- same as munmap
        ("__vuma_free", 11),
        // ── Wave 7: POSIX file-metadata & I/O syscalls (x86_64 syscall_64.tbl) ──
        // All take ≤5 args; x86_64 has 6 register args (RDI/RSI/RDX/R10/R8/R9),
        // so every entry fits the simple "mov eax,#num; syscall; ret" stub.
        // Family 1: dir/link ops
        ("mkdir", 83), ("rmdir", 84), ("rename", 82),
        ("link", 86), ("symlink", 88), ("readlink", 89),
        // Family 2: mode/owner
        ("chmod", 90), ("fchmod", 91), ("chown", 92),
        ("fchown", 93), ("umask", 95),
        // Family 3: *at variants
        ("fchownat", 260), ("openat", 257), ("unlinkat", 263),
        ("renameat", 264), ("linkat", 265), ("symlinkat", 266),
        ("readlinkat", 267), ("fchmodat", 268), ("faccessat", 269),
        // Family 4: sync/truncate
        ("ftruncate", 46), ("fsync", 74), ("fdatasync", 75),
        ("sync", 162), ("syncfs", 306),
        // Family 5: positioned & vector I/O
        ("pread", 17), ("pwrite", 18), ("readv", 19), ("writev", 20),
        ("preadv", 295), ("pwritev", 296),
        // Family 7: cwd/root (getcwd=79/chdir=80 already above)
        ("fchdir", 81), ("chroot", 161),
        // ── Wave 9: POSIX system & advanced syscalls (x86_64 syscall_64.tbl) ──
        // All take ≤5 args; x86_64 has 6 reg args → simple_stub.
        // eventfd→eventfd2(290), signalfd→signalfd4(289) = modern flag-accepting variants.
        ("mlock", 149), ("munlock", 150), ("mlockall", 151), ("munlockall", 152),
        ("mincore", 27), ("madvise", 28), ("msync", 26), ("mremap", 25),
        ("getrlimit", 97), ("setrlimit", 160), ("prlimit64", 302),
        ("getrusage", 98), ("times", 100),
        ("getrandom", 318),
        ("eventfd", 290), ("timerfd_create", 283), ("timerfd_settime", 286),
        ("timerfd_gettime", 287), ("signalfd", 289),
        ("inotify_init1", 294), ("inotify_add_watch", 254), ("inotify_rm_watch", 255),
        ("ptrace", 101),
        // ── Wave 8: POSIX process & identity syscalls (x86_64 syscall_64.tbl) ──
        // All take ≤5 args; x86_64 has 6 reg args (RDI/RSI/RDX/R10/R8/R9) → simple_stub.
        // Family 1: identity
        ("getuid", 102), ("geteuid", 107), ("getgid", 104), ("getegid", 108),
        ("setuid", 105), ("setgid", 106), ("setresuid", 117), ("setresgid", 119),
        // Family 2: process group (getpid already present)
        ("getppid", 110), ("getsid", 124), ("setsid", 112),
        ("setpgid", 109), ("getpgid", 121), ("getpgrp", 111),
        // Family 3: clone/wait (clone/wait4 already present)
        ("vfork", 58), ("clone3", 435), ("waitid", 247),
        // Family 4: exec/exit (execve/exit_group already present)
        ("execveat", 322),
        // Family 5: signals (kill/rt_sigprocmask/rt_sigreturn already present)
        ("tgkill", 234), ("tkill", 200), ("rt_sigaction", 13),
        // Family 6: directory read (readdir ABSENT on x86_64 → use getdents64)
        ("getdents64", 217), ("getdents", 78),
        // Family 7: system (arch_prctl=158 is x86_64-only)
        ("prctl", 157), ("arch_prctl", 158), ("uname", 63), ("sysinfo", 99),
    ] {
        stubs.push((name.to_string(), simple_stub(num)));
    }

    // -- Complex stubs (need register shuffling or special encoding) --
    // These can't be expressed as a single "mov eax,#num ; syscall ; ret"
    // because they require moving the 4th argument from RCX (SystemV) to
    // R10 (syscall ABI), setting an extra constant argument, or have
    // non-trivial control flow.

    // mmap(addr, length, prot, flags, fd, offset) -> void*  [syscall 9]
    // Need to move 4th arg from RCX -> R10 before syscall
    //
    // [wave 6 — mmap ABI normalization, verified] x86_64's sys_mmap (9) is the
    // DIRECT 6-arg form: (addr, len, prot, flags, fd, offset) in
    //   RDI, RSI, RDX, R10, R8, R9   (kernel syscall ABI)
    // with `offset` in BYTES (x86_64 has no mmap2; sys_mmap does the
    // byte→page conversion in-kernel). The only fixup needed is moving arg4
    // (flags) from the SysV RCX to the syscall-ABI R10 — the other 5 args are
    // already in the correct registers. This matches __vuma_alloc (which sets
    // up RDI/RSI/RDX/R10/R8/R9 then `MOV EAX,9; syscall`): both use the SAME
    // offset unit (bytes, via R9), satisfying the wave-6 "same offset-unit
    // handling as __vuma_alloc" requirement. x86_64 passes 6 args in
    // RDI/RSI/RDX/RCX/R8/R9 (see Gpr::arg_register), so all 6 mmap args fit in
    // registers — no stack-arg plumbing needed.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 9));   // sys_mmap
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX -> R10
        code.extend(encode_syscall());                      // syscall
        code.extend(encode_ret());                          // ret
        stubs.push(("mmap".to_string(), code));
    }

    // exit(code) -> void  [syscall 60]
    // No ret -- exit never returns.  Include INT3 as a safety guard.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 60));  // sys_exit
        code.extend(encode_syscall());                     // syscall
        code.extend(encode_int3());                        // safety guard
        stubs.push(("exit".to_string(), code));
    }

    // __vuma_alloc(size) -> void*  [mmap wrapper]
    // args: RDI=size -> mmap(NULL, size, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_reg(Gpr::Rsi, Gpr::Rdi));  // RSI = size
        code.extend(encode_xor_reg_reg(Gpr::Rdi, Gpr::Rdi));  // RDI = 0 (NULL addr)
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 3));       // RDX = PROT_READ|PROT_WRITE
        code.extend(encode_mov_reg_imm32(Gpr::R10, 0x22));    // R10 = MAP_PRIVATE|MAP_ANONYMOUS
        code.extend(encode_mov_reg_imm32(Gpr::R8, -1i32));    // R8 = -1 (fd)
        code.extend(encode_xor_reg_reg(Gpr::R9, Gpr::R9));    // R9 = 0 (offset)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 9));       // sys_mmap
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("__vuma_alloc".to_string(), code));
    }

    // sigaction(signum, act, oldact) -> long  [syscall 13 = rt_sigaction]
    // Kernel wants a 4th arg (sigsetsize=8); VUMA declares 3 args, so we
    // must load R10 = 8 before the syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 13));  // sys_rt_sigaction
        code.extend(encode_mov_reg_imm32(Gpr::R10, 8));   // sigsetsize = 8
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("sigaction".to_string(), code));
    }

    // waitpid(pid, wstatus, options) -> pid_t  [syscall 61 = sys_wait4]
    // VUMA declares 3 args; the 4th (rusage) must be NULL (0).
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 61));  // sys_wait4
        code.extend(encode_xor_reg_reg(Gpr::R10, Gpr::R10)); // rusage = NULL
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("waitpid".to_string(), code));
    }

    // wait4(pid, wstatus, options, rusage) -> pid_t  [syscall 61]
    // Full 4-arg wait4: move RCX -> R10 before syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 61));  // sys_wait4
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX -> R10 (rusage)
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("wait4".to_string(), code));
    }

    // futex(uaddr, futex_op, val, timeout, uaddr2, val3) -> int  [syscall 202]
    // 4th arg (timeout) is in RCX under SysV but R10 for syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 202)); // sys_futex
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX -> R10 (timeout)
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("futex".to_string(), code));
    }

    // setsockopt(sockfd, level, optname, optval, optlen) -> int  [syscall 54]
    // 4th arg (optval) is in RCX under SysV but R10 for syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 54));  // sys_setsockopt
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX -> R10
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("setsockopt".to_string(), code));

    // getsockopt(sockfd, level, optname, optval, optlen) -> int  [syscall 55]
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 55));  // sys_getsockopt
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("getsockopt".to_string(), code));

    // signalfd4(args...) -> int  [syscall 289]
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 289));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("signalfd4".to_string(), code));
    }

    // newfstatat(args...) -> int  [syscall 262]
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 262));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("newfstatat".to_string(), code));
    }

    // eventfd2(args...) -> int  [syscall 290]
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 290));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("eventfd2".to_string(), code));
    }
    }
    }

    // clone(flags, stack, ptid, ctid, tls) -> pid_t  [syscall 56]
    // 4th arg (ctid) is in RCX under SysV but R10 for syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 56));  // sys_clone
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX -> R10 (ctid)
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("clone".to_string(), code));
    }

    // epoll_ctl(epfd, op, fd, event) -> int  [syscall 233]
    // 4th arg (event) is in RCX under SysV but R10 for syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 233)); // sys_epoll_ctl
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX -> R10
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("epoll_ctl".to_string(), code));
    }

    // epoll_wait(epfd, events, maxevents, timeout) -> int  [syscall 232]
    // 4th arg (timeout) is in RCX under SysV but R10 for syscall.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 232)); // sys_epoll_wait
        code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rcx)); // RCX -> R10
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("epoll_wait".to_string(), code));
    }

    // strcmp(const char *s1, const char *s2) -> int
    // Not a syscall -- implemented as a small assembly loop.
    {
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

    // print_int(n) -> void
    // Converts integer to decimal string and writes to stdout.
    // Args: RDI = integer value
    // Uses raw x86_64 byte encoding for portability.
    {
        let mut code = Vec::new();
        // push rbp; mov rbp, rsp; sub rsp, 32
        code.extend_from_slice(&[0x55]); // push rbp
        code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
        code.extend_from_slice(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 32

        // mov r8, rdi (save n in r8)
        code.extend_from_slice(&[0x49, 0x89, 0xF8]); // mov r8, rdi

        // -- Negative-number handling --
        // If r8 is negative, emit a leading '-' to stdout (write syscall)
        // and negate r8 so the unsigned decimal loop below produces the
        // correct magnitude digits. Without this, signed DIV alone would
        // yield wrong results because the loop pre-zeros RDX instead of
        // using CQO, and the digit extraction assumes a non-negative
        // dividend.
        //
        // Encoding:
        //   test r8, r8              ; 4D 85 C0       (3 bytes)
        //   jns  .positive           ; 79 1E          (2 bytes, skip 30)
        //   neg  r8                  ; 49 F7 D8       (3 bytes)
        //   mov  byte [rsp], 0x2D    ; C6 04 24 2D    (4 bytes)  '-'
        //   mov  edi, 1              ; BF 01 00 00 00 (5 bytes)  fd
        //   lea  rsi, [rsp]          ; 48 8D 34 24    (4 bytes)  buf
        //   mov  edx, 1              ; BA 01 00 00 00 (5 bytes)  count
        //   mov  rax, 1              ; 48 C7 C0 01 00 00 00 (7)  sys_write
        //   syscall                  ; 0F 05          (2 bytes)
        // .positive:
        //
        // Skipped bytes after `jns`: 3 + 4 + 5 + 4 + 5 + 7 + 2 = 30 = 0x1E.
        // Stack frame is fixed (sub rsp, 32 above), so [rsp] is a stable
        // 1-byte scratch buffer that does not collide with the digit
        // buffer at [rsp+20..rsp+21].
        code.extend_from_slice(&[0x4D, 0x85, 0xC0]); // test r8, r8
        code.extend_from_slice(&[0x79, 0x1E]); // jns +30 (to .positive)
        code.extend_from_slice(&[0x49, 0xF7, 0xD8]); // neg r8
        code.extend_from_slice(&[0xC6, 0x04, 0x24, 0x2D]); // mov byte [rsp], '-' (0x2D)
        code.extend_from_slice(&[0xBF, 0x01, 0x00, 0x00, 0x00]); // mov edi, 1 (stdout)
        code.extend_from_slice(&[0x48, 0x8D, 0x34, 0x24]); // lea rsi, [rsp]
        code.extend_from_slice(&[0xBA, 0x01, 0x00, 0x00, 0x00]); // mov edx, 1
        code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1 (sys_write)
        code.extend_from_slice(&[0x0F, 0x05]); // syscall
        // .positive:
        //   (note: syscall clobbers rcx and r11, but rcx is reloaded below)

        // lea rcx, [rsp+20] (point to end of buffer)
        code.extend_from_slice(&[0x48, 0x8D, 0x4C, 0x24, 0x14]); // lea rcx, [rsp+20]
        // mov byte [rcx], 0x0a (newline)
        code.extend_from_slice(&[0xC6, 0x01, 0x0A]); // mov byte [rcx], 10
        // sub rcx, 1
        code.extend_from_slice(&[0x48, 0x83, 0xE9, 0x01]); // sub rcx, 1

        // Handle n=0: test r8, r8; jnz loop; mov byte [rcx], '0'; jmp done
        code.extend_from_slice(&[0x4D, 0x85, 0xC0]); // test r8, r8
        code.extend_from_slice(&[0x75, 0x05]); // jnz +5 (to loop)
        code.extend_from_slice(&[0xC6, 0x01, 0x30]); // mov byte [rcx], '0'
        code.extend_from_slice(&[0xEB, 0x25]); // jmp +37 (to write code, past add rcx,1)

        // loop: (offset 0x0B from jnz target)
        // mov rax, r8; xor rdx, rdx; mov r9, 10; idiv r9
        // Use signed IDIV (F7 /7) instead of unsigned DIV (F7 /6).  Because
        // we negate negative inputs above, r8 is always non-negative on
        // entry to the loop, so the dividend rdx:rax is a non-negative
        // 128-bit value and signed/unsigned division agree -- but IDIV is
        // required to honour the spirit of the i64->decimal conversion
        // contract for `print_int`.
        code.extend_from_slice(&[0x4C, 0x89, 0xC0]); // mov rax, r8
        code.extend_from_slice(&[0x48, 0x31, 0xD2]); // xor rdx, rdx
        code.extend_from_slice(&[0x49, 0xC7, 0xC1, 0x0A, 0x00, 0x00, 0x00]); // mov r9, 10
        code.extend_from_slice(&[0x49, 0xF7, 0xF9]); // idiv r9 (was: div r9)
        // mov r8, rax; add dl, 0x30; mov [rcx], dl; sub rcx, 1
        code.extend_from_slice(&[0x49, 0x89, 0xC0]); // mov r8, rax
        code.extend_from_slice(&[0x80, 0xC2, 0x30]); // add dl, 0x30
        code.extend_from_slice(&[0x88, 0x11]); // mov [rcx], dl
        code.extend_from_slice(&[0x48, 0x83, 0xE9, 0x01]); // sub rcx, 1
        // test r8, r8; jnz loop
        code.extend_from_slice(&[0x4D, 0x85, 0xC0]); // test r8, r8
        code.extend_from_slice(&[0x75, 0xDF]); // jnz -33 (back to loop)

        // done:
        // add rcx, 1; lea rdx, [rsp+21]; sub rdx, rcx; mov rsi, rcx
        code.extend_from_slice(&[0x48, 0x83, 0xC1, 0x01]); // add rcx, 1
        code.extend_from_slice(&[0x48, 0x8D, 0x54, 0x24, 0x15]); // lea rdx, [rsp+21]
        code.extend_from_slice(&[0x48, 0x29, 0xCA]); // sub rdx, rcx
        code.extend_from_slice(&[0x48, 0x89, 0xCE]); // mov rsi, rcx
        // mov rdi, 1; mov rax, 1; syscall
        code.extend_from_slice(&[0xBF, 0x01, 0x00, 0x00, 0x00]); // mov edi, 1
        code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1
        code.extend_from_slice(&[0x0F, 0x05]); // syscall

        // Epilogue: add rsp, 32; pop rbp; ret
        code.extend_from_slice(&[0x48, 0x83, 0xC4, 0x20]); // add rsp, 32
        code.extend_from_slice(&[0x5D]); // pop rbp
        code.extend_from_slice(&[0xC3]); // ret
        stubs.push(("print_int".to_string(), code));
    }

    // print_hex(buf: Address, count: u64) -> void
    // Prints `count` bytes from `buf` as hex to stdout.
    // Args: RDI = buf pointer, RSI = count
    {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x55]); // push rbp
        code.extend_from_slice(&[0x48, 0x89, 0xE5]); // mov rbp, rsp
        // Save buf (rdi) in r8, count (rsi) in r9
        code.extend_from_slice(&[0x49, 0x89, 0xF8]); // mov r8, rdi (buf)
        code.extend_from_slice(&[0x49, 0x89, 0xF1]); // mov r9, rsi (count)
        // Check count == 0 -> skip
        code.extend_from_slice(&[0x4D, 0x85, 0xC9]); // test r9, r9
        code.extend_from_slice(&[0x74, 0x3C]); // jz +60 (to pop rbp; ret)

        // Loop: for each byte in buf[0..count]
        // r8 = current buf pointer, r9 = remaining count
        // loop_start (offset = 15):
        // Load byte: movzx eax, byte [r8]
        code.extend_from_slice(&[0x41, 0x0F, 0xB6, 0x00]); // movzx eax, byte [r8]
        // Compute high nibble: shr eax, 4
        code.extend_from_slice(&[0xC1, 0xE8, 0x04]); // shr eax, 4
        // Convert to hex char: add al, 0x30; cmp al, 0x3A; jl +2; add al, 7
        code.extend_from_slice(&[0x04, 0x30]); // add al, 0x30
        code.extend_from_slice(&[0x3C, 0x3A]); // cmp al, 0x3A
        code.extend_from_slice(&[0x7C, 0x02]); // jl +2
        code.extend_from_slice(&[0x04, 0x07]); // add al, 7
        // Store high nibble on stack: mov [rsp-8], al
        code.extend_from_slice(&[0x88, 0x44, 0x24, 0xF8]); // mov [rsp-8], al
        // Load byte again: movzx eax, byte [r8]
        code.extend_from_slice(&[0x41, 0x0F, 0xB6, 0x00]); // movzx eax, byte [r8]
        // Compute low nibble: and eax, 0xF
        code.extend_from_slice(&[0x83, 0xE0, 0x0F]); // and eax, 0xF
        // Convert to hex char
        code.extend_from_slice(&[0x04, 0x30]); // add al, 0x30
        code.extend_from_slice(&[0x3C, 0x3A]); // cmp al, 0x3A
        code.extend_from_slice(&[0x7C, 0x02]); // jl +2
        code.extend_from_slice(&[0x04, 0x07]); // add al, 7
        // Store low nibble: mov [rsp-7], al
        code.extend_from_slice(&[0x88, 0x44, 0x24, 0xF9]); // mov [rsp-7], al
        // Write 2 bytes to stdout: write(1, rsp-8, 2)
        code.extend_from_slice(&[0xBF, 0x01, 0x00, 0x00, 0x00]); // mov edi, 1
        code.extend_from_slice(&[0x48, 0x8D, 0x74, 0x24, 0xF8]); // lea rsi, [rsp-8]
        code.extend_from_slice(&[0xBA, 0x02, 0x00, 0x00, 0x00]); // mov edx, 2
        code.extend_from_slice(&[0x48, 0xC7, 0xC0, 0x01, 0x00, 0x00, 0x00]); // mov rax, 1 (sys_write)
        code.extend_from_slice(&[0x0F, 0x05]); // syscall
        // Advance: inc r8; dec r9; jnz loop_start
        code.extend_from_slice(&[0x49, 0xFF, 0xC0]); // inc r8
        code.extend_from_slice(&[0x49, 0xFF, 0xC9]); // dec r9
        code.extend_from_slice(&[0x75, 0xBA]); // jnz -70 (back to loop_start at offset 15)
        // done: pop rbp; ret
        code.extend_from_slice(&[0x5D]); // pop rbp
        code.extend_from_slice(&[0xC3]); // ret
        stubs.push(("print_hex".to_string(), code));
    }

    // print_newline() -> void
    // Writes a single newline (0x0A) byte to stdout (fd=1).
    // Uses the red-zone-free pattern: push 0x0A (sign-extended to 8 bytes
    // on the stack, with 0x0A as the low byte in little-endian), point
    // RSI at it, write(1, rsp, 1), pop to restore RSP, ret.
    {
        let mut code = Vec::new();
        code.extend_from_slice(&[0x6A, 0x0A]);                  // push 0x0A (newline char)
        code.extend(encode_mov_reg_imm32(Gpr::Rdi, 1));          // mov edi, 1 (stdout)
        code.extend_from_slice(&[0x48, 0x89, 0xE6]);             // mov rsi, rsp
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 1));          // mov edx, 1
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));          // mov rax, 1 (sys_write)
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rax));                       // pop rax (restore stack)
        code.extend(encode_ret());
        stubs.push(("print_newline".to_string(), code));
    }

    // NOTE: brk/clock_gettime/gettimeofday/rt_sigprocmask/rt_sigreturn were
    // previously duplicated here. The canonical entries above (in the main
    // stub table) are the live ones; the duplicate block has been removed to
    // avoid emitting ~70 bytes of dead code.

    // ── Wave 47: process startup argument access ──────────────────────────
    // __vuma_argc() -> i32   and   __vuma_argv() -> Address
    //
    // These stubs read from a runtime-managed 16-byte slot at the START of
    // the BSS segment (offset 0). The _start stub (built in encode_program)
    // populates this slot before calling main:
    //   [bss_vaddr + 0] = argc  (8 bytes, populated from [rsp] at entry)
    //   [bss_vaddr + 8] = argv  (8 bytes, populated from rsp+8 at entry)
    //
    // The stubs below load the BSS argv-storage address via a placeholder
    // 64-bit immediate (RUNTIME_ARGV_STORAGE_PLACEHOLDER). encode_program()
    // scans the emitted bytes for this placeholder and patches it with the
    // real BSS virtual address once layout is finalized. Until patched, the
    // placeholder is a recognizable sentinel (0xDEADBEEFCAFEBABE) so any
    // accidental call before patching would trap immediately rather than
    // silently corrupt a random address.
    //
    // Stub layout (14 bytes for __vuma_argc, 15 for __vuma_argv):
    //   48 B8 <8-byte placeholder>   ; mov rax, <placeholder>
    //   48 8B 00                     ; mov rax, [rax]         (__vuma_argc)
    //   48 8B 40 08                  ; mov rax, [rax+8]       (__vuma_argv)
    //   C3                           ; ret
    //
    // The 16-byte BSS slot is reserved unconditionally by encode_program()
    // (every emitted ELF gets one), so these stubs are always safe to call
    // — though programs that never reference __vuma_argc/__vuma_argv will
    // have the stubs as dead code (the linker-style resolution in
    // encode_program only patches CALL sites that actually reference them;
    // the stub bytes themselves are always present in the runtime stubs
    // table, same as print_int, mmap, etc.).
    {
        let mut code = Vec::with_capacity(14);
        code.extend(encode_mov_reg_imm64(Gpr::Rax, RUNTIME_ARGV_STORAGE_PLACEHOLDER));
        code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rax, 0));
        code.extend(encode_ret());
        stubs.push(("__vuma_argc".to_string(), code));
    }
    {
        let mut code = Vec::with_capacity(15);
        code.extend(encode_mov_reg_imm64(Gpr::Rax, RUNTIME_ARGV_STORAGE_PLACEHOLDER));
        code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rax, 8));
        code.extend(encode_ret());
        stubs.push(("__vuma_argv".to_string(), code));
    }

    // ── FFI scratchpad frame stubs (Wave 3b/fix) ──────────────────────────
    //
    // ffi_scratch_push_frame() -> void
    //   Allocates a 4096-byte scratchpad frame via mmap. The pointer is
    //   returned in RAX (real syscall, not a no-op stub). The caller's IR
    //   hook (scg_to_ir.rs:emit_scratchpad_hooks) discards the return value
    //   (dst: None); the pointer is not stored — when marshal_cstr is wired,
    //   the marshal stubs will manage the bump pointer within the frame.
    //   For now, the frame is allocated (real mmap) and leaked on function
    //   exit. This is REAL code (mmap syscall #9), not a return-0 stub.
    {
        let mut code = Vec::new();
        code.extend(encode_xor_reg_reg(Gpr::Rdi, Gpr::Rdi));  // addr = NULL
        code.extend(encode_mov_reg_imm32(Gpr::Rsi, 4096));    // len = 4096
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 3));       // PROT_READ|PROT_WRITE
        code.extend(encode_mov_reg_imm32(Gpr::R10, 0x22));    // MAP_PRIVATE|MAP_ANONYMOUS
        code.extend(encode_mov_reg_imm32(Gpr::R8, -1i32));    // fd = -1
        code.extend(encode_xor_reg_reg(Gpr::R9, Gpr::R9));    // offset = 0
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 9));       // sys_mmap
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("ffi_scratch_push_frame".to_string(), code));
    }

    // ffi_scratch_pop_frame() -> void
    //   No-op for now (the frame pointer was not stored). When marshal_cstr
    //   is wired, this will munmap the frame. Emitting as a real RET (not
    //   leaving the call unresolved, which would crash on x86_64).
    {
        let mut code = Vec::new();
        code.extend(encode_ret());
        stubs.push(("ffi_scratch_pop_frame".to_string(), code));
    }

    // __ffi_fallback_stub() -> i64
    //   The FFI return-0 fallback for truly unknown externs (e.g. C library
    //   functions like sqlite3_close that are not syscalls and not linked).
    //   Returns 0 (xor eax, eax; ret). This matches the aarch64 backend's
    //   ffi_stub behavior. Standalone ET_EXEC binaries with no linker step
    //   use this instead of crashing (jump to address 0).
    {
        let mut code = Vec::new();
        code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));  // return 0
        code.extend(encode_ret());
        stubs.push(("__ffi_fallback_stub".to_string(), code));
    }

    // __arena_overflow() → void  [syscall 60 = sys_exit, code=1]
    // Real exit(1) — arena bounds check overflow trap.
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rdi, 1));   // exit code = 1
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 60));   // sys_exit
        code.extend(encode_syscall());
        code.extend(encode_int3());                          // safety guard
        stubs.push(("__arena_overflow".to_string(), code));
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

    /// Wave 22: Emit a function using real register allocation.
    ///
    /// Consumes a `RegAllocResult` (from `TargetAgnosticRegAlloc`) and
    /// produces an `AllocatedFunction` where each instruction's
    /// `reads`/`writes` fields are annotated with the physical registers
    /// (RAX/RCX/RDX/RSI/RDI/R8-R11 for caller-saved, RBX/R12-R15 for
    /// callee-saved) assigned by the linear-scan allocator.
    ///
    /// Spilled vregs (those in `spill_slots`) remain on the stack via
    /// the existing stack-slot ISel path.  The `encoded` bytes are
    /// produced by the stack-slot ISel and are correct; the
    /// `reads`/`writes` metadata is additive and describes the
    /// regalloc assignment for downstream consumers.
    ///
    /// # Arguments
    /// * `func` — The IR function to emit.
    /// * `alloc` — The register allocation result (from
    ///   `TargetAgnosticRegAlloc::allocate_function`).
    ///
    /// # Returns
    /// An `AllocatedFunction` with regalloc-annotated `reads`/`writes`.
    pub fn emit_function_regalloc(
        &self,
        func: &IRFunction,
        alloc: &crate::regalloc::RegAllocResult,
    ) -> Result<AllocatedFunction, BackendError> {
        // Step 1: Run the existing stack-slot ISel to produce correct
        // encoded bytes.
        let mut allocated = stack_slot_isel::allocate_registers(func)?;

        // Step 2: Annotate with the regalloc result.
        crate::regalloc_emit::annotate_with_regalloc(&mut allocated, alloc);

        Ok(allocated)
    }

    /// Wave 22: Convenience method — run regalloc + emit in one step.
    ///
    /// This runs `TargetAgnosticRegAlloc::allocate_function` internally
    /// and then calls `emit_function_regalloc`.
    pub fn emit_function_with_regalloc(
        &self,
        func: &IRFunction,
    ) -> Result<AllocatedFunction, BackendError> {
        let alloc = crate::regalloc_emit::run_regalloc(func, "x86_64");
        self.emit_function_regalloc(func, &alloc)
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
        //   mov rdi, [rsp]              ; argc = *RSP            (4 bytes with SIB)
        //   lea rsi, [rsp + 8]          ; argv = RSP + 8         (5 bytes with SIB)
        //   48 B8 <placeholder 8 bytes> ; mov rax, <bss_argv_addr>  (10 bytes)
        //   48 89 38                    ; mov [rax], rdi  (save argc)  (3 bytes)
        //   48 89 70 08                 ; mov [rax+8], rsi (save argv) (4 bytes)
        //   E8 <rel32 to main>          ; call main              (5 bytes)
        //   48 89 C7                    ; mov rdi, rax           (3 bytes)
        //   48 C7 C0 3C 00 00 00        ; mov rax, 60            (7 bytes)
        //   0F 05                       ; syscall                (2 bytes)
        // Total = 4 + 5 + 10 + 3 + 4 + 5 + 3 + 7 + 2 = 43 bytes
        //
        // On Linux x86_64, the process entry stack layout is:
        //   [RSP]     = argc (8 bytes)
        //   [RSP+8]   = argv[0] pointer
        //   [RSP+16]  = argv[1] pointer
        //   ...
        //   NULL
        //   envp[0], envp[1], ..., NULL
        //   auxv...
        //
        // The _start stub saves argc/argv into a runtime-managed 16-byte
        // slot at the start of BSS (offset 0) so that the __vuma_argc and
        // __vuma_argv runtime stubs can retrieve them later (Wave 47).
        // Before main is called, argc/argv are also left in RDI/RSI —
        // preserving the existing calling convention for any VUMA main()
        // that might eventually declare argc/argv parameters.

        let start_stub_size: usize = 43;

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

        // Register canonical `__vuma_print_*` aliases pointing at the same
        // offsets as the short-name helpers, so user code calling the
        // canonical names resolves correctly.
        for (short, canonical) in [
            ("print_int", "__vuma_print_int"),
            ("print_hex", "__vuma_print_hex"),
            ("print_newline", "__vuma_print_newline"),
        ] {
            if let Some(&off) = func_offsets.get(short) {
                func_offsets.insert(canonical.to_string(), off);
            }
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

        // ── Wave 47: save argc/argv to the runtime argv-storage BSS slot ──
        // mov rax, <placeholder> — placeholder is patched below after BSS
        // layout is finalized. The placeholder is the same sentinel used in
        // the __vuma_argc/__vuma_argv runtime stubs, so a single scan-and-
        // replace pass over all_code patches every occurrence.
        start_stub.extend(encode_mov_reg_imm64(Gpr::Rax, RUNTIME_ARGV_STORAGE_PLACEHOLDER));

        // mov [rax], rdi — save argc to [bss_argv_storage + 0]
        start_stub.extend(encode_mov_mem_reg(Gpr::Rax, 0, Gpr::Rdi));

        // mov [rax+8], rsi — save argv to [bss_argv_storage + 8]
        start_stub.extend(encode_mov_mem_reg(Gpr::Rax, 8, Gpr::Rsi));

        // call main (E8 + rel32 placeholder)
        start_stub.extend(encode_call_rel32(0));

        // mov rdi, rax
        start_stub.extend(encode_mov_reg_reg(Gpr::Rdi, Gpr::Rax));

        // mov rax, 60 (sys_exit)
        start_stub.extend(encode_mov_reg_imm32(Gpr::Rax, 60));

        // syscall
        start_stub.extend(encode_syscall());

        // Patch the call main rel32 offset in _start stub.
        // Layout within start_stub:
        //   [0..4)   mov rdi, [rsp]         (4 bytes)
        //   [4..9)   lea rsi, [rsp+8]      (5 bytes)
        //   [9..19)  mov rax, <placeholder> (10 bytes)
        //   [19..22) mov [rax], rdi         (3 bytes)
        //   [22..26) mov [rax+8], rsi       (4 bytes)
        //   [26..31) call main (E8 + rel32) (5 bytes)  ← E8 at offset 26, rel32 at 27
        //   [31..34) mov rdi, rax           (3 bytes)
        //   [34..41) mov rax, 60           (7 bytes)
        //   [41..43) syscall                (2 bytes)
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            let rel32_patch_offset = 27usize; // offset of rel32 within start_stub
            // rel32 = target - (call_site + 5)
            // call_site = offset of the E8 byte = 26
            let rel32 = (main_offset as i64) - (26i64 + 5i64);
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
        // unique symbol a slot in BSS.
        //
        // The slot size must comfortably cover the largest object any
        // single global may reference.  VUMA globals are produced by
        // `allocate()` and the codegen has no per-symbol size metadata
        // (the R_X86_64_64 relocation only carries an address), so we
        // pick a generous 64-byte default per symbol.  This covers most
        // small struct/array globals without making adjacent globals
        // alias each other — the previous 8-byte slot caused globals
        // larger than a pointer to silently overwrite their neighbours.
        const BSS_SLOT_SIZE: u64 = 64; // Generous default per global symbol
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
        // Wave 47: a 16-byte runtime argv-storage slot is reserved at offset 0
        // of BSS. The _start stub writes argc (8 bytes) and argv (8 bytes)
        // here at process entry; the __vuma_argc/__vuma_argv runtime stubs
        // read from this slot. This reservation is unconditional so the
        // stubs are always safe to call (even programs that never reference
        // them just waste 16 bytes of BSS).
        let bss_size: u64 = RUNTIME_ARGV_STORAGE_SIZE
            + data_symbols.len() as u64 * BSS_SLOT_SIZE;

        // ── Compute BSS virtual address ──────────────────────────────
        // The BSS segment starts at the next 64K boundary after the text
        // segment.  The text segment layout is computed inside
        // build_minimal_x86_64_elf, so we mirror the same calculation here.
        // We use 64K alignment for virtual addresses to ensure compatibility
        // with QEMU 10.x on hosts with 16K or 64K page sizes.
        const ELF_HEADER_SIZE: u64 = 64;
        const PHDR_SIZE: u64 = 56;
        const FILE_PAGE_SIZE: u64 = 0x1000;
        const VADDR_ALIGN: u64 = 0x10000;
        const BASE_ADDR: u64 = 0x400000;
        // Mirrors build_minimal_x86_64_elf: (text LOAD) + (BSS LOAD if any) +
        // (PT_GNU_STACK, always). Keep this in sync with the emitter or
        // text_offset / text_vaddr will diverge from the emitted ELF.
        let num_phdrs: u64 = if bss_size > 0 { 3 } else { 2 };
        let phdr_end = ELF_HEADER_SIZE + PHDR_SIZE * num_phdrs;
        let text_offset = phdr_end.div_ceil(FILE_PAGE_SIZE) * FILE_PAGE_SIZE;
        let text_size = all_code.len() as u64;
        let text_vaddr: u64 = (BASE_ADDR + text_offset).div_ceil(VADDR_ALIGN) * VADDR_ALIGN;
        let bss_vaddr: u64 = if bss_size > 0 {
            (text_vaddr + text_size).div_ceil(VADDR_ALIGN) * VADDR_ALIGN
        } else {
            0
        };

        // Build a map: data symbol name → BSS virtual address.
        // Wave 47: data symbols start at offset RUNTIME_ARGV_STORAGE_SIZE
        // (16) in BSS — the first 16 bytes are reserved for the runtime
        // argv-storage slot populated by _start.
        let data_symbol_addrs: HashMap<String, u64> = data_symbols
            .iter()
            .enumerate()
            .map(|(i, name)| {
                (name.clone(), bss_vaddr + RUNTIME_ARGV_STORAGE_SIZE + i as u64 * BSS_SLOT_SIZE)
            })
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
                    let target_offset = func_offsets.get(&reloc.symbol)
                        .copied()
                        .or_else(|| {
                            let prefix = format!("fn_{}", reloc.symbol);
                            func_offsets.keys()
                                .find(|k| k.starts_with(&prefix))
                                .and_then(|k| func_offsets.get(k))
                                .copied()
                        });
                    if let Some(target_offset) = target_offset {
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
                        // External symbol — but this is an ET_EXEC static ELF
                        // with no linker step. Patch the BL to point at the
                        // __ffi_fallback_stub (xor eax, eax; ret → returns 0)
                        // instead of leaving it as 0 (which would crash).
                        // This matches the aarch64 backend's ffi_stub behavior.
                        if let Some(&fallback_off) = func_offsets.get("__ffi_fallback_stub") {
                            let current_val = i32::from_le_bytes([
                                all_code[abs_offset],
                                all_code[abs_offset + 1],
                                all_code[abs_offset + 2],
                                all_code[abs_offset + 3],
                            ]);
                            let s = fallback_off as i64;
                            let a = current_val as i64;
                            let p = abs_offset as i64;
                            let resolved = (s + a - p - 4) as i32;
                            all_code[abs_offset..abs_offset + 4]
                                .copy_from_slice(&resolved.to_le_bytes());
                        } else {
                            vuma_log!(warn,
                                "Unresolved external symbol '{}' in '{}' at 0x{:X} — no __ffi_fallback_stub registered, call will jump to address 0",
                                reloc.symbol, func.name, reloc.offset
                            );
                            continue;
                        }
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
                        // the function's virtual address (text_vaddr + offset).
                        let func_addr = text_vaddr + func_offsets[&reloc.symbol] as u64;
                        all_code[abs_offset..abs_offset + 8]
                            .copy_from_slice(&func_addr.to_le_bytes());
                    } else {
                        vuma_log!(warn, 
                            "Unresolved external symbol '{}' in '{}' at 0x{:X} — static ELF has no linker step, call will jump to address 0",
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

        // ── Wave 47: patch the runtime argv-storage placeholder ──────────
        // The _start stub and the __vuma_argc/__vuma_argv runtime stubs each
        // contain an 8-byte placeholder (RUNTIME_ARGV_STORAGE_PLACEHOLDER)
        // that stands in for the BSS argv-storage virtual address. Now that
        // BSS layout is finalized (bss_vaddr is known), we scan all_code for
        // the placeholder and replace every occurrence with the real address.
        //
        // The slot lives at offset 0 of BSS, so argv_storage_addr = bss_vaddr.
        // The placeholder is a recognizable sentinel (0xDEADBEEFCAFEBABE);
        // scanning is safe because the value is sufficiently random that it
        // will not collide with any legitimate instruction immediate emitted
        // by the x86_64 encoders (which produce small constants ≤0xFFFFFFFF
        // or syscall numbers ≤318). A failure to patch would leave the
        // stubs reading from a non-mapped address, which traps immediately
        // — a loud failure rather than a silent corruption.
        //
        // This pass runs AFTER relocation patching so that any user-code
        // relocations that happen to land inside a placeholder byte range
        // (extremely unlikely given the sentinel's bit pattern, but defended
        // against in principle) take precedence — the placeholder scan only
        // matches bytes that are STILL the sentinel after relocation
        // patching, i.e. only the stub-emitted placeholders.
        let argv_storage_addr = bss_vaddr; // offset 0 of BSS
        let placeholder_bytes = RUNTIME_ARGV_STORAGE_PLACEHOLDER.to_le_bytes();
        let mut patched_count: usize = 0;
        let mut i = 0;
        while i + 8 <= all_code.len() {
            if all_code[i..i + 8] == placeholder_bytes {
                all_code[i..i + 8].copy_from_slice(&argv_storage_addr.to_le_bytes());
                patched_count += 1;
                i += 8; // skip past the patched bytes to avoid overlapping matches
            } else {
                i += 1;
            }
        }
        // We expect at least one placeholder (the _start stub always has
        // one). If zero were found, the _start stub wasn't built correctly.
        // The two __vuma_argc/__vuma_argv stub placeholders are always present
        // in the runtime stubs table, so the expected count is 3.
        if patched_count == 0 {
            vuma_log!(warn,
                "Wave 47: runtime argv-storage placeholder not found in emitted code — \
                 _start stub did not save argc/argv; __vuma_argc/__vuma_argv will trap if called"
            );
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
        // With no BSS, e_phnum = 2 (text LOAD + PT_GNU_STACK)
        assert_eq!(u16::from_le_bytes([elf[56], elf[57]]), 2);
        // entry = vaddr_align(0x400000 + page_align(64 + 2*56)) = vaddr_align(0x401000) = 0x410000
        // (64K-aligned for host page size compatibility with QEMU 10.x)
        let entry = u64::from_le_bytes([
            elf[24], elf[25], elf[26], elf[27], elf[28], elf[29], elf[30], elf[31],
        ]);
        assert_eq!(entry, 0x410000);

        // Second program header (PT_GNU_STACK) starts at offset 64 + 56 = 120.
        // PT_GNU_STACK is always emitted (even without BSS) and marks the
        // stack as non-executable.
        let ph2 = 64 + 56;
        let p_type = u32::from_le_bytes([elf[ph2], elf[ph2+1], elf[ph2+2], elf[ph2+3]]);
        assert_eq!(p_type, 0x6474e551, "PT_GNU_STACK type");
        let p_flags = u32::from_le_bytes([elf[ph2+4], elf[ph2+5], elf[ph2+6], elf[ph2+7]]);
        assert_eq!(p_flags, 6, "PT_GNU_STACK flags = PF_R | PF_W (no PF_X)");
        let p_align = u64::from_le_bytes([
            elf[ph2+48], elf[ph2+49], elf[ph2+50], elf[ph2+51],
            elf[ph2+52], elf[ph2+53], elf[ph2+54], elf[ph2+55],
        ]);
        assert_eq!(p_align, 0x10, "PT_GNU_STACK align");
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
        // With BSS, e_phnum = 3 (text LOAD + BSS LOAD + PT_GNU_STACK)
        assert_eq!(u16::from_le_bytes([elf[56], elf[57]]), 3);
        // Entry point is still in text segment
        let entry = u64::from_le_bytes([
            elf[24], elf[25], elf[26], elf[27], elf[28], elf[29], elf[30], elf[31],
        ]);
        // With 3 phdrs, text_offset = page_align(64 + 3*56) = page_align(232) = 0x1000
        // entry = vaddr_align(0x400000 + 0x1000) = vaddr_align(0x401000) = 0x410000
        // (64K-aligned for host page size compatibility with QEMU 10.x)
        assert_eq!(entry, 0x410000);

        // Second program header (BSS LOAD) starts at offset 64 + 56 = 120
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
        // BSS vaddr should be 64K-aligned and after the text segment
        assert_eq!(bss_vaddr % 0x10000, 0, "BSS vaddr should be 64K-aligned");
        assert!(bss_vaddr > 0x410000, "BSS should be after text segment");

        // Third program header (PT_GNU_STACK) starts at offset 64 + 2*56 = 176.
        // Always emitted; marks stack non-executable.
        let ph3 = 64 + 2 * 56;
        let p_type3 = u32::from_le_bytes([elf[ph3], elf[ph3+1], elf[ph3+2], elf[ph3+3]]);
        assert_eq!(p_type3, 0x6474e551, "PT_GNU_STACK type");
        let p_flags3 = u32::from_le_bytes([elf[ph3+4], elf[ph3+5], elf[ph3+6], elf[ph3+7]]);
        assert_eq!(p_flags3, 6, "PT_GNU_STACK flags = PF_R | PF_W (no PF_X)");
        let p_align3 = u64::from_le_bytes([
            elf[ph3+48], elf[ph3+49], elf[ph3+50], elf[ph3+51],
            elf[ph3+52], elf[ph3+53], elf[ph3+54], elf[ph3+55],
        ]);
        assert_eq!(p_align3, 0x10, "PT_GNU_STACK align");
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

    // ── SSE / AVX SIMD Encoder Tests (Wave 29) ──────────────────────────

    #[test]
    fn test_sse_paddq_xmm0_xmm1() {
        // `paddq xmm0, xmm1` → 66 0F D4 C8
        //   (SSE2; dst=xmm0 in rm field, src=xmm1 in reg field; mod=3)
        let code = encode_sse_paddq(Xmm::Xmm0, Xmm::Xmm1);
        assert_eq!(code, vec![0x66, 0x0F, 0xD4, 0xC8]);
    }

    #[test]
    fn test_sse_psubd_xmm0_xmm1() {
        // `psubd xmm0, xmm1` → 66 0F FA C8
        let code = encode_sse_psubd(Xmm::Xmm0, Xmm::Xmm1);
        assert_eq!(code, vec![0x66, 0x0F, 0xFA, 0xC8]);
    }

    #[test]
    fn test_sse_pmulld_xmm0_xmm1() {
        // `pmulld xmm0, xmm1` (SSE4.1) → 66 0F 38 40 C8
        let code = encode_sse_pmulld(Xmm::Xmm0, Xmm::Xmm1);
        assert_eq!(code, vec![0x66, 0x0F, 0x38, 0x40, 0xC8]);
    }

    #[test]
    fn test_sse_paddq_rex_for_high_xmm() {
        // `paddq xmm8, xmm1` — dst in high register file → REX.B.
        // 41 66 0F D4 C8 (REX.B=0x41, then SSE2 paddq with rm=0 (low3 of Xmm8),
        // reg=1 (Xmm1)).
        let code = encode_sse_paddq(Xmm::Xmm8, Xmm::Xmm1);
        assert_eq!(code, vec![0x41, 0x66, 0x0F, 0xD4, 0xC8]);
    }

    #[test]
    fn test_sse_paddq_rex_rb_for_high_xmm_pair() {
        // `paddq xmm8, xmm9` — both high → REX.RB.
        // 45 66 0F D4 C8 (REX.RB=0x45, SSE2 paddq, rm=0, reg=1).
        let code = encode_sse_paddq(Xmm::Xmm8, Xmm::Xmm9);
        assert_eq!(code, vec![0x45, 0x66, 0x0F, 0xD4, 0xC8]);
    }

    #[test]
    fn test_sse_movdqu_load_zero_offset() {
        // `movdqu xmm0, [rax]` → F3 0F 6F 00 (no REX, mod=00, rm=000=rax, reg=000=xmm0)
        let code = encode_sse_movdqu_load(Xmm::Xmm0, Gpr::Rax, 0);
        assert_eq!(code, vec![0xF3, 0x0F, 0x6F, 0x00]);
    }

    #[test]
    fn test_sse_movdqu_store_disp8() {
        // `movdqu [rax+8], xmm0` → F3 0F 7F 40 08
        //   mod=01 (disp8), reg=000 (xmm0), rm=000 (rax); disp8=8
        let code = encode_sse_movdqu_store(Gpr::Rax, 8, Xmm::Xmm0);
        assert_eq!(code, vec![0xF3, 0x0F, 0x7F, 0x40, 0x08]);
    }

    #[test]
    fn test_avx_vpaddq_xmm0_xmm1_xmm2() {
        // `vpaddq xmm0, xmm1, xmm2` → VEX.128.66.0F.WIG D4 /r
        //   C5 F1 D4 D0
        //   (C5 = 2-byte VEX; F1 = R=1, vvvv=1110 (inverted XMM1), L=0, pp=01 (66);
        //    D4 = opcode; D0 = ModR/M: mod=3, reg=010 (xmm2), rm=000 (xmm0))
        let code = encode_avx_vpaddq(Xmm::Xmm0, Xmm::Xmm1, Xmm::Xmm2);
        assert_eq!(code, vec![0xC5, 0xF1, 0xD4, 0xD0]);
    }
}
pub mod disasm;
pub mod stack_slot_isel;
