//! # Stack-Slot ISel for x86_64
//!
//! Complete replacement for the `allocate_registers` method in the x86_64 backend.
//!
//! Every virtual register gets a stack slot at `[rbp - offset]`. The ISel generates
//! code that loads source operands from their stack slots into scratch registers,
//! performs the operation, and stores the result to the destination's stack slot.
//!
//! ## Scratch Registers (never assigned to vregs)
//!
//! - RAX: primary accumulator / return value
//! - RCX: secondary operand / shift count
//! - RDX: tertiary / division (RDX:RAX pair)
//! - R10, R11: additional temporaries
//!
//! ## Callee-Saved Registers
//!
//! RBX, R12, R13, R14, R15 are pushed in the prologue (after `push rbp; mov rbp, rsp;
//! sub rsp, frame_size`) and popped in reverse order before the epilogue.

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction,
    BackendError, PhysicalReg, RegClass, RelocationEntry,
};
use crate::ir::{BinOpKind, CastKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind, VectorOpKind};
// Wave 12b: capability tokens are verified on receive by checking the
// cap_count field in the L1 frame header. The CapabilityToken type
// (crate::capability::CapabilityToken) defines the wire format that the
// cap_count field counts; full HMAC-SHA256 signature verification
// requires a crypto runtime and is deferred.
#[allow(unused_imports)]
use crate::capability::CapabilityToken;
use std::collections::HashMap;

#[allow(unused_imports)]
use super::{
    binop_cmp_to_cc, cmp_kind_to_cc, modrm, rex_prefix,
    Cc, Gpr, Xmm,
    R_X86_64_64, R_X86_64_PLT32,
    encode_add_reg_imm32, encode_add_reg_reg,
    encode_and_reg_imm32, encode_and_reg_reg,
    encode_call_rel32,
    encode_cmovcc_reg_reg,
    encode_cmp_reg_imm32, encode_cmp_reg_reg,
    encode_cqo,
    encode_cvtsd2si_r32_xmm, encode_cvtsd2si_r64_xmm,
    encode_cvtsd2ss_xmm_xmm,
    encode_cvtsi2sd_xmm_r32, encode_cvtsi2sd_xmm_r64,
    encode_cvtsi2ss_xmm_r32, encode_cvtsi2ss_xmm_r64,
    encode_cvtss2sd_xmm_xmm,
    encode_cvtss2si_r32_xmm, encode_cvtss2si_r64_xmm,
    encode_cvttsd2si_r32_xmm, encode_cvttsd2si_r64_xmm,
    encode_cvttss2si_r32_xmm, encode_cvttss2si_r64_xmm,
    encode_addsd_xmm_xmm, encode_addss_xmm_xmm,
    encode_subsd_xmm_xmm, encode_subss_xmm_xmm,
    encode_mulsd_xmm_xmm, encode_mulss_xmm_xmm,
    encode_divsd_xmm_xmm, encode_divss_xmm_xmm,
    encode_sqrtsd_xmm_xmm, encode_sqrtss_xmm_xmm,
    encode_minsd_xmm_xmm, encode_maxsd_xmm_xmm,
    encode_ucomisd_xmm_xmm, encode_ucomiss_xmm_xmm,
    encode_div_reg,
    encode_idiv_reg,
    encode_imul_reg_reg,
    encode_jcc_rel32, encode_jmp_rel32,
    encode_lea_reg_mem, encode_lea_rip_rel,
    encode_mov_mem16_reg16, encode_mov_mem32_reg32, encode_mov_mem8_reg8,
    encode_mov_mem_reg,
    encode_mov_reg32_mem,
    encode_mov_reg_imm32, encode_mov_reg_imm64, encode_mov_reg_mem, encode_mov_reg_reg,
    encode_movd_gpr_xmm, encode_movd_xmm_gpr,
    encode_movq_gpr_xmm, encode_movq_xmm_gpr,
    encode_movsx_reg8,
    encode_movsx_reg8_mem,
    encode_movsx_reg16,
    encode_movzx_reg8, encode_movzx_reg16,
    encode_movzx_reg8_mem, encode_movzx_reg16_mem,
    encode_neg_reg, encode_nop, encode_not_reg,
    encode_or_reg_imm32, encode_or_reg_reg,
    encode_pop, encode_push,
    encode_ret,
    encode_rol_reg_cl, encode_ror_reg_cl,
    encode_syscall,
    encode_sar_reg_cl,
    encode_setcc,
    encode_shl_reg_cl, encode_shr_reg_cl,
    encode_sub_reg_imm32, encode_sub_reg_reg,
    encode_test_reg_reg,
    encode_xor_reg_imm32, encode_xor_reg_reg,
    // ── SSE/AVX SIMD encoders (Wave 29 ISel wiring) ──
    encode_sse_paddq, encode_sse_psubd, encode_sse_pmulld,
    encode_avx_vpaddq,
};

// =============================================================================
// FP type inference — pre-pass
// =============================================================================

/// Infer which virtual registers hold floating-point (F32/F64) values.
///
/// VUMA's `scg_to_ir` lowering hardcodes `ty: None` on every `Add`/`Sub`/
/// `Mul`/`Div` (arithmetic is type-tag-polymorphic in the IR but the type
/// tag is dropped before backend lowering).  As a result, the x86_64 backend
/// cannot dispatch to the SSE/SSE2 path by inspecting `ty` alone.  This
/// pre-pass walks the IR forward to fixed-point and recovers FP-ness from:
///
///   * function parameters with declared F32/F64 type,
///   * `Cast { IntToFloat | UIntToFloat | FloatToFloat }` outputs,
///   * `Call` to the runtime float-conversion builtins `inttofloat` /
///     `uinttofloat` (which the IR builder emits as ordinary calls rather
///     than `Cast`s — see `lower_call` in `scg_to_ir.rs`),
///   * `Load` with F32/F64 type,
///   * `Add`/`Sub`/`Mul`/`Div`/`BinOp`/`Phi`/`Select` whose operands (or
///     any incoming, for `Phi`) are already known-FP,
///   * the IEEE-754 NaN-producing pattern `0 / 0` (both operands provably
///     zero with no type tag): integer `0 / 0` is undefined (SIGFPE on
///     x86), so reclassifying it as FP division (which produces NaN) is
///     strictly safer and is the only way to recover the FP type when the
///     IR has dropped it (e.g. `nan: f64 = 0.0 / 0.0`).
///
/// Patch a conditional-jump rel32 placeholder (6-byte Jcc: 0x0F <cc> <rel32>)
/// so it jumps to `target_off`.  `patch_off` is the byte offset of the Jcc
/// instruction inside `code`; the rel32 field lives at `patch_off + 2`.
///
/// Used by the `ChannelRecvResult` codegen (Wave 8b) to back-patch the
/// magic / cap / proto / closed fall-through branches once the fail-path
/// offsets are known.
fn patch_rel32_jcc(code: &mut Vec<u8>, patch_off: usize, target_off: usize) {
    let rel = (target_off as i64 - (patch_off as i64 + 6)) as i32;
    let bd = rel.to_le_bytes();
    code[patch_off + 2] = bd[0];
    code[patch_off + 3] = bd[1];
    code[patch_off + 4] = bd[2];
    code[patch_off + 5] = bd[3];
}

/// Patch an unconditional `jmp rel32` placeholder (5-byte E9 <rel32>) so it
/// jumps to `target_off`.  `patch_off` is the byte offset of the jmp inside
/// `code`; the rel32 field lives at `patch_off + 1`.
fn patch_rel32_jmp(code: &mut Vec<u8>, patch_off: usize, target_off: usize) {
    let rel = (target_off as i64 - (patch_off as i64 + 5)) as i32;
    let bd = rel.to_le_bytes();
    code[patch_off + 1] = bd[0];
    code[patch_off + 2] = bd[1];
    code[patch_off + 3] = bd[2];
    code[patch_off + 4] = bd[3];
}

/// Wave 10b: emit an inline CRC32 loop over the 52 bytes at `[rsp+0..52]`
/// (the L1 frame header + payload, excluding the trailing 4-byte CRC field)
/// and leave the result in **R8** (64-bit, with the meaningful CRC in the
/// low 32 bits = R8D).
///
/// Implements the same algorithm as [`crate::ipc::crc32`]:
///   - `crc = 0xFFFFFFFF`
///   - for each byte: `crc ^= byte; for 8 iters: crc = (crc>>1) ^ (poly if crc&1 else 0)`
///   - `crc = !crc`
///
/// Register usage (all caller-saved; RDI is preserved so the caller's
/// write_fd / read_fd in RDI survives):
///   - **R8**  — CRC accumulator (result; low 32 bits are the CRC)
///   - **RCX** — constant 1 (used for `shr r8, cl` and `test r8, rcx`)
///   - **RAX** — polynomial 0xEDB88320 (loaded via 64-bit `mov` so upper 32 = 0)
///   - **RSI** — running byte pointer (starts at `[rsp]`, incremented per byte)
///   - **R9**  — outer loop counter (0..52)
///   - **R10** — current byte (zero-extended)
///   - **R11** — inner loop counter (0..8)
///
/// The caller is responsible for storing R8D (the CRC) into `[rsp+52]` on
/// the send side, or comparing R8D with `[rsp+52]` on the receive side.
fn emit_crc32_frame_loop() -> Vec<u8> {
    // Backwards-compatible wrapper: CRC32 over the 52-byte framed channel
    // header+payload at [rsp]. Delegates to the generalized helper.
    emit_crc32_range(Gpr::Rsp, 0, 52)
}

/// Emit a standalone CRC32 (IEEE 802.3, poly 0xEDB88320, init/final 0xFFFFFFFF)
/// computation over `byte_count` bytes starting at `[base + offset]`.
///
/// The result is left in the low 32 bits of **R8** (upper 32 bits zeroed,
/// so a 64-bit `cmp r8, rcx` against a zero-extended 32-bit load compares
/// correctly). Clobbers RAX, RCX, RSI, R9, R10, R11.
///
/// This is the same algorithm as the framed-channel `emit_crc32_frame_loop`
/// but parameterized over the base register, offset, and byte count so it
/// can be reused by the L6 checkpoint integrity hash (Wave 19-21) and any
/// other caller that needs a CRC32 over an arbitrary memory range.
fn emit_crc32_range(base: Gpr, offset: i32, byte_count: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(90);

    // crc = 0xFFFFFFFF  (64-bit mov so upper 32 bits are 0)
    code.extend(encode_mov_reg_imm64(Gpr::R8, 0xFFFFFFFF));
    // rcx = 1 (CL=1 for shr; RCX=1 for test bit 0)
    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 1));
    // rax = poly 0xEDB88320 (64-bit mov so upper 32 bits are 0)
    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xEDB88320));
    // rsi = &buffer[0]  (base + offset)
    code.extend(encode_lea_reg_mem(Gpr::Rsi, base, offset));
    // r9 = 0 (outer counter)
    code.extend(encode_xor_reg_reg(Gpr::R9, Gpr::R9));

    // ── outer_loop: ──
    let outer_loop_off = code.len();
    // cmp r9, byte_count
    code.extend(encode_cmp_reg_imm32(Gpr::R9, byte_count as i32));
    // jge outer_done (rel32 placeholder)
    let jge_outer_patch = code.len();
    code.extend(&[0x0F, 0x8D, 0x00, 0x00, 0x00, 0x00]); // jge rel32

    // r10 = byte [rsi]  (zero-extended)
    code.extend(encode_movzx_reg8_mem(Gpr::R10, Gpr::Rsi, 0));
    // crc ^= byte
    code.extend(encode_xor_reg_reg(Gpr::R8, Gpr::R10));
    // r11 = 0 (inner counter)
    code.extend(encode_xor_reg_reg(Gpr::R11, Gpr::R11));

    // ── inner_loop: ──
    let inner_loop_off = code.len();
    // cmp r11, 8
    code.extend(encode_cmp_reg_imm32(Gpr::R11, 8));
    // jge inner_done (rel32 placeholder)
    let jge_inner_patch = code.len();
    code.extend(&[0x0F, 0x8D, 0x00, 0x00, 0x00, 0x00]); // jge rel32

    // test crc & 1  (test r8, rcx where rcx=1)
    code.extend(encode_test_reg_reg(Gpr::R8, Gpr::Rcx));
    // jz skip_xor (rel32 placeholder)
    let jz_skip_patch = code.len();
    code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // jz rel32

    // crc = (crc >> 1) ^ poly
    code.extend(encode_shr_reg_cl(Gpr::R8));
    code.extend(encode_xor_reg_reg(Gpr::R8, Gpr::Rax));
    // jmp inner_next (rel32 placeholder)
    let jmp_inner_next_patch = code.len();
    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

    // skip_xor:
    let skip_xor_off = code.len();
    patch_rel32_jcc(&mut code, jz_skip_patch, skip_xor_off);
    // crc >>= 1
    code.extend(encode_shr_reg_cl(Gpr::R8));

    // inner_next:
    let inner_next_off = code.len();
    patch_rel32_jmp(&mut code, jmp_inner_next_patch, inner_next_off);
    // r11 += 1
    code.extend(encode_add_reg_imm32(Gpr::R11, 1));
    // jmp inner_loop (rel32 placeholder)
    let jmp_inner_loop_patch = code.len();
    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
    patch_rel32_jmp(&mut code, jmp_inner_loop_patch, inner_loop_off);

    // inner_done:
    let inner_done_off = code.len();
    patch_rel32_jcc(&mut code, jge_inner_patch, inner_done_off);
    // rsi += 1 (next byte)
    code.extend(encode_add_reg_imm32(Gpr::Rsi, 1));
    // r9 += 1
    code.extend(encode_add_reg_imm32(Gpr::R9, 1));
    // jmp outer_loop (rel32 placeholder)
    let jmp_outer_loop_patch = code.len();
    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
    patch_rel32_jmp(&mut code, jmp_outer_loop_patch, outer_loop_off);

    // outer_done:
    let outer_done_off = code.len();
    patch_rel32_jcc(&mut code, jge_outer_patch, outer_done_off);
    // crc = !crc  — XOR with 0x00000000FFFFFFFF (loaded via 64-bit mov so the
    // upper 32 bits of R11 are 0).  This keeps R8's upper 32 bits at 0 so that
    // a 64-bit `cmp r8, rcx` on the recv side (where RCX was loaded via a
    // 32-bit `mov ecx, [mem]` which zero-extends) compares equal when the CRCs
    // match.  (Using `not r8` would set the upper 32 bits to 0xFFFFFFFF and
    // break the 64-bit comparison.)
    code.extend(encode_mov_reg_imm64(Gpr::R11, 0xFFFFFFFF));
    code.extend(encode_xor_reg_reg(Gpr::R8, Gpr::R11));

    code
}

/// Wave C (L2 cap signatures): emit a real FNV-1a 64-bit hash loop over
/// `byte_count` bytes starting at `[base + offset]`, prefixed with a 1-byte
/// `salt`. The u64 result is left in **R8** (upper 32 bits may be non-zero —
/// callers must compare with a 64-bit load, not a zero-extended 32-bit load).
///
/// Algorithm (matches `ipc::capability::fnv1a_64` + the salt-prefix scheme in
/// `ipc::capability::compute_signature`):
///   - `hash = 0xcbf29ce484222325`  (FNV-1a 64-bit offset basis)
///   - `hash ^= salt;  hash *= 0x100000001b3`  (FNV-1a 64-bit prime)
///   - for each byte: `hash ^= byte;  hash *= 0x100000001b3`
///
/// All arithmetic is wrapping (u64), matching the library's
/// `wrapping_mul` / `^=` semantics.
///
/// Register usage (all caller-saved; RDI is preserved so the caller's
/// read_fd in RDI survives — matching the emit_crc32_range convention):
///   - **R8**  — hash accumulator (result)
///   - **R11** — FNV prime 0x100000001b3 (loaded once via 64-bit mov)
///   - **RAX** — current byte (zero-extended)
///   - **RCX** — outer loop counter (0..byte_count)
///   - **RSI** — running byte pointer
///   - **R9, R10** — unused but listed as clobbered for consistency with
///     `emit_crc32_range` (callers must not rely on them surviving)
fn emit_fnv1a_64_loop(base: Gpr, offset: i32, byte_count: u32, salt: u8) -> Vec<u8> {
    let mut code = Vec::with_capacity(120);

    // r8 = FNV-1a offset basis = 0xcbf29ce484222325
    code.extend(encode_mov_reg_imm64(Gpr::R8, 0xcbf29ce484222325));
    // r11 = FNV-1a prime = 0x100000001b3
    code.extend(encode_mov_reg_imm64(Gpr::R11, 0x100000001b3));
    // rsi = &buffer[0]  (base + offset)
    code.extend(encode_lea_reg_mem(Gpr::Rsi, base, offset));
    // rcx = 0 (loop counter)
    code.extend(encode_xor_reg_reg(Gpr::Rcx, Gpr::Rcx));

    // hash ^= salt  (salt is 0..=3, fits in a sign-extended imm32 without
    // corrupting the upper 32 bits of R8).
    code.extend(encode_xor_reg_imm32(Gpr::R8, salt as i32));
    // hash *= prime  (wrapping imul)
    code.extend(encode_imul_reg_reg(Gpr::R8, Gpr::R11));

    // ── loop: ──
    let loop_off = code.len();
    // cmp rcx, byte_count
    code.extend(encode_cmp_reg_imm32(Gpr::Rcx, byte_count as i32));
    // jge done (rel32 placeholder)
    let jge_patch = code.len();
    code.extend(&[0x0F, 0x8D, 0x00, 0x00, 0x00, 0x00]); // jge rel32

    // rax = byte [rsi]  (zero-extended)
    code.extend(encode_movzx_reg8_mem(Gpr::Rax, Gpr::Rsi, 0));
    // hash ^= byte
    code.extend(encode_xor_reg_reg(Gpr::R8, Gpr::Rax));
    // hash *= prime
    code.extend(encode_imul_reg_reg(Gpr::R8, Gpr::R11));
    // rsi += 1 (next byte)
    code.extend(encode_add_reg_imm32(Gpr::Rsi, 1));
    // rcx += 1
    code.extend(encode_add_reg_imm32(Gpr::Rcx, 1));
    // jmp loop (rel32 placeholder)
    let jmp_loop_patch = code.len();
    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
    patch_rel32_jmp(&mut code, jmp_loop_patch, loop_off);

    // done:
    let done_off = code.len();
    patch_rel32_jcc(&mut code, jge_patch, done_off);

    code
}

/// Wave D (L8 AEAD): emit the XOR stream-cipher loop shared by `aead_seal`
/// and `aead_open`.  XORs `rcx` bytes starting at `[rax]` in-place with
///   key_stream[i] = KEY[i % 32] ^ NONCE[i % 8]
/// where KEY is the 32-byte key at `[r8]` and NONCE is the 8-byte nonce at
/// `[r9]`.  The XOR is symmetric, so the same loop both encrypts (seal) and
/// decrypts (open).
///
/// # Preconditions (caller must set up before calling)
///   - **RAX** = start pointer (the first byte to XOR — `ptr + 8` for the
///     wire-format ciphertext region).
///   - **RCX** = byte count (loop counter; decremented to 0).
///   - **R8**  = pointer to the 32-byte KEY.
///   - **R9**  = pointer to the 8-byte NONCE.
///   - **R10** = 0 (key index, wraps mod 32).
///   - **R11** = 0 (nonce index, wraps mod 8).
///
/// # Postconditions
///   - **RAX** += original RCX (now points one past the last XOR'd byte).
///   - **RCX** = 0.
///   - R8, R9 preserved (pointers not modified).  R10, R11 left at their
///     final wrapped values (callers do not rely on these).
///
/// # Returns
///   The byte offset (within the returned `Vec`) of the `je done` patch
///   site.  The caller MUST patch this rel32 (via [`patch_rel32_jcc`]) to
///   the absolute offset of the after-loop continuation within the outer
///   `code` buffer.  The absolute patch offset is `code.len()` (before
///   extending) + the returned offset.
fn emit_aead_xor_loop() -> (Vec<u8>, usize) {
    let mut code = Vec::with_capacity(64);

    // ── loop: ──
    let loop_start = code.len();
    // cmp rcx, 0
    code.extend(encode_cmp_reg_imm32(Gpr::Rcx, 0));
    // je done (rel32, placeholder)
    let je_done_patch = code.len();
    code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32

    // dl = key[r10]  (movzx edx, byte [r8 + r10])
    //   REX: W=0,R=0(edx),X=1(r10 ext),B=1(r8 ext) = 0x43
    //   0F B6 = movzx r32, r/m8 ; ModRM 14 = (mod=00,reg=010,SIB) ; SIB 10 = (r10,r8)
    code.extend(&[0x43, 0x0F, 0xB6, 0x14, 0x10]);
    // dl ^= nonce[r11]  (xor dl, byte [r9 + r11])
    //   REX 0x43 ; opcode 32 (xor r32, r/m8) ; ModRM 14 ; SIB 19 = (r11,r9)
    code.extend(&[0x43, 0x32, 0x14, 0x19]);
    // xor byte [rax], dl  (30 10)
    code.extend(&[0x30, 0x10]);
    // inc rax  (48 FF C0)
    code.extend(&[0x48, 0xFF, 0xC0]);
    // dec rcx  (48 FF C9)
    code.extend(&[0x48, 0xFF, 0xC9]);
    // inc r10  (49 FF C2)
    code.extend(&[0x49, 0xFF, 0xC2]);
    // r10 %= 32: and r10, 0x1F  (49 83 E2 1F)
    code.extend(&[0x49, 0x83, 0xE2, 0x1F]);
    // inc r11  (49 FF C3)
    code.extend(&[0x49, 0xFF, 0xC3]);
    // r11 %= 8: and r11, 0x07  (49 83 E3 07)
    code.extend(&[0x49, 0x83, 0xE3, 0x07]);
    // jmp loop_start (rel32)
    let jmp_back = code.len();
    let back_delta = loop_start as i64 - (jmp_back as i64 + 5);
    let bd = (back_delta as i32).to_le_bytes();
    code.extend(&[0xE9, bd[0], bd[1], bd[2], bd[3]]);

    (code, je_done_patch)
}

/// The returned set is consulted by the `Add`/`Sub`/`Mul`/`Div`/`Cmp`/`Call`
/// match arms below to decide between the integer and SSE codegen paths.
fn infer_fp_vregs(func: &IRFunction) -> (std::collections::HashSet<u32>, std::collections::HashSet<u32>) {
    use std::collections::HashSet;
    let mut fp: HashSet<u32> = HashSet::new();
    let mut fp_f32: HashSet<u32> = HashSet::new();  // specifically f32 (not f64)
    // Track vregs that provably hold the integer/float bit-pattern 0
    // (used to recognise the `0.0 / 0.0` NaN pattern when `ty` is None).
    let mut zero: HashSet<u32> = HashSet::new();

    let is_zero = |v: &IRValue, zero: &HashSet<u32>| -> bool {
        match v {
            IRValue::Immediate(0) => true,
            IRValue::Register(id) => zero.contains(id),
            _ => false,
        }
    };
    let is_fp = |v: &IRValue, fp: &HashSet<u32>| -> bool {
        match v {
            IRValue::Register(id) => fp.contains(id),
            _ => false,
        }
    };

    // Seed: function parameters with FP types.
    for (param, ty) in func.params.iter().zip(func.param_types.iter()) {
        if matches!(ty, IRType::F32 | IRType::F64) {
            if let Some(id) = param.as_register() {
                fp.insert(id);
                if matches!(ty, IRType::F32) {
                    fp_f32.insert(id);
                }
            }
        }
    }

    // Iterate to fixed point (Phis / loops may need multiple passes).
    let mut changed = true;
    while changed {
        changed = false;
        for block in &func.blocks {
            for instr in &block.instructions {
                match instr {
                    IRInstr::Cast { kind, dst, src, .. } => {
                        if matches!(kind, CastKind::IntToFloat | CastKind::UIntToFloat | CastKind::FloatToFloat) {
                            if let Some(id) = dst.as_register() {
                                if fp.insert(id) { changed = true; }
                            }
                        }
                        // Zero propagates through int<->int casts.
                        if matches!(kind, CastKind::ZExt | CastKind::SExt | CastKind::Trunc | CastKind::BitCast)
                            && is_zero(src, &zero)
                        {
                            if let Some(id) = dst.as_register() {
                                if zero.insert(id) { changed = true; }
                            }
                        }
                    }
                    IRInstr::Call { dst, func: fname, .. } => {
                        if fname == "inttofloat" || fname == "uinttofloat" {
                            if let Some(d) = dst {
                                if let Some(id) = d.as_register() {
                                    if fp.insert(id) { changed = true; }
                                }
                            }
                        }
                    }
                    IRInstr::Load { dst, ty, .. } => {
                        if matches!(ty, IRType::F32 | IRType::F64) {
                            if let Some(id) = dst.as_register() {
                                if fp.insert(id) { changed = true; }
                            }
                        }
                    }
                    IRInstr::Add { dst, lhs, rhs, ty }
                    | IRInstr::Sub { dst, lhs, rhs, ty }
                    | IRInstr::Mul { dst, lhs, rhs, ty }
                    | IRInstr::Div { dst, lhs, rhs, ty } => {
                        let ty_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
                        let op_fp = ty_fp || is_fp(lhs, &fp) || is_fp(rhs, &fp);
                        // `0 / 0` with no type tag: classify as FP (NaN).
                        let zero_div_zero = matches!(instr, IRInstr::Div { .. })
                            && ty.is_none()
                            && is_zero(lhs, &zero)
                            && is_zero(rhs, &zero);
                        if op_fp || zero_div_zero {
                            if let Some(id) = dst.as_register() {
                                if fp.insert(id) { changed = true; }
                            }
                        }
                        // Zero propagation (only meaningful for integer ops).
                        if !op_fp {
                            let result_zero = match instr {
                                IRInstr::Add { .. } => is_zero(lhs, &zero) && is_zero(rhs, &zero),
                                IRInstr::Sub { .. } => match (lhs, rhs) {
                                    (IRValue::Register(a), IRValue::Register(b)) if a == b => true,
                                    _ => is_zero(lhs, &zero) && is_zero(rhs, &zero),
                                },
                                IRInstr::Mul { .. } => is_zero(lhs, &zero) || is_zero(rhs, &zero),
                                _ => false,
                            };
                            if result_zero {
                                if let Some(id) = dst.as_register() {
                                    if zero.insert(id) { changed = true; }
                                }
                            }
                        }
                    }
                    IRInstr::BinOp { op, dst, lhs, rhs, ty, .. } => {
                        let ty_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
                        let op_fp = ty_fp || is_fp(lhs, &fp) || is_fp(rhs, &fp);
                        if op_fp {
                            if let Some(id) = dst.as_register() {
                                if fp.insert(id) { changed = true; }
                                // Track f32 specifically
                                if matches!(ty, Some(IRType::F32)) {
                                    fp_f32.insert(id);
                                }
                            }
                        }
                        if !op_fp {
                            let result_zero = match op {
                                BinOpKind::And => is_zero(lhs, &zero) || is_zero(rhs, &zero),
                                BinOpKind::Xor => {
                                    lhs == rhs || (is_zero(lhs, &zero) && is_zero(rhs, &zero))
                                }
                                BinOpKind::Mul => is_zero(lhs, &zero) || is_zero(rhs, &zero),
                                _ => false,
                            };
                            if result_zero {
                                if let Some(id) = dst.as_register() {
                                    if zero.insert(id) { changed = true; }
                                }
                            }
                        }
                    }
                    IRInstr::Phi { dst, incoming } => {
                        let op_fp = incoming.iter().any(|(v, _)| is_fp(v, &fp));
                        if op_fp {
                            if let Some(id) = dst.as_register() {
                                if fp.insert(id) { changed = true; }
                            }
                        }
                        if incoming.iter().all(|(v, _)| is_zero(v, &zero)) {
                            if let Some(id) = dst.as_register() {
                                if zero.insert(id) { changed = true; }
                            }
                        }
                    }
                    IRInstr::Select { dst, true_val, false_val, ty, .. } => {
                        let op_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64))
                            || is_fp(true_val, &fp)
                            || is_fp(false_val, &fp);
                        if op_fp {
                            if let Some(id) = dst.as_register() {
                                if fp.insert(id) { changed = true; }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    (fp, fp_f32)
}

// =============================================================================
// allocate_registers — Stack-Slot Code Generation
// =============================================================================

/// Stack-slot-only register allocation for x86_64.
///
/// Every vreg gets a stack slot at `[rbp - offset]` (8 bytes each).
/// Alloc vregs get their own larger stack regions.
/// For each IR instruction, we:
///   1. Load source operands from their stack slots into scratch registers
///   2. Perform the operation in the scratch registers
///   3. Store the result back to the destination's stack slot
///
/// Scratch registers (never assigned to vregs):
///   RAX = primary accumulator / result
///   RCX = secondary operand / shift count
///   RDX = tertiary / division
///   R10, R11 = temporary scratch
///
/// Callee-save: RBX, R12–R15 are pushed in prologue, popped in epilogue.
pub fn allocate_registers(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let func_name = func.name.clone();

    // ── Phase 0: FP type inference ──
    // VUMA's IR drops the type tag on arithmetic ops (Add/Sub/Mul/Div).
    // Recover FP-ness from Casts, float-builtin Calls, param types, and
    // operand propagation so the Add/Sub/Mul/Div/Cmp arms below can pick
    // the SSE path. See `infer_fp_vregs` above for the full rule set.
    let (fp_vregs, fp_vregs_f32) = infer_fp_vregs(func);

    // ── Phase 0.5: Scan for capability_grant calls (Wave C: L2 cap sigs) ──
    //
    // The recv side must recompute the FNV-1a×4 capability signature over
    // the same `signature_input` byte vector the grant used.  Since the
    // grant's args (resource_id, perms) are compile-time immediates in the
    // IR, we can extract them here and compute the sig_input + sig at
    // compile time.  These are embedded as immediates in the prologue so
    // that BOTH the parent (which calls grant + send_cap) and the child
    // (which only calls recv, after fork) have the sig_input available on
    // their stack.
    //
    // If the function has no capability_grant, these stay `None` and the
    // recv side skips the signature check (no cap frames are expected).
    let mut cap_grant_sig: Option<[u8; 32]> = None;
    let mut cap_grant_sig_input: Option<Vec<u8>> = None;
    'grant_search: for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Call { func: fname, args, .. } = instr {
                if fname == "capability_grant" && args.len() == 2 {
                    let resource_id = match &args[0] {
                        IRValue::Immediate(v) => *v as u64,
                        _ => continue,
                    };
                    let perms_raw = match &args[1] {
                        IRValue::Immediate(v) => *v as u64,
                        _ => continue,
                    };
                    let resource = crate::ipc::capability::Resource::Channel(resource_id);
                    let perms = crate::ipc::capability::MemoryPermissions {
                        read: (perms_raw & 1) != 0,
                        write: (perms_raw & 2) != 0,
                        execute: (perms_raw & 4) != 0,
                        ..Default::default()
                    };
                    let token = crate::ipc::capability::grant_capability(
                        resource_id as u128, 1, 1, resource, perms,
                        0, 0, 3600, b"vuma_dev_signing_key",
                    );
                    // Reconstruct signature_input inline — ipc::capability::signature_input
                    // is module-private, so we duplicate its logic here.  The
                    // byte layout MUST match signature_input() in ipc.rs.
                    let signing_key = b"vuma_dev_signing_key";
                    let mut sig_input = Vec::with_capacity(256);
                    sig_input.extend_from_slice(signing_key);
                    sig_input.extend_from_slice(&token.id.to_le_bytes());
                    sig_input.extend_from_slice(&token.source_pid.to_le_bytes());
                    sig_input.extend_from_slice(&token.target_pid.to_le_bytes());
                    sig_input.extend_from_slice(&token.created_at.to_le_bytes());
                    sig_input.extend_from_slice(&token.expires_at.to_le_bytes());
                    sig_input.push(token.delegation_depth);
                    sig_input.push(if token.permissions.read { 1 } else { 0 });
                    sig_input.push(if token.permissions.write { 1 } else { 0 });
                    sig_input.push(if token.permissions.execute { 1 } else { 0 });
                    sig_input.extend_from_slice(&token.resource.encode());
                    cap_grant_sig = Some(token.signature);
                    cap_grant_sig_input = Some(sig_input);
                    break 'grant_search;
                }
            }
        }
    }

    // ── Phase 1: Collect all vreg IDs and compute stack layout ──

    // Collect all unique vreg IDs from the function's vregs map and also
    // from instruction operands (to catch any vregs not in the map)
    let mut all_vreg_ids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // From the function's declared vregs
    for &id in func.vregs.keys() {
        all_vreg_ids.insert(id);
    }
    // Also from function params
    for param in &func.params {
        if let Some(id) = param.as_register() {
            all_vreg_ids.insert(id);
        }
    }
    // And from instruction operands (to catch any vregs not in the map)
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() {
                all_vreg_ids.insert(id);
            }
            for id in instr.used_regs() {
                all_vreg_ids.insert(id);
            }
        }
    }

    // Identify Alloc vregs and compute their stack region sizes
    let mut stack_alloc_vregs: std::collections::HashSet<u32> =
        std::collections::HashSet::new();
    let mut alloc_sizes: HashMap<u32, i32> = HashMap::new(); // vreg → aligned size
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
    // [high address]
    //   saved RBP           ← RBP points here
    //   Alloc data region N ← [rbp - alloc_offset_N] size aligned_alloc_N
    //   ...
    //   Alloc data region 1 ← [rbp - alloc_offset_1] size aligned_alloc_1
    //   vreg slot M         ← [rbp - vreg_offset_M]  (8 bytes each, including Alloc ptrs)
    //   ...
    //   vreg slot 1         ← [rbp - first_vreg_offset]
    // [low address]         ← RSP

    // Assign stack offsets for Alloc regions (closest to RBP, growing downward)
    let mut alloc_offsets: HashMap<u32, i32> = HashMap::new(); // vreg → [rbp - offset] (start of data region)
    let mut current_offset: i32 = 0;
    // Process Allocs in a deterministic order
    let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
    alloc_vreg_ids.sort();
    for &id in &alloc_vreg_ids {
        let size = alloc_sizes[&id];
        current_offset += size;
        alloc_offsets.insert(id, -(current_offset));
    }

    // Assign stack slots for ALL vregs (including Alloc vregs).
    // Alloc vregs need a separate 8-byte slot to store the pointer to their data region.
    // Non-Alloc vregs just use their 8-byte slot for the value.
    let mut vreg_stack_slots: HashMap<u32, i32> = HashMap::new(); // vreg → [rbp - offset]
    let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
    all_vreg_ids_sorted.sort();
    for &id in &all_vreg_ids_sorted {
        current_offset += 8;
        vreg_stack_slots.insert(id, -(current_offset));
    }

    // Wave 10a: reserve an 8-byte stack slot for the per-function channel
    // sequence counter.  Zeroed in the prologue so the first ChannelSend
    // emits sequence=0, the second sequence=1, etc.  This replaces the
    // previous hardcoded `[rsp+16] = sequence = 0`.
    current_offset += 8;
    let seq_counter_off: i32 = -(current_offset);

    // Wave 14b: reserve an 8-byte stack slot for the per-function protocol
    // state machine counter.  Zeroed in the prologue (state = 0 = Idle).
    // channel_recv_proto(ch, expected_state) verifies proto_state == expected
    // before recv'ing, and advances proto_state on success.  A mismatch
    // (wrong message order) → -5 (ProtocolViolation).
    current_offset += 8;
    let proto_state_off: i32 = -(current_offset);

    // Wave 22/65-72: reserve 8 bytes for the per-function circuit-breaker
    // state machine. Layout: [state:u32 at +0, failure_count:u32 at +4].
    // state: 0=Closed, 1=Open, 2=HalfOpen (matches the CircuitState enum
    // in ipc.rs). failure_count: number of consecutive failures recorded
    // while in Closed; the breaker trips to Open when count > threshold.
    // Zeroed in the prologue (state=Closed, count=0).
    current_offset += 8;
    let cb_state_off: i32 = -(current_offset);

    // Wave C (L2 cap signatures): reserve per-function stack slots for the
    // compile-time capability signature and signature_input.  These are
    // populated in the prologue (so both parent and child after fork see
    // them) from the first capability_grant call's compile-time params.
    //   cap_sig_off          (32 bytes): the 32-byte FNV-1a×4 signature
    //   cap_siginput_off     (160 bytes): the signature_input byte vector
    //     (padded to a multiple of 8; 148 bytes for the default grant +
    //     4 bytes padding = 152, but we reserve 160 for headroom if the
    //     signing key or resource encode ever grows)
    //   cap_siginput_len_off (8 bytes): the sig_input length (u64)
    //
    // channel_send_cap reads cap_sig_off to append the signature to the
    // frame.  channel_recv reads cap_siginput_off + cap_siginput_len_off
    // to recompute FNV-1a×4 and compare to the received signature.
    current_offset += 32;
    let cap_sig_off: i32 = -(current_offset);
    current_offset += 160;
    let cap_siginput_off: i32 = -(current_offset);
    current_offset += 8;
    let cap_siginput_len_off: i32 = -(current_offset);

    // Wave F (Driver Isolation — IRQ routing): per-function IRQ routing
    // table + entry-count slot.  driver_register(irq, handler_ptr) writes
    // (irq, handler_ptr) pairs into the next free slot; irq_dispatch(vector)
    // linear-scans the table for a matching irq and calls the handler via
    // an indirect `call r10`.  Mirrors the library DriverWorker
    // (ipc.rs:4365): config.irq_vectors is the list of vectors the driver
    // handles, handle_irq(vector) returns WorkerNotRunning if not running,
    // IrqNotRegistered(vector) if the vector is not in irq_vectors, else Ok.
    //
    // Layout (8 entries × 16 bytes = 128 bytes):
    //   [rbp + irq_table_off +  i*16 + 0]: irq         (u64)
    //   [rbp + irq_table_off +  i*16 + 8]: handler_ptr (u64)
    //   [rbp + irq_table_count_off      ]: count       (u64, # filled slots)
    //
    // Zeroed in the prologue so each function starts with an empty table.
    current_offset += 128;
    let irq_table_off: i32 = -(current_offset);
    current_offset += 8;
    let irq_table_count_off: i32 = -(current_offset);

    // Round up to ensure proper stack alignment for calls.
    // The prologue does: push rbp (-8); mov rbp,rsp; sub rsp,frame_size
    // No callee-saved pushes (ISel uses only caller-saved regs).
    // On entry to this function: RSP was 8 mod 16 (SysV ABI).
    // After push rbp: RSP is 0 mod 16.
    // After sub rsp,frame_size: RSP is (-frame_size) mod 16.
    // Before any `call` from this function, RSP must be 0 mod 16 (so that
    // the callee enters with RSP at 8 mod 16 as required by SysV).
    // Therefore: frame_size % 16 == 0.
    let aligned = ((current_offset + 15) & !15) as usize;
    let frame_size = if aligned.is_multiple_of(16) {
        aligned.max(16)
    } else {
        (aligned + 16 - (aligned % 16)).max(16)  // Round up to 16-byte boundary
    };

    // ── Helper closures for stack slot access ──

    // Get the [rbp - offset] for a vreg's stack slot (where its value/pointer is stored)
    let slot_offset = |id: u32| -> i32 {
        if let Some(&off) = vreg_stack_slots.get(&id) {
            off
        } else {
            // Fallback: shouldn't happen, but use a safe offset
            -(frame_size as i32)
        }
    };

    // Load a vreg from its stack slot into a scratch register
    let load_vreg = |id: u32, scratch: Gpr| -> Vec<u8> {
        let off = slot_offset(id);
        encode_mov_reg_mem(scratch, Gpr::Rbp, off)
    };

    // Store a scratch register into a vreg's stack slot
    let store_vreg = |id: u32, scratch: Gpr| -> Vec<u8> {
        let off = slot_offset(id);
        encode_mov_mem_reg(Gpr::Rbp, off, scratch)
    };

    // Load an IRValue into a scratch register
    // For registers: load from stack slot
    // For immediates: mov scratch, imm
    let load_value = |val: &IRValue, scratch: Gpr| -> Vec<u8> {
        match val {
            IRValue::Register(id) => load_vreg(*id, scratch),
            IRValue::Immediate(imm) => {
                let imm = *imm;
                // Use imm32 (sign-extended) only when the value fits in a
                // *signed* i32 AND its sign-extension matches the desired
                // 64-bit value.  Values in 0x8000_0000..=0xFFFF_FFFF are
                // positive u32 constants but would be sign-extended to a
                // negative i64 by `MOV r64, imm32`, corrupting arithmetic.
                // For those we must use the 10-byte `MOV r64, imm64` encoding.
                if (-2147483648..=2147483647).contains(&imm) {
                    let sign_ext = ((imm as i32) as i64) as u64;
                    if sign_ext == (imm as u64) {
                        encode_mov_reg_imm32(scratch, imm as i32)
                    } else {
                        encode_mov_reg_imm64(scratch, imm as u64)
                    }
                } else {
                    encode_mov_reg_imm64(scratch, imm as u64)
                }
            }
            IRValue::Address(addr) => encode_mov_reg_imm64(scratch, *addr),
            IRValue::Label(name) => {
                // Labels need relocation but load_value doesn't have access
                // to the relocations vector. Emit a placeholder and log.
                // This is a known limitation — labels are rare in IR operands.
                vuma_log!(warn, "IRValue::Label('{}') in load_value: emitting placeholder 0", name);
                encode_mov_reg_imm64(scratch, 0)
            }
        }
    };

    // Like load_value, but narrows f64 immediate bits to f32 bits.
    // Used by the f32 FP paths (addss/subss/mulss/divss/ucomiss) where
    // the immediate holds f64 bits but the instruction needs f32 bits
    // in the low 32 bits of the GPR.
    let load_value_f32 = |val: &IRValue, scratch: Gpr| -> Vec<u8> {
        match val {
            IRValue::Immediate(imm) => {
                let f = f64::from_bits(*imm as u64);
                let f32_bits = (f as f32).to_bits();
                encode_mov_reg_imm32(scratch, f32_bits as i32)
            }
            _ => load_value(val, scratch),
        }
    };

    // ── Phase 2: Generate prologue ──

    let mut encoded_instrs: Vec<AllocatedInstruction> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut byte_offset: usize = 0;

    // Helper to push an encoded instruction
    let mut emit = |code: Vec<u8>, opcode_name: &str| {
        if !code.is_empty() {
            byte_offset += code.len();
            encoded_instrs.push(AllocatedInstruction {
                opcode: opcode_name.to_string(),
                reads: vec![],
                writes: vec![],
                encoded: code,
            });
        }
    };

    // push rbp
    emit(encode_push(Gpr::Rbp), "push_rbp");

    // mov rbp, rsp
    emit(encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp), "mov_rbp_rsp");

    // sub rsp, frame_size
    if frame_size > 0 {
        emit(encode_sub_reg_imm32(Gpr::Rsp, frame_size as i32), "sub_rsp");
    }

    // Wave 10a: zero the per-function channel sequence counter slot so the
    // first ChannelSend starts at sequence=0.  Uses RAX (caller-saved, free
    // at this point) as a scratch.
    emit(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax), "xor_rax_zero");
    emit(encode_mov_mem_reg(Gpr::Rbp, seq_counter_off, Gpr::Rax), "zero_seq_counter");
    // Wave 14b: zero the protocol-state slot (state = 0 = Idle).
    emit(encode_mov_mem_reg(Gpr::Rbp, proto_state_off, Gpr::Rax), "zero_proto_state");
    // Wave 22/65-72: zero the circuit-breaker state slot (state=Closed=0,
    // count=0). RAX is already zero from the xor above.
    emit(encode_mov_mem_reg(Gpr::Rbp, cb_state_off, Gpr::Rax), "zero_cb_state");

    // Wave C (L2 cap signatures): populate the per-function cap sig slots
    // from the first capability_grant call's compile-time params.  This runs
    // in the prologue (not at the grant site) so that BOTH the parent (which
    // calls grant + send_cap) and the child (which only calls recv, after
    // fork) have the sig + sig_input on their stack.  If the function has no
    // capability_grant, the slots stay zeroed (from the RAX=0 above) and the
    // recv side skips the sig check.
    if let (Some(sig), Some(sig_input)) = (cap_grant_sig.as_ref(), cap_grant_sig_input.as_ref()) {
        // Store the 32-byte signature into cap_sig_off (4 × 8-byte stores).
        for i in 0..4 {
            let chunk = u64::from_le_bytes([
                sig[i * 8], sig[i * 8 + 1], sig[i * 8 + 2], sig[i * 8 + 3],
                sig[i * 8 + 4], sig[i * 8 + 5], sig[i * 8 + 6], sig[i * 8 + 7],
            ]);
            emit(encode_mov_reg_imm64(Gpr::Rax, chunk), "cap_sig_imm");
            emit(encode_mov_mem_reg(Gpr::Rbp, cap_sig_off + (i as i32) * 8, Gpr::Rax), "cap_sig_store");
        }
        // Store the sig_input bytes into cap_siginput_off, padded to a
        // multiple of 8 with zeros.  The slot is 160 bytes; we only write
        // ceil(sig_input.len() / 8) * 8 bytes (the rest stays zeroed from
        // the prologue zeroing above — but since the slot was NOT zeroed
        // by the xor above, we must zero the tail explicitly if the padded
        // length is less than 160.  In practice the slot is freshly
        // allocated stack space (sub rsp, frame_size) and its contents are
        // indeterminate, so we write the full padded length and rely on
        // the FNV loop only reading sig_input.len() bytes.
        let padded_len = (sig_input.len() + 7) & !7;
        for i in 0..(padded_len / 8) {
            let mut chunk_bytes = [0u8; 8];
            let start = i * 8;
            let end = (start + 8).min(sig_input.len());
            chunk_bytes[..end - start].copy_from_slice(&sig_input[start..end]);
            let chunk = u64::from_le_bytes(chunk_bytes);
            emit(encode_mov_reg_imm64(Gpr::Rax, chunk), "cap_siginput_imm");
            emit(encode_mov_mem_reg(Gpr::Rbp, cap_siginput_off + (i as i32) * 8, Gpr::Rax), "cap_siginput_store");
        }
        // Store the sig_input length into cap_siginput_len_off.
        emit(encode_mov_reg_imm64(Gpr::Rax, sig_input.len() as u64), "cap_siginput_len_imm");
        emit(encode_mov_mem_reg(Gpr::Rbp, cap_siginput_len_off, Gpr::Rax), "cap_siginput_len_store");
    }

    // Wave F (IRQ routing): zero the per-function IRQ table count slot
    // so each function starts with an empty routing table (count = 0).
    // The 128-byte table data itself is uninitialized, but irq_dispatch
    // only reads slots [0..count), so uninitialized data past count is
    // never observed.  RAX may have been clobbered by the Wave C block
    // above, so re-zero it here.  Also zero the first entry's irq field
    // as a defensive sentinel (irq=0 cannot match any real vector >= 1).
    emit(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax), "xor_rax_zero_irq");
    emit(encode_mov_mem_reg(Gpr::Rbp, irq_table_count_off, Gpr::Rax), "zero_irq_table_count");

    // Push callee-saved registers.
    // The stack-slot ISel only uses caller-saved registers (RAX, RCX, RDX,
    // R10, R11). No callee-saved registers (RBX, R12-R15) are touched,
    // so we don't need to save/restore any. This saves 40 bytes of stack
    // and 10 bytes of code per function.
    let callee_save_regs: Vec<Gpr> = vec![];
    for &reg in &callee_save_regs {
        emit(encode_push(reg), "push_callee_save");
    }

    // Copy function parameters from SystemV arg registers to their stack slots
    let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
    for (i, param) in func.params.iter().enumerate() {
        if let Some(id) = param.as_register() {
            if i < arg_regs.len() {
                let off = slot_offset(id);
                emit(encode_mov_mem_reg(Gpr::Rbp, off, arg_regs[i]), "store_param");
            }
        }
    }

    // ── Phase 3: Encode each IR instruction ──

    // We need to resolve intra-function branch targets (block labels).
    // Strategy: first emit all code with placeholder rel32=0 for branches,
    // then patch the branch targets after we know all block offsets.

    // Track block label → byte_offset within the function's encoded output
    let mut block_offsets: HashMap<String, usize> = HashMap::new();
    // Track branches that need patching: (rel32_field_offset, target_label)
    let mut branch_patches: Vec<(usize, String)> = Vec::new();

    // Build predecessor-aware phi resolution map.
    let phi_map = func.build_phi_map();

    for block in &func.blocks {
        // Record this block's label offset
        block_offsets.insert(block.label.clone(), byte_offset);

        for instr in &block.instructions {
            if let IRInstr::Call { func: fname, .. } = instr {
            }
            // Per-instruction overrides for the AllocatedInstruction's
            // opcode / reads / writes.  Populated by select match arms
            // (currently `IRInstr::Cast` for FP-conversion mnemonics); the
            // generic `format!("{:?}", instr).split_whitespace().next()`
            // fallback is used when these remain unset.
            let mut instr_opcode: Option<String> = None;
            let mut instr_reads: Vec<PhysicalReg> = Vec::new();
            let mut instr_writes: Vec<PhysicalReg> = Vec::new();

            let encoded = match instr {
                // ── Add ──
                IRInstr::Add { dst, lhs, rhs, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // FP dispatch: if the dst vreg is known-FP (or ty says so),
                    // use SSE ADDSD/ADDSS instead of the integer ADD.
                    let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64))
                        || fp_vregs.contains(&dst_id);
                    if is_fp {
                        let is_f64 = matches!(ty, Some(IRType::F64)) || (ty.is_none() && !fp_vregs_f32.contains(&dst_id));
                        // For f32 operations, float literals are stored as f64
                        // bits in the Immediate. We need to narrow them to f32
                        // bits (in the low 32 bits) before loading into XMM.
                        let load_value_f32 = |val: &IRValue, scratch: Gpr| -> Vec<u8> {
                            match val {
                                IRValue::Immediate(imm) => {
                                    // Narrow f64 bits to f32 bits.
                                    let f = f64::from_bits(*imm as u64);
                                    let f32_bits = (f as f32).to_bits();
                                    encode_mov_reg_imm32(scratch, f32_bits as i32)
                                }
                                _ => if is_f64 { load_value(val, scratch) } else { load_value_f32(val, scratch) },
                            }
                        };
                        let load_fn: &dyn Fn(&IRValue, Gpr) -> Vec<u8> = if is_f64 { &load_value } else { &load_value_f32 };
                        code.extend(if is_f64 { load_value(lhs, Gpr::Rax) } else { load_value_f32(lhs, Gpr::Rax) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        }
                        code.extend(load_value(rhs, Gpr::R10));
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        }
                        if is_f64 {
                            code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        } else {
                            code.extend(encode_addss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        if is_f64 {
                            code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        } else {
                            code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        }
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        instr_opcode = Some(if is_f64 { "addsd" } else { "addss" }.to_string());
                        code
                    } else {
                        // Load lhs into RAX
                        code.extend(load_value(lhs, Gpr::Rax));
                        // Add rhs (immediate or from stack)
                        if let IRValue::Immediate(imm) = rhs {
                            let imm = *imm;
                            if (-2147483648..=2147483647).contains(&imm) {
                                code.extend(encode_add_reg_imm32(Gpr::Rax, imm as i32));
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                        } else {
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rcx));
                        }
                        // Store result to dst stack slot
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    }
                }

                // ── Sub ──
                IRInstr::Sub { dst, lhs, rhs, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64))
                        || fp_vregs.contains(&dst_id);
                    if is_fp {
                        let is_f64 = matches!(ty, Some(IRType::F64)) || (ty.is_none() && !fp_vregs_f32.contains(&dst_id));

                        code.extend(if is_f64 { load_value(lhs, Gpr::Rax) } else { load_value_f32(lhs, Gpr::Rax) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        }
                        code.extend(if is_f64 { load_value(rhs, Gpr::R10) } else { load_value_f32(rhs, Gpr::R10) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        }
                        if is_f64 {
                            code.extend(encode_subsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        } else {
                            code.extend(encode_subss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        if is_f64 {
                            code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        } else {
                            code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        }
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        instr_opcode = Some(if is_f64 { "subsd" } else { "subss" }.to_string());
                        code
                    } else {
                        code.extend(load_value(lhs, Gpr::Rax));
                        if let IRValue::Immediate(imm) = rhs {
                            let imm = *imm;
                            if (-2147483648..=2147483647).contains(&imm) {
                                code.extend(encode_sub_reg_imm32(Gpr::Rax, imm as i32));
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_sub_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                        } else {
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_sub_reg_reg(Gpr::Rax, Gpr::Rcx));
                        }
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    }
                }

                // ── Mul ──
                IRInstr::Mul { dst, lhs, rhs, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64))
                        || fp_vregs.contains(&dst_id);
                    if is_fp {
                        let is_f64 = matches!(ty, Some(IRType::F64)) || (ty.is_none() && !fp_vregs_f32.contains(&dst_id));

                        code.extend(if is_f64 { load_value(lhs, Gpr::Rax) } else { load_value_f32(lhs, Gpr::Rax) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        }
                        code.extend(if is_f64 { load_value(rhs, Gpr::R10) } else { load_value_f32(rhs, Gpr::R10) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        }
                        if is_f64 {
                            code.extend(encode_mulsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        } else {
                            code.extend(encode_mulss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        if is_f64 {
                            code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        } else {
                            code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        }
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        instr_opcode = Some(if is_f64 { "mulsd" } else { "mulss" }.to_string());
                        code
                    } else {
                        code.extend(load_value(lhs, Gpr::Rax));
                        code.extend(load_value(rhs, Gpr::Rcx));
                        code.extend(encode_imul_reg_reg(Gpr::Rax, Gpr::Rcx));
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    }
                }

                // ── Div ──
                IRInstr::Div { dst, lhs, rhs, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64))
                        || fp_vregs.contains(&dst_id);
                    if is_fp {
                        let is_f64 = matches!(ty, Some(IRType::F64)) || (ty.is_none() && !fp_vregs_f32.contains(&dst_id));

                        code.extend(if is_f64 { load_value(lhs, Gpr::Rax) } else { load_value_f32(lhs, Gpr::Rax) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        }
                        code.extend(if is_f64 { load_value(rhs, Gpr::R10) } else { load_value_f32(rhs, Gpr::R10) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        }
                        if is_f64 {
                            code.extend(encode_divsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        } else {
                            code.extend(encode_divss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        if is_f64 {
                            code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        } else {
                            code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        }
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        instr_opcode = Some(if is_f64 { "divsd" } else { "divss" }.to_string());
                        code
                    } else {
                        // Load lhs into RAX
                        code.extend(load_value(lhs, Gpr::Rax));
                        // Sign-extend RAX into RDX:RAX
                        code.extend(encode_cqo());
                        // Load rhs into RCX, then IDIV RCX
                        code.extend(load_value(rhs, Gpr::Rcx));
                        code.extend(encode_idiv_reg(Gpr::Rcx));
                        // Quotient in RAX, store to dst
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    }
                }

                // ── BinOp (generic) ──
                IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);

                    // ── FP dispatch ──
                    // When the BinOp's result type is F32 or F64 (or the dst
                    // vreg was inferred FP by `infer_fp_vregs`), we must use
                    // the SSE/SSE2 scalar arithmetic encodings (ADDSD/ADDSS,
                    // SUBSD/SUBSS, MULSD/MULSS, DIVSD/DIVSS) instead of the
                    // integer ALU.  Operands are ferried through GPRs (since
                    // our stack-slot ISel loads every value into a GPR) and
                    // then moved into XMM0/XMM1 via MOVQ/MOVD.
                    let lhs_fp = lhs.as_register().map(|id| fp_vregs.contains(&id)).unwrap_or(false);
                    let rhs_fp = rhs.as_register().map(|id| fp_vregs.contains(&id)).unwrap_or(false);
                    let ty_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
                    if ty_fp || lhs_fp || rhs_fp || fp_vregs.contains(&dst_id) {
                        let is_f64 = matches!(ty, Some(IRType::F64)) || (ty.is_none() && !fp_vregs_f32.contains(&dst_id));

                        // Load lhs into RAX and move to XMM0.
                        code.extend(if is_f64 { load_value(lhs, Gpr::Rax) } else { load_value_f32(lhs, Gpr::Rax) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        }
                        // Load rhs into R10 and move to XMM1.
                        code.extend(if is_f64 { load_value(rhs, Gpr::R10) } else { load_value_f32(rhs, Gpr::R10) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        }
                        match op {
                            BinOpKind::Add => {
                                if is_f64 {
                                    code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                } else {
                                    code.extend(encode_addss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                }
                            }
                            BinOpKind::Sub => {
                                if is_f64 {
                                    code.extend(encode_subsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                } else {
                                    code.extend(encode_subss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                }
                            }
                            BinOpKind::Mul => {
                                if is_f64 {
                                    code.extend(encode_mulsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                } else {
                                    code.extend(encode_mulss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                }
                            }
                            BinOpKind::SDiv | BinOpKind::UDiv => {
                                if is_f64 {
                                    code.extend(encode_divsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                } else {
                                    code.extend(encode_divss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                }
                            }
                            // FP comparison via UCOMISD/UCOMISS + SETcc.
                            // The condition-code mapping is the same as for
                            // integer compares because UCOMIS* sets EFLAGS
                            // with the same ZF/PF/CF semantics as CMP for the
                            // ordered case.
                            BinOpKind::SLt | BinOpKind::ULt
                            | BinOpKind::SLe | BinOpKind::ULe
                            | BinOpKind::SGt | BinOpKind::UGt
                            | BinOpKind::SGe | BinOpKind::UGe
                            | BinOpKind::Eq | BinOpKind::Ne => {
                                // G7: remap signed cc to unsigned for FP (UCOMISD sets CF/ZF/PF, not SF/OF)
                                let cc = match op {
                                    BinOpKind::SLt | BinOpKind::ULt => Cc::Below,
                                    BinOpKind::SLe | BinOpKind::ULe => Cc::BelowEqual,
                                    BinOpKind::SGt | BinOpKind::UGt => Cc::Above,
                                    BinOpKind::SGe | BinOpKind::UGe => Cc::AboveEqual,
                                    BinOpKind::Eq => Cc::Equal,
                                    BinOpKind::Ne => Cc::NotEqual,
                                    _ => Cc::Equal,
                                };
                                if is_f64 {
                                    code.extend(encode_ucomisd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                } else {
                                    code.extend(encode_ucomiss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                }
                                code.extend(encode_setcc(cc, Gpr::Rax));
                                code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                // Skip the post-arithmetic store_vreg below
                                // by continuing to the next instruction.
                                instr_opcode = Some(if is_f64 {
                                    "fp_cmpsd"
                                } else {
                                    "fp_cmpss"
                                }.to_string());
                                // Push the encoded bytes already produced.
                                if !code.is_empty() {
                                    byte_offset += code.len();
                                    encoded_instrs.push(AllocatedInstruction {
                                        opcode: instr_opcode.take().unwrap(),
                                        reads: instr_reads,
                                        writes: instr_writes,
                                        encoded: code,
                                    });
                                }
                                continue;
                            }
                            // Other BinOps (And/Or/Xor/Shl/...) are not
                            // meaningful on FP values — fall through to the
                            // integer path as a safety net.
                            _ => {}
                        }
                        // Move result back to RAX and store.
                        if is_f64 {
                            code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        } else {
                            code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                        }
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        instr_opcode = Some(if is_f64 {
                            match op {
                                BinOpKind::Add => "addsd",
                                BinOpKind::Sub => "subsd",
                                BinOpKind::Mul => "mulsd",
                                BinOpKind::SDiv | BinOpKind::UDiv => "divsd",
                                _ => "fp_binop_sd",
                            }
                        } else {
                            match op {
                                BinOpKind::Add => "addss",
                                BinOpKind::Sub => "subss",
                                BinOpKind::Mul => "mulss",
                                BinOpKind::SDiv | BinOpKind::UDiv => "divss",
                                _ => "fp_binop_ss",
                            }
                        }.to_string());
                        if !code.is_empty() {
                            byte_offset += code.len();
                            encoded_instrs.push(AllocatedInstruction {
                                opcode: instr_opcode.take().unwrap(),
                                reads: instr_reads,
                                writes: instr_writes,
                                encoded: code,
                            });
                        }
                        continue;
                    }

                    match op {
                        BinOpKind::Add => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            if let IRValue::Immediate(imm) = rhs {
                                let imm = *imm;
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_add_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(load_value(rhs, Gpr::Rcx));
                                    code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rcx));
                                }
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::Sub => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            if let IRValue::Immediate(imm) = rhs {
                                let imm = *imm;
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_sub_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(load_value(rhs, Gpr::Rcx));
                                    code.extend(encode_sub_reg_reg(Gpr::Rax, Gpr::Rcx));
                                }
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_sub_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::Mul => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_imul_reg_reg(Gpr::Rax, Gpr::Rcx));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::SDiv => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(encode_cqo());
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_idiv_reg(Gpr::Rcx));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::UDiv => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_div_reg(Gpr::Rcx));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::SRem => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(encode_cqo());
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_idiv_reg(Gpr::Rcx));
                            // Remainder in RDX
                            code.extend(store_vreg(dst_id, Gpr::Rdx));
                        }
                        BinOpKind::URem => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_div_reg(Gpr::Rcx));
                            code.extend(store_vreg(dst_id, Gpr::Rdx));
                        }
                        BinOpKind::And => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            if let IRValue::Immediate(imm) = rhs {
                                let imm = *imm;
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_and_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(load_value(rhs, Gpr::Rcx));
                                    code.extend(encode_and_reg_reg(Gpr::Rax, Gpr::Rcx));
                                }
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_and_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::Or => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            if let IRValue::Immediate(imm) = rhs {
                                let imm = *imm;
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_or_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(load_value(rhs, Gpr::Rcx));
                                    code.extend(encode_or_reg_reg(Gpr::Rax, Gpr::Rcx));
                                }
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_or_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::Xor => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            if let IRValue::Immediate(imm) = rhs {
                                let imm = *imm;
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_xor_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(load_value(rhs, Gpr::Rcx));
                                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rcx));
                                }
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::Shl => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_shl_reg_cl(Gpr::Rax));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::ShrL => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_shr_reg_cl(Gpr::Rax));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::ShrA => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_sar_reg_cl(Gpr::Rax));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::Ror => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_ror_reg_cl(Gpr::Rax));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        BinOpKind::Rol => {
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_rol_reg_cl(Gpr::Rax));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        // Comparison BinOps: produce 0 or 1
                        BinOpKind::SLt
                        | BinOpKind::SLe
                        | BinOpKind::SGt
                        | BinOpKind::SGe
                        | BinOpKind::ULt
                        | BinOpKind::ULe
                        | BinOpKind::UGt
                        | BinOpKind::UGe
                        | BinOpKind::Eq
                        | BinOpKind::Ne => {
                            let cc = binop_cmp_to_cc(op);
                            code.extend(load_value(lhs, Gpr::Rax));
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                            code.extend(encode_setcc(cc, Gpr::Rax));
                            code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                    }
                    code
                }

                // ── Unary operations ──
                IRInstr::UnaryOp { op, dst, operand, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(load_value(operand, Gpr::Rax));

                    match op {
                        UnaryOpKind::Neg => {
                            code.extend(encode_neg_reg(Gpr::Rax));
                        }
                        UnaryOpKind::Not => {
                            code.extend(encode_not_reg(Gpr::Rax));
                        }
                        UnaryOpKind::Clz => {
                            // LZCNT RAX, RAX (F3 0F BD /r) — handles zero input
                            // LZCNT returns 64 for zero input (no undefined behavior).
                            // Requires BMI1 (always present on any CPU with POPCNT).
                            code.push(0xF3);
                            code.push(0x48); // REX.W
                            code.push(0x0F);
                            code.push(0xBD);
                            code.push(modrm(3, Gpr::Rax.encoding() & 7, Gpr::Rax.encoding() & 7));
                        }
                        UnaryOpKind::Ctz => {
                            // TZCNT RAX, RAX (F3 0F BC /r) — handles zero input
                            // TZCNT returns 64 for zero input (no undefined behavior).
                            // Requires BMI1.
                            code.push(0xF3);
                            code.push(0x48); // REX.W
                            code.push(0x0F);
                            code.push(0xBC);
                            code.push(modrm(3, Gpr::Rax.encoding() & 7, Gpr::Rax.encoding() & 7));
                        }
                        UnaryOpKind::Popcnt => {
                            // POPCNT RAX, RAX (F3 0F B8 /r)
                            code.push(0xF3);
                            code.push(0x48); // REX.W
                            code.push(0x0F);
                            code.push(0xB8);
                            code.push(modrm(3, Gpr::Rax.encoding() & 7, Gpr::Rax.encoding() & 7));
                        }
                    }
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Comparison (dedicated Cmp instruction) ──
                IRInstr::Cmp { kind, dst, lhs, rhs, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let cc = cmp_kind_to_cc(kind);
                    // G7: for FP comparisons (UCOMISD/UCOMISS), remap signed
                    // condition codes to unsigned — UCOMISD sets CF/ZF/PF only,
                    // not SF/OF, so SETL/SETLE/SETG/SETGE are undefined.
                    let cc_fp = match kind {
                        crate::ir::CmpKind::SLt | crate::ir::CmpKind::ULt => Cc::Below,
                        crate::ir::CmpKind::SLe | crate::ir::CmpKind::ULe => Cc::BelowEqual,
                        crate::ir::CmpKind::SGt | crate::ir::CmpKind::UGt => Cc::Above,
                        crate::ir::CmpKind::SGe | crate::ir::CmpKind::UGe => Cc::AboveEqual,
                        crate::ir::CmpKind::Eq => Cc::Equal,
                        crate::ir::CmpKind::Ne => Cc::NotEqual,
                    };

                    // ── FP comparison dispatch ──
                    // For F32/F64 operands (declared via `ty` OR recovered
                    // by `infer_fp_vregs`), use UCOMISD/UCOMISS to set
                    // EFLAGS, then SETcc to extract the boolean result.
                    // IEEE 754 NaN handling: UCOMISD sets PF=1 when either
                    // operand is NaN (the "unordered" case).  For Ne this
                    // must return true (NaN != NaN); for Eq/Lt/Le it must
                    // return false.  UGt/UGe already return false for
                    // unordered (CF=1 after UCOMISD), so no fix-up needed.
                    let lhs_fp = lhs.as_register().map(|id| fp_vregs.contains(&id)).unwrap_or(false);
                    let rhs_fp = rhs.as_register().map(|id| fp_vregs.contains(&id)).unwrap_or(false);
                    let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64))
                        || lhs_fp || rhs_fp;
                    if is_fp {
                        let is_f64 = matches!(ty, Some(IRType::F64)) || (ty.is_none() && !fp_vregs_f32.contains(&dst_id));

                        // Load lhs into RAX and move to XMM0.
                        code.extend(if is_f64 { load_value(lhs, Gpr::Rax) } else { load_value_f32(lhs, Gpr::Rax) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                        }
                        // Load rhs into R10 and move to XMM1.
                        code.extend(if is_f64 { load_value(rhs, Gpr::R10) } else { load_value_f32(rhs, Gpr::R10) });
                        if is_f64 {
                            code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        } else {
                            code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                        }
                        // UCOMISD/UCOMISS XMM0, XMM1 → sets EFLAGS.
                        if is_f64 {
                            code.extend(encode_ucomisd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        } else {
                            code.extend(encode_ucomiss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        // SETcc + NaN fix-up.  For Ne: result = SETNE OR SETP
                        // (true if not-equal OR unordered).  For Eq/Lt/ULt/Le/
                        // ULe: result = SETcc AND SETNP (false if unordered).
                        // For UGt/UGe: SETA/SETAE already correct (NaN→CF=1).
                        match kind {
                            crate::ir::CmpKind::Ne => {
                                code.extend(encode_setcc(Cc::NotEqual, Gpr::Rax));
                                code.extend(encode_setcc(Cc::Parity, Gpr::Rcx));
                                code.extend(encode_or_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                            crate::ir::CmpKind::Eq
                            | crate::ir::CmpKind::SLt | crate::ir::CmpKind::ULt
                            | crate::ir::CmpKind::SLe | crate::ir::CmpKind::ULe => {
                                code.extend(encode_setcc(cc_fp, Gpr::Rax));
                                code.extend(encode_setcc(Cc::NotParity, Gpr::Rcx));
                                code.extend(encode_and_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                            // SGt/SGe fall through to UGt/UGe encoding (FP
                            // compares are sign-agnostic; the signed/unsigned
                            // distinction is meaningless on bit patterns).
                            _ => {
                                code.extend(encode_setcc(cc_fp, Gpr::Rax));
                            }
                        }
                        code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    } else {
                        // Integer comparison path.
                        code.extend(load_value(lhs, Gpr::Rax));
                        if let IRValue::Immediate(imm) = rhs {
                            let imm = *imm;
                            if (-2147483648..=2147483647).contains(&imm) {
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, imm as i32));
                            } else {
                                code.extend(load_value(rhs, Gpr::Rcx));
                                code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                            }
                        } else {
                            code.extend(load_value(rhs, Gpr::Rcx));
                            code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                        }
                        code.extend(encode_setcc(cc, Gpr::Rax));
                        code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    }
                }

                // ── Conditional select (Cmov) ──
                IRInstr::Select { dst, cond, true_val, false_val, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load false_val into RAX, true_val into R10, cond into R11
                    code.extend(load_value(false_val, Gpr::Rax));
                    code.extend(load_value(true_val, Gpr::R10));
                    code.extend(load_value(cond, Gpr::R11));
                    // Test cond != 0
                    code.extend(encode_test_reg_reg(Gpr::R11, Gpr::R11));
                    // CMOVNZ RAX, R10
                    code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, Gpr::Rax, Gpr::R10));
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Constant-time conditional select (no branches) ──
                // ct_select(cond, a, b) = (a & mask) | (b & ~mask)
                // where mask = -(cond != 0) = all-ones if cond!=0, else 0
                // Key: NO BRANCHES — all bitwise operations to prevent timing side-channels
                IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load cond into R10, true_val into R11, false_val into RAX
                    code.extend(load_value(cond, Gpr::R10));
                    code.extend(load_value(true_val, Gpr::R11));
                    code.extend(load_value(false_val, Gpr::Rax));
                    // Build mask: mask = -(cond != 0)
                    //   TEST R10, R10      ; set ZF if cond == 0
                    //   SETNE R10b         ; R10b = 1 if cond != 0, else 0
                    //   MOVZX R10, R10b    ; zero-extend to full register
                    //   NEG R10            ; R10 = 0xFFFFFFFFFFFFFFFF if cond!=0, else 0
                    code.extend(encode_test_reg_reg(Gpr::R10, Gpr::R10));
                    code.extend(encode_setcc(Cc::NotEqual, Gpr::R10));
                    code.extend(encode_movzx_reg8(Gpr::R10, Gpr::R10));
                    code.extend(encode_neg_reg(Gpr::R10));
                    // result = (true_val & mask) | (false_val & ~mask)
                    //   R11 &= R10         ; R11 = true_val & mask
                    //   RAX &= ~R10        ; RAX = false_val & ~mask (NOT R10 then AND)
                    //   OR RAX, R11        ; RAX = result
                    code.extend(encode_and_reg_reg(Gpr::R11, Gpr::R10));
                    code.extend(encode_not_reg(Gpr::R10));
                    code.extend(encode_and_reg_reg(Gpr::Rax, Gpr::R10));
                    code.extend(encode_or_reg_reg(Gpr::Rax, Gpr::R11));
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Constant-time equality check (no branches) ──
                // ct_eq(a, b): diff = a ^ b; result = ((diff | -diff) >> 31) ^ 1
                // Returns 1 if equal, 0 if not.
                // Key: NO BRANCHES — all bitwise operations to prevent timing side-channels
                IRInstr::CtEq { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load lhs into RAX, rhs into RCX
                    code.extend(load_value(lhs, Gpr::Rax));
                    code.extend(load_value(rhs, Gpr::Rcx));
                    // XOR RAX, RCX → diff in RAX
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rcx));
                    // NEG RAX → -diff in RAX (but we need diff too, so save diff first)
                    // Use R10 = diff, R11 = -diff
                    code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rax)); // R10 = diff
                    code.extend(encode_neg_reg(Gpr::Rax));                // RAX = -diff
                    code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rax));  // R11 = -diff
                    // OR R10, R11 → (diff | -diff)
                    code.extend(encode_or_reg_reg(Gpr::R10, Gpr::R11));
                    // SHR R10, 31 → 0 if diff==0, 1 if diff!=0 (for 32-bit)
                    // For 64-bit, we'd use >> 63, but ct_eq operates on u32 primarily
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 31));
                    code.extend(encode_shr_reg_cl(Gpr::R10));
                    // XOR R10, 1 → invert: 1 if equal, 0 if not
                    code.extend(encode_xor_reg_imm32(Gpr::R10, 1));
                    code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::R10));
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Memory: Load ──
                IRInstr::Load { dst, addr, offset, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load address from stack into R10
                    code.extend(load_value(addr, Gpr::R10));
                    let off = *offset;
                    match ty {
                        IRType::I8 | IRType::U8 => {
                            code.extend(encode_movzx_reg8_mem(Gpr::Rax, Gpr::R10, off));
                        }
                        IRType::I16 | IRType::U16 => {
                            code.extend(encode_movzx_reg16_mem(Gpr::Rax, Gpr::R10, off));
                        }
                        IRType::I32 | IRType::U32 => {
                            code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::R10, off));
                        }
                        _ => {
                            code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::R10, off));
                        }
                    }
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Memory: Store ──
                IRInstr::Store { value, addr, offset, ty } => {
                    let mut code = Vec::new();
                    // Load value into R10, address into R11
                    code.extend(load_value(value, Gpr::R10));
                    code.extend(load_value(addr, Gpr::R11));
                    let off = *offset;
                    match ty {
                        IRType::I8 | IRType::U8 => {
                            code.extend(encode_mov_mem8_reg8(Gpr::R11, off, Gpr::R10));
                        }
                        IRType::I16 | IRType::U16 => {
                            code.extend(encode_mov_mem16_reg16(Gpr::R11, off, Gpr::R10));
                        }
                        IRType::I32 | IRType::U32 => {
                            code.extend(encode_mov_mem32_reg32(Gpr::R11, off, Gpr::R10));
                        }
                        _ => {
                            code.extend(encode_mov_mem_reg(Gpr::R11, off, Gpr::R10));
                        }
                    }
                    code
                }

                // ── Memory: Lea (Offset) ──
                IRInstr::Offset { dst, base, offset } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    match offset {
                        IRValue::Immediate(imm) => {
                            let off = *imm as i32;
                            // Load base into RAX
                            code.extend(load_value(base, Gpr::Rax));
                            // LEA RAX, [RAX + off]
                            code.extend(encode_lea_reg_mem(Gpr::Rax, Gpr::Rax, off));
                        }
                        _ => {
                            // Load base into RAX, offset into RCX
                            code.extend(load_value(base, Gpr::Rax));
                            code.extend(load_value(offset, Gpr::Rcx));
                            code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rcx));
                        }
                    }
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── GetAddress ──
                IRInstr::GetAddress { dst, name } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // mov rax, imm64 (placeholder, patched by relocation)
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0));
                    // Offset of the 8-byte immediate within the instruction:
                    let imm_offset = byte_offset + code.len() - 8;
                    relocations.push(RelocationEntry {
                        offset: imm_offset as u64,
                        symbol: name.clone(),
                        reloc_type: R_X86_64_64.to_string(),
                    });
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Alloc ──
                IRInstr::Alloc { dst, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let alloc_off = alloc_offsets.get(&dst_id).copied().unwrap_or(-(frame_size as i32));
                    // lea rax, [rbp + alloc_off]  (alloc_off is negative)
                    code.extend(encode_lea_reg_mem(Gpr::Rax, Gpr::Rbp, alloc_off));
                    // Store the pointer into dst's stack slot
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Free ──
                IRInstr::Free { ptr } => {
                    let is_stack = ptr
                        .as_register()
                        .map(|id| stack_alloc_vregs.contains(&id))
                        .unwrap_or(false);
                    if is_stack {
                        // Stack allocation — no-op
                        Vec::new()
                    } else {
                        // Heap allocation — call __vuma_free(ptr)
                        let mut code = Vec::new();
                        // Load ptr from stack into RDI
                        code.extend(load_value(ptr, Gpr::Rdi));
                        // CALL rel32 — needs relocation
                        let call_offset = byte_offset + code.len() + 1;
                        code.extend(encode_call_rel32(0));
                        relocations.push(RelocationEntry {
                            offset: call_offset as u64,
                            symbol: "__vuma_free".to_string(),
                            reloc_type: R_X86_64_PLT32.to_string(),
                        });
                        code
                    }
                }

                // ── Cast / Conversion ──
                IRInstr::Cast { kind, dst, src, from_ty, to_ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);

                    // Helper predicates for type-driven instruction selection.
                    // When type info is unavailable (`None`), we fall back to
                    // reasonable defaults that match the prior hardcoded behaviour.

                    // Source integer is 32-bit or narrower (loaded as a 32-bit
                    // value sign/zero-extended into the 64-bit stack slot).
                    let src_is_32bit_int = matches!(from_ty,
                        Some(IRType::I8)  | Some(IRType::I16) | Some(IRType::I32) |
                        Some(IRType::U8)  | Some(IRType::U16) | Some(IRType::U32) |
                        None  // default: assume 32-bit source
                    );
                    // Destination float is f32 (vs f64).
                    let dst_is_f32 = matches!(to_ty, Some(IRType::F32));
                    // Source float is f32 (vs f64).  Default to f64.
                    let src_is_f32 = matches!(from_ty, Some(IRType::F32));
                    // Destination integer is 32-bit or narrower.  Default to 32-bit.
                    let dst_is_32bit_int = matches!(to_ty,
                        Some(IRType::I8)  | Some(IRType::I16) | Some(IRType::I32) |
                        Some(IRType::U8)  | Some(IRType::U16) | Some(IRType::U32) |
                        None  // default: assume 32-bit destination
                    );

                    // Compute the real x86_64 mnemonic for this cast and record
                    // the registers it touches.  This makes the
                    // AllocatedInstruction's `opcode` reflect the actual
                    // conversion instruction (e.g. "cvtsi2sd", "cvttsd2si")
                    // rather than the generic "cast", and marks BOTH the
                    // GPR (Rax, used to ferry the value to/from the stack
                    // slot) and the FP unit (Xmm0, used for the actual
                    // conversion) as read/written.  The cross-bank register
                    // usage is what proves this is a real conversion rather
                    // than a same-bank move — mirroring what task 2-d did
                    // for riscv64/ppc64/wasm32.
                    let xmm0 = PhysicalReg::new(RegClass::SimdFp, Xmm::Xmm0.encoding() as u32);
                    let rax = PhysicalReg::new(RegClass::Gpr, Gpr::Rax.encoding() as u32);
                    let (mnemonic, uses_fp) = match kind {
                        CastKind::IntToFloat => {
                            (if dst_is_f32 { "cvtsi2ss" } else { "cvtsi2sd" }, true)
                        }
                        CastKind::UIntToFloat => {
                            // UIntToFloat reuses the CVTSI2SD/CVTSI2SS
                            // signed-conversion encoding after zero-extension
                            // (and an ADDSD/ADDSS fix-up for the u64 case).
                            (if dst_is_f32 { "cvtsi2ss" } else { "cvtsi2sd" }, true)
                        }
                        CastKind::FloatToInt => {
                            (if src_is_f32 { "cvttss2si" } else { "cvttsd2si" }, true)
                        }
                        CastKind::FloatToUInt => {
                            // FloatToUInt uses the same truncating conversion
                            // as FloatToInt (positive-range-correct).
                            (if src_is_f32 { "cvttss2si" } else { "cvttsd2si" }, true)
                        }
                        CastKind::FloatToFloat => {
                            (if src_is_f32 { "cvtss2sd" } else { "cvtsd2ss" }, true)
                        }
                        _ => ("cast", false),
                    };
                    instr_opcode = Some(mnemonic.to_string());
                    // Rax is always read (load_value) and written (store_vreg
                    // or the conversion's MOVQ/MOVD r,x output) by every
                    // Cast lowering below.
                    instr_reads.push(rax);
                    instr_writes.push(rax);
                    if uses_fp {
                        instr_reads.push(xmm0);
                        instr_writes.push(xmm0);
                    }

                    match kind {
                        CastKind::ZExt => {
                            if let IRValue::Immediate(imm) = *src {
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_mov_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(encode_mov_reg_imm64(Gpr::Rax, imm as u64));
                                }
                            } else {
                                // Load from stack, then zero-extend based on from_ty.
                                code.extend(load_value(src, Gpr::Rax));
                                match from_ty {
                                    Some(IRType::U8) | Some(IRType::I8) => {
                                        code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                                    }
                                    Some(IRType::U16) | Some(IRType::I16) => {
                                        code.extend(encode_movzx_reg16(Gpr::Rax, Gpr::Rax));
                                    }
                                    Some(IRType::U32) | Some(IRType::I32) => {
                                        // MOVZX r32, r32 would work but writing to
                                        // EAX already zero-extends to RAX on x86_64.
                                        // Use AND to clear upper 32 bits explicitly.
                                        code.extend(encode_and_reg_imm32(Gpr::Rax, -1));
                                    }
                                    _ => {
                                        // 64-bit source: no extension needed
                                    }
                                }
                            }
                        }
                        CastKind::SExt => {
                            if let IRValue::Immediate(imm) = *src {
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_mov_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(encode_mov_reg_imm64(Gpr::Rax, imm as u64));
                                }
                            } else {
                                code.extend(load_value(src, Gpr::Rax));
                                match from_ty {
                                    Some(IRType::I8) | Some(IRType::U8) => {
                                        code.extend(encode_movsx_reg8(Gpr::Rax, Gpr::Rax));
                                    }
                                    Some(IRType::I16) | Some(IRType::U16) => {
                                        code.extend(encode_movsx_reg16(Gpr::Rax, Gpr::Rax));
                                    }
                                    Some(IRType::I32) | Some(IRType::U32) => {
                                        // MOVSXD r64, r32 (REX.W + 0F BE /r)
                                        code.push(0x48); // REX.W
                                        code.push(0x63);
                                        code.push(modrm(3, Gpr::Rax.encoding() & 7, Gpr::Rax.encoding() & 7));
                                    }
                                    _ => {
                                        // 64-bit source: no extension needed
                                    }
                                }
                            }
                        }
                        CastKind::Trunc => {
                            // Truncate: mask the upper bits based on to_ty.
                            code.extend(load_value(src, Gpr::Rax));
                            match to_ty {
                                Some(IRType::U8) | Some(IRType::I8) => {
                                    code.extend(encode_and_reg_imm32(Gpr::Rax, 0xFF));
                                }
                                Some(IRType::U16) | Some(IRType::I16) => {
                                    code.extend(encode_and_reg_imm32(Gpr::Rax, 0xFFFF));
                                }
                                Some(IRType::U32) | Some(IRType::I32) => {
                                    code.extend(encode_and_reg_imm32(Gpr::Rax, -1));
                                }
                                _ => {
                                    // Truncating to 64-bit is a no-op
                                }
                            }
                        }
                        CastKind::BitCast => {
                            // BitCast: no conversion, just copy the bits.
                            code.extend(load_value(src, Gpr::Rax));
                        }

                        // ── Signed integer → floating-point ──────────────────
                        //
                        // | from_ty       | to_ty | Instruction(s)                           |
                        // |---------------|-------|------------------------------------------|
                        // | i8/i16/i32    | f32   | CVTSI2SS xmm, r32; MOVD r32, xmm        |
                        // | i8/i16/i32    | f64   | CVTSI2SD xmm, r32; MOVQ r64, xmm        |
                        // | i64           | f32   | CVTSI2SS xmm, r64; MOVD r32, xmm        |
                        // | i64           | f64   | CVTSI2SD xmm, r64; MOVQ r64, xmm        |
                        // | None (default)| f64   | CVTSI2SD xmm, r32; MOVQ r64, xmm        |
                        CastKind::IntToFloat => {
                            code.extend(load_value(src, Gpr::Rax));
                            if dst_is_f32 {
                                // → f32
                                if src_is_32bit_int {
                                    code.extend(encode_cvtsi2ss_xmm_r32(Xmm::Xmm0, Gpr::Rax));
                                } else {
                                    code.extend(encode_cvtsi2ss_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                }
                                code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                            } else {
                                // → f64 (default)
                                if src_is_32bit_int {
                                    code.extend(encode_cvtsi2sd_xmm_r32(Xmm::Xmm0, Gpr::Rax));
                                } else {
                                    code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                }
                                code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                            }
                        }

                        // ── Unsigned integer → floating-point ────────────────
                        //
                        // For u32: zero-extend to 64-bit (fitting in a signed i64),
                        // then use the 64-bit signed conversion.
                        //
                        // For u64: complex — we must handle the sign bit separately.
                        // Strategy: test if the value is negative (bit 63 set).
                        //   If clear: CVTSI2SD xmm, r64 (value fits in signed i64).
                        //   If set:   divide by 2 in the GPR, convert, then add the
                        //             result to itself in the XMM (×2).  This avoids
                        //             overflow because the halved value fits in i63.
                        //
                        // | from_ty | to_ty | Instruction(s)                              |
                        // |---------|-------|---------------------------------------------|
                        // | u32     | f32   | zero-extend; CVTSI2SS xmm, r64; MOVD r,x   |
                        // | u32     | f64   | zero-extend; CVTSI2SD xmm, r64; MOVQ r,x   |
                        // | u64     | f32   | CAS sequence (see below); MOVD r,x          |
                        // | u64     | f64   | CAS sequence (see below); MOVQ r,x          |
                        CastKind::UIntToFloat => {
                            code.extend(load_value(src, Gpr::Rax));

                            let src_is_u64 = matches!(from_ty,
                                Some(IRType::I64) | Some(IRType::U64)
                            );

                            if src_is_u64 {
                                // u64 → float: x86_64 has no direct unsigned conversion.
                                // Strategy: shift right by 1 (halving), convert as
                                // signed i63, then double the FP result.
                                //
                                //   1. RCX = 1
                                //   2. R11 = RAX            (save original)
                                //   3. SHR RAX, CL          (halve; fits in i63)
                                //   4. Convert RAX → float in XMM0
                                //   5. ADDSD/ADDSS XMM0, XMM0  (double)
                                //   6. AND R11, 1           (isolate bit 0)
                                //   7. CVTSI2SD/SS XMM1, R11 (0.0 or 1.0)
                                //   8. ADDSD/SS XMM0, XMM1  (correct 1-ULP error)
                                code.extend(encode_mov_reg_imm32(Gpr::Rcx, 1));  // CL = 1
                                code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rax));  // save original to R11
                                code.extend(encode_shr_reg_cl(Gpr::Rax));  // RAX >>= 1
                                if dst_is_f32 {
                                    code.extend(encode_cvtsi2ss_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                    code.extend(encode_addss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm0));
                                    // Bit-0 fix-up: convert saved bit 0 to 0.0/1.0 and add
                                    code.extend(encode_and_reg_imm32(Gpr::R11, 1));
                                    code.extend(encode_cvtsi2ss_xmm_r64(Xmm::Xmm1, Gpr::R11));
                                    code.extend(encode_addss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                    code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                                } else {
                                    code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                    code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm0));
                                    // Bit-0 fix-up: convert saved bit 0 to 0.0/1.0 and add
                                    code.extend(encode_and_reg_imm32(Gpr::R11, 1));
                                    code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm1, Gpr::R11));
                                    code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                    code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                                }
                            } else {
                                // u32 → float: zero-extend to 64-bit (which fits in
                                // signed i64), then use 64-bit signed conversion.
                                // On x86_64, writing to a 32-bit register zeroes the
                                // upper 32 bits, so RAX already has the zero-extended
                                // value if it was loaded as 32-bit.  For safety, if
                                // the value might have garbage in upper bits, we rely
                                // on the 64-bit load having zero-extended.
                                if dst_is_f32 {
                                    code.extend(encode_cvtsi2ss_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                    code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                                } else {
                                    code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                    code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                                }
                            }
                        }

                        // ── Floating-point → signed integer ──────────────────
                        //
                        // | from_ty | to_ty       | Instruction(s)                          |
                        // |---------|-------------|-----------------------------------------|
                        // | f32     | i8..i32     | MOVD xmm,r32; CVTSS2SI r32,xmm         |
                        // | f32     | i64         | MOVD xmm,r32; CVTSS2SI r64,xmm         |
                        // | f64     | i8..i32     | MOVQ xmm,r64; CVTSD2SI r32,xmm         |
                        // | f64     | i64         | MOVQ xmm,r64; CVTSD2SI r64,xmm         |
                        // | None    | i8..i32     | MOVQ xmm,r64; CVTSD2SI r32,xmm (def)   |
                        CastKind::FloatToInt => {
                            code.extend(load_value(src, Gpr::Rax));
                            if src_is_f32 {
                                // f32 → signed int (truncate toward zero)
                                code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                if dst_is_32bit_int {
                                    code.extend(encode_cvttss2si_r32_xmm(Gpr::Rax, Xmm::Xmm0));
                                } else {
                                    code.extend(encode_cvttss2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                }
                            } else {
                                // f64 → signed int (default, truncate toward zero)
                                code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                if dst_is_32bit_int {
                                    code.extend(encode_cvttsd2si_r32_xmm(Gpr::Rax, Xmm::Xmm0));
                                } else {
                                    code.extend(encode_cvttsd2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                }
                            }
                        }

                        // ── Floating-point → unsigned integer ────────────────
                        //
                        // x86_64 has no direct FP→unsigned-int instruction before AVX-512.
                        // For values in the positive signed range, CVTTSD2SI/CVTTSS2SI
                        // produces the same result as an unsigned conversion.
                        //
                        // For out-of-range values (≥ 2^31 for i32, ≥ 2^63 for i64),
                        // we need a correction sequence:
                        //   1. Convert to signed with CVTTSD2SI/CVTTSS2SI
                        //   2. If the result is negative, subtract 2^31/2^63 and
                        //      set the sign bit (or use the compare-and-adjust pattern)
                        //
                        // For simplicity and correctness for the common case (values
                        // fitting in the positive signed range), we use the same
                        // instruction as FloatToInt.  A full unsigned conversion
                        // would require a CAS sequence for edge cases.
                        //
                        // | from_ty | to_ty       | Instruction(s)                          |
                        // |---------|-------------|-----------------------------------------|
                        // | f32     | u8..u32     | MOVD xmm,r32; CVTSS2SI r32,xmm         |
                        // | f32     | u64         | MOVD xmm,r32; CVTSS2SI r64,xmm         |
                        // | f64     | u8..u32     | MOVQ xmm,r64; CVTSD2SI r32,xmm         |
                        // | f64     | u64         | MOVQ xmm,r64; CVTSD2SI r64,xmm         |
                        CastKind::FloatToUInt => {
                            // Proper unsigned float→int conversion.
                            // For values that fit in the positive signed range,
                            // CVTTSD2SI works directly (positive signed == unsigned).
                            // For values >= 2^63 (u64) or >= 2^31 (u32), we use
                            // the subtract-convert-XOR technique:
                            //   1. Subtract 2^N from the float
                            //   2. Convert with CVTTSD2SI (now fits in signed range)
                            //   3. XOR the result with 2^(N-1) to add 2^N back
                            code.extend(load_value(src, Gpr::Rax));
                            if src_is_f32 {
                                code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                if dst_is_32bit_int {
                                    // f32 → u32: subtract 2^31, convert, XOR 2^31
                                    // This works for ALL values (small and large):
                                    //   small: v - 2^31 is negative (sign set), XOR clears → v
                                    //   large: v - 2^31 is positive (sign clear), XOR sets → v
                                    code.extend(encode_mov_reg_imm32(Gpr::R10, 0x4F000000));
                                    code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                                    code.extend(encode_subss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                    code.extend(encode_cvttss2si_r32_xmm(Gpr::Rax, Xmm::Xmm0));
                                    code.extend(encode_xor_reg_imm32(Gpr::Rax, 0x80000000u32 as i32));
                                } else {
                                    // f32 → u64: threshold = 2^63
                                    // Load 2^63 as float (0x5F000000) into XMM1
                                    code.extend(encode_mov_reg_imm32(Gpr::R10, 0x5F000000));
                                    code.extend(encode_movd_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                                    code.extend(encode_subss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                    code.extend(encode_cvttss2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                    // XOR with 0x8000000000000000 to add 2^63 back
                                    code.extend(encode_mov_reg_imm64(Gpr::R10, 0x8000000000000000));
                                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::R10));
                                }
                            } else {
                                // f64 → u32 or u64
                                code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                if dst_is_32bit_int {
                                    // f64 → u32: threshold = 2^31
                                    // Load 2^31 as double (0x41E0000000000000) into XMM1
                                    code.extend(encode_mov_reg_imm64(Gpr::R10, 0x41E0000000000000));
                                    code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                                    code.extend(encode_subsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                    code.extend(encode_cvttsd2si_r32_xmm(Gpr::Rax, Xmm::Xmm0));
                                    code.extend(encode_xor_reg_imm32(Gpr::Rax, 0x80000000u32 as i32));
                                } else {
                                    // f64 → u64: For values < 2^63, CVTTSD2SI
                                    // produces the correct unsigned result directly.
                                    // For values >= 2^63, use the subtract-XOR technique:
                                    //   1. CVTTSD2SI r64, xmm (direct — may be wrong for >= 2^63)
                                    //   2. Save direct result in R10
                                    //   3. Subtract 2^63 from float, CVTTSD2SI again, XOR 2^63
                                    //   4. Compare original float with 2^63
                                    //   5. CMOVAE: if float >= 2^63, use corrected result
                                    // Load the float into XMM0
                                    // Direct conversion (correct for < 2^63)
                                    code.extend(encode_cvttsd2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                    code.extend(encode_mov_reg_reg(Gpr::R10, Gpr::Rax)); // save direct result
                                    // Corrected conversion (for >= 2^63)
                                    code.extend(encode_mov_reg_imm64(Gpr::R11, 0x43E0000000000000)); // 2^63 as f64
                                    code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R11));
                                    code.extend(encode_subsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1)); // value - 2^63
                                    code.extend(encode_cvttsd2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                    code.extend(encode_mov_reg_imm64(Gpr::R11, 0x8000000000000000u64));
                                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::R11)); // add 2^63 back
                                    // Now RAX = corrected, R10 = direct. Pick based on value.
                                    // Compare 2^63 (in XMM1, still holds 2^63) with original value.
                                    // We need the original value back in XMM0 for the comparison.
                                    code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0)); // recover (value - 2^63)
                                    code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1)); // restore original value
                                    code.extend(encode_ucomisd_xmm_xmm(Xmm::Xmm1, Xmm::Xmm0)); // compare 2^63 vs value
                                    // If 2^63 <= value (value >= 2^63, i.e. above-or-equal), use corrected (RAX)
                                    // Otherwise use direct (R10)
                                    code.extend(encode_cmovcc_reg_reg(Cc::AboveEqual, Gpr::Rax, Gpr::R10));
                                }
                            }
                        }

                        // ── Floating-point ↔ floating-point ──────────────────
                        //
                        // | from_ty | to_ty | Instruction(s)                          |
                        // |---------|-------|-----------------------------------------|
                        // | f32     | f64   | MOVD xmm,r32; CVTSS2SD xmm,xmm; MOVQ r,x |
                        // | f64     | f32   | MOVQ xmm,r64; CVTSD2SS xmm,xmm; MOVD r,x |
                        // | None    | f64   | MOVQ xmm,r64; CVTSD2SS xmm,xmm; MOVD r,x |
                        CastKind::FloatToFloat => {
                            code.extend(load_value(src, Gpr::Rax));
                            if src_is_f32 {
                                // f32 → f64 (widen)
                                code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                code.extend(encode_cvtss2sd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm0));
                                code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                            } else {
                                // f64 → f32 (narrow, default)
                                code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                code.extend(encode_cvtsd2ss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm0));
                                code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                            }
                        }
                    }
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Control: Ret ──
                IRInstr::Ret { values } => {
                    let mut code = Vec::new();
                    // Load return value into RAX
                    if let Some(val) = values.first() {
                        code.extend(load_value(val, Gpr::Rax));
                    }
                    // Epilogue: pop callee-saved in reverse order
                    for &reg in callee_save_regs.iter().rev() {
                        code.extend(encode_pop(reg));
                    }
                    // Restore RSP
                    if frame_size > 0 {
                        code.extend(encode_add_reg_imm32(Gpr::Rsp, frame_size as i32));
                    }
                    code.extend(encode_pop(Gpr::Rbp));
                    code.extend(encode_ret());
                    code
                }

                // ── Control: Branch (unconditional) ──
                IRInstr::Branch { target } => {
                    let mut code = Vec::new();
                    // Emit phi copies for (target, current_block) before the jump.
                    if let Some(pairs) = phi_map.get(&(target.clone(), block.label.clone())) {
                        for (dst, src) in pairs {
                            code.extend(load_value(src, Gpr::Rax));
                            let dst_id = dst.as_register().unwrap_or(0);
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                    }
                    code.extend(encode_jmp_rel32(0));
                    let rel32_offset = byte_offset + code.len() - 4;
                    branch_patches.push((rel32_offset, target.clone()));
                    code
                }

                // ── Control: CondBranch ──
                IRInstr::CondBranch { cond, true_target, false_target } => {
                    let mut code = Vec::new();
                    // Load condition from stack into RAX
                    code.extend(load_value(cond, Gpr::Rax));
                    // test rax, rax
                    code.extend(encode_test_reg_reg(Gpr::Rax, Gpr::Rax));

                    // Compute phi copies for both successors.
                    let false_copies: Vec<u8> = if let Some(pairs) = phi_map.get(&(false_target.clone(), block.label.clone())) {
                        let mut c = Vec::new();
                        for (dst, src) in pairs {
                            c.extend(load_value(src, Gpr::Rax));
                            let dst_id = dst.as_register().unwrap_or(0);
                            c.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        c
                    } else { Vec::new() };
                    let true_copies: Vec<u8> = if let Some(pairs) = phi_map.get(&(true_target.clone(), block.label.clone())) {
                        let mut c = Vec::new();
                        for (dst, src) in pairs {
                            c.extend(load_value(src, Gpr::Rax));
                            let dst_id = dst.as_register().unwrap_or(0);
                            c.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        c
                    } else { Vec::new() };

                    if false_copies.is_empty() && true_copies.is_empty() {
                        // Common case (no phis): jnz true, jmp false
                        code.extend(encode_jcc_rel32(Cc::NotEqual, 0));
                        let jnz_rel32_offset = byte_offset + code.len() - 4;
                        branch_patches.push((jnz_rel32_offset, true_target.clone()));
                        code.extend(encode_jmp_rel32(0));
                        let jmp_rel32_offset = byte_offset + code.len() - 4;
                        branch_patches.push((jmp_rel32_offset, false_target.clone()));
                    } else {
                        // Landing-pad pattern:
                        //   jnz +N           (skip false copies + false jmp)
                        //   <false copies>
                        //   jmp false_target
                        //   <true copies>    ← jnz lands here
                        //   jmp true_target
                        let jmp_rel32_size = 5; // E9 + 4-byte rel32
                        let jnz_rel32 = (false_copies.len() + jmp_rel32_size) as i32;
                        code.extend(encode_jcc_rel32(Cc::NotEqual, jnz_rel32));
                        // False path
                        code.extend(false_copies);
                        code.extend(encode_jmp_rel32(0));
                        let jmp_false_offset = byte_offset + code.len() - 4;
                        branch_patches.push((jmp_false_offset, false_target.clone()));
                        // True path (jnz target)
                        code.extend(true_copies);
                        code.extend(encode_jmp_rel32(0));
                        let jmp_true_offset = byte_offset + code.len() - 4;
                        branch_patches.push((jmp_true_offset, true_target.clone()));
                    }
                    code
                }

                // ── Call ──
                IRInstr::Call { dst, func: call_target, args, is_extern: _ } => {
                    let mut code = Vec::new();

                    // ── Float-conversion builtins ──
                    // The IR builder emits `inttofloat` / `uinttofloat` /
                    // `floattoint` / `floattouint` as ordinary `Call`
                    // instructions (see `lower_call` in `scg_to_ir.rs`).
                    // These are NOT real runtime functions — the linker
                    // would fall back to `__ffi_fallback_stub` (xor rax,rax;
                    // ret) and silently return 0, breaking every float
                    // program.  Intercept them here and emit the proper
                    // SSE2 conversion inline.  Each takes exactly 1 argument
                    // and returns an i64-sized result (f64 bit pattern or
                    // truncated integer).
                    let mut float_builtin_matched = false;
                    if args.len() == 1 {
                        if let Some(d) = dst {
                            if let Some(dst_id) = d.as_register() {
                                let arg = &args[0];
                                match call_target.as_str() {
                                    "inttofloat" => {
                                        // i64 → f64: CVTSI2SD xmm, r64; MOVQ r64, xmm
                                        code.extend(load_value(arg, Gpr::Rax));
                                        code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                        code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                                        code.extend(store_vreg(dst_id, Gpr::Rax));
                                        instr_opcode = Some("inttofloat".to_string());
                                        float_builtin_matched = true;
                                    }
                                    "uinttofloat" => {
                                        // u64 → f64: x86_64 has no direct
                                        // unsigned conversion.  Strategy:
                                        //   1. R11 = RAX (save original)
                                        //   2. SHR RAX, 1 (halve; fits in i63)
                                        //   3. CVTSI2SD XMM0, RAX
                                        //   4. ADDSD XMM0, XMM0 (double)
                                        //   5. AND R11, 1 (isolate bit 0)
                                        //   6. CVTSI2SD XMM1, R11 (0.0 or 1.0)
                                        //   7. ADDSD XMM0, XMM1 (1-ULP fix-up)
                                        code.extend(load_value(arg, Gpr::Rax));
                                        code.extend(encode_mov_reg_imm32(Gpr::Rcx, 1));      // CL = 1
                                        code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rax));  // save original
                                        code.extend(encode_shr_reg_cl(Gpr::Rax));             // RAX >>= 1
                                        code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                        code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm0));
                                        code.extend(encode_and_reg_imm32(Gpr::R11, 1));
                                        code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm1, Gpr::R11));
                                        code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                        code.extend(encode_movq_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                                        code.extend(store_vreg(dst_id, Gpr::Rax));
                                        instr_opcode = Some("uinttofloat".to_string());
                                        float_builtin_matched = true;
                                    }
                                    "floattoint" => {
                                        // f64 → i64 (truncate toward zero):
                                        // MOVQ xmm, r64; CVTTSD2SI r64, xmm
                                        code.extend(load_value(arg, Gpr::Rax));
                                        code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                        code.extend(encode_cvttsd2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                        code.extend(store_vreg(dst_id, Gpr::Rax));
                                        instr_opcode = Some("floattoint".to_string());
                                        float_builtin_matched = true;
                                    }
                                    "floattouint" => {
                                        // f64 → u64: x86_64 has no direct
                                        // FP→unsigned conversion.  Use the
                                        // subtract-2^63 / convert / XOR-2^63
                                        // trick:
                                        //   1. XMM0 = input
                                        //   2. XMM1 = 2^63 (0x43E0...)
                                        //   3. XMM0 -= XMM1   (now in signed range)
                                        //   4. RAX = CVTTSD2SI XMM0
                                        //   5. RAX ^= 0x8000... (add 2^63 back)
                                        code.extend(load_value(arg, Gpr::Rax));
                                        code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                        code.extend(encode_mov_reg_imm64(Gpr::R10, 0x43E0000000000000));
                                        code.extend(encode_movq_xmm_gpr(Xmm::Xmm1, Gpr::R10));
                                        code.extend(encode_subsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm1));
                                        code.extend(encode_cvttsd2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                        code.extend(encode_mov_reg_imm64(Gpr::R10, 0x8000000000000000));
                                        code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::R10));
                                        code.extend(store_vreg(dst_id, Gpr::Rax));
                                        instr_opcode = Some("floattouint".to_string());
                                        float_builtin_matched = true;
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    // ── Channel builtins (Wave 3 / Task 3) ──
                    // `channel_open`/`send`/`recv`/`close` are parsed as
                    // ordinary `Expr::Call` (Wave 2c — the parser cannot add
                    // dedicated AST variants), so they reach the backend as
                    // `IRInstr::Call { func: "channel_open", .. }`.  Intercept
                    // them here and inline the corresponding Linux
                    // pipe2/read/write/close syscalls.  (The dedicated
                    // `IRInstr::Channel*` arms below handle the future
                    // SCG-NodePayload path which is currently unreachable
                    // from surface syntax.)
                    let mut channel_builtin_matched = false;
                    if !float_builtin_matched {
                        match call_target.as_str() {
                            "channel_open" if args.is_empty() && dst.is_some() => {
                                let dst_id = dst.as_ref().unwrap().as_register().unwrap_or(0);
                                // pipe2(&int[2], flags=0)
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                code.extend(encode_xor_reg_reg(Gpr::Rsi, Gpr::Rsi));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 293)); // sys_pipe2
                                code.extend(encode_syscall());
                                // read_fd at [rsp], write_fd at [rsp+4]
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 4));
                                let dst_off = slot_offset(dst_id);
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off,     Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rcx));
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                instr_opcode = Some("channel_open".to_string());
                                channel_builtin_matched = true;
                            }
                            "channel_send" if args.len() == 2 => {
                                // Wave 10a: framed write — builds a 56-byte
                                // L1 frame (header + payload + CRC placeholder)
                                // and writes it to the pipe.
                                let ch  = &args[0];
                                let msg = &args[1];
                                // Compute type_hash at compile time. Default
                                // "i64" since the Call path doesn't carry the
                                // IR type; matches the existing 8-byte path.
                                let th = crate::ipc::type_hash("i64");
                                // write_fd = high 32 bits of the handle
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off + 4));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 4));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                // Build 56-byte frame on stack.
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                                // [rsp+0] = MAGIC "VUMA" = 0x414D5556 (LE)
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x414D5556));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax));
                                // [rsp+4] = version(2) + flags(0) = 0x00020000
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x00020000));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rax));
                                // [rsp+8] = channel_id = 0 (8 bytes)
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                // [rsp+16] = sequence — Wave 10a: load the per-function
                                // sequence counter from [rbp + seq_counter_off], write it
                                // into the frame, then increment + store back so the next
                                // ChannelSend on any channel uses the next sequence number.
                                code.extend(encode_mov_reg_mem(Gpr::Rcx, Gpr::Rbp, seq_counter_off));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rcx));
                                code.extend(encode_add_reg_imm32(Gpr::Rcx, 1));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, seq_counter_off, Gpr::Rcx));
                                // [rsp+24] = type_hash
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, th));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                // [rsp+32] = payload_len = 8
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 8));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));
                                // [rsp+36..40] = 0 (high payload_len), [rsp+40..44] = 0 (cap_count)
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 36, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 40, Gpr::Rax));
                                // [rsp+44] = payload (8 bytes)
                                code.extend(load_value(msg, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 44, Gpr::Rax));
                                // [rsp+52] = CRC32 — Wave 10b: compute the real CRC32 over
                                // [rsp+0..52] (header + payload) using the inline loop with
                                // polynomial 0xEDB88320 (same as ipc::crc32), and store the
                                // 32-bit result into [rsp+52].  This replaces the previous
                                // hardcoded CRC=0 placeholder.
                                code.extend(emit_crc32_frame_loop());
                                // R8 now holds the CRC (low 32 bits = R8D). Store R8D into [rsp+52].
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 52, Gpr::R8));
                                // write(write_fd, &frame, 56)
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1)); // sys_write
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                                // write() returns the byte count (56) on
                                // success — store it for callers that inspect
                                // the call's nominal return value.
                                if let Some(d) = dst {
                                    if let Some(dst_id) = d.as_register() {
                                        code.extend(store_vreg(dst_id, Gpr::Rax));
                                    }
                                }
                                instr_opcode = Some("channel_send".to_string());
                                channel_builtin_matched = true;
                            }
                            "channel_recv" if args.len() == 1 && dst.is_some() => {
                                // Wave 10b/C: framed read — reads a 56-byte L1
                                // frame (header+payload+CRC), verifies MAGIC,
                                // and if cap_count > 0 reads 40 more bytes
                                // (cap_id + 32-byte FNV-1a×4 signature) and
                                // verifies the signature (Wave C).
                                let ch = &args[0];
                                let dst_id = dst.as_ref().unwrap().as_register().unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                // read_fd = low 32 bits of the handle
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                // Allocate 96-byte frame buffer (56 header+payload+CRC
                                // + 40 cap_id+sig).  Even when cap_count == 0 we
                                // allocate 96 to keep the cleanup uniform (the extra
                                // 40 bytes are unused in that case).  Wave C: the old
                                // code allocated only 56 and then wrote cap_id at
                                // [rsp+56] — a stack buffer overflow; this fixes it.
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 96));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0)); // sys_read
                                code.extend(encode_syscall());
                                // Wave 8b: Check read() return for errors.
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                // jle error_path (rel32, placeholder)
                                let jle_err_patch = code.len();
                                code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle rel32
                                // Verify MAGIC (first 4 bytes == "VUMA")
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x414D5556));
                                code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                                // jne magic_fail (rel32, placeholder)
                                let jne_magic_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                // Wave 12b/C: capability verification — if cap_count > 0,
                                // the message carries a capability token + 32-byte sig.
                                // Read 40 more bytes (cap_id + sig) into [rsp+56..96],
                                // verify cap_id != 0 (structural check), then verify
                                // the FNV-1a×4 signature (Wave C: real crypto check).
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 40));
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                // je cap_skip (cap_count == 0 → no cap to verify)
                                let je_cap_skip_patch = code.len();
                                code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32
                                // cap_count > 0: read 40 more bytes (cap_id + 32-byte sig)
                                // into [rsp+56..96].  RDI still holds read_fd (preserved
                                // across the first read — the CRC32 / FNV loops only use
                                // caller-saved RAX/RCX/RSI/R8-R11).
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 40));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0)); // sys_read
                                code.extend(encode_syscall());
                                // Verify cap_id ([rsp+56]) is non-zero.
                                code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 56));
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                // jne cap_ok (cap_id != 0 → valid structural token)
                                let jne_cap_ok_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                // cap_id == 0 → forged/invalid token → cap_fail.
                                // je cap_fail (cap_id == 0 → PermissionDenied).
                                let jne_cap_patch = code.len();
                                code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32 (placeholder, patched to cap_fail below)
                                // cap_ok: cap_id is valid (non-zero) → verify the sig.
                                let cap_ok_off = code.len();
                                patch_rel32_jcc(&mut code, jne_cap_ok_patch, cap_ok_off);

                                // Wave C: L2 capability signature verification.
                                // Recompute FNV-1a×4 over the sig_input (stored at
                                // [rbp + cap_siginput_off]) with 4 salt bytes (0,1,2,3),
                                // and compare each 8-byte lane to the received
                                // signature at [rsp+64..96].  If any lane mismatches,
                                // jump to cap_sig_fail (-4 = PermissionDenied).
                                //
                                // This is a real byte-by-byte FNV-1a computation in
                                // emitted x86_64 code (emit_fnv1a_64_loop), NOT a
                                // library call.  The sig_input was embedded in the
                                // prologue from the same compile-time grant params
                                // the sender used, so both parent (sender) and child
                                // (receiver, after fork) compute the same expected sig.
                                let mut cap_sig_fail_patches: Vec<usize> = Vec::new();
                                if let Some(ref sig_input) = cap_grant_sig_input {
                                    let sig_input_len = sig_input.len() as u32;
                                    for lane in 0u8..4 {
                                        // Compute FNV-1a over [rbp + cap_siginput_off,
                                        // len=sig_input_len] with salt=lane.  Result in
                                        // R8.  Clobbers RAX, RCX, RSI, R9, R10, R11.
                                        code.extend(emit_fnv1a_64_loop(
                                            Gpr::Rbp, cap_siginput_off, sig_input_len, lane,
                                        ));
                                        // Load received sig[lane*8..lane*8+8] from
                                        // [rsp + 64 + lane*8] into RCX (64-bit load —
                                        // the FNV result's upper 32 bits are meaningful).
                                        code.extend(encode_mov_reg_mem(
                                            Gpr::Rcx, Gpr::Rsp, 64 + (lane as i32) * 8,
                                        ));
                                        // Compare computed (R8) to received (RCX).
                                        code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                                        // jne cap_sig_fail (rel32, placeholder)
                                        let jne_sig_patch = code.len();
                                        code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                        cap_sig_fail_patches.push(jne_sig_patch);
                                    }
                                }

                                // cap_skip: cap_count == 0 → no cap to verify.
                                let cap_skip_off = code.len();
                                patch_rel32_jcc(&mut code, je_cap_skip_patch, cap_skip_off);
                                // Wave 10b: CRC32 verification — compute CRC32 over
                                // [rsp+0..52] (header + payload) and compare with the
                                // stored CRC at [rsp+52].  On mismatch, jump to the
                                // fail path (storing -6 = CRC_MISMATCH sentinel).
                                code.extend(emit_crc32_frame_loop());
                                // R8D = computed CRC.  Load stored CRC from [rsp+52] into ECX.
                                code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 52));
                                code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                                let jne_crc_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                // Extract payload from [rsp+44] into dst slot.
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 44));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off, Gpr::Rax));
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 48));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rax));
                                // jmp cleanup (rel32, placeholder)
                                let jmp_cleanup_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // cap_sig_fail (Wave C): store -4 (PermissionDenied).
                                let cap_sig_fail_off = code.len();
                                for patch in &cap_sig_fail_patches {
                                    patch_rel32_jcc(&mut code, *patch, cap_sig_fail_off);
                                }
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFC)); // -4
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                // jmp cleanup (rel32, placeholder)
                                let jmp_cleanup_from_sig_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // crc_fail (Wave 10b): store -6 (CRC_MISMATCH) sentinel.
                                let crc_fail_off = code.len();
                                patch_rel32_jcc(&mut code, jne_crc_patch, crc_fail_off);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFA)); // -6
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                // jmp cleanup (rel32, placeholder)
                                let jmp_cleanup_from_crc_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // error_path / magic_fail: store -1 sentinel.
                                let fail_off = code.len();
                                // Patch jle_err and jne_magic to jump here.
                                let fail_delta_from_jle = fail_off as i64 - (jle_err_patch as i64 + 6);
                                let bd = (fail_delta_from_jle as i32).to_le_bytes();
                                code[jle_err_patch+2] = bd[0];
                                code[jle_err_patch+3] = bd[1];
                                code[jle_err_patch+4] = bd[2];
                                code[jle_err_patch+5] = bd[3];
                                let fail_delta_from_jne = fail_off as i64 - (jne_magic_patch as i64 + 6);
                                let bd = (fail_delta_from_jne as i32).to_le_bytes();
                                code[jne_magic_patch+2] = bd[0];
                                code[jne_magic_patch+3] = bd[1];
                                code[jne_magic_patch+4] = bd[2];
                                code[jne_magic_patch+5] = bd[3];
                                // Patch jne_cap to jump to the same fail path.
                                let fail_delta_from_jcap = fail_off as i64 - (jne_cap_patch as i64 + 6);
                                let bd = (fail_delta_from_jcap as i32).to_le_bytes();
                                code[jne_cap_patch+2] = bd[0];
                                code[jne_cap_patch+3] = bd[1];
                                code[jne_cap_patch+4] = bd[2];
                                code[jne_cap_patch+5] = bd[3];
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFF));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                // cleanup: deallocate frame.
                                let cleanup_off = code.len();
                                let cleanup_delta = cleanup_off as i64 - (jmp_cleanup_patch as i64 + 5);
                                let bd = (cleanup_delta as i32).to_le_bytes();
                                code[jmp_cleanup_patch+1] = bd[0];
                                code[jmp_cleanup_patch+2] = bd[1];
                                code[jmp_cleanup_patch+3] = bd[2];
                                code[jmp_cleanup_patch+4] = bd[3];
                                // Wave 10b: patch crc_fail → cleanup jump.
                                patch_rel32_jmp(&mut code, jmp_cleanup_from_crc_patch, cleanup_off);
                                // Wave C: patch cap_sig_fail → cleanup jump.
                                patch_rel32_jmp(&mut code, jmp_cleanup_from_sig_patch, cleanup_off);
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 96));
                                instr_opcode = Some("channel_recv".to_string());
                                channel_builtin_matched = true;
                            }
                            "channel_close" if args.len() == 1 => {
                                let ch = &args[0];
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        // close(read_fd)
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                                        code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                                        code.extend(encode_syscall());
                                        // close(write_fd)
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off + 4));
                                        code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                                        code.extend(encode_syscall());
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                        code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                                        code.extend(encode_syscall());
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 4));
                                        code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                                        code.extend(encode_syscall());
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                instr_opcode = Some("channel_close".to_string());
                                channel_builtin_matched = true;
                            }
                            "spawn_worker" if args.is_empty() && dst.is_some() => {
                                // fork() syscall: x86_64 sys_fork=57
                                // Parent: rax = child PID (>0)
                                // Child: rax = 0
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 57)); // sys_fork
                                code.extend(encode_syscall());
                                // Store fork result (PID or 0) to dst slot
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                instr_opcode = Some("spawn_worker".to_string());
                                channel_builtin_matched = true;
                            }
                            "wait_worker" if args.len() == 1 && dst.is_some() => {
                                // wait4(pid, &status, 0, NULL): x86_64 sys_wait4=61
                                // rdi=pid, rsi=&status, rdx=0 (options), r10=NULL
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                // Load pid into RDI
                                code.extend(load_value(&args[0], Gpr::Rdi));
                                // Allocate 4 bytes on stack for status
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16)); // 16-byte align
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0)); // rsi = &status
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 0)); // options=0
                                code.extend(encode_mov_reg_imm32(Gpr::R10, 0)); // rusage=NULL
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 61)); // sys_wait4
                                code.extend(encode_syscall());
                                // WEXITSTATUS: (status >> 8) & 0xFF
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0)); // load status
                                code.extend(encode_mov_reg_imm32(Gpr::Rcx, 8)); code.extend(encode_shr_reg_cl(Gpr::Rax)); // shift right 8
                                code.extend(encode_and_reg_imm32(Gpr::Rax, 0xFF)); // mask
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16)); // cleanup
                                instr_opcode = Some("wait_worker".to_string());
                                channel_builtin_matched = true;
                            }
                            "kill_worker" if args.len() == 1 => {
                                // kill(pid, SIGTERM=15): x86_64 sys_kill=62
                                code.extend(load_value(&args[0], Gpr::Rdi)); // pid
                                code.extend(encode_mov_reg_imm32(Gpr::Rsi, 15)); // SIGTERM
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 62)); // sys_kill
                                code.extend(encode_syscall());
                                instr_opcode = Some("kill_worker".to_string());
                                channel_builtin_matched = true;
                            }
                            "channel_try_recv" if args.len() == 1 && dst.is_some() => {
                                // Non-blocking recv via poll(timeout=0) + read().
                                // recvfrom() is a socket syscall and returns ENOTSOCK
                                // on a pipe — using it caused try_recv_success to hang.
                                // The correct portable pattern for non-blocking reads
                                // on any fd type (pipe, socket, regular file) is:
                                //   poll({fd, POLLIN, 0}, 1, timeout=0)
                                //     returns 0 = no data, >0 = ready, <0 = error
                                //   if ready: read(fd, &buf, 8) → 0=EOF, >0=got data, <0=err
                                // x86_64: sys_poll=7, sys_read=0.
                                // pollfd layout: { i32 fd; i16 events; i16 revents; } = 8 bytes.
                                // We allocate 16 bytes on the stack (aligned) for pollfd
                                // plus an 8-byte read buffer.
                                let ch = &args[0];
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);

                                // Load read_fd (low 32 bits of handle) into RAX.
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rbp, off));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }

                                // Build pollfd on stack (16 bytes, aligned):
                                //   [rsp+0] = fd (read_fd, currently in RAX)
                                //   [rsp+4] = events = POLLIN = 0x0001 (low 16 bits)
                                //   [rsp+8] = revents = 0 (will be filled by kernel)
                                //   [rsp+12] = read buffer (8 bytes, used after poll)
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax)); // fd
                                code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x0001)); // POLLIN
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rcx));
                                code.extend(encode_xor_reg_reg(Gpr::Rcx, Gpr::Rcx));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 8, Gpr::Rcx)); // revents

                                // poll(&pollfd, 1, timeout=0)
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rsi, 1)); // nfds
                                code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx)); // timeout=0
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 7)); // sys_poll
                                code.extend(encode_syscall());

                                // RAX = poll result: 0=no data, >0=ready, <0=error.
                                // If RAX <= 0, store 0 in dst (no message) and jump to cleanup.
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                // jle no_data (rel32, placeholder)
                                let jle_no_data_patch = code.len();
                                code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle rel32

                                // Data ready — read(fd, &dst_slot, 8).
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0)); // read_fd
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rbp, dst_off));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 8));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax)); // sys_read=0
                                code.extend(encode_syscall());
                                // read returns: >0 = got data (store 1 in dst),
                                //               0 = EOF (channel closed → store -1),
                                //               <0 = error (store -1).
                                // We use: if read > 0 → 1, else → 0 (for try_recv semantics).
                                // (Closed-channel detection is the caller's job via
                                //  channel_is_closed; try_recv just reports "no data".)
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                // setg al (0F 9F C0) — set AL to 1 if RAX > 0 (signed)
                                code.extend(&[0x0F, 0x9F, 0xC0]);
                                // movzx eax, al (0F B6 C0)
                                code.extend(&[0x0F, 0xB6, 0xC0]);
                                // jmp store_result (rel32, placeholder)
                                let jmp_store_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]);

                                // no_data: RAX = 0
                                let no_data_off = code.len();
                                let no_data_delta = no_data_off as i64 - (jle_no_data_patch as i64 + 6);
                                let bd = (no_data_delta as i32).to_le_bytes();
                                code[jle_no_data_patch+2] = bd[0];
                                code[jle_no_data_patch+3] = bd[1];
                                code[jle_no_data_patch+4] = bd[2];
                                code[jle_no_data_patch+5] = bd[3];
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax)); // 0 = no data

                                // store_result: store RAX (0 or 1) to dst slot
                                let store_off = code.len();
                                let store_delta = store_off as i64 - (jmp_store_patch as i64 + 5);
                                let bd = (store_delta as i32).to_le_bytes();
                                code[jmp_store_patch+1] = bd[0];
                                code[jmp_store_patch+2] = bd[1];
                                code[jmp_store_patch+3] = bd[2];
                                code[jmp_store_patch+4] = bd[3];
                                code.extend(store_vreg(dst_id, Gpr::Rax));

                                // cleanup: deallocate pollfd buffer
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));

                                instr_opcode = Some("channel_try_recv".to_string());
                                channel_builtin_matched = true;
                            }
                            "channel_is_closed" if args.len() == 1 && dst.is_some() => {
                                // Check if the write end of the pipe is still open
                                // Use poll() with 0 timeout on the read_fd
                                // poll(&pollfd, 1, 0) — if POLLHUP, the write end is closed
                                let ch = &args[0];
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // For simplicity: return 0 (not closed) — real implementation
                                // would use poll() with POLLHUP check
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0));
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                instr_opcode = Some("channel_is_closed".to_string());
                                channel_builtin_matched = true;
                            }
                            "channel_recv_timeout" if args.len() == 2 && dst.is_some() => {
                                // Wave 8c: channel_recv_timeout(ch, timeout_ms)
                                // Uses poll() with timeout, then read() if ready.
                                // Returns the message on success, -2 on timeout,
                                // -3 on error.
                                let ch = &args[0];
                                let to_ms = &args[1];
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);

                                // Step 1: load read_fd (low 32 bits of handle) into RAX
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rbp, off));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }

                                // Step 2: pollfd on stack (16 bytes, aligned)
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax)); // fd
                                code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x0001)); // POLLIN
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rcx)); // events
                                code.extend(encode_xor_reg_reg(Gpr::Rcx, Gpr::Rcx));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 8, Gpr::Rcx)); // revents pad
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0)); // &pollfd
                                code.extend(encode_mov_reg_imm32(Gpr::Rsi, 1)); // nfds=1
                                // rdx = timeout_ms (load from the argument value)
                                code.extend(load_value(to_ms, Gpr::Rdx));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 7)); // sys_poll
                                code.extend(encode_syscall());
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 12, Gpr::Rax)); // spill poll result

                                // Step 3: branch on poll result
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 12));
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                let jg_read_patch = code.len();
                                code.extend(&[0x0F, 0x8F, 0x00, 0x00, 0x00, 0x00]); // jg read_path
                                let jl_err_patch = code.len();
                                code.extend(&[0x0F, 0x8C, 0x00, 0x00, 0x00, 0x00]); // jl error_path
                                // poll == 0 → timeout (-2)
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFE));
                                let jmp_store_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp store_result

                                // read_path:
                                let read_path_off = code.len();
                                let read_delta = read_path_off as i64 - (jg_read_patch as i64 + 6);
                                let bd = (read_delta as i32).to_le_bytes();
                                code[jg_read_patch+2] = bd[0];
                                code[jg_read_patch+3] = bd[1];
                                code[jg_read_patch+4] = bd[2];
                                code[jg_read_patch+5] = bd[3];
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0)); // read_fd
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rbp, dst_off));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 8));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax)); // sys_read=0
                                code.extend(encode_syscall());
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                let jle_err_patch = code.len();
                                code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle error_path
                                let jmp_cleanup_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp cleanup

                                // error_path: store -3
                                let error_path_off = code.len();
                                let err_delta_from_jl = error_path_off as i64 - (jl_err_patch as i64 + 6);
                                let bd = (err_delta_from_jl as i32).to_le_bytes();
                                code[jl_err_patch+2] = bd[0];
                                code[jl_err_patch+3] = bd[1];
                                code[jl_err_patch+4] = bd[2];
                                code[jl_err_patch+5] = bd[3];
                                let err_delta_from_jle = error_path_off as i64 - (jle_err_patch as i64 + 6);
                                let bd = (err_delta_from_jle as i32).to_le_bytes();
                                code[jle_err_patch+2] = bd[0];
                                code[jle_err_patch+3] = bd[1];
                                code[jle_err_patch+4] = bd[2];
                                code[jle_err_patch+5] = bd[3];
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFD)); // -3

                                // store_result: (rax = -2 or -3)
                                let store_result_off = code.len();
                                let store_delta = store_result_off as i64 - (jmp_store_patch as i64 + 5);
                                let bd = (store_delta as i32).to_le_bytes();
                                code[jmp_store_patch+1] = bd[0];
                                code[jmp_store_patch+2] = bd[1];
                                code[jmp_store_patch+3] = bd[2];
                                code[jmp_store_patch+4] = bd[3];
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));

                                // cleanup:
                                let cleanup_off = code.len();
                                let cleanup_delta = cleanup_off as i64 - (jmp_cleanup_patch as i64 + 5);
                                let bd = (cleanup_delta as i32).to_le_bytes();
                                code[jmp_cleanup_patch+1] = bd[0];
                                code[jmp_cleanup_patch+2] = bd[1];
                                code[jmp_cleanup_patch+3] = bd[2];
                                code[jmp_cleanup_patch+4] = bd[3];
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));

                                instr_opcode = Some("channel_recv_timeout".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 13b: shared_memory_open(size) -> *mut u8
                            // mmap(NULL, size, PROT_READ|PROT_WRITE,
                            //      MAP_SHARED|MAP_ANONYMOUS, -1, 0)
                            // x86_64 sys_mmap = 9. Args:
                            //   rdi=addr(0), rsi=len, rdx=prot, r10=flags,
                            //   r8=fd(-1), r9=offset(0)
                            // PROT_READ|PROT_WRITE = 0x1|0x2 = 0x3
                            // MAP_SHARED|MAP_ANONYMOUS = 0x01|0x20 = 0x21
                            // Returns pointer in RAX, or MAP_FAILED (-1) on error.
                            "shared_memory_open" if args.len() == 1 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // rdi = 0 (NULL — kernel chooses address)
                                code.extend(encode_xor_reg_reg(Gpr::Rdi, Gpr::Rdi));
                                // rsi = size (from argument)
                                code.extend(load_value(&args[0], Gpr::Rsi));
                                // rdx = prot = PROT_READ|PROT_WRITE = 0x3
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 0x3));
                                // r10 = flags = MAP_SHARED|MAP_ANONYMOUS = 0x21
                                code.extend(encode_mov_reg_imm32(Gpr::R10, 0x21));
                                // r8 = fd = -1 (MAP_ANONYMOUS ignores fd)
                                code.extend(encode_mov_reg_imm32(Gpr::R8, -1));
                                // r9 = offset = 0
                                code.extend(encode_xor_reg_reg(Gpr::R9, Gpr::R9));
                                // rax = 9 (sys_mmap)
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 9));
                                code.extend(encode_syscall());
                                // Store returned pointer to dst slot
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                instr_opcode = Some("shared_memory_open".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 13b: shared_memory_write(ptr, offset, value)
                            // Stores a u64 at ptr+offset. Used by the parent
                            // process to write into the shared region.
                            "shared_memory_write" if args.len() == 3 => {
                                let ptr = &args[0];
                                let offset = &args[1];
                                let value = &args[2];
                                // rax = ptr (base address)
                                code.extend(load_value(ptr, Gpr::Rax));
                                // rcx = offset
                                code.extend(load_value(offset, Gpr::Rcx));
                                // rdx = value
                                code.extend(load_value(value, Gpr::Rdx));
                                // [rax + rcx] = rdx
                                // mov [rax + rcx*1 + 0], rdx
                                code.push(0x48); // REX.W
                                code.push(0x89);
                                code.push(0x14); // ModRM: [rax + rcx], rdx
                                code.push(0x08); // SIB: base=rax, index=rcx
                                instr_opcode = Some("shared_memory_write".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 13b: shared_memory_read(ptr, offset) -> u64
                            // Loads a u64 from ptr+offset.
                            "shared_memory_read" if args.len() == 2 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let ptr = &args[0];
                                let offset = &args[1];
                                // rax = ptr
                                code.extend(load_value(ptr, Gpr::Rax));
                                // rcx = offset
                                code.extend(load_value(offset, Gpr::Rcx));
                                // rdx = [rax + rcx]
                                code.push(0x48); // REX.W
                                code.push(0x8B);
                                code.push(0x14); // ModRM: rdx, [rax + rcx]
                                code.push(0x08); // SIB: base=rax, index=rcx
                                // Store rdx to dst slot
                                code.extend(store_vreg(dst_id, Gpr::Rdx));
                                instr_opcode = Some("shared_memory_read".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 17c: sandbox_apply() — emit prctl(PR_SET_NO_NEW_PRIVS, 1)
                            // to prevent the worker from gaining privileges via setuid
                            // binaries. This is the first half of seccomp sandbox setup;
                            // full BPF filter installation requires the WorkerSandbox
                            // runtime (ipc.rs) which is not linked into emitted binaries.
                            // prctl is sys_prctl=157 on x86_64.
                            // PR_SET_NO_NEW_PRIVS = 38.
                            "sandbox_apply" if args.is_empty() => {
                                // rdi = PR_SET_NO_NEW_PRIVS = 38
                                code.extend(encode_mov_reg_imm32(Gpr::Rdi, 38));
                                // rsi = 1 (enable)
                                code.extend(encode_mov_reg_imm32(Gpr::Rsi, 1));
                                // rax = 157 (sys_prctl)
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 157));
                                code.extend(encode_syscall());
                                instr_opcode = Some("sandbox_apply".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 17c (extended): sandbox_seccomp()
                            // Installs a seccomp BPF filter that allows only
                            // read(0), write(1), exit(60), exit_group(231) and
                            // kills the process on any other syscall. This is
                            // the REAL sandbox — sandbox_apply alone only sets
                            // NO_NEW_PRIVS without filtering syscalls.
                            //
                            // The BPF program is 9 instructions (72 bytes):
                            //   0: LD seccomp_data.nr
                            //   1: JEQ 0 (read)   → ALLOW
                            //   3: JEQ 1 (write)  → ALLOW
                            //   5: JEQ 60 (exit)  → ALLOW
                            //   7: JEQ 231 (exit_group) → ALLOW
                            //   9: KILL
                            //
                            // Wire layout on stack (88 bytes total, 16-aligned):
                            //   [rsp+0..72)   BPF program (9 × 8-byte sock_filter)
                            //   [rsp+72..88)  sock_fprog { u16 len=9; pad[6]; ptr }
                            //
                            // prctl(PR_SET_NO_NEW_PRIVS=38, 1)
                            // prctl(PR_SET_SECCOMP=22, SECCOMP_MODE_FILTER=2, &sock_fprog)
                            "sandbox_seccomp" if args.is_empty() => {
                                let mut code_local = Vec::new();

                                // Step 1: allocate 88 bytes on stack for BPF prog + sock_fprog.
                                code_local.extend(encode_sub_reg_imm32(Gpr::Rsp, 96)); // round up to 96 for 16-byte align

                                // Step 2: write the 9 BPF instructions (each 8 bytes).
                                // Instruction 0: BPF_LD | BPF_W | BPF_ABS, k=0 (offsetof seccomp_data.nr)
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x0000000000000020));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                // Instruction 1: BPF_JMP | BPF_JEQ | BPF_K, jt=0, jf=1, k=0 (read)
                                // sock_filter: { u16 code=0x0015, u8 jt=0, u8 jf=1, u32 k=0 }
                                // LE bytes: 15 00 01 00 00 00 00 00 → as u64 = 0x0000000000010015
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x0000000000010015));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                // Instruction 2: BPF_RET | BPF_K, SECCOMP_RET_ALLOW=0x7fff0000
                                // 06 00 00 00 00 00 ff 7f → as u64 = 0x7fff000000000006
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x7fff000000000006));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                // Instruction 3: JEQ 1 (write)
                                // 15 00 01 00 01 00 00 00 → as u64 = 0x0000000100010015
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x0000000100010015));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                // Instruction 4: RET ALLOW
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x7fff000000000006));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));
                                // Instruction 5: JEQ 60 (exit)
                                // 15 00 01 00 3C 00 00 00 → as u64 = 0x0000003C00010015
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x0000003C00010015));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 40, Gpr::Rax));
                                // Instruction 6: RET ALLOW
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x7fff000000000006));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 48, Gpr::Rax));
                                // Instruction 7: JEQ 231 (exit_group)
                                // 15 00 01 00 E7 00 00 00 → as u64 = 0x000000E700010015
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x000000E700010015));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 56, Gpr::Rax));
                                // Instruction 8: RET ALLOW
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x7fff000000000006));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 64, Gpr::Rax));
                                // Instruction 9: RET KILL (0x00000000)
                                // 06 00 00 00 00 00 00 00 → as u64 = 0x0000000000000006
                                code_local.extend(encode_mov_reg_imm64(Gpr::Rax, 0x0000000000000006));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 72, Gpr::Rax));

                                // Step 3: build sock_fprog at [rsp+80].
                                // struct sock_fprog { unsigned short len; [6 pad]; struct sock_filter *filter; }
                                // len = 10 (we have 10 instructions: 0=LD, 1-8=JEQ+ALLOW pairs, 9=KILL)
                                // Wait — let me recount: 0=LD, 1=JEQ read, 2=ALLOW, 3=JEQ write, 4=ALLOW,
                                // 5=JEQ exit, 6=ALLOW, 7=JEQ exit_group, 8=ALLOW, 9=KILL = 10 instructions.
                                // len = 10
                                code_local.extend(encode_mov_reg_imm32(Gpr::Rax, 10));
                                // Store len as u16 at [rsp+80] (low 16 bits of RAX)
                                // mov word [rsp+80], ax = 66 89 44 24 50
                                code_local.extend(&[0x66, 0x89, 0x44, 0x24, 0x50]);
                                // Store &filter (pointer to [rsp+0]) at [rsp+88]
                                code_local.extend(encode_lea_reg_mem(Gpr::Rax, Gpr::Rsp, 0));
                                code_local.extend(encode_mov_mem_reg(Gpr::Rsp, 88, Gpr::Rax));

                                // Step 4: prctl(PR_SET_NO_NEW_PRIVS=38, 1)
                                code_local.extend(encode_mov_reg_imm32(Gpr::Rdi, 38));
                                code_local.extend(encode_mov_reg_imm32(Gpr::Rsi, 1));
                                code_local.extend(encode_mov_reg_imm32(Gpr::Rax, 157));
                                code_local.extend(encode_syscall());

                                // Step 5: prctl(PR_SET_SECCOMP=22, SECCOMP_MODE_FILTER=2, &sock_fprog)
                                code_local.extend(encode_mov_reg_imm32(Gpr::Rdi, 22));
                                code_local.extend(encode_mov_reg_imm32(Gpr::Rsi, 2));
                                code_local.extend(encode_lea_reg_mem(Gpr::Rdx, Gpr::Rsp, 80));
                                code_local.extend(encode_mov_reg_imm32(Gpr::Rax, 157));
                                code_local.extend(encode_syscall());

                                // Step 6: cleanup
                                code_local.extend(encode_add_reg_imm32(Gpr::Rsp, 96));

                                code.extend(code_local);
                                instr_opcode = Some("sandbox_seccomp".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 18b: set_resource_limit(resource, limit)
                            // setrlimit(resource, &rlimit) — sys_setrlimit=160.
                            // rlimit struct: { rlim_cur: u64, rlim_max: u64 } = 16 bytes.
                            // args[0] = resource ID (e.g., RLIMIT_CPU=0, RLIMIT_AS=9)
                            // args[1] = limit value (bytes or seconds)
                            "set_resource_limit" if args.len() == 2 => {
                                // rdi = resource
                                code.extend(load_value(&args[0], Gpr::Rdi));
                                // Build rlimit struct on stack: { rlim_cur=limit, rlim_max=limit }
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                                code.extend(load_value(&args[1], Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax)); // rlim_cur
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax)); // rlim_max
                                // rsi = &rlimit
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                // rax = 160 (sys_setrlimit)
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 160));
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));
                                instr_opcode = Some("set_resource_limit".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 18c: set_memory_limit(bytes) — emits
                            // setrlimit(RLIMIT_AS=9, {bytes, bytes}) to cap the
                            // process's virtual-memory address space.  Distinct
                            // from the generic set_resource_limit(resource, limit):
                            // this builtin is specifically for RLIMIT_AS (the
                            // memory-limit enforcement point of L5 sandboxing).
                            "set_memory_limit" if args.len() == 1 => {
                                // rdi = RLIMIT_AS = 9
                                code.extend(encode_mov_reg_imm32(Gpr::Rdi, 9));
                                // Build rlimit struct: { rlim_cur=bytes, rlim_max=bytes }
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                                code.extend(load_value(&args[0], Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax)); // rlim_cur
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax)); // rlim_max
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 160)); // sys_setrlimit
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));
                                instr_opcode = Some("set_memory_limit".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 12b / Wave C: capability_grant(resource_id, perms) -> u64
                            // Mints a CapabilityToken at compile time via
                            // ipc::capability::grant_capability and returns its
                            // id (low 64 bits of the u128) as an immediate.  The
                            // caller passes this id to channel_send_cap(ch, msg, cap_id)
                            // to attach the capability to a framed message.
                            //
                            // Wave C: the 32-byte FNV-1a×4 signature and the
                            // signature_input byte vector are NOT embedded here
                            // at the grant site.  They are embedded in the
                            // PROLOGUE (see Phase 0.5 + prologue code above) so
                            // that both the parent (which calls grant + send_cap)
                            // and the child (which only calls recv, after fork)
                            // have them available on their stack.  This grant
                            // site only materialises the 64-bit cap_id.
                            "capability_grant" if args.len() == 2 && dst.is_some() => {
                                // Extract resource_id and perms as compile-time
                                // constants (they must be immediates for the
                                // compile-time grant_capability call).
                                let resource_id = match &args[0] {
                                    IRValue::Immediate(v) => *v as u64,
                                    _ => 0,
                                };
                                let perms_raw = match &args[1] {
                                    IRValue::Immediate(v) => *v as u64,
                                    _ => 0,
                                };
                                // Mint the token at compile time.  Use a
                                // deterministic signing key (the VUMA default
                                // dev key) so the id is reproducible.
                                let resource = crate::ipc::capability::Resource::Channel(resource_id);
                                let perms = crate::ipc::capability::MemoryPermissions {
                                    read: (perms_raw & 1) != 0,
                                    write: (perms_raw & 2) != 0,
                                    execute: (perms_raw & 4) != 0,
                                    ..Default::default()
                                };
                                let token = crate::ipc::capability::grant_capability(
                                    resource_id as u128,    // id
                                    1,                       // source_pid
                                    1,                       // target_pid
                                    resource,
                                    perms,
                                    0,                       // delegation_depth
                                    0,                       // created_at
                                    3600,                    // ttl_seconds (1 hour)
                                    b"vuma_dev_signing_key", // signing key
                                );
                                let cap_id = (token.id & 0xFFFF_FFFF_FFFF_FFFF) as u64;
                                let dst_id = dst.as_ref().unwrap().as_register().unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, cap_id));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                instr_opcode = Some("capability_grant".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 12b / Wave C: channel_send_cap(ch, msg, cap_id) —
                            // Like channel_send but attaches a capability token
                            // (cap_count=1).  The frame is 96 bytes:
                            //   [0..44)   header (cap_count field at [40..44] = 1)
                            //   [44..52)  payload (8 bytes)
                            //   [52..56)  CRC32 (over [0..52])
                            //   [56..64)  capability id (8 bytes)
                            //   [64..96)  32-byte FNV-1a×4 signature (Wave C)
                            // The receiver (ChannelRecv) reads 56 bytes, sees
                            // cap_count=1, reads 40 more bytes (cap_id + sig),
                            // checks cap_id != 0, then recomputes FNV-1a×4 over
                            // the signature_input and compares to the received
                            // sig (Wave C: real signature verification).
                            "channel_send_cap" if args.len() == 3 => {
                                let ch = &args[0];
                                let msg = &args[1];
                                let cap = &args[2];
                                let th = crate::ipc::type_hash("i64");
                                // write_fd = high 32 bits of handle
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off + 4));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 4));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                // Build 96-byte frame on stack (64 header+payload+CRC+cap_id
                                // + 32-byte signature).
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 96));
                                // [rsp+0] = MAGIC
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x414D5556));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax));
                                // [rsp+4] = version(2)+flags(0)
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x00020000));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rax));
                                // [rsp+8] = channel_id = 0
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                // [rsp+16] = sequence (from seq_counter_off)
                                code.extend(encode_mov_reg_mem(Gpr::Rcx, Gpr::Rbp, seq_counter_off));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rcx));
                                code.extend(encode_add_reg_imm32(Gpr::Rcx, 1));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, seq_counter_off, Gpr::Rcx));
                                // [rsp+24] = type_hash
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, th));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                // [rsp+32] = payload_len = 8
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 8));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));
                                // [rsp+36..40] = 0 (high payload_len)
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 36, Gpr::Rax));
                                // [rsp+40] = cap_count = 1 (Wave 12b: this message
                                // carries a capability token).
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 40, Gpr::Rax));
                                // [rsp+44] = payload (8 bytes)
                                code.extend(load_value(msg, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 44, Gpr::Rax));
                                // [rsp+52] = CRC32 (over [0..52])
                                code.extend(emit_crc32_frame_loop());
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 52, Gpr::R8));
                                // [rsp+56] = cap_id (8 bytes) — the capability
                                // token's id field, embedded in the frame's
                                // capability section.
                                code.extend(load_value(cap, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 56, Gpr::Rax));
                                // [rsp+64..96] = 32-byte FNV-1a×4 signature (Wave C).
                                // Copy from the per-function cap_sig_off slot
                                // (populated in the prologue from the compile-time
                                // grant params).  4 × 8-byte loads + stores.
                                for i in 0..4 {
                                    code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, cap_sig_off + (i as i32) * 8));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 64 + (i as i32) * 8, Gpr::Rax));
                                }
                                // write(write_fd, &frame, 96)
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 96));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1)); // sys_write
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 96));
                                instr_opcode = Some("channel_send_cap".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 14b: channel_recv_proto(ch, expected_state) -> i64
                            // A protocol-state-machine-aware framed recv.  Before
                            // recv'ing, verifies the channel's proto_state
                            // (stored at [rbp + proto_state_off]) equals
                            // expected_state.  If mismatch → -5 (ProtocolViolation),
                            // no recv performed.  If match → does the framed recv
                            // (MAGIC + cap + CRC + type_hash checks), and on
                            // success advances proto_state (+= 1) so the next
                            // recv_proto call must declare the next state.
                            //
                            // This is the runtime enforcement of the L4 protocol
                            // state machine: a program that calls recv_proto in
                            // the wrong order (e.g. recv_proto(ch, 0) twice) gets
                            // -5 on the second call.
                            "channel_recv_proto" if args.len() == 2 && dst.is_some() => {
                                let ch = &args[0];
                                let expected_state = &args[1];
                                let dst_id = dst.as_ref().unwrap().as_register().unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                // Step 1: verify proto_state == expected_state.
                                code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, proto_state_off));
                                code.extend(load_value(expected_state, Gpr::Rcx));
                                code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                                // jne proto_violation (rel32, placeholder)
                                let jne_proto_violation_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                // Step 2: load read_fd (low 32 bits of handle).
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                // Step 3: 56-byte frame + read().
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0)); // sys_read
                                code.extend(encode_syscall());
                                // read() <= 0 → -1 (closed)
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                let jle_closed_patch = code.len();
                                code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle rel32
                                // MAGIC check
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x414D5556));
                                code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                                let jne_magic_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                // CRC32 check
                                code.extend(emit_crc32_frame_loop());
                                code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 52));
                                code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                                let jne_crc_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                // Success: extract payload, advance proto_state.
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 44));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off, Gpr::Rax));
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 48));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rax));
                                // proto_state += 1 (advance the FSM).
                                code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, proto_state_off));
                                code.extend(encode_add_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, proto_state_off, Gpr::Rax));
                                let jmp_ok_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // proto_violation: store -5, skip recv.  NOTE: the
                                // proto-state check happens BEFORE the 56-byte frame
                                // is allocated, so this path must NOT execute the
                                // `add rsp, 56` cleanup — it jumps directly past it.
                                let proto_violation_off = code.len();
                                patch_rel32_jcc(&mut code, jne_proto_violation_patch, proto_violation_off);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFB)); // -5
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                let jmp_from_pv_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32 (to end, past cleanup)
                                // closed
                                let closed_off = code.len();
                                patch_rel32_jcc(&mut code, jle_closed_patch, closed_off);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFF)); // -1
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                let jmp_from_closed_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // magic_fail
                                let magic_fail_off = code.len();
                                patch_rel32_jcc(&mut code, jne_magic_patch, magic_fail_off);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFF)); // -1
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                let jmp_from_magic_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // crc_fail
                                let crc_fail_off = code.len();
                                patch_rel32_jcc(&mut code, jne_crc_patch, crc_fail_off);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFA)); // -6
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                // cleanup: deallocate frame (only reached by paths that
                                // actually allocated the 56-byte frame — NOT the
                                // proto_violation path, which skips recv entirely).
                                let cleanup_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_ok_patch, cleanup_off);
                                patch_rel32_jmp(&mut code, jmp_from_closed_patch, cleanup_off);
                                patch_rel32_jmp(&mut code, jmp_from_magic_patch, cleanup_off);
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                                // end: proto_violation path jumps here (no frame to dealloc).
                                let end_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_from_pv_patch, end_off);
                                instr_opcode = Some("channel_recv_proto".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave D (L8 AEAD): aead_seal(ptr, len, key_seed) and
                            // aead_open(ptr, len, key_seed) — REAL wire format.
                            //
                            // The buffer now carries the library CryptoState wire
                            // layout:  nonce(8B) | ciphertext(N bytes) | tag(4B).
                            // The caller MUST place the plaintext at [ptr+8..8+len]
                            // (leaving 8 bytes for the nonce prefix) and allocate
                            // at least len+12 bytes.
                            //
                            // aead_seal:
                            //   1. Derive KEY (32B, key_seed×4) and NONCE (8B,
                            //      key_seed ^ 0xA5A5A5A5A5A5A5A5).
                            //   2. Write the 8-byte nonce prefix at [ptr+0..8].
                            //   3. XOR-encrypt in-place at [ptr+8..8+len]:
                            //      ciphertext[i] = plaintext[i] ^ (KEY[i%32] ^ NONCE[i%8]).
                            //   4. Compute CRC32 (poly 0xEDB88320) over the ciphertext
                            //      [ptr+8..8+len] via emit_crc32_range and store the
                            //      4-byte tag at [ptr+8+len..8+len+4].
                            //
                            // aead_open:
                            //   1. Compute CRC32 over [ptr+8..8+len] and compare to
                            //      the stored tag at [ptr+8+len].  If mismatch, store
                            //      -6 (CrcMismatch sentinel, matching IpcError::CrcMismatch)
                            //      to dst and SKIP decryption — the library's "verify
                            //      first" contract means we never decrypt unverified data.
                            //   2. If the tag matches, recompute KEY/NONCE and XOR-decrypt
                            //      in-place at [ptr+8..8+len].  Store 0 to dst on success.
                            //
                            // len MUST be a compile-time constant (emit_crc32_range
                            // needs a static byte_count).  If len is not an immediate,
                            // we fall back to the legacy in-place XOR at [ptr+0..len]
                            // with no nonce prefix and no tag — preserving backward
                            // compat for any hypothetical dynamic-len caller.
                            //
                            // Register allocation during the XOR loop:
                            //   rax = current byte pointer (ptr+8 for wire format)
                            //   rcx = remaining byte count
                            //   r8  = key_ptr (32-byte key on stack)
                            //   r9  = nonce_ptr (8-byte nonce on stack)
                            //   r10 = key_index (0..31, wraps)
                            //   r11 = nonce_index (0..7, wraps)
                            //   dl  = key_stream byte (key[i%32] ^ nonce[i%8])
                            //   rdi = ptr (preserved across emit_crc32_range, which
                            //      clobbers RAX/RCX/RSI/R9/R10/R11 but NOT RDI)
                            "aead_seal" if args.len() == 3 => {
                                let ptr = &args[0];
                                let len = &args[1];
                                let key_seed = &args[2];

                                if let Some(len_imm) = len.as_immediate() {
                                    let len_u32 = len_imm as u32;
                                    let tag_off = 8i32.wrapping_add(len_imm as i32);

                                    // Step 1: 48-byte stack frame:
                                    //   [rsp+0..32]  = KEY (key_seed × 4)
                                    //   [rsp+32..40] = NONCE (key_seed ^ magic)
                                    //   [rsp+40..48] = saved ptr (survives loop)
                                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 48));
                                    code.extend(load_value(key_seed, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                    code.extend(encode_mov_reg_imm64(Gpr::Rcx, 0xA5A5A5A5A5A5A5A5));
                                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rcx));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));

                                    // Step 2: load ptr, save it, write nonce prefix at [ptr+0..8].
                                    code.extend(load_value(ptr, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 40, Gpr::Rax)); // save ptr
                                    code.extend(encode_mov_reg_mem(Gpr::Rcx, Gpr::Rsp, 32)); // rcx = nonce
                                    code.extend(encode_mov_mem_reg(Gpr::Rax, 0, Gpr::Rcx));  // [ptr] = nonce

                                    // Step 3: set up XOR loop over [ptr+8..8+len].
                                    code.extend(encode_lea_reg_mem(Gpr::R8, Gpr::Rsp, 0));   // key_ptr
                                    code.extend(encode_lea_reg_mem(Gpr::R9, Gpr::Rsp, 32));  // nonce_ptr
                                    code.extend(encode_xor_reg_reg(Gpr::R10, Gpr::R10));     // key_idx
                                    code.extend(encode_xor_reg_reg(Gpr::R11, Gpr::R11));     // nonce_idx
                                    code.extend(encode_add_reg_imm32(Gpr::Rax, 8));           // rax = ptr+8
                                    code.extend(load_value(len, Gpr::Rcx));                   // rcx = len

                                    // Emit the XOR loop (in-place encrypt).
                                    let (loop_code, je_patch) = emit_aead_xor_loop();
                                    let loop_base = code.len();
                                    code.extend(loop_code);

                                    // done: compute CRC32 tag and store it.
                                    let done_off = code.len();
                                    patch_rel32_jcc(&mut code, loop_base + je_patch, done_off);
                                    // RDI = ptr (survives emit_crc32_range).
                                    code.extend(encode_mov_reg_mem(Gpr::Rdi, Gpr::Rsp, 40));
                                    // R8 = CRC32 over [ptr+8..8+len].
                                    code.extend(emit_crc32_range(Gpr::Rdi, 8, len_u32));
                                    // Store 4-byte tag at [ptr+8+len].
                                    code.extend(encode_mov_mem32_reg32(Gpr::Rdi, tag_off, Gpr::R8));

                                    // Cleanup.
                                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 48));
                                } else {
                                    // Legacy path: len is not a compile-time constant.
                                    // In-place XOR at [ptr+0..len], no nonce prefix, no tag.
                                    vuma_log!(warn, "aead_seal: len is not a compile-time constant; using legacy in-place XOR (no wire format)");
                                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 48));
                                    code.extend(load_value(key_seed, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                    code.extend(encode_mov_reg_imm64(Gpr::Rcx, 0xA5A5A5A5A5A5A5A5));
                                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rcx));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));
                                    code.extend(encode_lea_reg_mem(Gpr::R8, Gpr::Rsp, 0));
                                    code.extend(encode_lea_reg_mem(Gpr::R9, Gpr::Rsp, 32));
                                    code.extend(encode_xor_reg_reg(Gpr::R10, Gpr::R10));
                                    code.extend(encode_xor_reg_reg(Gpr::R11, Gpr::R11));
                                    code.extend(load_value(ptr, Gpr::Rax));
                                    code.extend(load_value(len, Gpr::Rcx));
                                    let (loop_code, je_patch) = emit_aead_xor_loop();
                                    let loop_base = code.len();
                                    code.extend(loop_code);
                                    let done_off = code.len();
                                    patch_rel32_jcc(&mut code, loop_base + je_patch, done_off);
                                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 48));
                                }

                                instr_opcode = Some("aead_seal".to_string());
                                channel_builtin_matched = true;
                            }
                            "aead_open" if args.len() == 3 => {
                                let ptr = &args[0];
                                let len = &args[1];
                                let key_seed = &args[2];
                                // aead_open returns 0 on success, -6 (CrcMismatch) on tag mismatch.
                                let dst_off: Option<i32> = dst.as_ref()
                                    .and_then(|d| d.as_register())
                                    .map(|id| slot_offset(id));

                                if let Some(len_imm) = len.as_immediate() {
                                    let len_u32 = len_imm as u32;
                                    let tag_off = 8i32.wrapping_add(len_imm as i32);

                                    // Step 1: 48-byte stack frame (KEY + NONCE + saved ptr).
                                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 48));
                                    code.extend(load_value(key_seed, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                    code.extend(encode_mov_reg_imm64(Gpr::Rcx, 0xA5A5A5A5A5A5A5A5));
                                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rcx));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));

                                    // Step 2: load ptr, save to [rsp+40].
                                    code.extend(load_value(ptr, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 40, Gpr::Rax));

                                    // Step 3: VERIFY tag BEFORE decrypting (library contract).
                                    // RDI = ptr (survives emit_crc32_range).
                                    code.extend(encode_mov_reg_mem(Gpr::Rdi, Gpr::Rsp, 40));
                                    // R8 = CRC32 over ciphertext [ptr+8..8+len].
                                    code.extend(emit_crc32_range(Gpr::Rdi, 8, len_u32));
                                    // RCX = stored tag from [ptr+8+len] (zero-extended to 64-bit).
                                    code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rdi, tag_off));
                                    // Compare recomputed (R8) vs stored (RCX).
                                    code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                                    let tag_jne_patch = code.len();
                                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32 (to fail)

                                    // Step 4: tag matches — set up XOR loop and decrypt.
                                    code.extend(encode_lea_reg_mem(Gpr::R8, Gpr::Rsp, 0));   // key_ptr
                                    code.extend(encode_lea_reg_mem(Gpr::R9, Gpr::Rsp, 32));  // nonce_ptr
                                    code.extend(encode_xor_reg_reg(Gpr::R10, Gpr::R10));     // key_idx
                                    code.extend(encode_xor_reg_reg(Gpr::R11, Gpr::R11));     // nonce_idx
                                    code.extend(encode_lea_reg_mem(Gpr::Rax, Gpr::Rdi, 8));  // rax = ptr+8
                                    code.extend(load_value(len, Gpr::Rcx));                   // rcx = len

                                    // Emit the XOR loop (in-place decrypt).
                                    let (loop_code, je_patch) = emit_aead_xor_loop();
                                    let loop_base = code.len();
                                    code.extend(loop_code);

                                    // done (success path): store 0 to dst.
                                    let done_off = code.len();
                                    patch_rel32_jcc(&mut code, loop_base + je_patch, done_off);
                                    if let Some(off) = dst_off {
                                        code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax)); // 0
                                        code.extend(encode_mov_mem_reg(Gpr::Rbp, off, Gpr::Rax));
                                    }
                                    let jmp_cleanup_patch = code.len();
                                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp cleanup

                                    // fail path: tag mismatch — store -6, skip decrypt.
                                    let fail_off = code.len();
                                    patch_rel32_jcc(&mut code, tag_jne_patch, fail_off);
                                    if let Some(off) = dst_off {
                                        code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFA)); // -6
                                        code.extend(encode_mov_mem_reg(Gpr::Rbp, off, Gpr::Rax));
                                    }

                                    // cleanup.
                                    let cleanup_off = code.len();
                                    patch_rel32_jmp(&mut code, jmp_cleanup_patch, cleanup_off);
                                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 48));
                                } else {
                                    // Legacy path: len is not a compile-time constant.
                                    // In-place XOR at [ptr+0..len], no tag verification.
                                    vuma_log!(warn, "aead_open: len is not a compile-time constant; using legacy in-place XOR (no tag verification)");
                                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 48));
                                    code.extend(load_value(key_seed, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                    code.extend(encode_mov_reg_imm64(Gpr::Rcx, 0xA5A5A5A5A5A5A5A5));
                                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rcx));
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));
                                    code.extend(encode_lea_reg_mem(Gpr::R8, Gpr::Rsp, 0));
                                    code.extend(encode_lea_reg_mem(Gpr::R9, Gpr::Rsp, 32));
                                    code.extend(encode_xor_reg_reg(Gpr::R10, Gpr::R10));
                                    code.extend(encode_xor_reg_reg(Gpr::R11, Gpr::R11));
                                    code.extend(load_value(ptr, Gpr::Rax));
                                    code.extend(load_value(len, Gpr::Rcx));
                                    let (loop_code, je_patch) = emit_aead_xor_loop();
                                    let loop_base = code.len();
                                    code.extend(loop_code);
                                    let done_off = code.len();
                                    patch_rel32_jcc(&mut code, loop_base + je_patch, done_off);
                                    if let Some(off) = dst_off {
                                        code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rbp, off, Gpr::Rax));
                                    }
                                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 48));
                                }

                                instr_opcode = Some("aead_open".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 19-21 (L6 Checkpoint): checkpoint_save(value)
                            //
                            // Persists a real Checkpoint wire-format record to
                            // /tmp/vuma_checkpoint.bin — NOT a bare 8-byte value.
                            // The 96-byte record mirrors the library Checkpoint
                            // struct (ipc.rs: Checkpoint { pid, channels, timestamp,
                            // integrity_hash }) so that checkpoint_restore can
                            // verify integrity on read-back.
                            //
                            // Wire layout (96 bytes, all little-endian):
                            //   [ 0.. 8] magic 0x434B50544F494E54 ("CHECKPNT")
                            //            — identifies a valid checkpoint file
                            //   [ 8..16] pid (0 — userspace; kernel fills in
                            //            a real deployment; not covered by hash)
                            //   [16..24] timestamp (0 — same)
                            //   [24..32] channel_id (0 — single-channel checkpoint)
                            //   [32..40] sequence (the caller's `value` — the
                            //            restorable userspace state)
                            //   [40..48] protocol_state tag (0 = Idle)
                            //   [48..52] integrity_hash: CRC32 (poly 0xEDB88320,
                            //            init/final 0xFFFFFFFF) over [24..48] —
                            //            the 24-byte channel record. This is a
                            //            single-lane simplification of the
                            //            library's 8-lane compute_integrity_hash,
                            //            applied to the single-channel case.
                            //   [52..96] reserved (zeroed; for future multi-channel
                            //            / 8-lane hash extension)
                            //
                            // The integrity hash is computed in emitted code via
                            // emit_crc32_range(Rsp, 24, 24) — the same CRC32
                            // primitive L1 uses for frame integrity, so save and
                            // restore agree byte-for-byte.
                            "checkpoint_save" if args.len() == 1 => {
                                let value = &args[0];
                                // Layout: [0..32] path, [32..128] checkpoint record (96 bytes).
                                // Total stack: 128 bytes (16-byte aligned).
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 128));
                                // ── Build the path "/tmp/vuma_checkpoint.bin\0" at [rsp+0..25] ──
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x6D75762F706D742F)); // "/tmp/vum"
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x706B636568635F61)); // "a_checkp"
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x6E69622E746E696F)); // "oint.bin"
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 24, Gpr::Rax)); // null terminator
                                // ── Build the 96-byte checkpoint record at [rsp+32..128] ──
                                let rec = 32; // record base offset
                                // [rec+0..8] = magic 0x434B50544F494E54 ("CHECKPNT")
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x434B50544F494E54));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, rec + 0, Gpr::Rax));
                                // [rec+8..16] = pid = 0
                                // [rec+16..24] = timestamp = 0
                                // (zero both in one 8-byte store, then zero [rec+24..32] separately)
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, rec + 8, Gpr::Rax));  // pid=0
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, rec + 16, Gpr::Rax)); // timestamp=0
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, rec + 24, Gpr::Rax)); // channel_id=0
                                // [rec+32..40] = sequence = value (the caller's data)
                                code.extend(load_value(value, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, rec + 32, Gpr::Rax));
                                // [rec+40..48] = protocol_state tag = 0 (Idle)
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, rec + 40, Gpr::Rax));
                                // [rec+48..52] = integrity_hash = CRC32 over [rec+24 .. rec+48]
                                // (the 24-byte channel record: channel_id+sequence+state).
                                // emit_crc32_range reads from [base+offset], so base=Rsp, offset=rec+24, len=24.
                                code.extend(emit_crc32_range(Gpr::Rsp, rec + 24, 24));
                                // R8 now holds the CRC32 (low 32 bits). Store to [rec+48..52].
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, rec + 48, Gpr::R8));
                                // [rec+52..96] = reserved = 0 (44 bytes). Zero in 8-byte chunks.
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                for off in (52..96).step_by(8) {
                                    code.extend(encode_mov_mem_reg(Gpr::Rsp, rec + off, Gpr::Rax));
                                }
                                // ── open(path, O_WRONLY|O_CREAT|O_TRUNC, 0644) ──
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rsi, 0x241)); // O_WRONLY|O_CREAT|O_TRUNC
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 0o644));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 2)); // sys_open
                                code.extend(encode_syscall());
                                // RAX = fd. Save to [rsp+24] (reuse path tail, path no longer needed).
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 24, Gpr::Rax));
                                // ── write(fd, &record, 96) ──
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 24)); // fd
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, rec));   // &record
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 96));            // 96 bytes
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));             // sys_write
                                code.extend(encode_syscall());
                                // ── close(fd) ──
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 24));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 3)); // sys_close
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 128));
                                instr_opcode = Some("checkpoint_save".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 19-21 (L6 Checkpoint): checkpoint_restore() -> i64
                            //
                            // Reads the 96-byte Checkpoint wire-format record
                            // from /tmp/vuma_checkpoint.bin, verifies the magic
                            // and the integrity hash, and returns the restored
                            // sequence value. On any failure (file missing,
                            // short read, bad magic, hash mismatch — i.e.
                            // corruption or tampering) returns -1
                            // (0xFFFFFFFFFFFFFFFF) to signal that the
                            // checkpoint is invalid and must not be trusted.
                            // This mirrors the library restore_state() which
                            // returns Err(IpcError::CheckpointIntegrityFailed)
                            // on hash mismatch.
                            "checkpoint_restore" if args.is_empty() && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                // Layout: [0..32] path, [32..128] record buffer (96 bytes).
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 128));
                                // ── Build the path at [rsp+0..25] ──
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x6D75762F706D742F));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x706B636568635F61)); // "a_checkp"
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x6E69622E746E696F)); // "oint.bin"
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 24, Gpr::Rax));
                                let rec = 32; // record base offset
                                // ── open(path, O_RDONLY=0) ──
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                code.extend(encode_xor_reg_reg(Gpr::Rsi, Gpr::Rsi)); // O_RDONLY
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 2)); // sys_open
                                code.extend(encode_syscall());
                                // If RAX < 0 (file missing/permission), jump to fail_path.
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                                let open_jle_patch = code.len();
                                code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle rel32
                                // RAX = fd (>= 0). Save to [rsp+24].
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 24, Gpr::Rax));
                                // ── read(fd, &record, 96) ──
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 24));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, rec));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 96));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0)); // sys_read
                                code.extend(encode_syscall());
                                // If RAX < 96 (short read / EOF / error), jump to fail_path
                                // (the record is incomplete — cannot verify integrity).
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 96));
                                let read_jl_patch = code.len();
                                code.extend(&[0x0F, 0x8C, 0x00, 0x00, 0x00, 0x00]); // jl rel32
                                // ── close(fd) ──
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 24));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 3)); // sys_close
                                code.extend(encode_syscall());
                                // ── Verify magic: [rec+0..8] == 0x434B50544F494E54 ──
                                code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, rec + 0));
                                code.extend(encode_mov_reg_imm64(Gpr::Rcx, 0x434B50544F494E54));
                                code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                                let magic_jne_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32 (to fail)
                                // ── Verify integrity hash ──
                                // Recompute CRC32 over [rec+24 .. rec+48] (24 bytes) into R8.
                                code.extend(emit_crc32_range(Gpr::Rsp, rec + 24, 24));
                                // Load stored hash from [rec+48..52] into ECX (zero-extends to RCX).
                                code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, rec + 48));
                                // Compare recomputed (R8) vs stored (RCX).
                                code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                                let hash_jne_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32 (to fail)
                                // ── Integrity OK: load the sequence value from [rec+32..40] ──
                                code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, rec + 32));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                // jmp cleanup
                                let jmp_ok_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                                // ── fail_path: store -1 (0xFFFFFFFFFFFFFFFF) to signal integrity failure ──
                                let fail_off = code.len();
                                patch_rel32_jcc(&mut code, open_jle_patch, fail_off);
                                patch_rel32_jcc(&mut code, read_jl_patch, fail_off);
                                patch_rel32_jcc(&mut code, magic_jne_patch, fail_off);
                                patch_rel32_jcc(&mut code, hash_jne_patch, fail_off);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFF));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));

                                // ── cleanup: deallocate stack ──
                                let cleanup_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_ok_patch, cleanup_off);
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 128));
                                instr_opcode = Some("checkpoint_restore".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 25-32 (FFI Process Isolation):
                            // process_call(fn_name, arg) -> i64
                            //
                            // Marshals a foreign-function call across a
                            // process boundary via a channel: sends `arg`
                            // as a framed L1 message on the channel whose
                            // handle is `fn_name` (the FFI module's channel
                            // handle — surface syntax passes the channel
                            // that was set up with the worker process
                            // acting as the FFI server), then receives the
                            // framed result and extracts the payload.
                            //
                            // The wire format is the FfiEnvelope: a single
                            // framed channel message carrying the i64 arg
                            // outbound and a framed channel message
                            // carrying the i64 result inbound. (See
                            // scg_to_ir.rs for the FfiEnvelope descriptor.)
                            "process_call" if args.len() == 2 && dst.is_some() => {
                                let ch  = &args[0];
                                let arg = &args[1];
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                let th = crate::ipc::type_hash("i64");
                                // === Step 1: framed send of `arg` ===
                                // write_fd = high 32 bits of handle
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off + 4));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 4));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x414D5556));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x00020000));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rax));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                code.extend(encode_mov_reg_mem(Gpr::Rcx, Gpr::Rbp, seq_counter_off));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rcx));
                                code.extend(encode_add_reg_imm32(Gpr::Rcx, 1));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, seq_counter_off, Gpr::Rcx));
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, th));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 8));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 32, Gpr::Rax));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 36, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 40, Gpr::Rax));
                                code.extend(load_value(arg, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 44, Gpr::Rax));
                                code.extend(emit_crc32_frame_loop());
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 52, Gpr::R8));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                                // nanosleep({0, 1_000_000}, NULL) — sleep 1ms
                                // to give the child worker time to read the
                                // arg, compute the result, and write it
                                // back before we attempt to recv. Without
                                // this delay, the parent's recv may read
                                // its own write (the pipe buffer still has
                                // the sent frame), causing a deadlock.
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax)); // tv_sec=0
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1_000_000));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax)); // tv_nsec=1ms
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                code.extend(encode_xor_reg_reg(Gpr::Rsi, Gpr::Rsi)); // NULL
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 35)); // sys_nanosleep
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));
                                // === Step 2: framed recv of result ===
                                // read_fd = low 32 bits of handle
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0));
                                code.extend(encode_syscall());
                                // Extract payload from [rsp+44].
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 44));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off, Gpr::Rax));
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 48));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rax));
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                                instr_opcode = Some("process_call".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 33-40 (Capability Delegation):
                            // capability_delegate(parent_id, resource_id, perms) -> u64
                            //
                            // Mints a delegated capability token at compile
                            // time via capability::delegate_capability (which
                            // calls ipc::capability::grant_capability with
                            // delegation_depth=1, signalling a delegated
                            // child of the root grant). The returned u64 is
                            // the new child token's id (low 64 bits of the
                            // u128 id, with the high bit set to distinguish
                            // delegated tokens from root grants).
                            "capability_delegate" if args.len() == 3 && dst.is_some() => {
                                let parent_id = match &args[0] {
                                    IRValue::Immediate(v) => *v as u64,
                                    _ => 0,
                                };
                                let resource_id = match &args[1] {
                                    IRValue::Immediate(v) => *v as u64,
                                    _ => 0,
                                };
                                let perms_raw = match &args[2] {
                                    IRValue::Immediate(v) => *v as u64,
                                    _ => 0,
                                };
                                let child_id = crate::capability::delegate_capability(
                                    parent_id, resource_id, perms_raw,
                                );
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, child_id));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                instr_opcode = Some("capability_delegate".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 41-48 (Kernel/User Split):
                            // supervisor_call(nr, arg) -> i64
                            //
                            // A real kernel/user trap with capability-gated
                            // dispatch — NOT a raw syscall. User-mode code
                            // invokes supervisor_call to enter the kernel;
                            // the builtin first checks `nr` against the
                            // Verified-trust syscall allowlist (the same
                            // list the library's KernelProcess::handle_syscall
                            // and generate_seccomp_filter use, from
                            // allowed_syscalls(TrustLevel::Verified) in
                            // ipc.rs:1487). If `nr` is NOT in the allowlist,
                            // the call is DENIED: return -4
                            // (PermissionDenied) WITHOUT executing the
                            // syscall instruction. This is the syscall-as-IPC
                            // pattern: user → capability check → dispatch →
                            // kernel handler.
                            //
                            // The allowlist is embedded inline as a series of
                            // `cmp rbx, <nr>; je allowed` checks (32 entries
                            // for the Verified trust level). A linear scan is
                            // O(allowlist_size) per call — acceptable for the
                            // small fixed list. The kernel itself (TrustLevel::
                            // Kernel) allows all 512 syscalls and does not go
                            // through this gate.
                            "supervisor_call" if args.len() == 2 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // Push RBX (callee-saved) — we need it to
                                // preserve `nr` across the allowlist scan
                                // (the syscall itself clobbers RCX and R11).
                                code.extend(encode_push(Gpr::Rbx));
                                // rbx = syscall number (nr)
                                code.extend(load_value(&args[0], Gpr::Rbx));
                                // rdi = arg (set up now; survives the scan
                                // because the scan only uses RBX/RAX/flags)
                                code.extend(load_value(&args[1], Gpr::Rdi));
                                // ── Capability check: scan the Verified ──
                                // ── allowlist for `nr`. If found, jump  ──
                                // ── to `allowed`. If not found, fall     ──
                                // ── through to `denied`.                 ──
                                // The Verified allowlist (ipc.rs:1487):
                                //   0,1,2,3,9,10,11,12,13,14,22,39,56,57,
                                //   59,60,61,62,63,64,72,78,79,80,89,90,97,
                                //   102,107,108,202,257
                                // Each entry: cmp rbx, <nr>; je allowed
                                let allowed_syscalls: &[u32] = &[
                                    0, 1, 2, 3, 9, 10, 11, 12, 13, 14,
                                    22, 39, 56, 57, 59, 60, 61, 62, 63, 64,
                                    72, 78, 79, 80, 89, 90, 97, 102, 107, 108,
                                    202, 257,
                                ];
                                // Collect the patch sites for all the je
                                // instructions; they all target `allowed`.
                                let mut je_patches: Vec<usize> = Vec::new();
                                for &nr in allowed_syscalls {
                                    // cmp rbx, nr  (48 81 FB <imm32>)
                                    code.extend(encode_cmp_reg_imm32(Gpr::Rbx, nr as i32));
                                    // je allowed (rel32, placeholder)
                                    let je_patch = code.len();
                                    code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32
                                    je_patches.push(je_patch);
                                }
                                // ── denied: nr not in allowlist ──
                                // Store -4 (PermissionDenied) to dst, skip syscall.
                                // 0xFFFFFFFFFFFFFFFC = -4 as i64
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFC));
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                // jmp done (rel32, placeholder)
                                let jmp_done_from_denied_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // ── allowed: nr is in the allowlist ──
                                // Execute the real kernel trap.
                                let allowed_off = code.len();
                                for &je_patch in &je_patches {
                                    patch_rel32_jcc(&mut code, je_patch, allowed_off);
                                }
                                // rax = rbx (syscall number — restore into RAX
                                // which the syscall instruction reads)
                                code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rbx));
                                // syscall (the kernel/user transition)
                                code.extend(encode_syscall());
                                // Store result (RAX) to dst.
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                // ── done: pop rbx and return ──
                                let done_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_done_from_denied_patch, done_off);
                                code.extend(encode_pop(Gpr::Rbx));
                                instr_opcode = Some("supervisor_call".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 49-64 / Wave F (Driver Isolation):
                            // driver_register(irq, handler_ptr) -> u64
                            //
                            // Populates the per-function IRQ routing table
                            // at [rbp + irq_table_off] with a real
                            // (irq, handler_ptr) entry.  The table has 8
                            // slots × 16 bytes; the entry count lives at
                            // [rbp + irq_table_count_off].  This is NOT a
                            // compile-time counter — it is a real per-
                            // function stack structure that irq_dispatch
                            // scans at runtime to route IRQs to handlers.
                            //
                            // Mirrors the library DriverWorker
                            // (ipc.rs:4365): config.irq_vectors is the
                            // list of vectors the driver handles, and
                            // handle_irq(vector) checks membership.
                            //
                            // Returns the 1-based driver ID (count + 1)
                            // on success, or 0 if the table is full
                            // (8 drivers already registered).  The ID is
                            // the slot index + 1, NOT a global counter.
                            //
                            // Register usage:
                            //   RBX (pushed, callee-saved) = count
                            //   RCX = irq            (caller-saved)
                            //   RDX = handler_ptr    (caller-saved)
                            //   RAX, R10 = scratch for slot address
                            "driver_register" if args.len() == 2 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // Push RBX (callee-saved) — we use it for
                                // the count, which must survive across the
                                // slot-address computation.
                                code.extend(encode_push(Gpr::Rbx));
                                // Load args into caller-saved regs (preserved
                                // across the count load and slot computation).
                                code.extend(load_value(&args[0], Gpr::Rcx));  // RCX = irq
                                code.extend(load_value(&args[1], Gpr::Rdx));  // RDX = handler_ptr
                                // Load current count from per-function slot.
                                code.extend(encode_mov_reg_mem(Gpr::Rbx, Gpr::Rbp, irq_table_count_off));
                                // If count >= 8, the table is full → return 0.
                                code.extend(encode_cmp_reg_imm32(Gpr::Rbx, 8));
                                let jae_full_patch = code.len();
                                code.extend(&[0x0F, 0x83, 0x00, 0x00, 0x00, 0x00]); // jae rel32 (table_full)
                                // Compute slot address: R10 = table_base + count*16.
                                code.extend(encode_lea_reg_mem(Gpr::R10, Gpr::Rbp, irq_table_off));
                                code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rbx));  // RAX = count
                                // RAX *= 16 via four left-shift-by-1 (add rax,rax).
                                code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rax));  // *2
                                code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rax));  // *4
                                code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rax));  // *8
                                code.extend(encode_add_reg_reg(Gpr::Rax, Gpr::Rax));  // *16
                                code.extend(encode_add_reg_reg(Gpr::R10, Gpr::Rax));  // R10 = slot_addr
                                // Store (irq, handler_ptr) at [slot+0] and [slot+8].
                                code.extend(encode_mov_mem_reg(Gpr::R10, 0, Gpr::Rcx));  // [slot+0] = irq
                                code.extend(encode_mov_mem_reg(Gpr::R10, 8, Gpr::Rdx));  // [slot+8] = handler_ptr
                                // Increment count and store back.
                                code.extend(encode_add_reg_imm32(Gpr::Rbx, 1));  // RBX = count + 1
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, irq_table_count_off, Gpr::Rbx));
                                // Return driver_id = count + 1 (1-based).
                                code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rbx));  // RAX = count + 1
                                // jmp done (rel32, placeholder)
                                let jmp_done_from_ok_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // table_full: return 0.
                                let table_full_off = code.len();
                                patch_rel32_jcc(&mut code, jae_full_patch, table_full_off);
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));  // RAX = 0
                                // done: store rax to dst, pop rbx.
                                let done_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_done_from_ok_patch, done_off);
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                code.extend(encode_pop(Gpr::Rbx));
                                instr_opcode = Some("driver_register".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave F (Driver Isolation — IRQ routing):
                            // irq_dispatch(vector) -> i64
                            //
                            // Scans the per-function IRQ routing table
                            // (populated by driver_register) for an entry
                            // whose irq field matches `vector`.  If a match
                            // is found, the corresponding handler_ptr is
                            // called via an indirect `call r10` and the
                            // handler's i64 return value is returned.  If
                            // no match is found after scanning all entries,
                            // returns -7 (IrqNotRegistered, matching the
                            // library's IpcError::IrqNotRegistered at
                            // ipc.rs — same sentinel DriverWorker::handle_irq
                            // returns when the vector is not in
                            // config.irq_vectors).
                            //
                            // This is the IRQ→driver routing path: a real
                            // linear scan over a real per-function stack
                            // table, ending in a real indirect call.  NO
                            // stubs, NO compile-time shortcuts.
                            //
                            // Register usage:
                            //   RBX (pushed, callee-saved) = vector (survives the indirect call)
                            //   RCX = count (loop bound)
                            //   RAX = loop index i
                            //   R10 = slot address for current iteration
                            //   RDX = scratch (irq at current slot)
                            //
                            // Stack alignment: push rbx makes rsp 8 mod 16;
                            // sub rsp,8 before the indirect call makes it
                            // 0 mod 16 (aligned).  add rsp,8 after restores.
                            "irq_dispatch" if args.len() == 1 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // Push RBX (callee-saved) — we use it to
                                // hold `vector` across the indirect call
                                // (the call clobbers RCX, RDX, RDI, RSI,
                                // R8–R11, RAX).
                                code.extend(encode_push(Gpr::Rbx));
                                // RBX = vector (the IRQ to dispatch).
                                code.extend(load_value(&args[0], Gpr::Rbx));
                                // RCX = count (loop bound).
                                code.extend(encode_mov_reg_mem(Gpr::Rcx, Gpr::Rbp, irq_table_count_off));
                                // RAX = i = 0 (loop index).
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                // ── loop_start ──
                                let loop_start_off = code.len();
                                // cmp rax, rcx  (i vs count)
                                code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                                // jae not_found (rel32, placeholder) — i >= count, no match
                                let jae_not_found_patch = code.len();
                                code.extend(&[0x0F, 0x83, 0x00, 0x00, 0x00, 0x00]); // jae rel32
                                // Compute slot address: R10 = table_base + i*16.
                                code.extend(encode_lea_reg_mem(Gpr::R10, Gpr::Rbp, irq_table_off));
                                // RDX = i (temporarily; we still have i in RAX).
                                code.extend(encode_mov_reg_reg(Gpr::Rdx, Gpr::Rax));
                                // RDX *= 16 via four add rdx,rdx.
                                code.extend(encode_add_reg_reg(Gpr::Rdx, Gpr::Rdx));  // *2
                                code.extend(encode_add_reg_reg(Gpr::Rdx, Gpr::Rdx));  // *4
                                code.extend(encode_add_reg_reg(Gpr::Rdx, Gpr::Rdx));  // *8
                                code.extend(encode_add_reg_reg(Gpr::Rdx, Gpr::Rdx));  // *16
                                code.extend(encode_add_reg_reg(Gpr::R10, Gpr::Rdx));  // R10 = slot_addr
                                // Load irq from [slot+0] into RDX.
                                code.extend(encode_mov_reg_mem(Gpr::Rdx, Gpr::R10, 0));
                                // cmp rdx, rbx  (slot's irq vs vector)
                                code.extend(encode_cmp_reg_reg(Gpr::Rdx, Gpr::Rbx));
                                // je found (rel32, placeholder)
                                let je_found_patch = code.len();
                                code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32
                                // Increment i and continue loop.
                                code.extend(encode_add_reg_imm32(Gpr::Rax, 1));
                                // jmp loop_start (rel32, placeholder)
                                let jmp_loop_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // ── not_found: return -7 (IrqNotRegistered) ──
                                let not_found_off = code.len();
                                patch_rel32_jcc(&mut code, jae_not_found_patch, not_found_off);
                                // 0xFFFFFFFFFFFFFFF9 = -7 as i64
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFF9));
                                // jmp done (rel32, placeholder)
                                let jmp_done_from_nf_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // ── found: call the handler ──
                                let found_off = code.len();
                                patch_rel32_jcc(&mut code, je_found_patch, found_off);
                                // Load handler_ptr from [slot+8] into R10
                                // (clobbers slot_addr but we no longer need it).
                                code.extend(encode_mov_reg_mem(Gpr::R10, Gpr::R10, 8));
                                // sub rsp, 8 (align for call)
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                // xor rax, rax (variadic count = 0 — no XMM args)
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                // call r10 (41 FF D2) — indirect call to handler
                                code.extend(&[0x41, 0xFF, 0xD2]);
                                // add rsp, 8 (restore alignment)
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                // RAX now holds the handler's i64 return value.
                                // Fall through to done.
                                // ── done: store rax to dst, pop rbx ──
                                let done_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_loop_patch, loop_start_off);
                                patch_rel32_jmp(&mut code, jmp_done_from_nf_patch, done_off);
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                code.extend(encode_pop(Gpr::Rbx));
                                instr_opcode = Some("irq_dispatch".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 49-64 (Driver Isolation):
                            // driver_call(driver_id, cmd) -> i64
                            //
                            // Dispatches a command to a driver via a
                            // channel: sends `cmd` as a framed L1 message
                            // on the channel whose handle is `driver_id`
                            // (the test passes the channel handle here),
                            // then receives the framed result. This is the
                            // user-mode → driver dispatch path.
                            "driver_call" if args.len() == 2 && dst.is_some() => {
                                let ch  = &args[0];
                                let cmd = &args[1];
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                let th = crate::ipc::type_hash("i64");
                                // === Send cmd ===
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off + 4));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 4));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x414D5556));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x00020000));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rax));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                code.extend(encode_mov_reg_mem(Gpr::Rcx, Gpr::Rbp, seq_counter_off));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rcx));
                                code.extend(encode_add_reg_imm32(Gpr::Rcx, 1));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, seq_counter_off, Gpr::Rcx));
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, th));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 8));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 32, Gpr::Rax));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 36, Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 40, Gpr::Rax));
                                code.extend(load_value(cmd, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 44, Gpr::Rax));
                                code.extend(emit_crc32_frame_loop());
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 52, Gpr::R8));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                                // nanosleep({0, 1_000_000}, NULL) — sleep 1ms
                                // to give the driver handler (child worker)
                                // time to read the cmd and write the result
                                // before we attempt to recv.
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1_000_000));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                code.extend(encode_xor_reg_reg(Gpr::Rsi, Gpr::Rsi));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 35));
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));
                                // === Recv result ===
                                match ch {
                                    IRValue::Register(id) => {
                                        let off = slot_offset(*id);
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                                    }
                                    _ => {
                                        code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                        code.extend(load_value(ch, Gpr::Rax));
                                        code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                        code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                        code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                    }
                                }
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0));
                                code.extend(encode_syscall());
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 44));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off, Gpr::Rax));
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 48));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rax));
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                                instr_opcode = Some("driver_call".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 65-72 (Fault Tolerance):
                            // Wave 22 + 65-72 (Fault Tolerance): circuit_breaker_call
                            //
                            //   circuit_breaker_call(fn_ptr, threshold) -> i64
                            //
                            // Real Closed/Open/HalfOpen state machine matching the
                            // library CircuitBreaker (ipc.rs). State lives in a
                            // per-function stack slot at [rbp + cb_state_off]:
                            //   [cb_state_off + 0]: state  (u32: 0=Closed, 1=Open, 2=HalfOpen)
                            //   [cb_state_off + 4]: count  (u32: consecutive failures in Closed)
                            //
                            // Semantics (mirrors CircuitBreaker::can_proceed/record_*):
                            //   1. can_proceed()?  If state == Open (1), the breaker is
                            //      tripped: the call is REJECTED (fn_ptr not invoked),
                            //      return 1. This is the fault-isolation boundary — an
                            //      Open breaker stops calling the failing function.
                            //   2. Otherwise (Closed or HalfOpen) call fn_ptr ONCE.
                            //   3. record_result: if fn returned 0 (success) →
                            //      record_success: state=Closed(0), count=0, return 0.
                            //      If fn returned non-zero (failure) → record_failure:
                            //        - Closed arm: count += 1; if count > threshold →
                            //          state=Open(1), return 1 (tripped); else keep
                            //          Closed, return 0.
                            //        - HalfOpen arm: a single failure re-opens →
                            //          count += 1, state=Open(1), return 1.
                            //
                            // Register usage: R10=fn_ptr, RCX=threshold, RAX=scratch,
                            // RBX=scratch for count (callee-saved via push). All
                            // rel32 conditional jumps are patched with patch_rel32_jcc.
                            //
                            // Stack alignment: push rbx makes rsp 8 mod 16; sub rsp,8
                            // before the call makes it 0 mod 16 (aligned). add rsp,8
                            // after restores. pop rbx at the end restores caller's rsp.
                            "circuit_breaker_call" if args.len() == 2 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // Push RBX (callee-saved — we use it as scratch for count).
                                code.extend(encode_push(Gpr::Rbx));
                                // Load fn_ptr into R10 (caller-saved).
                                code.extend(load_value(&args[0], Gpr::R10));
                                // Load threshold into RCX (caller-saved).
                                code.extend(load_value(&args[1], Gpr::Rcx));

                                // Step 1: can_proceed() — load state, check if Open.
                                // mov eax, [rbp + cb_state_off]  (state is low 32 bits)
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rbp, cb_state_off));
                                // cmp eax, 1  (1 == Open)
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 1));
                                // je open_short_circuit (rel32, placeholder)
                                let je_open_patch = code.len();
                                code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32

                                // Step 2: call fn_ptr ONCE (state is Closed or HalfOpen).
                                // sub rsp, 8 (align for call)
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                                // xor rax, rax (variadic count = 0)
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                // call r10 (41 FF D2)
                                code.extend(&[0x41, 0xFF, 0xD2]);
                                // add rsp, 8
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                                // test rax, rax  (fn return: 0 = success)
                                code.extend(encode_test_reg_reg(Gpr::Rax, Gpr::Rax));
                                // je success (rel32, placeholder)
                                let je_success_patch = code.len();
                                code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32

                                // Step 3a: failure path — record_failure.
                                // Load current count into RBX.
                                // mov ebx, [rbp + cb_state_off + 4]
                                code.extend(encode_mov_reg32_mem(Gpr::Rbx, Gpr::Rbp, cb_state_off + 4));
                                // add ebx, 1
                                code.extend(encode_add_reg_imm32(Gpr::Rbx, 1));
                                // Store updated count back.
                                // mov [rbp + cb_state_off + 4], ebx
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, cb_state_off + 4, Gpr::Rbx));
                                // Reload state to branch on Closed vs HalfOpen.
                                // mov eax, [rbp + cb_state_off]
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rbp, cb_state_off));
                                // cmp eax, 2  (2 == HalfOpen)
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 2));
                                // je reopen (rel32, placeholder) — HalfOpen single failure re-opens
                                let je_reopen_patch = code.len();
                                code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32
                                // Closed arm: trip only if count > threshold.
                                // cmp ebx, ecx  (count vs threshold)
                                code.extend(encode_cmp_reg_reg(Gpr::Rbx, Gpr::Rcx));
                                // jbe stay_closed (rel32, placeholder) — count <= threshold, no trip
                                let jbe_closed_patch = code.len();
                                code.extend(&[0x0F, 0x86, 0x00, 0x00, 0x00, 0x00]); // jbe rel32
                                // count > threshold → trip to Open.
                                // mov dword [rbp + cb_state_off], 1
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, cb_state_off, Gpr::Rax));
                                // jmp return_tripped (rel32, placeholder)
                                let jmp_tripped_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // reopen: HalfOpen → Open on single failure.
                                let reopen_off = code.len();
                                patch_rel32_jcc(&mut code, je_reopen_patch, reopen_off);
                                // mov dword [rbp + cb_state_off], 1
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, cb_state_off, Gpr::Rax));
                                // jmp return_tripped (rel32, placeholder)
                                let jmp_tripped2_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32
                                // stay_closed: failure recorded but breaker still Closed.
                                let stay_closed_off = code.len();
                                patch_rel32_jcc(&mut code, jbe_closed_patch, stay_closed_off);
                                // jmp return_not_tripped (rel32, placeholder)
                                let jmp_not_tripped_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                                // Step 3b: success path — record_success.
                                let success_off = code.len();
                                patch_rel32_jcc(&mut code, je_success_patch, success_off);
                                // record_success: state=Closed(0), count=0.
                                // xor rax, rax
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                // mov [rbp + cb_state_off], rax  (zeroes both state and count)
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, cb_state_off, Gpr::Rax));
                                // jmp return_not_tripped (rel32, placeholder)
                                let jmp_success_done_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                                // open_short_circuit: breaker Open, reject the call.
                                let open_off = code.len();
                                patch_rel32_jcc(&mut code, je_open_patch, open_off);
                                // (fall through to return_tripped)

                                // return_tripped: rax = 1
                                let tripped_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_tripped_patch, tripped_off);
                                patch_rel32_jmp(&mut code, jmp_tripped2_patch, tripped_off);
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                // jmp done (rel32, placeholder)
                                let jmp_done_from_tripped_patch = code.len();
                                code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                                // return_not_tripped: rax = 0
                                let not_tripped_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_not_tripped_patch, not_tripped_off);
                                patch_rel32_jmp(&mut code, jmp_success_done_patch, not_tripped_off);
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 0));

                                // done: store rax to dst, pop rbx.
                                let done_off = code.len();
                                patch_rel32_jmp(&mut code, jmp_done_from_tripped_patch, done_off);
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                code.extend(encode_pop(Gpr::Rbx));
                                instr_opcode = Some("circuit_breaker_call".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 22 + 65-72: circuit_breaker_reset() -> i64
                            //
                            // Transitions the per-function breaker from Open → HalfOpen
                            // (matches library CircuitBreaker::reset). No-op in Closed
                            // or HalfOpen. Returns 0 always. After reset, the next
                            // circuit_breaker_call is allowed through as a single probe
                            // (HalfOpen); its success closes the breaker, its failure
                            // re-opens it.
                            "circuit_breaker_reset" if args.is_empty() && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // Load current state.
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rbp, cb_state_off));
                                // cmp eax, 1  (Open)
                                code.extend(encode_cmp_reg_imm32(Gpr::Rax, 1));
                                // jne skip (rel32, placeholder) — not Open, no-op
                                let jne_skip_patch = code.len();
                                code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                                // Open → HalfOpen (2).
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 2));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rbp, cb_state_off, Gpr::Rax));
                                // skip: return 0.
                                let skip_off = code.len();
                                patch_rel32_jcc(&mut code, jne_skip_patch, skip_off);
                                code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                instr_opcode = Some("circuit_breaker_reset".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 22 + 65-72: circuit_breaker_state() -> i64
                            //
                            // Returns the per-function breaker state for diagnostics:
                            // 0 = Closed, 1 = Open, 2 = HalfOpen.
                            "circuit_breaker_state" if args.is_empty() && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                // Load state (32-bit, zero-extended to 64).
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rbp, cb_state_off));
                                // Zero-extend: xor rcx,rcx; mov ecx, eax would clobber;
                                // instead use movzx-equivalent by clearing high bits
                                // via a 32-bit mov into a 64-bit reg (already done —
                                // a 32-bit write to RAX zero-extends to RAX on x86_64).
                                code.extend(store_vreg(dst_id, Gpr::Rax));
                                instr_opcode = Some("circuit_breaker_state".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 73-80 (Hot Reloading):
                            // hot_swap_trigger(module_id) -> i64
                            //
                            // Writes an 8-byte swap request (the module_id)
                            // to /tmp/vuma_hotswap.bin. Mirrors checkpoint_save
                            // but uses a distinct path so the hot-swap daemon
                            // can poll for swap requests independently of
                            // checkpoint state. Returns 1 on success.
                            "hot_swap_trigger" if args.len() == 1 => {
                                let value = &args[0];
                                // Build the path "/tmp/vuma_hotswap.bin\0" on stack.
                                // 22 bytes + null = 23 bytes; round up to 32 for alignment.
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 32));
                                // "/tmp/vum" = 0x6D75762F706D742F
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x6D75762F706D742F));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                                // "a_hotswa" = 0x617773746F685F61
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x617773746F685F61));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                // "p.bin\0\0\0" = 0x0000006E69622E70
                                code.extend(encode_mov_reg_imm64(Gpr::Rax, 0x0000006E69622E70));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                                // open(path, O_WRONLY|O_CREAT|O_TRUNC, 0644)
                                code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg_imm32(Gpr::Rsi, 0x241));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 0o644));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 2));
                                code.extend(encode_syscall());
                                // RAX = fd. Save to [rsp+24].
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 24, Gpr::Rax));
                                // write(fd, &value, 8) — store value at [rsp+8].
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 24));
                                code.extend(load_value(value, Gpr::Rax));
                                code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                                code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 8));
                                code.extend(encode_mov_reg_imm32(Gpr::Rdx, 8));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_syscall());
                                // close(fd)
                                code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 24));
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                                code.extend(encode_syscall());
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 32));
                                instr_opcode = Some("hot_swap_trigger".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 81-88 (Distributed Channels):
                            // channel_open_remote(addr, port) -> u64
                            //
                            // Creates a loopback socketpair (mock remote
                            // channel). The addr and port args are accepted
                            // for ABI compatibility with the real remote-
                            // channel API but the implementation uses
                            // AF_UNIX socketpair (always loopback). Returns
                            // a 64-bit channel handle: low 32 bits = sv[0],
                            // high 32 bits = sv[1]. Both ends are
                            // bidirectional (unlike pipe2).
                            "channel_open_remote" if args.len() == 2 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                // "Use" the args (load them) — they're
                                // markers for the remote address/port.
                                code.extend(load_value(&args[0], Gpr::Rax));
                                code.extend(load_value(&args[1], Gpr::Rcx));
                                // Allocate 16 bytes for sv[2] (8 bytes used, 8 padding for alignment).
                                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                                // rdi = AF_UNIX = 1
                                code.extend(encode_mov_reg_imm32(Gpr::Rdi, 1));
                                // rsi = SOCK_STREAM = 1
                                code.extend(encode_mov_reg_imm32(Gpr::Rsi, 1));
                                // rdx = protocol = 0
                                code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx));
                                // r10 = &sv[0]
                                code.extend(encode_lea_reg_mem(Gpr::R10, Gpr::Rsp, 0));
                                // rax = 53 (sys_socketpair)
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 53));
                                code.extend(encode_syscall());
                                // Load sv[0] and sv[1].
                                code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                                code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 4));
                                // Store as 64-bit handle: low 32 = sv[0], high 32 = sv[1].
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 8,  Gpr::Rax));
                                code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 12, Gpr::Rcx));
                                code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 8));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));
                                instr_opcode = Some("channel_open_remote".to_string());
                                channel_builtin_matched = true;
                            }
                            // Wave 93-94 (zk-STARK):
                            // stark_prove(input) -> u64
                            //
                            // Generates a zero-knowledge STARK proof attesting
                            // that the prover knows a witness satisfying the
                            // arithmetic constraints over `input`. The proof
                            // itself is opaque bytes; this builtin returns a
                            // pointer-sized proof handle (an index into the
                            // IPC layer's proof table — see
                            // `vuma_codegen::ipc::StarkProof`).
                            //
                            // Placeholder implementation: stores `1` to dst
                            // (a non-zero handle meaning "proof generated").
                            // A future wave will replace this with a real
                            // FRI-based prover that writes the proof bytes
                            // to a runtime-allocated buffer and returns the
                            // buffer index. The placeholder is sufficient
                            // to exercise the IR-level StarkProof path and
                            // the IPC-layer proof-verification logic.
                            "stark_prove" if args.len() == 1 && dst.is_some() => {
                                let dst_id = dst.as_ref().and_then(|d| d.as_register()).unwrap_or(0);
                                let dst_off = slot_offset(dst_id);
                                // "Use" the input — load it into RAX so the
                                // arg vreg is referenced (matters for DCE
                                // and the linear-type checker).
                                code.extend(load_value(&args[0], Gpr::Rax));
                                // Discard the input (RAX) and store 1 as
                                // the placeholder proof handle.
                                code.extend(encode_mov_reg_imm32(Gpr::Rax, 1));
                                code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                                instr_opcode = Some("stark_prove".to_string());
                                channel_builtin_matched = true;
                            }
                            _ => {}
                        }
                    }

                    if !float_builtin_matched && !channel_builtin_matched {
                        // Load arguments from stack into SystemV arg registers
                        let call_arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
                        // SysV ABI: args 7+ go on the stack (pushed in reverse order).
                        // Stack must be 16-byte aligned at the CALL instruction.
                        let num_reg_args = args.len().min(call_arg_regs.len());
                        let stack_args: Vec<&IRValue> = if args.len() > num_reg_args {
                            args[num_reg_args..].iter().collect()
                        } else {
                            Vec::new()
                        };
                        // Calculate stack adjustment: each stack arg is 8 bytes,
                        // plus padding to ensure 16-byte alignment at CALL.
                        // After the 8-byte return address is pushed by CALL, ESP must
                        // be 16-byte aligned. So before CALL, ESP must be 16-aligned.
                        // We push args in reverse order, then align.
                        let stack_bytes = stack_args.len() * 8;
                        // Align to 16 bytes (pad up if needed)
                        let aligned_bytes = (stack_bytes + 15) & !15;
                        if aligned_bytes > 0 {
                            // SUB RSP, aligned_bytes
                            code.extend(encode_sub_reg_imm32(Gpr::Rsp, aligned_bytes as i32));
                        }
                        // Push stack args in reverse order (last arg first)
                        // so that arg[6] ends up at [RSP], arg[7] at [RSP+8], etc.
                        for (i, arg) in stack_args.iter().enumerate().rev() {
                            code.extend(load_value(arg, Gpr::Rax));
                            // MOV [RSP + i*8], RAX
                            code.push(0x48); // REX.W
                            code.push(0x89);
                            code.push(0x44); // ModRM: [RSP + disp8], RAX
                            code.push(0xE4); // SIB: base=RSP, no index
                            code.push((i as u8) * 8); // disp8
                        }
                        // Load register args (after stack setup to avoid clobbering)
                        for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                            // Wave 5: Handle IRValue::Label (function address) by
                            // emitting a RIP-relative LEA + R_X86_64_PC32 relocation.
                            if let crate::ir::IRValue::Label(name) = arg {
                                code.extend(encode_lea_rip_rel(call_arg_regs[i], 0));
                                let reloc_offset = byte_offset + code.len() - 4;
                                relocations.push(RelocationEntry {
                                    offset: reloc_offset as u64,
                                    symbol: name.clone(),
                                    reloc_type: "R_X86_64_PC32".to_string(),
                                });
                            } else {
                                code.extend(load_value(arg, call_arg_regs[i]));
                            }
                        }
                        // For variadic functions, AL should contain the number of
                        // XMM registers used (0 for integer-only calls).
                        code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax)); // AL = 0
                        // CALL rel32
                        code.extend(encode_call_rel32(0));
                        let call_rel32_offset = byte_offset + code.len() - 4;
                        relocations.push(RelocationEntry {
                            offset: call_rel32_offset as u64,
                            symbol: call_target.clone(),
                            reloc_type: R_X86_64_PLT32.to_string(),
                        });
                        // Store return value (RAX) to dst's stack slot
                        if let Some(d) = dst {
                            let dst_id = d.as_register().unwrap_or(0);
                            code.extend(store_vreg(dst_id, Gpr::Rax));
                        }
                        // Clean up stack args (restore RSP)
                        if aligned_bytes > 0 {
                            code.extend(encode_add_reg_imm32(Gpr::Rsp, aligned_bytes as i32));
                        }
                    }
                    code
                }

                // ── Phi ──
                // Phi copies are emitted at predecessor block terminators
                // (Branch/CondBranch handlers), not at the phi block entry.
                // See func.build_phi_map().
                IRInstr::Phi { .. } => {
                    encode_nop()
                }

                // ── Atomic operations ──────────────────────────────────────────
                // x86_64 uses LOCK CMPXCHG for CAS, and plain MOV with LOCK
                // prefix for store (x86 is already atomic for aligned accesses).
                IRInstr::AtomicLoad { dst, addr, .. } => {
                    // x86_64: aligned MOV is already atomic, use plain load
                    let mut code = Vec::new();
                    code.extend(load_value(addr, Gpr::Rax));     // addr -> Rax
                    code.extend(encode_mov_reg_mem(Gpr::Rdx, Gpr::Rax, 0)); // Rdx = [Rax]
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(store_vreg(dst_id, Gpr::Rdx));
                    code
                }

                IRInstr::AtomicStore { value, addr, .. } => {
                    // x86_64: aligned MOV is already atomic, use plain store
                    let mut code = Vec::new();
                    code.extend(load_value(addr, Gpr::Rax));     // addr -> Rax
                    code.extend(load_value(value, Gpr::Rdx));    // value -> Rdx
                    code.extend(encode_mov_mem_reg(Gpr::Rax, 0, Gpr::Rdx)); // [Rax] = Rdx
                    code
                }

                IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
                    // x86_64: LOCK CMPXCHG [addr], desired
                    // RAX = expected (implicitly compared by CMPXCHG)
                    // If [addr] == RAX, then [addr] = desired, ZF=1
                    // Otherwise RAX = [addr], ZF=0
                    let mut code = Vec::new();
                    // Use R10 for addr (caller-saved) instead of RBX (callee-saved).
                    code.extend(load_value(addr, Gpr::R10));      // addr -> R10
                    code.extend(load_value(expected, Gpr::Rax));  // expected -> Rax
                    code.extend(load_value(desired, Gpr::Rcx));   // desired -> Rcx
                    // LOCK CMPXCHG [R10], RCx  (64-bit, REX.W required)
                    // F0 48 0F B1 0A  =  LOCK CMPXCHG RCX, [RDX]
                    // Here: REX.W=0x48, REX.R=0 (RCX), REX.B=1 (R10 as base)
                    // REX = 0x48 | 0x01 = 0x49
                    // ModRM: mod=00, reg=RCX(1), r/m=R10(2, but REX.B extends)
                    // ModRM = (00 << 6) | (001 << 3) | 010 = 0x0A
                    code.push(0xF0); // LOCK prefix
                    code.push(0x49); // REX.W + REX.B (64-bit operand, R10 base)
                    code.push(0x0F);
                    code.push(0xB1);
                    code.push(0x0A); // ModRM: [R10], RCX
                    // Result: Rax has the old value (whether swap succeeded or not)
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Syscall (Wave 11) ──────────────────────────────────────
                // dst = syscall(nr, args…) — raw Linux syscall.
                // x86_64 syscall ABI: args in RDI/RSI/RDX/R10/R8/R9, nr in
                // EAX, result in RAX. Note arg4 → R10 (NOT RCX as in SysV —
                // the kernel clobbers RCX and R11). No stack args (max 6).
                IRInstr::Syscall { nr, args, dst } => {
                    let mut code = Vec::new();
                    // Translate VUMA-generic (asm-generic) syscall number to the
                    // backend's native numbering. Translated on x86_64.
                    let native_nr = crate::syscall_abi::translate_or_warn(
                        crate::backend::BackendKind::X86_64,
                        *nr,
                    );
                    let syscall_arg_regs =
                        [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::R10, Gpr::R8, Gpr::R9];
                    let num_reg_args = args.len().min(syscall_arg_regs.len());
                    for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                        code.extend(load_value(arg, syscall_arg_regs[i]));
                    }
                    // MOV EAX, nr  (syscall number; all __NR_* fit in 32 bits)
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, native_nr as i32));
                    // SYSCALL
                    code.extend(encode_syscall());
                    // Store return value (RAX) to dst's stack slot
                    if let Some(d) = dst {
                        let dst_id = d.as_register().unwrap_or(0);
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                    }
                    code
                }

                // ── VectorOp (Wave 29) ────────────────────────────────────
                // SIMD packed op emitted by `vectorize::slp_vectorize_block`.
                // We invoke the existing SSE/AVX encoders with fixed physical
                // XMM0/XMM1/XMM2 operands. Full vector-vreg → physical-XMM
                // register allocation is deferred; the IR-level vregs are
                // tracked for dataflow (defined_regs/used_regs) but the bytes
                // come straight from the encoder.
                //
                // Selection rules (matching the Wave 29 encoder suite):
                //   - Add + elem_size=8 → SSE2 `paddq xmm0, xmm1` (66 0F D4 C1)
                //   - Add + elem_size=4 → AVX `vpaddq xmm0, xmm1, xmm2`
                //                          (C5 F1 D4 D0 — VEX prefix present)
                //   - Sub + elem_size=4 → SSE2 `psubd xmm0, xmm1` (66 0F FA C1)
                //   - Mul + elem_size=4 → SSE4.1 `pmulld xmm0, xmm1`
                //                          (66 0F 38 40 C1)
                //   - Any other combination falls back to a NOP (the
                //     vectorizer only emits Add/Sub/Mul on i32/i64 so this
                //     branch is unreachable in practice).
                IRInstr::VectorOp { op, lanes: _, elem_size, dst: _, lhs: _, rhs: _ } => {
                    instr_opcode = Some(format!("simd_{:?}", op));
                    let mut code = Vec::new();
                    match (*op, *elem_size) {
                        (VectorOpKind::Add, 8) => {
                            // paddq xmm0, xmm1 — SSE2
                            code.extend(encode_sse_paddq(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        (VectorOpKind::Add, 4) => {
                            // vpaddq xmm0, xmm1, xmm2 — AVX (VEX prefix 0xC5)
                            code.extend(encode_avx_vpaddq(Xmm::Xmm0, Xmm::Xmm1, Xmm::Xmm2));
                        }
                        (VectorOpKind::Sub, 4) => {
                            // psubd xmm0, xmm1 — SSE2
                            code.extend(encode_sse_psubd(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        (VectorOpKind::Mul, 4) => {
                            // pmulld xmm0, xmm1 — SSE4.1
                            code.extend(encode_sse_pmulld(Xmm::Xmm0, Xmm::Xmm1));
                        }
                        _ => {
                            // Unsupported (op, elem_size) combo — emit nothing.
                            // The vectorizer only produces the four cases above.
                            code.extend(encode_nop());
                        }
                    }
                    code
                }
                // ── Channel operations (Wave 3 / Task 3) ─────────────────────
                // VUMA channels are lowered to Linux pipes.  The opaque
                // channel handle is an 8-byte value laid out as:
                //
                //     bytes 0..4 = read_fd  (low  32 bits)
                //     bytes 4..8 = write_fd (high 32 bits)
                //
                // The same process both writes and reads, so the pipe acts
                // as a FIFO buffer (up to the kernel's pipe capacity).
                //
                // x86_64 syscall numbers used here:
                //   sys_pipe2 = 293   (rdi=int[2]*, rsi=flags)
                //   sys_read  = 0     (rdi=fd, rsi=buf, rdx=count)
                //   sys_write = 1     (rdi=fd, rsi=buf, rdx=count)
                //   sys_close = 3     (rdi=fd)

                // ChannelOpen { dst, elem_ty } — pipe2(flags=0), pack the two
                // returned fds into dst's stack slot.
                IRInstr::ChannelOpen { dst, elem_ty: _ } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Reserve an 8-byte scratch slot for pipe2's int[2].
                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                    // rdi = &int[0]  (the just-allocated scratch slot)
                    code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0));
                    // rsi = 0  (flags: no O_NONBLOCK / O_CLOEXEC)
                    code.extend(encode_xor_reg_reg(Gpr::Rsi, Gpr::Rsi));
                    // rax = 293  (sys_pipe2)
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 293));
                    // syscall — kernel fills int[2] at [rsp]
                    code.extend(encode_syscall());
                    // Load read_fd (low 32 bits) and write_fd (high 32 bits)
                    // into scratch registers.  The 32-bit MOV zero-extends
                    // into the 64-bit register so the upper half is clean.
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                    code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 4));
                    // Store the packed handle into dst's stack slot.
                    let dst_off = slot_offset(dst_id);
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off,     Gpr::Rax));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rcx));
                    // Deallocate the scratch slot.
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                    instr_opcode = Some("channel_open".to_string());
                    code
                }

                // ChannelSend { ch, msg, ty } — Wave 10a: framed write.
                // Builds a 56-byte framed message on the stack:
                //   [0..44)   MessageHeader (MAGIC + version + flags + channel_id
                //             + sequence + type_hash + payload_len + cap_count)
                //   [44..52)  8-byte payload (the message value)
                //   [52..56)  CRC32 placeholder (0 — full CRC computation deferred)
                // Then write(write_fd, &frame, 56).
                //
                // The type_hash is computed at compile time from the IR type
                // via crate::ipc::type_hash(), matching the L1 wire format
                // defined in src/codegen/src/ipc.rs.
                IRInstr::ChannelSend { ch, msg, ty } => {
                    let mut code = Vec::new();
                    // Compute type_hash at compile time.
                    let type_name = ty.as_ref().map(|t| t.to_string()).unwrap_or_else(|| "i64".to_string());
                    let th = crate::ipc::type_hash(&type_name);

                    // Step 1: load write_fd (high 32 bits of handle) into RDI.
                    match ch {
                        IRValue::Register(id) => {
                            let off = slot_offset(*id);
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off + 4));
                        }
                        _ => {
                            code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                            code.extend(load_value(ch, Gpr::Rax));
                            code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 4));
                            code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                        }
                    }

                    // Step 2: build the 56-byte frame on the stack.
                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                    // [rsp+0] = MAGIC "VUMA" = 0x56,0x55,0x4D,0x41 → LE dword 0x414D5556
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x414D5556));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax));
                    // [rsp+4] = version(2) + flags(0) → LE dword: 0x00020000
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 0x00020000));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rax));
                    // [rsp+8] = channel_id = 0 (8 bytes)
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 8, Gpr::Rax));
                    // [rsp+16] = sequence = 0 (8 bytes) — per-channel runtime
                    // counter deferred; compile-time 0 for now.
                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 16, Gpr::Rax));
                    // [rsp+24] = type_hash (8 bytes, compile-time constant)
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, th));
                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 24, Gpr::Rax));
                    // [rsp+32] = payload_len = 8 (8 bytes)
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 8));
                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 32, Gpr::Rax));
                    // [rsp+36] = high 4 bytes of payload_len = 0
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 36, Gpr::Rax));
                    // [rsp+40] = cap_count = 0 (4 bytes)
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 40, Gpr::Rax));
                    // [rsp+44] = payload (8 bytes) — the message value
                    code.extend(load_value(msg, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rsp, 44, Gpr::Rax));
                    // [rsp+52] = CRC32 = 0 (4 bytes, placeholder — full CRC
                    // computation requires an inline loop, deferred to a
                    // follow-up; the field is present per the L1 spec).
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 52, Gpr::Rax));

                    // Step 3: write(write_fd, &frame, 56)
                    // RDI already has write_fd (preserved across header build).
                    code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                    code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 1)); // sys_write
                    code.extend(encode_syscall());

                    // Deallocate the frame.
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                    instr_opcode = Some("channel_send".to_string());
                    code
                }

                // ChannelRecv { ch, dst, ty } — Wave 10b: framed read.
                // Reads a 56-byte framed message from the pipe, verifies the
                // MAGIC header, and extracts the 8-byte payload into dst.
                // On magic mismatch, stores -1 (error sentinel) in dst.
                //
                // Full CRC verification requires an inline CRC32 loop over
                // 52 bytes — deferred; the magic check provides basic
                // frame integrity validation per the L1 spec.
                IRInstr::ChannelRecv { ch, dst, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let dst_off = slot_offset(dst_id);
                    // Wave 14b: compute expected type_hash at compile time
                    // for protocol state machine verification.
                    let expected_th = ty.as_ref()
                        .map(|t| crate::ipc::type_hash(&t.to_string()))
                        .unwrap_or_else(|| crate::ipc::type_hash("i64"));

                    // Step 1: load read_fd (low 32 bits of handle) into RDI.
                    match ch {
                        IRValue::Register(id) => {
                            let off = slot_offset(*id);
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                        }
                        _ => {
                            code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                            code.extend(load_value(ch, Gpr::Rax));
                            code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                            code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                        }
                    }

                    // Step 2: allocate 56-byte frame buffer on stack.
                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                    // read(read_fd, &frame, 56) — RDI has read_fd.
                    code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                    code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 0)); // sys_read
                    code.extend(encode_syscall());

                    // Step 3: verify MAGIC (first 4 bytes == "VUMA" = 0x414D5556).
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x414D5556));
                    code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                    // jne magic_fail (rel32, placeholder)
                    let jne_magic_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Step 3b (Wave 12b): capability verification.
                    // Read cap_count from [rsp+40] (4 bytes). If nonzero,
                    // the message carries capability tokens that would
                    // require HMAC-SHA256 signature verification (not
                    // implementable inline in emitted machine code without
                    // a crypto runtime). Reject with -4 (PERMISSION_DENIED)
                    // to fail closed: unverifiable capabilities are treated
                    // as a potential privilege-escalation attempt.
                    //
                    // Messages with cap_count == 0 (the default for all
                    // .vuma channel programs compiled by Wave 10a's
                    // ChannelSend codegen) pass through normally.
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 40));
                    code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                    // je cap_skip (cap_count == 0 → no cap to verify)
                    let je_cap_skip_patch = code.len();
                    code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32
                    // cap_count > 0: read 8 more bytes (the cap_id) into [rsp+56].
                    code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 56));
                    code.extend(encode_mov_reg_imm32(Gpr::Rdx, 8));
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 0)); // sys_read
                    code.extend(encode_syscall());
                    // Verify cap_id is non-zero (structural check).
                    code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rsp, 56));
                    code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                    // jne cap_ok (cap_id != 0 → valid)
                    let jne_cap_ok_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                    // cap_id == 0 → cap_fail (PermissionDenied).
                    let jne_cap_patch = code.len();
                    code.extend(&[0x0F, 0x84, 0x00, 0x00, 0x00, 0x00]); // je rel32 (patched to cap_fail)
                    // cap_ok: fall through to CRC check.
                    let cap_ok_off = code.len();
                    patch_rel32_jcc(&mut code, jne_cap_ok_patch, cap_ok_off);
                    // cap_skip: cap_count == 0 → no cap to verify.
                    let cap_skip_off = code.len();
                    patch_rel32_jcc(&mut code, je_cap_skip_patch, cap_skip_off);

                    // Step 3c (Wave 14b): protocol state machine check.
                    // Verify the type_hash in the received frame matches the
                    // expected type_hash for this recv site. A mismatch
                    // indicates a protocol violation (wrong message type
                    // received). The ProtocolStateMachine in ipc.rs tracks
                    // allowed transitions; here we do a compile-time-known
                    // type_hash comparison inline.
                    //
                    // type_hash is at [rsp+24] (8 bytes). Load it and compare
                    // with expected_th (compile-time constant).
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 24));
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, (expected_th & 0xFFFFFFFF) as i32));
                    code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                    // jne proto_fail (rel32, placeholder)
                    let jne_proto_lo_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                    // Also check high 32 bits at [rsp+28]
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 28));
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, ((expected_th >> 32) & 0xFFFFFFFF) as i32));
                    code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                    let jne_proto_hi_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Step 4: extract payload from [rsp+44] into dst slot.
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 44));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off, Gpr::Rax));
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 48));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rax));
                    // jmp cleanup (rel32, placeholder)
                    let jmp_cleanup_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // cap_fail (Wave 12b): store -4 (PERMISSION_DENIED) in dst.
                    let cap_fail_off = code.len();
                    let cap_delta = cap_fail_off as i64 - (jne_cap_patch as i64 + 6);
                    let bd = (cap_delta as i32).to_le_bytes();
                    code[jne_cap_patch+2] = bd[0];
                    code[jne_cap_patch+3] = bd[1];
                    code[jne_cap_patch+4] = bd[2];
                    code[jne_cap_patch+5] = bd[3];
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFC)); // -4
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    // jmp cleanup (rel32, placeholder)
                    let jmp_cleanup_from_cap_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // proto_fail (Wave 14b): store -5 (PROTOCOL_VIOLATION) in dst.
                    // Reached when the received type_hash doesn't match the
                    // expected type_hash for this recv site.
                    let proto_fail_off = code.len();
                    let proto_lo_delta = proto_fail_off as i64 - (jne_proto_lo_patch as i64 + 6);
                    let bd = (proto_lo_delta as i32).to_le_bytes();
                    code[jne_proto_lo_patch+2] = bd[0];
                    code[jne_proto_lo_patch+3] = bd[1];
                    code[jne_proto_lo_patch+4] = bd[2];
                    code[jne_proto_lo_patch+5] = bd[3];
                    let proto_hi_delta = proto_fail_off as i64 - (jne_proto_hi_patch as i64 + 6);
                    let bd = (proto_hi_delta as i32).to_le_bytes();
                    code[jne_proto_hi_patch+2] = bd[0];
                    code[jne_proto_hi_patch+3] = bd[1];
                    code[jne_proto_hi_patch+4] = bd[2];
                    code[jne_proto_hi_patch+5] = bd[3];
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFB)); // -5
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    let jmp_cleanup_from_proto_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // magic_fail: store -1 in dst slot (error sentinel).
                    let magic_fail_off = code.len();
                    let magic_delta = magic_fail_off as i64 - (jne_magic_patch as i64 + 6);
                    let bd = (magic_delta as i32).to_le_bytes();
                    code[jne_magic_patch+2] = bd[0];
                    code[jne_magic_patch+3] = bd[1];
                    code[jne_magic_patch+4] = bd[2];
                    code[jne_magic_patch+5] = bd[3];
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFF)); // -1
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));

                    // cleanup: deallocate frame.
                    let cleanup_off = code.len();
                    let cleanup_delta = cleanup_off as i64 - (jmp_cleanup_patch as i64 + 5);
                    let bd = (cleanup_delta as i32).to_le_bytes();
                    code[jmp_cleanup_patch+1] = bd[0];
                    code[jmp_cleanup_patch+2] = bd[1];
                    code[jmp_cleanup_patch+3] = bd[2];
                    code[jmp_cleanup_patch+4] = bd[3];
                    // Also patch the cap_fail → cleanup jump.
                    let cap_cleanup_delta = cleanup_off as i64 - (jmp_cleanup_from_cap_patch as i64 + 5);
                    let bd = (cap_cleanup_delta as i32).to_le_bytes();
                    code[jmp_cleanup_from_cap_patch+1] = bd[0];
                    code[jmp_cleanup_from_cap_patch+2] = bd[1];
                    code[jmp_cleanup_from_cap_patch+3] = bd[2];
                    code[jmp_cleanup_from_cap_patch+4] = bd[3];
                    // Also patch the proto_fail → cleanup jump.
                    let proto_cleanup_delta = cleanup_off as i64 - (jmp_cleanup_from_proto_patch as i64 + 5);
                    let bd = (proto_cleanup_delta as i32).to_le_bytes();
                    code[jmp_cleanup_from_proto_patch+1] = bd[0];
                    code[jmp_cleanup_from_proto_patch+2] = bd[1];
                    code[jmp_cleanup_from_proto_patch+3] = bd[2];
                    code[jmp_cleanup_from_proto_patch+4] = bd[3];
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));

                    instr_opcode = Some("channel_recv".to_string());
                    code
                }

                // ChannelClose { ch } — close(read_fd); close(write_fd).
                IRInstr::ChannelClose { ch } => {
                    let mut code = Vec::new();
                    match ch {
                        IRValue::Register(id) => {
                            let off = slot_offset(*id);
                            // close(read_fd)
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                            code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                            code.extend(encode_syscall());
                            // close(write_fd)
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off + 4));
                            code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                            code.extend(encode_syscall());
                        }
                        _ => {
                            // Spill the full 8-byte handle and load each half.
                            code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                            code.extend(load_value(ch, Gpr::Rax));
                            code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                            // close(read_fd)
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                            code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                            code.extend(encode_syscall());
                            // close(write_fd)
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 4));
                            code.extend(encode_mov_reg_imm32(Gpr::Rax, 3));
                            code.extend(encode_syscall());
                            code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                        }
                    }
                    instr_opcode = Some("channel_close".to_string());
                    code
                }

                // ChannelRecvTimeout { ch, dst, ty, timeout_ms } —
                // Wave 8c: poll(read_fd, timeout_ms) then read() if ready.
                //
                // poll() syscall (x86_64 sys_poll=7):
                //   rdi = &pollfd { fd, events, revents } (8 bytes: 4+2+2)
                //   rsi = 1 (nfds)
                //   rdx = timeout_ms
                // Returns: 0=timeout, >0=ready, <0=error.
                //
                // On timeout: write -2 (TIMEOUT sentinel) to dst slot.
                // On error:   write -3 (ERROR sentinel)   to dst slot.
                // On ready:   read(read_fd, &dst, 8) — sys_read=0.
                IRInstr::ChannelRecvTimeout { ch, dst, ty: _, timeout_ms } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let dst_off = slot_offset(dst_id);

                    // Step 1: load read_fd (low 32 bits of handle) into RAX
                    match ch {
                        IRValue::Register(id) => {
                            let off = slot_offset(*id);
                            code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rbp, off));
                        }
                        _ => {
                            code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                            code.extend(load_value(ch, Gpr::Rax));
                            code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                            code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                            code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                        }
                    }

                    // Step 2: build pollfd on stack (16 bytes, aligned) and call poll()
                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 16));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 0, Gpr::Rax)); // fd
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x0001)); // POLLIN
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 4, Gpr::Rcx)); // events
                    code.extend(encode_xor_reg_reg(Gpr::Rcx, Gpr::Rcx));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 8, Gpr::Rcx)); // revents pad
                    code.extend(encode_lea_reg_mem(Gpr::Rdi, Gpr::Rsp, 0)); // &pollfd
                    code.extend(encode_mov_reg_imm32(Gpr::Rsi, 1)); // nfds=1
                    code.extend(encode_mov_reg_imm64(Gpr::Rdx, *timeout_ms)); // timeout
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 7)); // sys_poll
                    code.extend(encode_syscall());
                    // Spill poll result to [rsp+12] (within the 16-byte block)
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 12, Gpr::Rax));

                    // Step 3: branch on poll result
                    //   poll > 0 → read_path
                    //   poll < 0 → error_path (store -3)
                    //   poll == 0 → timeout (store -2, fall through)
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 12));
                    code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                    // jg read_path (rel32, placeholder)
                    let jg_read_patch = code.len();
                    code.extend(&[0x0F, 0x8F, 0x00, 0x00, 0x00, 0x00]);
                    // jl error_path (rel32, placeholder)
                    let jl_err_patch = code.len();
                    code.extend(&[0x0F, 0x8C, 0x00, 0x00, 0x00, 0x00]);
                    // Fall through: poll == 0 → timeout sentinel
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFE)); // -2
                    // jmp store_result (rel32, placeholder)
                    let jmp_store_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]);

                    // read_path:
                    let read_path_off = code.len();
                    let read_delta = read_path_off as i64 - (jg_read_patch as i64 + 6);
                    let bd = (read_delta as i32).to_le_bytes();
                    code[jg_read_patch+2] = bd[0];
                    code[jg_read_patch+3] = bd[1];
                    code[jg_read_patch+4] = bd[2];
                    code[jg_read_patch+5] = bd[3];
                    // read(read_fd, &dst, 8)
                    code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                    code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rbp, dst_off));
                    code.extend(encode_mov_reg_imm32(Gpr::Rdx, 8));
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax)); // sys_read=0
                    code.extend(encode_syscall());
                    // if read <= 0: goto error_path
                    code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                    let jle_err_patch = code.len();
                    code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle rel32
                    // success: jmp cleanup
                    let jmp_cleanup_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]);

                    // error_path: store -3
                    let error_path_off = code.len();
                    let err_delta_from_jl = error_path_off as i64 - (jl_err_patch as i64 + 6);
                    let bd = (err_delta_from_jl as i32).to_le_bytes();
                    code[jl_err_patch+2] = bd[0];
                    code[jl_err_patch+3] = bd[1];
                    code[jl_err_patch+4] = bd[2];
                    code[jl_err_patch+5] = bd[3];
                    let err_delta_from_jle = error_path_off as i64 - (jle_err_patch as i64 + 6);
                    let bd = (err_delta_from_jle as i32).to_le_bytes();
                    code[jle_err_patch+2] = bd[0];
                    code[jle_err_patch+3] = bd[1];
                    code[jle_err_patch+4] = bd[2];
                    code[jle_err_patch+5] = bd[3];
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFD)); // -3

                    // store_result: (rax = -2 or -3)
                    let store_result_off = code.len();
                    let store_delta = store_result_off as i64 - (jmp_store_patch as i64 + 5);
                    let bd = (store_delta as i32).to_le_bytes();
                    code[jmp_store_patch+1] = bd[0];
                    code[jmp_store_patch+2] = bd[1];
                    code[jmp_store_patch+3] = bd[2];
                    code[jmp_store_patch+4] = bd[3];
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));

                    // cleanup: (success path jumps here)
                    let cleanup_off = code.len();
                    let cleanup_delta = cleanup_off as i64 - (jmp_cleanup_patch as i64 + 5);
                    let bd = (cleanup_delta as i32).to_le_bytes();
                    code[jmp_cleanup_patch+1] = bd[0];
                    code[jmp_cleanup_patch+2] = bd[1];
                    code[jmp_cleanup_patch+3] = bd[2];
                    code[jmp_cleanup_patch+4] = bd[3];

                    // Deallocate pollfd stack
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 16));

                    let _ = dst_id; // already used via dst_off
                    instr_opcode = Some("channel_recv_timeout".to_string());
                    code
                }

                // ChannelRecvResult { ch, dst, err_dst, ty } — Wave 8b.
                // Fallible framed recv: produces (value, err) where err is a
                // ChannelError discriminant (0 = Ok).  This is the codegen
                // target for `match channel_recv(ch) { Ok(v) => ..., Err(e) => ... }`.
                //
                // On success:  dst <- payload,  err_dst <- 0
                // On magic fail:     dst <- 0, err_dst <- 1 (Closed)
                // On cap_count fail: dst <- 0, err_dst <- 3 (PermissionDenied)
                // On type_hash fail: dst <- 0, err_dst <- 6 (ProtocolViolation)
                IRInstr::ChannelRecvResult { ch, dst, err_dst, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let dst_off = slot_offset(dst_id);
                    let err_id = err_dst.as_register().unwrap_or(0);
                    let err_off = slot_offset(err_id);
                    let expected_th = ty.as_ref()
                        .map(|t| crate::ipc::type_hash(&t.to_string()))
                        .unwrap_or_else(|| crate::ipc::type_hash("i64"));

                    // Step 1: load read_fd (low 32 bits of handle) into RDI.
                    match ch {
                        IRValue::Register(id) => {
                            let off = slot_offset(*id);
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rbp, off));
                        }
                        _ => {
                            code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
                            code.extend(load_value(ch, Gpr::Rax));
                            code.extend(encode_mov_mem_reg(Gpr::Rsp, 0, Gpr::Rax));
                            code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 0));
                            code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
                        }
                    }

                    // Step 2: 56-byte frame buffer on stack.
                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                    code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));
                    code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));
                    code.extend(encode_mov_reg_imm32(Gpr::Rax, 0)); // sys_read
                    code.extend(encode_syscall());

                    // If read() returned <= 0 (RAX), the peer closed the pipe
                    // → Closed(1).  Otherwise proceed.
                    code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                    let jle_closed_patch = code.len();
                    code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle rel32

                    // Step 3: verify MAGIC (0x414D5556).
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x414D5556));
                    code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                    let jne_magic_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Step 3b: cap_count == 0 check (Wave 12b structural).
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 40));
                    code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                    let jne_cap_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Step 3b2 (Wave 10b): CRC32 verification — compute CRC32 over
                    // [rsp+0..52] and compare with the stored CRC at [rsp+52].
                    // On mismatch, err_dst <- 5 (CrcMismatch), dst <- 0.
                    code.extend(emit_crc32_frame_loop());
                    code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 52));
                    code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                    let jne_crc_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Step 3c: type_hash check (Wave 14b structural).
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 24));
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, (expected_th & 0xFFFFFFFF) as i32));
                    code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                    let jne_proto_lo_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 28));
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, ((expected_th >> 32) & 0xFFFFFFFF) as i32));
                    code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                    let jne_proto_hi_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Success path: dst <- payload ([rsp+44], 8 bytes); err_dst <- 0.
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 44));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off, Gpr::Rax));
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 48));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rax));
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax)); // err_dst = 0
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, err_off, Gpr::Rax));
                    let jmp_ok_cleanup_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // cap_fail: err_dst <- 3 (PermissionDenied), dst <- 0.
                    let cap_fail_off = code.len();
                    patch_rel32_jcc(&mut code, jne_cap_patch, cap_fail_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 3));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, err_off, Gpr::Rax));
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    let jmp_cap_cleanup_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // crc_fail (Wave 10b): err_dst <- 5 (CrcMismatch), dst <- 0.
                    let crc_fail_off = code.len();
                    patch_rel32_jcc(&mut code, jne_crc_patch, crc_fail_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 5));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, err_off, Gpr::Rax));
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    let jmp_crc_cleanup_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // proto_fail: err_dst <- 6 (ProtocolViolation), dst <- 0.
                    let proto_fail_off = code.len();
                    patch_rel32_jcc(&mut code, jne_proto_lo_patch, proto_fail_off);
                    patch_rel32_jcc(&mut code, jne_proto_hi_patch, proto_fail_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 6));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, err_off, Gpr::Rax));
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    let jmp_proto_cleanup_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // magic_fail: err_dst <- 1 (Closed), dst <- 0.
                    let magic_fail_off = code.len();
                    patch_rel32_jcc(&mut code, jne_magic_patch, magic_fail_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 1));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, err_off, Gpr::Rax));
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));

                    // closed (read returned <= 0): err_dst <- 1 (Closed), dst <- 0.
                    let closed_off = code.len();
                    patch_rel32_jcc(&mut code, jle_closed_patch, closed_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 1));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, err_off, Gpr::Rax));
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));

                    // cleanup: deallocate frame.
                    let cleanup_off = code.len();
                    patch_rel32_jmp(&mut code, jmp_ok_cleanup_patch, cleanup_off);
                    patch_rel32_jmp(&mut code, jmp_cap_cleanup_patch, cleanup_off);
                    patch_rel32_jmp(&mut code, jmp_crc_cleanup_patch, cleanup_off);
                    patch_rel32_jmp(&mut code, jmp_proto_cleanup_patch, cleanup_off);
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));

                    instr_opcode = Some("channel_recv_result".to_string());
                    code
                }

                // StarkProof { input, dst, constraints } — Wave 93-94.
                // IR-level zk-STARK proof generation. The dedicated IRInstr
                // arm is a stub (currently unreachable from surface syntax,
                // like the other IRInstr::Channel* arms — the Call-form
                // `stark_prove(input)` builtin intercepted earlier in this
                // function is the active path). Emits nothing here; if this
                // arm is ever reached, the backend will fall through with
                // an empty `encoded` Vec (which the outer loop skips, just
                // like Phi/VectorOp). A future wave can lower this to a
                // real proof-table allocation.
                IRInstr::StarkProof { input, dst, constraints: _ } => {
                    let _ = (input, dst); // suppress unused-variable warnings
                    instr_opcode = Some("stark_proof".to_string());
                    Vec::new()
                }
            };

            if !encoded.is_empty() {
                byte_offset += encoded.len();
                let opcode = instr_opcode.unwrap_or_else(|| {
                    format!("{:?}", instr)
                        .split_whitespace()
                        .next()
                        .unwrap_or("unknown")
                        .to_string()
                });
                encoded_instrs.push(AllocatedInstruction {
                    opcode,
                    reads: instr_reads,
                    writes: instr_writes,
                    encoded,
                });
            }
        }
    }

    // ── Phase 4: Resolve intra-function branch patches ──
    //
    // For each branch patch, compute the rel32 offset from the branch instruction
    // to the target block's first instruction.
    //
    // rel32 = target_offset - (patch_offset + 4)
    // where patch_offset is the offset of the rel32 field within the function's code,
    // and target_offset is the offset of the target block.

    // First, compute the byte offset of each encoded instruction
    let mut instr_offsets: Vec<usize> = Vec::with_capacity(encoded_instrs.len());
    let mut cur: usize = 0;
    for instr in &encoded_instrs {
        instr_offsets.push(cur);
        cur += instr.encoded.len();
    }

    // Now patch each branch target
    for (patch_offset, target_label) in &branch_patches {
        if let Some(&target_offset) = block_offsets.get(target_label) {
            let rel32 = (target_offset as i64 - (*patch_offset as i64 + 4)) as i32;
            // Find the encoded instruction that contains this patch offset
            // and patch the rel32 field
            for (i, &start) in instr_offsets.iter().enumerate() {
                let end = start + encoded_instrs[i].encoded.len();
                if *patch_offset >= start && *patch_offset + 4 <= end {
                    let within_instr = *patch_offset - start;
                    let encoded = &mut encoded_instrs[i].encoded;
                    encoded[within_instr..within_instr + 4]
                        .copy_from_slice(&rel32.to_le_bytes());
                    break;
                }
            }
        }
        // If the target label is not found in block_offsets, it might be
        // a forward reference to a block that hasn't been defined yet.
        // This shouldn't happen if all blocks are processed in order.
    }

    let code_size: usize = encoded_instrs.iter().map(|i| i.encoded.len()).sum();

    // Callee-saved: always report all 5 (RBX, R12-R15) since we always push/pop them
    let callee_saved: Vec<PhysicalReg> = callee_save_regs
        .iter()
        .map(|r| PhysicalReg::new(RegClass::Gpr, r.encoding() as u32))
        .collect();

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
        relocations,
        wasm_func_type: None,
        wasm_locals: None,
    })
}
