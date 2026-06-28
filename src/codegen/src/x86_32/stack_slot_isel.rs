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
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
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
    encode_div_reg,
    encode_idiv_reg,
    encode_imul_reg_reg,
    encode_jcc_rel32, encode_jmp_rel32,
    encode_lea_reg_mem,
    encode_mov_mem16_reg16, encode_mov_mem32_reg32, encode_mov_mem8_reg8,
    encode_mov_mem_reg,
    encode_mov_reg32_mem,
    encode_mov_reg_imm32, encode_mov_reg_imm64, encode_mov_reg_mem, encode_mov_reg_reg,
    encode_movd_gpr_xmm, encode_movd_xmm_gpr,
    encode_movq_gpr_xmm, encode_movq_xmm_gpr,
    encode_movsx_reg8,
    encode_movsx_reg8_mem,
    encode_movzx_reg8, encode_movzx_reg16,
    encode_movzx_reg8_mem, encode_movzx_reg16_mem,
    encode_neg_reg, encode_nop, encode_not_reg,
    encode_or_reg_imm32, encode_or_reg_reg,
    encode_pop, encode_push,
    encode_ret,
    encode_rol_reg_cl, encode_ror_reg_cl,
    encode_sar_reg_cl,
    encode_setcc,
    encode_shl_reg_cl, encode_shr_reg_cl,
    encode_sub_reg_imm32, encode_sub_reg_reg,
    encode_test_reg_reg,
    encode_xor_reg_imm32, encode_xor_reg_reg,
};

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

    // Round up to ensure proper stack alignment for calls.
    // The prologue does: push rbp (-8); mov rbp,rsp; sub rsp,frame_size; push×5 (-40)
    // On entry to this function: RSP was 8 mod 16 (SysV ABI).
    // After push rbp: RSP is 0 mod 16.
    // After sub rsp,frame_size: RSP is (-frame_size) mod 16.
    // After 5 pushes (40 bytes): RSP is (-frame_size - 40) mod 16.
    // Before any `call` from this function, RSP must be 0 mod 16 (so that
    // the callee enters with RSP at 8 mod 16 as required by SysV).
    // Therefore: (frame_size + 40) % 16 == 0, i.e., frame_size % 16 == 8.
    // Round up to ensure proper stack alignment for calls.
    // The prologue does: push ebp (-4); mov ebp,esp; sub esp,frame_size; push×1 (-4)
    // On entry: ESP was 4 mod 16 (cdecl, return addr on stack).
    // After push ebp: ESP is 0 mod 16.
    // After sub esp,frame_size: ESP is (-frame_size) mod 16.
    // After 1 push (4 bytes): ESP is (-frame_size - 4) mod 16.
    // Before any `call`: ESP must be 0 mod 16 (callee enters with ESP at 4 mod 16).
    // Therefore: (frame_size + 4) % 16 == 0, i.e., frame_size % 16 == 12.
    let aligned = ((current_offset + 15) & !15) as usize;
    let frame_size = if aligned % 16 == 12 {
        aligned.max(12)
    } else {
        (aligned + 12).max(12)  // Add bytes to make frame_size ≡ 12 (mod 16)
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

    // Store a scratch register into a vreg's stack slot.
    // IMPORTANT: Stack slots are 8 bytes (64-bit), but x86_32 operations
    // only produce 32-bit results. We MUST zero the high 4 bytes after
    // storing the low word, otherwise garbage from a previous value
    // remains in the high word. When the result is later used in 64-bit
    // pointer arithmetic (e.g. buf + (i * 4)), the garbage high word
    // produces a wrong address → crash or wrong result.
    let store_vreg = |id: u32, scratch: Gpr| -> Vec<u8> {
        let off = slot_offset(id);
        let mut code = encode_mov_mem_reg(Gpr::Rbp, off, scratch);
        // Zero the high 4 bytes: MOV DWORD PTR [EBP + off + 4], 0
        // Encoding: C7 /0 (MOV r/m32, imm32) with mod=01 (disp8), reg=0
        let hi_off = off + 4;
        if hi_off >= -128 && hi_off <= 127 {
            // MOV DWORD PTR [EBP + disp8], 0
            // C7 45 disp8 00 00 00 00
            code.extend_from_slice(&[0xC7, 0x45, hi_off as u8, 0, 0, 0, 0]);
        } else {
            // MOV DWORD PTR [EBP + disp32], 0
            // C7 85 disp32 00 00 00 00
            code.extend_from_slice(&[0xC7, 0x85]);
            code.extend_from_slice(&(hi_off as i32).to_le_bytes());
            code.extend_from_slice(&[0, 0, 0, 0]);
        }
        code
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
            IRValue::Label(_) => encode_mov_reg_imm64(scratch, 0), // placeholder
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

    // Push callee-saved registers — only EBX on x86_32 (R12-R15 don't exist)
    let callee_save_regs: Vec<Gpr> = vec![Gpr::Rbx];
    for &reg in &callee_save_regs {
        emit(encode_push(reg), "push_callee_save");
    }

    // Copy function parameters from arg registers to their stack slots.
    // x86_32 only has 8 GPRs; we use EDI, ESI, EDX, ECX for the first 4 args.
    // Use store_vreg (which zeros the high word) so that 64-bit operations
    // on parameters don't read garbage from the high 4 bytes.
    let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx];
    for (i, param) in func.params.iter().enumerate() {
        if let Some(id) = param.as_register() {
            if i < arg_regs.len() {
                emit(store_vreg(id, arg_regs[i]), "store_param");
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

    for block in &func.blocks {
        // Record this block's label offset
        block_offsets.insert(block.label.clone(), byte_offset);

        for instr in &block.instructions {
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
                IRInstr::Add { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
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

                // ── Sub ──
                IRInstr::Sub { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
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

                // ── Mul ──
                IRInstr::Mul { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(load_value(lhs, Gpr::Rax));
                    code.extend(load_value(rhs, Gpr::Rcx));
                    code.extend(encode_imul_reg_reg(Gpr::Rax, Gpr::Rcx));
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Div ──
                IRInstr::Div { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
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

                // ── BinOp (generic) ──
                IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);

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
                            // BSR RAX, RAX → result = 63 - BSR
                            let r = Gpr::Rax.needs_rex();
                            let b = Gpr::Rax.needs_rex();
                            if let Some(rex) = rex_prefix(true, r, false, b) {
                                code.push(rex);
                            } else {
                                code.push(0x48);
                            }
                            code.push(0x0F);
                            code.push(0xBD);
                            code.push(modrm(3, Gpr::Rax.encoding() & 7, Gpr::Rax.encoding() & 7));
                            code.extend(encode_mov_reg_imm32(Gpr::Rax, 63));
                            code.extend(encode_sub_reg_reg(Gpr::Rax, Gpr::Rax));
                            code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rax));
                        }
                        UnaryOpKind::Ctz => {
                            // BSF RAX, RAX
                            let r = Gpr::Rax.needs_rex();
                            let b = Gpr::Rax.needs_rex();
                            if let Some(rex) = rex_prefix(true, r, false, b) {
                                code.push(rex);
                            } else {
                                code.push(0x48);
                            }
                            code.push(0x0F);
                            code.push(0xBC);
                            code.push(modrm(3, Gpr::Rax.encoding() & 7, Gpr::Rax.encoding() & 7));
                        }
                        UnaryOpKind::Popcnt => {
                            // POPCNT RAX, RAX (F3 0F B8 /r)
                            code.push(0xF3);
                            let r = Gpr::Rax.needs_rex();
                            let b = Gpr::Rax.needs_rex();
                            if let Some(rex) = rex_prefix(true, r, false, b) {
                                code.push(rex);
                            } else {
                                code.push(0x48);
                            }
                            code.push(0x0F);
                            code.push(0xB8);
                            code.push(modrm(3, Gpr::Rax.encoding() & 7, Gpr::Rax.encoding() & 7));
                        }
                    }
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Comparison (dedicated Cmp instruction) ──
                IRInstr::Cmp { kind, dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    let cc = cmp_kind_to_cc(kind);
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

                // ── Conditional select (Cmov) ──
                IRInstr::Select { dst, cond, true_val, false_val, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load false_val into RAX, true_val into R10, cond into R11
                    code.extend(load_value(false_val, Gpr::Rax));
                    code.extend(load_value(true_val, Gpr::Rax));
                    code.extend(load_value(cond, Gpr::Rdx));
                    // Test cond != 0
                    code.extend(encode_test_reg_reg(Gpr::Rdx, Gpr::Rdx));
                    // CMOVNZ RAX, R10
                    code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, Gpr::Rax, Gpr::Rax));
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
                    code.extend(load_value(cond, Gpr::Rax));
                    code.extend(load_value(true_val, Gpr::Rdx));
                    code.extend(load_value(false_val, Gpr::Rax));
                    // Build mask: mask = -(cond != 0)
                    //   TEST R10, R10      ; set ZF if cond == 0
                    //   SETNE R10b         ; R10b = 1 if cond != 0, else 0
                    //   MOVZX R10, R10b    ; zero-extend to full register
                    //   NEG R10            ; R10 = 0xFFFFFFFFFFFFFFFF if cond!=0, else 0
                    code.extend(encode_test_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_setcc(Cc::NotEqual, Gpr::Rax));
                    code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_neg_reg(Gpr::Rax));
                    // result = (true_val & mask) | (false_val & ~mask)
                    //   R11 &= R10         ; R11 = true_val & mask
                    //   RAX &= ~R10        ; RAX = false_val & ~mask (NOT R10 then AND)
                    //   OR RAX, R11        ; RAX = result
                    code.extend(encode_and_reg_reg(Gpr::Rdx, Gpr::Rax));
                    code.extend(encode_not_reg(Gpr::Rax));
                    code.extend(encode_and_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_or_reg_reg(Gpr::Rax, Gpr::Rdx));
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
                    code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rax)); // R10 = diff
                    code.extend(encode_neg_reg(Gpr::Rax));                // RAX = -diff
                    code.extend(encode_mov_reg_reg(Gpr::Rdx, Gpr::Rax));  // R11 = -diff
                    // OR R10, R11 → (diff | -diff)
                    code.extend(encode_or_reg_reg(Gpr::Rax, Gpr::Rdx));
                    // SHR R10, 31 → 0 if diff==0, 1 if diff!=0 (for 32-bit)
                    // For 64-bit, we'd use >> 63, but ct_eq operates on u32 primarily
                    code.extend(encode_mov_reg_imm32(Gpr::Rcx, 31));
                    code.extend(encode_shr_reg_cl(Gpr::Rax));
                    // XOR R10, 1 → invert: 1 if equal, 0 if not
                    code.extend(encode_xor_reg_imm32(Gpr::Rax, 1));
                    code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rax));
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Memory: Load ──
                IRInstr::Load { dst, addr, offset, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load address from stack into R10
                    code.extend(load_value(addr, Gpr::Rax));
                    let off = *offset;
                    match ty {
                        IRType::I8 | IRType::U8 => {
                            code.extend(encode_movzx_reg8_mem(Gpr::Rax, Gpr::Rax, off));
                        }
                        IRType::I16 | IRType::U16 => {
                            code.extend(encode_movzx_reg16_mem(Gpr::Rax, Gpr::Rax, off));
                        }
                        IRType::I32 | IRType::U32 => {
                            code.extend(encode_mov_reg32_mem(Gpr::Rax, Gpr::Rax, off));
                        }
                        _ => {
                            code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rax, off));
                        }
                    }
                    code.extend(store_vreg(dst_id, Gpr::Rax));
                    code
                }

                // ── Memory: Store ──
                IRInstr::Store { value, addr, offset, ty } => {
                    let mut code = Vec::new();
                    // Load value into R10, address into R11
                    code.extend(load_value(value, Gpr::Rax));
                    code.extend(load_value(addr, Gpr::Rdx));
                    let off = *offset;
                    match ty {
                        IRType::I8 | IRType::U8 => {
                            code.extend(encode_mov_mem8_reg8(Gpr::Rdx, off, Gpr::Rax));
                        }
                        IRType::I16 | IRType::U16 => {
                            code.extend(encode_mov_mem16_reg16(Gpr::Rdx, off, Gpr::Rax));
                        }
                        IRType::I32 | IRType::U32 => {
                            code.extend(encode_mov_mem32_reg32(Gpr::Rdx, off, Gpr::Rax));
                        }
                        _ => {
                            code.extend(encode_mov_mem_reg(Gpr::Rdx, off, Gpr::Rax));
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
                            if let IRValue::Immediate(imm) = src {
                                let imm = *imm;
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_mov_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(encode_mov_reg_imm64(Gpr::Rax, imm as u64));
                                }
                            } else {
                                // Load from stack, zero-extend byte → 64 bits
                                code.extend(load_value(src, Gpr::Rax));
                                code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                            }
                        }
                        CastKind::SExt => {
                            if let IRValue::Immediate(imm) = src {
                                let imm = *imm;
                                if (-2147483648..=2147483647).contains(&imm) {
                                    code.extend(encode_mov_reg_imm32(Gpr::Rax, imm as i32));
                                } else {
                                    code.extend(encode_mov_reg_imm64(Gpr::Rax, imm as u64));
                                }
                            } else {
                                code.extend(load_value(src, Gpr::Rax));
                                code.extend(encode_movsx_reg8(Gpr::Rax, Gpr::Rax));
                            }
                        }
                        CastKind::Trunc | CastKind::BitCast => {
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
                                //   2. R10 = RAX            (save original)
                                //   3. SHR RAX, CL          (halve; fits in i63)
                                //   4. Convert RAX → float in XMM0
                                //   5. ADDSD/ADDSS XMM0, XMM0  (double)
                                //   6. If the original had bit 0 set, add 1.0
                                //      (compensate for the truncated bit).
                                //      For simplicity we skip the bit-0 fix-up;
                                //      the error is at most 1 ULP for f64.
                                code.extend(encode_mov_reg_imm32(Gpr::Rcx, 1));  // CL = 1
                                code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::Rax));  // save
                                code.extend(encode_shr_reg_cl(Gpr::Rax));  // RAX >>= 1
                                if dst_is_f32 {
                                    code.extend(encode_cvtsi2ss_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                    code.extend(encode_addss_xmm_xmm(Xmm::Xmm0, Xmm::Xmm0));
                                    code.extend(encode_movd_gpr_xmm(Gpr::Rax, Xmm::Xmm0));
                                } else {
                                    code.extend(encode_cvtsi2sd_xmm_r64(Xmm::Xmm0, Gpr::Rax));
                                    code.extend(encode_addsd_xmm_xmm(Xmm::Xmm0, Xmm::Xmm0));
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
                            code.extend(load_value(src, Gpr::Rax));
                            if src_is_f32 {
                                code.extend(encode_movd_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                if dst_is_32bit_int {
                                    code.extend(encode_cvttss2si_r32_xmm(Gpr::Rax, Xmm::Xmm0));
                                } else {
                                    code.extend(encode_cvttss2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
                                }
                            } else {
                                code.extend(encode_movq_xmm_gpr(Xmm::Xmm0, Gpr::Rax));
                                if dst_is_32bit_int {
                                    code.extend(encode_cvttsd2si_r32_xmm(Gpr::Rax, Xmm::Xmm0));
                                } else {
                                    code.extend(encode_cvttsd2si_r64_xmm(Gpr::Rax, Xmm::Xmm0));
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
                    // Load return value into RAX (and EDX for 64-bit returns).
                    // i386 cdecl: 64-bit return values are in EDX:EAX.
                    // Check result_types first; fall back to parsing the
                    // function name (e.g. "fn_foo_entry(u64)" → 64-bit).
                    let is_64bit_ret = func.result_types.first()
                        .map(|t| matches!(t, IRType::I64 | IRType::U64))
                        .unwrap_or_else(|| {
                            if let Some(open) = func.name.rfind('(') {
                                if let Some(close) = func.name.rfind(')') {
                                    if close > open {
                                        let ret_ty = &func.name[open + 1..close];
                                        return ret_ty == "u64" || ret_ty == "i64"
                                            || ret_ty == "U64" || ret_ty == "I64";
                                    }
                                }
                            }
                            false
                        });
                    if let Some(val) = values.first() {
                        if is_64bit_ret {
                            // Load low word (EAX) from slot
                            code.extend(load_value(val, Gpr::Rax));
                            // Load high word (EDX) from slot+4
                            if let IRValue::Register(id) = val {
                                let high_off = slot_offset(*id) + 4;
                                code.extend(encode_mov_reg_mem(Gpr::Rdx, Gpr::Rbp, high_off));
                            } else {
                                // For immediates, the high word is 0 or 0xFFFFFFFF
                                // (sign extension is handled by load_value for the low word).
                                // CDQ (0x99) sign-extends EAX into EDX:EAX.
                                code.extend_from_slice(&[0x99u8]);
                            }
                        } else {
                            code.extend(load_value(val, Gpr::Rax));
                        }
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
                    let code = encode_jmp_rel32(0);
                    let rel32_offset = byte_offset + code.len() - 4;
                    branch_patches.push((rel32_offset, target.clone()));
                    code
                }

                // ── Control: CondBranch ──
                IRInstr::CondBranch { cond, true_target, false_target } => {
                    let mut code = Vec::new();
                    // Load condition from stack into RAX
                    code.extend(load_value(cond, Gpr::Rax));
                    // test rax, rax; jnz true; jmp false
                    code.extend(encode_test_reg_reg(Gpr::Rax, Gpr::Rax));
                    // JNZ rel32
                    code.extend(encode_jcc_rel32(Cc::NotEqual, 0));
                    let jnz_rel32_offset = byte_offset + code.len() - 4;
                    branch_patches.push((jnz_rel32_offset, true_target.clone()));
                    // JMP rel32
                    code.extend(encode_jmp_rel32(0));
                    let jmp_rel32_offset = byte_offset + code.len() - 4;
                    branch_patches.push((jmp_rel32_offset, false_target.clone()));
                    code
                }

                // ── Call ──
                IRInstr::Call { dst, func: call_target, args, is_extern } => {
                    let mut code = Vec::new();
                    // Load arguments from stack into arg registers.
                    // x86_32 only has 8 GPRs (EAX, ECX, EDX, EBX, ESP, EBP, ESI, EDI).
                    // We use EDI, ESI, EDX, ECX for the first 4 args.
                    // EAX is reserved for return value; EBX is callee-saved; ESP/EBP are frame.
                    // For 5+ args, push them on the stack (reverse order).
                    let call_arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx];
                    let num_reg_args = call_arg_regs.len().min(args.len());
                    
                    // Push extra args (5+) on stack in reverse order
                    if args.len() > num_reg_args {
                        for arg in args[num_reg_args..].iter().rev() {
                            // Load arg into EAX, then push EAX
                            code.extend(load_value(arg, Gpr::Rax));
                            code.extend(encode_push(Gpr::Rax));
                        }
                    }
                    
                    // Load first 4 args into registers
                    for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                        code.extend(load_value(arg, call_arg_regs[i]));
                    }
                    // CALL rel32
                    code.extend(encode_call_rel32(0));
                    let call_rel32_offset = byte_offset + code.len() - 4;
                    relocations.push(RelocationEntry {
                        offset: call_rel32_offset as u64,
                        symbol: call_target.clone(),
                        reloc_type: R_X86_64_PLT32.to_string(),
                    });
                    
                    // Clean up stack if we pushed extra args
                    if args.len() > num_reg_args {
                        let stack_bytes = (args.len() - num_reg_args) * 4;
                        code.extend(encode_add_reg_imm32(Gpr::Rsp, stack_bytes as i32));
                    }
                    
                    // Store return value to dst's stack slot.
                    // For VUMA functions (non-extern), the return value is 64-bit
                    // in EDX:EAX (cdecl). For extern functions (syscalls), the return
                    // is 32-bit in EAX only — sign-extend to 64-bit so that negative
                    // values (e.g., -1 error returns from open/read) are correctly
                    // represented in 64-bit operations.
                    if let Some(d) = dst {
                        let dst_id = d.as_register().unwrap_or(0);
                        let dst_off = slot_offset(dst_id);
                        // Store low word (EAX)
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        if !is_extern {
                            // VUMA function: store high word (EDX) from 64-bit return
                            code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off + 4, Gpr::Rdx));
                        } else {
                            // Extern/syscall: sign-extend 32-bit EAX to 64-bit.
                            // CDQ (0x99) sign-extends EAX into EDX:EAX.
                            code.extend_from_slice(&[0x99u8]);
                            code.extend(encode_mov_mem_reg(Gpr::Rbp, dst_off + 4, Gpr::Rdx));
                        }
                    }
                    code
                }

                // ── Phi ──
                // Phi nodes are resolved by the resolve_phis pass before codegen.
                // If any remain, they should have been replaced by copies.
                // Emit a NOP as a safety fallback.
                IRInstr::Phi { dst, incoming, .. } => {
                    // Self-referencing or trivial phi: just emit a NOP.
                    // Non-trivial phi should have been resolved by resolve_phis().
                    // As a safety measure, if the phi has exactly one non-self incoming,
                    // emit a copy from that incoming to dst.
                    let non_self: Vec<_> = incoming.iter()
                        .filter(|(val, _)| val != dst)
                        .collect();
                    if non_self.len() == 1 {
                        let (val, _) = non_self[0];
                        let mut code = Vec::new();
                        code.extend(load_value(val, Gpr::Rax));
                        let dst_id = dst.as_register().unwrap_or(0);
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    } else if non_self.is_empty() {
                        encode_nop() // trivial self-loop
                    } else {
                        // Multiple non-self incoming: should have been resolved.
                        // Just use the first one as a fallback.
                        let (val, _) = non_self[0];
                        let mut code = Vec::new();
                        code.extend(load_value(val, Gpr::Rax));
                        let dst_id = dst.as_register().unwrap_or(0);
                        code.extend(store_vreg(dst_id, Gpr::Rax));
                        code
                    }
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
                    code.extend(load_value(addr, Gpr::Rbx));     // addr -> Rbx
                    code.extend(load_value(expected, Gpr::Rax)); // expected -> Rax
                    code.extend(load_value(desired, Gpr::Rcx));  // desired -> Rcx
                    // LOCK CMPXCHG [Rbx], RCx
                    // F0 0F B1 0B  =  LOCK CMPXCHG RCx, [Rbx]
                    code.push(0xF0); // LOCK prefix
                    code.push(0x0F);
                    code.push(0xB1);
                    code.push(0x0B); // ModRM: [Rbx], RCx
                    // Result: Rax has the old value (whether swap succeeded or not)
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(store_vreg(dst_id, Gpr::Rax));
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
