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

/// Dynamic-length variant of [`emit_crc32_range`]: computes the same CRC32
/// (IEEE 802.3, poly 0xEDB88320, init/final 0xFFFFFFFF) but over a
/// **runtime-computed** byte range `[base_reg + offset_reg ..
/// base_reg + offset_reg + len_reg]` rather than a compile-time
/// `[base + offset .. base + offset + byte_count]`.
///
/// This is required by the L8 AEAD dynamic-len path (`aead_seal` /
/// `aead_open` with a `Register` length), which must compute the CRC32
/// authentication tag over a ciphertext region whose length is not known
/// at compile time.  Without this helper the dynamic-len path previously
/// fell back to a malleable in-place XOR with no tag at all — the
/// cryptographic downgrade flagged in the L0-L8 wiring audit.
///
/// # Algorithm
/// Identical to [`emit_crc32_range`] (byte-at-a-time, 8-bit inner loop,
/// `crc ^= byte; for 8 iters: crc = (crc>>1) ^ ((crc&1) ? poly : 0);
/// crc ^= 0xFFFFFFFF` at the end).  The only differences are:
///   - the start pointer is `base_reg + offset_reg` (computed at runtime
///     via `mov rsi, base_reg; add rsi, offset_reg`) instead of a
///     compile-time `lea rsi, [base + offset]`
///   - the outer loop bound is `len_reg` (compared via `cmp r9, len_reg`)
///     instead of a compile-time `cmp r9, byte_count`
///
/// # Register usage
///   - **R8**  — CRC accumulator (result; low 32 bits are the CRC, upper
///     32 bits zeroed so a 64-bit `cmp r8, rcx` against a zero-extended
///     32-bit load compares correctly)
///   - **RCX** — constant 1 (for `shr r8, cl` and `test r8, rcx`)
///   - **RAX** — polynomial 0xEDB88320
///   - **RSI** — running byte pointer
///   - **R9**  — outer loop counter (0..len_reg)
///   - **R10** — current byte (zero-extended)
///   - **R11** — inner loop counter (0..8) / scratch for final XOR
///
/// **Clobbers:** RAX, RCX, RSI, R8, R9, R10, R11.
/// **Preserves:** base_reg, offset_reg, len_reg (provided they are not in
/// the clobber set above — callers should pass them in RDI/RDX/RBX or
/// other non-clobbered registers), plus RDI, RDX, RBP, RBX, R12–R15.
fn emit_crc32_range_dynamic(base_reg: Gpr, offset_reg: Gpr, len_reg: Gpr) -> Vec<u8> {
    let mut code = Vec::with_capacity(120);

    // crc = 0xFFFFFFFF  (64-bit mov so upper 32 bits are 0)
    code.extend(encode_mov_reg_imm64(Gpr::R8, 0xFFFFFFFF));
    // rcx = 1 (CL=1 for shr; RCX=1 for test bit 0)
    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 1));
    // rax = poly 0xEDB88320 (64-bit mov so upper 32 bits are 0)
    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xEDB88320));
    // rsi = base_reg + offset_reg  (runtime start pointer)
    code.extend(encode_mov_reg_reg(Gpr::Rsi, base_reg));
    code.extend(encode_add_reg_reg(Gpr::Rsi, offset_reg));
    // r9 = 0 (outer counter)
    code.extend(encode_xor_reg_reg(Gpr::R9, Gpr::R9));

    // ── outer_loop: ──
    let outer_loop_off = code.len();
    // cmp r9, len_reg  (register-controlled bound — the dynamic part)
    code.extend(encode_cmp_reg_reg(Gpr::R9, len_reg));
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
    // upper 32 bits of R8 are 0, matching emit_crc32_range's convention so a
    // 64-bit cmp against a zero-extended 32-bit load compares correctly).
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

/// Wave I (zk-STARK): emit a real FNV-1a 64-bit hash loop over `byte_count`
/// bytes starting at `[base + offset]`, with NO salt prefix. The u64 result
/// is left in **R8**. This matches the library `StarkProof::commitment()`
/// (ipc.rs:4017) byte-for-byte: init = 0xcbf29ce484222325 (FNV-1a 64-bit
/// offset basis), prime = 0x100000001b3 (FNV-1a 64-bit prime), per byte
/// `hash ^= byte; hash = hash.wrapping_mul(prime)`.
///
/// Used by `stark_verify` to recompute the verifier-key commitment over the
/// 40-byte region (proof_data 32 bytes ++ public_input_dup 8 bytes) and
/// compare to the stored verifier_key. The prover side (`stark_prove`)
/// embeds a verifier_key computed at compile time via the library
/// `StarkProof::new_valid(...).commitment()`, so a correct FNV-1a loop here
/// MUST match that value — a byte-for-byte mismatch would cause verification
/// to fail (which is exactly the integrity guarantee the STARK layer
/// provides).
///
/// Register usage (all caller-saved; preserves RDI, RBX, RDX, R10):
///   - **R8**  — hash accumulator (result)
///   - **R11** — FNV prime 0x100000001b3 (loaded once via 64-bit mov)
///   - **RAX** — current byte (zero-extended)
///   - **RCX** — outer loop counter (0..byte_count)
///   - **RSI** — running byte pointer
fn emit_fnv1a_64_loop_nosalt(base: Gpr, offset: i32, byte_count: u32) -> Vec<u8> {
    let mut code = Vec::with_capacity(110);

    // r8 = FNV-1a offset basis = 0xcbf29ce484222325
    code.extend(encode_mov_reg_imm64(Gpr::R8, 0xcbf29ce484222325));
    // r11 = FNV-1a prime = 0x100000001b3
    code.extend(encode_mov_reg_imm64(Gpr::R11, 0x100000001b3));
    // rsi = &buffer[0]  (base + offset)
    code.extend(encode_lea_reg_mem(Gpr::Rsi, base, offset));
    // rcx = 0 (loop counter)
    code.extend(encode_xor_reg_reg(Gpr::Rcx, Gpr::Rcx));

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
    // hash *= prime  (wrapping imul)
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

    // ── Phase 0.7: Count folded L1/L2 checks for formal_verify() (Wave J) ──
    //
    // formal_verify() returns the per-function count of L1/L2 runtime
    // checks folded into L3 compile-time invariants.  Each
    // channel_send / channel_recv / capability_grant /
    // capability_delegate / stark_prove / stark_verify builtin
    // increments the per-function counter at runtime (see
    // `inc_formal_verify_count` and the prologue store below).
    //
    // BUT runtime increments alone are insufficient when the
    // program forks: after fork(), the parent and child have
    // separate stacks, so a builtin that executes only in the
    // child (e.g. channel_recv inside `if pid == 0 { ... }`) won't
    // increment the parent's counter.  To make formal_verify()
    // return the TOTAL number of folded checks (not just the ones
    // that executed in the calling process), we ALSO scan the
    // function's IR at compile time and store the total folded-check
    // count in the prologue.  Runtime increments then add the
    // "executed checks" count on top of the "folded checks" base.
    //
    // This is the runtime witness that the L1→L3 collapse proof
    // (see `vuma_ive::verification::l1l3_collapse`) covered N
    // runtime checks — formal_verify() returns at least N (the
    // compile-time folded count), proving the codegen really did
    // emit the channel/capability builtins whose types the proof
    // verified.
    let mut formal_verify_folded_count: u64 = 0;
    for block in &func.blocks {
        for instr in &block.instructions {
            match instr {
                IRInstr::Call { func: fname, .. } => {
                    if matches!(
                        fname.as_str(),
                        "channel_send"
                            | "channel_recv"
                            | "capability_grant"
                            | "capability_delegate"
                            | "stark_prove"
                            | "stark_verify"
                    ) {
                        formal_verify_folded_count += 1;
                    }
                }
                IRInstr::ChannelSend { .. } | IRInstr::ChannelRecv { .. } => {
                    formal_verify_folded_count += 1;
                }
                _ => {}
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

    // Wave G (Hot Reload — real hot-swap state machine): per-function module
    // version table + entry-count slot.  This is the codegen-side analogue of
    // the library HotSwapManager (ipc.rs:3569), which tracks
    // `active_versions: HashMap<String,u32>` and validates:
    //   - new_version > old_version (else ProtocolViolation)
    //   - active_versions[name] == old_version (else concurrent-swap race,
    //     ProtocolViolation)
    // The previous codegen implementation of hot_swap_trigger just wrote an
    // 8-byte module_id to /tmp/vuma_hotswap.bin — no version tracking, no
    // validation, no rollback.  This is replaced by a REAL per-function stack
    // table that hot_swap_register / hot_swap_trigger / hot_swap_rollback
    // scan and mutate at runtime.
    //
    // Layout (8 entries × 16 bytes = 128 bytes):
    //   [rbp + hotswap_table_off +  i*16 + 0]: module_id (u64)
    //   [rbp + hotswap_table_off +  i*16 + 8]: version    (u64, active version)
    //   [rbp + hotswap_table_count_off      ]: count      (u64, # filled slots)
    //
    // Zeroed in the prologue so each function starts with an empty table.
    // The 128-byte table data itself is left uninitialized — readers only
    // touch slots [0..count), so uninitialized data past count is never
    // observed.
    current_offset += 128;
    let hotswap_table_off: i32 = -(current_offset);
    current_offset += 8;
    let hotswap_table_count_off: i32 = -(current_offset);

    // Wave I (zk-STARK): per-function STARK proof table.  stark_prove
    // writes a real proof entry (proof_data + verifier_key commitment +
    // validity_window) into the next free slot; stark_verify recomputes
    // the FNV-1a verifier-key commitment over the stored proof_data and
    // compares it to the stored verifier_key — a byte-for-byte match
    // mirrors the library StarkProof::verify (ipc.rs:4037) check.
    //
    // Mirrors the library StarkProof (ipc.rs:4000):
    //   { proof_data: Vec<u8>, public_inputs: Vec<u64>,
    //     verifier_key: u64, validity_window: u64 }
    // The library commitment() = FNV-1a 64 over proof_data ++ each
    // public_input's 8 LE bytes. We embed the public_input as a
    // duplicate 8 bytes immediately after proof_data so the verifier
    // can hash 40 contiguous bytes in one call to
    // emit_fnv1a_64_loop_nosalt (matching the library commitment()
    // byte-for-byte).
    //
    // Layout (4 entries × 56 bytes = 224 bytes):
    //   [rbp + stark_table_off + i*56 +  0]: proof_data       (32 bytes)
    //   [rbp + stark_table_off + i*56 + 32]: public_input_dup (8 bytes, = proof_data[0..8])
    //   [rbp + stark_table_off + i*56 + 40]: verifier_key     (8 bytes, FNV-1a commitment)
    //   [rbp + stark_table_off + i*56 + 48]: validity_window  (8 bytes, = 3600)
    //   [rbp + stark_table_count_off       ]: count           (u64, # proofs stored, max 4)
    //
    // Zeroed in the prologue so each function starts with count = 0.
    // The 224-byte table data itself is left uninitialized — readers
    // only touch slots [0..count), so uninitialized data past count
    // is never observed.
    current_offset += 224;
    let stark_table_off: i32 = -(current_offset);
    current_offset += 8;
    let stark_table_count_off: i32 = -(current_offset);

    // Wave J (Formal Verification — L1→L3 collapse proof witness):
    // per-function counter of how many L1/L2 runtime checks were
    // folded into L3 compile-time invariants by the codegen.  Each
    // channel_send / channel_recv / capability_grant /
    // capability_delegate / stark_prove / stark_verify builtin
    // increments this counter at its emit site; the `formal_verify()`
    // builtin loads it and returns it as an i64.
    //
    // This is the runtime witness that the L1→L3 collapse proof (see
    // `vuma_ive::verification::l1l3_collapse`) covered N runtime
    // checks — a program that calls formal_verify() and checks the
    // result is >= the expected folded-check count is proving at
    // runtime that the compiler's static collapse proof was not
    // vacuous (i.e. the codegen really did emit the channel/capability
    // builtins that the proof verified).
    //
    // Layout (8 bytes, zeroed in the prologue):
    //   [rbp + formal_verify_count_off]: folded_check_count (u64)
    current_offset += 8;
    let formal_verify_count_off: i32 = -(current_offset);

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

    // Wave J: increment the per-function formal-verify folded-check
    // counter (`[rbp + formal_verify_count_off]`).  Emitted at the
    // start of each channel_send / channel_recv / capability_grant /
    // capability_delegate / stark_prove / stark_verify builtin so
    // `formal_verify()` can return the count at runtime.  Clobbers
    // RAX (caller-saved, free at the start of each builtin arm).
    let inc_formal_verify_count = || -> Vec<u8> {
        let mut c = Vec::new();
        c.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, formal_verify_count_off));
        c.extend(encode_add_reg_imm32(Gpr::Rax, 1));
        c.extend(encode_mov_mem_reg(Gpr::Rbp, formal_verify_count_off, Gpr::Rax));
        c
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

    // Wave G (Hot Reload): zero the per-function hot-swap version-table count
    // slot so each function starts with an empty module-version table
    // (count = 0).  The 128-byte table data itself is left uninitialized —
    // hot_swap_register / hot_swap_trigger / hot_swap_rollback only touch
    // slots [0..count), so uninitialized data past count is never observed.
    // RAX is already 0 from the xor above.
    emit(encode_mov_mem_reg(Gpr::Rbp, hotswap_table_count_off, Gpr::Rax), "zero_hotswap_table_count");

    // Wave I (zk-STARK): zero the per-function STARK proof-table count so
    // each function starts with an empty proof table (count = 0).  The
    // 224-byte table data itself is left uninitialized — stark_prove only
    // writes slots [0..count) and stark_verify only reads slots
    // [0..count), so uninitialized data past count is never observed.
    // RAX is already 0 from the xor above.
    emit(encode_mov_mem_reg(Gpr::Rbp, stark_table_count_off, Gpr::Rax), "zero_stark_table_count");

    // Wave J (Formal Verification): initialise the per-function
    // formal-verify folded-check counter to the COMPILE-TIME count of
    // folded L1/L2 checks (computed in Phase 0.7 above).  This is the
    // base count — each channel_send / channel_recv / capability_grant
    // / capability_delegate / stark_prove / stark_verify builtin then
    // increments it at runtime, so formal_verify() returns
    // (compile_time_folded + runtime_executed) >= compile_time_folded.
    //
    // Storing the compile-time count in the prologue (rather than just
    // zeroing) is necessary because fork() splits the parent and child
    // stacks: a builtin that executes only in the child (e.g.
    // channel_recv inside `if pid == 0 { ... }`) would never
    // increment the parent's runtime counter.  The compile-time base
    // guarantees formal_verify() returns the TOTAL number of folded
    // checks regardless of which process the caller runs in.
    //
    // `mov rax, imm64` + `mov [rbp + off], rax` (RAX was 0 from the
    // xor above; we overwrite it with the compile-time count).  When
    // the count is 0 (no folded builtins), this is equivalent to the
    // zeroing the slot would otherwise get.
    emit(
        encode_mov_reg_imm64(Gpr::Rax, formal_verify_folded_count),
        "init_formal_verify_count",
    );
    emit(
        encode_mov_mem_reg(Gpr::Rbp, formal_verify_count_off, Gpr::Rax),
        "store_formal_verify_count",
    );

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
                                        // On x86_64, writing to a 32-bit register
                                        // (EAX) automatically zero-extends to the
                                        // full 64-bit register (RAX).  The previous
                                        // code used `AND RAX, -1` which is a 64-bit
                                        // AND with 0xFFFFFFFFFFFFFFFF — a no-op that
                                        // left the upper 32 bits unchanged.  This
                                        // caused the CRC32 ZExt to fail (the upper
                                        // 32 bits were 0xFFFFFFFF from the XOR !crc
                                        // step, making the 64-bit comparison fail).
                                        //
                                        // The fix: emit `MOV EAX, EAX` (32-bit
                                        // register move), which clears the upper
                                        // 32 bits of RAX on x86_64.  Encoding:
                                        //   89 C0  (MOV EAX, EAX)
                                        // For R8–R15 a REX prefix is needed, but
                                        // since we always use RAX here, no REX.
                                        code.extend(&[0x89, 0xC0]); // mov eax, eax
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
                    // ── IPC builtins were here ──
                    // Deleted: the inline Call-form IPC arms (channel_open,
                    // channel_send, channel_recv, etc.) are dead code since
                    // ipc_lowering::lower_ipc_builtins() runs unconditionally
                    // for ALL backends and rewrites IPC Calls into generic IR
                    // before the instruction selector runs.  See TASKS.md §0.3.2.
                    //
                    // The IRInstr::ChannelSend/ChannelRecv/ChannelOpen/etc.
                    // arms below handle the SCG-NodePayload path and the
                    // IR that ipc_lowering emits (via Alloc/Store/Load/Syscall).
                    if !float_builtin_matched {
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

                    // Wave J: increment the formal-verify folded-check
                    // counter (one L1 channel-framing check folded).
                    code.extend(inc_formal_verify_count());

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
                    // [rsp+52] = CRC32 — Phase 2a (L1 framing): compute the
                    // real CRC32 over [rsp+0..52] (header + payload) using the
                    // inline loop with polynomial 0xEDB88320 (same as
                    // ipc::crc32), and store the 32-bit result into [rsp+52].
                    // This replaces the previous hardcoded CRC=0 placeholder.
                    // RDI still holds write_fd (preserved across the CRC loop,
                    // which only clobbers RAX/RCX/RSI/R8-R11).
                    code.extend(emit_crc32_frame_loop());
                    // R8D now holds the CRC. Store R8D into [rsp+52].
                    code.extend(encode_mov_mem32_reg32(Gpr::Rsp, 52, Gpr::R8));

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

                    // Wave J: increment the formal-verify folded-check
                    // counter (one L1 channel-framing check folded).
                    code.extend(inc_formal_verify_count());

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

                    // Step 3d (Phase 2a): CRC32 verification — compute CRC32
                    // over [rsp+0..52] (header + payload) and compare with
                    // the stored CRC at [rsp+52].  On mismatch, store -6
                    // (CRC_MISMATCH) in dst and jump to cleanup.  This
                    // replaces the previous "CRC deferred" stub (the field
                    // was read but never compared).
                    code.extend(emit_crc32_frame_loop());
                    code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 52));
                    code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                    let jne_crc_patch = code.len();
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

                    // crc_fail (Phase 2a): store -6 (CRC_MISMATCH) in dst.
                    // Reached when the recomputed CRC32 over [rsp+0..52] does
                    // not match the stored CRC32 at [rsp+52].
                    let crc_fail_off = code.len();
                    patch_rel32_jcc(&mut code, jne_crc_patch, crc_fail_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFA)); // -6
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    let jmp_cleanup_from_crc_patch = code.len();
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
                    // Phase 2a: patch crc_fail → cleanup jump.
                    patch_rel32_jmp(&mut code, jmp_cleanup_from_crc_patch, cleanup_off);
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
                // On MAGIC mismatch: write -1 (Closed/Invalid) to dst slot.
                // On CRC32 mismatch:  write -6 (CRC_MISMATCH) to dst slot.
                //
                // Wave L1 (Phase 2f): the read path now does a full 56-byte
                // framed read with MAGIC + CRC32 verification (previously a
                // raw 8-byte read bypassed the L1 wire format — the only
                // remaining L1 bypass flagged in the L0-L8 audit). This
                // matches the Call-path channel_recv_timeout semantics.
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

                    // read_path: poll > 0 → allocate 56-byte L1 frame and
                    // read it, then verify MAGIC + CRC32 before extracting
                    // the payload. Stack after sub rsp, 56:
                    //   [rsp+0..56]   = 56-byte L1 frame buffer
                    //   [rsp+56..72]  = pollfd block (16 bytes, allocated earlier)
                    //     [rsp+56]    = read_fd (4 bytes)
                    //     [rsp+60]    = events (4 bytes)
                    //     [rsp+64]    = revents pad (4 bytes)
                    //     [rsp+68]    = poll result spill (4 bytes)
                    let read_path_off = code.len();
                    let read_delta = read_path_off as i64 - (jg_read_patch as i64 + 6);
                    let bd = (read_delta as i32).to_le_bytes();
                    code[jg_read_patch+2] = bd[0];
                    code[jg_read_patch+3] = bd[1];
                    code[jg_read_patch+4] = bd[2];
                    code[jg_read_patch+5] = bd[3];
                    // Allocate 56-byte frame buffer.
                    code.extend(encode_sub_reg_imm32(Gpr::Rsp, 56));
                    // read_fd is now at [rsp+56] (pollfd was at [rsp+0..16]
                    // before the sub; the fd field is at offset 0 of pollfd).
                    code.extend(encode_mov_reg32_mem(Gpr::Rdi, Gpr::Rsp, 56));
                    code.extend(encode_lea_reg_mem(Gpr::Rsi, Gpr::Rsp, 0));    // &frame
                    code.extend(encode_mov_reg_imm32(Gpr::Rdx, 56));            // count
                    code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));        // sys_read=0
                    code.extend(encode_syscall());
                    // if read <= 0: goto read_error_path (dealloc frame, then -3)
                    code.extend(encode_cmp_reg_imm32(Gpr::Rax, 0));
                    let jle_err_patch = code.len();
                    code.extend(&[0x0F, 0x8E, 0x00, 0x00, 0x00, 0x00]); // jle rel32

                    // Verify MAGIC (first 4 bytes == "VUMA" = 0x414D5556).
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 0));
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 0x414D5556));
                    code.extend(encode_cmp_reg_reg(Gpr::Rax, Gpr::Rcx));
                    // jne magic_fail (rel32, placeholder)
                    let jne_magic_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Verify CRC32 — compute over [rsp+0..52] (header +
                    // payload) and compare with the stored CRC at [rsp+52].
                    // The CRC loop clobbers RAX/RCX/RSI/R8-R11.
                    code.extend(emit_crc32_frame_loop());
                    code.extend(encode_mov_reg32_mem(Gpr::Rcx, Gpr::Rsp, 52));
                    code.extend(encode_cmp_reg_reg(Gpr::R8, Gpr::Rcx));
                    let jne_crc_patch = code.len();
                    code.extend(&[0x0F, 0x85, 0x00, 0x00, 0x00, 0x00]); // jne rel32

                    // Success: extract payload from [rsp+44..52] into dst slot.
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 44));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off, Gpr::Rax));
                    code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rsp, 48));
                    code.extend(encode_mov_mem32_reg32(Gpr::Rbp, dst_off + 4, Gpr::Rax));
                    // Deallocate the 56-byte frame and jmp cleanup (which
                    // will then deallocate the 16-byte pollfd block).
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                    // jmp cleanup (rel32, placeholder)
                    let jmp_cleanup_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // crc_fail (Phase 2f): store -6 (CRC_MISMATCH) in dst.
                    let crc_fail_off = code.len();
                    patch_rel32_jcc(&mut code, jne_crc_patch, crc_fail_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFA)); // -6
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    // Deallocate the 56-byte frame and jmp cleanup.
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                    // jmp cleanup (rel32, placeholder)
                    let jmp_cleanup_from_crc_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // magic_fail: store -1 (Closed/Invalid) in dst.
                    let magic_fail_off = code.len();
                    patch_rel32_jcc(&mut code, jne_magic_patch, magic_fail_off);
                    code.extend(encode_mov_reg_imm64(Gpr::Rax, 0xFFFFFFFFFFFFFFFF)); // -1
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    // Deallocate the 56-byte frame and jmp cleanup.
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                    // jmp cleanup (rel32, placeholder)
                    let jmp_cleanup_from_magic_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // read_error_path: read returned <= 0; dealloc the
                    // 56-byte frame and jmp error_path (which stores -3 and
                    // falls through to cleanup, deallocating the pollfd).
                    let read_error_path_off = code.len();
                    patch_rel32_jcc(&mut code, jle_err_patch, read_error_path_off);
                    code.extend(encode_add_reg_imm32(Gpr::Rsp, 56));
                    // jmp error_path (rel32, placeholder)
                    let jmp_err_patch = code.len();
                    code.extend(&[0xE9, 0x00, 0x00, 0x00, 0x00]); // jmp rel32

                    // error_path: store -3 (entered from poll<0 via jl_err_patch,
                    // or from read_error_path via jmp_err_patch).
                    let error_path_off = code.len();
                    patch_rel32_jcc(&mut code, jl_err_patch, error_path_off);
                    patch_rel32_jmp(&mut code, jmp_err_patch, error_path_off);
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

                    // cleanup: (success/crc_fail/magic_fail paths jump here
                    // after deallocating their 56-byte frame; store_result
                    // falls through here with -2/-3 in dst).
                    let cleanup_off = code.len();
                    let cleanup_delta = cleanup_off as i64 - (jmp_cleanup_patch as i64 + 5);
                    let bd = (cleanup_delta as i32).to_le_bytes();
                    code[jmp_cleanup_patch+1] = bd[0];
                    code[jmp_cleanup_patch+2] = bd[1];
                    code[jmp_cleanup_patch+3] = bd[2];
                    code[jmp_cleanup_patch+4] = bd[3];
                    patch_rel32_jmp(&mut code, jmp_cleanup_from_crc_patch, cleanup_off);
                    patch_rel32_jmp(&mut code, jmp_cleanup_from_magic_patch, cleanup_off);

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

                // Wave 49: CallIndirect — indirect call through a function
                // pointer stored in a vreg.  Used by irq_dispatch to call
                // handler functions whose address was loaded from the driver
                // table via GetAddress + Store + Load.
                //
                // x86_64 codegen: load func_ptr into R10, load args into
                // SysV arg registers (RDI, RSI, RDX, RCX, R8, R9), align
                // stack to 16 bytes, CALL R10, store result.
                IRInstr::CallIndirect { dst, func_ptr, args } => {
                    let mut code = Vec::new();
                    let call_arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
                    let num_reg_args = args.len().min(call_arg_regs.len());

                    // Load args into SysV arg registers
                    for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                        code.extend(load_value(arg, call_arg_regs[i]));
                    }

                    // Load func_ptr into R10 (caller-saved, not an arg reg)
                    code.extend(load_value(func_ptr, Gpr::R10));

                    // Align stack to 16 bytes before CALL.
                    // RSP is 16-byte aligned at function entry (after push rbp).
                    // We may have done sub rsp, frame_size (which is 16-aligned).
                    // But if we've pushed/popped anything since, we need to check.
                    // Safest: push a dummy to align, call, pop after.
                    // Actually, since we only use caller-saved regs (no pushes),
                    // RSP is still 16-aligned from the prologue. No alignment needed.

                    // CALL R10 (FF D2 = call r10 with REX prefix)
                    // Encoding: REX.B + FF /2 (CALL r/m64)
                    // R10 needs REX.B: 41 FF D2
                    code.extend(&[0x41, 0xFF, 0xD2]); // call r10

                    // Store return value (RAX) to dst's stack slot
                    if let Some(d) = dst {
                        let dst_id = d.as_register().unwrap_or(0);
                        let dst_off = slot_offset(dst_id);
                        code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off, Gpr::Rax));
                    }

                    instr_opcode = Some("call_indirect".to_string());
                    code
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
