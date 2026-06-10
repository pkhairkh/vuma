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
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, AllocatedProgram, Backend,
    BackendError, PhysicalReg, RegClass, TargetInfo, X86_64TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRValue, UnaryOpKind};
use std::collections::HashMap;
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
        matches!(self, Gpr::Rbx | Gpr::R12 | Gpr::R13 | Gpr::R14 | Gpr::R15 | Gpr::Rbp)
    }

    /// Returns `true` if this register is an integer argument register under SystemV ABI.
    pub fn is_arg_reg(&self) -> bool {
        matches!(self, Gpr::Rdi | Gpr::Rsi | Gpr::Rdx | Gpr::Rcx | Gpr::R8 | Gpr::R9)
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
            let hex_bytes: Vec<String> = bytes[start..end].iter().map(|b| format!("{:02x}", b)).collect();
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
                    let imm = u64::from_le_bytes(bytes[pos..pos+8].try_into().unwrap_or([0;8]));
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
                    let rel = i32::from_le_bytes(bytes[pos..pos+4].try_into().unwrap_or([0;4]));
                    pos += 4;
                    format!("jmp {:#x}", (start_pc + (pos - start) as u64).wrapping_add(rel as u64))
                } else {
                    pos = bytes.len();
                    "jmp ???".to_string()
                }
            }

            // CALL rel32
            0xE8 => {
                if pos + 4 <= bytes.len() {
                    let rel = i32::from_le_bytes(bytes[pos..pos+4].try_into().unwrap_or([0;4]));
                    pos += 4;
                    format!("call {:#x}", (start_pc + (pos - start) as u64).wrapping_add(rel as u64))
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
                                0 => "jo", 1 => "jno", 2 => "jb", 3 => "jae",
                                4 => "je", 5 => "jne", 6 => "jbe", 7 => "ja",
                                8 => "js", 9 => "jns", 0xA => "jp", 0xB => "jnp",
                                0xC => "jl", 0xD => "jge", 0xE => "jle", 0xF => "jg",
                                _ => "j??",
                            };
                            if pos + 4 <= bytes.len() {
                                let rel = i32::from_le_bytes(bytes[pos..pos+4].try_into().unwrap_or([0;4]));
                                pos += 4;
                                format!("{} {:#x}", cc_name, (start_pc + (pos - start) as u64).wrapping_add(rel as u64))
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
                                0 => "seto", 1 => "setno", 2 => "setb", 3 => "setae",
                                4 => "sete", 5 => "setne", 6 => "setbe", 7 => "seta",
                                8 => "sets", 9 => "setns", 0xA => "setp", 0xB => "setnp",
                                0xC => "setl", 0xD => "setge", 0xE => "setle", 0xF => "setg",
                                _ => "set??",
                            };
                            format!("{} {}", cc_name, gpr_name_8(rm, rex != 0))
                        }
                        // CMOVcc r64, r64
                        0x40..=0x4F => {
                            let (r, rm, new_pos) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b);
                            pos = new_pos;
                            let cc_name = match op2 & 0xF {
                                0 => "cmovo", 1 => "cmovno", 2 => "cmovb", 3 => "cmovae",
                                4 => "cmove", 5 => "cmovne", 6 => "cmovbe", 7 => "cmova",
                                8 => "cmovs", 9 => "cmovns", 0xA => "cmovp", 0xB => "cmovnp",
                                0xC => "cmovl", 0xD => "cmovge", 0xE => "cmovle", 0xF => "cmovg",
                                _ => "cmov??",
                            };
                            format!("{} {}, {}", cc_name, gpr_name_64(r), gpr_name_64(rm))
                        }
                        _ => format!("0f {:02x}", op2),
                    }
                }
            }

            // ALU reg-reg opcodes (with ModR/M byte)
            0x01 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("add {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x03 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("add {}, {}", gpr_name_64(r), gpr_name_64(rm)) }
            0x09 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("or {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x0B => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("or {}, {}", gpr_name_64(r), gpr_name_64(rm)) }
            0x21 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("and {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x23 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("and {}, {}", gpr_name_64(r), gpr_name_64(rm)) }
            0x29 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("sub {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x2B => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("sub {}, {}", gpr_name_64(r), gpr_name_64(rm)) }
            0x31 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("xor {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x33 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("xor {}, {}", gpr_name_64(r), gpr_name_64(rm)) }
            0x39 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("cmp {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x3B => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("cmp {}, {}", gpr_name_64(r), gpr_name_64(rm)) }
            0x85 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("test {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x87 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("xchg {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x89 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("mov {}, {}", gpr_name_64(rm), gpr_name_64(r)) }
            0x8B => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("mov {}, {}", gpr_name_64(r), gpr_name_64(rm)) }
            0x8D => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("lea {}, [{}]", gpr_name_64(r), gpr_name_64(rm)) }
            0x63 => { let (r, rm, np) = decode_modrm_reg_rm(bytes, pos, rex_r, rex_b); pos = np; format!("movsxd {}, {}", gpr_name_64(r), gpr_name_64(rm)) }

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
                    let imm = i32::from_le_bytes(bytes[pos..pos+4].try_into().unwrap_or([0;4]));
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
                    let imm = i32::from_le_bytes(bytes[pos..pos+4].try_into().unwrap_or([0;4]));
                    pos += 4;
                    let op_name = match r {
                        0 => "add", 1 => "or", 2 => "adc", 3 => "sbb",
                        4 => "and", 5 => "sub", 6 => "xor", 7 => "cmp",
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
        let hex_bytes: Vec<String> = bytes[start..end].iter().map(|b| format!("{:02x}", b)).collect();
        lines.push(format!("{:#010x}:  {:20} {}", start_pc, hex_bytes.join(" "), mnemonic));

        offset = end;
        pc = start_pc + (end - start) as u64;
    }

    lines
}

/// Helper: get 64-bit GPR name from index (0-15).
fn gpr_name_64(idx: u8) -> &'static str {
    match idx & 0xF {
        0 => "rax", 1 => "rcx", 2 => "rdx", 3 => "rbx",
        4 => "rsp", 5 => "rbp", 6 => "rsi", 7 => "rdi",
        8 => "r8", 9 => "r9", 10 => "r10", 11 => "r11",
        12 => "r12", 13 => "r13", 14 => "r14", 15 => "r15",
        _ => "r??",
    }
}

/// Helper: get 8-bit GPR name from index (0-15).
fn gpr_name_8(idx: u8, has_rex: bool) -> &'static str {
    match idx & 0xF {
        0 => if has_rex { "r8b" } else { "al" },
        1 => if has_rex { "r9b" } else { "cl" },
        2 => if has_rex { "r10b" } else { "dl" },
        3 => if has_rex { "r11b" } else { "bl" },
        4 => if has_rex { "r12b" } else { "spl" }, // REX required for spl
        5 => if has_rex { "r13b" } else { "bpl" },
        6 => if has_rex { "r14b" } else { "sil" },
        7 => if has_rex { "r15b" } else { "dil" },
        8 => "r8b", 9 => "r9b", 10 => "r10b", 11 => "r11b",
        12 => "r12b", 13 => "r13b", 14 => "r14b", 15 => "r15b",
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
/// Produces a static executable with a single LOAD segment containing the
/// `.text` section. Entry point is at `base_addr` + header offset.
fn build_minimal_x86_64_elf(code: &[u8], base_addr: u64) -> Vec<u8> {
    let elf_header_size: u64 = 64;
    let phdr_size: u64 = 56;
    let text_offset = elf_header_size + phdr_size;
    let text_size = code.len() as u64;
    let entry_point = base_addr + text_offset;

    let mut elf = Vec::with_capacity(text_offset as usize + code.len());

    // --- e_ident ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']); // magic
    elf.push(2);   // ELFCLASS64
    elf.push(1);   // ELFDATA2LSB
    elf.push(1);   // EV_CURRENT
    elf.push(3);   // ELFOSABI_LINUX
    elf.push(0);   // padding
    elf.extend_from_slice(&[0u8; 7]); // padding

    // --- ELF header fields ---
    elf.extend_from_slice(&2u16.to_le_bytes());       // e_type = ET_EXEC
    elf.extend_from_slice(&62u16.to_le_bytes());      // e_machine = EM_X86_64
    elf.extend_from_slice(&1u32.to_le_bytes());       // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff
    elf.extend_from_slice(&0u64.to_le_bytes());       // e_shoff (no section headers)
    elf.extend_from_slice(&0u32.to_le_bytes());       // e_flags
    elf.extend_from_slice(&64u16.to_le_bytes());      // e_ehsize
    elf.extend_from_slice(&56u16.to_le_bytes());      // e_phentsize
    elf.extend_from_slice(&1u16.to_le_bytes());       // e_phnum
    elf.extend_from_slice(&64u16.to_le_bytes());      // e_shentsize
    elf.extend_from_slice(&0u16.to_le_bytes());       // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes());       // e_shstrndx

    // --- Program Header (single LOAD segment: PF_R | PF_X) ---
    elf.extend_from_slice(&1u32.to_le_bytes());       // p_type = PT_LOAD
    elf.extend_from_slice(&5u32.to_le_bytes());       // p_flags = PF_R | PF_X
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_vaddr
    elf.extend_from_slice(&(base_addr + text_offset).to_le_bytes()); // p_paddr
    elf.extend_from_slice(&text_size.to_le_bytes());  // p_filesz
    elf.extend_from_slice(&text_size.to_le_bytes());  // p_memsz
    elf.extend_from_slice(&16u64.to_le_bytes());      // p_align

    // --- Code section ---
    elf.extend_from_slice(code);

    elf
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

/// Compute the stack frame size for an IR function on x86_64.
///
/// Sums `Alloc` instruction sizes, adds 8 bytes for the RBP save,
/// and rounds up to 16-byte alignment.
fn x86_64_compute_frame_size(func: &IRFunction) -> usize {
    let mut total: usize = 8; // Saved RBP
    for block in &func.blocks {
        for instr in &block.instructions {
            if let crate::ir::IRInstr::Alloc { size, .. } = instr {
                let aligned = (*size as usize).div_ceil(16) * 16;
                total += aligned;
            }
        }
    }
    // Round up to 16-byte alignment
    (total + 15) & !15
}

// ── ISel helpers ─────────────────────────────────────────────────────────

/// Resolve an IRValue to a physical GPR.
/// For registers, looks up in the reg_map. For immediates, loads the value
/// into `scratch` and returns `scratch`. For addresses, loads into `scratch`.
fn resolve_gpr(val: &IRValue, reg_map: &HashMap<u32, Gpr>, scratch: Gpr) -> (Gpr, Vec<u8>) {
    match val {
        IRValue::Register(id) => (
            reg_map.get(id).copied().unwrap_or(Gpr::Rax),
            Vec::new(),
        ),
        IRValue::Immediate(imm) => {
            let imm = *imm;
            let code = if (-2147483648..=2147483647).contains(&imm) {
                encode_mov_reg_imm32(scratch, imm as i32)
            } else {
                encode_mov_reg_imm64(scratch, imm as u64)
            };
            (scratch, code)
        }
        IRValue::Address(addr) => {
            let code = encode_mov_reg_imm64(scratch, *addr);
            (scratch, code)
        }
        IRValue::Label(_) => {
            // Labels need relocation; emit a placeholder mov for now
            let code = encode_mov_reg_imm64(scratch, 0);
            (scratch, code)
        }
    }
}

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

/// Emit a CMP + SETcc + zero-extend sequence for a comparison that produces
/// a boolean (0 or 1) in the destination register.
fn emit_cmp_setcc(dst: Gpr, lhs: Gpr, rhs: Gpr, cc: Cc) -> Vec<u8> {
    let mut code = Vec::new();
    code.extend(encode_cmp_reg_reg(lhs, rhs));
    code.extend(encode_setcc(cc, dst));
    // Zero-extend the byte result to 64 bits to clear upper bits
    code.extend(encode_movzx_reg8(dst, dst));
    code
}

impl Backend for X86_64Backend {
    fn target_info(&self) -> &dyn TargetInfo {
        &self.target_info
    }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        // Simple register allocation: map virtual registers to physical registers
        // in a round-robin fashion over allocatable GPRs.
        let allocatable: Vec<Gpr> = [
            Gpr::Rax, Gpr::Rcx, Gpr::Rdx, Gpr::Rsi, Gpr::Rdi,
            Gpr::R8, Gpr::R9, Gpr::R10, Gpr::R11,
            Gpr::R12, Gpr::R13, Gpr::R14, Gpr::R15,
            Gpr::Rbx, Gpr::Rbp,
        ]
        .to_vec();

        let func_name = func.name.clone();
        let frame_size = x86_64_compute_frame_size(func);

        // Collect all virtual register IDs
        let mut vreg_ids: Vec<u32> = Vec::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                for id in instr.defined_regs() {
                    if !vreg_ids.contains(&id) {
                        vreg_ids.push(id);
                    }
                }
            }
        }

        // Map virtual registers to physical registers
        let mut reg_map: HashMap<u32, Gpr> = HashMap::new();
        for (i, &id) in vreg_ids.iter().enumerate() {
            reg_map.insert(id, allocatable[i % allocatable.len()]);
        }

        // Determine callee-saved registers used
        let callee_saved: Vec<PhysicalReg> = reg_map
            .values()
            .filter(|r| r.is_callee_saved())
            .map(|r| PhysicalReg::new(RegClass::Gpr, r.encoding() as u32))
            .collect();

        // Generate prologue
        let mut encoded_instrs: Vec<AllocatedInstruction> = Vec::new();

        // push rbp
        encoded_instrs.push(AllocatedInstruction {
            opcode: "push".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Rbp.encoding() as u32)],
            writes: vec![],
            encoded: encode_push(Gpr::Rbp),
        });

        // mov rbp, rsp
        encoded_instrs.push(AllocatedInstruction {
            opcode: "mov".to_string(),
            reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Rsp.encoding() as u32)],
            writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Rbp.encoding() as u32)],
            encoded: encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp),
        });

        // sub rsp, frame_size
        if frame_size > 0 {
            encoded_instrs.push(AllocatedInstruction {
                opcode: "sub".to_string(),
                reads: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Rsp.encoding() as u32)],
                writes: vec![PhysicalReg::new(RegClass::Gpr, Gpr::Rsp.encoding() as u32)],
                encoded: encode_sub_reg_imm32(Gpr::Rsp, frame_size as i32),
            });
        }

        // Encode each IR instruction
        for block in &func.blocks {
            for instr in &block.instructions {
                let encoded = match instr {
                    // ── Dedicated Add/Sub/Mul (with immediate-form optimisations) ──
                    IRInstr::Add { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (l, mut code) = resolve_gpr(lhs, &reg_map, Gpr::R10);
                        if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                        if let IRValue::Immediate(imm) = rhs {
                            if (-2147483648..=2147483647).contains(imm) {
                                code.extend(encode_add_reg_imm32(d, *imm as i32));
                            } else {
                                let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                                code.extend(pre);
                                code.extend(encode_add_reg_reg(d, r));
                            }
                        } else {
                            let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                            code.extend(pre);
                            code.extend(encode_add_reg_reg(d, r));
                        }
                        code
                    }
                    IRInstr::Sub { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (l, mut code) = resolve_gpr(lhs, &reg_map, Gpr::R10);
                        if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                        if let IRValue::Immediate(imm) = rhs {
                            if (-2147483648..=2147483647).contains(imm) {
                                code.extend(encode_sub_reg_imm32(d, *imm as i32));
                            } else {
                                let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                                code.extend(pre);
                                code.extend(encode_sub_reg_reg(d, r));
                            }
                        } else {
                            let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                            code.extend(pre);
                            code.extend(encode_sub_reg_reg(d, r));
                        }
                        code
                    }
                    IRInstr::Mul { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (l, mut code) = resolve_gpr(lhs, &reg_map, Gpr::R10);
                        let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                        code.extend(pre);
                        if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                        code.extend(encode_imul_reg_reg(d, r));
                        code
                    }

                    // ── Division: uses RAX/RDX implicitly ───────────────
                    IRInstr::Div { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (l, mut code) = resolve_gpr(lhs, &reg_map, Gpr::R10);
                        let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                        code.extend(pre);
                        // Move lhs into RAX
                        if l != Gpr::Rax { code.extend(encode_mov_reg_reg(Gpr::Rax, l)); }
                        // Sign-extend RAX into RDX:RAX
                        code.extend(encode_cqo());
                        // IDIV r
                        code.extend(encode_idiv_reg(r));
                        // Quotient is in RAX, move to dst if different
                        if d != Gpr::Rax { code.extend(encode_mov_reg_reg(d, Gpr::Rax)); }
                        code
                    }

                    // ── BinOp (generic) ──────────────────────────────────
                    IRInstr::BinOp { op, dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (l, mut code) = resolve_gpr(lhs, &reg_map, Gpr::R10);
                        let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                        code.extend(pre);

                        match op {
                            BinOpKind::Add => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                if let IRValue::Immediate(imm) = rhs {
                                    if (-2147483648..=2147483647).contains(imm) {
                                        code.extend(encode_add_reg_imm32(d, *imm as i32));
                                    } else {
                                        code.extend(encode_add_reg_reg(d, r));
                                    }
                                } else {
                                    code.extend(encode_add_reg_reg(d, r));
                                }
                            }
                            BinOpKind::Sub => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                if let IRValue::Immediate(imm) = rhs {
                                    if (-2147483648..=2147483647).contains(imm) {
                                        code.extend(encode_sub_reg_imm32(d, *imm as i32));
                                    } else {
                                        code.extend(encode_sub_reg_reg(d, r));
                                    }
                                } else {
                                    code.extend(encode_sub_reg_reg(d, r));
                                }
                            }
                            BinOpKind::Mul => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                code.extend(encode_imul_reg_reg(d, r));
                            }
                            BinOpKind::SDiv => {
                                if l != Gpr::Rax { code.extend(encode_mov_reg_reg(Gpr::Rax, l)); }
                                code.extend(encode_cqo());
                                code.extend(encode_idiv_reg(r));
                                if d != Gpr::Rax { code.extend(encode_mov_reg_reg(d, Gpr::Rax)); }
                            }
                            BinOpKind::UDiv => {
                                if l != Gpr::Rax { code.extend(encode_mov_reg_reg(Gpr::Rax, l)); }
                                // Zero-extend RAX into RDX for unsigned divide
                                code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx));
                                code.extend(encode_div_reg(r));
                                if d != Gpr::Rax { code.extend(encode_mov_reg_reg(d, Gpr::Rax)); }
                            }
                            BinOpKind::SRem => {
                                if l != Gpr::Rax { code.extend(encode_mov_reg_reg(Gpr::Rax, l)); }
                                code.extend(encode_cqo());
                                code.extend(encode_idiv_reg(r));
                                // Remainder in RDX
                                if d != Gpr::Rdx { code.extend(encode_mov_reg_reg(d, Gpr::Rdx)); }
                            }
                            BinOpKind::URem => {
                                if l != Gpr::Rax { code.extend(encode_mov_reg_reg(Gpr::Rax, l)); }
                                code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx));
                                code.extend(encode_div_reg(r));
                                // Remainder in RDX
                                if d != Gpr::Rdx { code.extend(encode_mov_reg_reg(d, Gpr::Rdx)); }
                            }
                            BinOpKind::And => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                code.extend(encode_and_reg_reg(d, r));
                            }
                            BinOpKind::Or => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                code.extend(encode_or_reg_reg(d, r));
                            }
                            BinOpKind::Xor => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                code.extend(encode_xor_reg_reg(d, r));
                            }
                            BinOpKind::Shl => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                // Shift amount must be in CL (RCX)
                                if r != Gpr::Rcx { code.extend(encode_mov_reg_reg(Gpr::Rcx, r)); }
                                code.extend(encode_shl_reg_cl(d));
                            }
                            BinOpKind::ShrL => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                if r != Gpr::Rcx { code.extend(encode_mov_reg_reg(Gpr::Rcx, r)); }
                                code.extend(encode_shr_reg_cl(d));
                            }
                            BinOpKind::ShrA => {
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                if r != Gpr::Rcx { code.extend(encode_mov_reg_reg(Gpr::Rcx, r)); }
                                code.extend(encode_sar_reg_cl(d));
                            }
                            // Comparison BinOps: produce 0 or 1
                            BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
                            | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
                            | BinOpKind::Eq | BinOpKind::Ne => {
                                let cc = binop_cmp_to_cc(op);
                                if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                                code.extend(emit_cmp_setcc(d, d, r, cc));
                            }
                        }
                        code
                    }

                    // ── Unary operations ─────────────────────────────────
                    IRInstr::UnaryOp { op, dst, operand } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (s, mut code) = resolve_gpr(operand, &reg_map, Gpr::R10);

                        match op {
                            UnaryOpKind::Neg => {
                                if s != d { code.extend(encode_mov_reg_reg(d, s)); }
                                code.extend(encode_neg_reg(d));
                            }
                            UnaryOpKind::Not => {
                                if s != d { code.extend(encode_mov_reg_reg(d, s)); }
                                code.extend(encode_not_reg(d));
                            }
                            UnaryOpKind::Clz => {
                                // BSR finds highest set bit; result = 63 - BSR
                                // BSR sets ZF if operand is 0
                                if s != d { code.extend(encode_mov_reg_reg(d, s)); }
                                // BSR dst, dst  (0F BD /r)
                                // We emit this manually since there's no helper
                                let r = d.needs_rex();
                                let b = d.needs_rex();
                                if let Some(rex) = rex_prefix(true, r, false, b) {
                                    code.push(rex);
                                } else {
                                    code.push(0x48);
                                }
                                code.push(0x0F);
                                code.push(0xBD);
                                code.push(modrm(3, d.encoding() & 7, d.encoding() & 7));
                                // XOR temp, temp; then if ZF, result = 64
                                // Simplified: result = 63 - BSR_result for non-zero
                                code.extend(encode_mov_reg_imm32(Gpr::R11, 63));
                                code.extend(encode_sub_reg_reg(Gpr::R11, d));
                                code.extend(encode_mov_reg_reg(d, Gpr::R11));
                            }
                            UnaryOpKind::Ctz => {
                                // BSF finds lowest set bit; result = BSF
                                if s != d { code.extend(encode_mov_reg_reg(d, s)); }
                                // BSF dst, dst  (0F BC /r)
                                let r = d.needs_rex();
                                let b = d.needs_rex();
                                if let Some(rex) = rex_prefix(true, r, false, b) {
                                    code.push(rex);
                                } else {
                                    code.push(0x48);
                                }
                                code.push(0x0F);
                                code.push(0xBC);
                                code.push(modrm(3, d.encoding() & 7, d.encoding() & 7));
                                // BSF result is already the count of trailing zeros
                            }
                            UnaryOpKind::Popcnt => {
                                if s != d { code.extend(encode_mov_reg_reg(d, s)); }
                                // POPCNT dst, dst  (F3 0F B8 /r)
                                // We need the F3 prefix
                                code.push(0xF3);
                                let r = d.needs_rex();
                                let b = d.needs_rex();
                                if let Some(rex) = rex_prefix(true, r, false, b) {
                                    code.push(rex);
                                } else {
                                    code.push(0x48);
                                }
                                code.push(0x0F);
                                code.push(0xB8);
                                code.push(modrm(3, d.encoding() & 7, d.encoding() & 7));
                            }
                        }
                        code
                    }

                    // ── Comparison (dedicated Cmp instruction) ───────────
                    IRInstr::Cmp { kind, dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (l, mut code) = resolve_gpr(lhs, &reg_map, Gpr::R10);
                        let cc = cmp_kind_to_cc(kind);
                        if l != d { code.extend(encode_mov_reg_reg(d, l)); }
                        if let IRValue::Immediate(imm) = rhs {
                            if (-2147483648..=2147483647).contains(imm) {
                                code.extend(encode_cmp_reg_imm32(d, *imm as i32));
                            } else {
                                let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                                code.extend(pre);
                                code.extend(encode_cmp_reg_reg(d, r));
                            }
                            code.extend(encode_setcc(cc, d));
                            code.extend(encode_movzx_reg8(d, d));
                        } else {
                            let (r, pre) = resolve_gpr(rhs, &reg_map, Gpr::R11);
                            code.extend(pre);
                            code.extend(emit_cmp_setcc(d, d, r, cc));
                        }
                        code
                    }

                    // ── Conditional select (Cmov) ────────────────────────
                    IRInstr::Select { dst, cond, true_val, false_val } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (c, mut code) = resolve_gpr(cond, &reg_map, Gpr::R10);
                        let (tv, pre_tv) = resolve_gpr(true_val, &reg_map, Gpr::R11);
                        let (fv, pre_fv) = resolve_gpr(false_val, &reg_map, Gpr::R8);
                        code.extend(pre_tv);
                        code.extend(pre_fv);

                        // Move false_val into dst first, then conditionally move true_val
                        if fv != d { code.extend(encode_mov_reg_reg(d, fv)); }
                        // Test cond != 0 (test sets ZF if cond == 0)
                        code.extend(encode_test_reg_reg(c, c));
                        // CMOVcc: if NOT zero (NZ = NotEqual), move true_val
                        code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, d, tv));
                        code
                    }

                    // ── Memory: Load ─────────────────────────────────────
                    IRInstr::Load { dst, addr } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (a, mut code) = resolve_gpr(addr, &reg_map, Gpr::R10);
                        code.extend(encode_mov_reg_mem(d, a, 0));
                        code
                    }

                    // ── Memory: Store ────────────────────────────────────
                    IRInstr::Store { value, addr } => {
                        let (v, mut code) = resolve_gpr(value, &reg_map, Gpr::R10);
                        let (a, pre) = resolve_gpr(addr, &reg_map, Gpr::R11);
                        code.extend(pre);
                        code.extend(encode_mov_mem_reg(a, 0, v));
                        code
                    }

                    // ── Memory: Lea (Offset) ─────────────────────────────
                    IRInstr::Offset { dst, base, offset } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (b, mut code) = resolve_gpr(base, &reg_map, Gpr::R10);
                        // If offset is an immediate, use LEA
                        match offset {
                            IRValue::Immediate(imm) => {
                                let off = *imm as i32;
                                code.extend(encode_lea_reg_mem(d, b, off));
                            }
                            _ => {
                                let (o, pre) = resolve_gpr(offset, &reg_map, Gpr::R11);
                                code.extend(pre);
                                if b != d { code.extend(encode_mov_reg_reg(d, b)); }
                                code.extend(encode_add_reg_reg(d, o));
                            }
                        }
                        code
                    }

                    // ── GetAddress ───────────────────────────────────────
                    IRInstr::GetAddress { dst, name: _ } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        // Placeholder: load 0 as the address (relocation needed at link time)
                        encode_mov_reg_imm64(d, 0)
                    }

                    // ── Alloc ────────────────────────────────────────────
                    IRInstr::Alloc { dst, size: _ } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        // Alloc is handled by the frame layout; compute the address
                        // using LEA from RBP at the appropriate offset
                        encode_lea_reg_mem(d, Gpr::Rbp, -(frame_size as i32))
                    }

                    // ── Free ─────────────────────────────────────────────
                    IRInstr::Free { ptr: _ } => {
                        // Free is lowered to a runtime call; emit NOP for now
                        encode_nop()
                    }

                    // ── Cast / Conversion ────────────────────────────────
                    IRInstr::Cast { kind, dst, src } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let (s, mut code) = resolve_gpr(src, &reg_map, Gpr::R10);

                        match kind {
                            CastKind::ZExt => {
                                // Zero-extend: pick the right extension based on
                                // the source value width.  When the source is a
                                // register, default to byte (8→64) extension; when
                                // the source is an immediate that fits in 32 bits,
                                // a simple 32-bit mov already zero-extends.
                                if let IRValue::Immediate(imm) = src {
                                    if (-2147483648..=2147483647).contains(imm) {
                                        code.extend(encode_mov_reg_imm32(d, *imm as i32));
                                    } else {
                                        code.extend(encode_mov_reg_imm64(d, *imm as u64));
                                    }
                                } else {
                                    // Default: zero-extend byte → 64 bits
                                    code.extend(encode_movzx_reg8(d, s));
                                }
                            }
                            CastKind::SExt => {
                                // Sign-extend: use MOVSX from byte, word, or dword.
                                // Default to byte sign-extension for register sources;
                                // for immediates, a mov with sign-extension suffices.
                                if let IRValue::Immediate(imm) = src {
                                    if (-128..=127).contains(imm) {
                                        // Sign-extend byte: MOVSX r64, r8
                                        code.extend(encode_mov_reg_imm32(d, *imm as i32));
                                    } else if (-32768..=32767).contains(imm) {
                                        // Sign-extend word
                                        code.extend(encode_mov_reg_imm32(d, *imm as i32));
                                    } else {
                                        code.extend(encode_mov_reg_imm32(d, *imm as i32));
                                    }
                                } else {
                                    // Default: sign-extend byte → 64 bits
                                    code.extend(encode_movsx_reg8(d, s));
                                }
                            }
                            CastKind::Trunc => {
                                // Truncation: just move the lower bits (on x86_64,
                                // writing to a 32-bit register zero-extends to 64 bits)
                                if s != d { code.extend(encode_mov_reg_reg(d, s)); }
                            }
                            CastKind::BitCast => {
                                // No data change, just a type reinterpretation
                                if s != d { code.extend(encode_mov_reg_reg(d, s)); }
                            }
                        }
                        code
                    }

                    // ── Control: Ret ─────────────────────────────────────
                    IRInstr::Ret { values } => {
                        let mut code = Vec::new();
                        if let Some(val) = values.first() {
                            if let Some(id) = val.as_register() {
                                if let Some(&src) = reg_map.get(&id) {
                                    if src != Gpr::Rax {
                                        code.extend(encode_mov_reg_reg(Gpr::Rax, src));
                                    }
                                }
                            } else if let Some(imm) = val.as_immediate() {
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, imm as i32));
                            }
                        }
                        // Epilogue: add rsp, frame_size; pop rbp; ret
                        if frame_size > 0 {
                            code.extend(encode_add_reg_imm32(Gpr::Rsp, frame_size as i32));
                        }
                        code.extend(encode_pop(Gpr::Rbp));
                        code.extend(encode_ret());
                        code
                    }

                    // ── Control: Branch (unconditional) ──────────────────
                    IRInstr::Branch { target: _ } => {
                        // JMP rel32 — offset will need relocation at link time
                        encode_jmp_rel32(0)
                    }

                    // ── Control: CondBranch ──────────────────────────────
                    IRInstr::CondBranch { cond, true_target: _, false_target: _ } => {
                        let (c, mut code) = resolve_gpr(cond, &reg_map, Gpr::R10);
                        // Test cond != 0; JNZ to true_target; JMP to false_target
                        code.extend(encode_test_reg_reg(c, c));
                        // JNZ rel32 — placeholder offset
                        code.extend(encode_jcc_rel32(Cc::NotEqual, 0));
                        // JMP rel32 — placeholder offset for false target
                        code.extend(encode_jmp_rel32(0));
                        code
                    }

                    // ── Call ─────────────────────────────────────────────
                    IRInstr::Call { dst, func: _, args } => {
                        let mut code = Vec::new();

                        // Move arguments into SystemV arg registers (RDI, RSI, RDX, RCX, R8, R9)
                        let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
                        for (i, arg) in args.iter().enumerate() {
                            if i < arg_regs.len() {
                                let (a, pre) = resolve_gpr(arg, &reg_map, Gpr::R10);
                                code.extend(pre);
                                if a != arg_regs[i] {
                                    code.extend(encode_mov_reg_reg(arg_regs[i], a));
                                }
                            }
                        }

                        // CALL rel32 — placeholder offset
                        code.extend(encode_call_rel32(0));

                        // Move return value from RAX to dst if needed
                        if let Some(d) = dst {
                            let dd = reg_map.get(&d.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                            if dd != Gpr::Rax {
                                code.extend(encode_mov_reg_reg(dd, Gpr::Rax));
                            }
                        }
                        code
                    }

                    // ── Phi ──────────────────────────────────────────────
                    IRInstr::Phi { .. } => {
                        // Phi nodes should be eliminated before codegen; emit NOP
                        encode_nop()
                    }
                };

                if !encoded.is_empty() {
                    encoded_instrs.push(AllocatedInstruction {
                        opcode: format!("{:?}", instr).split_whitespace().next().unwrap_or("unknown").to_string(),
                        reads: vec![],
                        writes: vec![],
                        encoded,
                    });
                }
            }
        }

        let code_size: usize = encoded_instrs.iter().map(|i| i.encoded.len()).sum();

        Ok(AllocatedFunction {
            name: func_name,
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions: encoded_instrs,
                code_offset: 0,
            }],
            frame_size,
            callee_saved,
            spill_slots: 0,
            code_size,
        })
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
        let mut all_code = Vec::new();
        for func in &program.functions {
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
        }
        Ok(build_minimal_x86_64_elf(&all_code, 0x400000))
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
        let elf = build_minimal_x86_64_elf(&code, 0x400000);

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
        // e_entry should be base + 64 + 56 = 0x400078
        let entry = u64::from_le_bytes([
            elf[24], elf[25], elf[26], elf[27],
            elf[28], elf[29], elf[30], elf[31],
        ]);
        assert_eq!(entry, 0x400078);
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
        });
        // Should contain an ADD r64, r64 instruction (opcode 0x01)
        assert!(code.iter().any(|&b| b == 0x01), "ADD opcode 0x01 not found in encoded output");
    }

    #[test]
    fn test_isel_add_imm32() {
        let code = isel_single_instr(IRInstr::Add {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(42),
        });
        // With immediate rhs, should use ADD r64, imm32 (opcode 0x81 /0)
        let has_add_imm = code.windows(2).any(|w| w[0] == 0x81 && (w[1] & 0xC0) == 0xC0 && (w[1] & 0x38) == 0x00);
        assert!(has_add_imm, "ADD r64, imm32 not found in encoded output");
    }

    #[test]
    fn test_isel_sub_reg_reg() {
        let code = isel_single_instr(IRInstr::Sub {
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
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
        });
        // With immediate, should use SUB r64, imm32 (0x81 /5)
        let has_sub_imm = code.windows(2).any(|w| w[0] == 0x81 && (w[1] & 0xC0) == 0xC0 && (w[1] & 0x38) == 0x28);
        assert!(has_sub_imm, "SUB r64, imm32 not found");
    }

    #[test]
    fn test_isel_binop_and() {
        let code = isel_single_instr(IRInstr::BinOp {
            op: BinOpKind::And,
            dst: IRValue::Register(0),
            lhs: IRValue::Register(1),
            rhs: IRValue::Register(2),
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
        });
        // SDiv uses CQO (0x48 0x99) + IDIV (0xF7 /7)
        assert!(code.windows(2).any(|w| w[0] == 0x48 && w[1] == 0x99), "CQO not found for SDiv");
        assert!(code.iter().any(|&b| b == 0xF7), "IDIV opcode not found for SDiv");
    }

    #[test]
    fn test_isel_unaryop_neg() {
        let code = isel_single_instr(IRInstr::UnaryOp {
            op: UnaryOpKind::Neg,
            dst: IRValue::Register(0),
            operand: IRValue::Register(1),
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
        });
        // CMP r64, imm32 (0x81 /7)
        let has_cmp_imm = code.windows(2).any(|w| w[0] == 0x81 && (w[1] & 0xC0) == 0xC0 && (w[1] & 0x38) == 0x38);
        assert!(has_cmp_imm, "CMP r64, imm32 not found");
        // Should also have SETcc (0F 9x) and MOVZX (0F B6)
        assert!(code.windows(2).any(|w| w[0] == 0x0F && w[1] >= 0x90 && w[1] <= 0x9F), "SETcc not found");
    }

    #[test]
    fn test_isel_cast_zext() {
        let code = isel_single_instr(IRInstr::Cast {
            kind: CastKind::ZExt,
            dst: IRValue::Register(0),
            src: IRValue::Register(1),
        });
        // ZExt of a register uses MOVZX r8→r64 (0F B6)
        assert!(code.windows(2).any(|w| w[0] == 0x0F && w[1] == 0xB6), "MOVZX r8 not found for ZExt");
    }

    #[test]
    fn test_isel_cast_sext() {
        let code = isel_single_instr(IRInstr::Cast {
            kind: CastKind::SExt,
            dst: IRValue::Register(0),
            src: IRValue::Register(1),
        });
        // SExt of a register uses MOVSX r8→r64 (0F BE)
        assert!(code.windows(2).any(|w| w[0] == 0x0F && w[1] == 0xBE), "MOVSX r8 not found for SExt");
    }

    #[test]
    fn test_isel_select() {
        let code = isel_single_instr(IRInstr::Select {
            dst: IRValue::Register(0),
            cond: IRValue::Register(1),
            true_val: IRValue::Register(2),
            false_val: IRValue::Register(3),
        });
        // Select uses TEST + CMOVcc
        assert!(code.windows(2).any(|w| w[0] == 0x48 && w[1] == 0x85), "TEST not found for Select");
        assert!(code.windows(2).any(|w| w[0] == 0x0F && w[1] >= 0x40 && w[1] <= 0x4F), "CMOVcc not found for Select");
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
        assert!(lines[0].contains("push"), "Expected push, got: {}", lines[0]);
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
