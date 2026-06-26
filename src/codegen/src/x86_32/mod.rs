//! # x86_32 Backend
//!
//! Implements the `Backend` trait for the x86_32 target (SystemV ABI).
//! This module provides:
//!
//! - `Gpr` — General-purpose register enum (RAX–R15)
//! - `Xmm` — SSE/SIMD register enum (XMM0–XMM15)
//! - REX prefix generation
//! - ModR/M + SIB byte encoding
//! - Instruction encoding for all key x86_32 instructions
//! - `X86_32Backend` — `Backend` implementation that lowers IR to x86_32 machine code
//!
//! ## x86_32 Register Convention (SystemV ABI)
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
    BackendError, PhysicalReg, RegClass, RelocationEntry, TargetInfo, X86_32TargetInfo,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
use std::collections::{HashMap, HashSet};
use std::fmt;

// ===========================================================================
// General-Purpose Registers
// ===========================================================================

/// x86_32 general-purpose registers (RAX–R15).
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

/// x86_32 SSE/SIMD registers (XMM0–XMM15).
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

/// x86_32 condition codes for SETcc, Jcc, and CMOVcc instructions.
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
    // x86_32: No REX prefix needed. Only 8 registers (0-7), no R8-R15.
    // Emit opcode + ModR/M directly for 32-bit operations.
    code.push(opcode);
    code.push(modrm(3, reg.encoding() & 7, rm.encoding() & 7));
}

/// Emit a REX.W prefix (always), then opcode, then ModR/M for reg-reg with
/// specific reg field (opcode extension) and rm register.
fn emit_rexw_opext_reg(code: &mut Vec<u8>, opcode: u8, opext: u8, rm: Gpr) {
    // x86_32: No REX prefix needed.
    code.push(opcode);
    code.push(modrm(3, opext & 7, rm.encoding() & 7));
}

/// Encode MOV r64, r64 (REX.W + 89 /r)
pub fn encode_mov_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x89, src, dst);
    code
}

/// Encode MOV r32, imm32 (B8+rd + 4-byte imm) — 32-bit immediate load.
/// For x86_32, this replaces the 64-bit MOV r64, imm64.
pub fn encode_mov_reg_imm64(dst: Gpr, imm: u64) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
    // No REX prefix for 32-bit. Use B8+rd opcode with 4-byte immediate.
    code.push(0xB8 + (dst.encoding() & 7));
    code.extend_from_slice(&(imm as u32).to_le_bytes());
    code
}

/// Encode MOV r32, imm32 (C7 /0 + 4-byte imm) — 32-bit immediate.
/// No REX prefix needed for x86_32.
pub fn encode_mov_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(6);
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
    let mut code = Vec::with_capacity(7);
    // x86_32: No REX prefix. MOV r32, [r32+offset] = 8B /r
    code.push(0x8B);

    // Handle special cases for ESP/EBP as base
    let base_enc = base.encoding() & 7;
    let dst_enc = dst.encoding() & 7;

    if base_enc == 4 {
        // ESP requires SIB byte
        if offset == 0 {
            code.push(modrm(0, dst_enc, 4));
            code.push(sib(0, 4, 4));
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst_enc, 4));
            code.push(sib(0, 4, 4));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst_enc, 4));
            code.push(sib(0, 4, 4));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if base_enc == 5 {
        // EBP requires disp8=0 for zero offset (mod=1)
        if offset == 0 {
            code.push(modrm(1, dst_enc, 5));
            code.push(0u8);
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst_enc, 5));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst_enc, 5));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else {
        // Normal base register
        if offset == 0 {
            code.push(modrm(0, dst_enc, base_enc));
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst_enc, base_enc));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst_enc, base_enc));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    }
    code
}

/// Encode MOV [r64+offset], r64 (REX.W + 89 /r + displacement)
pub fn encode_mov_mem_reg(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    // x86_32: No REX prefix. MOV [r32+offset], r32 = 89 /r
    code.push(0x89);

    let base_enc = base.encoding() & 7;
    let src_enc = src.encoding() & 7;

    if base_enc == 4 {
        // ESP requires SIB byte
        if offset == 0 {
            code.push(modrm(0, src_enc, 4));
            code.push(sib(0, 4, 4));
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, src_enc, 4));
            code.push(sib(0, 4, 4));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, src_enc, 4));
            code.push(sib(0, 4, 4));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if base_enc == 5 {
        // EBP requires disp8=0 for zero offset (mod=1)
        if offset == 0 {
            code.push(modrm(1, src_enc, 5));
            code.push(0u8);
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, src_enc, 5));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, src_enc, 5));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else {
        // Normal base register
        if offset == 0 {
            code.push(modrm(0, src_enc, base_enc));
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, src_enc, base_enc));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, src_enc, base_enc));
            code.extend_from_slice(&offset.to_le_bytes());
        }
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

/// Encode IMUL r32, r32 (0F AF /r) — 32-bit multiply.
/// No REX prefix needed for x86_32.
pub fn encode_imul_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
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
    let mut code = Vec::with_capacity(6);
    // x86_32: No REX prefix. CMP r32, imm32 = 81 /7 + imm32
    // For imm8 range, use 83 /7 + imm8 (shorter encoding)
    if imm >= -128 && imm <= 127 {
        code.push(0x83);
        code.push(modrm(3, 7, dst.encoding() & 7));
        code.push(imm as u8);
    } else {
        code.push(0x81);
        code.push(modrm(3, 7, dst.encoding() & 7));
        code.extend_from_slice(&imm.to_le_bytes());
    }
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
    // x86_32: SHL/SHR/SAR r32, CL = D3 /4 (no REX)
    vec![0xD3, modrm(3, 4, dst.encoding() & 7)]
}

/// Encode SHR r64, CL (REX.W + D3 /5)
pub fn encode_shr_reg_cl(dst: Gpr) -> Vec<u8> {
    // x86_32: SHL/SHR/SAR r32, CL = D3 /5 (no REX)
    vec![0xD3, modrm(3, 5, dst.encoding() & 7)]
}

/// Encode SAR r64, CL (REX.W + D3 /7)
pub fn encode_sar_reg_cl(dst: Gpr) -> Vec<u8> {
    // x86_32: SHL/SHR/SAR r32, CL = D3 /7 (no REX)
    vec![0xD3, modrm(3, 7, dst.encoding() & 7)]
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
    // x86_32: PUSH r32 = 50+rd (1 byte, no REX)
    vec![0x50 + (src.encoding() & 7)]
}

/// Encode POP r64 (58+rd or REX.B+58+rd for R8–R15)
pub fn encode_pop(dst: Gpr) -> Vec<u8> {
    // x86_32: POP r32 = 58+rd (1 byte, no REX)
    vec![0x58 + (dst.encoding() & 7)]
}

/// Encode SETcc r/m8 (0F 9x /r)
pub fn encode_setcc(cc: Cc, dst: Gpr) -> Vec<u8> {
    // x86_32: SETcc r8 = 0F 90+cc /0 (no REX)
    // Note: SETcc always writes to a byte register. Without REX,
    // the destination is AL/CL/DL/BL/AH/CH/DH/BH (encoding 0-7).
    // We need to ensure the upper bits of the register are zeroed
    // by the caller (typically via MOVZX after SETcc).
    vec![0x0F, 0x90 + cc as u8, modrm(3, 0, dst.encoding() & 7)]
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
    let mut code = Vec::with_capacity(3);
    // x86_32: No REX prefix. CMOVcc r32, r32 = 0F 40+cc /r
    code.push(0x0F);
    code.push(0x40 + cc as u8);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode LEA r64, [r64+offset] (REX.W + 8D /r)
pub fn encode_lea_reg_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    // x86_32: No REX prefix. LEA r32, [r32+offset] = 8D /r
    code.push(0x8D);

    let base_enc = base.encoding() & 7;
    let dst_enc = dst.encoding() & 7;

    if base_enc == 4 {
        // ESP requires SIB byte
        if offset == 0 {
            code.push(modrm(0, dst_enc, 4));
            code.push(sib(0, 4, 4));
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst_enc, 4));
            code.push(sib(0, 4, 4));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst_enc, 4));
            code.push(sib(0, 4, 4));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else if base_enc == 5 {
        // EBP requires disp8=0 for zero offset (mod=1)
        if offset == 0 {
            code.push(modrm(1, dst_enc, 5));
            code.push(0u8);
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst_enc, 5));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst_enc, 5));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    } else {
        // Normal base register
        if offset == 0 {
            code.push(modrm(0, dst_enc, base_enc));
        } else if offset >= -128 && offset <= 127 {
            code.push(modrm(1, dst_enc, base_enc));
            code.push(offset as u8);
        } else {
            code.push(modrm(2, dst_enc, base_enc));
            code.extend_from_slice(&offset.to_le_bytes());
        }
    }
    code
}

/// Encode MOVZX r64, r8 (REX.W + 0F B6 /r) — zero-extend byte to 64 bits
pub fn encode_movzx_reg8(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    // x86_32: No REX prefix. MOVZX r32, r8 = 0F B6 /r
    code.push(0x0F);
    code.push(0xB6);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVZX r64, r16 (REX.W + 0F B7 /r) — zero-extend word to 64 bits
pub fn encode_movzx_reg16(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    // x86_32: MOVZX r32, r16 = 66 0F B7 /r (66 prefix for 16-bit source)
    code.push(0x0F);
    code.push(0xB7);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVSX r64, r8 (REX.W + 0F BE /r) — sign-extend byte to 64 bits
pub fn encode_movsx_reg8(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    // x86_32: No REX prefix. MOVSX r32, r8 = 0F BE /r
    code.push(0x0F);
    code.push(0xBE);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVSX r64, r16 (REX.W + 0F BF /r) — sign-extend word to 64 bits
pub fn encode_movsx_reg16(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    // x86_32: MOVSX r32, r16 = 0F BF /r
    code.push(0x0F);
    code.push(0xBF);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVSX r64, r32 (REX.W + 63 /r) — sign-extend dword to 64 bits
pub fn encode_movsxd(dst: Gpr, src: Gpr) -> Vec<u8> {
    // x86_32: No MOVSXD needed (32-bit registers are already 32-bit).
    // Just do a regular MOV r32, r32.
    encode_mov_reg_reg(dst, src)
}

/// Encode XCHG rax, r64 (REX.W + 90+rd)
pub fn encode_xchg_rax_reg(src: Gpr) -> Vec<u8> {
    // x86_32: XCHG EAX, r32 = 90+rd (1 byte, no REX)
    if src.encoding() & 7 == 0 {
        // XCHG EAX, EAX = NOP (0x90)
        vec![0x90]
    } else {
        vec![0x90 + (src.encoding() & 7)]
    }
}

/// Encode SYSCALL (0F 05)
pub fn encode_syscall() -> Vec<u8> {
    vec![0xCD, 0x80]
}

/// Encode INT3 (CC)
pub fn encode_int3() -> Vec<u8> {
    vec![0xCC]
}

/// Encode NEG r64 (REX.W + F7 /3)
pub fn encode_neg_reg(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(2);
    // x86_32: NEG r32 = F7 /3
    code.push(0xF7);
    code.push(modrm(3, 3, dst.encoding() & 7));
    code
}

/// Encode NOT r64 (REX.W + F7 /2)
pub fn encode_not_reg(dst: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(2);
    // x86_32: NOT r32 = F7 /2
    code.push(0xF7);
    code.push(modrm(3, 2, dst.encoding() & 7));
    code
}

/// Encode MUL r64 (REX.W + F7 /4) — unsigned multiply, result in RDX:RAX
pub fn encode_mul_reg(src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(2);
    // x86_32: MUL r32 = F7 /4 (unsigned multiply, EDX:EAX = EAX * r32)
    code.push(0xF7);
    code.push(modrm(3, 4, src.encoding() & 7));
    code
}

/// Encode DIV r64 (REX.W + F7 /6) — unsigned divide
pub fn encode_div_reg(src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_opext_reg(&mut code, 0xF7, 6, src);
    code
}

/// Encode CDQ (99) — sign-extend EAX into EDX:EAX (32-bit version of CQO)
pub fn encode_cqo() -> Vec<u8> {
    vec![0x99]
}

/// Encode SUB r64, imm32 (REX.W + 81 /5 + imm)
pub fn encode_sub_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
        code.push(0x81);
    code.push(modrm(3, 5, dst.encoding() & 7));
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode ADD r64, imm32 (REX.W + 81 /0 + imm)
pub fn encode_add_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
        code.push(0x81);
    code.push(modrm(3, 0, dst.encoding() & 7));
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode AND r64, imm32 (REX.W + 81 /4 + imm32)
pub fn encode_and_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
        code.push(0x81);
    code.push(modrm(3, 4, dst.encoding() & 7)); // /4 is the AND extension
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode OR r64, imm32 (REX.W + 81 /1 + imm32)
pub fn encode_or_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
        code.push(0x81);
    code.push(modrm(3, 1, dst.encoding() & 7)); // /1 is the OR extension
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode XOR r64, imm32 (REX.W + 81 /6 + imm32)
pub fn encode_xor_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
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
    // x86_32: No REX prefix (no R8-R15, no 64-bit operands)
    code.push(0x0F);
    code.push(0xB6);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOV byte [r64 + offset], r8 (low byte of GPR) (88 /r with memory operand, no REX.W)
pub fn encode_mov_mem8_reg8(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    // x86_32: No REX prefix
    code.push(0x88);
    encode_mem_operand(&mut code, src.encoding() & 7, base, offset);
    code
}

/// Encode MOV dword [r64 + offset], r32 (89 /r with no REX.W, 32-bit store that zero-extends)
pub fn encode_mov_mem32_reg32(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    // x86_32: No REX prefix
    code.push(0x89);
    encode_mem_operand(&mut code, src.encoding() & 7, base, offset);
    code
}

/// Encode MOV r32, dword [r64 + offset] (8B /r with no REX.W, 32-bit load that zero-extends to 64)
pub fn encode_mov_reg32_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    // x86_32: No REX prefix
    code.push(0x8B);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOVSX r64, byte [r64 + offset] (REX.W + 0F BE /r with memory operand)
pub fn encode_movsx_reg8_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
                code.push(0x0F);
    code.push(0xBE);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOVSX r64, word [r64 + offset] (REX.W + 0F BF /r with memory operand)
pub fn encode_movsx_reg16_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
                code.push(0x0F);
    code.push(0xBF);
    encode_mem_operand(&mut code, dst.encoding() & 7, base, offset);
    code
}

/// Encode MOV word [r64 + offset], r16 (66 89 /r with memory operand)
pub fn encode_mov_mem16_reg16(base: Gpr, offset: i32, src: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
            // 16-bit operand size prefix
    code.push(0x66);
        code.push(0x89);
    encode_mem_operand(&mut code, src.encoding() & 7, base, offset);
    code
}

/// Encode MOVZX r64, word [r64 + offset] (REX.W + 0F B7 /r with memory operand)
pub fn encode_movzx_reg16_mem(dst: Gpr, base: Gpr, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
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
                code.push(0x66);
    code.push(0x0F);
    code.push(0x6E);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVD r32, xmm (66 0F 7E /r) — move low dword from XMM to GPR.
pub fn encode_movd_gpr_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0x66);
    code.push(0x0F);
    code.push(0x7E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MOVQ xmm, r64 (66 REX.W 0F 6E /r) — move 64-bit GPR into XMM.
pub fn encode_movq_xmm_gpr(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
                code.push(0x66);
    code.push(0x0F);
    code.push(0x6E);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode MOVQ r64, xmm (66 REX.W 0F 7E /r) — move 64-bit from XMM to GPR.
pub fn encode_movq_gpr_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
                code.push(0x66);
    code.push(0x0F);
    code.push(0x7E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSI2SD xmm, r32 (F2 0F 2A /r) — convert signed 32-bit int to f64.
pub fn encode_cvtsi2sd_xmm_r32(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF2);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSI2SD xmm, r64 (F2 REX.W 0F 2A /r) — convert signed 64-bit int to f64.
pub fn encode_cvtsi2sd_xmm_r64(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
                code.push(0xF2);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSI2SS xmm, r32 (F3 0F 2A /r) — convert signed 32-bit int to f32.
pub fn encode_cvtsi2ss_xmm_r32(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF3);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSI2SS xmm, r64 (F3 REX.W 0F 2A /r) — convert signed 64-bit int to f32.
pub fn encode_cvtsi2ss_xmm_r64(dst: Xmm, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
                code.push(0xF3);
    code.push(0x0F);
    code.push(0x2A);
    code.push(modrm(3, dst.encoding() & 7, src.encoding() & 7));
    code
}

/// Encode CVTSD2SI r32, xmm (F2 0F 2D /r) — convert f64 to signed 32-bit int.
pub fn encode_cvtsd2si_r32_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF2);
    code.push(0x0F);
    code.push(0x2D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSD2SI r64, xmm (F2 REX.W 0F 2D /r) — convert f64 to signed 64-bit int.
pub fn encode_cvtsd2si_r64_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
                code.push(0xF2);
    code.push(0x0F);
    code.push(0x2D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSS2SI r32, xmm (F3 0F 2D /r) — convert f32 to signed 32-bit int.
pub fn encode_cvtss2si_r32_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF3);
    code.push(0x0F);
    code.push(0x2D);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSS2SI r64, xmm (F3 REX.W 0F 2D /r) — convert f32 to signed 64-bit int.
pub fn encode_cvtss2si_r64_xmm(dst: Gpr, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(5);
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
                code.push(0xF3);
    code.push(0x0F);
    code.push(0x2C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSS2SD xmm, xmm (F3 0F 5A /r) — convert f32 to f64 (widen).
pub fn encode_cvtss2sd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF3);
    code.push(0x0F);
    code.push(0x5A);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode CVTSD2SS xmm, xmm (F2 0F 5A /r) — convert f64 to f32 (narrow).
pub fn encode_cvtsd2ss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF2);
    code.push(0x0F);
    code.push(0x5A);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode ADDSD xmm, xmm (F2 0F 58 /r) — add scalar double-precision floats.
pub fn encode_addsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF2);
    code.push(0x0F);
    code.push(0x58);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode ADDSS xmm, xmm (F3 0F 58 /r) — add scalar single-precision floats.
pub fn encode_addss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
                code.push(0xF3);
    code.push(0x0F);
    code.push(0x58);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

// ===========================================================================
// x86_32 Mnemonic Disassembler
// ===========================================================================

/// Decode x86_32 bytes into mnemonic strings with (offset, mnemonic) pairs.
///
/// Handles the top 20+ most common x86_32 instructions including mov, add, sub,
/// push, pop, call, ret, jmp, cmp, test, lea, xor, and, or, shl, shr, nop,
/// mul, div, imul.
fn disassemble_x86_32_mnemonic(bytes: &[u8], addr: u64) -> Vec<String> {
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

/// Build a minimal ELF64 binary for x86_32 from raw code bytes.
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
fn build_minimal_x86_32_elf(code: &[u8], base_addr: u64, bss_size: u64) -> Vec<u8> {
    // ELF32 for i386 — proper ELF32 format with 52-byte header and 32-byte phdrs.
    // Use 64K alignment for virtual addresses to ensure compatibility with
    // QEMU 10.x on hosts with 16K or 64K page sizes (same fix as other backends).
    const FILE_PAGE_SIZE: u32 = 0x1000; // 4 KB — file offset alignment
    const VADDR_ALIGN: u32 = 0x10000;   // 64 KB — virtual address alignment

    let base_addr: u32 = base_addr as u32;
    let bss_size: u32 = bss_size as u32;
    let elf_header_size: u32 = 52;  // ELF32 header is 52 bytes (not 64!)
    let phdr_size: u32 = 32;        // ELF32 Phdr is 32 bytes (not 56!)
    let num_phdrs: u32 = if bss_size > 0 { 2 } else { 1 };
    let phdr_end = elf_header_size + phdr_size * num_phdrs;
    // Page-align the text segment start for mmap compatibility.
    let text_offset: u32 = ((phdr_end + FILE_PAGE_SIZE - 1) / FILE_PAGE_SIZE) * FILE_PAGE_SIZE;
    let text_size: u32 = code.len() as u32;
    // Align text vaddr to 64K for host page size compatibility.
    let text_vaddr: u32 = ((base_addr + text_offset + VADDR_ALIGN - 1) / VADDR_ALIGN) * VADDR_ALIGN;
    let entry_point: u32 = text_vaddr;

    let mut elf = Vec::with_capacity(text_offset as usize + code.len());

    // --- e_ident (16 bytes) ---
    elf.extend_from_slice(&[0x7f, b'E', b'L', b'F']);
    elf.push(1); // ELFCLASS32 (not 2!)
    elf.push(1); // ELFDATA2LSB
    elf.push(1); // EV_CURRENT
    elf.push(3); // ELFOSABI_LINUX
    elf.push(0);
    elf.extend_from_slice(&[0u8; 7]);

    // --- ELF32 header fields (36 bytes, all u32/u16 — no u64!) ---
    elf.extend_from_slice(&2u16.to_le_bytes());       // e_type = ET_EXEC
    elf.extend_from_slice(&3u16.to_le_bytes());        // e_machine = EM_386 (not 62!)
    elf.extend_from_slice(&1u32.to_le_bytes());        // e_version
    elf.extend_from_slice(&entry_point.to_le_bytes()); // e_entry (u32)
    elf.extend_from_slice(&elf_header_size.to_le_bytes()); // e_phoff (u32)
    elf.extend_from_slice(&0u32.to_le_bytes());        // e_shoff (u32, no section headers)
    elf.extend_from_slice(&0u32.to_le_bytes());        // e_flags
    elf.extend_from_slice(&52u16.to_le_bytes());       // e_ehsize (52 for ELF32)
    elf.extend_from_slice(&32u16.to_le_bytes());       // e_phentsize (32 for ELF32 Phdr)
    elf.extend_from_slice(&(num_phdrs as u16).to_le_bytes()); // e_phnum
    elf.extend_from_slice(&40u16.to_le_bytes());       // e_shentsize (40 for ELF32 Shdr)
    elf.extend_from_slice(&0u16.to_le_bytes());        // e_shnum
    elf.extend_from_slice(&0u16.to_le_bytes());        // e_shstrndx

    // --- Program Header 1: LOAD .text (PF_R | PF_X) ---
    // ELF32 Phdr field order: p_type, p_offset, p_vaddr, p_paddr,
    //                         p_filesz, p_memsz, p_flags, p_align
    elf.extend_from_slice(&1u32.to_le_bytes());        // p_type = PT_LOAD
    elf.extend_from_slice(&text_offset.to_le_bytes()); // p_offset (u32)
    elf.extend_from_slice(&text_vaddr.to_le_bytes());  // p_vaddr (u32)
    elf.extend_from_slice(&text_vaddr.to_le_bytes());  // p_paddr (u32)
    elf.extend_from_slice(&text_size.to_le_bytes());   // p_filesz (u32)
    elf.extend_from_slice(&text_size.to_le_bytes());   // p_memsz (u32)
    elf.extend_from_slice(&5u32.to_le_bytes());        // p_flags = PF_R | PF_X
    elf.extend_from_slice(&FILE_PAGE_SIZE.to_le_bytes()); // p_align (u32)

    // --- Program Header 2: LOAD .bss (PF_R | PF_W) ---
    // Only emitted when there is BSS data. BSS starts at the next 64K boundary
    // after the text segment to avoid sharing a host page with text.
    if bss_size > 0 {
        let bss_vaddr: u32 = ((text_vaddr + text_size + VADDR_ALIGN - 1) / VADDR_ALIGN) * VADDR_ALIGN;
        elf.extend_from_slice(&1u32.to_le_bytes());        // p_type = PT_LOAD
        elf.extend_from_slice(&0u32.to_le_bytes());        // p_offset (no file content)
        elf.extend_from_slice(&bss_vaddr.to_le_bytes());   // p_vaddr (u32)
        elf.extend_from_slice(&bss_vaddr.to_le_bytes());   // p_paddr (u32)
        elf.extend_from_slice(&0u32.to_le_bytes());        // p_filesz (BSS is zero-filled)
        elf.extend_from_slice(&bss_size.to_le_bytes());    // p_memsz (u32)
        elf.extend_from_slice(&6u32.to_le_bytes());        // p_flags = PF_R | PF_W
        elf.extend_from_slice(&FILE_PAGE_SIZE.to_le_bytes()); // p_align (u32)
    }

    // --- Padding + Code section ---
    while (elf.len() as u32) < text_offset {
        elf.push(0);
    }
    elf.extend_from_slice(code);

    elf
}

// ===========================================================================
// Runtime Syscall Stubs
// ===========================================================================

/// Build runtime syscall stubs for x86_32 Linux.
///
/// These are tiny functions that use the `syscall` instruction to implement
/// POSIX operations without requiring libc. Each stub:
/// 1. Loads the syscall number into RAX
/// 2. Moves the 4th argument from RCX to R10 (for mmap, which has ≥4 args)
/// 3. Executes `syscall`
/// 4. Returns to the caller
///
/// # x86_32 Linux Syscall Numbers
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

    // ── i386 Linux syscall stubs ──
    // Convention: EAX=syscall#, EBX=arg1, ECX=arg2, EDX=arg3, ESI=arg4, EDI=arg5, EBP=arg6
    // VUMA args come in: EDI=arg0, ESI=arg1, EDX=arg2, ECX=arg3
    // Remap: EDI→EBX, ESI→ECX, EDX stays, ECX→ESI
    // Use PUSH/POP to avoid clobbering during remap.

    // write(fd, buf, count) → ssize_t  [i386 syscall 4]
    // args: EDI=fd, ESI=buf, EDX=count → EBX=fd, ECX=buf, EDX=count
    {
        let mut code = Vec::new();
        // push edx (save count before clobbering)
        code.extend(encode_push(Gpr::Rdx));       // push count
        // push esi (save buf)
        code.extend(encode_push(Gpr::Rsi));       // push buf
        // mov ebx, edi (fd)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        // pop ecx (buf)
        code.extend(encode_pop(Gpr::Rcx));
        // pop edx (count)
        code.extend(encode_pop(Gpr::Rdx));
        // mov eax, 4 (sys_write)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 4));
        code.extend(encode_syscall());             // int 0x80
        code.extend(encode_ret());
        stubs.push(("write".to_string(), code));
    }

    // read(fd, buf, count) → ssize_t  [i386 syscall 3]
    // args: EDI=fd, ESI=buf, EDX=count → EBX=fd, ECX=buf, EDX=count
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rdx));
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_pop(Gpr::Rdx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("read".to_string(), code));
    }

    // open(pathname, flags, mode) → int  [i386 syscall 5]
    // args: EDI=pathname, ESI=flags, EDX=mode → EBX=pathname, ECX=flags, EDX=mode
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rdx));
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_pop(Gpr::Rdx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 5));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("open".to_string(), code));
    }

    // close(fd) → int  [i386 syscall 6]
    // args: EDI=fd → EBX=fd
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 6));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("close".to_string(), code));
    }

    // exit(code) → void  [i386 syscall 1]
    // args: EDI=code → EBX=code
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
        code.extend(encode_syscall());
        code.extend(encode_int3()); // safety guard (exit never returns)
        stubs.push(("exit".to_string(), code));
    }

    // unlink(pathname) → int  [i386 syscall 10]
    // args: EDI=pathname → EBX=pathname
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 10));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("unlink".to_string(), code));
    }

    // mmap2(addr, length, prot, flags, fd, offset) → void*  [i386 syscall 192]
    // i386 uses mmap2 (offset in 4KB pages) instead of mmap.
    // VUMA args: EDI=addr, ESI=length, EDX=prot, ECX=flags
    //   (args 5-6 (fd, offset) are on the stack for i386)
    // For __vuma_alloc, we call mmap2(NULL, size, PROT_RW, MAP_PRIVATE|MAP_ANON, -1, 0)
    {
        let mut code = Vec::new();
        // The VUMA calling convention for x86_32 puts args 0-3 in EDI, ESI, EDX, ECX
        // and args 4-5 on the stack at [ESP+4] and [ESP+8] (after return address).
        // But __vuma_alloc only has 1 arg (size), so we construct the mmap2 args here.
        // For the general mmap case, assume args are: EDI=addr, ESI=length, EDX=prot
        // For __vuma_alloc, EDI=size, we set up all 6 args.
        // This stub handles __vuma_alloc specifically:
        // mmap2(NULL, size, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));    // addr = 0 (NULL) — will be overwritten below
        code.extend(encode_xor_reg_reg(Gpr::Rbx, Gpr::Rbx));    // EBX = 0 (addr = NULL)
        code.extend(encode_mov_reg_reg(Gpr::Rcx, Gpr::Rdi));    // ECX = length = size
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 3));         // EDX = PROT_READ|PROT_WRITE
        code.extend(encode_mov_reg_imm32(Gpr::Rsi, 0x22));      // ESI = MAP_PRIVATE|MAP_ANONYMOUS
        code.extend(encode_mov_reg_imm32(Gpr::Rdi, -1i32));     // EDI = fd = -1
        code.extend(encode_xor_reg_reg(Gpr::Rbp, Gpr::Rbp));    // EBP = offset = 0
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 192));       // EAX = sys_mmap2
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("__vuma_alloc".to_string(), code));
    }

    // __vuma_free(addr, size) → void  [i386 syscall 91 = munmap]
    // args: EDI=addr, ESI=size → EBX=addr, ECX=size
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rsi));                     // push size
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));    // EBX = addr
        code.extend(encode_pop(Gpr::Rcx));                      // ECX = size
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 91));        // EAX = sys_munmap
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("__vuma_free".to_string(), code));
    }

    // sigaction(signum, act, oldact) → long  [i386 syscall 174 = rt_sigaction]
    // Kernel: rt_sigaction(int signum, const struct sigaction *act,
    //                      struct sigaction *oldact, size_t sigsetsize)
    // args: EDI=signum, ESI=act, EDX=oldact → EBX=signum, ECX=act, EDX=oldact, ESI=8
    {
        let mut code = Vec::new();
        // Save oldact (EDX) before clobbering
        code.extend(encode_push(Gpr::Rdx));     // push oldact
        code.extend(encode_push(Gpr::Rsi));     // push act
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = signum
        code.extend(encode_pop(Gpr::Rcx));      // ECX = act
        code.extend(encode_pop(Gpr::Rdx));      // EDX = oldact
        code.extend(encode_mov_reg_imm32(Gpr::Rsi, 8)); // ESI = sigsetsize = 8
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 174)); // sys_rt_sigaction
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("sigaction".to_string(), code));
    }

    // alarm(seconds) → unsigned int  [i386 syscall 27]
    // args: EDI=seconds → EBX=seconds
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 27));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("alarm".to_string(), code));
    }

    // pipe(int pipefd[2]) → int  [i386 syscall 42]
    // args: EDI=pipefd → EBX=pipefd
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 42));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("pipe".to_string(), code));
    }

    // dup2(int oldfd, int newfd) → int  [i386 syscall 63]
    // args: EDI=oldfd, ESI=newfd → EBX=oldfd, ECX=newfd
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 63));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("dup2".to_string(), code));
    }

    // getpid() → pid_t  [i386 syscall 20]
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 20));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("getpid".to_string(), code));
    }

    // fork() → pid_t  [i386 syscall 2]
    {
        let mut code = Vec::new();
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 2));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("fork".to_string(), code));
    }

    // execve(pathname, argv, envp) → int  [i386 syscall 11]
    // args: EDI=pathname, ESI=argv, EDX=envp → EBX=pathname, ECX=argv, EDX=envp
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rdx));
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_pop(Gpr::Rdx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 11));
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("execve".to_string(), code));
    }

    // wait4(pid, wstatus, options, rusage) → pid_t  [i386 syscall 114]
    // args: EDI=pid, ESI=wstatus, EDX=options, ECX=rusage
    //   → EBX=pid, ECX=wstatus, EDX=options, ESI=rusage
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rcx));     // push rusage
        code.extend(encode_push(Gpr::Rdx));     // push options
        code.extend(encode_push(Gpr::Rsi));     // push wstatus
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = pid
        code.extend(encode_pop(Gpr::Rcx));      // ECX = wstatus
        code.extend(encode_pop(Gpr::Rdx));      // EDX = options
        code.extend(encode_pop(Gpr::Rsi));      // ESI = rusage
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 114)); // sys_wait4
        code.extend(encode_syscall());
        code.extend(encode_ret());
        stubs.push(("wait4".to_string(), code));
    }

    // ── print_hex: Print EAX as 8 hex digits to stdout ──
    // Argument: EDI = value to print (x86_32 calling convention)
    // Uses sys_write (4) with fd=1 (stdout).
    // Converts each nibble to hex char, writes to stack buffer, then sys_write.
    {
        let mut code = Vec::new();
        // push ebp; mov ebp, esp
        code.extend(encode_push(Gpr::Rbp));
        code.extend(encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp));
        // sub esp, 16 (space for 8 hex digits + padding)
        code.extend_from_slice(&[0x83, 0xEC, 0x10]); // sub esp, 16
        // mov eax, edi — load argument from EDI into EAX
        code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rdi));
        // mov ecx, esp (buffer pointer)
        code.extend(encode_mov_reg_reg(Gpr::Rcx, Gpr::Rsp));
        // mov edx, 8 (digit count)
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 8));
        // Loop: convert each nibble
        // .loop:
        let loop_offset = code.len();
        // mov ebx, eax
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rax));
        // and ebx, 0x0F (isolate lowest nibble)
        code.extend_from_slice(&[0x83, 0xE3, 0x0F]); // and ebx, 0x0F
        // cmp ebx, 10
        code.extend_from_slice(&[0x83, 0xFB, 0x0A]); // cmp ebx, 10
        // jb .digit
        code.extend_from_slice(&[0x72, 0x07]); // jb +7
        // add ebx, 'A' - 10
        code.extend(encode_mov_reg_imm32(Gpr::Rsi, 55)); // 'A' - 10 = 55
        code.extend(encode_add_reg_reg(Gpr::Rbx, Gpr::Rsi));
        // jmp .store
        code.extend_from_slice(&[0xEB, 0x05]); // jmp +5
        // .digit: add ebx, '0'
        code.extend_from_slice(&[0x83, 0xC3, 0x30]); // add ebx, 0x30
        // .store: mov [ecx], bl
        code.extend_from_slice(&[0x88, 0x19]); // mov [ecx], bl
        // shr eax, 4
        code.extend_from_slice(&[0xC1, 0xE8, 0x04]); // shr eax, 4
        // inc ecx
        code.extend_from_slice(&[0x41]); // inc ecx
        // dec edx
        code.extend_from_slice(&[0x4A]); // dec edx
        // jnz .loop (back to loop_offset)
        let loop_end = code.len();
        let back_offset = loop_offset as i32 - loop_end as i32 - 2;
        code.extend_from_slice(&[0x75, back_offset as u8]); // jnz

        // Now write 8 bytes from stack to stdout
        // mov eax, 4 (sys_write)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 4));
        // mov ebx, 1 (fd = stdout)
        code.extend(encode_mov_reg_imm32(Gpr::Rbx, 1));
        // mov ecx, esp (buffer)
        code.extend(encode_mov_reg_reg(Gpr::Rcx, Gpr::Rsp));
        // mov edx, 8 (count)
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 8));
        // int 0x80
        code.extend(encode_syscall());

        // mov esp, ebp; pop ebp; ret
        code.extend(encode_mov_reg_reg(Gpr::Rsp, Gpr::Rbp));
        code.extend(encode_pop(Gpr::Rbp));
        code.extend(encode_ret());
        stubs.push(("print_hex".to_string(), code));
    }

    // ── print_int: Print EAX as decimal integer to stdout ──
    // Argument: EDI = value to print (x86_32 calling convention)
    // Converts digit-by-digit into a stack buffer, then sys_write.
    {
        let mut code = Vec::new();
        // push ebp; mov ebp, esp
        code.extend(encode_push(Gpr::Rbp));
        code.extend(encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp));
        // sub esp, 32 (space for digits)
        code.extend_from_slice(&[0x83, 0xEC, 0x20]); // sub esp, 32
        // mov eax, edi — load argument from EDI into EAX
        code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rdi));
        // lea ecx, [esp+31] (point to end of buffer, write backwards)
        code.extend(encode_lea_reg_mem(Gpr::Rcx, Gpr::Rsp, 31));
        // mov byte [ecx], 10 (newline)
        code.extend_from_slice(&[0xC6, 0x01, 0x0A]); // mov byte [ecx], 10
        // dec ecx
        code.extend_from_slice(&[0x49]); // dec ecx

        // Check if EAX is 0
        // test eax, eax
        code.extend_from_slice(&[0x85, 0xC0]); // test eax, eax
        // jnz .loop
        code.extend_from_slice(&[0x75, 0x0A]); // jnz +10
        // Handle zero: mov byte [ecx], '0'; dec ecx; jmp .done
        code.extend_from_slice(&[0xC6, 0x01, 0x30]); // mov byte [ecx], '0'
        code.extend_from_slice(&[0x49]); // dec ecx
        code.extend_from_slice(&[0xEB, 0x10]); // jmp +16 (.done)

        // .loop:
        let loop_offset = code.len();
        // xor edx, edx (clear for division)
        code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx));
        // mov ebx, 10
        code.extend(encode_mov_reg_imm32(Gpr::Rbx, 10));
        // div ebx (unsigned: EAX = EAX/10, EDX = EAX%10)
        code.extend(encode_div_reg(Gpr::Rbx));
        // add dl, '0'
        code.extend_from_slice(&[0x80, 0xC2, 0x30]); // add dl, 0x30
        // mov [ecx], dl
        code.extend_from_slice(&[0x88, 0x11]); // mov [ecx], dl
        // dec ecx
        code.extend_from_slice(&[0x49]); // dec ecx
        // test eax, eax
        code.extend_from_slice(&[0x85, 0xC0]); // test eax, eax
        // jnz .loop
        let loop_end = code.len();
        let back_offset = loop_offset as i32 - loop_end as i32 - 2;
        code.extend_from_slice(&[0x75, back_offset as u8]); // jnz

        // .done:
        // inc ecx (point to first digit)
        code.extend_from_slice(&[0x41]); // inc ecx

        // Write to stdout
        // mov eax, 4 (sys_write)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 4));
        // mov ebx, 1 (stdout)
        code.extend(encode_mov_reg_imm32(Gpr::Rbx, 1));
        // ecx already points to the string
        // mov edx, [ebp-4] — actually we need to compute length
        // length = (esp+32) - ecx = ebp - 4 - ecx... let's compute differently
        // mov edx, ebp; sub edx, ecx; sub edx, 4
        code.extend(encode_mov_reg_reg(Gpr::Rdx, Gpr::Rbp));
        code.extend(encode_sub_reg_reg(Gpr::Rdx, Gpr::Rcx));
        code.extend_from_slice(&[0x83, 0xEA, 0x04]); // sub edx, 4
        // int 0x80
        code.extend(encode_syscall());

        // mov esp, ebp; pop ebp; ret
        code.extend(encode_mov_reg_reg(Gpr::Rsp, Gpr::Rbp));
        code.extend(encode_pop(Gpr::Rbp));
        code.extend(encode_ret());
        stubs.push(("print_int".to_string(), code));
    }

    stubs
}

// ===========================================================================
// X86_32Backend
// ===========================================================================

/// x86_32 code generation backend (SystemV ABI).
pub struct X86_32Backend {
    target_info: X86_32TargetInfo,
}

impl X86_32Backend {
    /// Create a new x86_32 backend.
    pub fn new() -> Self {
        Self {
            target_info: X86_32TargetInfo,
        }
    }
}

impl Default for X86_32Backend {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the stack frame size for an IR function on x86_32.
///
/// Sums `Alloc` instruction sizes, adds 8 bytes for the RBP save,
/// and rounds up to 16-byte alignment.
fn x86_32_compute_frame_size(func: &IRFunction) -> usize {
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

// ── x86_32 ELF Relocation Types ─────────────────────────────────────────

/// R_X86_64_64 — S + A, 64-bit absolute relocation.
const R_X86_64_64: &str = "R_X86_64_64";
/// R_X86_64_PLT32 — L + A - P, 32-bit PC-relative PLT relocation for calls/jumps.
const R_X86_64_PLT32: &str = "R_X86_64_PLT32";

// ── ISel helpers ─────────────────────────────────────────────────────────

/// Resolve an IRValue to a physical GPR.
/// For registers, looks up in the reg_map. For immediates, loads the value
/// into `scratch` and returns `scratch`. For addresses, loads into `scratch`.
fn resolve_gpr(val: &IRValue, reg_map: &HashMap<u32, Gpr>, scratch: Gpr) -> (Gpr, Vec<u8>) {
    match val {
        IRValue::Register(id) => (reg_map.get(id).copied().unwrap_or(Gpr::Rax), Vec::new()),
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

/// Map an IR CmpKind to an x86_32 condition code.
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

/// Map an IR BinOpKind comparison to an x86_32 condition code.
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

impl Backend for X86_32Backend {
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
        // On Linux x86_32, the process entry stack layout is:
        //   [RSP]     = argc (8 bytes)
        //   [RSP+8]   = argv[0] pointer
        //   [RSP+16]  = argv[1] pointer
        //   ...
        //   NULL
        //   envp[0], envp[1], ..., NULL
        //   auxv...

        // _start stub: call(5) + mov(2) + mov(6) + int(2) = 15 bytes
        // Note: encode_mov_reg_imm32 uses C7 /0 form (6 bytes, not 5)
        let start_stub_size: usize = 15;

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

        // Build _start stub — i386 Linux convention:
        //   call main       ; EAX = return value
        //   mov ebx, eax    ; EBX = exit code (arg1 for sys_exit)
        //   mov eax, 1      ; EAX = sys_exit (1 on i386, NOT 60!)
        //   int 0x80        ; syscall
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // call main (E8 + rel32 placeholder)
        start_stub.extend(encode_call_rel32(0));

        // mov ebx, eax — move main's return value to EBX (exit code arg)
        start_stub.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rax));

        // mov eax, 1 (sys_exit = 1 on i386)
        start_stub.extend(encode_mov_reg_imm32(Gpr::Rax, 1));

        // int 0x80
        start_stub.extend(encode_syscall());

        // Patch the call main rel32 offset in _start stub
        // The call is at offset 0 (E8 + 4 bytes = 5 bytes total)
        // rel32 is at offset 1 (after the E8 opcode byte)
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            let rel32_patch_offset = 1usize; // offset within start_stub
            // rel32 = target - (call_site + 5)
            // call_site = offset of the E8 byte = 0
            let rel32 = (main_offset as i64) - (0i64 + 5i64);
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
        // The BSS segment starts at the next 64K boundary after the text
        // segment.  We mirror the calculation from build_minimal_x86_32_elf
        // here, using ELF32 sizes (52-byte header, 32-byte phdrs) and 64K
        // virtual address alignment for QEMU 10.x host page size compatibility.
        const ELF_HEADER_SIZE: u64 = 52;  // ELF32 header
        const PHDR_SIZE: u64 = 32;        // ELF32 Phdr
        const FILE_PAGE_SIZE: u64 = 0x1000;
        const VADDR_ALIGN: u64 = 0x10000;
        const BASE_ADDR: u64 = 0x400000;
        let num_phdrs: u64 = if bss_size > 0 { 2 } else { 1 };
        let phdr_end = ELF_HEADER_SIZE + PHDR_SIZE * num_phdrs;
        let text_offset = ((phdr_end + FILE_PAGE_SIZE - 1) / FILE_PAGE_SIZE) * FILE_PAGE_SIZE;
        let text_size = all_code.len() as u64;
        let text_vaddr: u64 = ((BASE_ADDR + text_offset + VADDR_ALIGN - 1) / VADDR_ALIGN) * VADDR_ALIGN;
        let bss_vaddr: u64 = if bss_size > 0 {
            ((text_vaddr + text_size + VADDR_ALIGN - 1) / VADDR_ALIGN) * VADDR_ALIGN
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
                    // R_X86_64_PLT32 for x86_32 CALL/JMP rel32:
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
                        let func_addr = text_vaddr + func_offsets[&reloc.symbol] as u64;
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

        Ok(build_minimal_x86_32_elf(&all_code, BASE_ADDR, bss_size))
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
        disassemble_x86_32_mnemonic(bytes, addr)
    }

    fn name(&self) -> &'static str {
        "x86_32"
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
        assert_eq!(encode_syscall(), vec![0xCD, 0x80]);
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
        let backend = X86_32Backend::new();
        let stub = backend.return_stub();
        // xor eax, eax; ret
        assert_eq!(stub, vec![0x31, 0xC0, 0xC3]);
    }

    // ── Trampoline Test ────────────────────────────────────────────────

    #[test]
    fn test_trampoline() {
        let backend = X86_32Backend::new();
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
        let elf = build_minimal_x86_32_elf(&code, 0x400000, 0);

        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // ELFCLASS32 (1, not 2!)
        assert_eq!(elf[4], 1);
        // ELFDATA2LSB
        assert_eq!(elf[5], 1);
        // e_type = ET_EXEC (2)
        assert_eq!(u16::from_le_bytes([elf[16], elf[17]]), 2);
        // e_machine = EM_386 (3, not 62!)
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 3);
        // ELF32 header is 52 bytes; e_phnum is at offset 44 (not 56!)
        assert_eq!(u16::from_le_bytes([elf[44], elf[45]]), 1);
        // entry = vaddr_align(0x400000 + page_align(52 + 32)) = 0x410000
        let entry = u32::from_le_bytes([elf[24], elf[25], elf[26], elf[27]]);
        assert_eq!(entry, 0x410000);
    }

    #[test]
    fn test_elf_header_with_bss() {
        let code = encode_ret();
        let elf = build_minimal_x86_32_elf(&code, 0x400000, 16); // 16 bytes of BSS

        // Check ELF magic
        assert_eq!(&elf[0..4], &[0x7f, b'E', b'L', b'F']);
        // e_type = ET_EXEC
        assert_eq!(u16::from_le_bytes([elf[16], elf[17]]), 2);
        // e_machine = EM_386 (3)
        assert_eq!(u16::from_le_bytes([elf[18], elf[19]]), 3);
        // With BSS, e_phnum = 2 (at offset 44 for ELF32)
        assert_eq!(u16::from_le_bytes([elf[44], elf[45]]), 2);
        // Entry point (u32 at offset 24)
        let entry = u32::from_le_bytes([elf[24], elf[25], elf[26], elf[27]]);
        // With 2 phdrs, text_offset = page_align(52 + 2*32) = page_align(116) = 0x1000
        // entry = vaddr_align(0x400000 + 0x1000) = 0x410000
        assert_eq!(entry, 0x410000);

        // Second program header (BSS) starts at offset 52 + 32 = 84
        // ELF32 Phdr layout: p_type(4) p_offset(4) p_vaddr(4) p_paddr(4) p_filesz(4) p_memsz(4) p_flags(4) p_align(4)
        let ph2 = 52 + 32; // = 84
        let p_type = u32::from_le_bytes([elf[ph2], elf[ph2+1], elf[ph2+2], elf[ph2+3]]);
        assert_eq!(p_type, 1); // PT_LOAD
        // p_flags is at offset 24 within the phdr
        let p_flags = u32::from_le_bytes([elf[ph2+24], elf[ph2+25], elf[ph2+26], elf[ph2+27]]);
        assert_eq!(p_flags, 6); // PF_R | PF_W
        let p_filesz = u32::from_le_bytes([elf[ph2+16], elf[ph2+17], elf[ph2+18], elf[ph2+19]]);
        assert_eq!(p_filesz, 0); // BSS has no file content
        let p_memsz = u32::from_le_bytes([elf[ph2+20], elf[ph2+21], elf[ph2+22], elf[ph2+23]]);
        assert_eq!(p_memsz, 16);
        let bss_vaddr = u32::from_le_bytes([elf[ph2+8], elf[ph2+9], elf[ph2+10], elf[ph2+11]]);
        // BSS vaddr should be 64K-aligned and after the text segment
        assert_eq!(bss_vaddr % 0x10000, 0, "BSS vaddr should be 64K-aligned");
        assert!(bss_vaddr > 0x410000, "BSS should be after text segment");
    }

    // ── Backend Trait Dispatch Test ─────────────────────────────────────

    #[test]
    fn test_backend_trait_dispatch() {
        let backend: Box<dyn Backend> = Box::new(X86_32Backend::new());
        assert_eq!(backend.name(), "x86_32");
        assert_eq!(backend.target_info().isa_name(), "x86_32");
        assert_eq!(backend.target_info().elf_machine_type(), 62);
        assert_eq!(backend.target_info().calling_convention_name(), "systemv");
    }

    // ── Backend TargetInfo Consistency Test ─────────────────────────────

    #[test]
    fn test_target_info_consistency() {
        let backend = X86_32Backend::new();
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
        let backend = X86_32Backend::new();
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
        let backend = X86_32Backend::new();
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
        // The stack-slot isel (src/codegen/src/x86_32/stack_slot_isel.rs:640)
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
    fn test_x86_32_disassemble_nop() {
        let backend = X86_32Backend::new();
        let bytes = encode_nop();
        let lines = backend.disassemble(&bytes, 0x1000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("nop"), "Expected nop, got: {}", lines[0]);
    }

    #[test]
    fn test_x86_32_disassemble_ret() {
        let backend = X86_32Backend::new();
        let bytes = encode_ret();
        let lines = backend.disassemble(&bytes, 0x1000);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("ret"), "Expected ret, got: {}", lines[0]);
    }

    #[test]
    fn test_x86_32_disassemble_push_pop() {
        let backend = X86_32Backend::new();
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
    fn test_x86_32_disassemble_mov_reg_reg() {
        let backend = X86_32Backend::new();
        let bytes = encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp);
        let lines = backend.disassemble(&bytes, 0);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("mov"), "Expected mov, got: {}", lines[0]);
    }

    #[test]
    fn test_x86_32_disassemble_add_sub() {
        let backend = X86_32Backend::new();
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
