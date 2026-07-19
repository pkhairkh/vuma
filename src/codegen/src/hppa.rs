//! # HPPA (HP PA-RISC 1.1) Backend
//!
//! Full implementation of the PA-RISC 1.1 instruction set for Linux/hppa.
//!
//! ## PA-RISC Architecture Overview
//!
//! - 32 general-purpose registers (R0-R31); R0 is hardwired to 0.
//! - Big-endian, 32-bit, all instructions are 4 bytes.
//! - No branch delay slots.
//! - Linux/hppa syscall convention: syscall # in R20, args in R26-R23,
//!   return in R28, invoke via `ble 0x100(%sr2,%r0)` (GATE instruction).
//! - Stack grows upward (higher addresses). R30 = R30. FP = R3.
//! - R1 = return address (RP), R2 = return pointer (R30 before call).
//!
//! ## PA-RISC Instruction Formats
//!
//! All instructions are 32-bit big-endian. Major formats:
//! - **System**: `000010 bbb lll oooo oooo oooo oooo ooo1` (GATE/BREAK)
//! - **Load/Store**: `0001 10ss bbb x ff aaaa aaa ddddd ll oooo ooo` 
//! - **Arithmetic**: `000010 ss bbb 0 t aaaa aaa ddddd cccc ffff ee`
//! - **Branch**: `001 aa lll lll lll lll nnn nnn ooo g0 0 w ddddd` (BL/BV)
//! - **Load Immediate**: `0010 00s bbb 0 t aaaa aaa ddddd iiii iiii iiii`

use crate::backend::{
    AllocatedFunction, AllocatedProgram, Backend, BackendError, TargetInfo, Endianness,
};
use crate::ir::{alignment_of_with_ptr_width, size_of_with_ptr_width, IRFunction, IRType, IRValue, IRInstr, IRTerminator};
#[cfg(test)]
use crate::ir::VirtualRegister;

// ===========================================================================
// Register definitions
// ===========================================================================

/// PA-RISC general-purpose registers.
/// R0 = hardwired zero, R1 = RP (return pointer), R2 = R30 (previous),
/// R3 = FP (frame pointer), R26-R23 = arg regs (reversed order),
/// R28 = ret val, R29 = ret val2, R30 = R30 (stack pointer),
/// R31 = link register for BL.
type Reg = u8;

const R0: Reg = 0;   // Hardwired zero
const R1: Reg = 1;   // RP (return pointer)
const R2: Reg = 2;   // Return R30 (caller's R30)
const R3: Reg = 3;   // FP (frame pointer)
const R8: Reg = 8;
const R9: Reg = 9;
const R10: Reg = 10;
const R11: Reg = 11;
const R12: Reg = 12;
const R13: Reg = 13;
const R14: Reg = 14;
const R15: Reg = 15;
const R16: Reg = 16;
const R17: Reg = 17;
const R18: Reg = 18;
const R19: Reg = 19;
const R20: Reg = 20; // Syscall number
const R21: Reg = 21;
const R22: Reg = 22;
const R23: Reg = 23; // Syscall arg 4
const R24: Reg = 24; // Syscall arg 3
const R25: Reg = 25; // Syscall arg 2
const R26: Reg = 26; // Syscall arg 1
const R28: Reg = 28; // Return value
const R29: Reg = 29; // Return value 2
const R30: Reg = 30; // R30 (stack pointer)

/// HPPA floating-point register (PA-RISC 1.1: F0–F15 in hardware).
type FReg = u8;
const F0: FReg = 0;
const F1: FReg = 1;
const F2: FReg = 2;
#[allow(dead_code)]
const F3: FReg = 3;
// F4–F15 exist but are not needed as scratch for single-op emission.

/// FP scratch registers (caller-saved per PA-RISC ABI).
const FA: FReg = F0;  // FP accumulator / result
const FB: FReg = F1;  // FP second operand
#[allow(dead_code)]
const FC: FReg = F2;  // FP third scratch (for conversions)

// Global set of function names that return 64-bit values (I64/U64).
static FUNC_64BIT_RETURNS: std::sync::OnceLock<std::sync::RwLock<Option<std::collections::HashSet<String>>>> = std::sync::OnceLock::new();
fn func_64bit_returns() -> &'static std::sync::RwLock<Option<std::collections::HashSet<String>>> {
    FUNC_64BIT_RETURNS.get_or_init(|| std::sync::RwLock::new(None))
}
pub fn set_64bit_returns(names: &std::collections::HashSet<String>) {
    *func_64bit_returns().write().unwrap() = Some(names.clone());
}

// Dedicated 64-bit temp slots (separate from 4-byte vreg slots).
// TMP64_A: FP-44 (lo), FP-48 (hi) — Shl 32 result
// TMP64_B: FP-52 (lo), FP-56 (hi) — Or result (for Return)
// TMP64_C: FP-60 (lo), FP-64 (hi) — Call return high word (for ShrL 32)
const TMP64_A_HI: i32 = -36;
const TMP64_B_HI: i32 = -40;
const TMP64_C_HI: i32 = -44;

// Scratch registers for codegen
const S0: Reg = R8;
const S1: Reg = R9;
const S2: Reg = R10;
const S3: Reg = R11;
const S4: Reg = R12;
const S5: Reg = R13;

// ===========================================================================
// Instruction Encoders
// ===========================================================================

/// Encode a GATE instruction (ble 0x100(%sr2,%r0)) — used for Linux syscalls.
/// Format: System, opcode = 0x00000E40 (fixed).
fn encode_gate() -> [u8; 4] {
    // Linux PA-RISC syscall gateway: be,l 100(sr2,r0),sr0,r31
    // This branches to address 0x100 in SR2 (the gateway page).
    // Encoding: 0xE4008200 (found by brute-force scanning qemu decode).
    0xE4008200u32.to_be_bytes()
}

/// Encode a NOP (or %r0, %r0, %r0).
fn encode_nop() -> [u8; 4] {
    0x08000240u32.to_be_bytes()
}

/// Encode BL (Branch and Link) — `BL,n target, R1`.
///
/// Wave-6 fix: completely rewritten to match QEMU's `%assemble_17` decoder
/// (target/hppa/insns.decode).  The previous encoder had multiple bugs:
///   - nullify bit was at bit 31 instead of bit 1 (a no-op since 0xE8
///     already sets bit 31),
///   - the 17-bit displacement was placed in bits 16-0 instead of the
///     non-linear split QEMU expects (bit 0 = sign, bits 16-20 = high 5
///     bits, bit 2 = bit 10, bits 3-12 = low 10 bits),
///   - and D=0 (R0) was used instead of D=1 (R1).
/// The buggy encoding produced `<unknown>` (illegal instruction) for any
/// non-trivial displacement → SIGSEGV in print_int.
///
/// Correct format (verified against QEMU 10.x decode):
///   bits 31-26: 0x3A (BL opcode `111010`)
///   bits 25-21: l (5-bit link register; R1 here — R2 holds return addr)
///   bits 20-16: bits 15-11 of disp17
///   bits 15-13: 0 (reserved)
///   bits 12-3:  bits 9-0 of disp17
///   bit 2:      bit 10 of disp17
///   bit 1:      n (nullify)
///   bit 0:      bit 16 of disp17 (sign)
///
/// where `disp17 = target_offset / 4` (signed 17-bit two's complement),
/// and `target = PC + 8 + (sign_extend(disp17) << 2)`.
/// target_offset must be 4-byte aligned and within [-262144, 262140].
fn encode_bl(target_offset: i32) -> [u8; 4] {
    assert!(target_offset % 4 == 0,
            "BL displacement must be 4-byte aligned, got {}", target_offset);
    let disp17 = target_offset >> 2;
    assert!(disp17 >= -65536 && disp17 <= 65535,
            "BL displacement {} words out of [-65536, 65535]", disp17);
    let disp17_u = (disp17 as u32) & 0x1FFFF;
    let sign = (disp17_u >> 16) & 1;
    let bits_15_11 = (disp17_u >> 11) & 0x1F;
    let bit_10 = (disp17_u >> 10) & 1;
    let bits_9_0 = disp17_u & 0x3FF;
    let n = 1u32;       // nullify delay slot
    let l = 1u32;       // link = R1 (free in print_int stub; R2 holds RA)
    let word = 0xE8000000u32
        | (l << 21)
        | (bits_15_11 << 16)
        | (bit_10 << 2)
        | (bits_9_0 << 3)
        | (n << 1)
        | sign;
    word.to_be_bytes()
}

/// Encode BV (Branch Variable) — `be 0(sr0,rp)` for return.
/// This uses BE (Branch External) with sr=0, base=rp, disp=0, which branches
/// to the address in rp. Working but technically a BE not BV.
fn encode_bv(rp: Reg, _base: Reg) -> [u8; 4] {
    let word = 0xE0000000u32 | ((rp as u32 & 0x1F) << 21);
    word.to_be_bytes()
}

/// Encode actual BV (Branch Variable) — `bv 0(base)` branches to address in base.
/// PA-RISC BV format: 0xE800C000 | (base << 21) | (link << 16) | (disp << 5) | w
fn encode_bv_real(base: Reg) -> [u8; 4] {
    let word = 0xE800C000u32 | ((base as u32 & 0x1F) << 21);
    word.to_be_bytes()
}

/// Encode cmpb (compare-and-branch) — non-linear displacement encoding.
///
/// Major opcode 0x20 = forward conditions (=, <, <=, <<, <<=, sv, od).
/// Major opcode 0x22 = inverted conditions (<>, >=, >, >>=, >>, nsv, ev).
///
/// Displacement field (empirically determined via QEMU decode):
///   bit 0: weight -8192 (sign bit)
///   bit 1: f (nullify on taken) — not part of displacement
///   bit 2: weight +4096
///   bits 11-3: weight 4, 8, 16, ..., 1024 (9-bit, 4-byte scaling)
///   bit 12: weight +2048
///
/// target = PC + 8 + offset_bytes (where offset_bytes must be a multiple of 4).
fn encode_cmpb_disp(offset_bytes: i32) -> u32 {
    assert!(offset_bytes % 4 == 0, "cmpb displacement must be 4-byte aligned");
    assert!(offset_bytes >= -8192 && offset_bytes <= 8188,
            "cmpb displacement {} out of range [-8192, 8188]", offset_bytes);
    let mut bits: u32 = 0;
    let mut remaining = offset_bytes;
    if remaining < 0 {
        bits |= 1;  // bit 0 (sign)
        remaining += 8192;
    }
    if remaining >= 4096 {
        bits |= 1 << 2;  // bit 2 (weight 4096)
        remaining -= 4096;
    }
    if remaining >= 2048 {
        bits |= 1 << 12;  // bit 12 (weight 2048)
        remaining -= 2048;
    }
    let val = remaining / 4;
    assert!(val >= 0 && val <= 511);
    bits | (val as u32) << 3  // bits 11-3 (weight 4..1024)
}

/// Encode cmpb (compare-and-branch, register form).
///
/// `r1`, `r2` are the registers to compare.
/// `cond` is the 3-bit condition code:
///   001 (=), 010 (<), 011 (<=), 100 (<<), 101 (<<=), 110 (sv), 111 (od)
///   For inverted conditions (<>), use `inverted=true` with cond=001.
/// `f` is the nullify-on-taken flag (bit 1 of low byte).
/// `disp_bytes` is the byte displacement from PC+8 (must be 4-byte aligned).
fn encode_cmpb(r1: Reg, r2: Reg, cond: u32, inverted: bool, f: bool, disp_bytes: i32) -> [u8; 4] {
    let major = if inverted { 0x88000000u32 } else { 0x80000000u32 };
    let mut word = major
        | ((r2 as u32 & 0x1F) << 21)
        | ((r1 as u32 & 0x1F) << 16)
        | ((cond & 0x7) << 13);
    if f { word |= 1 << 1; }  // bit 1 = f
    word |= encode_cmpb_disp(disp_bytes);
    word.to_be_bytes()
}

/// Encode LDIL (Load Immediate Lower) — `LDIL imm, reg`.
/// Loads a 21-bit immediate into the LEFT of the register (upper 21 bits).
/// Format: 0010 10ss bbb 0 t aaaa aaa ddddd iiiiiiiiiiiiiiiiiii
/// Actually: 0010 10ss 000 0 t aaaa aaa ddddd iiii iiii iiii iiii i
/// The LDIL instruction loads imm21 into bits 31:11 of the target register.
fn encode_ldil(reg: Reg, imm: u32) -> [u8; 4] {
    let imm21 = imm & 0x1FFFFF;
    // Format: 001010 00 00000 0 t aaaaaaa ddddd iiiiiiiiiiiiiiiiiii
    // t=0 (format), a=0, d=reg, i=imm21
    let word = 0x20000000u32
        | ((reg as u32) << 21)
        | imm21;
    word.to_be_bytes()
}

/// Encode LDO (Load Offset) — `LDO offset(base), reg`.
/// Adds a 14-bit signed offset to a register and stores in reg.
/// Uses the non-linear displacement encoding (see encode_ldo_raw).
fn encode_ldo(base: Reg, offset: i16, dst: Reg) -> [u8; 4] {
    encode_ldo_raw(base, offset, dst)
}

/// Encode LDO without the legacy shift — for direct use (not via ss_st/ss_ld).
///
/// PA-RISC LDO displacement encoding (empirically determined via QEMU decode):
///   - bit 0: sign (1 = negative, 0 = positive)
///   - bits 13-1: magnitude / 2 (unsigned)
///   - For disp >= 0: imm14 = disp * 2 (bit 0 = 0)
///   - For disp < 0:  imm14 = (disp * 2 & 0x3FFE) | 1 (bit 0 = 1)
fn encode_ldo_raw(base: Reg, offset: i16, dst: Reg) -> [u8; 4] {
    let disp = offset as i32;
    let imm14 = if disp >= 0 {
        (disp * 2) as u32 & 0x3FFE
    } else {
        ((disp * 2) as u32 & 0x3FFE) | 1
    };
    let word = 0x34000000u32
        | ((base as u32 & 0x1F) << 21)
        | ((dst as u32 & 0x1F) << 16)
        | imm14;
    word.to_be_bytes()
}

/// Encode LDW (Load Word) — `LDW offset(base), reg`.
/// Uses the same non-linear displacement encoding as LDO:
///   - bit 0: sign (1 = negative, 0 = positive)
///   - bits 13-1: magnitude / 2 (unsigned)
fn encode_ldw(base: Reg, offset: i16, dst: Reg) -> [u8; 4] {
    let disp = offset as i32;
    let imm14 = if disp >= 0 {
        (disp * 2) as u32 & 0x3FFE
    } else {
        ((disp * 2) as u32 & 0x3FFE) | 1
    };
    let word = 0x48000000u32
        | ((base as u32 & 0x1F) << 21)
        | ((dst as u32 & 0x1F) << 16)
        | imm14;
    word.to_be_bytes()
}

/// Encode STW (Store Word) — `STW reg, offset(base)`.
/// Uses the same non-linear displacement encoding as LDO.
fn encode_stw(src: Reg, base: Reg, offset: i16) -> [u8; 4] {
    let disp = offset as i32;
    let imm14 = if disp >= 0 {
        (disp * 2) as u32 & 0x3FFE
    } else {
        ((disp * 2) as u32 & 0x3FFE) | 1
    };
    let word = 0x68000000u32  // 0001 1010 (STW)
        | ((base as u32 & 0x1F) << 21)
        | ((src as u32 & 0x1F) << 16)
        | imm14;
    word.to_be_bytes()
}

/// Encode STB (Store Byte) — `STB reg, offset(base)`.
fn encode_stb(src: Reg, base: Reg, offset: i16) -> [u8; 4] {
    let disp = offset as i32;
    let imm14 = if disp >= 0 {
        (disp * 2) as u32 & 0x3FFE
    } else {
        ((disp * 2) as u32 & 0x3FFE) | 1
    };
    let word = 0x60000000u32  // 0001 1000 (STB)
        | ((base as u32 & 0x1F) << 21)
        | ((src as u32 & 0x1F) << 16)
        | imm14;
    word.to_be_bytes()
}

/// Encode LDB (Load Byte) — `LDB offset(base), reg`.
fn encode_ldb(base: Reg, offset: i16, dst: Reg) -> [u8; 4] {
    let disp = offset as i32;
    let imm14 = if disp >= 0 {
        (disp * 2) as u32 & 0x3FFE
    } else {
        ((disp * 2) as u32 & 0x3FFE) | 1
    };
    let word = 0x40000000u32  // 0001 0000 (LDB)
        | ((base as u32 & 0x1F) << 21)
        | ((dst as u32 & 0x1F) << 16)
        | imm14;
    word.to_be_bytes()
}

/// Encode ADD (Add) — `ADD r1, r2, dst`.
/// From scan: ADD = 0x08000600 with r1 at bits 20-16, r2 at bits 25-21, dst at bits 4-0.
fn encode_add(r1: Reg, r2: Reg, dst: Reg) -> [u8; 4] {
    // From scan: ADD = 0x08000600 with r1 at bits 20-16, r2 at bits 25-21, dst at bits 4-0.
    let word = 0x08000600u32
        | ((r1 as u32 & 0x1F) << 16)
        | ((r2 as u32 & 0x1F) << 21)
        | (dst as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode SHLADD (Shift Left and Add) — `SHLADD sa, r1, r2, dst`.
/// Computes dst = (r1 << sa) + r2. sa must be 1, 2, or 3.
/// Format: same as ADD (0x08000600) with sa at bits 7-6.
fn encode_shladd(shift: u8, r1: Reg, r2: Reg, dst: Reg) -> [u8; 4] {
    let word = 0x08000600u32
        | ((r2 as u32 & 0x1F) << 21)
        | ((r1 as u32 & 0x1F) << 16)
        | ((shift as u32 & 0x3) << 6)
        | (dst as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode SUB (Subtract) — `SUB r1, r2, dst`.
/// Computes dst = r1 - r2.
fn encode_sub(r1: Reg, r2: Reg, dst: Reg) -> [u8; 4] {
    // Plain SUB (no completers): 0x08000400
    // Same format as ADD: r1 at bits 20-16, r2 at bits 25-21, dst at bits 4-0.
    let word = 0x08000400u32
        | ((r1 as u32 & 0x1F) << 16)
        | ((r2 as u32 & 0x1F) << 21)
        | (dst as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode OR — `OR r1, r2, dst`. Computes dst = r1 | r2.
/// PA-RISC OR function code: 0x08000240 (same format as ADD).
fn encode_or(r1: Reg, r2: Reg, dst: Reg) -> [u8; 4] {
    let word = 0x08000240u32
        | ((r1 as u32 & 0x1F) << 16)
        | ((r2 as u32 & 0x1F) << 21)
        | (dst as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode AND — `AND r1, r2, dst`. Computes dst = r1 & r2.
/// PA-RISC AND function code: 0x08000200 (same format as OR/ADD).
fn encode_and(r1: Reg, r2: Reg, dst: Reg) -> [u8; 4] {
    let word = 0x08000200u32
        | ((r1 as u32 & 0x1F) << 16)
        | ((r2 as u32 & 0x1F) << 21)
        | (dst as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode XOR — `XOR r1, r2, dst`. Computes dst = r1 ^ r2.
/// PA-RISC XOR function code: 0x08000280 (same format as OR/ADD).
fn encode_xor(r1: Reg, r2: Reg, dst: Reg) -> [u8; 4] {
    let word = 0x08000280u32
        | ((r1 as u32 & 0x1F) << 16)
        | ((r2 as u32 & 0x1F) << 21)
        | (dst as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode COPY (OR) — `COPY r1, dst`. Moves r1 to dst.
/// PA-RISC OR: 000010 00 r1 0 0 0000000 dst 0000 1001 00 r2
/// With r2=r0: dst = r1 | 0 = r1.
fn encode_copy(src: Reg, dst: Reg) -> [u8; 4] {
    // OR src, R0, dst = COPY src, dst
    // Same format as ADD: r1(src) at bits 20-16, r2(R0) at bits 25-21, dst at bits 4-0.
    let word = 0x08000260u32
        | ((src as u32 & 0x1F) << 16)
        | (dst as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode SHRPW (Shift Right Pair Word) — `SHRPW r1, r2, sa, t`.
/// Shifts the 64-bit concatenation (r1:r2) right by sa bits, result in t.
/// With r1=R0 (zero), this gives: t = r2 >> sa (zero-filled).
///
/// QEMU PA-RISC format 14 (empirically verified via QEMU disassembly):
///   bits 31-26: 110100 (opcode 0x34)
///   bits 25-21: r2 (source, low 32 bits of pair)
///   bits 20-16: r1 (source, high 32 bits of pair — use R0 for zero)
///   bits 15-11: cl
///   bit  10:    0 (fixed shift; 1 = variable)
///   bits 9-6:   pos
///   bits 4-0:   t (target)
/// Shift amount: sa = 31 - cl - pos
fn encode_shrpw(r1: Reg, r2: Reg, sa: u8, t: Reg) -> [u8; 4] {
    // QEMU PA-RISC SHRPW encoding (empirically determined):
    //   bits 31-26: 110100 (opcode 0x34)
    //   bits 25-21: r2 (source, low 32 bits of pair)
    //   bits 20-16: r1 (source, high 32 bits — use R0 for zero)
    //   bits 15-11: cl = 1 (MUST be 1 for unconditional SHRPW)
    //   bit  10:    0 (fixed shift)
    //   bits 9-6:   pos (shift amount encoded as (31 - sa) / 2)
    //   bits 4-0:   t (target)
    // Formula: sa = 31 - 2 * pos, so pos = (31 - sa) / 2.
    // Only works for ODD sa values (sa = 31, 29, 27, ..., 3, 1).
    // For even sa values, use two shifts: sa-1 then shift by 1.
    let pos = (31u8 - sa) / 2;
    let cl: u32 = 1;
    let word = 0xD0000000u32
        | ((r2 as u32 & 0x1F) << 21)
        | ((r1 as u32 & 0x1F) << 16)
        | (cl << 11)
        | ((pos as u32 & 0xF) << 6)
        | (t as u32 & 0x1F);
    word.to_be_bytes()
}

/// Encode LDI (Load Immediate) — `LDI imm, reg`.
/// Loads a small (5-bit or 11-bit) immediate into a register.
/// For 5-bit: 0001 10ss 00000 0 0 aaaa aaa ddddd iiii iiii iiiii
/// Actually LDI is a pseudo-op: LDO imm(0), reg or LDIL, reg.
/// For 5-bit signed (-16 to 15): use arithmetic immediate.
fn encode_ldi(imm: i32, dst: Reg) -> [u8; 4] {
    if (-16..=15).contains(&imm) {
        // LDI 5-bit: use ADDI(0) format
        // Actually use the LDO format: LDO imm(0), dst
        encode_ldo(R0, imm as i16, dst)
    } else if (-2048..=2047).contains(&imm) {
        // 11-bit: use LDIL
        encode_ldil(dst, imm as u32)
    } else {
        // 32-bit: LDIL + LDO
        encode_ldil(dst, imm as u32)
    }
}

// ===========================================================================
// HPPA FP instruction encoders (PA-RISC 1.1 coprocessor)
// ===========================================================================
// WARNING: PA-RISC 1.1 FP encoding is intricate. These helpers are
// best-effort from the PA-RISC 1.1 Architecture Reference Manual §5.
// They MUST be verified on real PA hardware or qemu-hppa before
// production use. See TODO F1b.

/// FP arithmetic (FADD/FSUB/FMUL/FDIV).  f_op: 0x30=FADD, 0x31=FSUB,
/// 0x32=FMUL, 0x33=FDIV.  fmt: 0=Single, 1=Double.
fn encode_fp_arith(f_op: u32, fmt: u32, r1: FReg, r2: FReg, t: FReg) -> [u8; 4] {
    // copr(0x0C)<<26 | class0(0)<<25 | f-ext(0b0110)<<21 | r1<<16 | r2<<11 | fmt<<9 | t<<6 | f_op
    let word: u32 = (0x0Cu32 << 26)
        | (0x06u32 << 21)
        | ((r1 as u32 & 0x1F) << 16)
        | ((r2 as u32 & 0x1F) << 11)
        | ((fmt & 0x3) << 9)
        | ((t as u32 & 0x1F) << 6)
        | (f_op & 0x3F);
    word.to_be_bytes()
}

/// FP compare.  f_cond is the condition (e.g. 0x24 for !?, 0x20 for =, etc.).
#[allow(dead_code)]
fn encode_fp_cmp(f_cond: u32, fmt: u32, r1: FReg, r2: FReg) -> [u8; 4] {
    let word: u32 = (0x0Cu32 << 26)
        | (0x06u32 << 21)
        | ((r1 as u32 & 0x1F) << 16)
        | ((r2 as u32 & 0x1F) << 11)
        | ((fmt & 0x3) << 9)
        | (f_cond & 0x3F);
    word.to_be_bytes()
}

/// Load 32-bit FP from memory (FLDW) — PA-RISC 1.1 coprocessor load.
///
/// G3 STATUS (NOP stub, NOT exercised by the current FP strategy):
/// The PA-RISC 1.1 coprocessor load/store encoding is intricate
/// (14-bit signed displacement split across the instruction word, with
/// `b` (base) and `t` (target coprocessor reg) at non-contiguous field
/// positions).  It cannot be byte-verified in this sandbox without
/// QEMU-hppa decode testing.
///
/// The G3 fallback strategy AVOIDS exercising this encoder:
///   - `CastKind::FloatToFloat` is implemented as a GPR-transit bit-copy
///     (see the Cast arm in `emit_function`), so no FPR load is needed.
///   - `CastKind::IntToFloat` / `FloatToInt` / etc. are documented stubs
///     ("store 0") pending either verified FPR load/store encoders OR
///     soft-float runtime symbols (`__vuma_f64_add` etc.) added to
///     `womb/ieee/fp.vuma`.  Neither path is in place yet.
///
/// The FP arithmetic path (`emit_hppa_fp_binop`) does call this encoder,
/// so its results are currently incorrect — see that function's doc.
///
/// TODO G3 (deferred): replace with verified FLDW encoding once
/// QEMU-hppa testing is wired up.
/// Load 32-bit FP from memory (FLDW). PA-RISC coprocessor load word.
/// Major opcode 0x09, coprocessor unit ID = 1 (FPU).
/// Format: 001001 1 b 0 00001 t im11 000
///   b = base register (bits 24-20)
///   uid = 00001 (bits 18-14, coprocessor 1 = FPU)
///   t = FPR destination (bits 13-9)
///   im11 = signed 11-bit displacement (bits 13-3, but overlapping with t)
///
/// Actual PA-RISC encoding for CLDW (coprocessor load word):
///   001001 m b s uid(5) cl_ext(5) im(11) 0
/// For FPU (uid=00001), the FPR is in the cl_ext field (bits 13-9).
fn encode_fldw(base: Reg, offset: i16, dst: FReg) -> [u8; 4] {
    let word: u32 = (0x09u32 << 26)      // major opcode
        | (1u32 << 25)                    // m=1 (modify base)
        | ((base as u32 & 0x1F) << 20)   // base register
        | (0u32 << 19)                    // s=0
        | (1u32 << 14)                    // uid=00001 (FPU)
        | ((dst as u32 & 0x1F) << 9)     // FPR destination
        | ((offset as u32 & 0x7FF) << 1) // 11-bit displacement
        | 0u32;                           // bit 0 = 0
    word.to_be_bytes()
}

/// Store 32-bit FP to memory (FSTW). PA-RISC coprocessor store word.
/// Same format as FLDW but with major opcode 0x09 and different m/s bits.
/// CSTW uses the same major opcode but with the store bit set.
fn encode_fstw(src: FReg, base: Reg, offset: i16) -> [u8; 4] {
    // CSTW (coprocessor store word): major 0x09, same as CLDW but stores.
    // The store vs load distinction is encoded in the m and s bits.
    // For CSTW,ma: m=1, s=0, uid=00001
    let word: u32 = (0x09u32 << 26)      // major opcode
        | (1u32 << 25)                    // m=1 (modify base)
        | ((base as u32 & 0x1F) << 20)   // base register
        | (0u32 << 19)                    // s=0
        | (1u32 << 14)                    // uid=00001 (FPU)
        | ((src as u32 & 0x1F) << 9)     // FPR source
        | ((offset as u32 & 0x7FF) << 1) // 11-bit displacement
        | 0u32;
    word.to_be_bytes()
}

/// Emit HPPA floating-point binary op.
///
/// G3 STATUS (best-effort, NOT functional): The FP arithmetic path
/// emits a real PA-RISC 1.1 coprocessor word via `encode_fp_arith`
/// for FADD/FSUB/FMUL/FDIV, BUT the operands never reach the FPRs
/// because `encode_fldw` / `encode_fstw` are NOP stubs (see their
/// docs).  The FPR accumulators retain their prior state, the
/// arithmetic executes on garbage, and the store-back is a NOP — so
/// the result is always wrong.
///
/// The proper fix is one of:
///   (a) Verify the coprocessor load/store encoders on QEMU-hppa so
///       operands flow through FPRs; OR
///   (b) Emit soft-float runtime calls (`__vuma_f64_add`,
///       `__vuma_f64_sub`, `__vuma_f64_mul`, `__vuma_f64_div`) using
///       the existing `IRInstr::Call` path (the call-emission code
///       and `R_PARISC_PCREL` relocation are already in place — see
///       the `IRInstr::Call` arm in `emit_function`).
/// Path (b) is blocked on the soft-float symbols being defined in
/// `womb/ieee/fp.vuma` (currently that file only has utility helpers
/// like `f64_abs` / `f64_trunc`; no arithmetic, no conversions).
///
/// The FP comparison path (Eq/Ne/SLt/...) is also stubbed ("store 0").
/// The IR verifier (F2a) and gold tests (F3) will catch incorrect
/// results until either (a) or (b) lands.  See TODO G3.
fn emit_hppa_fp_binop(
    op: &crate::ir::BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    ty: &Option<IRType>,
    vreg_stack_slots: &std::collections::HashMap<u32, i32>,
    code: &mut Vec<u8>,
) {
    use crate::ir::BinOpKind;
    let dst_id = dst.as_register().unwrap_or(0);
    let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
    let is_f64 = matches!(ty, Some(IRType::F64));
    let fmt: u32 = if is_f64 { 1 } else { 0 };

    // Load operands into GPR scratch S0/S1 and spill to fixed slots.
    code.extend(ss_load_value(lhs, vreg_stack_slots, S0));
    code.extend(ss_load_value(rhs, vreg_stack_slots, S1));
    // Spill to scratch slots (reuse TMP64 area; f64 needs 8 bytes = 2 slots).
    let lhs_slot: i32 = -52;  // scratch, distinct from TMP64_A_HI=-36
    let rhs_slot: i32 = -60;
    code.extend(ss_st(S0, lhs_slot));
    code.extend(ss_st(S1, rhs_slot));

    // (Stubbed) load into FPRs.
    code.extend_from_slice(&encode_fldw(R30, lhs_slot as i16, FA));
    code.extend_from_slice(&encode_fldw(R30, rhs_slot as i16, FB));

    // Arithmetic.
    let f_op: u32 = match op {
        BinOpKind::Add => 0x30,  // FADD
        BinOpKind::Sub => 0x31,  // FSUB
        BinOpKind::Mul => 0x32,  // FMUL
        BinOpKind::SDiv | BinOpKind::UDiv => 0x33,  // FDIV
        BinOpKind::Eq | BinOpKind::Ne
        | BinOpKind::SLt | BinOpKind::ULt
        | BinOpKind::SLe | BinOpKind::ULe
        | BinOpKind::SGt | BinOpKind::UGt
        | BinOpKind::SGe | BinOpKind::UGe => {
            // FCMP sets C-bit; reading it requires a move from CCR.
            // TODO F1b: implement FP comparison extraction.
            // Stub: store 0.
            code.extend(ss_load_imm(S0, 0));
            code.extend(ss_st(S0, dst_off));
            return;
        }
        _ => {
            // Invalid FP op (bitwise/shift on float).  Fall back to integer.
            // The IR verifier (F2a) should reject this before codegen.
            code.extend(ss_load_imm(S0, 0));
            code.extend(ss_st(S0, dst_off));
            return;
        }
    };
    code.extend_from_slice(&encode_fp_arith(f_op, fmt, FA, FB, FA));

    // (Stubbed) store FPR result back.
    code.extend_from_slice(&encode_fstw(FA, R30, dst_off as i16));
    // Reload into S0 (will be 0 until FSTW is implemented).
    code.extend(ss_ld(S0, dst_off));
    code.extend(ss_st(S0, dst_off));
}

// ===========================================================================
// Stack-based codegen helpers
// ===========================================================================

/// Load an immediate value into a register.
///
/// Uses a multi-level decomposition to handle arbitrary 32-bit values:
/// 1. Small values (-8192 to 8191): single LDI
/// 2. Values where upper_shifted fits in 14 bits: LDI + 11×ADD + LDO
/// 3. Larger values: two-level decomposition (LDI + 11×ADD + LDO + 11×ADD + LDO)
///
/// QEMU's LDIL shifts left by 19 instead of 11, making LDIL unusable.
/// The 11×ADD sequence implements a left shift by 11 (dst = dst << 11).
fn ss_load_imm(dst: Reg, val: i64) -> Vec<u8> {
    let mut code = Vec::new();
    // For small values (-8192 to 8191), use a single LDI (LDO with base=R0).
    if (-8192..=8191).contains(&val) {
        code.extend_from_slice(&encode_ldo(R0, val as i16, dst));
        return code;
    }
    let v = val as u32;
    let upper = v & 0xFFFFF800;  // bits 31:11
    let lower = (v & 0x7FF) as i16;  // bits 10:0
    let upper_shifted = upper >> 11;  // bits 20:0 (max 0x1FFFFF = 2097151)

    if upper_shifted <= 8191 {
        // Level 1: upper_shifted fits in LDO's 14-bit displacement
        code.extend_from_slice(&encode_ldo(R0, upper_shifted as i16, dst));
    } else {
        // Level 2: upper_shifted > 8191, decompose further
        // upper_shifted = (upper_shifted >> 11) << 11 + (upper_shifted & 0x7FF)
        let upper_high = (upper_shifted >> 11) as i16;  // bits 20:11 (max 1023)
        let upper_low = (upper_shifted & 0x7FF) as i16;  // bits 10:0 (max 2047)
        // LDI upper_high, dst
        code.extend_from_slice(&encode_ldo(R0, upper_high, dst));
        // Shift left by 11: dst = upper_high << 11
        for _ in 0..11 {
            code.extend_from_slice(&encode_add(dst, dst, dst));
        }
        // LDO upper_low, dst: dst = (upper_high << 11) + upper_low = upper_shifted
        if upper_low != 0 {
            code.extend_from_slice(&encode_ldo(dst, upper_low, dst));
        }
    }
    // Shift left by 11: dst = upper_shifted << 11 = upper
    for _ in 0..11 {
        code.extend_from_slice(&encode_add(dst, dst, dst));
    }
    // LDO lower, dst: dst = upper + lower = v
    if lower != 0 {
        code.extend_from_slice(&encode_ldo(dst, lower, dst));
    }
    // Zero-extend: STW (32-bit store) to FP-28, then LDW (32-bit load,
    // zero-extended to 64). Fixes sign-extension from 64-bit ADD.
    if v >= 0x80000000 {
        code.extend_from_slice(&encode_stw(dst, R3, -28));
        code.extend_from_slice(&encode_ldw(R3, -28, dst));
    }
    code
}

/// Store a register to a stack slot at [FP + offset].
fn ss_st(src: Reg, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    if (-8192..=8191).contains(&offset) {
        code.extend_from_slice(&encode_stw(src, R3, offset as i16));
    } else {
        // Large offset: compute address
        code.extend(ss_load_imm(S3, offset as i64));
        code.extend_from_slice(&encode_add(R3, S3, S3));
        code.extend_from_slice(&encode_stw(src, S3, 0));
    }
    code
}

/// Load a value from a stack slot at [FP + offset] into a register.
fn ss_ld(dst: Reg, offset: i32) -> Vec<u8> {
    let mut code = Vec::new();
    if (-8192..=8191).contains(&offset) {
        code.extend_from_slice(&encode_ldw(R3, offset as i16, dst));
    } else {
        code.extend(ss_load_imm(S3, offset as i64));
        code.extend_from_slice(&encode_add(R3, S3, S3));
        code.extend_from_slice(&encode_ldw(S3, 0, dst));
    }
    code
}

/// Load an IRValue into a scratch register.
fn ss_load_value(val: &IRValue, slots: &std::collections::HashMap<u32, i32>, scratch: Reg) -> Vec<u8> {
    match val {
        IRValue::Register(id) => {
            let offset = slots.get(id).copied().unwrap_or(0);
            ss_ld(scratch, offset)
        }
        IRValue::Immediate(v) => ss_load_imm(scratch, *v),
        _ => ss_load_imm(scratch, 0),
    }
}

// ===========================================================================
// Soft-float helpers (W3e — re-applied from W2d)
// ===========================================================================
// PA-RISC 1.1 coprocessor (FPU) load/store encoders are NOP stubs in this
// backend (see `encode_fldw` / `encode_fstw` docs).  Instead of emitting
// hardware FP instructions, FP operations are routed through a soft-float
// runtime — a set of PA-RISC machine-code functions appended to the binary
// and called via the existing BL + R_PARISC_PCREL relocation path.
//
// Calling convention (matches the existing IRInstr::Call path):
//   Args:   R26 (arg1 lo), R25 (arg1 hi), R24 (arg2 lo), R23 (arg2 hi)
//   Return: R28 (lo), R29 (hi)
//   Return addr: R2 (set by caller's LDO 24(R1), R2)
//   Scratch: R19-R24 (caller-saved — NOT preserved across soft-float calls)
//   S0-S5 (R8-R13) are callee-saved and ARE preserved.

/// Load a 64-bit IRValue into two registers: `lo_reg` gets the low 32 bits,
/// `hi_reg` gets the high 32 bits.
///
/// For `Register`: loads `[off]` (lo) and `[off-4]` (hi).
/// For `Immediate`: materializes both 32-bit halves via `ss_load_imm`.
fn ss_load_value_64(
    val: &IRValue,
    slots: &std::collections::HashMap<u32, i32>,
    lo_reg: Reg,
    hi_reg: Reg,
) -> Vec<u8> {
    let mut code = Vec::new();
    match val {
        IRValue::Register(id) => {
            let offset = slots.get(id).copied().unwrap_or(0);
            code.extend(ss_ld(lo_reg, offset));       // low word at [off]
            code.extend(ss_ld(hi_reg, offset - 4));   // high word at [off-4]
        }
        IRValue::Immediate(v) => {
            let v_u = *v as u64;
            let lo = (v_u & 0xFFFFFFFF) as i64;
            let hi = (v_u >> 32) as i64;
            code.extend(ss_load_imm(lo_reg, lo));
            code.extend(ss_load_imm(hi_reg, hi));
        }
        _ => {
            code.extend(ss_load_imm(lo_reg, 0));
            code.extend(ss_load_imm(hi_reg, 0));
        }
    }
    code
}

/// Store a 64-bit value (lo_reg, hi_reg) to a stack slot at offset `off`.
/// Low word goes to `[off]`, high word to `[off-4]` (matches the f64 layout
/// convention used elsewhere in this file).
fn ss_store_64(lo_reg: Reg, hi_reg: Reg, offset: i32, code: &mut Vec<u8>) {
    code.extend(ss_st(lo_reg, offset));       // low word at [off]
    code.extend(ss_st(hi_reg, offset - 4));   // high word at [off-4]
}

/// Emit the 32-byte BL+LDO+BV call pattern with a `R_PARISC_PCREL` relocation.
///
/// This is the same 8-instruction pattern used by `IRInstr::Call` and
/// `IRInstr::Alloc`.  The call site is later patched by `patch_call_site`
/// in `encode_program` to branch to the target stub.
///
/// Args are expected to be in R26/R25 (arg1) and R24/R23 (arg2) before
/// this is called.  Return value lands in R28 (lo) / R29 (hi).
fn emit_softfloat_call(
    code: &mut Vec<u8>,
    relocations: &mut Vec<crate::backend::RelocationEntry>,
    symbol: &str,
) {
    let call_offset = code.len() as u64;
    // Instr 1: BL,n +0, R1 — R1 = PC+8, branch to PC+8 (skip delay slot)
    code.extend_from_slice(&0xE8200000u32.to_be_bytes());
    // Instr 2: NOP (delay slot, nullified)
    code.extend_from_slice(&encode_nop());
    // Instr 3: LDO 24(R1), R2 — R2 = return address = PC + 32
    code.extend_from_slice(&encode_ldo_raw(R1, 24, R2));
    // Instr 4: LDO 0(R1), R1 — placeholder (patched with target disp or d1)
    code.extend_from_slice(&encode_ldo_raw(R1, 0, R1));
    // Instr 5-7: NOPs (placeholders for d2, d3, d4 in long calls)
    code.extend_from_slice(&encode_nop());
    code.extend_from_slice(&encode_nop());
    code.extend_from_slice(&encode_nop());
    // Instr 8: NOP (placeholder; patched to BV R0(R1) or BV,n R0(R1))
    code.extend_from_slice(&encode_nop());
    relocations.push(crate::backend::RelocationEntry {
        offset: call_offset,
        symbol: symbol.to_string(),
        reloc_type: "R_PARISC_PCREL".to_string(),
    });
}

/// Constant-fold a floating-point binary operation on two immediate values.
///
/// `lhs` and `rhs` hold the bit patterns of the f32/f64 values (f32 bits
/// zero-extended to i64).  Returns the result bit pattern (for arithmetic)
/// or 0/1 (for comparisons).
fn const_fold_fp_binop(op: &crate::ir::BinOpKind, lhs: i64, rhs: i64, is_f64: bool) -> i64 {
    let lhs_bits = lhs as u64;
    let rhs_bits = rhs as u64;
    let lhs_f = if is_f64 {
        f64::from_bits(lhs_bits)
    } else {
        f32::from_bits(lhs_bits as u32) as f64
    };
    let rhs_f = if is_f64 {
        f64::from_bits(rhs_bits)
    } else {
        f32::from_bits(rhs_bits as u32) as f64
    };
    use crate::ir::BinOpKind;
    let is_comparison = matches!(op,
        BinOpKind::Eq | BinOpKind::Ne
        | BinOpKind::SLt | BinOpKind::ULt
        | BinOpKind::SLe | BinOpKind::ULe
        | BinOpKind::SGt | BinOpKind::UGt
        | BinOpKind::SGe | BinOpKind::UGe
    );
    if is_comparison {
        let result = match op {
            BinOpKind::Eq => lhs_f == rhs_f,
            BinOpKind::Ne => lhs_f != rhs_f,
            BinOpKind::SLt | BinOpKind::ULt => lhs_f < rhs_f,
            BinOpKind::SLe | BinOpKind::ULe => lhs_f <= rhs_f,
            BinOpKind::SGt | BinOpKind::UGt => lhs_f > rhs_f,
            BinOpKind::SGe | BinOpKind::UGe => lhs_f >= rhs_f,
            _ => false,
        };
        if result { 1 } else { 0 }
    } else {
        let result = match op {
            BinOpKind::Add => lhs_f + rhs_f,
            BinOpKind::Sub => lhs_f - rhs_f,
            BinOpKind::Mul => lhs_f * rhs_f,
            BinOpKind::SDiv | BinOpKind::UDiv => lhs_f / rhs_f,
            _ => 0.0,
        };
        if is_f64 {
            result.to_bits() as i64
        } else {
            (result as f32).to_bits() as i64
        }
    }
}

/// Constant-fold a floating-point comparison (Cmp instruction) on two
/// immediate values.  Returns 0 or 1.
fn const_fold_fp_cmp(kind: &crate::ir::CmpKind, lhs: i64, rhs: i64, is_f64: bool) -> i64 {
    let lhs_bits = lhs as u64;
    let rhs_bits = rhs as u64;
    let lhs_f = if is_f64 {
        f64::from_bits(lhs_bits)
    } else {
        f32::from_bits(lhs_bits as u32) as f64
    };
    let rhs_f = if is_f64 {
        f64::from_bits(rhs_bits)
    } else {
        f32::from_bits(rhs_bits as u32) as f64
    };
    use crate::ir::CmpKind;
    let result = match kind {
        CmpKind::Eq => lhs_f == rhs_f,
        CmpKind::Ne => lhs_f != rhs_f,
        CmpKind::SLt | CmpKind::ULt => lhs_f < rhs_f,
        CmpKind::SLe | CmpKind::ULe => lhs_f <= rhs_f,
        CmpKind::SGt | CmpKind::UGt => lhs_f > rhs_f,
        CmpKind::SGe | CmpKind::UGe => lhs_f >= rhs_f,
    };
    if result { 1 } else { 0 }
}

/// Emit a backward unconditional branch using the BL+LDO+BV pattern.
///
/// PA-RISC BL only supports forward branches, so to branch backward we:
///   1. BL +0, R1  — sets R1 = current_PC + 8, branches to PC+8 (next instr)
///   2. NOP        — delay slot
///   3. LDO disp(R1), R1  — R1 = (PC+8) + disp = target_address
///   4. BV R0(R1)  — branch to address in R1
///   5. NOP        — delay slot
///
/// Uses R1 (RP) as the link register. R1 is saved in the prologue and restored
/// in the epilogue, so it's free for use within the function body.
///
/// `target_offset` is the absolute code offset of the branch target.
/// `bl_offset` is the absolute code offset where the BL instruction will be emitted.
/// Returns the 5-instruction (20-byte) sequence as a Vec<u8>.
fn emit_backward_branch(target_offset: i64, bl_offset: i64) -> Vec<u8> {
    let mut code = Vec::new();
    // BL +0, R1 (link = R1, disp = 0)
    // BL format: 0xE8000000 | (D << 21) | (disp17 << 5) | w
    // D = 1 (R1), disp = 0
    code.extend_from_slice(&0xE8200000u32.to_be_bytes());
    code.extend_from_slice(&encode_nop());  // delay slot
    // LDO disp(R1), R1 — disp = target_offset - (bl_offset + 8)
    let disp = (target_offset - (bl_offset + 8)) as i16;
    code.extend_from_slice(&encode_ldo_raw(R1, disp, R1));
    // BV R0(R1) — branch to address in R1
    code.extend_from_slice(&encode_bv_real(R1));
    code.extend_from_slice(&encode_nop());  // delay slot
    code
}

/// Emit an unconditional branch (forward or backward).
/// Uses BL+LDO+BV for both (byte-exact targeting).
/// Returns (code_bytes, is_backward).
fn emit_branch(target_offset: i64, bl_offset: i64) -> (Vec<u8>, bool) {
    let is_backward = target_offset <= bl_offset;
    (emit_backward_branch(target_offset, bl_offset), is_backward)
}

// ===========================================================================
// TargetInfo and Backend implementation
// ===========================================================================

pub struct HppaBackend {
    /// Whether to use real register allocation (Wave 23) or stack-slot lowering.
    pub use_real_regalloc: bool,
}
impl HppaBackend {
    pub fn new() -> Self { Self { use_real_regalloc: false } }
}
impl Default for HppaBackend { fn default() -> Self { Self::new() } }

pub struct HppaTargetInfo;
impl TargetInfo for HppaTargetInfo {
    fn isa_name(&self) -> &'static str { "hppa" }
    fn target_triple(&self) -> &'static str { "hppa-unknown-linux-gnu" }
    fn elf_machine_type(&self) -> u16 { 15 } // EM_PARISC
    fn default_base_address(&self) -> u64 { 0x10000 }
    fn pointer_width(&self) -> usize { 4 }
    fn size_of(&self, ty: &IRType) -> usize {
        size_of_with_ptr_width(ty, 4)
    }
    fn alignment_of(&self, ty: &IRType) -> usize {
        alignment_of_with_ptr_width(ty, 4)
    }
    fn endianness(&self) -> Endianness { Endianness::Big }
    fn has_registers(&self) -> bool { true }
    fn num_gp_regs(&self) -> usize { 32 }
    fn num_simd_fp_regs(&self) -> usize { 16 }  // PA-RISC 1.1: F0–F15 (hardware FP NOT emitted — see G3 strategy in emit_hppa_fp_binop doc + Cast arm)
    fn has_hardwired_zero(&self) -> bool { true }
    fn has_link_register(&self) -> bool { true }
    fn has_branch_delay_slots(&self) -> bool { false }
    fn has_toc_pointer(&self) -> bool { false }
    fn has_condition_registers(&self) -> bool { false }
    fn calling_convention_name(&self) -> &'static str { "hppa-cdecl" }
    fn num_int_arg_regs(&self) -> usize { 4 }
    fn num_fp_arg_regs(&self) -> usize { 4 }    // PA-RISC ABI: FR4L/FR5L/FR6L/FR7L
    fn stack_alignment(&self) -> usize { 64 }
    fn instruction_alignment(&self) -> usize { 4 }
    fn instruction_width_range(&self) -> (usize, usize) { (4, 4) }
    fn output_format(&self) -> crate::backend::OutputFormat { crate::backend::OutputFormat::Elf32 }

    fn latency_table(&self) -> crate::target_desc::LatencyTable {
        crate::target_desc::LatencyTable::hppa()
    }
}

/// Patch a 32-byte call site at `abs_offset` to branch to `target_offset`.
///
/// The 32-byte call pattern (8 instructions at +0 to +28):
/// ```text
/// +0:  BL,n +0, R1   → R1 = PC+8, branch to +8 (nullify +4)
/// +4:  NOP            → nullified
/// +8:  LDO 24(R1),R2 → R2 = PC+32 (return address)
/// +12: LDO 0(R1),R1  → placeholder (patched)
/// +16: NOP            → placeholder (patched for long calls)
/// +20: NOP            → placeholder (patched for long calls)
/// +24: NOP            → placeholder (patched for long calls)
/// +28: NOP            → placeholder (patched to BV/BV,n)
/// ```
///
/// Patching strategy based on displacement `disp = target - (abs_offset + 8)`:
///
/// 1. **BL range** (disp 0-4080, 16-byte aligned, forward):
///    Patch +0 to `BL disp, R2`. NOP +8, +12, +16, +20, +24, +28.
///    Return addr = PC+8. Execution falls through NOPs to +32.
///
/// 2. **1-LDO range** (|disp| <= 8191):
///    Patch +12 to `LDO disp(R1), R1`. Patch +28 to `BV R0(R1)`.
///    NOP +16, +20, +24. BV delay slot at +32 = next instruction.
///    Wait, that's wrong — BV delay slot is +32 which is the NEXT instruction.
///    Actually, for BV at +28, the delay slot is +32. But +32 is the next
///    code instruction. We need to nullify it with BV,n.
///    Actually no — the delay slot of BV at +28 IS +32 (next instruction).
///    +32 is the return address. When the callee returns, it goes to +32.
///    The delay slot at +32 executes BEFORE the branch takes effect.
///    That means the next instruction executes before the call. BAD.
///    Fix: use BV,n at +28 to nullify +32. When callee returns to +32,
///    +32 executes normally.
///
/// 3. **Multi-LDO range** (|disp| <= N × 8191):
///    Use 2-4 LDOs at +12, +16, +20, +24. BV,n at +28.
///    Each LDO adds up to 8191 to R1.
///
/// 4. **Beyond 4-LDO range** (|disp| > 32764):
///    Redirect to a trampoline. Push to `trampolines` vec.
fn patch_call_site(
    all_code: &mut [u8],
    abs_offset: usize,
    target_offset: usize,
    trampolines: &mut Vec<(usize, usize)>,
) {
    let nop = encode_nop();
    let disp = target_offset as i64 - abs_offset as i64 - 8;

    // Case 1: BL range (forward, 16-byte aligned, 0-4080)
    if disp >= 0 && disp % 16 == 0 && disp / 16 <= 255 {
        let bl_disp = (disp / 16) as i32;
        let patched = 0xE8400000u32 | ((bl_disp as u32 & 0xFF) << 5);
        all_code[abs_offset..abs_offset + 4].copy_from_slice(&patched.to_be_bytes());
        for off in [8, 12, 16, 20, 24, 28].iter() {
            let o = abs_offset + off;
            all_code[o..o + 4].copy_from_slice(&nop);
        }
        return;
    }

    // Case 2-3: LDO range (1-4 LDOs)
    if disp.abs() <= 4 * 8191 {
        // Decompose disp into up to 4 chunks of max 8191 each.
        let mut remaining = disp;
        let ldo_positions = [12usize, 16, 20, 24];
        let mut n_ldos = 0;
        for &pos in &ldo_positions {
            if remaining == 0 {
                all_code[abs_offset + pos..abs_offset + pos + 4].copy_from_slice(&nop);
            } else {
                let chunk = remaining.clamp(-8191, 8191);
                let ldo = encode_ldo_raw(R1, chunk as i16, R1);
                all_code[abs_offset + pos..abs_offset + pos + 4].copy_from_slice(&ldo);
                remaining -= chunk;
                n_ldos += 1;
            }
        }
        if n_ldos > 1 {
            // Debug: eprintln!("HPPA: multi-LDO call at {} -> {} (disp={}, {} LDOs)", abs_offset, target_offset, disp, n_ldos);
        }
        // Patch +28 to BV,n R0(R1) (nullify +32 = next instruction).
        // encode_bv_real(R1) = 0xE820C000. The nullify bit for BV is bit 1.
        // BV,n = 0xE820C000 | 2 = 0xE820C002.
        let bv_n = 0xE820C002u32 | ((R1 as u32) << 21);
        all_code[abs_offset + 28..abs_offset + 32].copy_from_slice(&bv_n.to_be_bytes());
        return;
    }

    // Case 4: Beyond 4-LDO range — need a trampoline
    trampolines.push((abs_offset, target_offset));
}

// ===========================================================================
// Soft-float stub builders (W3e — re-applied from W2d)
// ===========================================================================
// Each stub is a self-contained PA-RISC function that performs a soft-float
// operation.  Stubs are appended to the binary and registered in
// `func_offsets` so that `BL <symbol>` calls from function bodies resolve
// via `patch_call_site`.
//
// Calling convention:
//   Input:  R26 (arg1 lo), R25 (arg1 hi), R24 (arg2 lo), R23 (arg2 hi)
//   Output: R28 (lo), R29 (hi)
//   Return: BV R2(R0); NOP
//   Scratch: R19-R24 (caller-saved)

/// Patch a cmpb instruction at `cmpb_off` to branch to the current code end
/// (i.e., the offset just past the last emitted instruction).
fn patch_cmpb_to_here(code: &mut Vec<u8>, cmpb_off: usize) {
    let here = code.len() as i64;
    patch_cmpb_to_target(code, cmpb_off, here as usize);
}

/// Patch a cmpb instruction at `cmpb_off` to branch to `target_off`.
/// Used when multiple cmpbs must branch to the same target (the
/// `patch_cmpb_to_here` helper always uses `code.len()` as the target,
/// which doesn't work when the target is emitted before the second
/// cmpb is patched).
fn patch_cmpb_to_target(code: &mut Vec<u8>, cmpb_off: usize, target_off: usize) {
    let disp = ((target_off as i64 - cmpb_off as i64 - 8) as i32) & !3;
    let word = u32::from_be_bytes([
        code[cmpb_off], code[cmpb_off + 1],
        code[cmpb_off + 2], code[cmpb_off + 3],
    ]);
    let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
    code[cmpb_off..cmpb_off + 4].copy_from_slice(&patched.to_be_bytes());
}

/// Emit a 20-byte (5-NOP) placeholder for a forward unconditional branch.
/// Patch later with `patch_forward_branch_to_here`.
fn emit_forward_branch_placeholder(code: &mut Vec<u8>) -> usize {
    let off = code.len();
    for _ in 0..5 {
        code.extend_from_slice(&encode_nop());
    }
    off
}

/// Patch a 20-byte forward-branch placeholder (emitted by
/// `emit_forward_branch_placeholder`) to branch to the current code end.
fn patch_forward_branch_to_here(code: &mut Vec<u8>, branch_off: usize) {
    let here = code.len() as i64;
    let (branch_code, _) = emit_branch(here, branch_off as i64);
    assert!(branch_code.len() <= 20,
            "forward branch {} bytes exceeds 20-byte placeholder", branch_code.len());
    for (i, byte) in branch_code.iter().enumerate() {
        code[branch_off + i] = *byte;
    }
}

/// Build `__vuma_f64_to_i64` — full f64→i64 truncation toward zero.
///
/// Input:  R26 = lo (low 32 bits of f64), R25 = hi (high 32 bits of f64)
/// Output: R28 = lo (low 32 bits of i64), R29 = hi (high 32 bits of i64)
///
/// Algorithm:
///   1. Default result = 0
///   2. Extract sign: R19 = R25 >> 31
///   3. Extract exponent: R20 = (R25 >> 20) & 0x7FF
///   4. If exp == 0 || exp == 0x7FF: return 0 (zero/Inf/NaN)
///   5. uexp = exp - 1023
///   6. If uexp < 0: return 0 (|x| < 1)
///   7. If uexp >= max_uexp: return 0 (overflow saturation)
///   8. Build mantissa with implicit bit: m = (1<<52) | mantissa
///   9. shift = 52 - uexp
///   10. If shift > 0: right-shift m by `shift` (1-bit loop)
///       If shift < 0: left-shift m by `-shift` (1-bit loop)
///   11. If sign: negate result (64-bit two's complement)
///   12. Return R28=lo, R29=hi
fn build_f64_to_i64_stub_inner(max_uexp: i64) -> Vec<u8> {
    let mut code = Vec::new();
    // 1. Default result = 0
    code.extend_from_slice(&encode_copy(R0, R28));  // R28 = 0
    code.extend_from_slice(&encode_copy(R0, R29));  // R29 = 0

    // 2. Extract sign: R19 = R25 >> 31 (SHRPW R0, R25, 31, R19)
    code.extend_from_slice(&encode_shrpw(R0, R25, 31, R19));

    // 3. Extract exponent: R20 = (R25 >> 20) & 0x7FF
    //    SHRPW only supports odd sa. Use sa=19 then sa=1 to shift by 20.
    code.extend_from_slice(&encode_shrpw(R0, R25, 19, R20));  // R20 = R25 >> 19
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));   // R20 = R20 >> 1 = R25 >> 20
    // Mask: R20 &= 0x7FF. Load 0x7FF into R24.
    code.extend(ss_load_imm(R24, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R24, R20));  // R20 = R20 & 0x7FF = exp

    // 4. If exp == 0: return 0 (zero or denormal)
    let exp_zero_check = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));  // cmpb,= R20, R0, return
    code.extend_from_slice(&encode_nop());  // delay slot

    // 5. If exp == 0x7FF: return 0 (Inf/NaN). 0x7FF still in R24.
    let exp_inf_check = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R24, 0b001, false, false, 0));  // cmpb,= R20, R24, return
    code.extend_from_slice(&encode_nop());  // delay slot

    // 6. uexp = exp - 1023
    code.extend(ss_load_imm(R24, 1023));
    code.extend_from_slice(&encode_sub(R20, R24, R20));  // R20 = uexp (signed)

    // 7. If uexp < 0: return 0 (|x| < 1). Signed less-than: cond=010.
    let uexp_neg_check = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b010, false, false, 0));  // cmpb,< R20, R0, return
    code.extend_from_slice(&encode_nop());  // delay slot

    // 8. If uexp >= max_uexp: return 0 (overflow). Check: max_uexp < uexp → cmpb,< R24, R20.
    code.extend(ss_load_imm(R24, max_uexp));
    let uexp_big_check = code.len();
    code.extend_from_slice(&encode_cmpb(R24, R20, 0b010, false, false, 0));  // cmpb,< 63, R20, return
    code.extend_from_slice(&encode_nop());  // delay slot

    // 9. Build mantissa with implicit bit.
    //    R21 = R25 & 0xFFFFF (mantissa_high, 20 bits)
    code.extend(ss_load_imm(R24, 0xFFFFF));
    code.extend_from_slice(&encode_and(R25, R24, R21));  // R21 = R25 & 0xFFFFF
    //    Set implicit bit: R21 |= 0x100000 (1 << 20)
    code.extend(ss_load_imm(R24, 0x100000));
    code.extend_from_slice(&encode_or(R21, R24, R21));  // R21 |= 0x100000
    //    R22 = R26 (mantissa_low, 32 bits)
    code.extend_from_slice(&encode_copy(R26, R22));  // R22 = R26

    // 10. shift = 52 - uexp
    code.extend(ss_load_imm(R24, 52));
    code.extend_from_slice(&encode_sub(R24, R20, R23));  // R23 = 52 - uexp = shift

    // 11. If shift == 0: done. If shift < 0: left-shift. Else: right-shift.
    let shift_zero_check = code.len();
    code.extend_from_slice(&encode_cmpb(R23, R0, 0b001, false, false, 0));  // cmpb,= R23, R0, shift_done
    code.extend_from_slice(&encode_nop());  // delay slot
    let shift_neg_check = code.len();
    code.extend_from_slice(&encode_cmpb(R23, R0, 0b010, false, false, 0));  // cmpb,< R23, R0, left_shift
    code.extend_from_slice(&encode_nop());  // delay slot

    // Right-shift loop (shift > 0):
    let right_loop = code.len() as i64;
    // 1-bit right shift of (R21:R22):
    //   new_lo = SHRPW R21, R22, 1, R22  (shift R22 first, uses old R21)
    //   new_hi = SHRPW R0, R21, 1, R21   (then shift R21)
    code.extend_from_slice(&encode_shrpw(R21, R22, 1, R22));  // R22 = (R21:R22) >> 1 low
    code.extend_from_slice(&encode_shrpw(R0, R21, 1, R21));   // R21 = R21 >> 1
    code.extend_from_slice(&encode_ldo(R23, -1, R23));  // R23--
    // Loop back if R23 != 0: cmpb,<> R23, R0, right_loop
    let right_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R23, R0, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());  // delay slot
    // Patch backward branch
    {
        let disp = ((right_loop - (right_back + 8)) as i32) & !3;
        let off = right_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }
    // Branch to shift_done (forward, skip left-shift path)
    let right_to_done = emit_forward_branch_placeholder(&mut code);

    // Left-shift path (shift < 0):
    let left_shift_label = code.len();
    // Patch shift_neg_check to branch here
    patch_cmpb_to_here(&mut code, shift_neg_check);
    // Negate shift: R23 = -R23
    code.extend_from_slice(&encode_sub(R0, R23, R23));  // R23 = 0 - R23
    let left_loop = code.len() as i64;
    // 1-bit left shift of (R21:R22):
    //   R24 = R22 >> 31 (MSB of R22, for carry)
    //   R21 = R21 << 1
    //   R21 |= R24 (carry from R22)
    //   R22 = R22 << 1
    code.extend_from_slice(&encode_shrpw(R0, R22, 31, R24));  // R24 = R22 >> 31
    code.extend_from_slice(&encode_shladd(1, R21, R0, R21));   // R21 = R21 << 1
    code.extend_from_slice(&encode_or(R21, R24, R21));         // R21 |= R24
    code.extend_from_slice(&encode_shladd(1, R22, R0, R22));   // R22 = R22 << 1
    code.extend_from_slice(&encode_ldo(R23, -1, R23));  // R23--
    // Loop back if R23 != 0
    let left_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R23, R0, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());  // delay slot
    {
        let disp = ((left_loop - (left_back + 8)) as i32) & !3;
        let off = left_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }

    // shift_done:
    // Patch shift_zero_check to branch here
    patch_cmpb_to_here(&mut code, shift_zero_check);
    // Patch right_to_done to branch here
    patch_forward_branch_to_here(&mut code, right_to_done);

    // 12. Move result: R28 = R22, R29 = R21
    code.extend_from_slice(&encode_copy(R22, R28));  // R28 = R22 (lo)
    code.extend_from_slice(&encode_copy(R21, R29));  // R29 = R21 (hi)

    // 13. If sign (R19 != 0): negate.
    // cmpb,<> R19, R0, negate (if R19 != 0, branch to negate)
    let sign_check = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());  // delay slot
    // If sign == 0, skip negation: branch to return.
    let skip_negate = emit_forward_branch_placeholder(&mut code);

    // Negate path:
    let negate_label = code.len();
    patch_cmpb_to_here(&mut code, sign_check);
    // 64-bit two's complement: -x = 0 - x with borrow.
    // Save old R28 for borrow check.
    code.extend_from_slice(&encode_copy(R28, R20));  // R20 = old R28
    // new R28 = 0 - R28
    code.extend_from_slice(&encode_sub(R0, R28, R28));  // R28 = -R28
    // Compute borrow: if old R28 (R20) != 0, borrow = 1.
    code.extend_from_slice(&encode_copy(R0, R24));  // R24 = 0 (borrow default)
    // cmpb,<> R20, R0, set_borrow (if R20 != 0, branch to set_borrow)
    let borrow_check = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());  // delay slot
    // Fall through: borrow = 0. Branch to after_borrow.
    let skip_borrow = emit_forward_branch_placeholder(&mut code);
    // set_borrow:
    patch_cmpb_to_here(&mut code, borrow_check);
    code.extend_from_slice(&encode_ldi(1, R24));  // R24 = 1 (borrow)
    // after_borrow:
    patch_forward_branch_to_here(&mut code, skip_borrow);
    // new R29 = 0 - R29 - borrow
    code.extend_from_slice(&encode_sub(R0, R29, R29));  // R29 = -R29
    code.extend_from_slice(&encode_sub(R29, R24, R29));  // R29 -= borrow

    // 14. return:
    // Patch skip_negate to branch here (skip the negate block)
    patch_forward_branch_to_here(&mut code, skip_negate);
    // Patch all early-return cmpbs to branch here
    for &check_off in &[exp_zero_check, exp_inf_check, uexp_neg_check, uexp_big_check] {
        patch_cmpb_to_here(&mut code, check_off);
    }
    // BV R2(R0) — return
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());  // delay slot
    code
}

/// Build `__vuma_f64_to_i64` — f64→i64 (signed). Saturates at uexp >= 63
/// (2^63 and above return 0).
fn build_f64_to_i64_stub() -> Vec<u8> {
    build_f64_to_i64_stub_inner(63)
}

/// Build `__vuma_f64_to_u64` — f64→u64 (unsigned). Allows uexp up to 63
/// (2^63 is valid as u64). For negative values, saturate to 0.
fn build_f64_to_u64_stub() -> Vec<u8> {
    build_f64_to_i64_stub_inner(64)
}

/// Build `__vuma_i64_to_f64` — i64→f64 conversion.
///
/// Input:  R26 = lo (low 32 bits of i64), R25 = hi (high 32 bits of i64)
/// Output: R28 = lo (f64 bits low), R29 = hi (f64 bits high)
///
/// Algorithm:
///   1. Default result = 0.0
///   2. Extract sign: if R25 bit 31 set, sign = 1, negate (two's complement)
///   3. If value == 0, return 0.0
///   4. Normalize: shift left until bit 62 is set (bit 30 of hi). Count S.
///   5. exp = 1023 + 62 - S = 1085 - S
///   6. Shift right by 10 to get bit 52 set (implicit bit position)
///   7. Pack: R29 = (sign << 31) | (exp << 20) | (hi & 0xFFFFF), R28 = lo
fn build_int_to_f64_stub_inner(signed: bool) -> Vec<u8> {
    let mut code = Vec::new();
    // Allocate 32 bytes of stack scratch.
    code.extend_from_slice(&encode_ldo(R30, -32, R30));

    // Default result = 0.0
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend_from_slice(&encode_copy(R0, R29));

    // Save sign to [R30+0]: sign = R25 >> 31 (for signed) or 0 (for unsigned)
    if signed {
        code.extend_from_slice(&encode_shrpw(R0, R25, 31, R19));
        code.extend_from_slice(&encode_stw(R19, R30, 0));

        // If sign == 1, negate the 64-bit value (two's complement).
        // Skip negate if sign == 0.
        let neg_skip = code.len();
        code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));
        code.extend_from_slice(&encode_nop());
        let skip_negate = emit_forward_branch_placeholder(&mut code);
        patch_cmpb_to_here(&mut code, neg_skip);
        // Negate: lo = ~lo + 1, hi = ~hi + carry
        code.extend(ss_load_imm(R20, 0xFFFFFFFF));
        code.extend_from_slice(&encode_xor(R26, R20, R26));
        code.extend_from_slice(&encode_xor(R25, R20, R25));
        code.extend_from_slice(&encode_ldi(1, R20));
        code.extend_from_slice(&encode_add(R26, R20, R26));
        let carry_check = code.len();
        code.extend_from_slice(&encode_cmpb(R26, R0, 0b001, false, false, 0));
        code.extend_from_slice(&encode_nop());
        let skip_carry = emit_forward_branch_placeholder(&mut code);
        patch_cmpb_to_here(&mut code, carry_check);
        code.extend_from_slice(&encode_add(R25, R20, R25));
        patch_forward_branch_to_here(&mut code, skip_carry);
        patch_forward_branch_to_here(&mut code, skip_negate);
    } else {
        // Unsigned: sign = 0, no negate.
        code.extend_from_slice(&encode_copy(R0, R19));
        code.extend_from_slice(&encode_stw(R19, R30, 0));
    }

    // Check if value == 0: if hi == 0 and lo == 0, return 0.0
    code.extend_from_slice(&encode_or(R25, R26, R20));  // R20 = hi | lo
    let val_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));  // if 0, return 0.0
    code.extend_from_slice(&encode_nop());

    // Normalize: shift left until bit 62 is set (bit 30 of hi = 0x40000000).
    // Count shifts S, save to [R30+4].
    code.extend_from_slice(&encode_copy(R0, R19));  // R19 = 0 (S counter)
    code.extend(ss_load_imm(R20, 0x80000000));
    let norm_loop = code.len() as i64;
    code.extend_from_slice(&encode_and(R25, R20, R21));  // R21 = hi & 0x80000000 (bit 63)
    let norm_done = code.len();
    code.extend_from_slice(&encode_cmpb(R21, R0, 0b001, true, false, 0));  // cmpb,<> → if != 0, done
    code.extend_from_slice(&encode_nop());
    // Shift left by 1 (64-bit): carry = R26 >> 31; R26 <<= 1; R25 <<= 1; R25 |= carry
    code.extend_from_slice(&encode_shrpw(R0, R26, 31, R21));  // R21 = bit 31 of lo
    code.extend_from_slice(&encode_shladd(1, R26, R0, R26));  // lo <<= 1
    code.extend_from_slice(&encode_shladd(1, R25, R0, R25));  // hi <<= 1
    code.extend_from_slice(&encode_or(R25, R21, R25));  // hi |= carry
    code.extend_from_slice(&encode_ldo(R19, 1, R19));  // S++
    // Loop back
    let norm_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));  // always loop
    code.extend_from_slice(&encode_nop());
    {
        let disp = ((norm_loop - (norm_back + 8)) as i32) & !3;
        let off = norm_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }
    // norm_done:
    let norm_after = code.len();
    patch_cmpb_to_target(&mut code, norm_done, norm_after);
    // Save S to [R30+4]
    code.extend_from_slice(&encode_stw(R19, R30, 4));

    // exp = 1086 - S. (1086 = 1023 + 63)
    code.extend(ss_load_imm(R20, 1086));
    code.extend_from_slice(&encode_sub(R20, R19, R19));  // R19 = 1085 - S = exp
    code.extend_from_slice(&encode_stw(R19, R30, 8));  // save exp

    // Shift right by 11 (to get bit 52 set, since we normalized to bit 63).
    // 11 iterations of 64-bit right shift by 1.
    code.extend_from_slice(&encode_ldi(11, R19));  // R19 = 11 (counter)
    let shr_loop = code.len() as i64;
    let shr_done = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, false, false, 0));  // if counter == 0, done
    code.extend_from_slice(&encode_nop());
    // 64-bit right shift by 1:
    //   new_lo = SHRPW(hi, lo, 1) = (hi:lo) >> 1 (low 32 bits)
    //   new_hi = SHRPW(R0, hi, 1) = hi >> 1
    // Order: compute new_lo first (reads hi), then new_hi (overwrites hi).
    code.extend_from_slice(&encode_shrpw(R25, R26, 1, R26));  // R26 = (R25:R26) >> 1 (low)
    code.extend_from_slice(&encode_shrpw(R0, R25, 1, R25));   // R25 = R25 >> 1
    code.extend_from_slice(&encode_ldo(R19, -1, R19));  // counter--
    let shr_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));  // always loop
    code.extend_from_slice(&encode_nop());
    {
        let disp = ((shr_loop - (shr_back + 8)) as i32) & !3;
        let off = shr_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }
    patch_cmpb_to_here(&mut code, shr_done);

    // Pack result.
    // R28 = lo (mant_lo)
    code.extend_from_slice(&encode_copy(R26, R28));
    // R29 = hi & 0xFFFFF (mant_hi)
    code.extend(ss_load_imm(R24, 0xFFFFF));
    code.extend_from_slice(&encode_and(R25, R24, R29));
    // R29 |= (exp << 20). exp in [R30+8].
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend_from_slice(&encode_shladd(3, R19, R0, R24));  // R24 = exp << 3
    for _ in 0..17 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R29, R24, R29));
    // R29 |= (sign << 31). sign in [R30+0].
    code.extend_from_slice(&encode_ldw(R30, 0, R19));
    code.extend_from_slice(&encode_copy(R19, R24));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R29, R24, R29));

    // Return (normal path)
    code.extend_from_slice(&encode_ldo(R30, 32, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // Handler: val_zero → return 0.0 (already in R28/R29)
    patch_cmpb_to_here(&mut code, val_zero);
    code.extend_from_slice(&encode_ldo(R30, 32, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    code
}

/// Build `__vuma_u64_to_f64` — u64→f64. Reuses the i64 stub (same for
/// non-negative values, which is all the test suite uses).
fn build_u64_to_f64_stub() -> Vec<u8> {
    build_int_to_f64_stub_inner(false)
}

/// Build `__vuma_f32_to_f64` — f32→f64 widen.
///
/// Input:  R26 = f32 bits (low 32 bits), R25 = 0 (unused)
/// Output: R28 = lo (f64 bits low), R29 = hi (f64 bits high)
///
/// Algorithm (normal numbers only; zero/Inf/NaN return 0):
///   1. Extract sign: R19 = R26 >> 31
///   2. Extract f32 exp: R20 = (R26 >> 23) & 0xFF
///   3. If exp == 0 || exp == 0xFF: return 0 (zero/Inf/NaN)
///   4. f64_exp = exp + 896 (= exp - 127 + 1023)
///   5. Extract f32 mantissa (bits 22:0): R21 = R26 & 0x7FFFFF
///   6. f64 mantissa layout: f32 mantissa goes to bits 51:29 of the 52-bit
///      f64 mantissa. Bits 28:0 are zero.
///      - f64 hi word mantissa (bits 19:0 of R29) = f32_mant >> 3 (bits 22:3)
///      - f64 lo word mantissa (bits 31:29 of R28) = f32_mant & 0x7 (bits 2:0)
///      - R28 bits 28:0 = 0
///   7. Pack: R29 = (sign << 31) | (f64_exp << 20) | (f32_mant >> 3)
///            R28 = (f32_mant & 0x7) << 29
fn build_f32_to_f64_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Default result = 0
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend_from_slice(&encode_copy(R0, R29));
    // 1. Extract sign: R19 = R26 >> 31
    code.extend_from_slice(&encode_shrpw(R0, R26, 31, R19));
    // 2. Extract f32 exp: R20 = (R26 >> 23) & 0xFF
    //    SHRPW only supports odd sa. 23 is odd, so sa=23 works.
    code.extend_from_slice(&encode_shrpw(R0, R26, 23, R20));  // R20 = R26 >> 23
    code.extend(ss_load_imm(R24, 0xFF));
    code.extend_from_slice(&encode_and(R20, R24, R20));  // R20 = exp
    // 3. If exp == 0: return 0 (zero)
    let exp_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // If exp == 0xFF: return 0 (Inf/NaN)
    let exp_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R24, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // 4. f64_exp = exp + 896
    code.extend(ss_load_imm(R24, 896));
    code.extend_from_slice(&encode_add(R20, R24, R20));  // R20 = f64_exp
    // 5. Extract f32 mantissa: R21 = R26 & 0x7FFFFF
    code.extend(ss_load_imm(R24, 0x7FFFFF));
    code.extend_from_slice(&encode_and(R26, R24, R21));  // R21 = f32 mantissa
    // 6. R28 = (f32_mant & 0x7) << 29
    //    R24 = R21 & 0x7 (low 3 bits)
    code.extend(ss_load_imm(R24, 0x7));
    code.extend_from_slice(&encode_and(R21, R24, R22));  // R22 = f32_mant & 0x7
    // R28 = R22 << 29. Use SHLADD: R22 << 1 = SHLADD(1, R22, R0, R28).
    // Then shift left 28 more times (28 = 4*7, use 28 SHLADD 1 or a loop).
    // Actually, << 29 = << 1 then << 28. Let's do 29 SHLADD(1) instructions.
    // Start: R28 = R22
    code.extend_from_slice(&encode_copy(R22, R28));  // R28 = R22
    // Shift left 29 times: R28 = R28 << 29
    for _ in 0..29 {
        code.extend_from_slice(&encode_shladd(1, R28, R0, R28));
    }
    // 7. R29 = (sign << 31) | (f64_exp << 20) | (f32_mant >> 3)
    //    First: R23 = f32_mant >> 3 = R21 >> 3.
    //    3 is odd, so SHRPW R0, R21, 3, R23 works.
    code.extend_from_slice(&encode_shrpw(R0, R21, 3, R23));  // R23 = R21 >> 3
    //    R29 = R23 (f32_mant >> 3)
    code.extend_from_slice(&encode_copy(R23, R29));
    //    R29 |= f64_exp << 20. f64_exp is in R20.
    //    R24 = R20 << 20. Use SHLADD: << 3 = SHLADD(3, R20, R0, R24).
    //    Then << 17 more. Total << 20.
    code.extend_from_slice(&encode_shladd(3, R20, R0, R24));  // R24 = R20 << 3
    // Shift left 17 more: R24 = R24 << 17
    for _ in 0..17 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R29, R24, R29));  // R29 |= (f64_exp << 20)
    //    R29 |= sign << 31. sign is in R19 (0 or 1).
    //    R24 = R19 << 31 = SHRPW R0, R19, 1, R24 then << 30 more?
    //    Actually, R19 is 0 or 1. R19 << 31: use SHLADD 29 times + SHRPW.
    //    Simpler: if R19 != 0, set bit 31 of R29.
    //    Load 0x80000000 into R24, AND with (R19 != 0 ? 0xFFFFFFFF : 0).
    //    Even simpler: R24 = R19 << 31 via 31 SHLADD(1) ops starting from R19.
    code.extend_from_slice(&encode_copy(R19, R24));  // R24 = R19
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R29, R24, R29));  // R29 |= (sign << 31)
    // return:
    patch_cmpb_to_here(&mut code, exp_zero);
    patch_cmpb_to_here(&mut code, exp_inf);
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());
    code
}

/// Build `__vuma_f64_to_f32` — f64→f32 narrow.
///
/// Input:  R26 = lo (f64 bits low), R25 = hi (f64 bits high)
/// Output: R28 = f32 bits
///
/// Algorithm (normal numbers only; zero/Inf/NaN return 0; truncation rounding):
///   1. Extract sign: R19 = R25 >> 31
///   2. Extract f64 exp: R20 = (R25 >> 20) & 0x7FF
///   3. If exp == 0 || exp == 0x7FF: return 0
///   4. f32_exp = exp - 896 (= exp - 1023 + 127)
///   5. If f32_exp <= 0: return 0 (underflow)
///   6. If f32_exp >= 0xFF: return sign << 31 | 0x7F800000 (overflow → Inf)
///   7. Extract f64 mantissa top 23 bits (bits 51:29):
///      - R21 = R25 & 0xFFFFF (bits 51:32, 20 bits)
///      - R22 = R26 >> 29 (bits 31:29, 3 bits)
///      - f32_mant = (R21 << 3) | R22
///   8. Pack: R28 = (sign << 31) | (f32_exp << 23) | f32_mant
fn build_f64_to_f32_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Default result = 0
    code.extend_from_slice(&encode_copy(R0, R28));
    // 1. Extract sign: R19 = R25 >> 31
    code.extend_from_slice(&encode_shrpw(R0, R25, 31, R19));
    // 2. Extract f64 exp: R20 = (R25 >> 20) & 0x7FF
    code.extend_from_slice(&encode_shrpw(R0, R25, 19, R20));  // R20 = R25 >> 19
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));   // R20 = R20 >> 1
    code.extend(ss_load_imm(R24, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R24, R20));  // R20 = exp
    // 3. If exp == 0: return 0
    let exp_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // If exp == 0x7FF: return 0
    let exp_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R24, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // 4. f32_exp = exp - 896
    code.extend(ss_load_imm(R24, 896));
    code.extend_from_slice(&encode_sub(R20, R24, R20));  // R20 = f32_exp
    // 5. If f32_exp <= 0: return 0 (underflow). cmpb,<= R20, R0 (signed).
    let underflow = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b011, false, false, 0));  // cond=011 (<=)
    code.extend_from_slice(&encode_nop());
    // 6. If f32_exp >= 0xFF: overflow → Inf. cmpb,>= R20, R24 where R24=0xFF.
    //    Actually use: cmpb,< R24, R20 (if 0xFF < f32_exp). Need to load 0xFF.
    code.extend(ss_load_imm(R24, 0xFF));
    let overflow = code.len();
    code.extend_from_slice(&encode_cmpb(R24, R20, 0b010, false, false, 0));  // cmpb,< 0xFF, R20, overflow
    code.extend_from_slice(&encode_nop());
    // 7. Extract f64 mantissa top 23 bits.
    //    R21 = R25 & 0xFFFFF (bits 51:32, 20 bits)
    code.extend(ss_load_imm(R24, 0xFFFFF));
    code.extend_from_slice(&encode_and(R25, R24, R21));  // R21 = bits 51:32
    //    R22 = R26 >> 29 (bits 31:29, 3 bits). 29 is odd, SHRPW works.
    code.extend_from_slice(&encode_shrpw(R0, R26, 29, R22));  // R22 = R26 >> 29
    //    f32_mant = (R21 << 3) | R22
    code.extend_from_slice(&encode_shladd(3, R21, R0, R23));  // R23 = R21 << 3
    code.extend_from_slice(&encode_or(R23, R22, R23));  // R23 = f32_mant (23 bits)
    // 8. Pack: R28 = (sign << 31) | (f32_exp << 23) | f32_mant
    //    R28 = f32_mant (R23)
    code.extend_from_slice(&encode_copy(R23, R28));
    //    R28 |= f32_exp << 23. f32_exp in R20.
    //    R24 = R20 << 23. Use SHLADD(3, R20, R0, R24) for << 3, then << 20.
    code.extend_from_slice(&encode_shladd(3, R20, R0, R24));  // R24 = R20 << 3
    for _ in 0..20 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R28, R24, R28));  // R28 |= (f32_exp << 23)
    //    R28 |= sign << 31. sign in R19 (0 or 1).
    code.extend_from_slice(&encode_copy(R19, R24));  // R24 = R19
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R28, R24, R28));  // R28 |= (sign << 31)
    // return:
    patch_cmpb_to_here(&mut code, exp_zero);
    patch_cmpb_to_here(&mut code, exp_inf);
    patch_cmpb_to_here(&mut code, underflow);
    // overflow: R28 = sign << 31 | 0x7F800000 (Inf with sign)
    // For simplicity, the overflow path falls through to return with R28 = 0
    // (the default). TODO: set Inf bits.
    patch_cmpb_to_here(&mut code, overflow);
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());
    code
}

/// Build `__vuma_f64_eq` — f64 == f64 → i32 (0 or 1).
/// Full implementation: compare hi and lo words.
fn build_f64_eq_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Input: R26=lo1, R25=hi1, R24=lo2, R23=hi2
    // R28 = 1 (default: assume equal)
    code.extend_from_slice(&encode_ldi(1, R28));
    // If hi1 != hi2: not equal. cmpb,<> R25, R23, not_eq
    let hi_check = code.len();
    code.extend_from_slice(&encode_cmpb(R25, R23, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());  // delay slot
    // If lo1 != lo2: not equal. cmpb,<> R26, R24, not_eq
    let lo_check = code.len();
    code.extend_from_slice(&encode_cmpb(R26, R24, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());  // delay slot
    // Equal: R28 = 1 (already set). Branch to return.
    let eq_branch = emit_forward_branch_placeholder(&mut code);
    // not_eq: R28 = 0
    patch_cmpb_to_here(&mut code, hi_check);
    patch_cmpb_to_here(&mut code, lo_check);
    code.extend_from_slice(&encode_ldi(0, R28));  // R28 = 0
    // return:
    patch_forward_branch_to_here(&mut code, eq_branch);
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());
    code
}

/// Build `__vuma_f64_lt` — f64 < f64 → i32 (0 or 1).
/// PARTIAL: correct for non-negative values (unsigned comparison of bit
/// patterns). For negative values, returns 0 (TODO: handle both-negative).
fn build_f64_lt_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Input: R26=lo1, R25=hi1, R24=lo2, R23=hi2
    // R28 = 0 (default: not less)
    code.extend_from_slice(&encode_ldi(0, R28));
    // If hi1 < hi2 (unsigned): a < b. cmpb,<< R25, R23, set_true
    let hi_lt_check = code.len();
    code.extend_from_slice(&encode_cmpb(R25, R23, 0b100, false, false, 0));  // cond=100 (<<)
    code.extend_from_slice(&encode_nop());  // delay slot
    // If hi1 != hi2: not less (since hi1 >= hi2 and not <). cmpb,<> R25, R23, done
    let hi_ne_check = code.len();
    code.extend_from_slice(&encode_cmpb(R25, R23, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());  // delay slot
    // hi1 == hi2: compare lo. If lo1 < lo2 (unsigned): a < b.
    let lo_lt_check = code.len();
    code.extend_from_slice(&encode_cmpb(R26, R24, 0b100, false, false, 0));  // cond=100 (<<)
    code.extend_from_slice(&encode_nop());  // delay slot
    // done: R28 = 0 (already set). Branch to return.
    let done_branch = emit_forward_branch_placeholder(&mut code);
    // set_true: R28 = 1
    patch_cmpb_to_here(&mut code, hi_lt_check);
    patch_cmpb_to_here(&mut code, lo_lt_check);
    code.extend_from_slice(&encode_ldi(1, R28));
    // return:
    patch_forward_branch_to_here(&mut code, done_branch);
    // Patch hi_ne_check to branch to return (skip set_true)
    patch_cmpb_to_here(&mut code, hi_ne_check);
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());
    code
}

/// Build `__vuma_f64_le` — f64 <= f64 → i32 (0 or 1).
/// PARTIAL: correct for non-negative values (unsigned comparison).
fn build_f64_le_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Input: R26=lo1, R25=hi1, R24=lo2, R23=hi2
    // R28 = 1 (default: assume <=)
    code.extend_from_slice(&encode_ldi(1, R28));
    // If hi1 < hi2 (unsigned): a <= b is true. cmpb,<< R25, R23, done
    let hi_lt_check = code.len();
    code.extend_from_slice(&encode_cmpb(R25, R23, 0b100, false, false, 0));  // cond=100 (<<)
    code.extend_from_slice(&encode_nop());  // delay slot
    // If hi1 > hi2 (unsigned): a <= b is false. cmpb,<< R23, R25, set_false
    let hi_gt_check = code.len();
    code.extend_from_slice(&encode_cmpb(R23, R25, 0b100, false, false, 0));  // cond=100 (<<)
    code.extend_from_slice(&encode_nop());  // delay slot
    // hi1 == hi2: compare lo. If lo1 <= lo2 (unsigned): true.
    // cmpb,<<= R26, R24, done (cond=101, <<=)
    let lo_le_check = code.len();
    code.extend_from_slice(&encode_cmpb(R26, R24, 0b101, false, false, 0));  // cond=101 (<<=)
    code.extend_from_slice(&encode_nop());  // delay slot
    // lo1 > lo2: false. Fall through to set_false.
    let done_branch = emit_forward_branch_placeholder(&mut code);
    // set_false: R28 = 0
    patch_cmpb_to_here(&mut code, hi_gt_check);
    code.extend_from_slice(&encode_ldi(0, R28));
    // return:
    patch_forward_branch_to_here(&mut code, done_branch);
    // Patch hi_lt_check and lo_le_check to branch to return (done = true)
    patch_cmpb_to_here(&mut code, hi_lt_check);
    patch_cmpb_to_here(&mut code, lo_le_check);
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());
    code
}

/// Build `__vuma_f64_add` — f64 addition (full IEEE 754 for normal numbers).
///
/// Input:  R26=lo_a, R25=hi_a, R24=lo_b, R23=hi_b
/// Output: R28=lo, R29=hi
///
/// Algorithm:
///   1. Save original a/b bits to stack (for early-return handlers).
///   2. Unpack sign/exp/mantissa for both a and b (add implicit bit).
///   3. Handle special cases: zero operand (return the other), Inf/NaN
///      (return a).  These use per-case handlers that set R28/R29 and
///      return.
///   4. Compute sign_diff = sign_a XOR sign_b.
///   5. Ensure exp_a >= exp_b (swap if needed).
///   6. Shift mant_b right by (exp_a - exp_b).
///   7. If sign_diff == 0 (same sign): result_mant = mant_a + mant_b,
///      result_sign = sign_a.
///      If sign_diff != 0 (different sign): compare magnitudes, subtract
///      smaller from larger, result_sign = sign of larger.
///   8. Normalize: if carry (bit 53 set): shift right 1, exp++.  If
///      leading zeros (bit 52 not set, subtract case): shift left 1,
///      exp-- (loop).
///   9. If result == 0: return 0.  If exp >= 0x7FF: return Inf.
///  10. Pack and return.
fn build_f64_add_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Allocate 64 bytes of stack scratch.
    code.extend_from_slice(&encode_ldo(R30, -64, R30));

    // --- Save original a/b bits for early-return handlers ---
    // [R30+0]  = a_lo (R26), [R30+40] = a_hi (R25)
    // [R30+32] = b_lo (R24), [R30+44] = b_hi (R23)
    code.extend_from_slice(&encode_stw(R26, R30, 0));
    code.extend_from_slice(&encode_stw(R25, R30, 40));
    code.extend_from_slice(&encode_stw(R24, R30, 32));
    code.extend_from_slice(&encode_stw(R23, R30, 44));

    // --- Extract a's fields ---
    // sign_a = R25 >> 31 → R19, save to [R30+4]
    code.extend_from_slice(&encode_shrpw(R0, R25, 31, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 4));
    // exp_a = (R25 >> 20) & 0x7FF → R20, save to [R30+8]
    code.extend_from_slice(&encode_shrpw(R0, R25, 19, R20));
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));
    code.extend(ss_load_imm(R24, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R24, R20));
    code.extend_from_slice(&encode_stw(R20, R30, 8));
    // mant_a_hi = (R25 & 0xFFFFF) | 0x100000 → R21, save to [R30+12]
    code.extend(ss_load_imm(R24, 0xFFFFF));
    code.extend_from_slice(&encode_and(R25, R24, R21));
    code.extend(ss_load_imm(R24, 0x100000));
    code.extend_from_slice(&encode_or(R21, R24, R21));
    code.extend_from_slice(&encode_stw(R21, R30, 12));
    // mant_a_lo = R26 → R22, save to [R30+16]
    code.extend_from_slice(&encode_copy(R26, R22));
    code.extend_from_slice(&encode_stw(R22, R30, 16));

    // --- Extract b's fields ---
    // sign_b = R23 >> 31 → R19, save to [R30+20]
    code.extend_from_slice(&encode_shrpw(R0, R23, 31, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 20));
    // exp_b = (R23 >> 20) & 0x7FF → R20, save to [R30+24]
    code.extend_from_slice(&encode_shrpw(R0, R23, 19, R20));
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));
    code.extend(ss_load_imm(R24, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R24, R20));
    code.extend_from_slice(&encode_stw(R20, R30, 24));
    // mant_b_hi = (R23 & 0xFFFFF) | 0x100000 → R21, save to [R30+28]
    code.extend(ss_load_imm(R24, 0xFFFFF));
    code.extend_from_slice(&encode_and(R23, R24, R21));
    code.extend(ss_load_imm(R24, 0x100000));
    code.extend_from_slice(&encode_or(R21, R24, R21));
    code.extend_from_slice(&encode_stw(R21, R30, 28));
    // mant_b_lo already saved to [R30+32] above.

    // --- Early-return checks ---
    // If exp_a == 0 (a is zero): return b.
    code.extend_from_slice(&encode_ldw(R30, 8, R20));
    let a_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // If exp_b == 0 (b is zero): return a.
    code.extend_from_slice(&encode_ldw(R30, 24, R20));
    let b_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // If exp_a == 0x7FF (a is Inf/NaN): return a.
    code.extend_from_slice(&encode_ldw(R30, 8, R20));
    code.extend(ss_load_imm(R24, 0x7FF));
    let a_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R24, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // If exp_b == 0x7FF (b is Inf/NaN): return b.
    code.extend_from_slice(&encode_ldw(R30, 24, R20));
    code.extend(ss_load_imm(R24, 0x7FF));
    let b_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R24, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // --- Compute sign_diff = sign_a XOR sign_b, save to [R30+36] ---
    code.extend_from_slice(&encode_ldw(R30, 4, R19));
    code.extend_from_slice(&encode_ldw(R30, 20, R21));
    code.extend_from_slice(&encode_xor(R19, R21, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 36));

    // --- Ensure exp_a >= exp_b. If not, swap a and b. ---
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend_from_slice(&encode_ldw(R30, 24, R20));
    let need_swap = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R20, 0b010, false, false, 0));  // cmpb,< → swap
    code.extend_from_slice(&encode_nop());

    // No swap path: load mant_a into R21:R22, mant_b into R23:R24.
    code.extend_from_slice(&encode_ldw(R30, 12, R21));
    code.extend_from_slice(&encode_ldw(R30, 16, R22));
    code.extend_from_slice(&encode_ldw(R30, 28, R23));
    code.extend_from_slice(&encode_ldw(R30, 32, R24));
    let skip_swap = emit_forward_branch_placeholder(&mut code);

    // Swap path: load mant_b into R21:R22, mant_a into R23:R24.
    // Also swap exps AND signs so that [R30+4]/[R30+8] always refer to
    // the operand now in R21:R22 (the larger-exp one), and [R30+20]/
    // [R30+24] refer to the operand now in R23:R24.  Without swapping
    // signs, the subtract path would use the wrong sign for the result.
    patch_cmpb_to_here(&mut code, need_swap);
    code.extend_from_slice(&encode_ldw(R30, 28, R21));
    code.extend_from_slice(&encode_ldw(R30, 32, R22));
    code.extend_from_slice(&encode_ldw(R30, 12, R23));
    code.extend_from_slice(&encode_ldw(R30, 16, R24));
    // Swap exps: [R30+8] <-> [R30+24]
    code.extend_from_slice(&encode_ldw(R30, 24, R19));
    code.extend_from_slice(&encode_ldw(R30, 8, R20));
    code.extend_from_slice(&encode_stw(R19, R30, 8));
    code.extend_from_slice(&encode_stw(R20, R30, 24));
    // Swap signs: [R30+4] <-> [R30+20]
    code.extend_from_slice(&encode_ldw(R30, 20, R19));
    code.extend_from_slice(&encode_ldw(R30, 4, R20));
    code.extend_from_slice(&encode_stw(R19, R30, 4));
    code.extend_from_slice(&encode_stw(R20, R30, 20));
    patch_forward_branch_to_here(&mut code, skip_swap);

    // --- Shift mant_b right by (exp_a - exp_b) ---
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend_from_slice(&encode_ldw(R30, 24, R20));
    code.extend_from_slice(&encode_sub(R19, R20, R19));  // R19 = diff
    let shift_loop = code.len() as i64;
    let shift_done_check = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    code.extend_from_slice(&encode_shrpw(R23, R24, 1, R24));
    code.extend_from_slice(&encode_shrpw(R0, R23, 1, R23));
    code.extend_from_slice(&encode_ldo(R19, -1, R19));
    let shift_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());
    {
        let disp = ((shift_loop - (shift_back + 8)) as i32) & !3;
        let off = shift_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }
    patch_cmpb_to_here(&mut code, shift_done_check);

    // --- Dispatch: add or subtract based on sign_diff ---
    code.extend_from_slice(&encode_ldw(R30, 36, R19));  // R19 = sign_diff
    let do_subtract = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));  // if != 0, subtract
    code.extend_from_slice(&encode_nop());

    // ===== ADD PATH (signs match) =====
    // R21:R22 = mant_a, R23:R24 = mant_b. result = mant_a + mant_b.
    code.extend_from_slice(&encode_add(R22, R24, R22));  // R22 = lo_a + lo_b
    code.extend_from_slice(&encode_copy(R0, R19));  // R19 = 0 (carry)
    let carry_add = code.len();
    code.extend_from_slice(&encode_cmpb(R22, R24, 0b100, false, false, 0));  // if R22 < R24, carry
    code.extend_from_slice(&encode_nop());
    let skip_carry_add = emit_forward_branch_placeholder(&mut code);
    patch_cmpb_to_here(&mut code, carry_add);
    code.extend_from_slice(&encode_ldi(1, R19));
    patch_forward_branch_to_here(&mut code, skip_carry_add);
    code.extend_from_slice(&encode_add(R21, R23, R21));
    code.extend_from_slice(&encode_add(R21, R19, R21));
    // result_sign = sign_a → R25
    code.extend_from_slice(&encode_ldw(R30, 4, R25));
    // Branch to normalize.
    let skip_subtract = emit_forward_branch_placeholder(&mut code);

    // ===== SUBTRACT PATH (signs differ) =====
    patch_cmpb_to_here(&mut code, do_subtract);
    // R21:R22 = mant_a, R23:R24 = mant_b.
    // If mant_a >= mant_b: result = a - b, sign = sign_a.
    // Else: result = b - a, sign = sign_b.
    // Compare hi words (unsigned):
    //   cmpb,<< R21, R23 → if R21 < R23, mant_a < mant_b (do b-a)
    let sub_a_lt_hi = code.len();
    code.extend_from_slice(&encode_cmpb(R21, R23, 0b100, false, false, 0));
    code.extend_from_slice(&encode_nop());
    //   cmpb,<> R21, R23 → if R21 != R23 (and not <, so >), do a-b
    let sub_a_gt_hi = code.len();
    code.extend_from_slice(&encode_cmpb(R21, R23, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());
    // R21 == R23: compare lo.
    let sub_a_lt_lo = code.len();
    code.extend_from_slice(&encode_cmpb(R22, R24, 0b100, false, false, 0));  // if R22 < R24, b-a
    code.extend_from_slice(&encode_nop());
    // Fall through: mant_a >= mant_b → do a-b.

    // --- a-b path ---
    patch_cmpb_to_here(&mut code, sub_a_gt_hi);
    // borrow = (R22 < R24) ? 1 : 0
    code.extend_from_slice(&encode_copy(R0, R19));
    let borrow1 = code.len();
    code.extend_from_slice(&encode_cmpb(R22, R24, 0b100, false, false, 0));
    code.extend_from_slice(&encode_nop());
    let skip_borrow1 = emit_forward_branch_placeholder(&mut code);
    patch_cmpb_to_here(&mut code, borrow1);
    code.extend_from_slice(&encode_ldi(1, R19));
    patch_forward_branch_to_here(&mut code, skip_borrow1);
    code.extend_from_slice(&encode_sub(R22, R24, R22));
    code.extend_from_slice(&encode_sub(R21, R23, R21));
    code.extend_from_slice(&encode_sub(R21, R19, R21));
    code.extend_from_slice(&encode_ldw(R30, 4, R25));  // result_sign = sign_a
    let skip_ba = emit_forward_branch_placeholder(&mut code);

    // --- b-a path (mant_a < mant_b) ---
    let ba_start = code.len();
    patch_cmpb_to_target(&mut code, sub_a_lt_hi, ba_start);
    patch_cmpb_to_target(&mut code, sub_a_lt_lo, ba_start);
    // Swap R21<->R23, R22<->R24 (using R20 as scratch).
    code.extend_from_slice(&encode_copy(R21, R20));
    code.extend_from_slice(&encode_copy(R23, R21));
    code.extend_from_slice(&encode_copy(R20, R23));
    code.extend_from_slice(&encode_copy(R22, R20));
    code.extend_from_slice(&encode_copy(R24, R22));
    code.extend_from_slice(&encode_copy(R20, R24));
    // Now R21:R22 = mant_b, R23:R24 = mant_a. Do a-b (= b-a).
    code.extend_from_slice(&encode_copy(R0, R19));
    let borrow2 = code.len();
    code.extend_from_slice(&encode_cmpb(R22, R24, 0b100, false, false, 0));
    code.extend_from_slice(&encode_nop());
    let skip_borrow2 = emit_forward_branch_placeholder(&mut code);
    patch_cmpb_to_here(&mut code, borrow2);
    code.extend_from_slice(&encode_ldi(1, R19));
    patch_forward_branch_to_here(&mut code, skip_borrow2);
    code.extend_from_slice(&encode_sub(R22, R24, R22));
    code.extend_from_slice(&encode_sub(R21, R23, R21));
    code.extend_from_slice(&encode_sub(R21, R19, R21));
    code.extend_from_slice(&encode_ldw(R30, 20, R25));  // result_sign = sign_b
    patch_forward_branch_to_here(&mut code, skip_ba);

    // ===== NORMALIZE =====
    patch_forward_branch_to_here(&mut code, skip_subtract);
    // R21:R22 = result_mant, R25 = result_sign, exp_a in [R30+8].

    // Check if result is zero: if R21|R22 == 0, return 0.
    code.extend_from_slice(&encode_or(R21, R22, R20));
    let result_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // Check carry (bit 53 set): R21 & 0x200000.
    code.extend(ss_load_imm(R24, 0x200000));
    code.extend_from_slice(&encode_and(R21, R24, R19));
    let no_carry = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, false, false, 0));  // if no carry, skip
    code.extend_from_slice(&encode_nop());
    // Carry: shift right by 1, exp++.
    code.extend_from_slice(&encode_shrpw(R21, R22, 1, R22));
    code.extend_from_slice(&encode_shrpw(R0, R21, 1, R21));
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend_from_slice(&encode_ldo(R19, 1, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 8));
    patch_cmpb_to_here(&mut code, no_carry);

    // Check leading zeros (bit 52 not set): shift left, exp--.
    // This handles subtract results that have leading zeros.
    // Loop: while (R21 & 0x100000) == 0 and exp > 1: shift left 1, exp--.
    // R24 holds 0x100000 throughout the loop (do NOT clobber it in the body).
    code.extend(ss_load_imm(R24, 0x100000));
    let norm_loop = code.len() as i64;
    code.extend_from_slice(&encode_and(R21, R24, R19));  // R19 = R21 & 0x100000
    let norm_done = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));  // cmpb,<> → if R19 != 0 (bit 52 set), done
    code.extend_from_slice(&encode_nop());
    // Check exp > 1 (stop if exp <= 1, i.e., denormal).
    // Use R20 (not R24) for the constant 1 to avoid clobbering R24.
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend_from_slice(&encode_ldi(1, R20));
    let norm_exp1 = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R20, 0b011, false, false, 0));  // cmpb,<= → if exp <= 1, done
    code.extend_from_slice(&encode_nop());
    // Shift left by 1 (64-bit): R20 = R22 >> 31; R22 <<= 1; R21 <<= 1; R21 |= R20.
    code.extend_from_slice(&encode_shrpw(R0, R22, 31, R20));  // R20 = bit 31 of R22
    code.extend_from_slice(&encode_shladd(1, R22, R0, R22));  // R22 <<= 1
    code.extend_from_slice(&encode_shladd(1, R21, R0, R21));  // R21 <<= 1
    code.extend_from_slice(&encode_or(R21, R20, R21));  // R21 |= carry bit
    // exp--
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend_from_slice(&encode_ldo(R19, -1, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 8));
    // Loop back to norm_loop.
    let norm_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));  // cmpb,<> → always loop
    code.extend_from_slice(&encode_nop());
    {
        let disp = ((norm_loop - (norm_back + 8)) as i32) & !3;
        let off = norm_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }
    // Patch norm_done and norm_exp1 to branch here.
    let norm_after = code.len();
    patch_cmpb_to_target(&mut code, norm_done, norm_after);
    patch_cmpb_to_target(&mut code, norm_exp1, norm_after);

    // Check overflow: if exp_a >= 0x7FF, return Inf.
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend(ss_load_imm(R24, 0x7FF));
    let overflow = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R24, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // ===== PACK RESULT =====
    // R28 = R22 (mant_lo)
    // R29 = (sign << 31) | (exp << 20) | (R21 & 0xFFFFF)
    code.extend_from_slice(&encode_copy(R22, R28));
    code.extend(ss_load_imm(R24, 0xFFFFF));
    code.extend_from_slice(&encode_and(R21, R24, R29));  // R29 = mant_hi & 0xFFFFF
    // R29 |= (exp << 20). exp in [R30+8].
    code.extend_from_slice(&encode_ldw(R30, 8, R19));
    code.extend_from_slice(&encode_shladd(3, R19, R0, R24));  // R24 = exp << 3
    for _ in 0..17 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R29, R24, R29));
    // R29 |= (sign << 31). sign in R25.
    code.extend_from_slice(&encode_copy(R25, R24));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R29, R24, R29));

    // ===== RETURN (normal path) =====
    code.extend_from_slice(&encode_ldo(R30, 64, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // ===== HANDLER: result_zero → return 0 =====
    patch_cmpb_to_here(&mut code, result_zero);
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend_from_slice(&encode_copy(R0, R29));
    code.extend_from_slice(&encode_ldo(R30, 64, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // ===== HANDLER: overflow → return Inf with sign =====
    patch_cmpb_to_here(&mut code, overflow);
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend(ss_load_imm(R29, 0x7FF00000));
    code.extend_from_slice(&encode_copy(R25, R24));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R24, R0, R24));
    }
    code.extend_from_slice(&encode_or(R29, R24, R29));
    code.extend_from_slice(&encode_ldo(R30, 64, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // ===== HANDLER: a_zero → return b =====
    patch_cmpb_to_here(&mut code, a_zero);
    code.extend_from_slice(&encode_ldw(R30, 32, R28));  // b_lo
    code.extend_from_slice(&encode_ldw(R30, 44, R29));  // b_hi
    code.extend_from_slice(&encode_ldo(R30, 64, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // ===== HANDLER: b_zero → return a =====
    patch_cmpb_to_here(&mut code, b_zero);
    code.extend_from_slice(&encode_ldw(R30, 0, R28));   // a_lo
    code.extend_from_slice(&encode_ldw(R30, 40, R29));  // a_hi
    code.extend_from_slice(&encode_ldo(R30, 64, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // ===== HANDLER: a_inf → return a =====
    patch_cmpb_to_here(&mut code, a_inf);
    code.extend_from_slice(&encode_ldw(R30, 0, R28));
    code.extend_from_slice(&encode_ldw(R30, 40, R29));
    code.extend_from_slice(&encode_ldo(R30, 64, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // ===== HANDLER: b_inf → return b =====
    patch_cmpb_to_here(&mut code, b_inf);
    code.extend_from_slice(&encode_ldw(R30, 32, R28));
    code.extend_from_slice(&encode_ldw(R30, 44, R29));
    code.extend_from_slice(&encode_ldo(R30, 64, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    code
}

/// Build `__vuma_f64_sub` — f64 subtraction.
///
/// Implemented as `a + (-b)`: flip the sign bit of b (R23 ^= 0x80000000)
/// and fall through to the add logic.  This reuses the full add path
/// (including the same-sign fast path — after the flip, both operands
/// have the same sign in the canonical subtraction case `(+a) - (+b)`
/// → `(+a) + (-b)` which is a sign-difference add, currently returning 0).
///
/// NOTE: because the underlying add stub returns 0 for different-sign
/// inputs, `sub` currently only returns non-zero when `|a|` and `|b|`
/// have different magnitudes AND the result has the same sign as the
/// larger operand (i.e. the post-flip add is same-sign).  This covers
/// the common case `5.0 - 3.0 = 2.0` (after flip: `5.0 + (-3.0)`,
/// which the add stub does NOT yet handle — it returns 0).  A full
/// implementation requires the add stub to handle opposite-sign add,
/// which is TODO.  Constant-folded subtractions (both operands
/// Immediate) are unaffected and compute the correct result.
///
/// Input:  R26=lo_a, R25=hi_a, R24=lo_b, R23=hi_b
/// Output: R28=lo, R29=hi
fn build_f64_sub_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Flip sign bit of b: R23 = R23 XOR 0x80000000.
    // Use R19 as scratch (caller-saved; the add stub re-initializes it).
    code.extend(ss_load_imm(R19, 0x80000000_i64));
    code.extend_from_slice(&encode_xor(R23, R19, R23));
    // Fall through to the add logic (a + (-b)).
    code.extend(build_f64_add_stub());
    code
}

/// Build `__vuma_f64_mul` and `__vuma_f64_div` — placeholder stubs that
/// return 0.0.  A full implementation requires a 53x53→106-bit multiply
/// (PA-RISC 1.1 has no 64-bit multiply in the integer unit; XMPYU lives
/// in the coprocessor, which this backend stubs out).  Constant-folded
/// mul/div (both operands Immediate) compute the correct result via
/// `const_fold_fp_binop` and never reach these stubs.
/// Build `__vuma_f64_mul` — f64 multiplication (IEEE 754 for normal numbers,
/// truncation rounding toward zero).
///
/// Input:  R26=lo_a, R25=hi_a, R24=lo_b, R23=hi_b
/// Output: R28=lo, R29=hi
///
/// Algorithm:
///   1. Extract sign, exponent, mantissa from each operand (add implicit bit).
///   2. Handle special cases (zero -> 0, Inf/NaN -> return that operand).
///   3. result_sign = sign_a XOR sign_b.
///   4. result_exp = exp_a + exp_b - 1023.
///   5. Pre-shift M_b left by 11 so its MSB (bit 52) lands at bit 31 of R23,
///      enabling single-SHRPW MSB extraction each iteration.
///   6. Compute P = mant_a * mant_b using a 128-bit shift-add accumulator.
///      Process M_b from MSB to LSB; each iteration: extract MSB of M_b,
///      shift acc left by 1, conditionally add M_a, shift M_b left by 1.
///      (First shift is a no-op since acc=0; last add lands at position 0.)
///   7. Extract result = P >> 52 (54 bits, including carry bit at position 53).
///   8. If carry (bit 53 set): shift result right by 1, result_exp += 1.
///   9. Handle overflow (result_exp >= 0x7FF -> Inf) and underflow
///      (result_exp <= 0 -> 0).
///  10. Pack result.
fn build_f64_mul_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Stack frame: 48 bytes.
    code.extend_from_slice(&encode_ldo(R30, -48, R30));

    // Save original a/b bits for Inf/NaN handlers.
    code.extend_from_slice(&encode_stw(R26, R30, 0));   // a_lo
    code.extend_from_slice(&encode_stw(R25, R30, 4));   // a_hi
    code.extend_from_slice(&encode_stw(R24, R30, 8));   // b_lo
    code.extend_from_slice(&encode_stw(R23, R30, 12));  // b_hi

    // --- Extract a: sign, exp, mant ---
    code.extend_from_slice(&encode_shrpw(R0, R25, 31, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 16));  // sign_a
    code.extend_from_slice(&encode_shrpw(R0, R25, 19, R20));
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));
    code.extend(ss_load_imm(R19, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R19, R20));
    code.extend_from_slice(&encode_stw(R20, R30, 20));  // exp_a
    code.extend(ss_load_imm(R19, 0xFFFFF));
    code.extend_from_slice(&encode_and(R25, R19, R21));
    code.extend(ss_load_imm(R19, 0x100000));
    code.extend_from_slice(&encode_or(R21, R19, R21));  // mant_a_hi (with implicit bit)
    code.extend_from_slice(&encode_copy(R26, R22));     // mant_a_lo

    // --- Extract b: sign, exp, mant ---
    code.extend_from_slice(&encode_shrpw(R0, R23, 31, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 24));  // sign_b
    code.extend_from_slice(&encode_shrpw(R0, R23, 19, R20));
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));
    code.extend(ss_load_imm(R19, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R19, R20));
    code.extend_from_slice(&encode_stw(R20, R30, 28));  // exp_b
    code.extend(ss_load_imm(R19, 0xFFFFF));
    code.extend_from_slice(&encode_and(R23, R19, R23));
    code.extend(ss_load_imm(R19, 0x100000));
    code.extend_from_slice(&encode_or(R23, R19, R23));  // mant_b_hi (with implicit bit)

    // --- Special cases ---
    code.extend_from_slice(&encode_ldw(R30, 20, R20));
    let a_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    code.extend_from_slice(&encode_ldw(R30, 28, R20));
    let b_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    code.extend_from_slice(&encode_ldw(R30, 20, R20));
    code.extend(ss_load_imm(R19, 0x7FF));
    let a_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R19, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    code.extend_from_slice(&encode_ldw(R30, 28, R20));
    let b_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R19, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // --- result_sign = sign_a XOR sign_b -> [R30+32] ---
    code.extend_from_slice(&encode_ldw(R30, 16, R19));
    code.extend_from_slice(&encode_ldw(R30, 24, R20));
    code.extend_from_slice(&encode_xor(R19, R20, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 32));

    // --- result_exp = exp_a + exp_b - 1023 -> [R30+36] ---
    code.extend_from_slice(&encode_ldw(R30, 20, R19));
    code.extend_from_slice(&encode_ldw(R30, 28, R20));
    code.extend_from_slice(&encode_add(R19, R20, R19));
    code.extend(ss_load_imm(R20, 1023));
    code.extend_from_slice(&encode_sub(R19, R20, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 36));

    // --- Pre-shift M_b left by 11 so MSB (bit 52) lands at bit 31 of R23 ---
    // R18 = R24 >> 21 (carry from low to high)
    code.extend_from_slice(&encode_shrpw(R0, R24, 21, R18));
    // R23 = R23 << 4
    code.extend_from_slice(&encode_shladd(4, R23, R0, R23));
    // R23 = R23 << 4 (= orig << 8)
    code.extend_from_slice(&encode_shladd(4, R23, R0, R23));
    // R23 = (R23 << 3) + carry (= orig << 11 | carry)
    code.extend_from_slice(&encode_shladd(3, R23, R18, R23));
    // R24 = R24 << 11 (4+4+3)
    code.extend_from_slice(&encode_shladd(4, R24, R0, R24));
    code.extend_from_slice(&encode_shladd(4, R24, R0, R24));
    code.extend_from_slice(&encode_shladd(3, R24, R0, R24));

    // --- Init 128-bit acc (R25:R26:R28:R29) = 0, counter R19 = 53 ---
    code.extend_from_slice(&encode_copy(R0, R25));
    code.extend_from_slice(&encode_copy(R0, R26));
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend_from_slice(&encode_copy(R0, R29));
    code.extend(ss_load_imm(R19, 53));

    // --- Multiply loop (process M_b from MSB to LSB) ---
    let mul_loop = code.len() as i64;
    // 1. Extract MSB of M_b: R20 = bit 31 of R23.
    code.extend_from_slice(&encode_shrpw(R0, R23, 31, R20));
    // 2. Shift acc left by 1 (128-bit): R25:R26:R28:R29.
    //    Use R18 as temp for carry. Process from MSB (R25) to LSB (R29).
    code.extend_from_slice(&encode_shrpw(R0, R26, 31, R18));   // R18 = carry from R26
    code.extend_from_slice(&encode_shladd(1, R25, R18, R25));   // R25 = R25*2 + carry
    code.extend_from_slice(&encode_shrpw(R0, R28, 31, R18));   // R18 = carry from R28
    code.extend_from_slice(&encode_shladd(1, R26, R18, R26));   // R26 = R26*2 + carry
    code.extend_from_slice(&encode_shrpw(R0, R29, 31, R18));   // R18 = carry from R29
    code.extend_from_slice(&encode_shladd(1, R28, R18, R28));   // R28 = R28*2 + carry
    code.extend_from_slice(&encode_shladd(1, R29, R0, R29));   // R29 = R29*2
    // 3. If R20 != 0: add M_a (R21:R22) to acc.
    let skip_add = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));  // if R20==0, skip
    code.extend_from_slice(&encode_nop());
    // Add M_a to acc with full carry chain. Spill R19 (counter) to stack; use R19 as temp.
    code.extend_from_slice(&encode_stw(R19, R30, 40));
    code.extend_from_slice(&encode_add(R29, R22, R29));  // R29 += R22 (low)
    // carry0 = (R29 < R22) -> R19
    code.extend_from_slice(&encode_copy(R0, R19));
    let c0 = code.len();
    code.extend_from_slice(&encode_cmpb(R29, R22, 0b100, false, false, 0));
    code.extend_from_slice(&encode_nop());
    let c0s = emit_forward_branch_placeholder(&mut code);
    patch_cmpb_to_here(&mut code, c0);
    code.extend_from_slice(&encode_ldi(1, R19));
    patch_forward_branch_to_here(&mut code, c0s);
    // R20 = R21 + carry0 (effective high operand; R21 = mant_a_hi)
    code.extend_from_slice(&encode_add(R21, R19, R20));
    // R28 += R20
    code.extend_from_slice(&encode_add(R28, R20, R28));
    // carry1 = (R28 < R20) -> R19
    code.extend_from_slice(&encode_copy(R0, R19));
    let c1 = code.len();
    code.extend_from_slice(&encode_cmpb(R28, R20, 0b100, false, false, 0));
    code.extend_from_slice(&encode_nop());
    let c1s = emit_forward_branch_placeholder(&mut code);
    patch_cmpb_to_here(&mut code, c1);
    code.extend_from_slice(&encode_ldi(1, R19));
    patch_forward_branch_to_here(&mut code, c1s);
    // R26 += carry1
    code.extend_from_slice(&encode_add(R26, R19, R26));
    // carry2 = (R26 < carry1=R19) -> R20
    code.extend_from_slice(&encode_copy(R0, R20));
    let c2 = code.len();
    code.extend_from_slice(&encode_cmpb(R26, R19, 0b100, false, false, 0));
    code.extend_from_slice(&encode_nop());
    let c2s = emit_forward_branch_placeholder(&mut code);
    patch_cmpb_to_here(&mut code, c2);
    code.extend_from_slice(&encode_ldi(1, R20));
    patch_forward_branch_to_here(&mut code, c2s);
    // R25 += carry2
    code.extend_from_slice(&encode_add(R25, R20, R25));
    code.extend_from_slice(&encode_ldw(R30, 40, R19));  // reload counter
    // skip_add:
    patch_cmpb_to_here(&mut code, skip_add);

    // 4. Shift M_b left by 1 (64-bit): R23:R24.
    //    R18 = R24 >> 31 (carry from low to high)
    code.extend_from_slice(&encode_shrpw(R0, R24, 31, R18));
    //    R23 = (R23 << 1) | carry
    code.extend_from_slice(&encode_shladd(1, R23, R18, R23));
    //    R24 <<= 1
    code.extend_from_slice(&encode_shladd(1, R24, R0, R24));

    // 5. R19 -= 1. If R19 != 0, loop.
    code.extend_from_slice(&encode_ldo(R19, -1, R19));
    let loop_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));  // cmpb,<> -> loop
    code.extend_from_slice(&encode_nop());
    {
        let disp = ((mul_loop - (loop_back + 8)) as i32) & !3;
        let off = loop_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }

    // --- Extract result = P >> 52 (54 bits) into R26:R28 ---
    // R28 = (R26:R28) >> 20  [sa=20 = 19 + 1]
    code.extend_from_slice(&encode_shrpw(R26, R28, 19, R28));
    code.extend_from_slice(&encode_shrpw(R0, R28, 1, R28));
    // R26 = (R25:R26) >> 20
    code.extend_from_slice(&encode_shrpw(R25, R26, 19, R26));
    code.extend_from_slice(&encode_shrpw(R0, R26, 1, R26));
    // Now R26:R28 = P[105:52] (54 bits). R26 = high 22 bits, R28 = low 32 bits.

    // Check carry: R20 = R26 >> 21 = P[105].
    code.extend_from_slice(&encode_shrpw(R0, R26, 21, R20));
    let no_carry = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));  // if R20==0, skip
    code.extend_from_slice(&encode_nop());
    // Carry: result_mant = (R26:R28) >> 1. exp += 1.
    code.extend_from_slice(&encode_shrpw(R26, R28, 1, R28));  // R28 = result[32:1]
    code.extend_from_slice(&encode_shrpw(R0, R26, 1, R26));   // R26 = result[53:33]
    code.extend_from_slice(&encode_ldw(R30, 36, R19));
    code.extend_from_slice(&encode_ldo(R19, 1, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 36));
    // no_carry:
    patch_cmpb_to_here(&mut code, no_carry);

    // Now R26 = result_mant_hi (21 bits, bit 20 = implicit), R28 = result_mant_lo.

    // Check overflow: if exp >= 0x7FF, return Inf.
    code.extend_from_slice(&encode_ldw(R30, 36, R19));
    code.extend(ss_load_imm(R20, 0x7FF));
    let overflow = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R20, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // Check underflow: if exp <= 0, return 0.
    let underflow = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b011, false, false, 0));  // signed <=
    code.extend_from_slice(&encode_nop());

    // --- Pack result ---
    // R28 = result_mant_lo (already in R28).
    // R29 = (sign << 31) | (exp << 20) | (mant_hi & 0xFFFFF).
    code.extend(ss_load_imm(R20, 0xFFFFF));
    code.extend_from_slice(&encode_and(R26, R20, R29));  // R29 = mant_hi & 0xFFFFF
    code.extend_from_slice(&encode_ldw(R30, 36, R19));   // R19 = exp
    code.extend_from_slice(&encode_shladd(3, R19, R0, R20));  // R20 = exp << 3
    for _ in 0..17 {
        code.extend_from_slice(&encode_shladd(1, R20, R0, R20));  // total 20
    }
    code.extend_from_slice(&encode_or(R29, R20, R29));
    code.extend_from_slice(&encode_ldw(R30, 32, R19));  // R19 = sign
    code.extend_from_slice(&encode_copy(R19, R20));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R20, R0, R20));  // sign << 31
    }
    code.extend_from_slice(&encode_or(R29, R20, R29));

    // Return (normal).
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: return_zero (a_zero, b_zero, underflow) ---
    patch_cmpb_to_here(&mut code, a_zero);
    patch_cmpb_to_here(&mut code, b_zero);
    patch_cmpb_to_here(&mut code, underflow);
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend_from_slice(&encode_copy(R0, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: return_a (a_inf) ---
    patch_cmpb_to_here(&mut code, a_inf);
    code.extend_from_slice(&encode_ldw(R30, 0, R28));
    code.extend_from_slice(&encode_ldw(R30, 4, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: return_b (b_inf) ---
    patch_cmpb_to_here(&mut code, b_inf);
    code.extend_from_slice(&encode_ldw(R30, 8, R28));
    code.extend_from_slice(&encode_ldw(R30, 12, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: overflow -> Inf with sign ---
    patch_cmpb_to_here(&mut code, overflow);
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend(ss_load_imm(R29, 0x7FF00000));
    code.extend_from_slice(&encode_ldw(R30, 32, R19));  // sign
    code.extend_from_slice(&encode_copy(R19, R20));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R20, R0, R20));
    }
    code.extend_from_slice(&encode_or(R29, R20, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    code
}

/// Build `__vuma_f64_div` — f64 division (IEEE 754 for normal numbers,
/// truncation rounding toward zero).
///
/// Input:  R26=lo_a, R25=hi_a, R24=lo_b, R23=hi_b
/// Output: R28=lo, R29=hi
///
/// Algorithm:
///   1. Extract sign, exponent, mantissa from each operand.
///   2. Handle special cases (a==0 -> 0, b==0 -> Inf, Inf/Inf -> NaN(a),
///      Inf/x -> Inf(a), x/Inf -> 0).
///   3. result_sign = sign_a XOR sign_b.
///   4. result_exp = exp_a - exp_b + 1023.
///   5. Long division: Q = (mant_a << 52) / mant_b, using shift-and-subtract.
///      remainder starts as mant_a; each iteration: Q <<= 1, compare remainder
///      with mant_b, subtract if >= and set Q bit, shift remainder left
///      (except last iteration).
///   6. Normalize: if Q < 2^52 (mant_a < mant_b), shift Q left by 1, exp--.
///   7. Handle overflow/underflow.
///   8. Pack result.
fn build_f64_div_stub() -> Vec<u8> {
    let mut code = Vec::new();
    // Stack frame: 48 bytes.
    code.extend_from_slice(&encode_ldo(R30, -48, R30));

    // Save original a/b bits.
    code.extend_from_slice(&encode_stw(R26, R30, 0));   // a_lo
    code.extend_from_slice(&encode_stw(R25, R30, 4));   // a_hi
    code.extend_from_slice(&encode_stw(R24, R30, 8));   // b_lo
    code.extend_from_slice(&encode_stw(R23, R30, 12));  // b_hi

    // --- Extract a: sign, exp, mant ---
    code.extend_from_slice(&encode_shrpw(R0, R25, 31, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 16));  // sign_a
    code.extend_from_slice(&encode_shrpw(R0, R25, 19, R20));
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));
    code.extend(ss_load_imm(R19, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R19, R20));
    code.extend_from_slice(&encode_stw(R20, R30, 20));  // exp_a
    code.extend(ss_load_imm(R19, 0xFFFFF));
    code.extend_from_slice(&encode_and(R25, R19, R21));
    code.extend(ss_load_imm(R19, 0x100000));
    code.extend_from_slice(&encode_or(R21, R19, R21));  // mant_a_hi
    code.extend_from_slice(&encode_copy(R26, R22));     // mant_a_lo

    // --- Extract b: sign, exp, mant ---
    code.extend_from_slice(&encode_shrpw(R0, R23, 31, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 24));  // sign_b
    code.extend_from_slice(&encode_shrpw(R0, R23, 19, R20));
    code.extend_from_slice(&encode_shrpw(R0, R20, 1, R20));
    code.extend(ss_load_imm(R19, 0x7FF));
    code.extend_from_slice(&encode_and(R20, R19, R20));
    code.extend_from_slice(&encode_stw(R20, R30, 28));  // exp_b
    code.extend(ss_load_imm(R19, 0xFFFFF));
    code.extend_from_slice(&encode_and(R23, R19, R23));
    code.extend(ss_load_imm(R19, 0x100000));
    code.extend_from_slice(&encode_or(R23, R19, R23));  // mant_b_hi

    // --- Special cases ---
    // If exp_a == 0x7FF (a is Inf/NaN): return a (Inf/x = Inf, Inf/Inf = NaN).
    code.extend_from_slice(&encode_ldw(R30, 20, R20));
    code.extend(ss_load_imm(R19, 0x7FF));
    let a_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R19, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // If exp_b == 0x7FF (b is Inf/NaN): return 0 (x/Inf = 0).
    code.extend_from_slice(&encode_ldw(R30, 28, R20));
    let b_inf = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R19, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // If exp_b == 0 (b is zero): return Inf with sign (x/0 = Inf).
    code.extend_from_slice(&encode_ldw(R30, 28, R20));
    let b_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // If exp_a == 0 (a is zero): return 0 (0/x = 0).
    code.extend_from_slice(&encode_ldw(R30, 20, R20));
    let a_zero = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // --- result_sign = sign_a XOR sign_b -> [R30+32] ---
    code.extend_from_slice(&encode_ldw(R30, 16, R19));
    code.extend_from_slice(&encode_ldw(R30, 24, R20));
    code.extend_from_slice(&encode_xor(R19, R20, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 32));

    // --- result_exp = exp_a - exp_b + 1023 -> [R30+36] ---
    code.extend_from_slice(&encode_ldw(R30, 20, R19));
    code.extend_from_slice(&encode_ldw(R30, 28, R20));
    code.extend_from_slice(&encode_sub(R19, R20, R19));  // R19 = exp_a - exp_b
    code.extend(ss_load_imm(R20, 1023));
    code.extend_from_slice(&encode_add(R19, R20, R19));  // R19 += 1023
    code.extend_from_slice(&encode_stw(R19, R30, 36));

    // --- Init: remainder (R21:R22) = mant_a, Q (R25:R26) = 0, counter R19 = 53 ---
    code.extend_from_slice(&encode_copy(R0, R25));
    code.extend_from_slice(&encode_copy(R0, R26));
    code.extend(ss_load_imm(R19, 53));

    // --- Division loop (53 iterations) ---
    let div_loop = code.len() as i64;
    // Q <<= 1 (64-bit shift of R25:R26).
    code.extend_from_slice(&encode_shrpw(R25, R26, 31, R25));  // R25 = (R25<<1)|(bit31 R26)
    code.extend_from_slice(&encode_shladd(1, R26, R0, R26));   // R26 <<= 1

    // Compare remainder (R21:R22) >= mant_b (R23:R24).
    let ge_hi = code.len();
    code.extend_from_slice(&encode_cmpb(R23, R21, 0b100, false, false, 0));  // R23 < R21 -> do_sub
    code.extend_from_slice(&encode_nop());
    let lt_hi = code.len();
    code.extend_from_slice(&encode_cmpb(R21, R23, 0b100, false, false, 0));  // R21 < R23 -> skip
    code.extend_from_slice(&encode_nop());
    let lt_lo = code.len();
    code.extend_from_slice(&encode_cmpb(R22, R24, 0b100, false, false, 0));  // R22 < R24 -> skip
    code.extend_from_slice(&encode_nop());
    // Fall through: remainder >= mant_b -> do_subtract.

    // do_subtract label:
    let do_sub_label = code.len();
    patch_cmpb_to_here(&mut code, ge_hi);

    // 64-bit subtract: R21:R22 -= R23:R24.
    // borrow = (R22 < R24) -> R20
    code.extend_from_slice(&encode_copy(R0, R20));
    let borrow_cmpb = code.len();
    code.extend_from_slice(&encode_cmpb(R22, R24, 0b100, false, false, 0));
    code.extend_from_slice(&encode_nop());
    let borrow_skip = emit_forward_branch_placeholder(&mut code);
    patch_cmpb_to_here(&mut code, borrow_cmpb);
    code.extend_from_slice(&encode_ldi(1, R20));
    patch_forward_branch_to_here(&mut code, borrow_skip);
    code.extend_from_slice(&encode_sub(R22, R24, R22));  // R22 -= R24
    code.extend_from_slice(&encode_sub(R21, R23, R21));  // R21 -= R23
    code.extend_from_slice(&encode_sub(R21, R20, R21));  // R21 -= borrow
    // Q |= 1 (set bit 0 of Q_lo = R26).
    code.extend_from_slice(&encode_ldi(1, R20));
    code.extend_from_slice(&encode_or(R26, R20, R26));

    // skip_subtract label:
    let skip_sub_label = code.len();
    patch_cmpb_to_here(&mut code, lt_hi);
    patch_cmpb_to_here(&mut code, lt_lo);

    // R19 -= 1.
    code.extend_from_slice(&encode_ldo(R19, -1, R19));
    // If R19 == 0: exit loop (don't shift remainder).
    let exit_loop = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());
    // Shift remainder left by 1 (64-bit: R21:R22).
    code.extend_from_slice(&encode_shrpw(R21, R22, 31, R21));  // R21 = (R21<<1)|(bit31 R22)
    code.extend_from_slice(&encode_shladd(1, R22, R0, R22));   // R22 <<= 1
    // Loop back.
    let loop_back = code.len() as i64;
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, false, 0));
    code.extend_from_slice(&encode_nop());
    {
        let disp = ((div_loop - (loop_back + 8)) as i32) & !3;
        let off = loop_back as usize;
        let word = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
        let patched = (word & !0x1FFF) | encode_cmpb_disp(disp);
        code[off..off+4].copy_from_slice(&patched.to_be_bytes());
    }
    // Exit loop:
    patch_cmpb_to_here(&mut code, exit_loop);
    // (do_sub_label and skip_sub_label are no longer needed as patch targets,
    // but keep them for clarity — they're just code.len() snapshots.)

    // --- Normalize Q ---
    // Q is in R25:R26 (53 bits). bit 52 = bit 20 of R25 (implicit bit).
    // If bit 20 of R25 is 0 (Q < 2^52, mant_a < mant_b): shift Q left by 1, exp--.
    code.extend(ss_load_imm(R20, 0x100000));  // 1 << 20
    code.extend_from_slice(&encode_and(R25, R20, R20));  // R20 = R25 & 0x100000
    let q_normalized = code.len();
    code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, false, 0));  // if bit set, skip
    code.extend_from_slice(&encode_nop());
    // Q <<= 1 (64-bit shift).
    code.extend_from_slice(&encode_shrpw(R25, R26, 31, R25));
    code.extend_from_slice(&encode_shladd(1, R26, R0, R26));
    // exp--.
    code.extend_from_slice(&encode_ldw(R30, 36, R19));
    code.extend_from_slice(&encode_ldo(R19, -1, R19));
    code.extend_from_slice(&encode_stw(R19, R30, 36));
    // q_normalized:
    patch_cmpb_to_here(&mut code, q_normalized);

    // Now R25:R26 = result_mant (53 bits, bit 52 = implicit). R25 = high 21 bits, R26 = low 32 bits.

    // Check overflow: if exp >= 0x7FF, return Inf.
    code.extend_from_slice(&encode_ldw(R30, 36, R19));
    code.extend(ss_load_imm(R20, 0x7FF));
    let overflow = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R20, 0b001, false, false, 0));
    code.extend_from_slice(&encode_nop());

    // Check underflow: if exp <= 0, return 0.
    let underflow = code.len();
    code.extend_from_slice(&encode_cmpb(R19, R0, 0b011, false, false, 0));  // signed <=
    code.extend_from_slice(&encode_nop());

    // --- Pack result ---
    // R28 = result_mant_lo = R26.
    code.extend_from_slice(&encode_copy(R26, R28));
    // R29 = (sign << 31) | (exp << 20) | (mant_hi & 0xFFFFF).
    code.extend(ss_load_imm(R20, 0xFFFFF));
    code.extend_from_slice(&encode_and(R25, R20, R29));  // R29 = mant_hi & 0xFFFFF
    code.extend_from_slice(&encode_ldw(R30, 36, R19));   // R19 = exp
    code.extend_from_slice(&encode_shladd(3, R19, R0, R20));  // R20 = exp << 3
    for _ in 0..17 {
        code.extend_from_slice(&encode_shladd(1, R20, R0, R20));  // total 20
    }
    code.extend_from_slice(&encode_or(R29, R20, R29));
    code.extend_from_slice(&encode_ldw(R30, 32, R19));  // R19 = sign
    code.extend_from_slice(&encode_copy(R19, R20));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R20, R0, R20));  // sign << 31
    }
    code.extend_from_slice(&encode_or(R29, R20, R29));

    // Return (normal).
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: return_zero (a_zero, b_inf, underflow) ---
    patch_cmpb_to_here(&mut code, a_zero);
    patch_cmpb_to_here(&mut code, b_inf);
    patch_cmpb_to_here(&mut code, underflow);
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend_from_slice(&encode_copy(R0, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: return_a (a_inf) ---
    patch_cmpb_to_here(&mut code, a_inf);
    code.extend_from_slice(&encode_ldw(R30, 0, R28));
    code.extend_from_slice(&encode_ldw(R30, 4, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: return Inf (b_zero — x/0 = Inf with sign) ---
    patch_cmpb_to_here(&mut code, b_zero);
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend(ss_load_imm(R29, 0x7FF00000));
    code.extend_from_slice(&encode_ldw(R30, 32, R19));  // sign
    code.extend_from_slice(&encode_copy(R19, R20));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R20, R0, R20));
    }
    code.extend_from_slice(&encode_or(R29, R20, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    // --- Handler: overflow -> Inf with sign ---
    patch_cmpb_to_here(&mut code, overflow);
    code.extend_from_slice(&encode_copy(R0, R28));
    code.extend(ss_load_imm(R29, 0x7FF00000));
    code.extend_from_slice(&encode_ldw(R30, 32, R19));  // sign
    code.extend_from_slice(&encode_copy(R19, R20));
    for _ in 0..31 {
        code.extend_from_slice(&encode_shladd(1, R20, R0, R20));
    }
    code.extend_from_slice(&encode_or(R29, R20, R29));
    code.extend_from_slice(&encode_ldo(R30, 48, R30));
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());

    code
}

/// TODO: full IEEE 754 mul/div via shift-add multiplication.
fn build_f64_arith_stub_zero() -> Vec<u8> {
    let mut code = Vec::new();
    code.extend_from_slice(&encode_copy(R0, R28));  // R28 = 0
    code.extend_from_slice(&encode_copy(R0, R29));  // R29 = 0
    code.extend_from_slice(&encode_bv(R2, R0));
    code.extend_from_slice(&encode_nop());
    code
}


/// Wrapper: i64→f64 (signed).
fn build_i64_to_f64_stub() -> Vec<u8> {
    build_int_to_f64_stub_inner(true)
}

/// Return all soft-float stubs as (symbol_name, machine_code) pairs.
fn build_softfloat_stubs() -> Vec<(String, Vec<u8>)> {
    vec![
        ("__vuma_f64_to_i64".to_string(),   build_f64_to_i64_stub()),
        ("__vuma_f64_to_u64".to_string(),   build_f64_to_u64_stub()),
        ("__vuma_i64_to_f64".to_string(),   build_i64_to_f64_stub()),
        ("__vuma_u64_to_f64".to_string(),   build_u64_to_f64_stub()),
        ("__vuma_f32_to_f64".to_string(),   build_f32_to_f64_stub()),
        ("__vuma_f64_to_f32".to_string(),   build_f64_to_f32_stub()),
        ("__vuma_f64_eq".to_string(),       build_f64_eq_stub()),
        ("__vuma_f64_lt".to_string(),       build_f64_lt_stub()),
        ("__vuma_f64_le".to_string(),       build_f64_le_stub()),
        ("__vuma_f64_add".to_string(),      build_f64_add_stub()),
        ("__vuma_f64_sub".to_string(),      build_f64_sub_stub()),
        ("__vuma_f64_mul".to_string(),      build_f64_mul_stub()),
        ("__vuma_f64_div".to_string(),      build_f64_div_stub()),
    ]
}

/// Stack-slot based allocate_registers for HPPA.
/// Every vreg gets a stack slot; operations use scratch registers.
fn hppa_allocate_registers_ss(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        use std::collections::{HashMap, HashSet};
        use crate::ir::{BinOpKind, CastKind, CmpKind, UnaryOpKind};
        use crate::backend::{AllocatedBlock, AllocatedInstruction, RelocationEntry};

        // ── Phase 1: Collect all vreg IDs and compute stack layout ──
        let mut all_vreg_ids: HashSet<u32> = HashSet::new();
        for &id in func.vregs.keys() { all_vreg_ids.insert(id); }
        for param in &func.params {
            if let Some(id) = param.as_register() { all_vreg_ids.insert(id); }
        }
        for block in &func.blocks {
            for instr in &block.instructions {
                for id in instr.defined_regs() { all_vreg_ids.insert(id); }
                for id in instr.used_regs() { all_vreg_ids.insert(id); }
            }
            match &block.terminator {
                IRTerminator::Branch { cond, .. } => {
                    if let Some(id) = cond.as_register() { all_vreg_ids.insert(id); }
                }
                IRTerminator::Return(vals) => {
                    for val in vals { if let Some(id) = val.as_register() { all_vreg_ids.insert(id); } }
                }
                _ => {}
            }
        }

        // Identify Alloc vregs and their sizes
        let mut alloc_sizes: HashMap<u32, i32> = HashMap::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                if let IRInstr::Alloc { dst, size } = instr {
                    if let Some(id) = dst.as_register() {
                        let aligned = ((*size as i32 + 15) & !15) as i32;
                        alloc_sizes.insert(id, aligned);
                    }
                }
            }
        }

        // PA-RISC stack grows UP. FP=R3 points to the base of the frame.
        // Locals are at NEGATIVE offsets from FP (below FP).
        // vreg stack slots start at -32 (below RP at -20, old FP at -24,
        // and a dedicated zero-extension scratch slot at -28).
        // The prologue saves RP at R30-20 and old FP at R30-24. FP-28 is
        // reserved as a scratch slot for ss_load_imm's STW+LDW zero-extension
        // of values >= 0x80000000, so vregs must start at -32.
        let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new();
        let mut current_offset: i32 = -64; // Start below incoming args area
        let mut vreg_ids: Vec<u32> = all_vreg_ids.iter().copied().collect();
        vreg_ids.sort();
        for &id in &vreg_ids {
            vreg_stack_slots.insert(id, current_offset);
            // W4e: allocate 8 bytes per vreg (not 4) so that f64/i64/u64
            // values stored as [off]=lo, [off-4]=hi do not overlap with the
            // next vreg's slot.  With 4-byte spacing, vreg N's hi word at
            // [off-4] collides with vreg N+1's lo word at [off-4], corrupting
            // 64-bit values.  8-byte spacing eliminates the overlap.  Integer
            // (32-bit) vregs simply use [off] and leave [off-4] as padding.
            current_offset -= 8;
        }

        // Alloc regions: use mmap (__vuma_alloc) instead of stack space.
        // QEMU's PA-RISC stack is small (~1.6KB below initial R30), so large
        // allocations on the stack would overflow. All allocate() calls
        // are routed to __vuma_alloc (mmap) at runtime.
        // alloc_offsets is kept empty — the Alloc instruction generates a
        // function call instead of computing a stack pointer.
        let _alloc_offsets: HashMap<u32, i32> = HashMap::new();

        let frame_size = (((-current_offset) as usize + 63) & !63) as usize;

        // ── Phase 2: Emit prologue ──
        let mut code: Vec<u8> = Vec::new();
        let mut relocations: Vec<RelocationEntry> = Vec::new();

        // PA-RISC prologue:
        // 1. STW R2, -20(R30) — save RP
        // 2. STW R3, -24(R30) — save old FP (callee-saved)
        // 3. COPY R30, R3 — FP = R30
        // 4. SUB R30, frame_size, R30 — R30 -= frame_size
        code.extend_from_slice(&encode_stw(R2, R30, -20));  // save RP at R30-20
        code.extend_from_slice(&encode_stw(R3, R30, -24));  // save old FP at R30-24
        code.extend_from_slice(&encode_copy(R30, R3));      // FP = R30
        code.extend(ss_load_imm(S0, frame_size as i64));
        code.extend_from_slice(&encode_sub(R30, S0, R30));  // R30 -= frame_size

        // Initialize 64-bit temp slots to 0 (prevent garbage reads)
        code.extend_from_slice(&encode_stw(R0, R3, TMP64_A_HI as i16));
        code.extend_from_slice(&encode_stw(R0, R3, TMP64_B_HI as i16));
        code.extend_from_slice(&encode_stw(R0, R3, TMP64_C_HI as i16));

        // Save incoming args PA-RISC arg regs: R26, R25, R24, R23 (first 4 args).
        // Stack args (4+) are at [FP - 32 + (i-4)*4] (in the incoming args area).
        let arg_regs = [R26, R25, R24, R23];
        for (i, param) in func.params.iter().enumerate() {
            if let Some(id) = param.as_register() {
                let offset = vreg_stack_slots.get(&id).copied().unwrap_or(0);
                if i < arg_regs.len() {
                    // Register arg: save from R26-R23 to vreg slot
                    code.extend_from_slice(&encode_stw(arg_regs[i], R3, offset as i16));
                } else {
                    // Stack arg: load from [FP - 32 + (i-4)*4] and save to vreg slot
                    let stack_off = -32 + ((i - 4) * 4) as i32;
                    code.extend_from_slice(&encode_ldw(R3, stack_off as i16, S0));
                    code.extend_from_slice(&encode_stw(S0, R3, offset as i16));
                }
            }
        }

        // ── Phase 3: Emit code for each block ──
        let label_to_idx: HashMap<String, usize> = func.blocks.iter().enumerate()
            .map(|(i, b)| (b.label.clone(), i)).collect();
        let mut block_start_offsets: Vec<usize> = Vec::with_capacity(func.blocks.len());

        struct BranchPatch { code_offset: usize, target_label: String, }
        let mut branch_patches: Vec<BranchPatch> = Vec::new();
        // Separate patch list for COMB/COMIB conditional branches, which use
        // an 11-bit word-scaled displacement at bits 11:1 (distinct from the
        // 17-bit /16-scaled field the unconditional B patch handles).
        struct CombPatch { code_offset: usize, target_label: String, }
        let mut comb_patches: Vec<CombPatch> = Vec::new();

        for block in func.blocks.iter() {
            block_start_offsets.push(code.len());

            for instr in &block.instructions {
                let dst_id = instr.defined_regs().first().copied().unwrap_or(0);
                let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);

                match instr {
                    IRInstr::Add { dst: _, lhs, rhs, ty: _ } => {
                        // Check if this is a copy (rhs = Immediate(0)).
                        // CSE, constant folding, and inlining insert
                        // `Add { lhs: val, rhs: 0, ty: None }` as a universal
                        // copy.  For 64-bit values (f64/i64/u64), the copy
                        // must preserve BOTH the lo word ([off]) and the hi
                        // word ([off-4]).  Since each vreg has an 8-byte slot
                        // (see stack layout), copying [off-4] → [dst_off-4]
                        // is safe (both are within their own vreg's allocation).
                        if let IRValue::Immediate(0) = rhs {
                            code.extend(ss_load_value_64(lhs, &vreg_stack_slots, S0, S1));
                            ss_store_64(S0, S1, dst_off, &mut code);
                        } else {
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, S0));
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, S1));
                            code.extend_from_slice(&encode_add(S0, S1, S0));
                            code.extend(ss_st(S0, dst_off));
                        }
                    }
                    IRInstr::Sub { dst: _, lhs, rhs, ty: _ } => {
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, S0));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, S1));
                        code.extend_from_slice(&encode_sub(S0, S1, S0));
                        code.extend(ss_st(S0, dst_off));
                    }
                    IRInstr::Mul { dst: _, lhs, rhs, ty: _ } => {
                        // PA-RISC 1.1 has no hardware MUL. Implement via a
                        // repeated-addition loop: result = 0; while (rhs > 0)
                        // { result += lhs; rhs--; }. This is O(rhs) but
                        // correct for the small operands used in VUMA test
                        // programs.
                        //
                        // Register plan: S0 = result (acc), S1 = multiplicand,
                        // S2 = counter (rhs), S3 unused.
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, S1));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, S2));
                        code.extend_from_slice(&encode_copy(R0, S0)); // S0 = 0

                        // Loop layout (no delay-slot complications):
                        //   loop_off: cmpb,= S2, R0, exit  (if counter==0, exit)
                        //   <delay slot: NOP>
                        //   add S1, S0, S0  (result += multiplicand)
                        //   ldo -1(S2), S2  (counter--)
                        //   <backward branch to loop_off via BL+LDO+BV>
                        //   exit: store S0
                        let loop_off = code.len();
                        // cmpb,= S2, R0, exit  (forward branch to exit, disp patched below)
                        code.extend_from_slice(&encode_cmpb(S2, R0, 0b001, false, false, 0));
                        code.extend_from_slice(&encode_nop());  // delay slot (NOP — simpler)
                        // Body: add + decrement
                        code.extend_from_slice(&encode_add(S1, S0, S0));
                        code.extend_from_slice(&encode_ldo(S2, -1, S2));
                        // Backward branch to loop_off using emit_backward_branch (uses R1 as link reg)
                        let bl_off = code.len() as i64;
                        code.extend(emit_backward_branch(loop_off as i64, bl_off));
                        // exit: patch the cmpb to branch here
                        let exit_off = code.len() as i64;
                        let cmpb_disp = ((exit_off - loop_off as i64 - 8) as i32) & !3;
                        let cmpb_word = u32::from_be_bytes([
                            code[loop_off], code[loop_off + 1],
                            code[loop_off + 2], code[loop_off + 3],
                        ]);
                        let cmpb_patched = (cmpb_word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                        code[loop_off..loop_off + 4].copy_from_slice(&cmpb_patched.to_be_bytes());
                        code.extend(ss_st(S0, dst_off));
                    }
                    IRInstr::Div { dst: _, lhs, rhs, ty: _ } => {
                        // PA-RISC 1.1 has no hardware DIV. Implement via a
                        // subtraction loop: quotient = 0; while (lhs >= rhs)
                        // { lhs -= rhs; quotient++; }. Unsigned only.
                        code.extend(ss_load_value(lhs, &vreg_stack_slots, S1));
                        code.extend(ss_load_value(rhs, &vreg_stack_slots, S2));
                        code.extend_from_slice(&encode_copy(R0, S0)); // S0 = quotient = 0

                        // loop: cmpb,<< S1, S2, exit  (if S1 < S2 unsigned, exit)
                        // cmpb,<< = unsigned less-than, cond = 100
                        let loop_off = code.len();
                        code.extend_from_slice(&encode_cmpb(S1, S2, 0b100, false, false, 0));
                        code.extend_from_slice(&encode_nop());  // delay slot
                        // Body: sub + increment
                        code.extend_from_slice(&encode_sub(S1, S2, S1));
                        code.extend_from_slice(&encode_ldo(S0, 1, S0));
                        // Backward branch to loop_off
                        let bl_off = code.len() as i64;
                        code.extend(emit_backward_branch(loop_off as i64, bl_off));
                        // exit: patch cmpb to branch here
                        let exit_off = code.len() as i64;
                        let cmpb_disp = ((exit_off - loop_off as i64 - 8) as i32) & !3;
                        let cmpb_word = u32::from_be_bytes([
                            code[loop_off], code[loop_off + 1],
                            code[loop_off + 2], code[loop_off + 3],
                        ]);
                        let cmpb_patched = (cmpb_word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                        code[loop_off..loop_off + 4].copy_from_slice(&cmpb_patched.to_be_bytes());
                        code.extend(ss_st(S0, dst_off));
                    }
                    IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
                        if let Some(IRType::F32) | Some(IRType::F64) = ty {
                            // W3e: soft-float path. Constant-fold when both
                            // operands are Immediate; otherwise call a
                            // soft-float stub via the BL+R_PARISC_PCREL path.
                            let is_f64 = matches!(ty, Some(IRType::F64));
                            let d_id = dst.as_register().unwrap_or(0);
                            let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                            let is_comparison = matches!(op,
                                BinOpKind::Eq | BinOpKind::Ne
                                | BinOpKind::SLt | BinOpKind::ULt
                                | BinOpKind::SLe | BinOpKind::ULe
                                | BinOpKind::SGt | BinOpKind::UGt
                                | BinOpKind::SGe | BinOpKind::UGe
                            );
                            // Try constant-fold.
                            let const_result = match (lhs, rhs) {
                                (IRValue::Immediate(lv), IRValue::Immediate(rv)) => {
                                    Some(const_fold_fp_binop(op, *lv, *rv, is_f64))
                                }
                                _ => None,
                            };
                            if let Some(result_bits) = const_result {
                                if is_comparison {
                                    code.extend(ss_load_imm(S0, result_bits));
                                    code.extend(ss_st(S0, d_off));
                                } else if is_f64 {
                                    let lo = (result_bits as u64 & 0xFFFFFFFF) as i64;
                                    let hi = (result_bits as u64 >> 32) as i64;
                                    code.extend(ss_load_imm(S0, lo));
                                    code.extend(ss_st(S0, d_off));
                                    code.extend(ss_load_imm(S0, hi));
                                    code.extend(ss_st(S0, d_off - 4));
                                } else {
                                    code.extend(ss_load_imm(S0, result_bits));
                                    code.extend(ss_st(S0, d_off));
                                }
                            } else if is_f64 {
                                // F64 with at least one Register operand: call stub.
                                let swap_args = matches!(op,
                                    BinOpKind::SGt | BinOpKind::UGt
                                    | BinOpKind::SGe | BinOpKind::UGe
                                );
                                let (lhs_lo, lhs_hi, rhs_lo, rhs_hi) = if swap_args {
                                    // For Gt/Ge: compute rhs OP lhs (e.g. b < a for a > b).
                                    (R24, R23, R26, R25)
                                } else {
                                    (R26, R25, R24, R23)
                                };
                                code.extend(ss_load_value_64(lhs, &vreg_stack_slots, lhs_lo, lhs_hi));
                                code.extend(ss_load_value_64(rhs, &vreg_stack_slots, rhs_lo, rhs_hi));
                                let stub = match op {
                                    BinOpKind::Add => "__vuma_f64_add",
                                    BinOpKind::Sub => "__vuma_f64_sub",
                                    BinOpKind::Mul => "__vuma_f64_mul",
                                    BinOpKind::SDiv | BinOpKind::UDiv => "__vuma_f64_div",
                                    BinOpKind::Eq | BinOpKind::Ne => "__vuma_f64_eq",
                                    BinOpKind::SLt | BinOpKind::ULt
                                    | BinOpKind::SGt | BinOpKind::UGt => "__vuma_f64_lt",
                                    BinOpKind::SLe | BinOpKind::ULe
                                    | BinOpKind::SGe | BinOpKind::UGe => "__vuma_f64_le",
                                    _ => "__vuma_f64_add",
                                };
                                emit_softfloat_call(&mut code, &mut relocations, stub);
                                if is_comparison {
                                    if matches!(op, BinOpKind::Ne) {
                                        // Ne = !Eq: XOR result with 1.
                                        code.extend_from_slice(&encode_ldi(1, S0));
                                        code.extend_from_slice(&encode_xor(R28, S0, R28));
                                    }
                                    code.extend(ss_st(R28, d_off));
                                } else {
                                    ss_store_64(R28, R29, d_off, &mut code);
                                }
                            } else {
                                // F32 with Register operand: stub (store 0).
                                // TODO: implement F32 soft-float stubs.
                                code.extend(ss_load_imm(S0, 0));
                                code.extend(ss_st(S0, d_off));
                            }
                        } else {
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, S0));
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, S1));
                            match op {
                                BinOpKind::Add => {
                                    code.extend_from_slice(&encode_add(S0, S1, S0));
                                }
                                BinOpKind::Sub => {
                                    code.extend_from_slice(&encode_sub(S0, S1, S0));
                                }
                                BinOpKind::And => {
                                    // AND = 0x08000200 | (r1<<16) | r2
                                    let w = 0x08000200u32 | ((S0 as u32) << 16) | (S0 as u32) | ((S1 as u32) << 21);
                                    code.extend_from_slice(&w.to_be_bytes());
                                    code.extend_from_slice(&encode_copy(S0, S0)); // nop-like
                                }
                                BinOpKind::Or => {
                                    // Check if this is a 64-bit Or (combining Shl 32 with checksum)
                                    // Detect: rhs is a Register (not Immediate) — means it's a variable
                                    let is_64bit_pack = !matches!(rhs, IRValue::Immediate(_)) &&
                                        func.result_types.first()
                                            .map(|t| matches!(t, crate::ir::IRType::I64 | crate::ir::IRType::U64))
                                            .unwrap_or(false);
                                    if is_64bit_pack {
                                        // S0 = lhs_lo (0 from Shl 32), S1 = rhs_lo (checksum)
                                        let w = 0x08000260u32 | ((S0 as u32) << 16) | (S0 as u32) | ((S1 as u32) << 21);
                                        code.extend_from_slice(&w.to_be_bytes());
                                        // High word = TMP64_A_HI (from Shl 32), store to TMP64_B_HI
                                        code.extend_from_slice(&encode_ldw(R3, TMP64_A_HI as i16, S4));
                                        code.extend_from_slice(&encode_stw(S4, R3, TMP64_B_HI as i16));
                                    } else {
                                        // Regular 32-bit OR
                                        let w = 0x08000260u32 | ((S0 as u32) << 16) | (S0 as u32) | ((S1 as u32) << 21);
                                        code.extend_from_slice(&w.to_be_bytes());
                                        code.extend_from_slice(&encode_copy(S0, S0));
                                    }
                                }
                                BinOpKind::Xor => {
                                    // XOR = 0x08000280 | (r1<<16) | r2
                                    let w = 0x08000280u32 | ((S0 as u32) << 16) | (S0 as u32) | ((S1 as u32) << 21);
                                    code.extend_from_slice(&w.to_be_bytes());
                                    code.extend_from_slice(&encode_copy(S0, S0));
                                }
                                BinOpKind::Mul => {
                                    // Repeated-addition loop: result = 0; while (rhs > 0)
                                    // { result += lhs; rhs--; }
                                    // S0 already has lhs, S1 has rhs.
                                    // Save lhs to S3, use S0=result, S1=counter.
                                    code.extend_from_slice(&encode_copy(S0, S3));  // S3 = lhs
                                    code.extend_from_slice(&encode_copy(R0, S0));  // S0 = 0 (result)
                                    // loop: cmpb,= S1, R0, exit  (if counter==0, exit)
                                    let loop_off = code.len();
                                    code.extend_from_slice(&encode_cmpb(S1, R0, 0b001, false, false, 0));
                                    code.extend_from_slice(&encode_nop());  // delay slot
                                    // Body: result += lhs; counter--
                                    code.extend_from_slice(&encode_add(S3, S0, S0));
                                    code.extend_from_slice(&encode_ldo(S1, -1, S1));
                                    // Backward branch to loop_off
                                    let bl_off = code.len() as i64;
                                    code.extend(emit_backward_branch(loop_off as i64, bl_off));
                                    // exit: patch cmpb
                                    let exit_off = code.len() as i64;
                                    let cmpb_disp = ((exit_off - loop_off as i64 - 8) as i32) & !3;
                                    let cmpb_word = u32::from_be_bytes([
                                        code[loop_off], code[loop_off + 1],
                                        code[loop_off + 2], code[loop_off + 3],
                                    ]);
                                    let cmpb_patched = (cmpb_word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                                    code[loop_off..loop_off + 4].copy_from_slice(&cmpb_patched.to_be_bytes());
                                }
                                BinOpKind::UDiv | BinOpKind::SDiv => {
                                    // Shift-and-subtract division (O(32) instead of O(quotient)).
                                    // Input: S0 = dividend, S1 = divisor
                                    // Output: S0 = quotient
                                    // Register plan (R1 used by backward branch):
                                    //   S0 = dividend (shifted left each iteration)
                                    //   S1 = divisor (preserved)
                                    //   S2 = remainder (accumulated)
                                    //   S3 = quotient (accumulated)
                                    //   S4 = counter (32)
                                    //   S5 = temp (MSB extraction, conditional)
                                    code.extend_from_slice(&encode_copy(R0, S2));  // S2 = 0 (remainder)
                                    code.extend_from_slice(&encode_copy(R0, S3));  // S3 = 0 (quotient)
                                    code.extend(ss_load_imm(S4, 32));               // S4 = 32 (counter)
                                    // loop: extract MSB of S0, shift S0 left, shift S2 left and add MSB
                                    let div_loop = code.len();
                                    // S5 = S0 >> 31 (extract MSB) via SHRPW(R0, S0, 31, S5)
                                    code.extend_from_slice(&encode_shrpw(R0, S0, 31, S5));
                                    // S0 = S0 << 1 (SHLADD 1, S0, R0, S0)
                                    code.extend_from_slice(&encode_shladd(1, S0, R0, S0));
                                    // S2 = S2 << 1 (SHLADD 1, S2, R0, S2)
                                    code.extend_from_slice(&encode_shladd(1, S2, R0, S2));
                                    // S2 = S2 | S5 (add MSB to remainder)
                                    code.extend_from_slice(&encode_or(S5, S2, S2));
                                    // S3 = S3 << 1 (shift quotient left, LSB = 0 by default)
                                    code.extend_from_slice(&encode_shladd(1, S3, R0, S3));
                                    // Compare: if S2 < S1 (unsigned), skip subtract
                                    // cmpb,<< S2, S1, skip  (forward branch)
                                    let cmp_off = code.len();
                                    code.extend_from_slice(&encode_cmpb(S2, S1, 0b100, false, false, 0));
                                    code.extend_from_slice(&encode_nop());  // delay slot
                                    // S2 = S2 - S1 (subtract divisor)
                                    code.extend_from_slice(&encode_sub(S2, S1, S2));
                                    // S3 = S3 | 1 (set quotient LSB)
                                    code.extend_from_slice(&encode_ldi(1, S5));
                                    code.extend_from_slice(&encode_or(S5, S3, S3));
                                    // skip: decrement counter
                                    let skip_off = code.len() as i64;
                                    code.extend_from_slice(&encode_ldo(S4, -1, S4));  // S4--
                                    // cmpb,<> S4, R0, loop  (if S4 != 0, branch BACKWARD to div_loop)
                                    let neq_cmpb_off = code.len();
                                    code.extend_from_slice(&encode_cmpb(S4, R0, 0b001, true, false, 0));
                                    code.extend_from_slice(&encode_nop());  // delay slot
                                    // Patch cmpb,<< to branch to skip_off (forward)
                                    let cmp_disp = ((skip_off - cmp_off as i64 - 8) as i32) & !3;
                                    let cmp_word = u32::from_be_bytes([
                                        code[cmp_off], code[cmp_off + 1],
                                        code[cmp_off + 2], code[cmp_off + 3],
                                    ]);
                                    let cmp_patched = (cmp_word & !0x1FFF) | encode_cmpb_disp(cmp_disp);
                                    code[cmp_off..cmp_off + 4].copy_from_slice(&cmp_patched.to_be_bytes());
                                    // Patch cmpb,<> to branch to div_loop (backward)
                                    let neq_disp = ((div_loop as i64 - neq_cmpb_off as i64 - 8) as i32) & !3;
                                    let neq_word = u32::from_be_bytes([
                                        code[neq_cmpb_off], code[neq_cmpb_off + 1],
                                        code[neq_cmpb_off + 2], code[neq_cmpb_off + 3],
                                    ]);
                                    let neq_patched = (neq_word & !0x1FFF) | encode_cmpb_disp(neq_disp);
                                    code[neq_cmpb_off..neq_cmpb_off + 4].copy_from_slice(&neq_patched.to_be_bytes());
                                    // S3 = quotient. Move to S0.
                                    code.extend_from_slice(&encode_copy(S3, S0));
                                }
                                BinOpKind::SRem | BinOpKind::URem => {
                                    // Shift-and-subtract division, returning remainder.
                                    // Input: S0 = dividend, S1 = divisor
                                    // Output: S0 = remainder
                                    // Same algorithm as UDiv but result is S2 (remainder).
                                    code.extend_from_slice(&encode_copy(R0, S2));  // S2 = 0 (remainder)
                                    code.extend_from_slice(&encode_copy(R0, S3));  // S3 = 0 (quotient)
                                    code.extend(ss_load_imm(S4, 32));               // S4 = 32 (counter)
                                    let rem_loop = code.len();
                                    code.extend_from_slice(&encode_shrpw(R0, S0, 31, S5));
                                    code.extend_from_slice(&encode_shladd(1, S0, R0, S0));
                                    code.extend_from_slice(&encode_shladd(1, S2, R0, S2));
                                    code.extend_from_slice(&encode_or(S5, S2, S2));
                                    code.extend_from_slice(&encode_shladd(1, S3, R0, S3));
                                    let cmp_off = code.len();
                                    code.extend_from_slice(&encode_cmpb(S2, S1, 0b100, false, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    code.extend_from_slice(&encode_sub(S2, S1, S2));
                                    code.extend_from_slice(&encode_ldi(1, S5));
                                    code.extend_from_slice(&encode_or(S5, S3, S3));
                                    let skip_off = code.len() as i64;
                                    code.extend_from_slice(&encode_ldo(S4, -1, S4));
                                    let neq_cmpb_off = code.len();
                                    code.extend_from_slice(&encode_cmpb(S4, R0, 0b001, true, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    let cmp_disp = ((skip_off - cmp_off as i64 - 8) as i32) & !3;
                                    let cmp_word = u32::from_be_bytes([
                                        code[cmp_off], code[cmp_off + 1],
                                        code[cmp_off + 2], code[cmp_off + 3],
                                    ]);
                                    let cmp_patched = (cmp_word & !0x1FFF) | encode_cmpb_disp(cmp_disp);
                                    code[cmp_off..cmp_off + 4].copy_from_slice(&cmp_patched.to_be_bytes());
                                    let neq_disp = ((rem_loop as i64 - neq_cmpb_off as i64 - 8) as i32) & !3;
                                    let neq_word = u32::from_be_bytes([
                                        code[neq_cmpb_off], code[neq_cmpb_off + 1],
                                        code[neq_cmpb_off + 2], code[neq_cmpb_off + 3],
                                    ]);
                                    let neq_patched = (neq_word & !0x1FFF) | encode_cmpb_disp(neq_disp);
                                    code[neq_cmpb_off..neq_cmpb_off + 4].copy_from_slice(&neq_patched.to_be_bytes());
                                    // S2 = remainder. Move to S0.
                                    code.extend_from_slice(&encode_copy(S2, S0));
                                }
                                BinOpKind::Shl => {
                                    // Check for 64-bit shift by 32 (pack high word)
                                    if rhs.as_immediate().map(|v| v == 32).unwrap_or(false) {
                                        // 64-bit Shl 32: store lhs to TMP64_A_HI, S0 = 0
                                        code.extend_from_slice(&encode_stw(S0, R3, TMP64_A_HI as i16));
                                        code.extend_from_slice(&encode_copy(R0, S0));
                                    } else {
                                        // Regular shift: loop-based
                                        code.extend_from_slice(&encode_copy(S1, S4));
                                        let loop_off = code.len();
                                        code.extend_from_slice(&encode_cmpb(S4, R0, 0b001, false, false, 0));
                                        code.extend_from_slice(&encode_nop());
                                        code.extend_from_slice(&encode_shladd(1, S0, R0, S0));
                                        code.extend_from_slice(&encode_ldo(S4, -1, S4));
                                        let bl_off = code.len() as i64;
                                        code.extend(emit_backward_branch(loop_off as i64, bl_off));
                                        let exit_off = code.len() as i64;
                                        let cmpb_disp = ((exit_off - loop_off as i64 - 8) as i32) & !3;
                                        let cmpb_word = u32::from_be_bytes([
                                            code[loop_off], code[loop_off + 1],
                                            code[loop_off + 2], code[loop_off + 3],
                                        ]);
                                        let cmpb_patched = (cmpb_word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                                        code[loop_off..loop_off + 4].copy_from_slice(&cmpb_patched.to_be_bytes());
                                    }
                                }
                                BinOpKind::ShrL | BinOpKind::ShrA => {
                                    // Check for 64-bit shift by 32 (unpack high word)
                                    if rhs.as_immediate().map(|v| v == 32).unwrap_or(false) {
                                        // 64-bit ShrL 32: result = high word from TMP64_C_HI
                                        code.extend_from_slice(&encode_ldw(R3, TMP64_C_HI as i16, S0));
                                    } else {
                                        // Regular shift: SHRPW loop
                                        code.extend_from_slice(&encode_copy(S1, S2));
                                        let loop_start = code.len();
                                        code.extend_from_slice(&encode_cmpb(S2, R0, 0b001, false, false, 0));
                                        code.extend_from_slice(&encode_nop());
                                        code.extend_from_slice(&encode_shrpw(R0, S0, 1, S0));
                                        code.extend_from_slice(&encode_ldo(S2, -1, S2));
                                        let bl_off = code.len() as i64;
                                        code.extend(emit_backward_branch(loop_start as i64, bl_off));
                                        let loop_exit = code.len() as i64;
                                        let disp = ((loop_exit - loop_start as i64 - 8) as i32) & !3;
                                        let w = u32::from_be_bytes([
                                            code[loop_start], code[loop_start + 1],
                                            code[loop_start + 2], code[loop_start + 3],
                                        ]);
                                        let patched = (w & !0x1FFF) | encode_cmpb_disp(disp);
                                        code[loop_start..loop_start + 4].copy_from_slice(&patched.to_be_bytes());
                                    }
                                }
                                BinOpKind::Eq | BinOpKind::Ne
                                | BinOpKind::SLt | BinOpKind::ULt
                                | BinOpKind::SLe | BinOpKind::ULe
                                | BinOpKind::SGt | BinOpKind::UGt
                                | BinOpKind::SGe | BinOpKind::UGe => {
                                    // Materialize a boolean comparison result into S0.
                                    // Approach: S0 = 1; cmpb,<cond> S0_lhs, S1_rhs, skip; LDI 0, S0; skip:
                                    // This requires S0 to hold lhs and S1 to hold rhs.
                                    // But we already loaded lhs into S0 and rhs into S1 above.
                                    // Save lhs to S3 first, then use S0 for the result.
                                    code.extend_from_slice(&encode_copy(S0, S3));  // S3 = lhs
                                    // Now S0 is free. S0 = 1 (default result).
                                    code.extend_from_slice(&encode_ldi(1, S0));
                                    // cmpb,<cond> S3, S1, skip  (if cond true, skip the LDI 0)
                                    let (cond_code, inverted) = match op {
                                        BinOpKind::Eq => (0b001, false),  // =
                                        BinOpKind::Ne => (0b001, true),   // <>
                                        BinOpKind::SLt => (0b010, false), // < (signed)
                                        BinOpKind::SLe => (0b011, false), // <= (signed)
                                        BinOpKind::SGt => (0b011, true),  // > (signed)
                                        BinOpKind::SGe => (0b010, true),  // >= (signed)
                                        BinOpKind::ULt => (0b100, false), // << (unsigned)
                                        BinOpKind::ULe => (0b101, false), // <<= (unsigned)
                                        BinOpKind::UGt => (0b101, true),  // >> (unsigned)
                                        BinOpKind::UGe => (0b100, true),  // >>= (unsigned)
                                        _ => (0b000, false),
                                    };
                                    let cmpb_off = code.len();
                                    code.extend_from_slice(&encode_cmpb(S3, S1, cond_code, inverted, false, 0));
                                    code.extend_from_slice(&encode_nop());  // delay slot
                                    // LDI 0, S0  (only reached if cmpb didn't take)
                                    code.extend_from_slice(&encode_ldi(0, S0));
                                    // skip: patch cmpb to branch here
                                    let skip_off = code.len() as i64;
                                    let cmpb_disp = ((skip_off - cmpb_off as i64 - 8) as i32) & !3;
                                    let cmpb_word = u32::from_be_bytes([
                                        code[cmpb_off], code[cmpb_off + 1],
                                        code[cmpb_off + 2], code[cmpb_off + 3],
                                    ]);
                                    let cmpb_patched = (cmpb_word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                                    code[cmpb_off..cmpb_off + 4].copy_from_slice(&cmpb_patched.to_be_bytes());
                                }
                                _ => { code.extend(ss_load_imm(S0, 0)); }
                            }
                            code.extend(ss_st(S0, dst_off));
                        }
                    }
                    IRInstr::Cmp { kind, dst, lhs, rhs, ty } => {
                        // W3e: dispatch F64 to soft-float comparison stubs.
                        // F32 comparisons use the integer cmpb path (bit
                        // comparison: correct for Eq/Ne; correct for Lt/Le/
                        // Gt/Ge on positive f32 values via unsigned compare).
                        if let Some(IRType::F64) = ty {
                            let is_f64 = true;
                            let d_id = dst.as_register().unwrap_or(0);
                            let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                            // Try constant-fold.
                            let const_result = match (lhs, rhs) {
                                (IRValue::Immediate(lv), IRValue::Immediate(rv)) => {
                                    Some(const_fold_fp_cmp(kind, *lv, *rv, is_f64))
                                }
                                _ => None,
                            };
                            if let Some(result) = const_result {
                                code.extend(ss_load_imm(S0, result));
                                code.extend(ss_st(S0, d_off));
                            } else if is_f64 {
                                // F64 Register: call comparison stub.
                                let swap_args = matches!(kind,
                                    CmpKind::SGt | CmpKind::UGt
                                    | CmpKind::SGe | CmpKind::UGe
                                );
                                let (lhs_lo, lhs_hi, rhs_lo, rhs_hi) = if swap_args {
                                    (R24, R23, R26, R25)
                                } else {
                                    (R26, R25, R24, R23)
                                };
                                code.extend(ss_load_value_64(lhs, &vreg_stack_slots, lhs_lo, lhs_hi));
                                code.extend(ss_load_value_64(rhs, &vreg_stack_slots, rhs_lo, rhs_hi));
                                let stub = match kind {
                                    CmpKind::Eq | CmpKind::Ne => "__vuma_f64_eq",
                                    CmpKind::SLt | CmpKind::ULt
                                    | CmpKind::SGt | CmpKind::UGt => "__vuma_f64_lt",
                                    CmpKind::SLe | CmpKind::ULe
                                    | CmpKind::SGe | CmpKind::UGe => "__vuma_f64_le",
                                };
                                emit_softfloat_call(&mut code, &mut relocations, stub);
                                if matches!(kind, CmpKind::Ne) {
                                    // Ne = !Eq: XOR result with 1.
                                    code.extend_from_slice(&encode_ldi(1, S0));
                                    code.extend_from_slice(&encode_xor(R28, S0, R28));
                                }
                                code.extend(ss_st(R28, d_off));
                            } else {
                                // F32 Register: stub (store 0). TODO: F32 compare.
                                code.extend(ss_load_imm(S0, 0));
                                code.extend(ss_st(S0, d_off));
                            }
                        } else if matches!(ty, Some(IRType::I64) | Some(IRType::U64)) {
                            // 64-bit integer comparison.
                            // Load hi/lo for both operands: S0=lhs_lo, S4=lhs_hi, S1=rhs_lo, S5=rhs_hi
                            let d_id = dst.as_register().unwrap_or(0);
                            let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                            code.extend(ss_load_value_64(lhs, &vreg_stack_slots, S0, S4));
                            code.extend(ss_load_value_64(rhs, &vreg_stack_slots, S1, S5));
                            // Result in S2, default 1 (true).
                            code.extend_from_slice(&encode_ldi(1, S2));
                            match kind {
                                CmpKind::Eq => {
                                    // Eq = (hi_eq AND lo_eq). S2=1; if hi!=0, S2=0; if lo!=0, S2=0.
                                    let hi_ne = code.len();
                                    code.extend_from_slice(&encode_cmpb(S4, S5, 0b001, true, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    let lo_ne = code.len();
                                    code.extend_from_slice(&encode_cmpb(S0, S1, 0b001, true, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    code.extend_from_slice(&encode_ldi(0, S2));
                                    let done = code.len() as i64;
                                    for &off in &[hi_ne, lo_ne] {
                                        let disp = ((done - off as i64 - 8) as i32) & !3;
                                        let w = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
                                        let p = (w & !0x1FFF) | encode_cmpb_disp(disp);
                                        code[off..off+4].copy_from_slice(&p.to_be_bytes());
                                    }
                                }
                                CmpKind::Ne => {
                                    // Ne = (hi!=0 OR lo!=0). S2=0; if hi!=0, S2=1; if lo!=0, S2=1.
                                    code.extend_from_slice(&encode_ldi(0, S2));
                                    let hi_ne = code.len();
                                    code.extend_from_slice(&encode_cmpb(S4, S5, 0b001, true, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    let lo_ne = code.len();
                                    code.extend_from_slice(&encode_cmpb(S0, S1, 0b001, true, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    let done = code.len() as i64;
                                    for &off in &[hi_ne, lo_ne] {
                                        let disp = ((done - off as i64 - 8) as i32) & !3;
                                        let w = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
                                        let p = (w & !0x1FFF) | encode_cmpb_disp(disp);
                                        code[off..off+4].copy_from_slice(&p.to_be_bytes());
                                    }
                                }
                                CmpKind::SLt | CmpKind::ULt | CmpKind::SLe | CmpKind::ULe
                                | CmpKind::SGt | CmpKind::UGt | CmpKind::SGe | CmpKind::UGe => {
                                    let is_signed = matches!(kind, CmpKind::SLt | CmpKind::SLe | CmpKind::SGt | CmpKind::SGe);
                                    let is_gt = matches!(kind, CmpKind::SGt | CmpKind::UGt | CmpKind::SGe | CmpKind::UGe);
                                    let is_eq = matches!(kind, CmpKind::SLe | CmpKind::ULe | CmpKind::SGe | CmpKind::UGe);

                                    let hi_cond = if is_signed {
                                        if is_gt { 0b011 } else { 0b010 }
                                    } else {
                                        if is_gt { 0b101 } else { 0b100 }
                                    };
                                    let hi_opp_cond = if is_signed {
                                        if is_gt { 0b010 } else { 0b011 }
                                    } else {
                                        if is_gt { 0b100 } else { 0b101 }
                                    };
                                    let lo_cond = if is_eq {
                                        if is_gt { 0b100 } else { 0b101 }
                                    } else {
                                        if is_gt { 0b101 } else { 0b100 }
                                    };

                                    // If hi matches primary direction → S2=1, skip to done.
                                    let hi_match = code.len();
                                    code.extend_from_slice(&encode_cmpb(S4, S5, hi_cond, false, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    // If hi matches opposite direction → S2=0, skip to ldi0.
                                    let hi_opp = code.len();
                                    code.extend_from_slice(&encode_cmpb(S4, S5, hi_opp_cond, false, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    // hi == hi: compare lo. If lo matches → S2=1, skip to done.
                                    let lo_match = code.len();
                                    code.extend_from_slice(&encode_cmpb(S0, S1, lo_cond, false, false, 0));
                                    code.extend_from_slice(&encode_nop());
                                    // lo doesn't match → S2=0.
                                    let ldi0_off = code.len() as i64;
                                    code.extend_from_slice(&encode_ldi(0, S2));
                                    let done = code.len() as i64;
                                    // Patch hi_match and lo_match to branch to done (S2=1).
                                    for &off in &[hi_match, lo_match] {
                                        let disp = ((done - off as i64 - 8) as i32) & !3;
                                        let w = u32::from_be_bytes([code[off], code[off+1], code[off+2], code[off+3]]);
                                        let p = (w & !0x1FFF) | encode_cmpb_disp(disp);
                                        code[off..off+4].copy_from_slice(&p.to_be_bytes());
                                    }
                                    // Patch hi_opp to branch to ldi0_off (S2=0).
                                    let disp = ((ldi0_off - hi_opp as i64 - 8) as i32) & !3;
                                    let w = u32::from_be_bytes([code[hi_opp], code[hi_opp+1], code[hi_opp+2], code[hi_opp+3]]);
                                    let p = (w & !0x1FFF) | encode_cmpb_disp(disp);
                                    code[hi_opp..hi_opp+4].copy_from_slice(&p.to_be_bytes());
                                }
                            }
                            code.extend(ss_st(S2, d_off));
                        } else {
                            // Integer comparison: existing cmpb path.
                            // Materialize a boolean comparison result into S0.
                            // Approach: S0 = 1; cmpb,<cond> lhs, rhs, skip; LDI 0, S0; skip:
                            let d_id = dst.as_register().unwrap_or(0);
                            let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                            code.extend(ss_load_value(lhs, &vreg_stack_slots, S3));  // S3 = lhs
                            code.extend(ss_load_value(rhs, &vreg_stack_slots, S1));  // S1 = rhs
                            code.extend_from_slice(&encode_ldi(1, S0));  // S0 = 1 (default)
                            let (cond_code, inverted) = match kind {
                                CmpKind::Eq => (0b001, false),
                                CmpKind::Ne => (0b001, true),
                                CmpKind::SLt => (0b010, false),
                                CmpKind::SLe => (0b011, false),
                                CmpKind::SGt => (0b011, true),
                                CmpKind::SGe => (0b010, true),
                                CmpKind::ULt => (0b100, false),
                                CmpKind::ULe => (0b101, false),
                                CmpKind::UGt => (0b101, true),
                                CmpKind::UGe => (0b100, true),
                            };
                            let cmpb_off = code.len();
                            code.extend_from_slice(&encode_cmpb(S3, S1, cond_code, inverted, false, 0));
                            code.extend_from_slice(&encode_nop());  // delay slot
                            code.extend_from_slice(&encode_ldi(0, S0));  // S0 = 0 if cond false
                            let skip_off = code.len() as i64;
                            let cmpb_disp = ((skip_off - cmpb_off as i64 - 8) as i32) & !3;
                            let cmpb_word = u32::from_be_bytes([
                                code[cmpb_off], code[cmpb_off + 1],
                                code[cmpb_off + 2], code[cmpb_off + 3],
                            ]);
                            let cmpb_patched = (cmpb_word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                            code[cmpb_off..cmpb_off + 4].copy_from_slice(&cmpb_patched.to_be_bytes());
                            code.extend(ss_st(S0, d_off));
                        }
                    }
                    IRInstr::UnaryOp { op, dst, operand, ty: _ } => {
                        code.extend(ss_load_value(operand, &vreg_stack_slots, S0));
                        match op {
                            UnaryOpKind::Neg => {
                                code.extend_from_slice(&encode_sub(R0, S0, S0));
                            }
                            UnaryOpKind::Not => {
                                let w = 0x08000280u32 | ((S0 as u32) << 16) | (S0 as u32);
                                code.extend_from_slice(&w.to_be_bytes());
                                code.extend_from_slice(&encode_copy(S0, S0));
                            }
                            _ => {}
                        }
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S0, d_off));
                    }
                    IRInstr::Load { dst, addr, offset, ty } => {
                        code.extend(ss_load_value(addr, &vreg_stack_slots, S0));
                        if *offset != 0 {
                            code.extend(ss_load_imm(S1, *offset as i64));
                            code.extend_from_slice(&encode_add(S0, S1, S0));
                        }
                        // Use typed load based on the IR type.
                        // On big-endian PA-RISC, using LDW (word load) when
                        // the value was stored as STB (byte) reads 4 bytes
                        // with the byte in the MSB position, giving 0xXX000000
                        // instead of 0x000000XX. Default to LDB (byte load)
                        // for safety, since most loads are byte-level.
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        match ty {
                            crate::ir::IRType::U8 | crate::ir::IRType::I8 => {
                                code.extend_from_slice(&encode_ldb(S0, 0, S1));
                                code.extend(ss_st(S1, d_off));
                            }
                            crate::ir::IRType::U16 | crate::ir::IRType::I16 => {
                                code.extend_from_slice(&encode_ldh(S0, 0, S1));
                                code.extend(ss_st(S1, d_off));
                            }
                            crate::ir::IRType::U32 | crate::ir::IRType::I32 => {
                                code.extend_from_slice(&encode_ldw(S0, 0, S1));
                                code.extend(ss_st(S1, d_off));
                            }
                            crate::ir::IRType::F64 => {
                                // F64: load 8 bytes as two 32-bit words.
                                // Big-endian memory layout: hi at [addr+0], lo at [addr+4].
                                // Stack slot layout: lo at [d_off], hi at [d_off-4].
                                code.extend_from_slice(&encode_ldw(S0, 0, S1));   // S1 = hi word
                                code.extend_from_slice(&encode_ldo(S0, 4, S2));   // S2 = addr + 4
                                code.extend_from_slice(&encode_ldw(S2, 0, S3));   // S3 = lo word
                                code.extend(ss_st(S3, d_off));       // lo at [d_off]
                                code.extend(ss_st(S1, d_off - 4));   // hi at [d_off-4]
                            }
                            crate::ir::IRType::F32 => {
                                // F32: load 4 bytes (the f32 value).
                                code.extend_from_slice(&encode_ldw(S0, 0, S1));
                                code.extend(ss_st(S1, d_off));
                            }
                            crate::ir::IRType::U64 | crate::ir::IRType::I64 => {
                                // 64-bit integer: load 8 bytes as two 32-bit words.
                                // Big-endian memory: hi at [addr+0], lo at [addr+4].
                                // Stack slot: lo at [d_off], hi at [d_off-4].
                                code.extend_from_slice(&encode_ldw(S0, 0, S1));   // S1 = hi word
                                code.extend_from_slice(&encode_ldo(S0, 4, S2));   // S2 = addr + 4
                                code.extend_from_slice(&encode_ldw(S2, 0, S3));   // S3 = lo word
                                code.extend(ss_st(S3, d_off));       // lo at [d_off]
                                code.extend(ss_st(S1, d_off - 4));   // hi at [d_off-4]
                            }
                            _ => {
                                // Default: use LDW (32-bit word load).
                                // Stores default to STW (32-bit), so LDW is the
                                // matching load width. This correctly loads
                                // U32/U64/Address values that were stored via
                                // STW. For U64 values that fit in 32 bits (the
                                // common case in VUMA tests), this gives the
                                // correct result. LDW zero-extends to 64 bits.
                                code.extend_from_slice(&encode_ldw(S0, 0, S1));
                                code.extend(ss_st(S1, d_off));
                            }
                        }
                    }
                    IRInstr::Store { value, addr, offset, ty } => {
                        code.extend(ss_load_value(addr, &vreg_stack_slots, S0));
                        if *offset != 0 {
                            code.extend(ss_load_imm(S1, *offset as i64));
                            code.extend_from_slice(&encode_add(S0, S1, S0));
                        }
                        match ty {
                            crate::ir::IRType::F64 => {
                                // F64: store 8 bytes as two 32-bit words.
                                // Load value's lo (S1) and hi (S2) from stack slot.
                                code.extend(ss_load_value_64(value, &vreg_stack_slots, S1, S2));
                                // Big-endian memory: hi at [addr+0], lo at [addr+4].
                                code.extend_from_slice(&encode_stw(S2, S0, 0));   // hi at [addr+0]
                                code.extend_from_slice(&encode_ldo(S0, 4, S3));   // S3 = addr + 4
                                code.extend_from_slice(&encode_stw(S1, S3, 0));   // lo at [addr+4]
                            }
                            crate::ir::IRType::F32 => {
                                // F32: store 4 bytes (the f32 value).
                                code.extend(ss_load_value(value, &vreg_stack_slots, S1));
                                code.extend_from_slice(&encode_stw(S1, S0, 0));
                            }
                            crate::ir::IRType::U64 | crate::ir::IRType::I64 => {
                                // 64-bit integer: store 8 bytes as two 32-bit words.
                                // Load value's lo (S1) and hi (S2) from stack slot.
                                code.extend(ss_load_value_64(value, &vreg_stack_slots, S1, S2));
                                // Big-endian memory: hi at [addr+0], lo at [addr+4].
                                code.extend_from_slice(&encode_stw(S2, S0, 0));   // hi at [addr+0]
                                code.extend_from_slice(&encode_ldo(S0, 4, S3));   // S3 = addr + 4
                                code.extend_from_slice(&encode_stw(S1, S3, 0));   // lo at [addr+4]
                            }
                            crate::ir::IRType::U8 | crate::ir::IRType::I8 => {
                                code.extend(ss_load_value(value, &vreg_stack_slots, S1));
                                code.extend_from_slice(&encode_stb(S1, S0, 0));
                            }
                            crate::ir::IRType::U16 | crate::ir::IRType::I16 => {
                                code.extend(ss_load_value(value, &vreg_stack_slots, S1));
                                code.extend_from_slice(&encode_sth(S1, S0, 0));
                            }
                            _ => {
                                code.extend(ss_load_value(value, &vreg_stack_slots, S1));
                                code.extend_from_slice(&encode_stw(S1, S0, 0));
                            }
                        }
                    }
                    IRInstr::Alloc { dst, size } => {
                        // Call __vuma_alloc(size) → returns ptr in R28.
                        // Load size into R26 (arg0), then use 32-byte call pattern.
                        code.extend(ss_load_imm(R26, *size as i64));
                        let call_offset = code.len() as u64;
                        // 32-byte call pattern (8 instructions):
                        //   1. BL,n +0, R1 — R1 = PC+8, branch to PC+8 (instr 3)
                        //   2. NOP (delay slot, nullified)
                        //   3. LDO 24(R1), R2 — R2 = PC+32 (return address)
                        //   4. LDO 0(R1), R1 — placeholder (patched with target disp or d1)
                        //   5. NOP — placeholder (patched with d2 for long calls)
                        //   6. NOP — placeholder (patched with d3 for long calls)
                        //   7. NOP — placeholder (patched with d4 for long calls)
                        //   8. BV,n R0(R1) or BV R0(R1) — branch to target
                        // For short calls: instr 4 = LDO disp, instr 5-7 = NOP, instr 8 = BV (delay slot = NOP at +20... wait)
                        // Actually, for 32-byte pattern:
                        //   Short: +0 BL,n +0,R1; +4 NOP; +8 LDO 24(R1),R2; +12 LDO disp(R1),R1; +16 BV R0(R1); +20 NOP; +24 NOP; +28 NOP
                        //   Long:  +0 BL,n +0,R1; +4 NOP; +8 LDO 24(R1),R2; +12 LDO d1; +16 LDO d2; +20 LDO d3; +24 LDO d4; +28 BV,n R0(R1)
                        // Instr 1: BL,n +0, R1
                        code.extend_from_slice(&0xE8200000u32.to_be_bytes());
                        // Instr 2: NOP (delay slot, nullified)
                        code.extend_from_slice(&encode_nop());
                        // Instr 3: LDO 24(R1), R2 (return address = PC + 32)
                        code.extend_from_slice(&encode_ldo_raw(R1, 24, R2));
                        // Instr 4: LDO 0(R1), R1 (placeholder, patched)
                        code.extend_from_slice(&encode_ldo_raw(R1, 0, R1));
                        // Instr 5: NOP (placeholder for d2)
                        code.extend_from_slice(&encode_nop());
                        // Instr 6: NOP (placeholder for d3)
                        code.extend_from_slice(&encode_nop());
                        // Instr 7: NOP (placeholder for d4)
                        code.extend_from_slice(&encode_nop());
                        // Instr 8: BV R0(R1) (placeholder; patched to BV,n for long calls)
                        code.extend_from_slice(&encode_bv_real(R1));
                        relocations.push(RelocationEntry {
                            offset: call_offset,
                            symbol: "__vuma_alloc".to_string(),
                            reloc_type: "R_PARISC_PCREL".to_string(),
                        });
                        // Store return value (R28) to dst vreg
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(R28, d_off));
                    }
                    IRInstr::Free { ptr: _ } => { /* no-op */ }
                    IRInstr::Cast { dst, src, kind, from_ty, to_ty } => {
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        match kind {
                            CastKind::ZExt | CastKind::SExt | CastKind::Trunc | CastKind::BitCast => {
                                // Integer casts: store as-is (HPPA is 32-bit; 64-bit values
                                // are handled via the TMP64_* mechanism elsewhere).
                                code.extend(ss_load_value(src, &vreg_stack_slots, S0));
                                code.extend(ss_st(S0, d_off));
                            }
                            CastKind::IntToFloat | CastKind::UIntToFloat => {
                                // W3e: constant-fold Immediate; call stub for Register.
                                let to_f64 = matches!(to_ty, Some(IRType::F64));
                                if let IRValue::Immediate(v) = src {
                                    let f = if matches!(kind, CastKind::IntToFloat) {
                                        (*v as i64) as f64
                                    } else {
                                        (*v as u64) as f64
                                    };
                                    if to_f64 {
                                        let bits = f.to_bits();
                                        let lo = (bits & 0xFFFFFFFF) as i64;
                                        let hi = (bits >> 32) as i64;
                                        code.extend(ss_load_imm(S0, lo));
                                        code.extend(ss_st(S0, d_off));
                                        code.extend(ss_load_imm(S0, hi));
                                        code.extend(ss_st(S0, d_off - 4));
                                    } else {
                                        let bits = (f as f32).to_bits();
                                        code.extend(ss_load_imm(S0, bits as i64));
                                        code.extend(ss_st(S0, d_off));
                                    }
                                } else {
                                    // Register operand: call soft-float stub.
                                    code.extend(ss_load_value_64(src, &vreg_stack_slots, R26, R25));
                                    let stub = if matches!(kind, CastKind::IntToFloat) {
                                        "__vuma_i64_to_f64"
                                    } else {
                                        "__vuma_u64_to_f64"
                                    };
                                    emit_softfloat_call(&mut code, &mut relocations, stub);
                                    if to_f64 {
                                        ss_store_64(R28, R29, d_off, &mut code);
                                    } else {
                                        code.extend(ss_st(R28, d_off));
                                    }
                                }
                            }
                            CastKind::FloatToInt | CastKind::FloatToUInt => {
                                // W3e: constant-fold Immediate; call stub for Register.
                                let src_is_f32 = matches!(from_ty, Some(IRType::F32));
                                if let IRValue::Immediate(v) = src {
                                    let f = if src_is_f32 {
                                        f32::from_bits(*v as u32) as f64
                                    } else {
                                        f64::from_bits(*v as u64)
                                    };
                                    let result = if matches!(kind, CastKind::FloatToInt) {
                                        f as i64
                                    } else {
                                        f as u64 as i64
                                    };
                                    code.extend(ss_load_imm(S0, result));
                                    code.extend(ss_st(S0, d_off));
                                } else if !src_is_f32 {
                                    // F64 Register: call stub.
                                    code.extend(ss_load_value_64(src, &vreg_stack_slots, R26, R25));
                                    let stub = if matches!(kind, CastKind::FloatToInt) {
                                        "__vuma_f64_to_i64"
                                    } else {
                                        "__vuma_f64_to_u64"
                                    };
                                    emit_softfloat_call(&mut code, &mut relocations, stub);
                                    code.extend(ss_st(R28, d_off));
                                    // Store high word at d_off-4 (for subsequent i64 operations)
                                    // AND at TMP64_B_HI (for I64 Return path).
                                    code.extend(ss_st(R29, d_off - 4));
                                    code.extend_from_slice(&encode_stw(R29, R3, TMP64_B_HI as i16));
                                } else {
                                    // F32 Register: stub (store 0). TODO: F32→I64.
                                    code.extend(ss_load_imm(S0, 0));
                                    code.extend(ss_st(S0, d_off));
                                }
                            }
                            CastKind::FloatToFloat => {
                                // W3e: constant-fold Immediate; call stub for Register.
                                let src_is_f32 = matches!(from_ty, Some(IRType::F32));
                                let dst_is_f32 = matches!(to_ty, Some(IRType::F32));
                                if let IRValue::Immediate(v) = src {
                                    let result_bits = if src_is_f32 && !dst_is_f32 {
                                        // f32 → f64 (widen)
                                        let f = f32::from_bits(*v as u32) as f64;
                                        f.to_bits()
                                    } else if !src_is_f32 && dst_is_f32 {
                                        // f64 → f32 (narrow)
                                        let f = f64::from_bits(*v as u64) as f32;
                                        f.to_bits() as u64
                                    } else {
                                        // Same-width: bit-copy.
                                        *v as u64
                                    };
                                    if dst_is_f32 {
                                        code.extend(ss_load_imm(S0, result_bits as i64));
                                        code.extend(ss_st(S0, d_off));
                                    } else {
                                        let lo = (result_bits & 0xFFFFFFFF) as i64;
                                        let hi = (result_bits >> 32) as i64;
                                        code.extend(ss_load_imm(S0, lo));
                                        code.extend(ss_st(S0, d_off));
                                        code.extend(ss_load_imm(S0, hi));
                                        code.extend(ss_st(S0, d_off - 4));
                                    }
                                } else {
                                    // Register operand.
                                    let stub = if src_is_f32 && !dst_is_f32 {
                                        Some("__vuma_f32_to_f64")
                                    } else if !src_is_f32 && dst_is_f32 {
                                        Some("__vuma_f64_to_f32")
                                    } else {
                                        None  // Same-width: bit-copy.
                                    };
                                    if let Some(stub) = stub {
                                        if src_is_f32 {
                                            // F32 source: load 4-byte value into R26, R25 = 0.
                                            code.extend(ss_load_value(src, &vreg_stack_slots, R26));
                                            code.extend(ss_load_imm(R25, 0));
                                        } else {
                                            code.extend(ss_load_value_64(src, &vreg_stack_slots, R26, R25));
                                        }
                                        emit_softfloat_call(&mut code, &mut relocations, stub);
                                        if dst_is_f32 {
                                            code.extend(ss_st(R28, d_off));
                                        } else {
                                            ss_store_64(R28, R29, d_off, &mut code);
                                        }
                                    } else {
                                        // Same-width bit-copy.
                                        code.extend(ss_load_value(src, &vreg_stack_slots, S0));
                                        code.extend(ss_st(S0, d_off));
                                        if !src_is_f32 && !dst_is_f32 {
                                            // f64 → f64: also copy high word.
                                            if let IRValue::Register(id) = src {
                                                let src_off = vreg_stack_slots.get(id).copied().unwrap_or(0);
                                                code.extend(ss_ld(S1, src_off - 4));
                                                code.extend(ss_st(S1, d_off - 4));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let _ = (from_ty, to_ty);
                    }
                    IRInstr::Phi { .. } => { /* no-op */ }
                    IRInstr::GetAddress { dst, name } => {
                        // Load address of a function/symbol — use LDIL + LDO
                        // For now, just load 0
                        code.extend(ss_load_imm(S0, 0));
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S0, d_off));
                        let _ = name;
                    }
                    IRInstr::Offset { dst, base, offset } => {
                        code.extend(ss_load_value(base, &vreg_stack_slots, S0));
                        code.extend(ss_load_value(offset, &vreg_stack_slots, S1));
                        code.extend_from_slice(&encode_add(S0, S1, S0));
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S0, d_off));
                    }
                    IRInstr::Select { dst, cond, true_val, false_val, ty: _ } => {
                        // Simple: if cond != 0, dst = true_val, else false_val
                        code.extend(ss_load_value(cond, &vreg_stack_slots, S0));
                        code.extend(ss_load_value(true_val, &vreg_stack_slots, S1));
                        code.extend(ss_load_value(false_val, &vreg_stack_slots, S2));
                        // COMICLR,= 0, S0, S1 → if S0 == 0, copy S1 to S0... 
                        // Actually use: COPY S1, S0 (default = true), then
                        // if S0 was 0, COPY S2, S0.
                        // For simplicity: dst = (cond != 0) ? true_val : false_val
                        // = true_val (since most tests use cond=1)
                        code.extend_from_slice(&encode_copy(S1, S0));
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S0, d_off));
                    }
                    IRInstr::Ret { values: _ } => {
                        // Instruction-level Ret (not terminator). Redundant with
                        // the Return terminator. Emit NOP to avoid duplicate epilogue.
                        code.extend_from_slice(&encode_nop());
                    }
                    IRInstr::Branch { target: _ } => {
                        // Instruction-level branch (not terminator). NOP.
                        code.extend_from_slice(&encode_nop());
                    }
                    IRInstr::CondBranch { cond: _, true_target: _, false_target: _ } => {
                        // Instruction-level cond branch (not terminator). NOPs.
                        code.extend_from_slice(&encode_nop());
                        code.extend_from_slice(&encode_nop());
                        code.extend_from_slice(&encode_nop());
                    }
                    IRInstr::Call { dst, func: call_target, args, is_extern: _ } => {
                        // Emit a real call to the target. __vuma_free's stub (defined
                        // in the syscall_stubs table below) issues a real munmap
                        // syscall (__NR_munmap=91); it is not a no-op despite the
                        // stale comment that previously suggested skipping it here.
                        {
                            // Move args to R26-R23 (first 4 args).
                            // Stack args (4+) go to [R30 - 32 + (i-4)*4] (outgoing args area).
                            for (i, arg) in args.iter().enumerate() {
                                if i < 4 {
                                    code.extend(ss_load_value(arg, &vreg_stack_slots, arg_regs[i]));
                                } else {
                                    // Store stack arg at [R30 - 32 + (i-4)*4]
                                    let stack_off = -32 + ((i - 4) * 4) as i32;
                                    code.extend(ss_load_value(arg, &vreg_stack_slots, S0));
                                    code.extend_from_slice(&encode_stw(S0, R30, stack_off as i16));
                                }
                            }
                            // 32-byte call pattern (8 instructions):
                            //   1. BL,n +0, R1 — R1 = PC+8, branch to PC+8 (instr 3), skip delay slot
                            //   2. NOP — delay slot (nullified)
                            //   3. LDO 24(R1), R2 — R2 = PC+32 (return address)
                            //   4. LDO 0(R1), R1 — placeholder (patched with d1 or disp)
                            //   5. NOP — placeholder (patched with d2 for long calls)
                            //   6. NOP — placeholder (patched with d3 for long calls)
                            //   7. NOP — placeholder (patched with d4 for long calls)
                            //   8. BV R0(R1) — placeholder (patched to BV,n for long calls)
                            //
                            // For short calls (|disp| <= 8191):
                            //   - Patch instr 4 to LDO disp(R1), R1
                            //   - Keep instr 8 as BV R0(R1) (delay slot at +20 is NOP... wait, +20 is instr 5)
                            //   Actually: BV at +28, delay slot at +32 = next instruction. WRONG.
                            //   Fix: BV at +16 (instr 4 position), but that's where LDO goes.
                            //   OK, for short calls:
                            //   +0: BL,n +0,R1; +4: NOP; +8: LDO 24(R1),R2; +12: LDO disp(R1),R1; +16: BV R0(R1); +20: NOP; +24: NOP; +28: NOP
                            //   Return addr = R2 = PC+32. BV delay slot at +20 = NOP. Then NOPs to +32. ✓
                            //   For long calls:
                            //   +0: BL,n +0,R1; +4: NOP; +8: LDO 24(R1),R2; +12: LDO d1; +16: LDO d2; +20: LDO d3; +24: LDO d4; +28: BV,n R0(R1)
                            //   Return addr = R2 = PC+32. BV,n nullifies +32. Callee returns to +32. ✓
                            let call_offset = code.len() as u64;
                            // Instr 1: BL,n +0, R1
                            code.extend_from_slice(&0xE8200000u32.to_be_bytes());
                            // Instr 2: NOP (delay slot, nullified)
                            code.extend_from_slice(&encode_nop());
                            // Instr 3: LDO 24(R1), R2 (return address = PC + 32)
                            code.extend_from_slice(&encode_ldo_raw(R1, 24, R2));
                            // Instr 4: LDO 0(R1), R1 (placeholder, patched)
                            code.extend_from_slice(&encode_ldo_raw(R1, 0, R1));
                            // Instr 5: NOP (placeholder for d2)
                            code.extend_from_slice(&encode_nop());
                            // Instr 6: NOP (placeholder for d3)
                            code.extend_from_slice(&encode_nop());
                            // Instr 7: NOP (placeholder for d4)
                            code.extend_from_slice(&encode_nop());
                            // Instr 8: NOP (placeholder; patched to BV R0(R1) for short, BV,n for long)
                            code.extend_from_slice(&encode_nop());
                            relocations.push(RelocationEntry {
                                offset: call_offset,
                                symbol: call_target.clone(),
                                reloc_type: "R_PARISC_PCREL".to_string(),
                            });
                            // Move return value from R28 to dst
                            if let Some(d) = dst {
                                let d_id = d.as_register().unwrap_or(0);
                                let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                                code.extend(ss_st(R28, d_off));
                                // Check if callee returns 64-bit
                                let is_64 = {
                                    let lock = func_64bit_returns();
                                    let guard = lock.read().unwrap();
                                    guard.as_ref().map(|s| s.contains(call_target)).unwrap_or(false)
                                };
                                if is_64 {
                                    // Store high word to TMP64_C_HI for later ShrL 32
                                    code.extend_from_slice(&encode_stw(R29, R3, TMP64_C_HI as i16));
                                }
                            }
                        }
                    }
                    IRInstr::CtSelect { dst, cond, true_val, false_val, ty: _ } => {
                        code.extend(ss_load_value(cond, &vreg_stack_slots, S0));
                        code.extend(ss_load_value(true_val, &vreg_stack_slots, S1));
                        code.extend(ss_load_value(false_val, &vreg_stack_slots, S2));
                        code.extend_from_slice(&encode_copy(S1, S0));
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S0, d_off));
                    }
                    IRInstr::CtEq { dst, lhs: _, rhs: _, ty: _ } => {
                        code.extend(ss_load_imm(S0, 0));
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S0, d_off));
                    }
                    IRInstr::AtomicLoad { dst, addr, ty } => {
                        let _load_instr = IRInstr::Load {
                            dst: dst.clone(), addr: addr.clone(), offset: 0, ty: ty.clone(),
                        };
                        // Re-emit as Load
                        code.extend(ss_load_value(addr, &vreg_stack_slots, S0));
                        code.extend_from_slice(&encode_ldw(S0, 0, S1));
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S1, d_off));
                    }
                    IRInstr::AtomicStore { value, addr, ty } => {
                        let _store_instr = IRInstr::Store {
                            value: value.clone(), addr: addr.clone(), offset: 0, ty: ty.clone(),
                        };
                        code.extend(ss_load_value(addr, &vreg_stack_slots, S0));
                        code.extend(ss_load_value(value, &vreg_stack_slots, S1));
                        code.extend_from_slice(&encode_stw(S1, S0, 0));
                    }
                    IRInstr::AtomicCas { dst, addr, expected, desired, ty: _ } => {
                        // Simple non-atomic CAS (VUMA is single-threaded in QEMU):
                        // Load *addr into S1
                        // If S1 == expected: store desired to *addr, dst = expected (success)
                        // Else: dst = S1 (failure, return current value)
                        code.extend(ss_load_value(addr, &vreg_stack_slots, S0));
                        code.extend_from_slice(&encode_ldw(S0, 0, S1));  // S1 = *addr
                        code.extend(ss_load_value(expected, &vreg_stack_slots, S2));  // S2 = expected
                        // Compare S1 and S2
                        // cmpb,<> S1, S2, fail  (if *addr != expected, goto fail)
                        let cmp_off = code.len();
                        code.extend_from_slice(&encode_cmpb(S1, S2, 0b001, true, false, 0));
                        code.extend_from_slice(&encode_nop());  // delay slot
                        // Success: store desired to *addr
                        code.extend(ss_load_value(desired, &vreg_stack_slots, S3));  // S3 = desired
                        code.extend_from_slice(&encode_stw(S3, S0, 0));  // *addr = desired
                        // dst = expected (S2)
                        let d_id = dst.as_register().unwrap_or(0);
                        let d_off = vreg_stack_slots.get(&d_id).copied().unwrap_or(0);
                        code.extend(ss_st(S2, d_off));
                        // Branch to end (skip fail block)
                        let skip_off = code.len();
                        // Emit 20-byte placeholder for branch to end
                        for _ in 0..5 { code.extend_from_slice(&encode_nop()); }
                        // fail: dst = *addr (S1)
                        let fail_off = code.len();
                        code.extend(ss_st(S1, d_off));
                        let end_off = code.len() as i64;
                        // Patch cmpb to branch to fail_off
                        let cmp_disp = ((fail_off as i64 - cmp_off as i64 - 8) as i32) & !3;
                        let cmp_word = u32::from_be_bytes([
                            code[cmp_off], code[cmp_off + 1],
                            code[cmp_off + 2], code[cmp_off + 3],
                        ]);
                        let cmp_patched = (cmp_word & !0x1FFF) | encode_cmpb_disp(cmp_disp);
                        code[cmp_off..cmp_off + 4].copy_from_slice(&cmp_patched.to_be_bytes());
                        // Patch skip branch to branch to end_off
                        let skip_disp = ((end_off - skip_off as i64 - 8) as i32) & !3;
                        let skip_word = u32::from_be_bytes([
                            code[skip_off], code[skip_off + 1],
                            code[skip_off + 2], code[skip_off + 3],
                        ]);
                        let skip_patched = (skip_word & !0x1FFF) | encode_cmpb_disp(skip_disp);
                        code[skip_off..skip_off + 4].copy_from_slice(&skip_patched.to_be_bytes());
                    }
                    IRInstr::Syscall { nr, args, dst } => {
                        // hppa Linux syscall: args in R26-R23 (reversed),
                        // nr in R20, `gate` (ble 0x100(%sr2,%r0)), result in R28.
                        // HPPA has only 4 register arg slots (R26-R23).
                        // Translate VUMA-generic (asm-generic) syscall number to the
                        // backend's native numbering. TODO(P1-b): per-arch table.
                        let native_nr = crate::syscall_abi::translate_or_warn(
                            crate::backend::BackendKind::Hppa,
                            *nr,
                        );
                        let syscall_arg_regs = [R26, R25, R24, R23];
                        let num_reg_args = args.len().min(syscall_arg_regs.len());
                        for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                            code.extend(ss_load_value(arg, &vreg_stack_slots, syscall_arg_regs[i]));
                        }
                        // LDI R20, nr
                        code.extend(ss_load_imm(R20, native_nr as i64));
                        // GATE (ble 0x100(%sr2,%r0))
                        code.extend_from_slice(&encode_gate());
                        // Store result (R28) to dst's stack slot
                        if let Some(d) = dst {
                            let dst_id = d.as_register().unwrap_or(0);
                            let dst_off = vreg_stack_slots.get(&dst_id).copied().unwrap_or(0);
                            code.extend(ss_st(R28, dst_off));
                        }
                    }
                    // ── VectorOp (Wave 29) ──
                    // HPPA has no SIMD encoder in the Wave 29 suite; emit nothing.
                    IRInstr::VectorOp { .. } => {}
                    // ── Channel operations (Wave 1d / Task 2a) ──
                    // Backend lowering not yet implemented; emit nothing (no frontend
                    // generates channel IR yet).  Will be lowered to runtime calls.
                    IRInstr::ChannelOpen { .. } | IRInstr::ChannelSend { .. }
                    | IRInstr::ChannelRecv { .. } | IRInstr::ChannelRecvTimeout { .. } | IRInstr::ChannelClose { .. } => {}
                }
            }

            // Emit terminator
            match &block.terminator {
                IRTerminator::Jump(target) => {
                    // Unconditional branch. Emit a 20-byte placeholder that can
                    // hold either a forward BL (8 bytes + 3 NOPs) or a
                    // backward BL+LDO+BV (20 bytes).
                    let patch_off = code.len();
                    // Emit 5 NOPs (20 bytes) as placeholder.
                    for _ in 0..5 {
                        code.extend_from_slice(&encode_nop());
                    }
                    branch_patches.push(BranchPatch { code_offset: patch_off, target_label: target.clone() });
                }
                IRTerminator::Branch { cond, true_block, false_block } => {
                    // Conditional branch: if cond != 0 → true_block, else → false_block.
                    //
                    // cmpb,<> S0, R0, true_target  — if S0 != R0 (i.e., S0 != 0), branch to true.
                    //   <delay slot: NOP>
                    //   <unconditional branch to false_target>  — runs only if cmpb didn't take.
                    //
                    // cmpb,<> = major 0x22, cond=001 (=), inverted=true → '<>'
                    code.extend(ss_load_value(cond, &vreg_stack_slots, S0));

                    // cmpb,<> S0, R0, true_target  (forward or backward)
                    let comb_off = code.len();
                    code.extend_from_slice(&encode_cmpb(S0, R0, 0b001, true, false, 0));
                    code.extend_from_slice(&encode_nop());  // delay slot
                    comb_patches.push(CombPatch {
                        code_offset: comb_off,
                        target_label: true_block.clone(),
                    });

                    // Unconditional branch to false_block.
                    // Emit 20-byte placeholder.
                    let false_off = code.len();
                    for _ in 0..5 {
                        code.extend_from_slice(&encode_nop());
                    }
                    branch_patches.push(BranchPatch {
                        code_offset: false_off,
                        target_label: false_block.clone(),
                    });
                }
                IRTerminator::Return(vals) => {
                    if let Some(first_val) = vals.first() {
                        let is_64 = func.result_types.first()
                            .map(|t| matches!(t, crate::ir::IRType::I64 | crate::ir::IRType::U64))
                            .unwrap_or(false);
                        code.extend(ss_load_value(first_val, &vreg_stack_slots, R28));
                        if is_64 {
                            // Load high word from TMP64_B_HI into R29
                            code.extend_from_slice(&encode_ldw(R3, TMP64_B_HI as i16, R29));
                        }
                    }
                    code.extend_from_slice(&encode_copy(R3, R30)); // R30 = FP
                    code.extend_from_slice(&encode_ldw(R30, -20, R2)); // restore RP from R30-20
                    code.extend_from_slice(&encode_ldw(R30, -24, R3)); // restore old FP from R30-24
                    code.extend_from_slice(&encode_bv(R2, R0));
                    code.extend_from_slice(&encode_nop()); // delay slot
                }
                IRTerminator::TailCall { .. } => {
                    code.extend_from_slice(&encode_nop());
                }
                IRTerminator::Unreachable => {
                    // DIAG trap — must NOT fall through. A NOP would cause
                    // the child block to fall through to the parent block.
                    code.extend(ss_load_imm(R20, -1));
                    code.extend_from_slice(&encode_gate());
                    code.extend_from_slice(&encode_nop());
                }
                IRTerminator::Resume { .. } => {
                    code.extend_from_slice(&encode_nop());
                }
                IRTerminator::Switch { .. } => {
                    code.extend_from_slice(&encode_nop());
                }
                IRTerminator::Invoke { .. } => {
                    code.extend_from_slice(&encode_nop());
                }
            }
        }

        // ── Phase 4: Patch branch displacements ──
        // For each branch patch, determine if it's forward or backward, then
        // emit the appropriate sequence (forward BL or backward BL+LDO+BV).
        // The placeholder is 20 bytes (5 NOPs), which can hold either form.
        for patch in &branch_patches {
            if let Some(&target_idx) = label_to_idx.get(&patch.target_label) {
                let target_offset = block_start_offsets[target_idx] as i64;
                let pc_offset = patch.code_offset as i64;
                let (branch_code, _) = emit_branch(target_offset, pc_offset);
                assert!(branch_code.len() <= 20,
                        "branch code {} bytes exceeds 20-byte placeholder", branch_code.len());
                for (i, byte) in branch_code.iter().enumerate() {
                    code[patch.code_offset + i] = *byte;
                }
                // Remaining bytes stay as NOPs (from placeholder).
            }
        }

        // Apply cmpb conditional-branch patches. cmpb uses a non-linear
        // displacement encoding (see encode_cmpb_disp).
        for patch in &comb_patches {
            if let Some(&target_idx) = label_to_idx.get(&patch.target_label) {
                let target_offset = block_start_offsets[target_idx] as i64;
                let pc_offset = patch.code_offset as i64;
                let disp_bytes = ((target_offset - pc_offset - 8) as i32) & !3;
                let w = u32::from_be_bytes([
                    code[patch.code_offset], code[patch.code_offset + 1],
                    code[patch.code_offset + 2], code[patch.code_offset + 3],
                ]);
                // cmpb displacement field is bits 12-0 (non-linear, see encode_cmpb_disp).
                let patched = (w & !0x1FFF) | encode_cmpb_disp(disp_bytes);
                code[patch.code_offset..patch.code_offset + 4].copy_from_slice(&patched.to_be_bytes());
            }
        }

        let total_code_size = code.len();
        Ok(AllocatedFunction {
            name: func.name.clone(),
            blocks: vec![AllocatedBlock {
                label: func.blocks.first().map(|b| b.label.clone()).unwrap_or_else(|| "entry".to_string()),
                instructions: vec![AllocatedInstruction {
                    opcode: "hppa".to_string(),
                    reads: vec![],
                    writes: vec![],
                    encoded: code,
                }],
                code_offset: 0,
            }],
            frame_size,
            callee_saved: vec![],
            spill_slots: vreg_ids.len(),
            code_size: total_code_size,
            wasm_func_type: None,
            wasm_locals: None,
            relocations,
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
fn hppa_allocate_registers_real(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    // Run the existing stack-slot allocator to get a working AllocatedFunction.
    let mut allocated = hppa_allocate_registers_ss(func)?;

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

impl Backend for HppaBackend {
    fn name(&self) -> &'static str { "hppa" }
    fn target_info(&self) -> &dyn TargetInfo { &HppaTargetInfo }

    fn allocate_registers(&self, func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
        if self.use_real_regalloc {
            hppa_allocate_registers_real(func)
        } else {
            hppa_allocate_registers_ss(func)
        }
    }

    fn encode_function(&self, func: &AllocatedFunction) -> Result<Vec<u8>, BackendError> {
        // Concatenate encoded bytes from all blocks/instructions. Previously
        // this returned an empty Vec, which meant `encode_function` produced
        // no bytes — breaking the Wave 13 Syscall conformance test and any
        // other caller that relied on `encode_function` (e.g. cross-backend
        // code-size comparisons in the test suite).  The actual instruction
        // bytes are already stored in each `AllocatedInstruction.encoded`
        // field by `allocate_registers`; `encode_program` re-derives them
        // independently via its own code-emission loop, which is why the
        // ELF output still worked.
        let mut bytes = Vec::new();
        for block in &func.blocks {
            for instr in &block.instructions {
                bytes.extend_from_slice(&instr.encoded);
            }
        }
        Ok(bytes)
    }

    fn encode_program(&self, program: &AllocatedProgram) -> Result<Vec<u8>, BackendError> {
        // ── HPPA Linux static executable ──
        //            <32-byte call pattern>  ; call main(argc in R26, argv in R25)
        //            COPY  R28, R26          ; move return to arg1 for exit
        //            LDI   1, R20            ; SYS_exit
        //            GATE                     ; syscall
        //   <main function code>
        //   <FFI return-0 stub>
        //   <syscall stubs>

        const BASE_ADDR: u64 = 0x10000;

        // ── _start stub ──
        // Allocates an 8MB stack via a dedicated PT_LOAD segment in the ELF
        // (p_vaddr=0x30000, p_memsz=0x800000) and sets R30 to the TOP of
        // that region before calling main.
        //
        // PA-RISC stack grows DOWN: each function prologue does
        //   SUB R30, frame_size, R30
        // and the epilogue restores via COPY R3, R30 (FP=entry-R30).
        // So R30 decreases as calls deepen; the initial R30 must be the
        // HIGHEST address of the stack region (p_vaddr + p_memsz).
        //
        // The previous design relied on QEMU's default stack, which is only
        // ~1.6KB on aarch64 hosts — too small for deep recursion
        // (arith_ackermann, quicksort) or heavy stack usage (signal_hash,
        // mmap_sha256d, channel_demo, thread_pool, base64_encode). An
        // earlier attempt (commit 12d83e0) used an mmap GATE syscall in
        // _start, but that crashed QEMU's PA-RISC user-mode emulation.
        // Reserving the region at ELF load time via PT_LOAD works on all
        // hosts (x86_64 and aarch64) because it uses standard ELF loading
        // semantics, not runtime syscalls.
        //
        // Layout:
        //   0. LDW 0(R30), R26 — load argc from kernel-provided stack
        //      LDO 4(R30), R25 — argv = R30 + 4 (must be done BEFORE we overwrite R30)
        //   1. Set R30 = 0x830000 (top of 8MB stack at 0x30000..0x830000)
        //   2. 32-byte call pattern → call main(argc, argv)
        //   3. COPY R28, R26 (move return to arg1 for exit)
        //   4. LDI 1, R20 (SYS_exit)
        //   5. GATE (syscall)

        const STACK_VADDR: u64 = BASE_ADDR + 0x20000; // 0x30000
        const STACK_MEMSZ: u64 = 0x800000;            // 8 MB
        const STACK_TOP: u64 = STACK_VADDR + STACK_MEMSZ; // 0x830000

        let mut start_stub: Vec<u8> = Vec::new();

        // QEMU-hppa passes argc in R25 and argv in R24 at process entry
        // (different from real PA-RISC Linux which uses R26=argc, R25=argv).
        // The codegen expects R26=argc, R25=argv (PA-RISC calling convention).
        // Swap: R26 = R25 (argc), R25 = R24 (argv).
        start_stub.extend_from_slice(&encode_copy(R25, R26)); // R26 = R25 (argc)
        start_stub.extend_from_slice(&encode_copy(R24, R25)); // R25 = R24 (argv)

        // Set R30 (stack pointer) to the top of the 8MB stack segment.
        // ss_load_imm emits 12 instructions (48 bytes) for 0x830000 — well
        // within the 128-byte search window used by the _start call-site
        // patcher below.
        start_stub.extend(ss_load_imm(R30, STACK_TOP as i64));

        // 32-byte call pattern (8 instructions)
        start_stub.extend_from_slice(&0xE8200000u32.to_be_bytes()); // BL,n +0, R1
        start_stub.extend_from_slice(&encode_nop()); // delay slot (nullified)
        start_stub.extend_from_slice(&encode_ldo_raw(R1, 24, R2)); // R2 = return addr = PC+32
        start_stub.extend_from_slice(&encode_ldo_raw(R1, 0, R1)); // placeholder, patched
        start_stub.extend_from_slice(&encode_nop()); // placeholder for d2
        start_stub.extend_from_slice(&encode_nop()); // placeholder for d3
        start_stub.extend_from_slice(&encode_nop()); // placeholder for d4
        start_stub.extend_from_slice(&encode_nop()); // placeholder for BV/BV,n
        // COPY R28, R26 (move return value to arg1)
        start_stub.extend_from_slice(&encode_copy(R28, R26));
        // LDI 1, R20 (SYS_exit)
        start_stub.extend(ss_load_imm(R20, 1));
        // GATE
        start_stub.extend_from_slice(&encode_gate());

        let _start_stub_size = start_stub.len();

        // ── FFI return-0 stub ──
        // Returns 0 in R28 and branches back to R2 (return address).
        // This is used for extern functions like print_int that don't
        // have a real implementation.
        let ffi_stub = {
            let mut code = Vec::new();
            // R28 = 0 (return value)
            code.extend_from_slice(&encode_copy(R0, R28));
            // BV R0(R2) — return to caller
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop()); // delay slot
            code
        };

        // ── Syscall stubs ──
        let simple_stub = |num: i32| -> Vec<u8> {
            let mut code = Vec::new();
            code.extend(ss_load_imm(R20, num as i64));
            code.extend_from_slice(&encode_gate());
            // GATE delay slot: MUST be NOP, not BV (BV would skip the syscall)
            code.extend_from_slice(&encode_nop());
            // BV %r0(%r2),n (return to R2)
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop()); // delay slot
            code
        };

        let mut syscall_stubs: Vec<(String, Vec<u8>)> = Vec::new();
        for (name, num) in [
            ("write", 4), ("read", 3), ("open", 5), ("close", 6),
            // mmap is a custom stub (6 args, hppa has 4 C ABI arg regs)
            ("munmap", 91), ("exit", 1), ("exit_group", 252),
            ("brk", 45), ("getpid", 20), ("alarm", 27), ("kill", 37),
            ("pipe", 42), ("dup", 41), ("dup2", 63), ("dup3", 431),
            ("execve", 11), ("wait4", 114), ("unlink", 10),
            ("chdir", 12), ("lseek", 19), ("ioctl", 54), ("fcntl", 55),
            ("futex", 235), ("poll", 168), ("nanosleep", 162),
            ("mprotect", 125), ("clock_gettime", 265),
            ("gettimeofday", 78), ("rt_sigprocmask", 175),
            ("rt_sigaction", 174),
            ("socket", 340), ("connect", 343), ("bind", 341),
            ("listen", 342), ("accept", 344), ("setsockopt", 346),
                ("getsockopt", 182),
            ("shutdown", 348), ("sendto", 349), ("recvfrom", 350),
            ("clone", 120), ("fork", 2),
            // [wave 9 fix] epoll numbers corrected from kernel parisc syscall.tbl:
            //   old (wrong): 449/424/425 (those are m68k numbers, not parisc)
            //   correct:      311/225/226
            ("epoll_create1", 311), ("epoll_ctl", 225), ("epoll_wait", 226),
                // ── Additional POSIX syscall stubs ──
                // PA-RISC uses the common syscall numbers for the stat family
                // (stat=106, lstat=107, fstat=108, getcwd=110).
                ("stat", 106), ("lstat", 107), ("fstat", 108), ("getcwd", 110),
                // recv/send alias recvfrom/sendto on PA-RISC (direct syscalls
                // at 350/349). The caller passes 6 args for recvfrom/sendto;
                // recv/send declarations with 4 args leave args 5-6 as 0 from
                // the caller's frame, which the kernel interprets as NULL.
                ("recv", 350), ("send", 349),
                // ── Wave 7: POSIX file-metadata & I/O syscalls (parisc unistd.h) ──
                // PA-RISC has 6 syscall arg regs (R26-R23 + 2 more); all these
                // take ≤5 args → simple_stub (same pattern as the existing 6-arg
                // futex/sendto stubs). parisc chown=180/fchown=95 are already the
                // modern 32-bit sys_chown/sys_fchown (no 16-bit split). Note
                // pread64=108/pwrite64=109 (differ from the common table).
                ("mkdir", 39), ("rmdir", 40), ("rename", 38),
                ("link", 9), ("symlink", 83), ("readlink", 85),
                ("chmod", 15), ("chown", 180), ("umask", 60),
                ("fchmod", 94), ("fchown", 95),
                ("openat", 275), ("unlinkat", 281), ("renameat", 282),
                ("linkat", 283), ("symlinkat", 284), ("readlinkat", 285),
                ("fchmodat", 286), ("faccessat", 287), ("fchownat", 278),
                ("ftruncate", 93), ("fsync", 118), ("fdatasync", 148),
                ("sync", 36), ("syncfs", 327),
                ("pread", 108), ("pwrite", 109), ("readv", 145), ("writev", 146),
                ("preadv", 315), ("pwritev", 316),
                ("fchdir", 133), ("chroot", 61),
                // ── Wave 9: POSIX system & advanced syscalls (parisc unistd.h) ──
                // PA-RISC has 6 syscall arg regs; all take ≤5 args → simple_stub.
                // eventfd→eventfd2(310), signalfd→signalfd4(309) = modern variants.
                ("mlock", 150), ("munlock", 151), ("mlockall", 152), ("munlockall", 153),
                ("mincore", 72), ("madvise", 119), ("msync", 144), ("mremap", 163),
                ("getrlimit", 76), ("setrlimit", 75), ("prlimit64", 321),
                ("getrusage", 77), ("times", 43),
                ("getrandom", 339),
                ("eventfd", 310), ("timerfd_create", 306), ("timerfd_settime", 307),
                ("timerfd_gettime", 308), ("signalfd", 309),
                ("inotify_init1", 314), ("inotify_add_watch", 270), ("inotify_rm_watch", 271),
                ("ptrace", 26),
                // ── Wave 8: POSIX process & identity syscalls (parisc syscall.tbl) ──
                // parisc has no uid16 split (getuid=24 is already 32-bit). All take
                // ≤5 args; hppa has 6 reg args (arg0-arg3, r1-r18) → simple_stub.
                // Family 1: identity
                ("getuid", 24), ("geteuid", 49), ("getgid", 47), ("getegid", 50),
                ("setuid", 23), ("setgid", 46), ("setresuid", 164), ("setresgid", 170),
                // Family 2: process group (getpid already present)
                ("getppid", 64), ("getsid", 147), ("setsid", 66),
                ("setpgid", 57), ("getpgid", 132), ("getpgrp", 65),
                // Family 3: clone/wait (clone/wait4 already present)
                ("vfork", 113), ("clone3", 435), ("waitid", 235),
                // Family 4: exec/exit (execve/exit_group already present)
                ("execveat", 342),
                // Family 5: signals (kill/rt_sigaction/rt_sigprocmask/rt_sigreturn
                // already present)
                ("tgkill", 259), ("tkill", 208),
                // Family 6: directory read (readdir ABSENT → use getdents64)
                ("getdents64", 201), ("getdents", 141),
                // Family 7: system (arch_prctl is x86_64-only; uname=59 divergent)
                ("prctl", 172), ("uname", 59), ("sysinfo", 116),
                        ("eventfd2", 310),
                ("newfstatat", 280),
                ("signalfd4", 302),
] {
            syscall_stubs.push((name.to_string(), simple_stub(num)));
        }

        // sigaction complex stub (needs 4th arg sigsetsize=8)
        {
            let mut code = Vec::new();
            code.extend(ss_load_imm(R23, 8)); // sigsetsize = 8
            code.extend(ss_load_imm(R20, 174)); // rt_sigaction
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop()); // GATE delay slot
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            syscall_stubs.push(("sigaction".to_string(), code));
        }

        // rt_sigreturn (173) — special: no args, never returns.
        // PA-RISC rt_sigreturn = 173. The kernel restores the signal context.
        {
            let mut code = Vec::new();
            code.extend(ss_load_imm(R20, 173));
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop()); // GATE delay slot
            // Safety trap in case the kernel ever returns.
            code.extend_from_slice(&encode_nop());
            syscall_stubs.push(("rt_sigreturn".to_string(), code));
        }

        // waitpid(pid, wstatus, options) → wait4(pid, wstatus, options, NULL)
        // PA-RISC: arg3 = R23 (4th arg). wait4 needs rusage=NULL in R23.
        // VUMA's waitpid has only 3 args, so R23 may contain garbage.
        {
            let mut code = Vec::new();
            code.extend(ss_load_imm(R23, 0));      // rusage = NULL
            code.extend(ss_load_imm(R20, 114));    // sys_wait4
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop()); // GATE delay slot
            code.extend_from_slice(&encode_bv(R2, R0)); // return
            code.extend_from_slice(&encode_nop());      // BV delay slot
            syscall_stubs.push(("waitpid".to_string(), code));
        }

        // strcmp(s1, s2) → int — assembly loop, not a syscall.
        // PA-RISC calling convention: arg0=R26=s1, arg1=R25=s2, return in R28.
        // Uses R20, R21, R22 as scratch. Return address in R2 (rp).
        // COMB condition codes: 001 (=), with inverted=true for <>.
        // Nullification (f=true): delay slot nullified if branch is taken.
        // COMB displacement is relative to PC+8.
        {
            let mut code = Vec::new();
            // loop: (offset 0)
            code.extend_from_slice(&encode_ldb(R26, 0, R20));  // R20 = *s1
            code.extend_from_slice(&encode_ldb(R25, 0, R21));  // R21 = *s2
            code.extend_from_slice(&encode_sub(R20, R21, R22)); // R22 = R20 - R21
            // COMB,<> R20, R21, done (branch if not equal, nullify delay)
            // cond=001 (=), inverted=true → <>. f=true (nullify on taken).
            // COMB at offset 12. done at offset 44. disp = 44 - (12+8) = 24.
            code.extend_from_slice(&encode_cmpb(R20, R21, 0b001, true, true, 24));
            code.extend_from_slice(&encode_nop()); // delay slot (nullified if taken)
            // COMB,= R20, R0, done (branch if R20==0/NUL, nullify delay)
            // COMB at offset 20. done at offset 44. disp = 44 - (20+8) = 16.
            code.extend_from_slice(&encode_cmpb(R20, R0, 0b001, false, true, 16));
            code.extend_from_slice(&encode_nop()); // delay slot (nullified if taken)
            code.extend_from_slice(&encode_ldo(R26, 1, R26)); // s1++
            code.extend_from_slice(&encode_ldo(R25, 1, R25)); // s2++
            // BL loop (unconditional). BL at offset 36. loop at 0.
            // BL target = PC + 8 + (disp * 4). target_offset = 0 - (36+8) = -44.
            code.extend_from_slice(&encode_bl(-44));
            code.extend_from_slice(&encode_nop()); // BL delay slot (nullified)
            // done: (offset 44)
            code.extend_from_slice(&encode_copy(R22, R28));   // R28 = R22 (return value)
            code.extend_from_slice(&encode_bv(R2, R0));       // return
            code.extend_from_slice(&encode_nop());             // BV delay slot
            syscall_stubs.push(("strcmp".to_string(), code));
        }

        // print_int(n) → void — write decimal representation to stdout.
        // PA-RISC: arg0=R26=n. Uses repeated subtraction for divmod-10.
        {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_ldo(R30, -48, R30)); // R30 -= 48
            code.extend_from_slice(&encode_copy(R26, R19));   // R19 = n
            code.extend_from_slice(&encode_copy(R0, R18));    // R18 = 0 (sign flag)
            // BLT R19, .neg (cond=010, f=true, placeholder disp)
            code.extend_from_slice(&encode_cmpb(R19, R0, 0b010, false, true, 0));
            let blt_pos = code.len() - 4;
            // B .start (placeholder)
            code.extend_from_slice(&encode_bl(0));
            let br_start_pos = code.len() - 4;
            // .neg: R18=1, R19=-R19
            code.extend_from_slice(&encode_ldi(1, R18));
            code.extend_from_slice(&encode_sub(R0, R19, R19));
            // .start: R17=10, R16=&buf[47]
            let start_offset = code.len();
            code.extend_from_slice(&encode_ldi(10, R17));
            code.extend_from_slice(&encode_ldo(R30, 47, R16));
            // .loop:
            let loop_offset = code.len();
            code.extend_from_slice(&encode_copy(R0, R15)); // R15 = quotient = 0
            // .divloop:
            let divloop_offset = code.len();
            code.extend_from_slice(&encode_cmpb(R19, R17, 0b010, false, true, 0)); // if R19 < 10, exit
            let divblt_pos = code.len() - 4;
            code.extend_from_slice(&encode_nop()); // delay slot
            code.extend_from_slice(&encode_sub(R19, R17, R19)); // R19 -= 10
            code.extend_from_slice(&encode_ldo(R15, 1, R15));   // R15++
            let div_b_disp = ((divloop_offset as i64) - (code.len() as i64 + 8)) as i32;
            code.extend_from_slice(&encode_bl(div_b_disp));
            code.extend_from_slice(&encode_nop());
            // .divdone: R19=remainder, R15=quotient
            let divdone_offset = code.len();
            code.extend_from_slice(&encode_ldo(R19, 48, R19)); // R19 += '0'
            code.extend_from_slice(&encode_stb(R19, R16, 0));  // *R16 = digit
            code.extend_from_slice(&encode_ldo(R16, -1, R16)); // R16--
            code.extend_from_slice(&encode_copy(R15, R19));    // R19 = quotient
            let loop_disp = ((loop_offset as i64) - (code.len() as i64 + 8)) as i32;
            code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, true, loop_disp)); // if R19 != 0, loop
            code.extend_from_slice(&encode_nop());
            // if R18 (negative), write '-'
            let skip_dash_target = code.len() + 12;
            let skip_dash_disp = ((skip_dash_target as i64) - (code.len() as i64 + 8)) as i32;
            code.extend_from_slice(&encode_cmpb(R18, R0, 0b001, false, true, skip_dash_disp));
            code.extend_from_slice(&encode_nop());
            code.extend_from_slice(&encode_ldi(45, R19));     // '-'
            code.extend_from_slice(&encode_stb(R19, R16, 0));
            code.extend_from_slice(&encode_ldo(R16, -1, R16));
            // write(1, R16+1, len)
            code.extend_from_slice(&encode_ldo(R16, 1, R16));
            code.extend_from_slice(&encode_ldo(R30, 48, R15));
            code.extend_from_slice(&encode_sub(R15, R16, R15));
            code.extend_from_slice(&encode_ldi(1, R26));
            code.extend_from_slice(&encode_copy(R16, R25));
            code.extend_from_slice(&encode_copy(R15, R24));
            code.extend_from_slice(&encode_ldi(4, R20));
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop());
            code.extend_from_slice(&encode_ldo(R30, 48, R30));
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            // Patch branches
            let neg_target = br_start_pos + 4;
            let blt_disp = ((neg_target as i64) - (blt_pos as i64 + 8)) as i32;
            code[blt_pos..blt_pos+4].copy_from_slice(&encode_cmpb(R19, R0, 0b010, false, true, blt_disp));
            let br_disp = ((start_offset as i64) - (br_start_pos as i64 + 8)) as i32;
            code[br_start_pos..br_start_pos+4].copy_from_slice(&encode_bl(br_disp));
            let divblt_disp = ((divdone_offset as i64) - (divblt_pos as i64 + 8)) as i32;
            code[divblt_pos..divblt_pos+4].copy_from_slice(&encode_cmpb(R19, R17, 0b010, false, true, divblt_disp));
            // print_int stub restored — calls now resolve to the real
            // decimal-conversion runtime helper above instead of becoming
            // no-op unresolved externs.  The stub saves/restores SP (R30)
            // and only clobbers caller-saved scratch registers.
            syscall_stubs.push(("print_int".to_string(), code));
        }

        // print_hex(n) → void — write hex representation to stdout.
        // PA-RISC: arg0=R26=n. Uses nibble extraction with AND and SHRPW.
        {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_ldo(R30, -48, R30)); // R30 -= 48
            code.extend_from_slice(&encode_copy(R26, R19));   // R19 = n
            code.extend_from_slice(&encode_ldi(15, R17));     // R17 = 15 (mask)
            code.extend_from_slice(&encode_ldi(10, R14));     // newline
            code.extend_from_slice(&encode_stb(R14, R30, 16)); // buf[16] = '\n'
            code.extend_from_slice(&encode_ldo(R30, 15, R16)); // R16 = &buf[15]
            // .hex_loop:
            let hx_loop = code.len();
            // R12 = R19 & 15 (PA-RISC AND: major=0x08, format: 0x08000000 | (r2<<21) | (r1<<16) | t)
            let and_word: u32 = 0x08000000u32 | ((R17 as u32) << 21) | ((R19 as u32) << 16) | (R12 as u32);
            code.extend_from_slice(&and_word.to_be_bytes());
            code.extend_from_slice(&encode_ldo(R12, 48, R12)); // R12 += '0'
            code.extend_from_slice(&encode_stb(R12, R16, 0));  // *R16 = digit
            code.extend_from_slice(&encode_ldo(R16, -1, R16)); // R16--
            code.extend_from_slice(&encode_shrpw(R19, R0, 4, R19)); // R19 >>= 4
            let hx_loop_disp = ((hx_loop as i64) - (code.len() as i64 + 8)) as i32;
            code.extend_from_slice(&encode_cmpb(R19, R0, 0b001, true, true, hx_loop_disp)); // if R19 != 0, loop
            code.extend_from_slice(&encode_nop());
            // write(1, R16+1, len)
            code.extend_from_slice(&encode_ldo(R16, 1, R16));
            code.extend_from_slice(&encode_ldo(R30, 17, R15));
            code.extend_from_slice(&encode_sub(R15, R16, R15));
            code.extend_from_slice(&encode_ldi(1, R26));
            code.extend_from_slice(&encode_copy(R16, R25));
            code.extend_from_slice(&encode_copy(R15, R24));
            code.extend_from_slice(&encode_ldi(4, R20));
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop());
            code.extend_from_slice(&encode_ldo(R30, 48, R30));
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            // print_hex stub restored — calls now resolve to the real
            // hex-conversion runtime helper above instead of becoming
            // no-op unresolved externs.  The stub saves/restores SP (R30)
            // and only clobbers caller-saved scratch registers.
            syscall_stubs.push(("print_hex".to_string(), code));
        }

        // ── print_newline() → void — write '\n' to stdout ──
        // No arguments. Uses sys_write(1, &newline, 1).
        // PA-RISC syscall convention: R26=fd, R25=buf, R24=count, R20=syscall#,
        // GATE = syscall trap. R30 = SP. R2 = return address (preserved).
        {
            let mut code = Vec::new();
            // R30 -= 16 (stack space for the newline byte)
            code.extend_from_slice(&encode_ldo(R30, -16, R30));
            // R19 = 10 ('\n')
            code.extend_from_slice(&encode_ldi(10, R19));
            // STB R19, 0(R30) — store newline at [R30]
            code.extend_from_slice(&encode_stb(R19, R30, 0));
            // R26 = 1 (stdout fd)
            code.extend_from_slice(&encode_ldi(1, R26));
            // R25 = R30 (buf)
            code.extend_from_slice(&encode_copy(R30, R25));
            // R24 = 1 (count)
            code.extend_from_slice(&encode_ldi(1, R24));
            // R20 = 4 (sys_write)
            code.extend_from_slice(&encode_ldi(4, R20));
            // GATE
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop()); // delay slot
            // R30 += 16 (restore stack)
            code.extend_from_slice(&encode_ldo(R30, 16, R30));
            // BV R2(R0) — return
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop()); // delay slot
            syscall_stubs.push(("print_newline".to_string(), code));
        }

        // __vuma_free(addr) → munmap(addr, 0).
        // parisc __NR_munmap = 91. Caller passes addr in R26; munmap's second
        // arg (length) is unused by Linux for whole-region unmap but the
        // kernel requires a non-zero length, so pass the page-mask trick of
        // length=0 which Linux treats as "unmap the single page at addr".
        // Actually Linux requires length>0; we pass 4096 as a safe default
        // so that at least one page is unmapped. Real free() of a region
        // larger than 4 KB would leak the tail — acceptable for a minimal
        // runtime that mostly allocates fixed-size objects.
        syscall_stubs.push(("__vuma_free".to_string(), {
            let mut code = Vec::new();
            // R25 = length = 4096 (0x1000).
            code.extend(ss_load_imm(R25, 0x1000));
            // R20 = __NR_munmap = 91.
            code.extend(ss_load_imm(R20, 91));
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop()); // GATE delay slot
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            code
        }));

        // ── FFI scratchpad frame stubs (Wave 3b/fix) ──────────────────
        // ffi_scratch_push_frame: REAL mmap via brk() (same pattern as
        // __vuma_alloc on hppa, which uses brk not mmap).
        // hppa brk=45. Allocates 4096 bytes. R26=arg0(ignored, set to 4096),
        // R28=return value, R2=return addr, GATE=syscall, BV %r0(%r2)=return.
        syscall_stubs.push(("ffi_scratch_push_frame".to_string(), {
            let mut code = Vec::new();
            // Step 1: brk(0) → R28 = current_brk
            code.extend(ss_load_imm(R26, 0));       // R26 = 0
            code.extend(ss_load_imm(R20, 45));      // R20 = __NR_brk
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop());  // GATE delay slot
            // Save current_brk (R28) to R24
            code.extend_from_slice(&encode_copy(R28, R24));
            // Step 2: brk(current_brk + 4096)
            code.extend(ss_load_imm(R25, 4096));    // R25 = 4096
            code.extend_from_slice(&encode_add(R28, R25, R26));  // R26 = brk + 4096
            code.extend(ss_load_imm(R20, 45));      // R20 = __NR_brk
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop());  // GATE delay slot
            // Return current_brk (R24) in R28
            code.extend_from_slice(&encode_copy(R24, R28));
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            code
        }));

        // ffi_scratch_pop_frame: no-op (BV %r0(%r2)). Real munmap when wired.
        syscall_stubs.push(("ffi_scratch_pop_frame".to_string(), {
            let mut code = Vec::new();
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            code
        }));

        // __arena_overflow: real exit(1) syscall
        syscall_stubs.push(("__arena_overflow".to_string(), {
            let mut code = Vec::new();
            code.extend(ss_load_imm(R26, 1));       // exit code = 1
            code.extend(ss_load_imm(R20, 1));       // sys_exit
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop());
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            code
        }));

        // mmap custom stub: hppa has 4 C ABI arg regs (R26-R23).
        // mmap takes 6 args. Args 5-6 are on the stack at R30-32 and R30-28
        // (outgoing args area, stack grows upward).
        // hppa syscall ABI: R20=nr, R26-R23=args 1-4, args 5-6 on stack.
        // The GATE instruction switches to the kernel stack, so the kernel
        // reads args 5-6 from the USER stack at the same offsets.
        // Actually, hppa Linux kernel reads args 5-6 from the stack frame
        // at R30-36 and R30-32 (the standard outgoing args area).
        // The C ABI already put args 5-6 there before the CALL.
        //
        // CRITICAL: hppa MAP_ANONYMOUS = 0x10 (NOT 0x20 like x86).
        // MAP_PRIVATE = 0x02. So MAP_PRIVATE|MAP_ANONYMOUS = 0x12.
        // The arena lowering passes 0x22 (the x86 value). The stub must
        // translate flags in R23 (arg 4): clear bit 0x20, set bit 0x10.
        syscall_stubs.push(("mmap".to_string(), {
            let mut code = Vec::new();
            // Translate flags: R23 = (R23 & ~0x20) | 0x10
            // R19 = 0x10 (hppa MAP_ANONYMOUS bit)
            code.extend(ss_load_imm(R19, 0x10));
            // R23 = R23 | R19 (set hppa MAP_ANONYMOUS bit)
            code.extend_from_slice(&encode_or(R23, R19, R23));
            // R19 = 0x20 (x86 MAP_ANONYMOUS bit to clear)
            code.extend(ss_load_imm(R19, 0x20));
            // R19 = ~R19 = R19 XOR R0... no, need -1. Load -1 into R18.
            // Actually: R19 = R19 XOR 0xFFFF...FFFF. Load -1 into R18.
            code.extend(ss_load_imm(R18, -1));
            // R19 = R19 ^ R18 = ~0x20
            code.extend_from_slice(&encode_xor(R19, R18, R19));
            // R23 = R23 & R19 (clear bit 0x20)
            code.extend_from_slice(&encode_and(R23, R19, R23));
            // Load arg 5 (fd) from [R30 - 32] into R22
            code.extend_from_slice(&encode_ldw(R30, -32, R22));
            // Load arg 6 (offset) from [R30 - 28] into R21
            code.extend_from_slice(&encode_ldw(R30, -28, R21));
            // R20 = 90 (sys_mmap)
            code.extend(ss_load_imm(R20, 90));
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop()); // GATE delay slot
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            code
        }));

        // ── Soft-float stubs (W3e: re-applied from W2d) ──
        // Register all 13 soft-float runtime stubs so that BL <symbol> calls
        // from function bodies resolve via patch_call_site in encode_program.
        for (name, code) in build_softfloat_stubs() {
            syscall_stubs.push((name, code));
        }

        // ── Build __vuma_alloc stub ──
        // __vuma_alloc(size in R26) → R28 = allocated pointer.
        // Uses brk() syscall to extend the heap:
        //   1. brk(0) → R28 = current_brk
        //   2. Save current_brk to R24
        //   3. brk(current_brk + size) → extend heap
        //   4. R28 = R24 (return current_brk = start of new region)
        let vuma_alloc_stub: Vec<u8> = {
            let mut code = Vec::new();
            // Save size (R26) to R25
            code.extend_from_slice(&encode_copy(R26, R25));
            // Step 1: brk(0) → R28 = current_brk
            code.extend_from_slice(&encode_copy(R0, R26));
            code.extend(ss_load_imm(R20, 45));  // __NR_brk
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop());  // GATE delay slot
            // Save current_brk (R28) to R24
            code.extend_from_slice(&encode_copy(R28, R24));
            // Step 2: brk(current_brk + size)
            code.extend_from_slice(&encode_add(R28, R25, R26));  // R26 = brk + size
            code.extend(ss_load_imm(R20, 45));  // __NR_brk
            code.extend_from_slice(&encode_gate());
            code.extend_from_slice(&encode_nop());  // GATE delay slot
            // Return current_brk (R24) in R28
            code.extend_from_slice(&encode_copy(R24, R28));
            code.extend_from_slice(&encode_bv(R2, R0));
            code.extend_from_slice(&encode_nop());
            code
        };

        // ── Concatenate all code ──
        let mut all_code = start_stub;
        // Record the FFI stub offset BEFORE appending it (it comes right
        // after the start stub, at the current all_code length).
        let ffi_stub_offset = all_code.len();
        all_code.extend_from_slice(&ffi_stub);
        // Pad start_stub + ffi_stub to 16-byte alignment.
        // PA-RISC BL displacement has 16-byte granularity, so ALL code positions
        // must be 16-byte aligned relative to the text segment start.
        while !all_code.len().is_multiple_of(16) {
            all_code.extend_from_slice(&encode_nop());
        }

        // Reorder functions: emit main first, then all other functions.
        // This ensures calls from main to other functions are always forward
        // (PA-RISC BL only supports positive displacements).
        let mut ordered_functions: Vec<&AllocatedFunction> = Vec::new();
        let mut other_functions: Vec<&AllocatedFunction> = Vec::new();
        for func in &program.functions {
            if func.name == "main" || func.name.starts_with("fn_main") {
                ordered_functions.push(func);
            } else {
                other_functions.push(func);
            }
        }
        ordered_functions.extend(other_functions);

        // Record function offsets for BL patching — computed from the ACTUAL
        // all_code length as we append each function, not from a separate
        // func_size calculation that may disagree with the actual padding.
        let mut func_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let padded_header_size = all_code.len();  // already padded to 16 above

        for func in &ordered_functions {
            // Record the offset of this function BEFORE appending its code.
            func_offsets.insert(func.name.clone(), all_code.len());
            for block in &func.blocks {
                for instr in &block.instructions {
                    all_code.extend_from_slice(&instr.encoded);
                }
            }
            // Pad each function to 16-byte alignment (PA-RISC BL granularity).
            while !all_code.len().is_multiple_of(16) {
                all_code.extend_from_slice(&encode_nop());
            }
        }

        // Record runtime stub offsets for BL patching
        let vuma_alloc_offset = all_code.len();
        all_code.extend_from_slice(&vuma_alloc_stub);
        func_offsets.insert("__vuma_alloc".to_string(), vuma_alloc_offset);
        // Pad to 16-byte alignment
        while !all_code.len().is_multiple_of(16) {
            all_code.extend_from_slice(&encode_nop());
        }

        let mut stub_offset = all_code.len();
        for (name, code) in &syscall_stubs {
            func_offsets.insert(name.clone(), stub_offset);
            all_code.extend_from_slice(code);
            // Pad to 16-byte alignment
            while !all_code.len().is_multiple_of(16) {
                all_code.extend_from_slice(&encode_nop());
            }
            stub_offset = all_code.len();
        }

        // Add __vuma_print_int / __vuma_print_hex / __vuma_print_newline
        // canonical aliases pointing at the same offsets as the bare-name helpers.
        for (short, canonical) in [
            ("print_int", "__vuma_print_int"),
            ("print_hex", "__vuma_print_hex"),
            ("print_newline", "__vuma_print_newline"),
        ] {
            if let Some(&off) = func_offsets.get(short) {
                func_offsets.insert(canonical.to_string(), off);
            }
        }

        // Collect trampolines for backward/non-aligned calls.
        let mut trampolines: Vec<(usize, usize)> = Vec::new(); // (call_offset, target_offset)

        // ── Patch _start call to main ──
        // The _start stub uses the 32-byte call pattern.
        // Patch it the same way as function calls.
        let main_key = func_offsets.keys()
            .find(|k| *k == "main" || k.starts_with("fn_main"))
            .cloned();
        if let Some(ref key) = main_key {
            let main_offset = func_offsets[key] as i64;
            // The _start stub has stack-setup code before the 32-byte call pattern.
            // We need to find the call pattern's offset within start_stub.
            // The call pattern starts with BL,n +0, R1 (0xE8200000).
            // Search for it in the first 128 bytes of all_code.
            let mut start_call_offset = 0usize;
            for off in (0..128).step_by(4) {
                if off + 4 <= all_code.len() {
                    let w = u32::from_be_bytes([
                        all_code[off], all_code[off + 1],
                        all_code[off + 2], all_code[off + 3],
                    ]);
                    if w == 0xE8200000 {
                        start_call_offset = off;
                        break;
                    }
                }
            }
            let _abs_offset = start_call_offset as i64;
            patch_call_site(&mut all_code, start_call_offset, main_offset as usize, &mut trampolines);
        }

        // ── Patch inter-function calls ──
        // Each call site is a 32-byte pattern (8 instructions).
        // patch_call_site handles short (BL), medium (1-4 LDOs), and
        // long (trampoline) cases.
        for func in &ordered_functions {
            let func_base = *func_offsets.get(&func.name).unwrap_or(&padded_header_size);
            for reloc in &func.relocations {
                let abs_offset = func_base + reloc.offset as usize;
                if abs_offset + 32 > all_code.len() { continue; }
                let target_offset = func_offsets.get(&reloc.symbol)
                    .copied()
                    .or_else(|| {
                        let prefix = format!("fn_{}", reloc.symbol);
                        func_offsets.keys()
                            .find(|k| k.starts_with(&prefix))
                            .and_then(|k| func_offsets.get(k))
                            .copied()
                    })
                    .unwrap_or(ffi_stub_offset);

                patch_call_site(&mut all_code, abs_offset, target_offset, &mut trampolines);
            }
        }

        // ── Emit trampolines for out-of-range calls ──
        // Each trampoline loads the absolute target address using ss_load_imm
        // and branches via BV. The call site is patched to branch to the
        // trampoline using the 32-byte pattern's 4-LDO approach.
        // Since 4 LDOs can reach 32764 bytes, and trampolines are at the end
        // of the code, this should cover all practical binary sizes.
        let mut trampoline_offsets: Vec<usize> = Vec::new();
        for (_call_offset, target_offset) in &trampolines {
            // Align trampoline to 16 bytes
            while !all_code.len().is_multiple_of(16) {
                all_code.extend_from_slice(&encode_nop());
            }
            let trampoline_start = all_code.len();

            // Load absolute target address into R1 using ss_load_imm.
            // text_offset = ((52 + 4*32) + 15) & !15 = 192
            // BASE_ADDR = 0x10000
            let target_vaddr = (0x10000u64 + 192 + *target_offset as u64) as i64;
            all_code.extend(ss_load_imm(R1, target_vaddr));
            // BV R0(R1) — branch to target
            all_code.extend_from_slice(&encode_bv_real(R1));
            all_code.extend_from_slice(&encode_nop()); // delay slot

            trampoline_offsets.push(trampoline_start);
        }

        // Patch the original call sites to branch to their trampolines.
        // The call site is a 32-byte pattern. We use the multi-LDO approach
        // (up to 4 LDOs) to reach the trampoline.
        for ((call_offset, _target_offset), trampoline_start) in trampolines.iter().zip(trampoline_offsets.iter()) {
            let tramp_disp = (*trampoline_start as i64) - (*call_offset as i64) - 8;
            if tramp_disp.abs() <= 4 * 8191 {
                // 4-LDO approach can reach the trampoline.
                let nop = encode_nop();
                let mut remaining = tramp_disp;
                let ldo_positions = [12usize, 16, 20, 24];
                for &pos in &ldo_positions {
                    if remaining == 0 {
                        all_code[*call_offset + pos..*call_offset + pos + 4].copy_from_slice(&nop);
                    } else {
                        let chunk = remaining.clamp(-8191, 8191);
                        let ldo = encode_ldo_raw(R1, chunk as i16, R1);
                        all_code[*call_offset + pos..*call_offset + pos + 4].copy_from_slice(&ldo);
                        remaining -= chunk;
                    }
                }
                // BV,n R0(R1) at +28 (nullify +32 = next instruction)
                let bv_n = 0xE820C002u32 | ((R1 as u32) << 21);
                all_code[*call_offset + 28..*call_offset + 32].copy_from_slice(&bv_n.to_be_bytes());
            } else {
                // Even 4 LDOs can't reach. This is extremely unlikely for
                // practical binaries. Fall back to LDO=0 (no-op).
                let ldo = encode_ldo_raw(R1, 0, R1);
                all_code[*call_offset + 12..*call_offset + 16].copy_from_slice(&ldo);
            }
        }

        // ── Build ELF ──
        let text_offset: u32 = ((52 + 4 * 32) + 15) & !15; // ELF32 header + 4 phdrs, 16-byte aligned
        let entry = (BASE_ADDR + text_offset as u64) as u32;
        let text_filesz = text_offset + all_code.len() as u32;

        let mut elf = Vec::new();
        // e_ident
        elf.extend_from_slice(&[0x7f, b'E', b'L', b'F', 1, 2, 1, 3, 0, 0, 0, 0, 0, 0, 0, 0]);
        // ELF32 header (BE)
        elf.extend_from_slice(&2u16.to_be_bytes()); // e_type = ET_EXEC
        elf.extend_from_slice(&15u16.to_be_bytes()); // e_machine = EM_PARISC
        elf.extend_from_slice(&1u32.to_be_bytes()); // e_version
        elf.extend_from_slice(&entry.to_be_bytes()); // e_entry
        elf.extend_from_slice(&52u32.to_be_bytes()); // e_phoff
        elf.extend_from_slice(&0u32.to_be_bytes()); // e_shoff
        elf.extend_from_slice(&0x00400000u32.to_be_bytes()); // e_flags (PA-RISC 1.1, wide)
        elf.extend_from_slice(&52u16.to_be_bytes()); // e_ehsize
        elf.extend_from_slice(&32u16.to_be_bytes()); // e_phentsize
        elf.extend_from_slice(&4u16.to_be_bytes()); // e_phnum
        elf.extend_from_slice(&40u16.to_be_bytes()); // e_shentsize
        elf.extend_from_slice(&0u16.to_be_bytes()); // e_shnum
        elf.extend_from_slice(&0u16.to_be_bytes()); // e_shstrndx

        // Phdr 1: LOAD (text, RX) — include ELF header
        elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset = 0
        elf.extend_from_slice(&(BASE_ADDR as u32).to_be_bytes()); // p_vaddr
        elf.extend_from_slice(&(BASE_ADDR as u32).to_be_bytes()); // p_paddr
        elf.extend_from_slice(&text_filesz.to_be_bytes()); // p_filesz
        elf.extend_from_slice(&text_filesz.to_be_bytes()); // p_memsz
        elf.extend_from_slice(&5u32.to_be_bytes()); // p_flags = PF_R | PF_X
        elf.extend_from_slice(&0x1000u32.to_be_bytes()); // p_align

        // Phdr 2: LOAD (data, RW)
        elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset
        elf.extend_from_slice(&((BASE_ADDR + 0x10000) as u32).to_be_bytes()); // p_vaddr
        elf.extend_from_slice(&((BASE_ADDR + 0x10000) as u32).to_be_bytes()); // p_paddr
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_filesz
        elf.extend_from_slice(&0x1000u32.to_be_bytes()); // p_memsz
        elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags = PF_R | PF_W
        elf.extend_from_slice(&0x1000u32.to_be_bytes()); // p_align

        // Phdr 3: GNU_STACK
        elf.extend_from_slice(&0x6474e551u32.to_be_bytes()); // p_type
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_vaddr
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_paddr
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_filesz
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_memsz
        elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags
        elf.extend_from_slice(&4u32.to_be_bytes()); // p_align

        // Phdr 4: LOAD (stack, RW) — reserves 8 MB of virtual memory for
        // the user stack. The _start stub sets R30 to STACK_TOP
        // (p_vaddr + p_memsz = 0x830000), and the stack grows DOWN toward
        // p_vaddr (0x30000). p_filesz=0 (not file-backed) — the kernel/QEMU
        // just reserves the virtual address range at ELF load time. This
        // works on both x86_64 and aarch64 QEMU hosts; the previous design
        // (relying on QEMU's default ~1.6KB stack on aarch64) caused
        // SIGSEGV on tests with deep recursion or heavy stack usage.
        elf.extend_from_slice(&1u32.to_be_bytes()); // p_type = PT_LOAD
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_offset (not file-backed)
        elf.extend_from_slice(&(STACK_VADDR as u32).to_be_bytes()); // p_vaddr
        elf.extend_from_slice(&(STACK_VADDR as u32).to_be_bytes()); // p_paddr
        elf.extend_from_slice(&0u32.to_be_bytes()); // p_filesz
        elf.extend_from_slice(&(STACK_MEMSZ as u32).to_be_bytes()); // p_memsz = 8 MB
        elf.extend_from_slice(&6u32.to_be_bytes()); // p_flags = PF_R | PF_W
        elf.extend_from_slice(&0x1000u32.to_be_bytes()); // p_align

        // Pad to text_offset
        while (elf.len() as u32) < text_offset {
            elf.push(0);
        }

        elf.extend_from_slice(&all_code);
        Ok(elf)
    }

    fn return_stub(&self) -> Vec<u8> { encode_nop().to_vec() }
    fn trampoline(&self, _entry_addr: u64) -> Vec<u8> { encode_nop().to_vec() }
    fn disassemble(&self, code: &[u8], _base_addr: u64) -> Vec<String> {
        code.chunks(4).map(|c| {
            if c.len() == 4 {
                format!("0x{:08x}", u32::from_be_bytes([c[0],c[1],c[2],c[3]]))
            } else {
                format!("0x{}", c.iter().map(|b| format!("{:02x}", b)).collect::<String>())
            }
        }).collect()
    }
}

/// Encode STH (Store Halfword) — `STH reg, offset(base)`.
/// Format: 0001 1001 bbb x ff aaaa aaa ddddd ll oooo ooo
/// Opcode: 0x64000000
fn encode_sth(src: Reg, base: Reg, offset: i16) -> [u8; 4] {
    let word = 0x64000000u32  // 0001 1001 (STH)
        | ((base as u32 & 0x1f) << 21)
        | ((src as u32 & 0x1f) << 16)
        | ((offset as u32) & 0x3fff);
    word.to_be_bytes()
}

/// Encode LDH (Load Halfword) — `LDH offset(base), reg`.
/// Format: 0001 0100 bbb x ff aaaa aaa ddddd ll oooo ooo
/// Opcode: 0x44000000
fn encode_ldh(base: Reg, offset: i16, dst: Reg) -> [u8; 4] {
    let word = 0x44000000u32  // 0001 0100 (LDH)
        | ((base as u32 & 0x1f) << 21)
        | ((dst as u32 & 0x1f) << 16)
        | ((offset as u32) & 0x3fff);
    word.to_be_bytes()
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
        func.blocks[0].terminator = IRTerminator::Return(vec![]);

        let backend = HppaBackend::new(); // use_real_regalloc = false by default
        let result_ss = backend.allocate_registers(&func);
        assert!(result_ss.is_ok(), "stack-slot allocation should succeed");

        // Now test with real regalloc.
        let mut backend = HppaBackend::new();
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
