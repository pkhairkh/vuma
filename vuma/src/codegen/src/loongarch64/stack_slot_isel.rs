//! # Stack-Slot ISel for LoongArch64
//!
//! Complete replacement for the `allocate_registers` method in the LoongArch64 backend.
//!
//! Every virtual register gets a stack slot at `[fp - offset]`. The ISel generates
//! code that loads source operands from their stack slots into scratch registers,
//! performs the operation, and stores the result to the destination's stack slot.
//!
//! ## Scratch Registers (never assigned to vregs)
//!
//! - $r4 (a0): primary scratch / return value
//! - $r5 (a1): secondary operand
//! - $r12 (t0): tertiary scratch
//! - $r13 (t1): quaternary scratch
//!
//! ## Stack Layout
//!
//! ```text
//! High address (toward stack base)
//!   ┌────────────────────┐
//!   │ Old frame          │
//!   ├────────────────────┤ ← $fp (= old $sp = $sp + frame_size after prologue)
//!   │ $ra (8 bytes)      │ ← $fp - 8
//!   │ Old $fp (8 bytes)  │ ← $fp - 16
//!   │ Vreg 0 (8 bytes)   │ ← $fp - 24
//!   │ Vreg 1 (8 bytes)   │ ← $fp - 32
//!   │ ...                │
//!   │ Vreg N-1           │ ← $fp - (24 + 8*(N-1))
//!   │ Alloc region 0     │ ← $fp - (24 + 8*N + alloc_offset_0)
//!   │ ...                │
//!   └────────────────────┘ ← $sp (after prologue)
//! Low address
//! ```

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction,
    BackendError, PhysicalReg, RegClass, RelocationEntry,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
use std::collections::HashMap;

use super::{Gpr, Instruction};

// =============================================================================
// Scratch registers
// =============================================================================

const S0: Gpr = Gpr::A0; // $r4 — primary scratch / return value
const S1: Gpr = Gpr::A1; // $r5 — secondary operand
const S2: Gpr = Gpr::T0; // $r12 — tertiary scratch
const S3: Gpr = Gpr::T1; // $r13 — quaternary scratch

// =============================================================================
// Helpers for instruction emission
// =============================================================================

fn emit(code: Vec<u8>, name: &str) -> AllocatedInstruction {
    AllocatedInstruction {
        opcode: name.to_string(),
        reads: vec![],
        writes: vec![],
        encoded: code,
    }
}

/// Encode a single instruction into an AllocatedInstruction.
fn emit_instr(inst: Instruction, name: &str) -> AllocatedInstruction {
    emit(inst.encode().to_vec(), name)
}

/// Load a 64-bit immediate into a register using the canonical LoongArch sequence.
///
/// Strategy:
/// 1. `lu12i.w rd, bits[31:12]` — sets bits[31:12] and sign-extends to 64 bits
/// 2. `ori rd, rd, bits[11:0]` — sets bits[11:0]
/// 3. If bits[63:32] don't match the sign-extension of bits[31]:
///    a. If it's a 32-bit unsigned value with bit 31 set: use `slli.d`+`srli.d` to zero-extend
///    b. Otherwise: `lu32i.d rd, bits[51:32]` — sets bits[51:32], preserving bits[31:0] and bits[63:52]
///    c. Then: `lu52i.d rd, rd, bits[63:52]` — sets bits[63:52], preserving bits[51:0]
fn encode_load_imm(rd: Gpr, imm: i64) -> Vec<u8> {
    let val = imm as u64;
    let mut code = Vec::with_capacity(16);

    // Step 1: lu12i.w rd, bits[31:12]
    let hi20 = ((val >> 12) & 0xFFFFF) as i32;
    code.extend_from_slice(&Instruction::Lu12iW { rd, imm20: hi20 }.encode());

    // Step 2: ori rd, rd, bits[11:0]
    let lo12 = (val & 0xFFF) as u32;
    code.extend_from_slice(&Instruction::Ori { rd, rj: rd, imm12: lo12 }.encode());

    // After lu12i.w + ori, rd = SignExtend(bits[31:0])
    // Check if the upper 32 bits match the sign extension
    let lower32 = val & 0xFFFFFFFF;
    let sign_ext = if lower32 & 0x80000000 != 0 {
        0xFFFFFFFF00000000u64
    } else {
        0u64
    };

    if val >> 32 == sign_ext >> 32 {
        // Upper bits already correct from sign extension — nothing more needed
        return code;
    }

    // The upper bits don't match sign extension.
    // Check if this is a 32-bit unsigned value (upper 32 bits should be 0
    // but sign extension made them 0xFFFFFFFF).
    if val >> 32 == 0 && lower32 & 0x80000000 != 0 {
        // Zero-extend: slli.d rd, rd, 32; srli.d rd, rd, 32
        code.extend_from_slice(&Instruction::SlliD { rd, rj: rd, imm8: 32 }.encode());
        code.extend_from_slice(&Instruction::SrliD { rd, rj: rd, imm8: 32 }.encode());
        return code;
    }

    // Full 64-bit value: use lu32i.d then lu52i.d
    // Step 3: lu32i.d rd, bits[51:32] — sets bits[51:32], preserves bits[31:0] and bits[63:52]
    let hi32 = ((val >> 32) & 0xFFFFF) as i32;
    code.extend_from_slice(&Instruction::Lu32iD { rd, imm20: hi32 }.encode());

    // Step 4: lu52i.d rd, rd, bits[63:52] — sets bits[63:52], preserves bits[51:0]
    let hi52 = ((val >> 52) & 0xFFF) as i32;
    code.extend_from_slice(&Instruction::Lu52iD { rd, rj: rd, imm12: hi52 }.encode());

    code
}

/// Always emit the full 4-instruction sequence for loading a 64-bit immediate.
///
/// This is needed for `GetAddress` and other cases where the immediate will be
/// patched later by an R_LARCH_64 relocation.  The full sequence is always 16 bytes
/// (4 × 4-byte instructions), guaranteeing room for any 64-bit value:
///
///   lu12i.w rd, bits[31:12]
///   ori     rd, rd, bits[11:0]
///   lu32i.d rd, bits[51:32]
///   lu52i.d rd, rd, bits[63:52]
fn encode_load_imm_full_64(rd: Gpr, val: u64) -> Vec<u8> {
    let mut code = Vec::with_capacity(16);

    // Step 1: lu12i.w rd, bits[31:12]
    let hi20 = ((val >> 12) & 0xFFFFF) as i32;
    code.extend_from_slice(&Instruction::Lu12iW { rd, imm20: hi20 }.encode());

    // Step 2: ori rd, rd, bits[11:0]
    let lo12 = (val & 0xFFF) as u32;
    code.extend_from_slice(&Instruction::Ori { rd, rj: rd, imm12: lo12 }.encode());

    // Step 3: lu32i.d rd, bits[51:32]
    let hi32 = ((val >> 32) & 0xFFFFF) as i32;
    code.extend_from_slice(&Instruction::Lu32iD { rd, imm20: hi32 }.encode());

    // Step 4: lu52i.d rd, rd, bits[63:52]
    let hi52 = ((val >> 52) & 0xFFF) as i32;
    code.extend_from_slice(&Instruction::Lu52iD { rd, rj: rd, imm12: hi52 }.encode());

    code
}

/// Check if a value fits in a signed 12-bit range.
fn fits_si12(val: i64) -> bool {
    (-2048..=2047).contains(&val)
}

/// Load a vreg from its stack slot into a scratch register.
/// Stack slot is at $fp - offset_from_fp.
fn encode_load_vreg(scratch: Gpr, fp: Gpr, offset_from_fp: i32) -> Vec<u8> {
    // offset_from_fp is negative (e.g., -24 for vreg 0)
    // Use ld.d (load doubleword) since stack slots are 8 bytes and may hold
    // 64-bit values (pointers, I64, etc.). For 32-bit values, the upper 32 bits
    // are guaranteed clean because store uses st.d which writes all 64 bits.
    if fits_si12(offset_from_fp as i64) {
        Instruction::LdD { rd: scratch, rj: fp, imm12: offset_from_fp }.encode().to_vec()
    } else {
        // Compute address: load offset into temp, add to $fp, then load
        let mut code = Vec::new();
        code.extend(encode_load_imm(S2, offset_from_fp as i64));
        code.extend_from_slice(&Instruction::AddD { rd: S2, rj: fp, rk: S2 }.encode());
        code.extend_from_slice(&Instruction::LdD { rd: scratch, rj: S2, imm12: 0 }.encode());
        code
    }
}

/// Store a scratch register into a vreg's stack slot.
fn encode_store_vreg(scratch: Gpr, fp: Gpr, offset_from_fp: i32) -> Vec<u8> {
    // Use st.d (store doubleword) to store all 64 bits. This ensures 64-bit
    // values (pointers, I64, etc.) are preserved. For 32-bit values, the upper
    // 32 bits are clean from zero-extending .w operations or explicit masking.
    if fits_si12(offset_from_fp as i64) {
        Instruction::StD { rd: scratch, rj: fp, imm12: offset_from_fp }.encode().to_vec()
    } else {
        // Compute address: load offset into temp, add to $fp, then store
        let mut code = Vec::new();
        code.extend(encode_load_imm(S2, offset_from_fp as i64));
        code.extend_from_slice(&Instruction::AddD { rd: S2, rj: fp, rk: S2 }.encode());
        code.extend_from_slice(&Instruction::StD { rd: scratch, rj: S2, imm12: 0 }.encode());
        code
    }
}

/// Load an IRValue into a scratch register.
fn encode_load_value(val: &IRValue, scratch: Gpr, fp: Gpr, vreg_slots: &HashMap<u32, i32>) -> Vec<u8> {
    match val {
        IRValue::Register(id) => {
            let off = vreg_slots.get(id).copied().unwrap_or(-24);
            encode_load_vreg(scratch, fp, off)
        }
        IRValue::Immediate(imm) => {
            encode_load_imm(scratch, *imm)
        }
        IRValue::Address(addr) => {
            encode_load_imm(scratch, *addr as i64)
        }
        IRValue::Label(_) => {
            encode_load_imm(scratch, 0) // placeholder
        }
    }
}

/// Store a scratch register to a vreg's stack slot (by vreg ID).
fn encode_store_to_vreg(scratch: Gpr, vreg_id: u32, fp: Gpr, vreg_slots: &HashMap<u32, i32>) -> Vec<u8> {
    let off = vreg_slots.get(&vreg_id).copied().unwrap_or(-24);
    encode_store_vreg(scratch, fp, off)
}

// =============================================================================
// Comparison lowering
// =============================================================================

fn encode_cmp(kind: &CmpKind, dst: Gpr, lhs: Gpr, rhs: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    match kind {
        CmpKind::Eq => {
            // xor dst, lhs, rhs; sltui dst, dst, 1
            code.extend_from_slice(&Instruction::Xor { rd: dst, rj: lhs, rk: rhs }.encode());
            code.extend_from_slice(&Instruction::Sltui { rd: dst, rj: dst, imm12: 1 }.encode());
        }
        CmpKind::Ne => {
            // xor dst, lhs, rhs; sltu dst, $r0, dst
            code.extend_from_slice(&Instruction::Xor { rd: dst, rj: lhs, rk: rhs }.encode());
            code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: Gpr::R0, rk: dst }.encode());
        }
        CmpKind::SLt => {
            code.extend_from_slice(&Instruction::Slt { rd: dst, rj: lhs, rk: rhs }.encode());
        }
        CmpKind::SLe => {
            // slt dst, rhs, lhs; xori dst, dst, 1
            code.extend_from_slice(&Instruction::Slt { rd: dst, rj: rhs, rk: lhs }.encode());
            code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode());
        }
        CmpKind::SGt => {
            code.extend_from_slice(&Instruction::Slt { rd: dst, rj: rhs, rk: lhs }.encode());
        }
        CmpKind::SGe => {
            // slt dst, lhs, rhs; xori dst, dst, 1
            code.extend_from_slice(&Instruction::Slt { rd: dst, rj: lhs, rk: rhs }.encode());
            code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode());
        }
        CmpKind::ULt => {
            code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: lhs, rk: rhs }.encode());
        }
        CmpKind::ULe => {
            code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: rhs, rk: lhs }.encode());
            code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode());
        }
        CmpKind::UGt => {
            code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: rhs, rk: lhs }.encode());
        }
        CmpKind::UGe => {
            code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: lhs, rk: rhs }.encode());
            code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode());
        }
    }
    code
}

fn binop_kind_to_cmp_kind(op: &BinOpKind) -> CmpKind {
    match op {
        BinOpKind::SLt => CmpKind::SLt,
        BinOpKind::SLe => CmpKind::SLe,
        BinOpKind::SGt => CmpKind::SGt,
        BinOpKind::SGe => CmpKind::SGe,
        BinOpKind::ULt => CmpKind::ULt,
        BinOpKind::ULe => CmpKind::ULe,
        BinOpKind::UGt => CmpKind::UGt,
        BinOpKind::UGe => CmpKind::UGe,
        BinOpKind::Eq => CmpKind::Eq,
        BinOpKind::Ne => CmpKind::Ne,
        other => unreachable!("BinOpKind::{:?} is not a comparison", other),
    }
}

// =============================================================================
// Main allocation function
// =============================================================================

pub fn allocate_registers(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let func_name = func.name.clone();

    // ── Phase 1: Collect all vreg IDs and compute stack layout ──

    let mut all_vreg_ids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &id in func.vregs.keys() {
        all_vreg_ids.insert(id);
    }
    for param in &func.params {
        if let Some(id) = param.as_register() {
            all_vreg_ids.insert(id);
        }
    }
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() {
                all_vreg_ids.insert(id);
            }
            for id in instr.used_regs() {
                all_vreg_ids.insert(id);
            }
        }
        // Also check terminators for vreg usage
        match &block.terminator {
            crate::ir::IRTerminator::Branch { cond, .. } => {
                if let Some(id) = cond.as_register() {
                    all_vreg_ids.insert(id);
                }
            }
            crate::ir::IRTerminator::Return(vals) => {
                for val in vals {
                    if let Some(id) = val.as_register() {
                        all_vreg_ids.insert(id);
                    }
                }
            }
            _ => {}
        }
    }

    // Identify Alloc vregs and compute their sizes
    let mut stack_alloc_vregs: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut alloc_sizes: HashMap<u32, i32> = HashMap::new();
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
    // $fp - 8: $ra
    // $fp - 16: old $fp
    // $fp - 24: vreg 0
    // $fp - 32: vreg 1
    // ...
    // $fp - (24 + 8*(N-1)): vreg N-1
    // $fp - (24 + 8*N): alloc region 0 (if any)
    // ...

    // Assign stack slots for ALL vregs (including Alloc vregs).
    let mut vreg_slots: HashMap<u32, i32> = HashMap::new(); // vreg → offset from $fp (negative)
    let mut all_vreg_ids_sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
    all_vreg_ids_sorted.sort();
    for (i, &id) in all_vreg_ids_sorted.iter().enumerate() {
        let offset = -(24 + 8 * i as i32);
        vreg_slots.insert(id, offset);
    }

    // Assign alloc region offsets (after all vreg slots)
    let num_vregs = all_vreg_ids_sorted.len() as i32;
    let vreg_area_end = 24 + 8 * num_vregs; // total bytes used for ra/fp/vregs
    let mut alloc_offsets: HashMap<u32, i32> = HashMap::new(); // vreg → offset from $fp (negative)
    let mut alloc_running: i32 = vreg_area_end;
    let mut alloc_vreg_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
    alloc_vreg_ids.sort();
    for &id in &alloc_vreg_ids {
        let size = alloc_sizes[&id];
        alloc_offsets.insert(id, -(alloc_running + size));
        alloc_running += size;
    }

    // Frame size = total space from $sp to $fp
    let frame_size = ((alloc_running + 15) & !15) as usize;

    // ── Phase 2: Generate code ──

    let mut instrs: Vec<AllocatedInstruction> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();

    // Track byte offset for branch patching
    let mut byte_offset: usize = 0;
    let mut push_code = |code: Vec<u8>, name: &str| {
        if !code.is_empty() {
            byte_offset += code.len();
            instrs.push(emit(code, name));
        }
    };

    let fp = Gpr::Fp; // $r22

    // ── Prologue ──
    // addi.d $sp, $sp, -frame_size
    // st.d $ra, $sp, frame_size-8
    // st.d $fp, $sp, frame_size-16
    // addi.d $fp, $sp, frame_size

    let fs = frame_size as i32;
    if fits_si12(-(fs as i64)) {
        push_code(
            Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -fs }.encode().to_vec(),
            "addi.d sp, sp, -frame_size",
        );
    } else {
        // Large frame: load -fs into scratch, then sub.d
        let mut code = encode_load_imm(S0, -(fs as i64));
        code.extend_from_slice(&Instruction::AddD { rd: Gpr::Sp, rj: Gpr::Sp, rk: S0 }.encode());
        push_code(code, "sub sp, sp, frame_size");
    }

    // st.d $ra, $sp, frame_size-8
    let ra_off = fs - 8;
    if fits_si12(ra_off as i64) {
        push_code(
            Instruction::StD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: ra_off }.encode().to_vec(),
            "st.d ra, sp, fs-8",
        );
    } else {
        let mut code = encode_load_imm(S0, ra_off as i64);
        code.extend_from_slice(&Instruction::AddD { rd: S0, rj: Gpr::Sp, rk: S0 }.encode());
        code.extend_from_slice(&Instruction::StD { rd: Gpr::Ra, rj: S0, imm12: 0 }.encode());
        push_code(code, "st.d ra, sp, fs-8");
    }

    // st.d $fp, $sp, frame_size-16
    let fp_off = fs - 16;
    if fits_si12(fp_off as i64) {
        push_code(
            Instruction::StD { rd: fp, rj: Gpr::Sp, imm12: fp_off }.encode().to_vec(),
            "st.d fp, sp, fs-16",
        );
    } else {
        let mut code = encode_load_imm(S0, fp_off as i64);
        code.extend_from_slice(&Instruction::AddD { rd: S0, rj: Gpr::Sp, rk: S0 }.encode());
        code.extend_from_slice(&Instruction::StD { rd: fp, rj: S0, imm12: 0 }.encode());
        push_code(code, "st.d fp, sp, fs-16");
    }

    // addi.d $fp, $sp, frame_size
    if fits_si12(fs as i64) {
        push_code(
            Instruction::AddiD { rd: fp, rj: Gpr::Sp, imm12: fs }.encode().to_vec(),
            "addi.d fp, sp, frame_size",
        );
    } else {
        let mut code = encode_load_imm(S0, fs as i64);
        code.extend_from_slice(&Instruction::AddD { rd: fp, rj: Gpr::Sp, rk: S0 }.encode());
        push_code(code, "add fp, sp, frame_size");
    }

    // Store function parameters from argument registers to their stack slots.
    // Per the LP64 ABI: first 8 params in $a0–$a7, remaining on the stack
    // at $fp+0, $fp+8, … (the caller's outgoing arg area).
    let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
    for (i, param) in func.params.iter().enumerate() {
        if let Some(id) = param.as_register() {
            if i < arg_regs.len() {
                // Parameters 0–7: already in arg registers, store to vreg slot
                push_code(
                    encode_store_to_vreg(arg_regs[i], id, fp, &vreg_slots),
                    "store_param",
                );
            } else {
                // Parameters 8+: passed on the stack at $fp + (i-8)*8
                let stack_off = ((i - 8) as i32) * 8;
                // Load from the stack into S0 ($a0), then store to vreg slot
                if fits_si12(stack_off as i64) {
                    push_code(
                        Instruction::LdD { rd: S0, rj: fp, imm12: stack_off }.encode().to_vec(),
                        "load_stack_param",
                    );
                } else {
                    let mut code = encode_load_imm(S2, stack_off as i64);
                    code.extend_from_slice(&Instruction::AddD { rd: S2, rj: fp, rk: S2 }.encode());
                    code.extend_from_slice(&Instruction::LdD { rd: S0, rj: S2, imm12: 0 }.encode());
                    push_code(code, "load_stack_param");
                }
                push_code(
                    encode_store_to_vreg(S0, id, fp, &vreg_slots),
                    "store_stack_param",
                );
            }
        }
    }

    // ── Phase 3: Encode each IR instruction ──

    // Track block label → byte_offset within the function's encoded output
    let mut block_offsets: HashMap<String, usize> = HashMap::new();
    // Track branches that need patching: (instr_index, target_label, is_cond_true)
    // For unconditional branches: (instr_index, target_label, false)
    // For conditional branches: two entries
    let mut branch_patches: Vec<(usize, String)> = Vec::new(); // (byte_offset_of_branch_instr, target_label)

    for block in &func.blocks {
        // Record this block's label offset
        block_offsets.insert(block.label.clone(), byte_offset);

        for instr in &block.instructions {
            let code = match instr {
                // ── Add ──
                IRInstr::Add { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load lhs into S0
                    code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                    // Add rhs
                    if let IRValue::Immediate(imm) = rhs {
                        let imm = *imm;
                        if fits_si12(imm) {
                            code.extend_from_slice(&Instruction::AddiD { rd: S0, rj: S0, imm12: imm as i32 }.encode());
                        } else {
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::AddD { rd: S0, rj: S0, rk: S1 }.encode());
                        }
                    } else {
                        code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                        code.extend_from_slice(&Instruction::AddD { rd: S0, rj: S0, rk: S1 }.encode());
                    }
                    // Store result
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Sub ──
                IRInstr::Sub { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                    if let IRValue::Immediate(imm) = rhs {
                        let imm = *imm;
                        if fits_si12(-imm) {
                            code.extend_from_slice(&Instruction::AddiD { rd: S0, rj: S0, imm12: -(imm as i32) }.encode());
                        } else {
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::SubD { rd: S0, rj: S0, rk: S1 }.encode());
                        }
                    } else {
                        code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                        code.extend_from_slice(&Instruction::SubD { rd: S0, rj: S0, rk: S1 }.encode());
                    }
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Mul ──
                IRInstr::Mul { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                    code.extend_from_slice(&Instruction::MulD { rd: S0, rj: S0, rk: S1 }.encode());
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Div ──
                IRInstr::Div { dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                    code.extend_from_slice(&Instruction::DivD { rd: S0, rj: S0, rk: S1 }.encode());
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── BinOp (generic) ──
                IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);

                    match op {
                        BinOpKind::Add => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                let imm = *imm;
                                if fits_si12(imm) {
                                    code.extend_from_slice(&Instruction::AddiD { rd: S0, rj: S0, imm12: imm as i32 }.encode());
                                } else {
                                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                    code.extend_from_slice(&Instruction::AddD { rd: S0, rj: S0, rk: S1 }.encode());
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::AddD { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::Sub => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                let imm = *imm;
                                if fits_si12(-imm) {
                                    code.extend_from_slice(&Instruction::AddiD { rd: S0, rj: S0, imm12: -(imm as i32) }.encode());
                                } else {
                                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                    code.extend_from_slice(&Instruction::SubD { rd: S0, rj: S0, rk: S1 }.encode());
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::SubD { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::Mul => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::MulD { rd: S0, rj: S0, rk: S1 }.encode());
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::SDiv => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::DivD { rd: S0, rj: S0, rk: S1 }.encode());
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::UDiv => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::DivDu { rd: S0, rj: S0, rk: S1 }.encode());
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::SRem => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::ModD { rd: S0, rj: S0, rk: S1 }.encode());
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::URem => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::ModDu { rd: S0, rj: S0, rk: S1 }.encode());
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::And => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                let uimm = *imm as u64;
                                if uimm < 4096 {
                                    code.extend_from_slice(&Instruction::Andi { rd: S0, rj: S0, imm12: uimm as u32 }.encode());
                                } else {
                                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                    code.extend_from_slice(&Instruction::And { rd: S0, rj: S0, rk: S1 }.encode());
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::And { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::Or => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                let uimm = *imm as u64;
                                if uimm < 4096 {
                                    code.extend_from_slice(&Instruction::Ori { rd: S0, rj: S0, imm12: uimm as u32 }.encode());
                                } else {
                                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                    code.extend_from_slice(&Instruction::Or { rd: S0, rj: S0, rk: S1 }.encode());
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::Or { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::Xor => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                if *imm == -1 {
                                    // XOR with -1 is NOT: use nor rd, rj, $r0 (flips all 64 bits)
                                    code.extend_from_slice(&Instruction::Nor { rd: S0, rj: S0, rk: Gpr::R0 }.encode());
                                } else {
                                    let uimm = *imm as u64;
                                    if uimm < 4096 {
                                        code.extend_from_slice(&Instruction::Xori { rd: S0, rj: S0, imm12: uimm as u32 }.encode());
                                    } else {
                                        code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                        code.extend_from_slice(&Instruction::Xor { rd: S0, rj: S0, rk: S1 }.encode());
                                    }
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::Xor { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::Shl => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                if *imm >= 0 && *imm < 64 {
                                    code.extend_from_slice(&Instruction::SlliD { rd: S0, rj: S0, imm8: *imm as u32 }.encode());
                                } else {
                                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                    code.extend_from_slice(&Instruction::SllD { rd: S0, rj: S0, rk: S1 }.encode());
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::SllD { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::ShrL => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                if *imm >= 0 && *imm < 64 {
                                    code.extend_from_slice(&Instruction::SrliD { rd: S0, rj: S0, imm8: *imm as u32 }.encode());
                                } else {
                                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                    code.extend_from_slice(&Instruction::SrlD { rd: S0, rj: S0, rk: S1 }.encode());
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::SrlD { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::ShrA => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            if let IRValue::Immediate(imm) = rhs {
                                if *imm >= 0 && *imm < 64 {
                                    code.extend_from_slice(&Instruction::SraiD { rd: S0, rj: S0, imm8: *imm as u32 }.encode());
                                } else {
                                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                    code.extend_from_slice(&Instruction::SraD { rd: S0, rj: S0, rk: S1 }.encode());
                                }
                            } else {
                                code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                                code.extend_from_slice(&Instruction::SraD { rd: S0, rj: S0, rk: S1 }.encode());
                            }
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::Ror => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend_from_slice(&Instruction::RotrD { rd: S0, rj: S0, rk: S1 }.encode());
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        BinOpKind::Rol => {
                            // ROL(x, n) = ROTR(x, 64-n)
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            // Compute 64-n: load 64 into S2, sub S2, S2, S1; then rotr.d S0, S0, S2
                            code.extend_from_slice(&Instruction::AddiD { rd: S2, rj: Gpr::R0, imm12: 64 }.encode());
                            code.extend_from_slice(&Instruction::SubD { rd: S2, rj: S2, rk: S1 }.encode());
                            code.extend_from_slice(&Instruction::RotrD { rd: S0, rj: S0, rk: S2 }.encode());
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                        // Comparison BinOps
                        BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
                        | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
                        | BinOpKind::Eq | BinOpKind::Ne => {
                            code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                            code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                            code.extend(encode_cmp(&binop_kind_to_cmp_kind(op), S0, S0, S1));
                            code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        }
                    }
                    code
                }

                // ── UnaryOp ──
                IRInstr::UnaryOp { op, dst, operand, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(operand, S0, fp, &vreg_slots));
                    match op {
                        UnaryOpKind::Neg => {
                            code.extend_from_slice(&Instruction::SubD { rd: S0, rj: Gpr::R0, rk: S0 }.encode());
                        }
                        UnaryOpKind::Not => {
                            code.extend_from_slice(&Instruction::Nor { rd: S0, rj: Gpr::R0, rk: S0 }.encode());
                        }
                        UnaryOpKind::Clz => {
                            // clz(x) = clo(~x)
                            code.extend_from_slice(&Instruction::Nor { rd: S0, rj: Gpr::R0, rk: S0 }.encode());
                            code.extend_from_slice(&Instruction::CloD { rd: S0, rj: S0 }.encode());
                        }
                        UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                            // Placeholder: just keep the value as-is
                        }
                    }
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Cmp ──
                IRInstr::Cmp { kind, dst, lhs, rhs, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(lhs, S0, fp, &vreg_slots));
                    code.extend(encode_load_value(rhs, S1, fp, &vreg_slots));
                    code.extend(encode_cmp(kind, S0, S0, S1));
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Load ──
                IRInstr::Load { dst, addr, offset, ty } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(addr, S0, fp, &vreg_slots));
                    // Add offset to address
                    if *offset != 0 {
                        if fits_si12(*offset as i64) {
                            code.extend_from_slice(&Instruction::AddiD { rd: S0, rj: S0, imm12: *offset }.encode());
                        } else {
                            code.extend(encode_load_imm(S2, *offset as i64));
                            code.extend_from_slice(&Instruction::AddD { rd: S0, rj: S0, rk: S2 }.encode());
                        }
                    }
                    // Load from address
                    let load_inst = match ty {
                        IRType::I8 => Instruction::LdB { rd: S0, rj: S0, imm12: 0 },
                        IRType::U8 => Instruction::LdBu { rd: S0, rj: S0, imm12: 0 },
                        IRType::I16 => Instruction::LdH { rd: S0, rj: S0, imm12: 0 },
                        IRType::U16 => Instruction::LdHu { rd: S0, rj: S0, imm12: 0 },
                        IRType::I32 => Instruction::LdW { rd: S0, rj: S0, imm12: 0 },
                        IRType::U32 => Instruction::LdWu { rd: S0, rj: S0, imm12: 0 },
                        _ => Instruction::LdD { rd: S0, rj: S0, imm12: 0 },
                    };
                    code.extend_from_slice(&load_inst.encode());
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Store ──
                IRInstr::Store { value, addr, offset, ty } => {
                    let mut code = Vec::new();
                    code.extend(encode_load_value(value, S0, fp, &vreg_slots));
                    code.extend(encode_load_value(addr, S1, fp, &vreg_slots));
                    // Add offset to address
                    if *offset != 0 {
                        if fits_si12(*offset as i64) {
                            code.extend_from_slice(&Instruction::AddiD { rd: S1, rj: S1, imm12: *offset }.encode());
                        } else {
                            code.extend(encode_load_imm(S2, *offset as i64));
                            code.extend_from_slice(&Instruction::AddD { rd: S1, rj: S1, rk: S2 }.encode());
                        }
                    }
                    // Store value at address
                    let store_inst = match ty {
                        IRType::I8 | IRType::U8 => Instruction::StB { rd: S0, rj: S1, imm12: 0 },
                        IRType::I16 | IRType::U16 => Instruction::StH { rd: S0, rj: S1, imm12: 0 },
                        IRType::I32 | IRType::U32 => Instruction::StW { rd: S0, rj: S1, imm12: 0 },
                        _ => Instruction::StD { rd: S0, rj: S1, imm12: 0 },
                    };
                    code.extend_from_slice(&store_inst.encode());
                    code
                }

                // ── Alloc ──
                IRInstr::Alloc { dst, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    if let Some(&alloc_off) = alloc_offsets.get(&dst_id) {
                        // Compute address: $fp + alloc_off (alloc_off is negative)
                        if fits_si12(alloc_off as i64) {
                            code.extend_from_slice(&Instruction::AddiD { rd: S0, rj: fp, imm12: alloc_off }.encode());
                        } else {
                            code.extend(encode_load_imm(S0, alloc_off as i64));
                            code.extend_from_slice(&Instruction::AddD { rd: S0, rj: fp, rk: S0 }.encode());
                        }
                    } else {
                        // Fallback: use $sp
                        code.extend_from_slice(&Instruction::AddiD { rd: S0, rj: Gpr::Sp, imm12: 0 }.encode());
                    }
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Ret ──
                IRInstr::Ret { values } => {
                    // Just load the return value into $a0 (S0 = A0).
                    // The actual epilogue is emitted by the IRTerminator::Return handler.
                    let mut code = Vec::new();
                    if let Some(val) = values.first() {
                        code.extend(encode_load_value(val, S0, fp, &vreg_slots));
                    }
                    code
                }

                // ── Call ──
                IRInstr::Call { dst, func: call_target, args } => {
                    let mut code = Vec::new();
                    // Per the LP64 ABI: first 8 args in $a0–$a7, args 8+ on
                    // the stack at $sp+0, $sp+8, …
                    let call_arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];

                    let num_stack_args = if args.len() > 8 { args.len() - 8 } else { 0 };
                    // Stack arg space, 16-byte aligned (ABI requirement)
                    let stack_arg_space = ((num_stack_args * 8 + 15) & !15) as i32;

                    // Step 1: Allocate stack space for stack-passed arguments
                    if stack_arg_space > 0 {
                        if fits_si12(-(stack_arg_space as i64)) {
                            code.extend_from_slice(
                                &Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -stack_arg_space }.encode(),
                            );
                        } else {
                            code.extend(encode_load_imm(S2, -(stack_arg_space as i64)));
                            code.extend_from_slice(
                                &Instruction::AddD { rd: Gpr::Sp, rj: Gpr::Sp, rk: S2 }.encode(),
                            );
                        }
                    }

                    // Step 2: Store stack-passed arguments (index >= 8) at $sp + (i-8)*8
                    for (i, arg) in args.iter().enumerate() {
                        if i >= call_arg_regs.len() {
                            let sp_off = ((i - call_arg_regs.len()) as i32) * 8;
                            // Load arg value into S0 (scratch), then store at $sp + sp_off
                            code.extend(encode_load_value(arg, S0, fp, &vreg_slots));
                            if fits_si12(sp_off as i64) {
                                code.extend_from_slice(
                                    &Instruction::StD { rd: S0, rj: Gpr::Sp, imm12: sp_off }.encode(),
                                );
                            } else {
                                // Large offset: compute address in S2, then store
                                code.extend(encode_load_imm(S2, sp_off as i64));
                                code.extend_from_slice(
                                    &Instruction::AddD { rd: S2, rj: Gpr::Sp, rk: S2 }.encode(),
                                );
                                code.extend_from_slice(
                                    &Instruction::StD { rd: S0, rj: S2, imm12: 0 }.encode(),
                                );
                            }
                        }
                    }

                    // Step 3: Load register-passed arguments (index 0–7) into $a0–$a7
                    for (i, arg) in args.iter().enumerate() {
                        if i < call_arg_regs.len() {
                            code.extend(encode_load_value(arg, call_arg_regs[i], fp, &vreg_slots));
                        }
                    }

                    // Step 4: BL — record a relocation for later patching
                    let bl_byte_offset = byte_offset + code.len();
                    code.extend_from_slice(&Instruction::Bl { offs26: 0 }.encode());
                    relocations.push(RelocationEntry {
                        offset: bl_byte_offset as u64,
                        symbol: call_target.clone(),
                        reloc_type: "R_LARCH_B26".to_string(),
                    });

                    // Step 5: Deallocate stack argument space
                    if stack_arg_space > 0 {
                        if fits_si12(stack_arg_space as i64) {
                            code.extend_from_slice(
                                &Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: stack_arg_space }.encode(),
                            );
                        } else {
                            code.extend(encode_load_imm(S2, stack_arg_space as i64));
                            code.extend_from_slice(
                                &Instruction::AddD { rd: Gpr::Sp, rj: Gpr::Sp, rk: S2 }.encode(),
                            );
                        }
                    }

                    // Store return value ($a0) to dst's stack slot
                    if let Some(d) = dst {
                        let dst_id = d.as_register().unwrap_or(0);
                        code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    }
                    code
                }

                // ── Branch (unconditional) ──
                IRInstr::Branch { target } => {
                    let patch_offset = byte_offset;
                    let code = Instruction::B { offs26: 0 }.encode().to_vec();
                    branch_patches.push((patch_offset, target.clone()));
                    code
                }

                // ── CondBranch ──
                IRInstr::CondBranch { cond, true_target, false_target } => {
                    let mut code = Vec::new();
                    // Load condition from stack into S0
                    code.extend(encode_load_value(cond, S0, fp, &vreg_slots));
                    // bnez S0, +1 (skip next instruction if true)
                    // But we need to patch this, so emit with placeholder
                    // The BNEZ offset is 1 (skip 1 instruction = 4 bytes) to fall through to true path
                    // Actually we need to patch it to jump to true_target
                    let bnez_offset = byte_offset + code.len();
                    code.extend_from_slice(&Instruction::Bnez { rj: S0, offs21: 0 }.encode());
                    branch_patches.push((bnez_offset, true_target.clone()));
                    // B false_target (unconditional)
                    let b_offset = byte_offset + code.len();
                    code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
                    branch_patches.push((b_offset, false_target.clone()));
                    code
                }

                // ── Cast ──
                IRInstr::Cast { kind, dst, src, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(src, S0, fp, &vreg_slots));
                    match kind {
                        CastKind::ZExt => {
                            // Zero-extend lower 32 bits: slli.d + srli.d clears upper 32 bits
                            code.extend_from_slice(&Instruction::SlliD { rd: S0, rj: S0, imm8: 32 }.encode());
                            code.extend_from_slice(&Instruction::SrliD { rd: S0, rj: S0, imm8: 32 }.encode());
                        }
                        CastKind::SExt => {
                            // Sign-extend from 32 bits: slli.w shifts lower 32 bits and
                            // sign-extends bit 31 to bits[63:32], then srai.d preserves
                            // the sign-extended value
                            code.extend_from_slice(&Instruction::SlliW { rd: S0, rj: S0, imm8: 0 }.encode());
                        }
                        CastKind::Trunc | CastKind::BitCast => {
                            // No-op: truncation happens naturally when storing to stack
                        }
                        CastKind::IntToFloat | CastKind::UIntToFloat |
                        CastKind::FloatToInt | CastKind::FloatToUInt |
                        CastKind::FloatToFloat => {
                            // FP conversion casts require FP register support;
                            // for now, treat as a no-op move.
                        }
                    }
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Select ──
                IRInstr::Select { dst, cond, true_val, false_val, .. } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Use branchless maskeqz/masknez:
                    //   maskeqz S0, false_val, cond  →  S0 = (cond == 0) ? false_val : 0
                    //   masknez S1, true_val, cond   →  S1 = (cond != 0) ? true_val : 0
                    //   or S0, S0, S1               →  S0 = result
                    code.extend(encode_load_value(false_val, S0, fp, &vreg_slots));
                    code.extend(encode_load_value(true_val, S1, fp, &vreg_slots));
                    code.extend(encode_load_value(cond, S2, fp, &vreg_slots));
                    // maskeqz S0, S0, S2: if cond==0, S0=false_val; else S0=0
                    code.extend_from_slice(&Instruction::Maskeqz { rd: S0, rj: S0, rk: S2 }.encode());
                    // masknez S1, S1, S2: if cond!=0, S1=true_val; else S1=0
                    code.extend_from_slice(&Instruction::Masknez { rd: S1, rj: S1, rk: S2 }.encode());
                    // or S0, S0, S1: merge
                    code.extend_from_slice(&Instruction::Or { rd: S0, rj: S0, rk: S1 }.encode());
                    // Store result
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Offset ──
                IRInstr::Offset { dst, base, offset } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    code.extend(encode_load_value(base, S0, fp, &vreg_slots));
                    code.extend(encode_load_value(offset, S1, fp, &vreg_slots));
                    code.extend_from_slice(&Instruction::AddD { rd: S0, rj: S0, rk: S1 }.encode());
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── GetAddress ──
                IRInstr::GetAddress { dst, name } => {
                    let mut code = Vec::new();
                    let dst_id = dst.as_register().unwrap_or(0);
                    // Load placeholder address using the full 4-instruction sequence
                    // (always 16 bytes) so the R_LARCH_64 relocation can patch in
                    // any 64-bit address later.
                    let imm_offset = byte_offset + code.len();
                    code.extend(encode_load_imm_full_64(S0, 0));
                    // Record relocation for the immediate
                    // The immediate is spread across 4 instructions (16 bytes).
                    // The relocation offset points to the first instruction.
                    relocations.push(RelocationEntry {
                        offset: imm_offset as u64,
                        symbol: name.clone(),
                        reloc_type: "R_LARCH_64".to_string(),
                    });
                    code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                    code
                }

                // ── Free ──
                IRInstr::Free { ptr: _ } => {
                    // Stack allocation — no-op
                    Vec::new()
                }

                // ── Phi ──
                IRInstr::Phi { dst, incoming, .. } => {
                    // Self-referencing or trivial phi: emit a copy if needed
                    let non_self: Vec<_> = incoming.iter()
                        .filter(|(val, _)| val != dst)
                        .collect();
                    if non_self.len() == 1 {
                        let (val, _) = non_self[0];
                        let mut code = Vec::new();
                        let dst_id = dst.as_register().unwrap_or(0);
                        code.extend(encode_load_value(val, S0, fp, &vreg_slots));
                        code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        code
                    } else if non_self.is_empty() {
                        // Trivial self-loop — no-op
                        Vec::new()
                    } else {
                        // Multiple non-self incoming: use the first one
                        let (val, _) = non_self[0];
                        let mut code = Vec::new();
                        let dst_id = dst.as_register().unwrap_or(0);
                        code.extend(encode_load_value(val, S0, fp, &vreg_slots));
                        code.extend(encode_store_to_vreg(S0, dst_id, fp, &vreg_slots));
                        code
                    }
                }
            };

            if !code.is_empty() {
                byte_offset += code.len();
                instrs.push(emit(code, &format!("{:?}", instr).split_whitespace().next().unwrap_or("unknown")));
            }
        }

        // Handle block terminators
        match &block.terminator {
            crate::ir::IRTerminator::Jump(target) => {
                let patch_offset = byte_offset;
                let code = Instruction::B { offs26: 0 }.encode().to_vec();
                branch_patches.push((patch_offset, target.clone()));
                byte_offset += code.len();
                instrs.push(emit(code, "jump"));
            }
            crate::ir::IRTerminator::Branch { cond, true_block, false_block } => {
                let mut code = Vec::new();
                // Load condition
                code.extend(encode_load_value(cond, S0, fp, &vreg_slots));
                // bnez S0, true_block (placeholder)
                let bnez_off = byte_offset + code.len();
                code.extend_from_slice(&Instruction::Bnez { rj: S0, offs21: 0 }.encode());
                branch_patches.push((bnez_off, true_block.clone()));
                // B false_block (placeholder)
                let b_off = byte_offset + code.len();
                code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
                branch_patches.push((b_off, false_block.clone()));
                byte_offset += code.len();
                instrs.push(emit(code, "cond_branch"));
            }
            crate::ir::IRTerminator::Return(vals) => {
                let mut code = Vec::new();
                // Load return value into $a0 (S0)
                if let Some(val) = vals.first() {
                    code.extend(encode_load_value(val, S0, fp, &vreg_slots));
                }
                // Epilogue
                code.extend_from_slice(&Instruction::LdD { rd: Gpr::Ra, rj: fp, imm12: -8 }.encode());
                code.extend_from_slice(&Instruction::LdD { rd: fp, rj: fp, imm12: -16 }.encode());
                if fits_si12(fs as i64) {
                    code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: fs }.encode());
                } else {
                    code.extend(encode_load_imm(S2, fs as i64));
                    code.extend_from_slice(&Instruction::AddD { rd: Gpr::Sp, rj: Gpr::Sp, rk: S2 }.encode());
                }
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                byte_offset += code.len();
                instrs.push(emit(code, "return"));
            }
            crate::ir::IRTerminator::Unreachable => {
                // Emit a break instruction
                let code = Instruction::Break.encode().to_vec();
                byte_offset += code.len();
                instrs.push(emit(code, "unreachable"));
            }
            crate::ir::IRTerminator::Switch { .. } => {
                // Not implemented — emit break
                let code = Instruction::Break.encode().to_vec();
                byte_offset += code.len();
                instrs.push(emit(code, "switch_unimplemented"));
            }
            crate::ir::IRTerminator::Invoke { .. }
            | crate::ir::IRTerminator::TailCall { .. }
            | crate::ir::IRTerminator::Resume { .. } => {
                // Not implemented — emit break
                let code = Instruction::Break.encode().to_vec();
                byte_offset += code.len();
                instrs.push(emit(code, "unimplemented_terminator"));
            }
        }
    }

    // ── Phase 4: Patch intra-function branch targets ──

    // Compute byte offset of each instruction
    let mut instr_offsets: Vec<usize> = Vec::with_capacity(instrs.len());
    let mut cur: usize = 0;
    for instr in &instrs {
        instr_offsets.push(cur);
        cur += instr.encoded.len();
    }

    // Patch each branch
    for (patch_offset, target_label) in &branch_patches {
        if let Some(&target_offset) = block_offsets.get(target_label) {
            // Find the instruction that contains this patch offset
            for (i, &start) in instr_offsets.iter().enumerate() {
                let end = start + instrs[i].encoded.len();
                if *patch_offset >= start && *patch_offset < end {
                    let within_instr = *patch_offset - start;
                    // Read the instruction word
                    if within_instr + 4 <= instrs[i].encoded.len() {
                        let word = u32::from_le_bytes([
                            instrs[i].encoded[within_instr],
                            instrs[i].encoded[within_instr + 1],
                            instrs[i].encoded[within_instr + 2],
                            instrs[i].encoded[within_instr + 3],
                        ]);
                        let opcode = (word >> 26) & 0x3F;

                        // Compute offset in instructions (4 bytes each)
                        let offset_bytes = target_offset as i64 - *patch_offset as i64;
                        let offset_instrs = offset_bytes / 4;

                        if opcode == 0x14 || opcode == 0x15 {
                            // B or BL: I26 format with non-linear bit layout
                            // Instruction bits: opcode[31:26] | offs26[15:0] in [25:10] | offs26[25:16] in [9:0]
                            let offs26 = (offset_instrs as u32) & 0x3FFFFFF;
                            let new_word = (word & 0xFC000000)
                                | ((offs26 & 0xFFFF) << 10)
                                | ((offs26 >> 16) & 0x3FF);
                            instrs[i].encoded[within_instr..within_instr + 4]
                                .copy_from_slice(&new_word.to_le_bytes());
                        } else if opcode == 0x10 || opcode == 0x11 {
                            // BEQZ (0x10) or BNEZ (0x11): 1RI21 format with non-linear bit layout
                            // Instruction bits: opcode[31:26] | offs[15:0] at [25:10] | rj at [9:5] | offs[20:16] at [4:0]
                            let offs21 = (offset_instrs as u32) & 0x1FFFFF;
                            let rj = (word >> 5) & 0x1F;
                            let new_word = ((opcode & 0x3F) << 26)
                                | ((offs21 & 0xFFFF) << 10)
                                | ((rj & 0x1F) << 5)
                                | ((offs21 >> 16) & 0x1F);
                            instrs[i].encoded[within_instr..within_instr + 4]
                                .copy_from_slice(&new_word.to_le_bytes());
                        } else if opcode == 0x13 {
                            // JIRL: not typically patched, but handle anyway
                            // offs16 in 2RI16 format
                            let offs16 = (offset_instrs as i32) & 0xFFFF;
                            let rd = word & 0x1F;
                            let rj = (word >> 5) & 0x1F;
                            let new_word = ((opcode & 0x3F) << 26)
                                | ((offs16 as u32 & 0xFFFF) << 10)
                                | ((rj & 0x1F) << 5)
                                | (rd & 0x1F);
                            instrs[i].encoded[within_instr..within_instr + 4]
                                .copy_from_slice(&new_word.to_le_bytes());
                        }
                        // 2RI16 conditional branches (BEQ, BNE, etc.) handled similarly
                        else if (0x16..=0x1B).contains(&opcode) {
                            let offs16 = (offset_instrs as i32) & 0xFFFF;
                            let rd = word & 0x1F;
                            let rj = (word >> 5) & 0x1F;
                            let new_word = ((opcode & 0x3F) << 26)
                                | ((offs16 as u32 & 0xFFFF) << 10)
                                | ((rj & 0x1F) << 5)
                                | (rd & 0x1F);
                            instrs[i].encoded[within_instr..within_instr + 4]
                                .copy_from_slice(&new_word.to_le_bytes());
                        }
                    }
                    break;
                }
            }
        }
    }

    let code_size: usize = instrs.iter().map(|i| i.encoded.len()).sum();

    // Callee-saved: we save $ra and $fp in the prologue
    let callee_saved: Vec<PhysicalReg> = vec![
        PhysicalReg::new(RegClass::Gpr, Gpr::Ra.encoding()),
        PhysicalReg::new(RegClass::Gpr, Gpr::Fp.encoding()),
    ];

    Ok(AllocatedFunction {
        name: func_name,
        blocks: vec![AllocatedBlock {
            label: "entry".to_string(),
            instructions: instrs,
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
