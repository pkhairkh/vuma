//! Full register-based instruction selection for hppa (HP PA-RISC 1.1).
//!
//! 3-operand arithmetic, big-endian, fixed 4-byte instructions, no delay slots
//! (but GATE needs a NOP after). Uses the target-agnostic register allocator
//! (`crate::regalloc::TargetAgnosticRegAlloc`) for vreg→preg assignment and
//! emits PA-RISC machine code directly.
//!
//! ## Frame Layout (FP-relative, FP = caller's SP)
//!
//! ```text
//!   FP-20                  saved RP (R2)
//!   FP-24                  saved old FP (R3)
//!   FP-28                  zero-extension scratch slot
//!   FP-32 .. FP-(32+cs*4)  saved callee-saved registers
//!   FP-(32+cs*4+8*i+8)     spill slot i (8 bytes each)
//!   FP-frame_size          SP (bottom of frame)
//! ```
//!
//! The allocator's `slot.offset` values (-8, -16, ...) are translated by
//! `spill_offset()` to skip the saved area, avoiding conflicts with RP/FP/cs.

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg,
    RelocationEntry,
};
use crate::ir::{
    BinOpKind, CmpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue, UnaryOpKind,
};
use crate::regalloc::{GenericSpillCode, RegAllocResult};
use super::*;

enum ResolvedVal {
    Reg(Reg),
    Imm(i64),
}
struct BranchFixup {
    offset: usize,
    target: String,
}
/// Fixup for cmpb (conditional branch) — uses a 13-bit non-linear
/// displacement encoding (see `encode_cmpb_disp`), patched in-place at
/// the 4-byte cmpb instruction.
struct CmpbFixup {
    offset: usize,
    target: String,
}

/// Emit a full function using the target-agnostic register allocator result.
///
/// Produces an `AllocatedFunction` with:
/// - A prologue (save RP/FP, set up frame, save callee-saved).
/// - An arg-shuffle (move R26-R23 → allocator-assigned regs).
/// - One `AllocatedBlock` per IR block, each containing the block's
///   instructions (with spill/reload code inserted per the allocator's
///   `spill_code` map).
/// - A trailing epilogue (restore callee-saved, restore RP/FP, BV R2).
/// - Branch fixups resolved against `label_offsets`.
pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // Determine which callee-saved registers the allocator actually used.
    // Exclude R3 (FP) — saved separately as "old FP". Exclude R1 (RP) and
    // R30 (SP) — they're reserved. R8-R14 (S0-S6) are not_allocatable, so
    // the allocator won't assign them, but filter just in case.
    let cs: Vec<Reg> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_reg(p))
        .filter(|r| *r != R3 && *r != R1 && *r != R30 && *r != R2)
        .collect();
    let cs_count = cs.len();

    // Frame size computation.
    // Layout (FP-relative, FP = caller's SP):
    //   FP-20              saved RP (R2)
    //   FP-24              saved old FP (R3)
    //   FP-28              zero-extension scratch (ss_load_imm)
    //   FP-32 .. FP-48     reserved for Mul/TMP64 helpers (FP-48 used by
    //                      emit_hppa_mulu32_to_64 as a scratch store slot)
    //   FP-52 .. FP-(52+cs*4)  saved callee-saved registers
    //   FP-(52+cs*4+8*i+8)     spill slot i (8 bytes each)
    //   FP-frame_size          SP (bottom of frame)
    //
    // The 16-byte reservation at FP-32..FP-48 ensures the Mul helper's
    // FP-48 scratch store does not collide with spill slots. Aligned to
    // 64 bytes (PA-RISC ABI requirement).
    let saved_area = 52 + cs_count * 4;
    let spill_bytes = alloc.total_spill_slots as usize * 8;
    let raw_frame = saved_area + spill_bytes;
    let frame_size = ((raw_frame + 63) & !63) as i32;

    // First spill slot (slot 0) goes just below the saved area.
    let spill_base: i32 = -(saved_area as i32 + 8);

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut cmpb_fixups: Vec<CmpbFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    // ── Prologue ──
    // PA-RISC stack grows DOWN. On entry, R30 (SP) = caller's SP.
    //   STW R2, -20(SP)       ; save RP at SP-20
    //   STW R3, -24(SP)       ; save old FP at SP-24
    //   COPY SP, FP           ; FP = SP (caller's SP)
    //   STW cs[i], -(52+i*4)(FP)  ; save callee-saved (below Mul scratch area)
    //   LDO -frame_size(SP), SP ; SP -= frame_size
    let p_start = all_code.len();
    all_code.extend_from_slice(&encode_stw(R2, R30, -20));
    all_code.extend_from_slice(&encode_stw(R3, R30, -24));
    all_code.extend_from_slice(&encode_copy(R30, R3));
    for (i, &r) in cs.iter().enumerate() {
        let off = -(52 + (i as i32) * 4);
        all_code.extend_from_slice(&encode_stw(r, R3, off as i16));
    }
    if (-8192..=8191).contains(&-frame_size) {
        all_code.extend_from_slice(&encode_ldo(R30, -frame_size as i16, R30));
    } else {
        // Large frame: load size into S3, then SUB.
        all_code.extend(ss_load_imm(S3, frame_size as i64));
        all_code.extend_from_slice(&encode_sub(R30, S3, R30));
    }
    let p_end = all_code.len();
    let prologue = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: vec![],
        encoded: all_code[p_start..p_end].to_vec(),
    };

    // ── Arg shuffle ──
    // PA-RISC arg regs (reversed): R26=arg0, R25=arg1, R24=arg2, R23=arg3.
    // Move each arg from the input reg to the allocator-assigned reg,
    // breaking cycles via S4 (R12, not_allocatable scratch).
    let as_start = all_code.len();
    let arg_regs: [Reg; 4] = [R26, R25, R24, R23];
    let mut pending: Vec<(Reg, Reg)> = Vec::new();
    for (i, p) in func.params.iter().enumerate() {
        if i >= 4 {
            break;
        }
        if let IRValue::Register(vid) = p {
            let r = alloc.coalesced_map.get(vid).unwrap_or(vid);
            if let Some(pg) = alloc.vreg_to_preg.get(r) {
                if let Some(d) = preg_to_reg(pg) {
                    let s = arg_regs[i];
                    if d != s {
                        pending.push((s, d));
                    }
                }
            }
        }
    }
    let mut prog = true;
    while prog && !pending.is_empty() {
        prog = false;
        let mut i = 0;
        while i < pending.len() {
            let (s, d) = pending[i];
            let mut cycle = false;
            for (j, (_, od)) in pending.iter().enumerate() {
                if i != j && *od == s {
                    cycle = true;
                    break;
                }
            }
            if !cycle {
                all_code.extend_from_slice(&encode_copy(s, d));
                pending.remove(i);
                prog = true;
            } else {
                i += 1;
            }
        }
    }
    for (s, d) in pending {
        all_code.extend_from_slice(&encode_copy(s, S4));
        all_code.extend_from_slice(&encode_copy(S4, d));
    }
    let as_end = all_code.len();
    let has_as = as_end > as_start;

    // ── Body ──
    let mut gp: u32 = 0;
    for block in &func.blocks {
        let bo = all_code.len();
        label_offsets.insert(block.label.clone(), bo);
        let mut instrs: Vec<AllocatedInstruction> = Vec::new();
        for instr in &block.instructions {
            if let Some(spills) = alloc.spill_code.get(&gp) {
                for sp in spills {
                    let s = all_code.len();
                    emit_spill(&mut all_code, sp, spill_base);
                    if all_code.len() > s {
                        instrs.push(AllocatedInstruction {
                            opcode: match sp {
                                GenericSpillCode::Spill { .. } => "spill",
                                _ => "reload",
                            }
                            .to_string(),
                            reads: vec![],
                            writes: vec![],
                            encoded: all_code[s..].to_vec(),
                        });
                    }
                }
            }
            let s = all_code.len();
            let (op, r, w) = emit_instr(&mut all_code, instr, alloc, &mut fixups, &mut cmpb_fixups, &mut relocations)?;
            let e = all_code.len();
            if e > s {
                instrs.push(AllocatedInstruction {
                    opcode: op,
                    reads: r,
                    writes: w,
                    encoded: all_code[s..e].to_vec(),
                });
            }
            gp += 2;
        }
        if let Some(spills) = alloc.spill_code.get(&gp) {
            for sp in spills {
                let s = all_code.len();
                emit_spill(&mut all_code, sp, spill_base);
                if all_code.len() > s {
                    instrs.push(AllocatedInstruction {
                        opcode: match sp {
                            GenericSpillCode::Spill { .. } => "spill",
                            _ => "reload",
                        }
                        .to_string(),
                        reads: vec![],
                        writes: vec![],
                        encoded: all_code[s..].to_vec(),
                    });
                }
            }
        }
        let s = all_code.len();
        emit_term(&mut all_code, &block.terminator, alloc, &mut fixups, &mut cmpb_fixups);
        let e = all_code.len();
        if e > s {
            instrs.push(AllocatedInstruction {
                opcode: "terminator".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: all_code[s..e].to_vec(),
            });
        }
        gp += 2;
        blocks.push(AllocatedBlock {
            label: block.label.clone(),
            instructions: instrs,
            code_offset: bo,
        });
    }

    // ── Trailing epilogue ──
    //   COPY FP, SP            ; SP = FP (deallocate frame)
    //   LDW -20(SP), R2        ; restore RP
    //   LDW -24(SP), R3        ; restore old FP
    //   LDW -(52+i*4)(SP), cs[i]  ; restore callee-saved (reverse order)
    //   BV R2(R0)              ; return
    //   NOP                    ; delay slot
    let ep_s = all_code.len();
    all_code.extend_from_slice(&encode_copy(R3, R30));
    all_code.extend_from_slice(&encode_ldw(R30, -20, R2));
    all_code.extend_from_slice(&encode_ldw(R30, -24, R3));
    for (i, &r) in cs.iter().enumerate().rev() {
        let off = -(52 + (i as i32) * 4);
        all_code.extend_from_slice(&encode_ldw(R30, off as i16, r));
    }
    all_code.extend_from_slice(&encode_bv(R2, R0));
    all_code.extend_from_slice(&encode_nop());
    let ep_e = all_code.len();

    if let Some(fb) = blocks.first_mut() {
        if has_as {
            fb.instructions.insert(
                0,
                AllocatedInstruction {
                    opcode: "arg_shuffle".to_string(),
                    reads: vec![],
                    writes: vec![],
                    encoded: all_code[as_start..as_end].to_vec(),
                },
            );
        }
        fb.instructions.insert(0, prologue);
    }
    if let Some(lb) = blocks.last_mut() {
        lb.instructions.push(AllocatedInstruction {
            opcode: "epilogue_trailing".to_string(),
            reads: vec![],
            writes: vec![],
            encoded: all_code[ep_s..ep_e].to_vec(),
        });
    }

    // ── Branch fixups ──
    // Each branch placeholder is 20 bytes (5 NOPs). The emit_branch helper
    // produces a BL+LDO+BV sequence (up to 20 bytes) that handles both
    // forward and backward branches.
    for f in &fixups {
        if let Some(&t) = label_offsets.get(&f.target) {
            let bl_offset = f.offset as i64;
            let target_offset = t as i64;
            let (branch_code, _) = emit_branch(target_offset, bl_offset);
            assert!(
                branch_code.len() <= 20,
                "branch code {} bytes exceeds 20-byte placeholder",
                branch_code.len()
            );
            for (i, byte) in branch_code.iter().enumerate() {
                all_code[f.offset + i] = *byte;
            }
        }
    }

    // ── Cmpb fixups (conditional branches) ──
    // cmpb uses a 13-bit non-linear displacement encoding (see
    // `encode_cmpb_disp`). Patch the 4-byte cmpb instruction in-place.
    // Displacement is relative to PC+8 (bytes), must be 4-byte aligned.
    for f in &cmpb_fixups {
        if let Some(&t) = label_offsets.get(&f.target) {
            let disp_bytes = ((t as i64 - f.offset as i64 - 8) as i32) & !3;
            let off = f.offset;
            let word = u32::from_be_bytes([
                all_code[off],
                all_code[off + 1],
                all_code[off + 2],
                all_code[off + 3],
            ]);
            let patched = (word & !0x1FFF) | encode_cmpb_disp(disp_bytes);
            all_code[off..off + 4].copy_from_slice(&patched.to_be_bytes());
        }
    }

    // Re-derive instruction encoded bytes from all_code (post-fixup).
    let mut off = 0;
    for b in &mut blocks {
        b.code_offset = off;
        for i in &mut b.instructions {
            let l = i.encoded.len();
            if l > 0 && off + l <= all_code.len() {
                i.encoded = all_code[off..off + l].to_vec();
            }
            off += l;
        }
    }

    let cs_phys: Vec<PhysicalReg> = cs
        .iter()
        .map(|r| PhysicalReg::new(crate::backend::RegClass::Gpr, *r as u32))
        .collect();

    Ok(AllocatedFunction {
        name: func.name.clone(),
        blocks,
        frame_size: frame_size as usize,
        callee_saved: cs_phys,
        spill_slots: alloc.total_spill_slots as usize,
        code_size: all_code.len(),
        relocations,
        wasm_func_type: None,
        wasm_locals: None,
    })
}

/// Convert a `PhysicalReg` to a PA-RISC `Reg` (u8).
/// Returns `None` for non-GPR registers or out-of-range indices.
fn preg_to_reg(p: &PhysicalReg) -> Option<Reg> {
    if p.class != crate::backend::RegClass::Gpr {
        return None;
    }
    if p.index > 31 {
        return None;
    }
    Some(p.index as u8)
}

/// Resolve an `IRValue` to either a register or an immediate.
fn resolve(v: &IRValue, a: &RegAllocResult) -> ResolvedVal {
    match v {
        IRValue::Register(id) => {
            let r = a.coalesced_map.get(id).unwrap_or(id);
            if let Some(p) = a.vreg_to_preg.get(r) {
                if let Some(g) = preg_to_reg(p) {
                    return ResolvedVal::Reg(g);
                }
            }
            ResolvedVal::Reg(R0)
        }
        IRValue::Immediate(i) => ResolvedVal::Imm(*i),
        IRValue::Address(a) => ResolvedVal::Imm(*a as i64),
        IRValue::Label(_) => ResolvedVal::Reg(R0),
    }
}

/// Load an `IRValue` into a register. If the value is an immediate, uses
/// S4 (R12, not_allocatable scratch) as a temporary. S4 is chosen because
/// it is NOT used by any of the codegen helpers (emit_hppa_mulu32_to_64
/// uses S0,S1,S2,S3,S5,S6; Div uses S0,S1,S3; Shl/ShrL/ShrA loops use
/// S5,S6). This ensures loading an immediate never clobbers a vreg.
///
/// **Caveat**: if two operands of the same instruction are both
/// immediates, the second `load_to_reg` call clobbers the first's result.
/// Callers must handle the both-Imm case specially (e.g., constant-fold
/// or use a different scratch for the second operand).
fn load_to_reg(v: &IRValue, a: &RegAllocResult, c: &mut Vec<u8>) -> Reg {
    match resolve(v, a) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(i) => {
            let s = S4;
            c.extend(ss_load_imm(s, i));
            s
        }
    }
}

/// Compute the actual FP-relative offset for a spill slot, translating the
/// allocator's `slot.offset` (which assumes slots start at FP-8) to skip
/// the saved area (RP, old FP, scratch, callee-saved).
fn spill_offset(slot: &crate::regalloc::GenericSpillSlot, spill_base: i32) -> i32 {
    // slot.offset = -(slot.index + 1) * 8 from the allocator.
    // We translate: actual = spill_base - slot.index * 8
    // (slot 0 at spill_base, slot 1 at spill_base-8, etc.)
    let _ = slot.offset; // ignored — we recompute from index.
    spill_base - (slot.index as i32) * 8
}

/// Emit spill (store) or reload (load) code for a `GenericSpillCode`.
fn emit_spill(c: &mut Vec<u8>, s: &GenericSpillCode, spill_base: i32) {
    match s {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(g) = preg_to_reg(preg) {
                let off = spill_offset(slot, spill_base);
                c.extend(ss_st(g, off));
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(g) = preg_to_reg(preg) {
                let off = spill_offset(slot, spill_base);
                c.extend(ss_ld(g, off));
            }
        }
    }
}

#[allow(clippy::possible_missing_else, unused_variables, unreachable_patterns)]
fn emit_instr(
    c: &mut Vec<u8>,
    instr: &IRInstr,
    a: &RegAllocResult,
    fx: &mut Vec<BranchFixup>,
    cfx: &mut Vec<CmpbFixup>,
    rel: &mut Vec<RelocationEntry>,
) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let op = match instr {
        IRInstr::Add { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return fp_fb(instr);
            }
            let d = load_to_reg(dst, a, c);
            let l = load_to_reg(lhs, a, c);
            match resolve(rhs, a) {
                ResolvedVal::Reg(r) => {
                    c.extend_from_slice(&encode_add(l, r, d));
                    reads.push(ph(r));
                }
                ResolvedVal::Imm(i) => {
                    if i == 0 {
                        if l != d {
                            c.extend_from_slice(&encode_copy(l, d));
                        }
                    } else if (-8192..=8191).contains(&i) {
                        // LDO dst = base + offset (PA-RISC LDO adds a 14-bit signed imm).
                        c.extend_from_slice(&encode_ldo(l, i as i16, d));
                    } else {
                        let s = load_to_reg(rhs, a, c);
                        c.extend_from_slice(&encode_add(l, s, d));
                    }
                }
            }
            reads.push(ph(l));
            writes.push(ph(d));
            "add".to_string()
        }
        IRInstr::Sub { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return fp_fb(instr);
            }
            let d = load_to_reg(dst, a, c);
            let l = load_to_reg(lhs, a, c);
            let r = load_to_reg(rhs, a, c);
            c.extend_from_slice(&encode_sub(l, r, d));
            reads.push(ph(l));
            reads.push(ph(r));
            writes.push(ph(d));
            "sub".to_string()
        }
        IRInstr::Mul { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return fp_fb(instr);
            }
            let d = load_to_reg(dst, a, c);
            let l = load_to_reg(lhs, a, c);
            let r = load_to_reg(rhs, a, c);
            // PA-RISC has no hardware MUL. Use the shift-and-add helper
            // emit_hppa_mulu32_to_64, which expects operands in S0/S1 and
            // returns lo in S0, hi in S2. S0-S6 are not_allocatable, so
            // clobbering them is safe.
            c.extend_from_slice(&encode_copy(l, S0));
            c.extend_from_slice(&encode_copy(r, S1));
            emit_hppa_mulu32_to_64(c);
            c.extend_from_slice(&encode_copy(S0, d));
            reads.push(ph(l));
            reads.push(ph(r));
            writes.push(ph(d));
            "mul".to_string()
        }
        IRInstr::Div { dst, lhs, rhs, ty: _ } => {
            let d = load_to_reg(dst, a, c);
            let l = load_to_reg(lhs, a, c);
            let r = load_to_reg(rhs, a, c);
            // PA-RISC has no hardware DIV. Use a subtraction loop
            // (quotient = 0; while (l >= r) { l -= r; quotient++; }).
            // Operands in S0 (l), S1 (r), result in S3. S0-S6 are safe to clobber.
            c.extend_from_slice(&encode_copy(l, S0));
            c.extend_from_slice(&encode_copy(r, S1));
            c.extend_from_slice(&encode_copy(R0, S3)); // S3 = quotient = 0
            let loop_off = c.len() as i64;
            // cmpb,<< S0, S1, exit  (unsigned less-than: if S0 <u S1, exit)
            c.extend_from_slice(&encode_cmpb(S0, S1, 0b100, false, false, 0));
            c.extend_from_slice(&encode_nop()); // delay slot
            c.extend_from_slice(&encode_sub(S0, S1, S0)); // S0 -= S1
            c.extend_from_slice(&encode_ldo(S3, 1, S3)); // S3++
            // Backward branch to loop_off
            let bl_off = c.len() as i64;
            c.extend(emit_backward_branch(loop_off, bl_off));
            // exit: patch cmpb to branch here
            let exit_off = c.len() as i64;
            let cmpb_disp = ((exit_off - loop_off - 8) as i32) & !3;
            let off = loop_off as usize;
            let word = u32::from_be_bytes([c[off], c[off + 1], c[off + 2], c[off + 3]]);
            let patched = (word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
            c[off..off + 4].copy_from_slice(&patched.to_be_bytes());
            c.extend_from_slice(&encode_copy(S3, d));
            reads.push(ph(l));
            reads.push(ph(r));
            writes.push(ph(d));
            "div".to_string()
        }
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return fp_fb(instr);
            }
            let d = load_to_reg(dst, a, c);
            let l = load_to_reg(lhs, a, c);
            // Check if rhs is a small immediate we can fold into Add/Sub via
            // LDO. PA-RISC LDO encodes a 14-bit signed displacement
            // (-8192..=8191) added to a base register. Using LDO directly
            // avoids loading rhs into the S4 scratch register, which would
            // clobber lhs when lhs is also an immediate loaded into S4.
            // And/Or/Xor have no clean immediate form on PA-RISC, so they
            // still use load_to_reg (the both-Imm clobber is rare for them).
            let rhs_val = resolve(rhs, a);
            let use_imm = match &rhs_val {
                ResolvedVal::Imm(i) => match op {
                    BinOpKind::Add => (-8192..=8191).contains(i),
                    // Sub: d = l - i = l + (-i); need -i to fit in LDO range.
                    BinOpKind::Sub => (-8191..=8192).contains(i),
                    _ => false,
                },
                _ => false,
            };
            let r = if use_imm {
                R0 // placeholder, not used by the immediate-form path
            } else {
                load_to_reg(rhs, a, c)
            };
            match op {
                BinOpKind::And | BinOpKind::Or | BinOpKind::Xor | BinOpKind::Add
                | BinOpKind::Sub | BinOpKind::Mul => {
                    match op {
                        BinOpKind::And => c.extend_from_slice(&encode_and(l, r, d)),
                        BinOpKind::Or => c.extend_from_slice(&encode_or(l, r, d)),
                        BinOpKind::Xor => c.extend_from_slice(&encode_xor(l, r, d)),
                        BinOpKind::Add => {
                            if use_imm {
                                if let ResolvedVal::Imm(i) = rhs_val {
                                    if i == 0 {
                                        if l != d {
                                            c.extend_from_slice(&encode_copy(l, d));
                                        }
                                    } else {
                                        c.extend_from_slice(&encode_ldo(l, i as i16, d));
                                    }
                                }
                            } else {
                                c.extend_from_slice(&encode_add(l, r, d));
                            }
                        }
                        BinOpKind::Sub => {
                            if use_imm {
                                if let ResolvedVal::Imm(i) = rhs_val {
                                    if i == 0 {
                                        if l != d {
                                            c.extend_from_slice(&encode_copy(l, d));
                                        }
                                    } else {
                                        // d = l - i = l + (-i)
                                        c.extend_from_slice(&encode_ldo(l, (-i) as i16, d));
                                    }
                                }
                            } else {
                                c.extend_from_slice(&encode_sub(l, r, d));
                            }
                        }
                        BinOpKind::Mul => {
                            // Use the shift-and-add helper.
                            c.extend_from_slice(&encode_copy(l, S0));
                            c.extend_from_slice(&encode_copy(r, S1));
                            emit_hppa_mulu32_to_64(c);
                            c.extend_from_slice(&encode_copy(S0, d));
                        }
                        _ => unreachable!(),
                    }
                    if !use_imm {
                        reads.push(ph(r));
                    }
                }
                BinOpKind::Shl => {
                    // PA-RISC SHRPW is right-shift only. For left shift by
                    // a register amount, use SHLADD with shift=1 in a loop,
                    // or use the immediate form if rhs is a constant.
                    match resolve(rhs, a) {
                        ResolvedVal::Imm(i) => {
                            let shift = (i & 31) as u8;
                            if shift == 0 {
                                if l != d {
                                    c.extend_from_slice(&encode_copy(l, d));
                                }
                            } else {
                                // Use SHLADD with shift=1, repeated `shift` times.
                                // SHLADD(1, l, R0, d) = d = l << 1.
                                // For shift > 3, repeat.
                                c.extend_from_slice(&encode_copy(l, d));
                                for _ in 0..shift {
                                    c.extend_from_slice(&encode_shladd(1, d, R0, d));
                                }
                            }
                        }
                        ResolvedVal::Reg(r) => {
                            // Loop: shift left by 1, r times.
                            c.extend_from_slice(&encode_copy(l, d));
                            c.extend_from_slice(&encode_copy(r, S5)); // S5 = counter
                            let loop_off = c.len() as i64;
                            // cmpb,= S5, R0, exit  (if S5 == 0, exit)
                            c.extend_from_slice(&encode_cmpb(S5, R0, 0b001, false, false, 0));
                            c.extend_from_slice(&encode_nop()); // delay slot
                            c.extend_from_slice(&encode_shladd(1, d, R0, d)); // d <<= 1
                            c.extend_from_slice(&encode_ldo(S5, -1, S5)); // S5--
                            let bl_off = c.len() as i64;
                            c.extend(emit_backward_branch(loop_off, bl_off));
                            let exit_off = c.len() as i64;
                            let cmpb_disp = ((exit_off - loop_off - 8) as i32) & !3;
                            let off = loop_off as usize;
                            let word = u32::from_be_bytes([
                                c[off],
                                c[off + 1],
                                c[off + 2],
                                c[off + 3],
                            ]);
                            let patched = (word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                            c[off..off + 4].copy_from_slice(&patched.to_be_bytes());
                            reads.push(ph(r));
                        }
                    }
                }
                BinOpKind::ShrL => {
                    // Logical right shift. SHRPW R0, l, sa, d = d = l >> sa.
                    match resolve(rhs, a) {
                        ResolvedVal::Imm(i) => {
                            let shift = (i & 31) as u8;
                            c.extend_from_slice(&encode_shrpw(R0, l, shift, d));
                        }
                        ResolvedVal::Reg(r) => {
                            // Loop: shift right by 1, r times.
                            c.extend_from_slice(&encode_copy(l, d));
                            c.extend_from_slice(&encode_copy(r, S5));
                            let loop_off = c.len() as i64;
                            c.extend_from_slice(&encode_cmpb(S5, R0, 0b001, false, false, 0));
                            c.extend_from_slice(&encode_nop());
                            c.extend_from_slice(&encode_shrpw(R0, d, 1, d));
                            c.extend_from_slice(&encode_ldo(S5, -1, S5));
                            let bl_off = c.len() as i64;
                            c.extend(emit_backward_branch(loop_off, bl_off));
                            let exit_off = c.len() as i64;
                            let cmpb_disp = ((exit_off - loop_off - 8) as i32) & !3;
                            let off = loop_off as usize;
                            let word = u32::from_be_bytes([
                                c[off],
                                c[off + 1],
                                c[off + 2],
                                c[off + 3],
                            ]);
                            let patched = (word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                            c[off..off + 4].copy_from_slice(&patched.to_be_bytes());
                            reads.push(ph(r));
                        }
                    }
                }
                BinOpKind::ShrA => {
                    // Arithmetic right shift. PA-RISC EXTRS (extract signed)
                    // would be ideal, but we don't have an encoder. Use a
                    // sign-propagating loop: copy MSB to S6, shift right
                    // logically, then OR in S6 at the top.
                    match resolve(rhs, a) {
                        ResolvedVal::Imm(i) => {
                            let shift = (i & 31) as u8;
                            if shift == 0 {
                                if l != d {
                                    c.extend_from_slice(&encode_copy(l, d));
                                }
                            } else {
                                // S6 = -(l >> 31) = 0 or 0xFFFFFFFF (sign mask)
                                c.extend_from_slice(&encode_shrpw(R0, l, 31, S6));
                                c.extend_from_slice(&encode_sub(R0, S6, S6));
                                // d = l >> shift (logical)
                                c.extend_from_slice(&encode_shrpw(R0, l, shift, d));
                                // S6 <<= (32 - shift) via repeated SHLADD(1)
                                let fill = 32 - shift as u32;
                                for _ in 0..fill {
                                    c.extend_from_slice(&encode_shladd(1, S6, R0, S6));
                                }
                                // d |= S6 (fill top bits with sign)
                                c.extend_from_slice(&encode_or(d, S6, d));
                            }
                        }
                        ResolvedVal::Reg(r) => {
                            // Loop-based: too complex for now; fall back to
                            // logical shift (incorrect for negative operands).
                            c.extend_from_slice(&encode_copy(l, d));
                            c.extend_from_slice(&encode_copy(r, S5));
                            let loop_off = c.len() as i64;
                            c.extend_from_slice(&encode_cmpb(S5, R0, 0b001, false, false, 0));
                            c.extend_from_slice(&encode_nop());
                            c.extend_from_slice(&encode_shrpw(R0, d, 1, d));
                            c.extend_from_slice(&encode_ldo(S5, -1, S5));
                            let bl_off = c.len() as i64;
                            c.extend(emit_backward_branch(loop_off, bl_off));
                            let exit_off = c.len() as i64;
                            let cmpb_disp = ((exit_off - loop_off - 8) as i32) & !3;
                            let off = loop_off as usize;
                            let word = u32::from_be_bytes([
                                c[off],
                                c[off + 1],
                                c[off + 2],
                                c[off + 3],
                            ]);
                            let patched = (word & !0x1FFF) | encode_cmpb_disp(cmpb_disp);
                            c[off..off + 4].copy_from_slice(&patched.to_be_bytes());
                            reads.push(ph(r));
                        }
                    }
                }
                BinOpKind::Eq
                | BinOpKind::Ne
                | BinOpKind::SLt
                | BinOpKind::ULt
                | BinOpKind::SLe
                | BinOpKind::ULe
                | BinOpKind::SGt
                | BinOpKind::UGt
                | BinOpKind::SGe
                | BinOpKind::UGe => {
                    // Comparison producing 0/1.
                    let r = load_to_reg(rhs, a, c);
                    emit_cmp(c, d, l, r, op);
                    reads.push(ph(r));
                }
                _ => {
                    return Err(BackendError::RegisterAllocFailed {
                        isa: "hppa",
                        reason: format!("BinOp {:?} not supported", op),
                    })
                }
            }
            reads.push(ph(l));
            writes.push(ph(d));
            "binop".to_string()
        }
        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let d = load_to_reg(dst, a, c);
            let s = load_to_reg(operand, a, c);
            match op {
                UnaryOpKind::Neg => {
                    // d = 0 - s
                    c.extend_from_slice(&encode_sub(R0, s, d));
                }
                UnaryOpKind::Not => {
                    // d = ~s = s XOR -1. Load -1 into S6.
                    c.extend_from_slice(&encode_copy(s, d));
                    c.extend(ss_load_imm(S6, -1));
                    c.extend_from_slice(&encode_xor(d, S6, d));
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    // Not implemented; store 0.
                    c.extend_from_slice(&encode_copy(R0, d));
                }
            }
            reads.push(ph(s));
            writes.push(ph(d));
            "unaryop".to_string()
        }
        IRInstr::Load { dst, addr, offset, ty } => {
            let d = load_to_reg(dst, a, c);
            let b = load_to_reg(addr, a, c);
            let o = *offset as i16;
            match ty {
                IRType::U8 | IRType::I8 => {
                    c.extend_from_slice(&encode_ldb(b, o, d));
                }
                IRType::U16 | IRType::I16 => {
                    c.extend_from_slice(&encode_ldh(b, o, d));
                }
                _ => {
                    c.extend_from_slice(&encode_ldw(b, o, d));
                }
            }
            reads.push(ph(b));
            writes.push(ph(d));
            "load".to_string()
        }
        IRInstr::Store { value, addr, offset, ty } => {
            let v = load_to_reg(value, a, c);
            let b = load_to_reg(addr, a, c);
            let o = *offset as i16;
            match ty {
                IRType::U8 | IRType::I8 => {
                    c.extend_from_slice(&encode_stb(v, b, o));
                }
                IRType::U16 | IRType::I16 => {
                    c.extend_from_slice(&encode_sth(v, b, o));
                }
                _ => {
                    c.extend_from_slice(&encode_stw(v, b, o));
                }
            }
            reads.push(ph(v));
            reads.push(ph(b));
            "store".to_string()
        }
        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => {
            let l = load_to_reg(lhs, a, c);
            let r = load_to_reg(rhs, a, c);
            let d = load_to_reg(dst, a, c);
            emit_cmp_kind(c, d, l, r, *kind);
            reads.push(ph(l));
            reads.push(ph(r));
            writes.push(ph(d));
            "cmp".to_string()
        }
        IRInstr::Select { dst, cond, true_val, false_val, .. }
        | IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let c_reg = load_to_reg(cond, a, c);
            let d = load_to_reg(dst, a, c);
            let f = load_to_reg(false_val, a, c);
            let t = load_to_reg(true_val, a, c);
            // d = f; if (c_reg != 0) d = t;
            c.extend_from_slice(&encode_copy(f, d));
            // cmpb,= c_reg, R0, skip  (if c_reg == 0, skip the move)
            c.extend_from_slice(&encode_cmpb(c_reg, R0, 0b001, false, true, 8));
            c.extend_from_slice(&encode_nop()); // delay slot (nullified if taken)
            c.extend_from_slice(&encode_copy(t, d));
            reads.push(ph(c_reg));
            reads.push(ph(f));
            reads.push(ph(t));
            writes.push(ph(d));
            "select".to_string()
        }
        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let l = load_to_reg(lhs, a, c);
            let r = load_to_reg(rhs, a, c);
            let d = load_to_reg(dst, a, c);
            // d = 0; cmpb,<> l, r, skip  (if l != r, skip the set-1)
            c.extend_from_slice(&encode_copy(R0, d));
            c.extend_from_slice(&encode_cmpb(l, r, 0b001, true, true, 8));
            c.extend_from_slice(&encode_nop());
            c.extend_from_slice(&encode_ldi(1, d));
            reads.push(ph(l));
            reads.push(ph(r));
            writes.push(ph(d));
            "ct_eq".to_string()
        }
        IRInstr::Cast { dst, src, .. } => {
            let s = load_to_reg(src, a, c);
            let d = load_to_reg(dst, a, c);
            if s != d {
                c.extend_from_slice(&encode_copy(s, d));
            }
            reads.push(ph(s));
            writes.push(ph(d));
            "cast".to_string()
        }
        IRInstr::Alloc { dst, size, .. } => {
            // Route to __vuma_alloc (brk-based allocator). Arg in R26, return in R28.
            let d = load_to_reg(dst, a, c);
            c.extend(ss_load_imm(R26, *size as i64));
            emit_call_pattern(c, rel, "__vuma_alloc");
            if d != R28 {
                c.extend_from_slice(&encode_copy(R28, d));
            }
            writes.push(ph(d));
            "alloc".to_string()
        }
        IRInstr::Free { ptr, .. } => {
            let p = load_to_reg(ptr, a, c);
            if p != R26 {
                c.extend_from_slice(&encode_copy(p, R26));
            }
            emit_call_pattern(c, rel, "__vuma_free");
            "free".to_string()
        }
        IRInstr::GetAddress { dst, name } => {
            let d = load_to_reg(dst, a, c);
            // Emit a placeholder that encode_program patches with the symbol's
            // absolute address (R_PARISC_DIR32 relocation, same pattern as
            // the stack-slot backend uses for GetAddress).
            // The placeholder is: LDO upper(R0), S0; 11× ADD S0,S0,S0; LDO lower(S0), S0.
            // Total: 13 instructions = 52 bytes. The patcher overwrites the
            // first LDO and the last LDO with the symbol's address split.
            let reloc_off = c.len() as u64;
            c.extend_from_slice(&encode_ldo_raw(R0, 0, S0)); // placeholder
            for _ in 0..11 {
                c.extend_from_slice(&encode_add(S0, S0, S0));
            }
            c.extend_from_slice(&encode_ldo_raw(S0, 0, S0)); // placeholder
            c.extend_from_slice(&encode_copy(S0, d));
            rel.push(RelocationEntry {
                offset: reloc_off,
                symbol: name.clone(),
                reloc_type: "R_PARISC_DIR32".to_string(),
            });
            writes.push(ph(d));
            "getaddr".to_string()
        }
        IRInstr::Offset { dst, base, offset, .. } => {
            let d = load_to_reg(dst, a, c);
            let b = load_to_reg(base, a, c);
            match resolve(offset, a) {
                ResolvedVal::Imm(i) => {
                    if (-8192..=8191).contains(&i) {
                        c.extend_from_slice(&encode_ldo(b, i as i16, d));
                    } else {
                        let s = load_to_reg(offset, a, c);
                        c.extend_from_slice(&encode_add(b, s, d));
                    }
                }
                ResolvedVal::Reg(o) => {
                    c.extend_from_slice(&encode_add(b, o, d));
                }
            }
            reads.push(ph(b));
            writes.push(ph(d));
            "offset".to_string()
        }
        IRInstr::Phi { dst, .. } => {
            let d = load_to_reg(dst, a, c);
            c.extend_from_slice(&encode_nop());
            writes.push(ph(d));
            "phi".to_string()
        }
        IRInstr::Ret { values } => {
            if let Some(f) = values.first() {
                let r = load_to_reg(f, a, c);
                if r != R28 {
                    c.extend_from_slice(&encode_copy(r, R28));
                }
            }
            c.extend_from_slice(&encode_nop());
            "ret".to_string()
        }
        IRInstr::Branch { target } => {
            // 20-byte placeholder for BL+LDO+BV (forward or backward).
            let pos = c.len();
            for _ in 0..5 {
                c.extend_from_slice(&encode_nop());
            }
            fx.push(BranchFixup { offset: pos, target: target.clone() });
            "branch".to_string()
        }
        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let c_reg = load_to_reg(cond, a, c);
            // cmpb,<> c_reg, R0, true_target  (branch if c_reg != 0)
            // delay slot: NOP (nullified if taken)
            // fall-through: 20-byte placeholder for false_target
            let p1 = c.len();
            c.extend_from_slice(&encode_cmpb(c_reg, R0, 0b001, true, true, 0));
            c.extend_from_slice(&encode_nop());
            // Cmpb displacement fixup (4-byte patch, non-linear encoding).
            cfx.push(CmpbFixup { offset: p1, target: true_target.clone() });
            // 20-byte placeholder for false_target (BL+LDO+BV)
            let p2 = c.len();
            for _ in 0..5 {
                c.extend_from_slice(&encode_nop());
            }
            fx.push(BranchFixup { offset: p2, target: false_target.clone() });
            reads.push(ph(c_reg));
            "cond_branch".to_string()
        }
        IRInstr::Syscall { nr, args, dst } => {
            let n = crate::syscall_abi::translate_or_warn(crate::backend::BackendKind::Hppa, *nr);
            // Load syscall number into R20.
            c.extend(ss_load_imm(R20, n as i64));
            // Load args into R26-R23 (reversed: arg0=R26, arg1=R25, arg2=R24, arg3=R23).
            let ar = [R26, R25, R24, R23];
            for (i, arg) in args.iter().enumerate().take(4) {
                let r = load_to_reg(arg, a, c);
                if r != ar[i] {
                    c.extend_from_slice(&encode_copy(r, ar[i]));
                }
            }
            // GATE + NOP (delay slot).
            c.extend_from_slice(&encode_gate());
            c.extend_from_slice(&encode_nop());
            if let Some(dv) = dst {
                let d = load_to_reg(dv, a, c);
                if d != R28 {
                    c.extend_from_slice(&encode_copy(R28, d));
                }
                writes.push(ph(d));
            }
            "syscall".to_string()
        }
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            // Load args into R26-R23 (reversed).
            let ar = [R26, R25, R24, R23];
            for (i, arg) in args.iter().enumerate().take(4) {
                let r = load_to_reg(arg, a, c);
                if r != ar[i] {
                    c.extend_from_slice(&encode_copy(r, ar[i]));
                }
            }
            // 32-byte call pattern (BL+LDO+BV with R_PARISC_PCREL relocation).
            emit_call_pattern(c, rel, fname);
            if let Some(dv) = dst {
                let d = load_to_reg(dv, a, c);
                if d != R28 {
                    c.extend_from_slice(&encode_copy(R28, d));
                }
                writes.push(ph(d));
            }
            if *is_extern {
                "call_extern".to_string()
            } else {
                "call".to_string()
            }
        }
        IRInstr::AtomicLoad { dst, addr, .. } => {
            let d = load_to_reg(dst, a, c);
            let b = load_to_reg(addr, a, c);
            c.extend_from_slice(&encode_ldw(b, 0, d));
            reads.push(ph(b));
            writes.push(ph(d));
            "atomic_load".to_string()
        }
        IRInstr::AtomicStore { value, addr, .. } => {
            let v = load_to_reg(value, a, c);
            let b = load_to_reg(addr, a, c);
            c.extend_from_slice(&encode_stw(v, b, 0));
            reads.push(ph(v));
            reads.push(ph(b));
            "atomic_store".to_string()
        }
        IRInstr::AtomicCas { .. } => {
            c.extend_from_slice(&encode_nop());
            "atomic_cas".to_string()
        }
        _ => {
            c.extend_from_slice(&encode_nop());
            "unhandled".to_string()
        }
    };
    Ok((op, reads, writes))
}

/// Emit a comparison producing 0/1 in `d`, based on a `BinOpKind` (used
/// for comparison BinOps like Eq, SLt, etc.).
///
/// Logic: `d = 0; if (cond) d = 1`. The cmpb branches when cond is FALSE
/// (to skip the `d = 1`), so we use the INVERTED form of the condition.
fn emit_cmp(c: &mut Vec<u8>, d: Reg, l: Reg, r: Reg, op: &BinOpKind) {
    c.extend_from_slice(&encode_copy(R0, d)); // d = 0
    // For "if (cond) d = 1", branch when cond is FALSE (skip d = 1).
    // PA-RISC cmpb: non-inverted branches when cond TRUE; inverted branches
    // when cond FALSE. So we flip the inverted flag.
    let (cond, inverted) = match op {
        BinOpKind::Eq => (0b001u32, true),  // branch when != (skip d=1 if not equal)
        BinOpKind::Ne => (0b001, false),    // branch when = (skip d=1 if equal)
        BinOpKind::SLt => (0b010, true),    // branch when >= (skip d=1 if not <)
        BinOpKind::SLe => (0b011, true),    // branch when >  (skip d=1 if not <=)
        BinOpKind::SGt => (0b010, false),   // branch when <  (skip d=1 if not >)
        BinOpKind::SGe => (0b011, false),   // branch when <  (skip d=1 if not >=)
        BinOpKind::ULt => (0b100, true),    // branch when >= (unsigned)
        BinOpKind::ULe => (0b101, true),    // branch when >  (unsigned)
        BinOpKind::UGt => (0b100, false),   // branch when <  (unsigned)
        BinOpKind::UGe => (0b101, false),   // branch when <  (unsigned)
        _ => (0b001, true),
    };
    // cmpb,<cond>[,inv] l, r, +8  (branch to PC+8+8 = PC+16, skipping d=1).
    c.extend_from_slice(&encode_cmpb(l, r, cond, inverted, true, 8));
    c.extend_from_slice(&encode_nop()); // delay slot (nullified if taken)
    c.extend_from_slice(&encode_ldi(1, d)); // d = 1
}

/// Emit a comparison producing 0/1 in `d`, based on a `CmpKind`.
fn emit_cmp_kind(c: &mut Vec<u8>, d: Reg, l: Reg, r: Reg, kind: CmpKind) {
    c.extend_from_slice(&encode_copy(R0, d)); // d = 0
    let (cond, inverted) = match kind {
        CmpKind::Eq => (0b001u32, true),
        CmpKind::Ne => (0b001, false),
        CmpKind::SLt => (0b010, true),
        CmpKind::SLe => (0b011, true),
        CmpKind::SGt => (0b010, false),
        CmpKind::SGe => (0b011, false),
        CmpKind::ULt => (0b100, true),
        CmpKind::ULe => (0b101, true),
        CmpKind::UGt => (0b100, false),
        CmpKind::UGe => (0b101, false),
    };
    c.extend_from_slice(&encode_cmpb(l, r, cond, inverted, true, 8));
    c.extend_from_slice(&encode_nop());
    c.extend_from_slice(&encode_ldi(1, d));
}

/// Emit the 32-byte BL+LDO+BV call pattern with an `R_PARISC_PCREL` relocation.
/// The `encode_program` pass patches this to branch to the target symbol.
fn emit_call_pattern(c: &mut Vec<u8>, rel: &mut Vec<RelocationEntry>, symbol: &str) {
    let call_offset = c.len() as u64;
    // Instr 1: BL,n +0, R1  → R1 = PC+8, branch to PC+8 (skip delay slot)
    c.extend_from_slice(&0xE8200000u32.to_be_bytes());
    // Instr 2: NOP (delay slot, nullified)
    c.extend_from_slice(&encode_nop());
    // Instr 3: LDO 24(R1), R2  → R2 = return address = PC+32
    c.extend_from_slice(&encode_ldo_raw(R1, 24, R2));
    // Instr 4: LDO 0(R1), R1  → placeholder (patched with target disp)
    c.extend_from_slice(&encode_ldo_raw(R1, 0, R1));
    // Instr 5-7: NOPs (placeholders for long calls)
    c.extend_from_slice(&encode_nop());
    c.extend_from_slice(&encode_nop());
    c.extend_from_slice(&encode_nop());
    // Instr 8: NOP (placeholder; patched to BV R0(R1) or BV,n R0(R1))
    c.extend_from_slice(&encode_nop());
    rel.push(RelocationEntry {
        offset: call_offset,
        symbol: symbol.to_string(),
        reloc_type: "R_PARISC_PCREL".to_string(),
    });
}

/// Emit terminator code (Jump / Branch / Return / Unreachable).
fn emit_term(
    c: &mut Vec<u8>,
    term: &IRTerminator,
    a: &RegAllocResult,
    fx: &mut Vec<BranchFixup>,
    cfx: &mut Vec<CmpbFixup>,
) {
    match term {
        IRTerminator::Jump(label) => {
            // 20-byte placeholder for BL+LDO+BV.
            let pos = c.len();
            for _ in 0..5 {
                c.extend_from_slice(&encode_nop());
            }
            fx.push(BranchFixup { offset: pos, target: label.clone() });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let c_reg = load_to_reg(cond, a, c);
            // cmpb,<> c_reg, R0, true_block  (branch if c_reg != 0)
            let p1 = c.len();
            c.extend_from_slice(&encode_cmpb(c_reg, R0, 0b001, true, true, 0));
            c.extend_from_slice(&encode_nop());
            cfx.push(CmpbFixup { offset: p1, target: true_block.clone() });
            // 20-byte placeholder for false_block.
            let p2 = c.len();
            for _ in 0..5 {
                c.extend_from_slice(&encode_nop());
            }
            fx.push(BranchFixup { offset: p2, target: false_block.clone() });
        }
        IRTerminator::Return(vals) => {
            if let Some(f) = vals.first() {
                let r = load_to_reg(f, a, c);
                if r != R28 {
                    c.extend_from_slice(&encode_copy(r, R28));
                }
            }
            // Epilogue: COPY FP, SP; LDW -20(SP), R2; LDW -24(SP), R3; BV R2(R0); NOP
            c.extend_from_slice(&encode_copy(R3, R30));
            c.extend_from_slice(&encode_ldw(R30, -20, R2));
            c.extend_from_slice(&encode_ldw(R30, -24, R3));
            c.extend_from_slice(&encode_bv(R2, R0));
            c.extend_from_slice(&encode_nop());
        }
        IRTerminator::Unreachable => {
            // Trap: load -1 into R20 (invalid syscall number) and GATE.
            c.extend(ss_load_imm(R20, -1));
            c.extend_from_slice(&encode_gate());
            c.extend_from_slice(&encode_nop());
        }
        _ => {
            c.extend_from_slice(&encode_nop());
        }
    }
}

/// Convert a `Reg` to a `PhysicalReg` for metadata.
fn ph(r: Reg) -> PhysicalReg {
    PhysicalReg::new(crate::backend::RegClass::Gpr, r as u32)
}

/// FP BinOp/Cmp fallback — returns an error since native FP is not supported
/// in the register-based path. The stack-slot backend handles FP via
/// soft-float stubs; the regalloc path defers to it.
fn fp_fb(instr: &IRInstr) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed {
        isa: "hppa",
        reason: format!(
            "FP not supported in register-based path (instr: {:?}); stack-slot backend handles FP via soft-float stubs",
            instr
        ),
    })
}
