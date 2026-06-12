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
    Cc, Gpr,
    R_X86_64_64, R_X86_64_PLT32,
    encode_add_reg_imm32, encode_add_reg_reg,
    encode_and_reg_imm32, encode_and_reg_reg,
    encode_call_rel32,
    encode_cmovcc_reg_reg,
    encode_cmp_reg_imm32, encode_cmp_reg_reg,
    encode_cqo,
    encode_div_reg,
    encode_idiv_reg,
    encode_imul_reg_reg,
    encode_jcc_rel32, encode_jmp_rel32,
    encode_lea_reg_mem,
    encode_mov_mem16_reg16, encode_mov_mem32_reg32, encode_mov_mem8_reg8,
    encode_mov_mem_reg,
    encode_mov_reg32_mem,
    encode_mov_reg_imm32, encode_mov_reg_imm64, encode_mov_reg_mem, encode_mov_reg_reg,
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
    let aligned = ((current_offset + 15) & !15) as usize;
    let frame_size = if aligned % 16 == 8 {
        aligned.max(8)
    } else {
        (aligned + 8).max(8)  // Add 8 bytes padding to make frame_size ≡ 8 (mod 16)
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

    // Push callee-saved registers (RBX, R12–R15) — always, to be safe
    let callee_save_regs: Vec<Gpr> = vec![Gpr::Rbx, Gpr::R12, Gpr::R13, Gpr::R14, Gpr::R15];
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

    for block in &func.blocks {
        // Record this block's label offset
        block_offsets.insert(block.label.clone(), byte_offset);

        for instr in &block.instructions {
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
                            code.extend(encode_mov_reg_imm32(Gpr::R10, 63));
                            code.extend(encode_sub_reg_reg(Gpr::R10, Gpr::Rax));
                            code.extend(encode_mov_reg_reg(Gpr::Rax, Gpr::R10));
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
                    code.extend(load_value(true_val, Gpr::R10));
                    code.extend(load_value(cond, Gpr::R11));
                    // Test cond != 0
                    code.extend(encode_test_reg_reg(Gpr::R11, Gpr::R11));
                    // CMOVNZ RAX, R10
                    code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, Gpr::Rax, Gpr::R10));
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
                IRInstr::Cast { kind, dst, src } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);

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
                IRInstr::Call { dst, func: call_target, args } => {
                    let mut code = Vec::new();
                    // Load arguments from stack into SystemV arg registers
                    let call_arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
                    for (i, arg) in args.iter().enumerate() {
                        if i < call_arg_regs.len() {
                            code.extend(load_value(arg, call_arg_regs[i]));
                        }
                    }
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
            };

            if !encoded.is_empty() {
                byte_offset += encoded.len();
                encoded_instrs.push(AllocatedInstruction {
                    opcode: format!("{:?}", instr)
                        .split_whitespace()
                        .next()
                        .unwrap_or("unknown")
                        .to_string(),
                    reads: vec![],
                    writes: vec![],
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
    })
}
