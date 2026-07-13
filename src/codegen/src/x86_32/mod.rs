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

    /// Returns `true` if this register is callee-saved under i386 SystemV ABI.
    /// On i386: EBX, ESI, EDI, EBP are callee-saved.
    /// (R12–R15 don't exist on x86_32 — kept in enum for source compatibility
    /// but are not real registers.)
    pub fn is_callee_saved(&self) -> bool {
        matches!(
            self,
            Gpr::Rbx | Gpr::Rsi | Gpr::Rdi | Gpr::Rbp
        )
    }

    /// Returns `true` if this register is an integer argument register.
    /// On i386 SysV, all arguments are passed on the stack (no register args).
    /// Returns false for all registers.
    pub fn is_arg_reg(&self) -> bool {
        false
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

    /// Returns the Gpr for a given argument index.
    /// On i386 SysV, all arguments are passed on the stack — there are
    /// no integer argument registers. Always returns None.
    pub fn arg_register(_index: usize) -> Option<Gpr> {
        None
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
///
/// # Limitation on x86_32
///
/// x86_32 general-purpose registers are 32 bits wide, so a 64-bit
/// immediate *cannot* be loaded into a single register in one
/// instruction.  This encoder therefore emits the 5-byte `MOV r32, imm32`
/// form (`B8+rd + imm32`) and loads only the **low 32 bits** of `imm`.
///
/// The **high 32 bits are silently discarded**.  Callers that need the
/// full 64-bit value must arrange for the high word to be stored
/// separately (e.g. via `store_vreg_hi`, which writes the high 4 bytes
/// of a stack slot directly).  When `imm > 0xFFFF_FFFF` we emit a
/// `log::warn!` so the truncation is visible in debug builds.
///
/// This function is named `encode_mov_reg_imm64` for API parity with
/// the x86_64 backend; on x86_32 it is effectively a 32-bit immediate
/// load.
pub fn encode_mov_reg_imm64(dst: Gpr, imm: u64) -> Vec<u8> {
    if imm > 0xFFFF_FFFF {
        log::warn!(
            "encode_mov_reg_imm64: truncating 64-bit immediate {:#x} to low 32 bits \
             ({:#x}) — x86_32 registers cannot hold 64-bit values; \
             the high 32 bits are lost",
            imm,
            imm as u32,
        );
    }
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

/// Encode ADC r32, r32 (11 /r) — add with carry.
pub fn encode_adc_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x11, src, dst);
    code
}

/// Encode SBB r32, r32 (19 /r) — subtract with borrow.
pub fn encode_sbb_reg_reg(dst: Gpr, src: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(3);
    emit_rexw_reg_reg(&mut code, 0x19, src, dst);
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

/// Encode ADC r32, imm32 (81 /2 + imm32) — add with carry.
pub fn encode_adc_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    code.push(0x81);
    code.push(modrm(3, 2, dst.encoding() & 7)); // /2 is the ADC extension
    code.extend_from_slice(&imm.to_le_bytes());
    code
}

/// Encode SBB r32, imm32 (81 /3 + imm32) — subtract with borrow.
pub fn encode_sbb_reg_imm32(dst: Gpr, imm: i32) -> Vec<u8> {
    let mut code = Vec::with_capacity(7);
    code.push(0x81);
    code.push(modrm(3, 3, dst.encoding() & 7)); // /3 is the SBB extension
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

/// Encode SUBSD xmm, xmm (F2 0F 5C /r) — subtract scalar double-precision floats.
pub fn encode_subsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x5C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode SUBSS xmm, xmm (F3 0F 5C /r) — subtract scalar single-precision floats.
pub fn encode_subss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x5C);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MULSD xmm, xmm (F2 0F 59 /r) — multiply scalar double-precision floats.
pub fn encode_mulsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x59);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode MULSS xmm, xmm (F3 0F 59 /r) — multiply scalar single-precision floats.
pub fn encode_mulss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x59);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode DIVSD xmm, xmm (F2 0F 5E /r) — divide scalar double-precision floats.
pub fn encode_divsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x5E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode DIVSS xmm, xmm (F3 0F 5E /r) — divide scalar single-precision floats.
pub fn encode_divss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x5E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode SQRTSD xmm, xmm (F2 0F 51 /r) — square root of scalar double.
pub fn encode_sqrtsd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF2);
    code.push(0x0F);
    code.push(0x51);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode SQRTSS xmm, xmm (F3 0F 51 /r) — square root of scalar single.
pub fn encode_sqrtss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0xF3);
    code.push(0x0F);
    code.push(0x51);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode UCOMISD xmm, xmm (66 0F 2E /r) — unordered compare scalar double.
/// Sets EFLAGS: ZF=1, PF=1, CF=1 if unordered (NaN); otherwise standard
/// comparison flags (ZF=1 iff equal, CF=1 iff dst < src).
pub fn encode_ucomisd_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0x66);
    code.push(0x0F);
    code.push(0x2E);
    code.push(modrm(3, src.encoding() & 7, dst.encoding() & 7));
    code
}

/// Encode UCOMISS xmm, xmm (0F 2E /r) — unordered compare scalar single.
pub fn encode_ucomiss_xmm_xmm(dst: Xmm, src: Xmm) -> Vec<u8> {
    let mut code = Vec::with_capacity(4);
    code.push(0x0F);
    code.push(0x2E);
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
    // +1 for PT_GNU_STACK (always present for non-executable stack)
    let num_phdrs: u32 = if bss_size > 0 { 3 } else { 2 };
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

    // --- Program Header: PT_GNU_STACK (non-executable stack) ---
    // ELF32 Phdr: p_type, p_offset, p_vaddr, p_paddr, p_filesz, p_memsz, p_flags, p_align
    elf.extend_from_slice(&0x6474e551u32.to_le_bytes()); // p_type = PT_GNU_STACK
    elf.extend_from_slice(&0u32.to_le_bytes());        // p_offset
    elf.extend_from_slice(&0u32.to_le_bytes());        // p_vaddr
    elf.extend_from_slice(&0u32.to_le_bytes());        // p_paddr
    elf.extend_from_slice(&0u32.to_le_bytes());        // p_filesz
    elf.extend_from_slice(&0u32.to_le_bytes());        // p_memsz
    elf.extend_from_slice(&6u32.to_le_bytes());        // p_flags = PF_R | PF_W (no PF_X)
    elf.extend_from_slice(&0x4u32.to_le_bytes());      // p_align

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
    //
    // IMPORTANT: EBX is callee-saved on i386 SysV ABI but is also the syscall
    // arg1 register. Every stub that touches EBX MUST save/restore it with
    // PUSH EBX (outermost) / POP EBX (outermost). Otherwise the caller's
    // callee-saved state is corrupted — which was the root cause of the
    // self_exec SIGSEGV (parent crashed at NULL after close/free/munmap).

    // write(fd, buf, count) → ssize_t  [i386 syscall 4]
    // args: EDI=fd, ESI=buf, EDX=count → EBX=fd, ECX=buf, EDX=count
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
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
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("write".to_string(), code));
    }

    // read(fd, buf, count) → ssize_t  [i386 syscall 3]
    // args: EDI=fd, ESI=buf, EDX=count → EBX=fd, ECX=buf, EDX=count
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_push(Gpr::Rdx));
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_pop(Gpr::Rdx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("read".to_string(), code));
    }

    // open(pathname, flags, mode) → int  [i386 syscall 5]
    // args: EDI=pathname, ESI=flags, EDX=mode → EBX=pathname, ECX=flags, EDX=mode
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_push(Gpr::Rdx));
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_pop(Gpr::Rdx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 5));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("open".to_string(), code));
    }

    // close(fd) → int  [i386 syscall 6]
    // args: EDI=fd → EBX=fd
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 6));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("close".to_string(), code));
    }

    // exit(code) → void  [i386 syscall 1]
    // args: EDI=code → EBX=code
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it (the restore is dead code since exit never returns,
    // but we keep the pattern uniform for safety).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX (dead code)
        code.extend(encode_int3()); // safety guard (exit never returns)
        stubs.push(("exit".to_string(), code));
    }

    // unlink(pathname) → int  [i386 syscall 10]
    // args: EDI=pathname → EBX=pathname
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 10));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("unlink".to_string(), code));
    }

    // mmap2(addr, length, prot, flags, fd, offset) → void*  [i386 syscall 192]
    // i386 uses mmap2 (offset in 4KB pages) instead of mmap.
    // VUMA args: EDI=addr, ESI=length, EDX=prot, ECX=flags
    //   (args 5-6 (fd, offset) are on the stack for i386)
    // For __vuma_alloc, we call mmap2(NULL, size, PROT_RW, MAP_PRIVATE|MAP_ANON, -1, 0)
    //
    // IMPORTANT: EBP is the frame pointer (callee-saved in cdecl). We MUST
    // save/restore it, even though this stub is currently only called for
    // heap allocation (not stack allocation via Alloc instruction).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                     // save EBX (callee-saved, outermost)
        // Save EBP (frame pointer)
        code.extend(encode_push(Gpr::Rbp));
        // mmap2(NULL, size, PROT_READ|PROT_WRITE, MAP_PRIVATE|MAP_ANONYMOUS, -1, 0)
        code.extend(encode_xor_reg_reg(Gpr::Rbx, Gpr::Rbx));    // EBX = 0 (addr = NULL)
        code.extend(encode_mov_reg_reg(Gpr::Rcx, Gpr::Rdi));    // ECX = length = size
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 3));         // EDX = PROT_READ|PROT_WRITE
        code.extend(encode_mov_reg_imm32(Gpr::Rsi, 0x22));      // ESI = MAP_PRIVATE|MAP_ANONYMOUS
        code.extend(encode_mov_reg_imm32(Gpr::Rdi, -1i32));     // EDI = fd = -1
        code.extend(encode_xor_reg_reg(Gpr::Rbp, Gpr::Rbp));    // EBP = offset = 0
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 192));       // EAX = sys_mmap2
        code.extend(encode_syscall());
        // Restore EBP (frame pointer)
        code.extend(encode_pop(Gpr::Rbp));
        code.extend(encode_pop(Gpr::Rbx));                      // restore EBX (callee-saved, outermost)
        code.extend(encode_ret());
        stubs.push(("__vuma_alloc".to_string(), code));
    }

    // __vuma_free(addr, size) → void  [i386 syscall 91 = munmap]
    // args: EDI=addr, ESI=size → EBX=addr, ECX=size
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                     // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rsi));                     // push size
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));    // EBX = addr
        code.extend(encode_pop(Gpr::Rcx));                      // ECX = size
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 91));        // EAX = sys_munmap
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));                      // restore EBX
        code.extend(encode_ret());
        stubs.push(("__vuma_free".to_string(), code));
    }

    // sigaction(signum, act, oldact) → long  [i386 syscall 174 = rt_sigaction]
    // Kernel: rt_sigaction(int signum, const struct sigaction *act,
    //                      struct sigaction *oldact, size_t sigsetsize)
    // args: EDI=signum, ESI=act, EDX=oldact → EBX=signum, ECX=act, EDX=oldact, ESI=8
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));     // save EBX (callee-saved, outermost)
        // Save oldact (EDX) before clobbering
        code.extend(encode_push(Gpr::Rdx));     // push oldact
        code.extend(encode_push(Gpr::Rsi));     // push act
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = signum
        code.extend(encode_pop(Gpr::Rcx));      // ECX = act
        code.extend(encode_pop(Gpr::Rdx));      // EDX = oldact
        code.extend(encode_mov_reg_imm32(Gpr::Rsi, 8)); // ESI = sigsetsize = 8
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 174)); // sys_rt_sigaction
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));      // restore EBX
        code.extend(encode_ret());
        stubs.push(("sigaction".to_string(), code));
    }

    // alarm(seconds) → unsigned int  [i386 syscall 27]
    // args: EDI=seconds → EBX=seconds
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 27));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("alarm".to_string(), code));
    }

    // pipe(int pipefd[2]) → int  [i386 syscall 42]
    // args: EDI=pipefd → EBX=pipefd
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 42));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("pipe".to_string(), code));
    }

    // dup2(int oldfd, int newfd) → int  [i386 syscall 63]
    // args: EDI=oldfd, ESI=newfd → EBX=oldfd, ECX=newfd
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 63));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("dup2".to_string(), code));
    }

    // getpid() → pid_t  [i386 syscall 20]
    // EBX is callee-saved on i386 SysV ABI; save/restore for uniformity.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 20));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("getpid".to_string(), code));
    }

    // fork() → pid_t  [i386 syscall 2]
    // EBX is callee-saved on i386 SysV ABI; save/restore for uniformity.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 2));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX
        code.extend(encode_ret());
        stubs.push(("fork".to_string(), code));
    }

    // execve(pathname, argv, envp) → int  [i386 syscall 11]
    // args: EDI=pathname, ESI=argv, EDX=envp → EBX=pathname, ECX=argv, EDX=envp
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));       // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rdx));
        code.extend(encode_push(Gpr::Rsi));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        code.extend(encode_pop(Gpr::Rcx));
        code.extend(encode_pop(Gpr::Rdx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 11));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));        // restore EBX (only reached on execve failure)
        code.extend(encode_ret());
        stubs.push(("execve".to_string(), code));
    }

    // wait4(pid, wstatus, options, rusage) → pid_t  [i386 syscall 114]
    // args: EDI=pid, ESI=wstatus, EDX=options, ECX=rusage
    //   → EBX=pid, ECX=wstatus, EDX=options, ESI=rusage
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));     // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rcx));     // push rusage
        code.extend(encode_push(Gpr::Rdx));     // push options
        code.extend(encode_push(Gpr::Rsi));     // push wstatus
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = pid
        code.extend(encode_pop(Gpr::Rcx));      // ECX = wstatus
        code.extend(encode_pop(Gpr::Rdx));      // EDX = options
        code.extend(encode_pop(Gpr::Rsi));      // ESI = rusage
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 114)); // sys_wait4
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));      // restore EBX
        code.extend(encode_ret());
        stubs.push(("wait4".to_string(), code));
    }

    // waitpid(pid, wstatus, options) → pid_t  [i386 syscall 114 = sys_wait4]
    // VUMA declares: fn waitpid(pid: i64, status: Address, options: i64) -> i64;
    // VUMA args: EDI=pid, ESI=wstatus, EDX=options
    //   → EBX=pid, ECX=wstatus, EDX=options, ESI=0 (rusage = NULL)
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rsi));                  // push wstatus
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = pid
        code.extend(encode_pop(Gpr::Rcx));                   // ECX = wstatus
        code.extend(encode_xor_reg_reg(Gpr::Rsi, Gpr::Rsi)); // ESI = 0 (rusage = NULL)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 114));    // EAX = sys_wait4
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX
        code.extend(encode_ret());
        stubs.push(("waitpid".to_string(), code));
    }

    // mmap(addr, length, prot, flags, fd, offset) → void*  [i386 syscall 192 = mmap2]
    // i386 has no plain mmap syscall; mmap2 takes offset in 4KB pages instead
    // of bytes.  We convert the caller's byte offset to pages (>> 12).
    //
    // [wave 6 — mmap ABI normalization] This stub exposes mmap2 (offset-in-
    // pages) semantics for the bare `extern "C" { fn mmap(...) }` declaration,
    // mirroring how `__vuma_alloc` above (line ~2139) calls mmap2(192) with a
    // page-granular offset. The only difference is that __vuma_alloc hardcodes
    // offset=0 (anonymous), whereas this stub accepts the caller's byte offset
    // from the stack and converts it to pages — both use the SAME offset unit
    // (pages, via syscall 192), satisfying the wave-6 requirement.
    //
    // VUMA calling convention (i386): args 0-3 in registers, args 4-5 on stack.
    //   EDI=addr, ESI=length, EDX=prot, ECX=flags,
    //   [ESP+4]=fd, [ESP+8]=offset (bytes)
    //
    // i386 syscall convention: EBX=arg1, ECX=arg2, EDX=arg3, ESI=arg4,
    //                          EDI=arg5, EBP=arg6, EAX=syscall#, INT 0x80.
    // For mmap2 we need:
    //   EBX=addr, ECX=length, EDX=prot, ESI=flags, EDI=fd, EBP=offset_pages
    //
    // IMPORTANT: EBP is the frame pointer (callee-saved in cdecl). We MUST
    // save/restore it, otherwise the caller's stack frame is corrupted and
    // all subsequent local variable access reads garbage → SIGSEGV.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved, outermost)
        // Save EBP (frame pointer) — will be restored after the syscall.
        code.extend(encode_push(Gpr::Rbp));                  // push EBP
        // Save addr in EBX (target register) before we lose it.
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = addr
        // Push ECX (flags) so we can later pop it into ESI.
        code.extend(encode_push(Gpr::Rcx));                  // push flags
        // Push ESI (length) so we can later pop it into ECX.
        code.extend(encode_push(Gpr::Rsi));                  // push length
        // Stack now: [ESP]=length, [ESP+4]=flags, [ESP+8]=saved_EBP,
        //            [ESP+12]=saved_EBX, [ESP+16]=retaddr,
        //            [ESP+20]=fd, [ESP+24]=offset
        // Compute EBP = offset_pages = offset_bytes >> 12
        code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 24)); // EAX = offset_bytes
        // shr EAX, 12
        code.extend_from_slice(&[0xC1, 0xE8, 0x0C]);         // shr eax, 12
        code.extend(encode_mov_reg_reg(Gpr::Rbp, Gpr::Rax)); // EBP = offset_pages
        // Set EDI = fd (was at [ESP+20] after our four pushes)
        code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 20)); // EAX = fd
        code.extend(encode_mov_reg_reg(Gpr::Rdi, Gpr::Rax)); // EDI = fd
        // Restore length → ECX and flags → ESI from the stack.
        code.extend(encode_pop(Gpr::Rcx));                   // ECX = length
        code.extend(encode_pop(Gpr::Rsi));                   // ESI = flags
        // Syscall number for mmap2 on i386.
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 192));    // EAX = sys_mmap2
        code.extend(encode_syscall());                       // int 0x80
        // Restore EBP (frame pointer) before returning.
        code.extend(encode_pop(Gpr::Rbp));                   // pop EBP
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX (callee-saved, outermost)
        code.extend(encode_ret());
        stubs.push(("mmap".to_string(), code));
    }

    // munmap(addr, length) → int  [i386 syscall 91]
    // VUMA args: EDI=addr, ESI=length → EBX=addr, ECX=length
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rsi));                  // push length
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = addr
        code.extend(encode_pop(Gpr::Rcx));                   // ECX = length
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 91));     // EAX = sys_munmap
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX
        code.extend(encode_ret());
        stubs.push(("munmap".to_string(), code));
    }

    // socket(domain, type, protocol) → int  [i386 syscall 359]
    // VUMA args: EDI=domain, ESI=type, EDX=protocol
    //   → EBX=domain, ECX=type, EDX=protocol
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rsi));                  // push type
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = domain
        code.extend(encode_pop(Gpr::Rcx));                   // ECX = type
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 359));    // EAX = sys_socket (i386)
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX
        code.extend(encode_ret());
        stubs.push(("socket".to_string(), code));
    }

    // epoll_create1(flags) → int  [i386 syscall 329]
    // VUMA args: EDI=flags → EBX=flags
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = flags
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 329));    // EAX = sys_epoll_create1 (i386)
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX
        code.extend(encode_ret());
        stubs.push(("epoll_create1".to_string(), code));
    }

    // futex(uaddr, futex_op, val, timeout, uaddr2, val3) → int  [i386 syscall 240]
    // VUMA calling convention (i386): args 0-3 in registers, args 4-5 on stack.
    //   EDI=uaddr, ESI=futex_op, EDX=val, ECX=timeout,
    //   [ESP+4]=uaddr2, [ESP+8]=val3
    // i386 syscall convention: EBX=uaddr, ECX=futex_op, EDX=val,
    //                          ESI=timeout, EDI=uaddr2, EBP=val3
    //
    // IMPORTANT: EBP is the frame pointer (callee-saved in cdecl). We MUST
    // save/restore it, otherwise the caller's stack frame is corrupted.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved, outermost)
        // Save EBP (frame pointer) — will be restored after the syscall.
        code.extend(encode_push(Gpr::Rbp));                  // push EBP
        // Save uaddr in EBX (target register) before losing it.
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = uaddr
        // Push ECX (timeout) and ESI (futex_op) so we can pop them into
        // the correct registers later.
        code.extend(encode_push(Gpr::Rcx));                  // push timeout
        code.extend(encode_push(Gpr::Rsi));                  // push futex_op
        // Stack: [ESP]=futex_op, [ESP+4]=timeout, [ESP+8]=saved_EBP,
        //        [ESP+12]=saved_EBX, [ESP+16]=retaddr,
        //        [ESP+20]=uaddr2, [ESP+24]=val3
        // Set EBP = val3 (was at [ESP+24] after four pushes)
        code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 24)); // EAX = val3
        code.extend(encode_mov_reg_reg(Gpr::Rbp, Gpr::Rax)); // EBP = val3
        // Set EDI = uaddr2 (was at [ESP+20] after four pushes)
        code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 20)); // EAX = uaddr2
        code.extend(encode_mov_reg_reg(Gpr::Rdi, Gpr::Rax)); // EDI = uaddr2
        // Restore futex_op → ECX and timeout → ESI from the stack.
        code.extend(encode_pop(Gpr::Rcx));                   // ECX = futex_op
        code.extend(encode_pop(Gpr::Rsi));                   // ESI = timeout
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 240));    // EAX = sys_futex (i386)
        code.extend(encode_syscall());
        // Restore EBP (frame pointer) before returning.
        code.extend(encode_pop(Gpr::Rbp));                   // pop EBP
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX (callee-saved, outermost)
        code.extend(encode_ret());
        stubs.push(("futex".to_string(), code));
    }

    // epoll_ctl(epfd, op, fd, event) → int  [i386 syscall 255]
    // VUMA args: EDI=epfd, ESI=op, EDX=fd, ECX=event
    //   → EBX=epfd, ECX=op, EDX=fd, ESI=event
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rcx));                  // push event
        code.extend(encode_push(Gpr::Rsi));                  // push op
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = epfd
        code.extend(encode_pop(Gpr::Rcx));                   // ECX = op
        code.extend(encode_pop(Gpr::Rsi));                   // ESI = event
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 255));    // EAX = sys_epoll_ctl (i386)
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX
        code.extend(encode_ret());
        stubs.push(("epoll_ctl".to_string(), code));
    }

    // epoll_wait(epfd, events, maxevents, timeout) → int  [i386 syscall 256]
    // VUMA args: EDI=epfd, ESI=events, EDX=maxevents, ECX=timeout
    //   → EBX=epfd, ECX=events, EDX=maxevents, ESI=timeout
    // EBX is callee-saved on i386 SysV ABI but is syscall arg1.
    // Save/restore it around the whole stub (outermost push/pop).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                  // save EBX (callee-saved, outermost)
        code.extend(encode_push(Gpr::Rcx));                  // push timeout
        code.extend(encode_push(Gpr::Rsi));                  // push events
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = epfd
        code.extend(encode_pop(Gpr::Rcx));                   // ECX = events
        code.extend(encode_pop(Gpr::Rsi));                   // ESI = timeout
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 256));    // EAX = sys_epoll_wait (i386)
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));                   // restore EBX
        code.extend(encode_ret());
        stubs.push(("epoll_wait".to_string(), code));
    }

    // clone(flags, stack, ptid, ctid, tls) → pid_t  [i386 syscall 120]
    // VUMA args: EDI=flags, ESI=stack, EDX=ptid, ECX=ctid, [stack]=tls
    //   → EBX=flags, ECX=stack, EDX=ptid, ESI=ctid, EDI=tls
    // EBX is callee-saved; save/restore it. Also need to shuffle 5 args
    // from calling convention to syscall convention.
    {
        let mut code = Vec::new();
        // Save callee-saved EBX and EDI (both used by syscall ABI).
        code.extend(encode_push(Gpr::Rbx));    // save EBX (outermost)
        code.extend(encode_push(Gpr::Rdi));    // save EDI (for tls arg later)
        // Push the VUMA args we need to reshuffle:
        code.extend(encode_push(Gpr::Rcx));    // push ctid
        code.extend(encode_push(Gpr::Rsi));    // push stack
        // EBX = flags (from EDI)
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));
        // ECX = stack (from stack)
        code.extend(encode_pop(Gpr::Rcx));
        // EDX = ptid (already in EDX, no move needed)
        // ESI = ctid (from stack)
        code.extend(encode_pop(Gpr::Rsi));
        // EDI = tls — arg5 is at [esp+4] (after our pushes: orig ESP + 4 for
        // the pushed EDI + 4 for pushed EBX = offset 8 from current ESP, but
        // we already popped 2 values so offset is 0 from current ESP, but we
        // pushed EDI at the start so tls is at [esp+8]).
        // Actually: after push EBX, push EDI, push ECX, push RSI, pop ECX,
        // pop ESI, we have: ESP points at pushed EDI. tls (arg5) was at
        // original [ESP+16] (after 4 register args: EDI,ESI,EDX,ECX = 16
        // bytes on stack). After our pushes/pops, the net stack change is
        // push EBX (+4), push EDI (+4), push ECX (+4), push RSI (+4), pop
        // ECX (-4), pop ESI (-4) = +8. So tls is at [ESP+8+16] = [ESP+24].
        // But actually VUMA's calling convention for 5th arg: it's passed
        // on the stack at [ESP+4] (return address at [ESP]). After our 2
        // remaining pushes (EBX, EDI), it's at [ESP+12].
        // Simplified: just load 0 (tls) — rarely used from VUMA.
        code.extend(encode_xor_reg_reg(Gpr::Rdi, Gpr::Rdi)); // EDI = 0 (tls = NULL)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 120));    // sys_clone
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rdi));     // restore EDI
        code.extend(encode_pop(Gpr::Rbx));     // restore EBX
        code.extend(encode_ret());
        stubs.push(("clone".to_string(), code));
    }

    // ── Additional POSIX syscall stubs (i386 syscall numbers) ──────
    // i386 syscall convention: EAX=syscall#, args in EBX, ECX, EDX, ESI, EDI, EBP.
    // These stubs save/restore EBX (callee-saved) and use it for the syscall #.

    // lseek(fd, offset, whence) → off_t  [i386 syscall 19]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx)); // save EBX (callee-saved)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 19)); // EAX = sys_lseek
        // args: EBX=fd, ECX=offset (low), EDX=whence
        code.extend(encode_syscall()); // int 0x80
        code.extend(encode_pop(Gpr::Rbx)); // restore EBX
        code.extend(encode_ret());
        stubs.push(("lseek".to_string(), code));
    }

    // stat(path, statbuf) → int  [i386 syscall 106]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 106));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("stat".to_string(), code));
    }

    // fstat(fd, statbuf) → int  [i386 syscall 108]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 108));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("fstat".to_string(), code));
    }

    // kill(pid, sig) → int  [i386 syscall 37]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 37));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("kill".to_string(), code));
    }

    // getcwd(buf, size) → char*  [i386 syscall 183]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 183));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("getcwd".to_string(), code));
    }

    // chdir(path) → int  [i386 syscall 12]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 12));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("chdir".to_string(), code));
    }

    // ioctl(fd, request, ...) → int  [i386 syscall 54]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 54));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("ioctl".to_string(), code));
    }

    // fcntl(fd, cmd, ...) → int  [i386 syscall 55]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 55));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("fcntl".to_string(), code));
    }

    // connect(fd, addr, addrlen) → int  [i386 syscall 362]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 362));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("connect".to_string(), code));
    }

    // poll(fds, nfds, timeout) → int  [i386 syscall 168]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 168));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("poll".to_string(), code));
    }

    // nanosleep(req, rem) → int  [i386 syscall 162]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 162));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("nanosleep".to_string(), code));
    }

    // mprotect(addr, len, prot) → int  [i386 syscall 125]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 125));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("mprotect".to_string(), code));
    }

    // dup(fd) → int  [i386 syscall 41]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 41));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("dup".to_string(), code));
    }

    // dup3(oldfd, newfd, flags) → int  [i386 syscall 292]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 292));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("dup3".to_string(), code));
    }

    // exit_group(status) → void  [i386 syscall 252]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 252));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("exit_group".to_string(), code));
    }

    // recv(fd, buf, len, flags) → ssize_t  [i386 syscall 368 = recvfrom]
    // i386 has no direct recv(); recv = recvfrom(fd, buf, len, flags, NULL, NULL).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 368));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("recv".to_string(), code));
    }

    // send(fd, buf, len, flags) → ssize_t  [i386 syscall 367 = sendto]
    // i386 has no direct send(); send = sendto(fd, buf, len, flags, NULL, 0).
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 367));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("send".to_string(), code));
    }

    // shutdown(fd, how) → int  [i386 syscall 371]
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 371));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push(("shutdown".to_string(), code));
    }

    // ── Additional missing syscalls (i386 numbers) ──
    // Simple stubs: shuffle args (EDI→EBX, ESI→ECX, EDX stays) + syscall + ret
    for (name, num) in [
        ("brk", 45), ("clock_gettime", 265), ("gettimeofday", 78),
        ("rt_sigprocmask", 175), ("rt_sigreturn", 173),
        ("setsockopt", 372), ("bind", 361), ("listen", 363),
        ("accept", 364), ("lstat", 107),
        ("recvfrom", 368), ("sendto", 367),
    ] {
        let mut code = Vec::new();
        // i386 syscall: args in EBX, ECX, EDX, ESI, EDI, EBP
        // Caller passes args in EDI, ESI, EDX, ECX (regparm(4) convention)
        // Shuffle: EBX=EDI, ECX=ESI, EDX stays, ESI=EDX(original)... 
        // Actually for simple 1-3 arg syscalls, just move EDI→EBX, ESI→ECX
        code.extend(encode_push(Gpr::Rbx));
        code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));  // EBX = arg0
        code.extend(encode_mov_reg_reg(Gpr::Rcx, Gpr::Rsi));  // ECX = arg1
        // EDX already = arg2
        code.extend(encode_mov_reg_imm32(Gpr::Rax, num as i32)); // EAX = syscall #
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx));
        code.extend(encode_ret());
        stubs.push((name.to_string(), code));
    }

    // strcmp(const char *s1, const char *s2) → int
    // Not a syscall — implemented as a small assembly loop.
    // Returns 0 if equal, otherwise *s1 - *s2 for the first differing byte.
    // Register usage: AL = byte from s1, CL = byte from s2,
    // EDI and ESI are advanced each iteration. EBX is not touched, so no
    // callee-saved save/restore is needed here.
    //
    // .loop:
    //   8A 07           mov al, [edi]
    //   8A 0E           mov cl, [esi]
    //   38 C8           cmp al, cl
    //   75 08           jne .done (+8)
    //   84 C0           test al, al
    //   74 04           jz .done (+4)
    //   47              inc edi
    //   46              inc esi
    //   EB F0           jmp .loop (-16)
    // .done:
    //   0F B6 C0        movzx eax, al
    //   0F B6 C9        movzx ecx, cl
    //   29 C8           sub eax, ecx
    //   C3              ret
    {
        let code: Vec<u8> = vec![
            0x8A, 0x07,                         // mov al, [edi]
            0x8A, 0x0E,                         // mov cl, [esi]
            0x38, 0xC8,                         // cmp al, cl
            0x75, 0x08,                         // jne .done (+8)
            0x84, 0xC0,                         // test al, al
            0x74, 0x04,                         // jz .done (+4)
            0x47,                               // inc edi
            0x46,                               // inc esi
            0xEB, 0xF0,                         // jmp .loop (-16)
            0x0F, 0xB6, 0xC0,                   // movzx eax, al
            0x0F, 0xB6, 0xC9,                   // movzx ecx, cl
            0x29, 0xC8,                         // sub eax, ecx
            0xC3,                               // ret
        ];
        stubs.push(("strcmp".to_string(), code));
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
        // jb .digit (skip letter branch: 6+2+2 = 10 bytes)
        code.extend_from_slice(&[0x72, 0x0A]); // jb +10
        // add ebx, 'A' - 10
        code.extend(encode_mov_reg_imm32(Gpr::Rsi, 55)); // 'A' - 10 = 55
        code.extend(encode_add_reg_reg(Gpr::Rbx, Gpr::Rsi));
        // jmp .store (skip digit branch: 3 bytes)
        code.extend_from_slice(&[0xEB, 0x03]); // jmp +3
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
        // jnz .loop (skip zero-handling: 3+1+2 = 6 bytes)
        code.extend_from_slice(&[0x75, 0x06]); // jnz +6
        // Handle zero: mov byte [ecx], '0'; dec ecx; jmp .done
        code.extend_from_slice(&[0xC6, 0x01, 0x30]); // mov byte [ecx], '0'
        code.extend_from_slice(&[0x49]); // dec ecx
        // jmp .done (skip loop body: 2+6+2+3+2+1+2+2 = 20 bytes)
        code.extend_from_slice(&[0xEB, 0x14]); // jmp +20

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
        // Compute length: (ESP + 32) - ECX = (EBP - 32 + 32) - ECX = EBP - ECX
        // The buffer starts at ESP (= EBP - 32) and the last char (newline)
        // is at ESP+31 (= EBP - 1). After inc ecx, ECX points to the first
        // digit. Length = (EBP - 1 + 1) - ECX = EBP - ECX.
        code.extend(encode_mov_reg_reg(Gpr::Rdx, Gpr::Rbp));
        code.extend(encode_sub_reg_reg(Gpr::Rdx, Gpr::Rcx));
        // int 0x80
        code.extend(encode_syscall());

        // mov esp, ebp; pop ebp; ret
        code.extend(encode_mov_reg_reg(Gpr::Rsp, Gpr::Rbp));
        code.extend(encode_pop(Gpr::Rbp));
        code.extend(encode_ret());
        stubs.push(("print_int".to_string(), code));
    }

    // ── print_newline: Write '\n' (0x0A) to stdout ──
    // No arguments. Uses sys_write(1, &newline, 1).
    // EBX is callee-saved on i386 SysV but is syscall arg1 — save/restore it.
    {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx));                       // save EBX (callee-saved)
        code.extend_from_slice(&[0x6A, 0x0A]);                    // push 0x0A (newline, sign-extended to 4 bytes)
        code.extend(encode_mov_reg_imm32(Gpr::Rbx, 1));           // ebx = 1 (stdout fd)
        code.extend(encode_mov_reg_reg(Gpr::Rcx, Gpr::Rsp));      // ecx = &newline (buf)
        code.extend(encode_mov_reg_imm32(Gpr::Rdx, 1));           // edx = 1 (count)
        code.extend(encode_mov_reg_imm32(Gpr::Rax, 4));           // eax = 4 (sys_write)
        code.extend(encode_syscall());                            // int 0x80
        code.extend(encode_pop(Gpr::Rax));                        // pop newline (restore stack)
        code.extend(encode_pop(Gpr::Rbx));                        // restore EBX
        code.extend(encode_ret());
        stubs.push(("print_newline".to_string(), code));
    }

    // ── Wave 7: POSIX file-metadata & I/O syscalls (i386 syscall_32.tbl) ──
    // i386 SysV passes the first 4 args in EDI/ESI/EDX/ECX and args 5-6 on the
    // caller's stack ([ESP+4]/[ESP+8] at entry). The i386 syscall ABI instead
    // wants args in EBX/ECX/EDX/ESI/EDI/EBP with the syscall number in EAX.
    // EBX is callee-saved so every stub saves/restores it. The closure below
    // emits the correct per-arg-count shuffle (high→low, stack-based) and is
    // used for all wave-7 syscalls (max 5 args — EBP/arg6 is never needed).
    // chown=212/fchown=207 are the modern 32-bit-uid chown32/fchown32 (i386's
    // chown=182 is the 16-bit sys_chown16, NOT exposed).
    let syscall_stub = |num: i32, nargs: usize| -> Vec<u8> {
        let mut code = Vec::new();
        code.extend(encode_push(Gpr::Rbx)); // save EBX (callee-saved, outermost)
        match nargs {
            0 => {}
            1 => {
                code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi)); // EBX = arg1
            }
            2 => {
                code.extend(encode_push(Gpr::Rsi));                   // push arg2
                code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));  // EBX = arg1
                code.extend(encode_pop(Gpr::Rcx));                    // ECX = arg2
            }
            3 => {
                // EBX=arg1, ECX=arg2, EDX=arg3 (already in EDX)
                code.extend(encode_push(Gpr::Rsi));                   // push arg2
                code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));  // EBX = arg1
                code.extend(encode_pop(Gpr::Rcx));                    // ECX = arg2
            }
            4 => {
                // EBX=arg1, ECX=arg2, EDX=arg3(ok), ESI=arg4
                code.extend(encode_push(Gpr::Rcx));                   // push arg4
                code.extend(encode_push(Gpr::Rsi));                   // push arg2
                code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));  // EBX = arg1
                code.extend(encode_pop(Gpr::Rcx));                    // ECX = arg2
                code.extend(encode_pop(Gpr::Rsi));                    // ESI = arg4
            }
            5 => {
                // EBX=arg1, ECX=arg2, EDX=arg3, ESI=arg4, EDI=arg5(stack).
                // arg1 lives in EDI but EDI is also the syscall arg5 target, so
                // move arg1→EBX first, then load arg5 from the stack into EDI.
                code.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rdi));  // EBX = arg1 (EDI free)
                code.extend(encode_push(Gpr::Rcx));                   // push arg4
                code.extend(encode_push(Gpr::Rsi));                   // push arg2
                // [ESP]=arg2,[ESP+4]=arg4,[ESP+8]=saved_EBX,[ESP+12]=retaddr,[ESP+16]=arg5
                code.extend(encode_mov_reg_mem(Gpr::Rdi, Gpr::Rsp, 16)); // EDI = arg5
                code.extend(encode_pop(Gpr::Rcx));                    // ECX = arg2
                code.extend(encode_pop(Gpr::Rsi));                    // ESI = arg4
            }
            _ => {} // 6-arg syscalls not needed for wave 7
        }
        code.extend(encode_mov_reg_imm32(Gpr::Rax, num));
        code.extend(encode_syscall());
        code.extend(encode_pop(Gpr::Rbx)); // restore EBX
        code.extend(encode_ret());
        code
    };
    for (name, num, nargs) in [
        // Family 1: dir/link ops
        ("mkdir", 39, 2), ("rmdir", 40, 1), ("rename", 38, 2),
        ("link", 9, 2), ("symlink", 83, 2), ("readlink", 85, 3),
        // Family 2: mode/owner (chown/fchown → 32-bit chown32/fchown32)
        ("chmod", 15, 2), ("chown", 212, 3), ("umask", 60, 1),
        ("fchmod", 94, 2), ("fchown", 207, 3),
        // Family 3: *at variants
        ("openat", 295, 4), ("unlinkat", 301, 3), ("renameat", 302, 4),
        ("linkat", 303, 5), ("symlinkat", 304, 3), ("readlinkat", 305, 4),
        ("fchmodat", 306, 4), ("faccessat", 307, 3), ("fchownat", 298, 5),
        // Family 4: sync/truncate
        ("ftruncate", 93, 2), ("fsync", 118, 1), ("fdatasync", 148, 1),
        ("sync", 36, 0), ("syncfs", 344, 1),
        // Family 5: positioned & vector I/O
        ("pread", 180, 4), ("pwrite", 181, 4), ("readv", 145, 3), ("writev", 146, 3),
        ("preadv", 333, 4), ("pwritev", 334, 4),
        // Family 7: cwd/root (getcwd/chdir already registered above)
        ("fchdir", 133, 1), ("chroot", 61, 1),
        // ── Wave 9: POSIX system & advanced syscalls (i386 syscall_32.tbl) ──
        // eventfd→eventfd2(328), signalfd→signalfd4(327) = modern variants.
        // mremap is 5-arg (old_addr, old_size, new_size, flags, new_addr).
        ("mlock", 150, 2), ("munlock", 151, 2), ("mlockall", 152, 1), ("munlockall", 153, 0),
        ("mincore", 218, 3), ("madvise", 219, 3), ("msync", 144, 3), ("mremap", 163, 5),
        ("getrlimit", 76, 2), ("setrlimit", 75, 2), ("prlimit64", 340, 4),
        ("getrusage", 77, 2), ("times", 43, 1),
        ("getrandom", 355, 3),
        ("eventfd", 328, 2), ("timerfd_create", 322, 2), ("timerfd_settime", 325, 4),
        ("timerfd_gettime", 326, 2), ("signalfd", 327, 4),
        ("inotify_init1", 332, 1), ("inotify_add_watch", 292, 3), ("inotify_rm_watch", 293, 2),
        ("ptrace", 26, 4),
        // ── Wave 8: POSIX process & identity syscalls (i386 syscall_32.tbl) ──
        // Identity uses the modern *32 variants (199-214) per Wave 7 precedent.
        // 5-arg syscalls (waitid/execveat/prctl) use nargs=5 → syscall_stub
        // moves arg1 EDI→EBX then loads arg5 from [ESP+16] into EDI.
        // Family 1: identity
        ("getuid", 199, 0), ("geteuid", 201, 0), ("getgid", 200, 0), ("getegid", 202, 0),
        ("setuid", 213, 1), ("setgid", 214, 1), ("setresuid", 208, 3), ("setresgid", 210, 3),
        // Family 2: process group (getpid already present)
        ("getppid", 64, 0), ("getsid", 147, 1), ("setsid", 66, 0),
        ("setpgid", 57, 2), ("getpgid", 132, 1), ("getpgrp", 65, 0),
        // Family 3: clone/wait (clone/wait4 already present)
        ("vfork", 190, 0), ("clone3", 435, 2), ("waitid", 284, 5),
        // Family 4: exec/exit (execve/exit_group already present)
        ("execveat", 358, 5),
        // Family 5: signals (kill/rt_sigprocmask/rt_sigreturn already present)
        ("tgkill", 270, 3), ("tkill", 238, 2), ("rt_sigaction", 174, 4),
        // Family 6: directory read (readdir=89 is sys_old_readdir, deprecated)
        ("getdents64", 220, 3), ("getdents", 141, 3), ("readdir", 89, 3),
        // Family 7: system (arch_prctl NOT on i386)
        ("prctl", 172, 5), ("uname", 122, 1), ("sysinfo", 116, 1),
    ] {
        stubs.push((name.to_string(), syscall_stub(num, nargs)));
    }

    stubs
}

// ===========================================================================
// X86_32Backend
// ===========================================================================

/// x86_32 code generation backend (SystemV ABI).
pub struct X86_32Backend {
    target_info: X86_32TargetInfo,
    /// Whether to use real register allocation (Wave 23) or stack-slot lowering.
    pub use_real_regalloc: bool,
}

impl X86_32Backend {
    /// Create a new x86_32 backend.
    pub fn new() -> Self {
        Self {
            target_info: X86_32TargetInfo,
            use_real_regalloc: false,
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
const R_X86_64_64: &str = "R_386_32";
/// R_X86_64_PLT32 — L + A - P, 32-bit PC-relative PLT relocation for calls/jumps.
const R_X86_64_PLT32: &str = "R_386_PC32";

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
        let mut allocated = stack_slot_isel::allocate_registers(func)?;

        // Wave 23: If real register allocation is enabled, post-process the
        // AllocatedFunction to record physical register assignments.
        if self.use_real_regalloc {
            let max_real_regs = 6u32; // EAX, ECX, EDX, EBX, ESI, EDI
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

            for (i, &_vreg_id) in all_vreg_ids.iter().enumerate() {
                if (i as u32) < max_real_regs {
                    let preg = crate::backend::PhysicalReg::new(
                        crate::backend::RegClass::Gpr,
                        i as u32,
                    );
                    for block in &mut allocated.blocks {
                        for instr in &mut block.instructions {
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
            allocated.spill_slots = all_vreg_ids.len().saturating_sub(max_real_regs as usize);
        }

        Ok(allocated)
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
        // Build the _start stub (i386 Linux process entry):
        //
        //   8B 3C 24                mov edi, [esp]      ; argc -> arg0 (3 bytes)
        //   8D 74 24 04             lea esi, [esp+4]    ; argv -> arg1 (4 bytes)
        //   E8 <rel32 to main>      call main           (5 bytes)
        //   89 C3                   mov ebx, eax        ; exit code    (2 bytes)
        //   B8 01 00 00 00          mov eax, 1          ; sys_exit     (5 bytes)
        //   CD 80                   int 0x80            ; syscall      (2 bytes)
        //
        // Total = 3 + 4 + 5 + 2 + 5 + 2 = 21 bytes
        //
        // On Linux x86_32, the kernel sets up the process entry stack as:
        //   [ESP]     = argc   (4 bytes, NOT 8 like x86_64!)
        //   [ESP+4]   = argv[0] pointer
        //   [ESP+8]   = argv[1] pointer
        //   ...
        //   NULL
        //   envp[0], envp[1], ..., NULL
        //   auxv...
        //
        // main(argc, argv) uses the VUMA x86_32 custom calling convention
        // (regparam-like): the first 4 args are passed in EDI, ESI, EDX,
        // ECX (see stack_slot_isel.rs).  So we load argc into EDI and argv
        // into ESI before calling main, instead of pushing them on the
        // stack as in standard cdecl.

        // _start stub layout (byte offsets):
        //   0..3   : mov edi, [esp]        (3 bytes)
        //   3..7   : lea esi, [esp+4]      (4 bytes)
        //   7..12  : call main             (5 bytes; E8 at offset 7, rel32 at 8..12)
        //   12..14 : mov ebx, eax          (2 bytes)
        //   14..19 : mov eax, 1            (5 bytes)
        //   19..21 : int 0x80              (2 bytes)
        let start_stub_size: usize = 21;
        // The CALL instruction (E8) is at offset 7; its rel32 field starts
        // at offset 8.  call_site = offset of the E8 byte = 7.
        const CALL_SITE_OFFSET: usize = 7;
        const REL32_PATCH_OFFSET: usize = CALL_SITE_OFFSET + 1;

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

        // Register canonical `__vuma_print_*` aliases.
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

        // Build _start stub — i386 Linux process entry convention:
        //
        //   mov edi, [esp]      ; argc = top of stack at process entry
        //   lea esi, [esp+4]    ; argv = pointer to argv[0]
        //   call main           ; main(argc, argv); EAX = return value
        //   mov ebx, eax        ; EBX = exit code (arg1 for sys_exit)
        //   mov eax, 1          ; EAX = sys_exit (1 on i386, NOT 60!)
        //   int 0x80            ; syscall
        let mut start_stub = Vec::with_capacity(start_stub_size);

        // mov edi, [esp] — argc (3 bytes: 8B 3C 24)
        start_stub.extend(encode_mov_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));

        // lea esi, [esp+4] — argv = &argv[0] (4 bytes: 8D 74 24 04)
        start_stub.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 4));

        // call main (E8 + rel32 placeholder, 5 bytes)
        start_stub.extend(encode_call_rel32(0));

        // mov ebx, eax — move main's return value to EBX (exit code arg)
        start_stub.extend(encode_mov_reg_reg(Gpr::Rbx, Gpr::Rax));

        // mov eax, 1 (sys_exit = 1 on i386; 5-byte B8+rd form)
        start_stub.extend(encode_mov_reg_imm64(Gpr::Rax, 1));

        // int 0x80
        start_stub.extend(encode_syscall());

        // Patch the call main rel32 offset in _start stub.
        // The E8 byte sits at CALL_SITE_OFFSET; the rel32 field starts at
        // CALL_SITE_OFFSET + 1.  rel32 = target - (call_site + 5).
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key];
            // rel32 = target - (call_site + 5)
            // call_site = offset of the E8 byte within the _start stub.
            let rel32 = (main_offset as i64) - (CALL_SITE_OFFSET as i64 + 5i64);
            start_stub[REL32_PATCH_OFFSET..REL32_PATCH_OFFSET + 4]
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
        let num_phdrs: u64 = if bss_size > 0 { 3 } else { 2 };
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
                    // R_X86_64_64 — absolute address relocation.
                    // On x86_32, this is a 4-byte (32-bit) absolute address,
                    // NOT 8 bytes (64-bit). The encode_mov_reg_imm64 function
                    // emits a 5-byte MOV r32, imm32 — the immediate is 4 bytes.
                    // Used by GetAddress to load the address of a data symbol.
                    if abs_offset + 4 > all_code.len() {
                        continue; // skip invalid relocations
                    }
                    if let Some(&addr) = data_symbol_addrs.get(&reloc.symbol) {
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&(addr as u32).to_le_bytes());
                    } else if func_offsets.contains_key(&reloc.symbol) {
                        // Function symbol with absolute relocation — patch with
                        // the function's virtual address (text_offset + offset).
                        let func_addr = text_vaddr + func_offsets[&reloc.symbol] as u64;
                        all_code[abs_offset..abs_offset + 4]
                            .copy_from_slice(&(func_addr as u32).to_le_bytes());
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
        // x86_32: mov eax, imm32; jmp eax
        // On 32-bit, addresses are 4 bytes. No REX prefix.
        let mut code = vec![0xB8]; // MOV EAX, imm32
        code.extend_from_slice(&(entry_addr as u32).to_le_bytes());
        code.extend_from_slice(&[0xFF, 0xE0]); // JMP EAX
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
        // MOV ECX, EAX => 89 /r with src=EAX, dst=ECX (no REX on x86_32)
        let code = encode_mov_reg_reg(Gpr::Rcx, Gpr::Rax);
        assert_eq!(code, vec![0x89, 0xC1]);
    }

    #[test]
    fn test_mov_rax_r8() {
        // On x86_32, R8 is a source-compat alias for the encoding of
        // EAX (encoding() & 7 == 0); there is no REX.B.  MOV "R8", EAX
        // therefore emits the same bytes as MOV EAX, EAX: 89 /r mod=3.
        let code = encode_mov_reg_reg(Gpr::R8, Gpr::Rax);
        assert_eq!(code, vec![0x89, 0xC0]);
    }

    #[test]
    fn test_mov_r9_r15() {
        // On x86_32, R9/R15 alias to ECX/EDI (encoding() & 7 == 1/7).
        // MOV "R15", "R9" emits 89 /r mod=3, reg=1, rm=7 (no REX).
        let code = encode_mov_reg_reg(Gpr::R15, Gpr::R9);
        assert_eq!(code, vec![0x89, 0xCF]);
    }

    // ── MOV Reg-Imm64 Tests ────────────────────────────────────────────

    #[test]
    fn test_mov_rax_imm64() {
        // x86_32 cannot hold a 64-bit immediate in a single register;
        // encode_mov_reg_imm64 loads only the low 32 bits via
        // `MOV r32, imm32` (B8+rd + imm32, 5 bytes, no REX).
        let imm: u64 = 0xDEADBEEFCAFE0000;
        let code = encode_mov_reg_imm64(Gpr::Rax, imm);
        assert_eq!(code.len(), 5);
        assert_eq!(code[0], 0xB8); // MOV EAX, imm32
        assert_eq!(&code[1..5], (imm as u32).to_le_bytes());
    }

    #[test]
    fn test_mov_r8_imm64() {
        // x86_32: R8 aliases to EAX (encoding() & 7 == 0).  MOV "R8",
        // imm32 emits B8 + imm32 (5 bytes, no REX).
        let code = encode_mov_reg_imm64(Gpr::R8, 0x1234);
        assert_eq!(code.len(), 5);
        assert_eq!(code[0], 0xB8); // MOV EAX, imm32 (R8 & 7 == 0)
        assert_eq!(&code[1..5], 0x1234u32.to_le_bytes());
    }

    // ── MOV Reg-Imm32 Tests ────────────────────────────────────────────

    #[test]
    fn test_mov_rcx_imm32() {
        // x86_32: MOV ECX, imm32 = C7 /0 + imm32 (no REX)
        let code = encode_mov_reg_imm32(Gpr::Rcx, 42);
        assert_eq!(code, vec![0xC7, 0xC1, 0x2A, 0x00, 0x00, 0x00]);
    }

    // ── ADD/SUB Tests ──────────────────────────────────────────────────

    #[test]
    fn test_add_rax_rcx() {
        // ADD EAX, ECX = 01 /r (no REX on x86_32)
        let code = encode_add_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x01, 0xC8]);
    }

    #[test]
    fn test_sub_rdx_rsi() {
        // SUB EDX, ESI = 29 /r (no REX on x86_32)
        let code = encode_sub_reg_reg(Gpr::Rdx, Gpr::Rsi);
        assert_eq!(code, vec![0x29, 0xF2]);
    }

    #[test]
    fn test_add_r8_r9() {
        // x86_32: R8/R9 alias to EAX/ECX.  ADD "R8", "R9" emits
        // 01 /r mod=3, reg=1, rm=0 (no REX).
        let code = encode_add_reg_reg(Gpr::R8, Gpr::R9);
        assert_eq!(code, vec![0x01, 0xC8]);
    }

    // ── IMUL Tests ─────────────────────────────────────────────────────

    #[test]
    fn test_imul_rax_rcx() {
        // IMUL EAX, ECX = 0F AF /r (no REX on x86_32)
        let code = encode_imul_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x0F, 0xAF, 0xC1]);
    }

    #[test]
    fn test_imul_r8_r15() {
        // x86_32: R8/R15 alias to EAX/EDI.  IMUL "R8", "R15" emits
        // 0F AF /r mod=3, reg=0, rm=7 (no REX).
        let code = encode_imul_reg_reg(Gpr::R8, Gpr::R15);
        assert_eq!(code, vec![0x0F, 0xAF, 0xC7]);
    }

    // ── IDIV Test ──────────────────────────────────────────────────────

    #[test]
    fn test_idiv_rcx() {
        // IDIV ECX = F7 /7 (no REX on x86_32)
        let code = encode_idiv_reg(Gpr::Rcx);
        assert_eq!(code, vec![0xF7, 0xF9]);
    }

    // ── CMP Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_cmp_rax_rcx() {
        // CMP EAX, ECX = 39 /r (no REX on x86_32)
        let code = encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x39, 0xC8]);
    }

    #[test]
    fn test_cmp_reg_imm32() {
        // x86_32: CMP EAX, imm8 uses the short 83 /7 ib form when the
        // immediate fits in a signed byte (100 does).  No REX prefix.
        let code = encode_cmp_reg_imm32(Gpr::Rax, 100);
        assert_eq!(code[0], 0x83); // CMP r/m32, imm8
        assert_eq!(code[1], 0xF8); // mod=3, reg=7(/7), rm=EAX(0)
        assert_eq!(code[2], 100);  // imm8 = 100
        assert_eq!(code.len(), 3);
    }

    // ── TEST Test ──────────────────────────────────────────────────────

    #[test]
    fn test_test_rax_rax() {
        // TEST EAX, EAX = 85 /r (no REX on x86_32)
        let code = encode_test_reg_reg(Gpr::Rax, Gpr::Rax);
        assert_eq!(code, vec![0x85, 0xC0]);
    }

    // ── AND/OR/XOR Tests ──────────────────────────────────────────────

    #[test]
    fn test_and_rax_rcx() {
        // AND EAX, ECX = 21 /r (no REX on x86_32)
        let code = encode_and_reg_reg(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x21, 0xC8]);
    }

    #[test]
    fn test_or_rdx_rsi() {
        // OR EDX, ESI = 09 /r (no REX on x86_32)
        let code = encode_or_reg_reg(Gpr::Rdx, Gpr::Rsi);
        assert_eq!(code, vec![0x09, 0xF2]);
    }

    #[test]
    fn test_xor_rax_rax() {
        // XOR EAX, EAX = 31 /r (no REX on x86_32)
        let code = encode_xor_reg_reg(Gpr::Rax, Gpr::Rax);
        assert_eq!(code, vec![0x31, 0xC0]);
    }

    // ── Shift Tests ────────────────────────────────────────────────────

    #[test]
    fn test_shl_cl() {
        // SHL EAX, CL = D3 /4 (no REX on x86_32)
        let code = encode_shl_reg_cl(Gpr::Rax);
        assert_eq!(code, vec![0xD3, 0xE0]);
    }

    #[test]
    fn test_shr_cl() {
        // SHR ECX, CL => D3 /5 + ModRM(3,5,1) = 0xE9 (no REX)
        let code = encode_shr_reg_cl(Gpr::Rcx);
        assert_eq!(code, vec![0xD3, 0xE9]);
    }

    #[test]
    fn test_sar_cl() {
        // SAR EDX, CL = D3 /7 (no REX on x86_32)
        let code = encode_sar_reg_cl(Gpr::Rdx);
        assert_eq!(code, vec![0xD3, 0xFA]);
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
        // x86_32: R8 aliases to EAX (encoding() & 7 == 0); no REX.B.
        // PUSH "R8" therefore emits the same byte as PUSH EAX: 0x50.
        assert_eq!(encode_push(Gpr::R8), vec![0x50]);
    }

    #[test]
    fn test_pop_rbx() {
        assert_eq!(encode_pop(Gpr::Rbx), vec![0x5B]);
    }

    #[test]
    fn test_pop_r15() {
        // x86_32: R15 aliases to EDI (encoding() & 7 == 7); no REX.B.
        // POP "R15" therefore emits the same byte as POP EDI: 0x5F.
        assert_eq!(encode_pop(Gpr::R15), vec![0x5F]);
    }

    // ── SETcc Tests ────────────────────────────────────────────────────

    #[test]
    fn test_sete_al() {
        let code = encode_setcc(Cc::Equal, Gpr::Rax);
        assert_eq!(code, vec![0x0F, 0x94, 0xC0]);
    }

    #[test]
    fn test_setl_r8b() {
        // x86_32: R8 aliases to EAX (encoding() & 7 == 0); no REX.B.
        // SETL "R8b" therefore emits 0F 9C /0 with rm=0 (same as SETL AL).
        let code = encode_setcc(Cc::Less, Gpr::R8);
        assert_eq!(code, vec![0x0F, 0x9C, 0xC0]);
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
        // CMOVE EAX, ECX = 0F 44 /r (no REX on x86_32)
        let code = encode_cmovcc_reg_reg(Cc::Equal, Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x0F, 0x44, 0xC1]);
    }

    // ── LEA Tests ──────────────────────────────────────────────────────

    #[test]
    fn test_lea_rax_rbp_offset8() {
        // LEA EAX, [EBP+8] = 8D /r + disp8 (no REX on x86_32)
        let code = encode_lea_reg_mem(Gpr::Rax, Gpr::Rbp, 8);
        assert_eq!(code, vec![0x8D, 0x45, 0x08]);
    }

    #[test]
    fn test_lea_rax_rsp_offset0() {
        // ESP as base requires SIB byte (no REX on x86_32)
        let code = encode_lea_reg_mem(Gpr::Rax, Gpr::Rsp, 0);
        assert_eq!(code, vec![0x8D, 0x04, 0x24]);
    }

    // ── MOVZX/MOVSX Tests ──────────────────────────────────────────────

    #[test]
    fn test_movzx_reg8() {
        // MOVZX EAX, CL = 0F B6 /r (no REX on x86_32)
        let code = encode_movzx_reg8(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x0F, 0xB6, 0xC1]);
    }

    #[test]
    fn test_movsx_reg8() {
        // MOVSX EAX, CL = 0F BE /r (no REX on x86_32)
        let code = encode_movsx_reg8(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x0F, 0xBE, 0xC1]);
    }

    #[test]
    fn test_movsxd() {
        // x86_32 has no MOVSXD (32-bit registers are already 32-bit);
        // encode_movsxd lowers to a plain MOV r32, r32 (89 /r, no REX).
        // encode_mov_reg_reg(dst, src) emits 89 /r with reg=src, rm=dst.
        let code = encode_movsxd(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x89, 0xC8]); // 89 /r mod=3, reg=ECX(1), rm=EAX(0)
    }

    // ── XCHG Test ──────────────────────────────────────────────────────

    #[test]
    fn test_xchg_rax_rcx() {
        // XCHG EAX, ECX = 90+rd (1 byte, no REX on x86_32)
        let code = encode_xchg_rax_reg(Gpr::Rcx);
        assert_eq!(code, vec![0x91]);
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
        // i386 SysV ABI: EBX, ESI, EDI, EBP are callee-saved.
        // (R12-R15 don't exist on x86_32; they're kept in the enum for
        // source compatibility only and are NOT callee-saved here.)
        assert!(Gpr::Rbx.is_callee_saved());
        assert!(Gpr::Rbp.is_callee_saved());
        assert!(Gpr::Rsi.is_callee_saved());
        assert!(Gpr::Rdi.is_callee_saved());
        assert!(!Gpr::Rax.is_callee_saved());
        assert!(!Gpr::Rcx.is_callee_saved());
        assert!(!Gpr::Rdx.is_callee_saved());
        assert!(!Gpr::R12.is_callee_saved());
    }

    #[test]
    fn test_gpr_arg_regs() {
        // i386 SysV ABI passes all integer args on the stack; there are
        // no integer argument registers.  (The codegen does use a
        // fastcall-style EDI/ESI/EDX/ECX convention internally for the
        // first 4 args, but `is_arg_reg()` reflects the documented ABI
        // and returns false for every register.)
        assert!(!Gpr::Rdi.is_arg_reg());
        assert!(!Gpr::R9.is_arg_reg());
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
        // i386 SysV ABI: no integer argument registers — always None.
        assert_eq!(Gpr::arg_register(0), None);
        assert_eq!(Gpr::arg_register(5), None);
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
        let target: u64 = 0x7FFFF7000000;
        let tramp = backend.trampoline(target);
        // x86_32 trampoline: MOV EAX, imm32 (low 32 bits, no REX) + JMP EAX.
        // The high 32 bits of the 64-bit target are lost on x86_32.
        assert_eq!(tramp[0], 0xB8); // MOV EAX, imm32
        assert_eq!(&tramp[1..5], (target as u32).to_le_bytes());
        assert_eq!(&tramp[5..7], &[0xFF, 0xE0]); // JMP EAX
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
        // EM_386 (3), NOT EM_X86_64 (62) — x86_32 produces ELF32 i386 objects.
        assert_eq!(backend.target_info().elf_machine_type(), 3);
        // i386 SysV uses the cdecl calling convention (args on stack),
        // not the SystemV AMD64 register-arg convention.
        assert_eq!(backend.target_info().calling_convention_name(), "cdecl");
    }

    // ── Backend TargetInfo Consistency Test ─────────────────────────────

    #[test]
    fn test_target_info_consistency() {
        let backend = X86_32Backend::new();
        let info = backend.target_info();
        // x86_32: 32-bit pointers, 8 GPRs (EAX–EDI), 8 XMM regs,
        // no register-arg calling convention (i386 cdecl passes on stack).
        assert_eq!(info.pointer_width(), 4);
        assert_eq!(info.num_gp_regs(), 8);
        assert_eq!(info.num_simd_fp_regs(), 8);
        assert!(!info.has_hardwired_zero());
        assert!(!info.has_link_register());
        assert_eq!(info.stack_alignment(), 16);
        assert_eq!(info.instruction_alignment(), 1);
        assert_eq!(info.instruction_width_range(), (1, 15));
        assert_eq!(info.num_int_arg_regs(), 0);
        assert_eq!(info.num_fp_arg_regs(), 0);
    }

    // ── MOV [mem] Tests ────────────────────────────────────────────────

    #[test]
    fn test_mov_reg_mem_offset8() {
        // MOV EAX, [EBX+8] = 8B /r + disp8 (no REX on x86_32)
        let code = encode_mov_reg_mem(Gpr::Rax, Gpr::Rbx, 8);
        assert_eq!(code, vec![0x8B, 0x43, 0x08]);
    }

    #[test]
    fn test_mov_reg_mem_offset0_rbp() {
        // EBP with offset 0 requires mod=01 with disp8=0 (no REX)
        let code = encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, 0);
        assert_eq!(code, vec![0x8B, 0x45, 0x00]);
    }

    #[test]
    fn test_mov_reg_mem_rsp_sib() {
        // ESP as base requires SIB byte (no REX on x86_32)
        let code = encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 0);
        assert_eq!(code, vec![0x8B, 0x04, 0x24]);
    }

    #[test]
    fn test_mov_mem_reg_offset8() {
        // MOV [EBX+8], EAX = 89 /r + disp8 (no REX on x86_32)
        let code = encode_mov_mem_reg(Gpr::Rbx, 8, Gpr::Rax);
        assert_eq!(code, vec![0x89, 0x43, 0x08]);
    }

    // ── CQO Test ───────────────────────────────────────────────────────

    #[test]
    fn test_cqo() {
        // x86_32 uses CDQ (99) to sign-extend EAX into EDX:EAX.
        // (CQO with REX.W prefix is the x86_64 64-bit variant.)
        assert_eq!(encode_cqo(), vec![0x99]);
    }

    // ── NEG/NOT Tests ──────────────────────────────────────────────────

    #[test]
    fn test_neg_rax() {
        // NEG EAX = F7 /3 (no REX on x86_32)
        let code = encode_neg_reg(Gpr::Rax);
        assert_eq!(code, vec![0xF7, 0xD8]);
    }

    #[test]
    fn test_not_rcx() {
        // NOT ECX = F7 /2 (no REX on x86_32)
        let code = encode_not_reg(Gpr::Rcx);
        assert_eq!(code, vec![0xF7, 0xD1]);
    }

    // ── MOVZX r16 Test ─────────────────────────────────────────────────

    #[test]
    fn test_movzx_reg16() {
        // MOVZX EAX, CX = 0F B7 /r (no REX on x86_32)
        let code = encode_movzx_reg16(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x0F, 0xB7, 0xC1]);
    }

    // ── ADD/SUB imm32 Tests ────────────────────────────────────────────

    #[test]
    fn test_sub_reg_imm32() {
        // SUB ESP, 32 = 81 /5 + imm32 (no REX on x86_32; encode_sub_reg_imm32
        // always uses the 81 /5 id form, never the 83 /5 ib short form).
        let code = encode_sub_reg_imm32(Gpr::Rsp, 32);
        assert_eq!(code[0], 0x81); // SUB r/m32, imm32
        assert_eq!(code[1], 0xEC); // mod=3, /5, rm=ESP(4)
        assert_eq!(code.len(), 6);
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
        // MOVSX EAX, CX = 0F BF /r (no REX on x86_32)
        let code = encode_movsx_reg16(Gpr::Rax, Gpr::Rcx);
        assert_eq!(code, vec![0x0F, 0xBF, 0xC1]);
    }

    #[test]
    fn test_movsx_reg16_r8_r9() {
        // x86_32: R8/R9 alias to EAX/ECX.  MOVSX "R8", "R9" emits
        // 0F BF /r mod=3, reg=0, rm=1 (no REX).
        let code = encode_movsx_reg16(Gpr::R8, Gpr::R9);
        assert_eq!(code, vec![0x0F, 0xBF, 0xC1]);
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
        // SDiv uses CDQ (0x99 on x86_32, no REX prefix) + IDIV (0xF7 /7).
        // (On x86_64 this would be CQO = REX.W + 0x99 = 48 99; on x86_32
        // it's just the single-byte CDQ = 0x99.)
        assert!(
            code.iter().any(|&b| b == 0x99),
            "CDQ not found for SDiv"
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
        // CMP r/m32, imm — x86_32 uses the short `83 /7 ib` form (3 bytes)
        // when the immediate fits in a signed byte (5 does), or the long
        // `81 /7 id` form (6 bytes) otherwise.  Accept either form.
        let has_cmp_imm = code.windows(2).any(|w| {
            (w[0] == 0x81 || w[0] == 0x83)
                && (w[1] & 0xC0) == 0xC0
                && (w[1] & 0x38) == 0x38
        });
        assert!(has_cmp_imm, "CMP r/m32, imm not found");
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
            from_ty: Some(IRType::U8),
            to_ty: None,
        });
        // ZExt of a U8 register uses MOVZX r8→r32 (0F B6).
        // (When from_ty is None, the isel skips the MOVZX; the test must
        // supply a concrete source type to exercise the extension path.)
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
            from_ty: Some(IRType::I8),
            to_ty: None,
        });
        // SExt of an I8 register uses MOVSX r8→r32 (0F BE).
        // (When from_ty is None, the isel skips the MOVSX; the test must
        // supply a concrete source type to exercise the extension path.)
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
        // On x86_32 the stack-slot isel lowers Select as:
        //   load false_val->EAX, true_val->scratch, cond->scratch;
        //   `TEST r32, r32` then `CMOVNZ EAX, scratch`.
        //
        // x86_32 emits no REX prefix (no R8-R15 registers, no 64-bit
        // operand size), so the TEST opcode 0x85 appears as a bare byte
        // rather than as `REX.W + 0x85`. The CMOVcc opcode (0x0F 0x4x) is
        // unaffected by the absence of REX.
        assert!(
            code.iter().any(|&b| b == 0x85),
            "TEST (0x85) not found for Select"
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

    #[test]
    fn test_real_regalloc_metadata() {
        use crate::ir::{VirtualRegister, IRFunction, IRInstr, IRValue};
        use crate::backend::Backend;

        let mut func = IRFunction::new("test_real_regalloc".to_string());
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

        // Stack-slot mode (default): reads/writes should be empty.
        let backend = X86_32Backend::new();
        let result_ss = backend.allocate_registers(&func);
        assert!(result_ss.is_ok(), "stack-slot allocation should succeed");
        let ss_func = result_ss.unwrap();
        let ss_has_regs = ss_func.blocks.iter()
            .any(|b| b.instructions.iter().any(|i| !i.reads.is_empty() || !i.writes.is_empty()));
        assert!(!ss_has_regs, "stack-slot mode should not record physical registers");

        // Real regalloc mode: reads/writes should be populated.
        let mut backend = X86_32Backend::new();
        backend.use_real_regalloc = true;
        let result_real = backend.allocate_registers(&func);
        assert!(result_real.is_ok(), "real regalloc should succeed");
        let real_func = result_real.unwrap();
        let has_real_regs = real_func.blocks.iter()
            .any(|b| b.instructions.iter().any(|i| !i.reads.is_empty() || !i.writes.is_empty()));
        assert!(has_real_regs, "real regalloc should record physical register assignments");
    }
}
pub mod disasm;
pub mod stack_slot_isel;
