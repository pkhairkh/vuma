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
use crate::ir::IRFunction;
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
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if offset >= -128 && offset <= 127 {
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
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, src.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, src.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if offset >= -128 && offset <= 127 {
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
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst.encoding() & 7, 4));
            code.push(sib(0, 4, base.encoding() & 7));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if offset >= -128 && offset <= 127 {
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
// Simple x86_64 Disassembler (byte-level)
// ===========================================================================

/// Disassemble x86_64 bytes into hex-dump lines.
///
/// Since x86_64 has variable-length instructions, a full disassembler would
/// need to decode each instruction. This implementation provides a simple
/// hex dump with a reasonable attempt at instruction boundary detection
/// based on known opcode patterns.
fn disassemble_x86_64(bytes: &[u8], addr: u64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut offset = 0usize;
    let mut pc = addr;

    while offset < bytes.len() {
        let start = offset;
        let start_pc = pc;

        // Simple length estimation based on opcode byte patterns
        let len = estimate_instruction_length(bytes, offset);
        let end = (offset + len).min(bytes.len());

        let hex_bytes: Vec<String> = bytes[start..end]
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        lines.push(format!("{:#010x}:  {}", start_pc, hex_bytes.join(" ")));

        offset = end;
        pc += (end - start) as u64;
    }

    lines
}

/// Estimate the length of the x86_64 instruction starting at `bytes[offset]`.
///
/// This is a heuristic that covers the most common instruction patterns.
/// For unknown opcodes, defaults to 1 byte.
fn estimate_instruction_length(bytes: &[u8], offset: usize) -> usize {
    if offset >= bytes.len() {
        return 1;
    }

    let mut len = 1usize;
    let mut pos = offset;

    // Skip legacy prefixes (66, 67, F2, F3)
    while pos < bytes.len() && matches!(bytes[pos], 0x66 | 0x67 | 0xF2 | 0xF3) {
        len += 1;
        pos += 1;
    }

    // REX prefix (0x40-0x4F)
    if pos < bytes.len() && bytes[pos] >= 0x40 && bytes[pos] <= 0x4F {
        len += 1;
        pos += 1;
    }

    if pos >= bytes.len() {
        return len;
    }

    let opcode = bytes[pos];
    len += 1;
    pos += 1;

    match opcode {
        // Single-byte instructions
        0x90 | 0xC3 | 0xCC => len,

        // Two-byte instructions: 0F xx
        0x0F => {
            if pos >= bytes.len() { return len + 1; }
            let op2 = bytes[pos];
            len += 1;
            match op2 {
                // SYSCALL (0F 05)
                0x05 => len,
                // SETcc (0F 9x /r)
                0x90..=0x9F => { len += 1; len }
                // Jcc (0F 8x + rel32)
                0x80..=0x8F => { len += 1 + 4; len }
                // IMUL (0F AF /r)
                0xAF => { len += 1; len }
                // MOVZX/MOVSX byte (0F B6/B7/BE)
                0xB6 | 0xB7 | 0xBE | 0xBF => { len += 1; len }
                // CMOVcc (0F 4x /r)
                0x40..=0x4F => { len += 1; len }
                _ => { len += 1; len }
            }
        }

        // PUSH/POP r64 (50-5F)
        0x50..=0x5F => len,

        // MOV r64, imm64 (B8-BF + 8 bytes)
        0xB8..=0xBF => { len += 8; len }

        // ALU reg-reg (01, 09, 21, 29, 31, 39, 85) + ModR/M
        0x01 | 0x03 | 0x09 | 0x0B | 0x21 | 0x23 | 0x29 | 0x2B
        | 0x31 | 0x33 | 0x39 | 0x3B | 0x85 | 0x87 | 0x89 | 0x8B
        | 0x8D | 0x63 => {
            len += 1; // ModR/M
            if pos < bytes.len() {
                let modrm_byte = bytes[pos - 1]; // we already incremented pos
                let _ = modrm_byte; // could add displacement analysis
            }
            len
        }

        // F7 xx (1-byte opcode + ModR/M, may have no immediate)
        0xF7 => { len += 1; len }

        // D3 xx (shift by CL)
        0xD3 => { len += 1; len }

        // C7 /0 + imm32
        0xC7 => { len += 1 + 4; len }

        // 81 /r + imm32
        0x81 => { len += 1 + 4; len }

        // JMP rel32 (E9 + 4 bytes)
        0xE9 => { len += 4; len }

        // CALL rel32 (E8 + 4 bytes)
        0xE8 => { len += 4; len }

        // XCHG rax, r64 (90-97, but 90 is NOP)
        0x91..=0x97 => len,

        // 99 (CQO)
        0x99 => len,

        _ => len,
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
                let aligned = ((*size as usize + 15) / 16) * 16;
                total += aligned;
            }
        }
    }
    // Round up to 16-byte alignment
    (total + 15) & !15
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
        let mut reg_map: std::collections::HashMap<u32, Gpr> = std::collections::HashMap::new();
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
                    crate::ir::IRInstr::Add { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let l = reg_map.get(&lhs.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let r = reg_map.get(&rhs.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rcx);
                        let mut code = Vec::new();
                        if l != d {
                            code.extend(encode_mov_reg_reg(d, l));
                        }
                        code.extend(encode_add_reg_reg(d, r));
                        code
                    }
                    crate::ir::IRInstr::Sub { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let l = reg_map.get(&lhs.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let r = reg_map.get(&rhs.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rcx);
                        let mut code = Vec::new();
                        if l != d {
                            code.extend(encode_mov_reg_reg(d, l));
                        }
                        code.extend(encode_sub_reg_reg(d, r));
                        code
                    }
                    crate::ir::IRInstr::Mul { dst, lhs, rhs } => {
                        let d = reg_map.get(&dst.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let l = reg_map.get(&lhs.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rax);
                        let r = reg_map.get(&rhs.as_register().unwrap_or(0)).copied().unwrap_or(Gpr::Rcx);
                        let mut code = Vec::new();
                        if l != d {
                            code.extend(encode_mov_reg_reg(d, l));
                        }
                        code.extend(encode_imul_reg_reg(d, r));
                        code
                    }
                    crate::ir::IRInstr::Ret { values } => {
                        let mut code = Vec::new();
                        if let Some(val) = values.first() {
                            if let Some(id) = val.as_register() {
                                if let Some(&src) = reg_map.get(&id) {
                                    if src != Gpr::Rax {
                                        code.extend(encode_mov_reg_reg(Gpr::Rax, src));
                                    }
                                }
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
                    _ => {
                        // For unhandled instructions, emit a NOP
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
        disassemble_x86_64(bytes, addr)
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
}
