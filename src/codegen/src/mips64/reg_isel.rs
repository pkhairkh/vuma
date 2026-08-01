//! Full register-based instruction selection for mips64 (and mips64be).
//!
//! Uses MIPS64 N64 ABI encoding. mips64be inherits (byte-swap happens later).
//!
//! # Key MIPS Differences from RISC-V
//!
//! - **Branch delay slots**: every branch/jump/syscall must be followed by
//!   a NOP (or useful instruction in the delay slot).
//! - **4 arg registers**: $a0-$a3 (vs 8 on RISC-V).
//! - **Multiply/divide use HI/LO**: `dmult rs, rt; mflo rd` (two instructions).
//! - **Syscall**: number in $v0 (not $a7), `syscall` instruction, NOP after.
//! - **Frame pointer**: $fp ($30), stack pointer $sp ($29), return addr $ra ($31).
//! - **Scratch**: $at ($1, already not_allocatable in target_desc).

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CastKind, CmpKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::mips64::*;

enum ResolvedVal {
    Reg(Gpr),
    Imm(i64),
}

struct BranchFixup {
    offset: usize,
    target: String,
}

pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    let callee_saved_gprs: Vec<Gpr> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::Fp && *g != Gpr::Sp && *g != Gpr::Zero && *g != Gpr::Ra)
        .collect();
    let cs_count = 2 + callee_saved_gprs.len(); // ra + fp + callee-saved
    let callee_saved_size = cs_count * 8;
    let spill_size = alloc.total_spill_slots as usize * 8;
    let raw_frame = callee_saved_size + spill_size;
    let frame_size = ((raw_frame + 15) & !15) as i32;

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // ── Prologue ──
    // daddiu sp, sp, -frame_size
    // sd ra, frame_size-8(sp)
    // sd fp, frame_size-16(sp)
    // daddiu fp, sp, frame_size
    // sd s0, frame_size-24(sp); ...
    let prologue_start = all_code.len();
    all_code.extend_from_slice(&Instruction::Daddiu { rt: Gpr::Sp, rs: Gpr::Sp, imm: -frame_size }.encode());
    all_code.extend_from_slice(&Instruction::Sd { rt: Gpr::Ra, base: Gpr::Sp, offset: frame_size - 8 }.encode());
    all_code.extend_from_slice(&Instruction::Sd { rt: Gpr::Fp, base: Gpr::Sp, offset: frame_size - 16 }.encode());
    all_code.extend_from_slice(&Instruction::Daddiu { rt: Gpr::Fp, rs: Gpr::Sp, imm: frame_size }.encode());
    let mut cs_offset = frame_size - 24;
    for &g in &callee_saved_gprs {
        if cs_offset < 0 { break; }
        all_code.extend_from_slice(&Instruction::Sd { rt: g, base: Gpr::Sp, offset: cs_offset }.encode());
        cs_offset -= 8;
    }

    let prologue_end = all_code.len();
    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: callee_saved_gprs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect(),
        encoded: all_code[prologue_start..prologue_end].to_vec(),
    };

    // ── Argument shuffle (a0-a3 → allocator-assigned regs) ──
    let arg_shuffle_start = all_code.len();
    let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new();
    for (i, param) in func.params.iter().enumerate() {
        if i >= 4 { break; }
        if let IRValue::Register(vreg_id) = param {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(dst_gpr) = preg_to_gpr(preg) {
                    let src = arg_regs[i];
                    if dst_gpr != src {
                        pending.push((src, dst_gpr));
                    }
                }
            }
        }
    }
    let mut progress = true;
    while progress && !pending.is_empty() {
        progress = false;
        let mut i = 0;
        while i < pending.len() {
            let (src, dst) = pending[i];
            let mut conflict = false;
            for (j, (_, other_dst)) in pending.iter().enumerate() {
                if i != j && *other_dst == src { conflict = true; break; }
            }
            if !conflict {
                all_code.extend_from_slice(&Instruction::Or { rd: dst, rs: src, rt: Gpr::Zero }.encode()); // mov dst, src
                pending.remove(i);
                progress = true;
            } else { i += 1; }
        }
    }
    for (src, dst) in pending {
        all_code.extend_from_slice(&Instruction::Or { rd: Gpr::At, rs: src, rt: Gpr::Zero }.encode());
        all_code.extend_from_slice(&Instruction::Or { rd: dst, rs: Gpr::At, rt: Gpr::Zero }.encode());
    }
    let arg_shuffle_end = all_code.len();
    let has_arg_shuffle = arg_shuffle_end > arg_shuffle_start;

    // ── Body ──
    let mut global_pos: u32 = 0;
    for block in &func.blocks {
        let block_offset = all_code.len();
        label_offsets.insert(block.label.clone(), block_offset);
        let mut instrs: Vec<AllocatedInstruction> = Vec::new();

        for instr in &block.instructions {
            if let Some(spills) = alloc.spill_code.get(&global_pos) {
                for spill in spills {
                    let spill_start = all_code.len();
                    emit_spill_code(&mut all_code, spill);
                    if all_code.len() > spill_start {
                        instrs.push(AllocatedInstruction {
                            opcode: match spill { GenericSpillCode::Spill { .. } => "spill", _ => "reload" }.to_string(),
                            reads: vec![], writes: vec![], encoded: all_code[spill_start..].to_vec(),
                        });
                    }
                }
            }
            let instr_start = all_code.len();
            let (opcode, reads, writes) = emit_instruction(&mut all_code, instr, alloc, &mut fixups, &mut relocations)?;
            let instr_end = all_code.len();
            if instr_end > instr_start {
                instrs.push(AllocatedInstruction { opcode, reads, writes, encoded: all_code[instr_start..instr_end].to_vec() });
            }
            global_pos += 2;
        }

        if let Some(spills) = alloc.spill_code.get(&global_pos) {
            for spill in spills {
                let spill_start = all_code.len();
                emit_spill_code(&mut all_code, spill);
                if all_code.len() > spill_start {
                    instrs.push(AllocatedInstruction {
                        opcode: match spill { GenericSpillCode::Spill { .. } => "spill", _ => "reload" }.to_string(),
                        reads: vec![], writes: vec![], encoded: all_code[spill_start..].to_vec(),
                    });
                }
            }
        }

        let term_start = all_code.len();
        emit_terminator(&mut all_code, &block.terminator, alloc, frame_size, &callee_saved_gprs, &mut fixups);
        let term_end = all_code.len();
        if term_end > term_start {
            instrs.push(AllocatedInstruction { opcode: "terminator".to_string(), reads: vec![], writes: vec![], encoded: all_code[term_start..term_end].to_vec() });
        }
        global_pos += 2;
        blocks.push(AllocatedBlock { label: block.label.clone(), instructions: instrs, code_offset: block_offset });
    }

    // Trailing epilogue
    let epilogue_start = all_code.len();
    all_code.extend(emit_epilogue_bytes(frame_size, &callee_saved_gprs));
    let epilogue_end = all_code.len();

    if let Some(first_block) = blocks.first_mut() {
        if has_arg_shuffle {
            first_block.instructions.insert(0, AllocatedInstruction { opcode: "arg_shuffle".to_string(), reads: vec![], writes: vec![], encoded: all_code[arg_shuffle_start..arg_shuffle_end].to_vec() });
        }
        first_block.instructions.insert(0, prologue_instr);
    }
    if let Some(last_block) = blocks.last_mut() {
        last_block.instructions.push(AllocatedInstruction { opcode: "epilogue_trailing".to_string(), reads: vec![], writes: vec![], encoded: all_code[epilogue_start..epilogue_end].to_vec() });
    }

    // Resolve branch fixups — MIPS branch offset is (target - PC - 4) >> 2
    for fixup in &fixups {
        if let Some(&target_offset) = label_offsets.get(&fixup.target) {
            let rel = target_offset as i32 - fixup.offset as i32 - 4;
            let imm = ((rel >> 2) as u32) & 0xFFFF;
            let instr_bytes = u32::from_le_bytes([all_code[fixup.offset], all_code[fixup.offset+1], all_code[fixup.offset+2], all_code[fixup.offset+3]]);
            let patched = (instr_bytes & 0xFFFF0000) | imm;
            let bytes = patched.to_le_bytes();
            all_code[fixup.offset..fixup.offset+4].copy_from_slice(&bytes);
        }
    }

    // Re-slice
    let mut offset = 0usize;
    for block in &mut blocks {
        block.code_offset = offset;
        for instr in &mut block.instructions {
            let len = instr.encoded.len();
            if len > 0 && offset + len <= all_code.len() {
                instr.encoded = all_code[offset..offset + len].to_vec();
            }
            offset += len;
        }
    }

    let callee_saved_phys: Vec<PhysicalReg> = callee_saved_gprs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect();
    Ok(AllocatedFunction { name: func.name.clone(), blocks, frame_size: frame_size as usize, callee_saved: callee_saved_phys, spill_slots: alloc.total_spill_slots as usize, code_size: all_code.len(), relocations, wasm_func_type: None, wasm_locals: None })
}

fn preg_to_gpr(preg: &PhysicalReg) -> Option<Gpr> {
    if preg.class != crate::backend::RegClass::Gpr { return None; }
    // MIPS Gpr enum has explicit discriminants 0-31, so transmute is safe.
    match preg.index {
        0 => Some(Gpr::Zero), 1 => Some(Gpr::At), 2 => Some(Gpr::V0), 3 => Some(Gpr::V1),
        4 => Some(Gpr::A0), 5 => Some(Gpr::A1), 6 => Some(Gpr::A2), 7 => Some(Gpr::A3),
        8 => Some(Gpr::T0), 9 => Some(Gpr::T1), 10 => Some(Gpr::T2), 11 => Some(Gpr::T3),
        12 => Some(Gpr::T4), 13 => Some(Gpr::T5), 14 => Some(Gpr::T6), 15 => Some(Gpr::T7),
        16 => Some(Gpr::S0), 17 => Some(Gpr::S1), 18 => Some(Gpr::S2), 19 => Some(Gpr::S3),
        20 => Some(Gpr::S4), 21 => Some(Gpr::S5), 22 => Some(Gpr::S6), 23 => Some(Gpr::S7),
        24 => Some(Gpr::T8), 25 => Some(Gpr::T9), 26 => Some(Gpr::K0), 27 => Some(Gpr::K1),
        28 => Some(Gpr::Gp), 29 => Some(Gpr::Sp), 30 => Some(Gpr::Fp), 31 => Some(Gpr::Ra),
        _ => None,
    }
}

fn resolve_value(val: &IRValue, alloc: &RegAllocResult) -> ResolvedVal {
    match val {
        IRValue::Register(vreg_id) => {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(gpr) = preg_to_gpr(preg) { return ResolvedVal::Reg(gpr); }
            }
            ResolvedVal::Reg(Gpr::V0)
        }
        IRValue::Immediate(imm) => ResolvedVal::Imm(*imm),
        IRValue::Address(addr) => ResolvedVal::Imm(*addr as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Gpr::V0),
    }
}

fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Gpr {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            let scratch = Gpr::At;
            emit_load_imm(code, scratch, imm);
            scratch
        }
    }
}

fn emit_load_imm(code: &mut Vec<u8>, rd: Gpr, imm: i64) {
    if imm >= -32768 && imm <= 32767 {
        code.extend_from_slice(&Instruction::Daddiu { rt: rd, rs: Gpr::Zero, imm: imm as i32 }.encode());
        return;
    }
    let val = imm as i32;
    let upper = (val + 0x800) >> 12;
    let lower = val - (upper << 12);
    code.extend_from_slice(&Instruction::Lui { rt: rd, imm: upper as u32 }.encode());
    if lower != 0 {
        code.extend_from_slice(&Instruction::Daddiu { rt: rd, rs: rd, imm: lower }.encode());
    }
}

fn emit_spill_code(code: &mut Vec<u8>, spill: &GenericSpillCode) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                code.extend_from_slice(&Instruction::Sd { rt: gpr, base: Gpr::Fp, offset: slot.offset }.encode());
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                code.extend_from_slice(&Instruction::Ld { rt: gpr, base: Gpr::Fp, offset: slot.offset }.encode());
            }
        }
    }
}

fn emit_epilogue_bytes(frame_size: i32, callee_saved_gprs: &[Gpr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 + callee_saved_gprs.len() * 4);
    // daddiu sp, fp, -frame_size (restore SP from FP)
    out.extend_from_slice(&Instruction::Daddiu { rt: Gpr::Sp, rs: Gpr::Fp, imm: -frame_size }.encode());
    let mut cs_off = frame_size - 24;
    let mut saved: Vec<(Gpr, i32)> = Vec::new();
    for &g in callee_saved_gprs { saved.push((g, cs_off)); cs_off -= 8; }
    for (g, off) in saved.iter().rev() {
        out.extend_from_slice(&Instruction::Ld { rt: *g, base: Gpr::Sp, offset: *off }.encode());
    }
    out.extend_from_slice(&Instruction::Ld { rt: Gpr::Ra, base: Gpr::Sp, offset: frame_size - 8 }.encode());
    out.extend_from_slice(&Instruction::Ld { rt: Gpr::Fp, base: Gpr::Sp, offset: frame_size - 16 }.encode());
    out.extend_from_slice(&Instruction::Daddiu { rt: Gpr::Sp, rs: Gpr::Sp, imm: frame_size }.encode());
    out.extend_from_slice(&Instruction::Jr { rs: Gpr::Ra }.encode());
    out.extend_from_slice(&Instruction::Nop.encode()); // delay slot
    out
}

fn emit_instruction(
    code: &mut Vec<u8>,
    instr: &IRInstr,
    alloc: &RegAllocResult,
    fixups: &mut Vec<BranchFixup>,
    relocations: &mut Vec<RelocationEntry>,
) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    let mut reads = Vec::new();
    let mut writes = Vec::new();

    let opcode = match instr {
        IRInstr::Add { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend_from_slice(&Instruction::Daddu { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode());
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    if imm >= -32768 && imm <= 32767 {
                        code.extend_from_slice(&Instruction::Daddiu { rt: dst_reg, rs: lhs_reg, imm: imm as i32 }.encode());
                    } else {
                        let s = load_to_reg(rhs, alloc, code);
                        code.extend_from_slice(&Instruction::Daddu { rd: dst_reg, rs: lhs_reg, rt: s }.encode());
                    }
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "add".to_string()
        }

        IRInstr::Sub { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            code.extend_from_slice(&Instruction::Dsubu { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode());
            reads.push(phys(lhs_reg)); reads.push(phys(rhs_reg)); writes.push(phys(dst_reg));
            "sub".to_string()
        }

        IRInstr::Mul { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            code.extend_from_slice(&Instruction::Dmult { rs: lhs_reg, rt: rhs_reg }.encode());
            code.extend_from_slice(&Instruction::Mflo { rd: dst_reg }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // mflo delay
            reads.push(phys(lhs_reg)); reads.push(phys(rhs_reg)); writes.push(phys(dst_reg));
            "mul".to_string()
        }

        // ── Div (standalone, from scg_to_ir) ──
        // Treated as unsigned division (UDiv). VUMA uses u32 types for most
        // arithmetic; FP types are redirected to the BinOp FP fallback.
        IRInstr::Div { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            // ddivu; mflo; nop (delay slot)
            code.extend_from_slice(&Instruction::Ddivu { rs: lhs_reg, rt: rhs_reg }.encode());
            code.extend_from_slice(&Instruction::Mflo { rd: dst_reg }.encode());
            code.extend_from_slice(&Instruction::Nop.encode());
            reads.push(phys(lhs_reg)); reads.push(phys(rhs_reg)); writes.push(phys(dst_reg));
            "div".to_string()
        }

        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            // Use immediate form for ops that support it (Add, Sub, And, Or,
            // Xor, Shl, ShrL, ShrA) when rhs is a small immediate. This
            // avoids loading the immediate into At ($1) scratch which would
            // clobber lhs if lhs was also an immediate loaded into At.
            let rhs_val = resolve_value(rhs, alloc);
            let use_imm = match (&op, &rhs_val) {
                // Daddiu imm is 16-bit signed: -32768..=32767
                (BinOpKind::Add, ResolvedVal::Imm(imm)) => *imm >= -32768 && *imm <= 32767,
                // Sub uses Daddiu with negated imm: need -imm in [-32768, 32767]
                (BinOpKind::Sub, ResolvedVal::Imm(imm)) => *imm >= -32767 && *imm <= 32768,
                // Andi/Ori/Xori imm is 16-bit unsigned: 0..=65535
                (BinOpKind::And, ResolvedVal::Imm(imm)) => *imm >= 0 && *imm <= 0xFFFF,
                (BinOpKind::Or,  ResolvedVal::Imm(imm)) => *imm >= 0 && *imm <= 0xFFFF,
                (BinOpKind::Xor, ResolvedVal::Imm(imm)) => *imm >= 0 && *imm <= 0xFFFF,
                // Dsll/Dsrl/Dsra sa is 6-bit: 0..=63
                (BinOpKind::Shl,  ResolvedVal::Imm(imm)) => *imm >= 0 && *imm <= 63,
                (BinOpKind::ShrL, ResolvedVal::Imm(imm)) => *imm >= 0 && *imm <= 63,
                (BinOpKind::ShrA, ResolvedVal::Imm(imm)) => *imm >= 0 && *imm <= 63,
                _ => false,
            };
            let rhs_reg = if use_imm { Gpr::Zero } else { load_to_reg(rhs, alloc, code) };
            match op {
                BinOpKind::SDiv => { code.extend_from_slice(&Instruction::Ddiv { rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Mflo { rd: dst_reg }.encode()); code.extend_from_slice(&Instruction::Nop.encode()); }
                BinOpKind::UDiv => { code.extend_from_slice(&Instruction::Ddivu { rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Mflo { rd: dst_reg }.encode()); code.extend_from_slice(&Instruction::Nop.encode()); }
                BinOpKind::SRem => { code.extend_from_slice(&Instruction::Ddiv { rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Mfhi { rd: dst_reg }.encode()); code.extend_from_slice(&Instruction::Nop.encode()); }
                BinOpKind::URem => { code.extend_from_slice(&Instruction::Ddivu { rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Mfhi { rd: dst_reg }.encode()); code.extend_from_slice(&Instruction::Nop.encode()); }
                BinOpKind::And => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Andi { rt: dst_reg, rs: lhs_reg, imm: imm as u32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::And { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); }
                }
                BinOpKind::Or => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Ori { rt: dst_reg, rs: lhs_reg, imm: imm as u32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); }
                }
                BinOpKind::Xor => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Xori { rt: dst_reg, rs: lhs_reg, imm: imm as u32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::Xor { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); }
                }
                BinOpKind::Shl => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Dsll { rd: dst_reg, rt: lhs_reg, sa: imm as u32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::Dsllv { rd: dst_reg, rt: lhs_reg, rs: rhs_reg }.encode()); }
                }
                BinOpKind::ShrL => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Dsrl { rd: dst_reg, rt: lhs_reg, sa: imm as u32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::Dsrlv { rd: dst_reg, rt: lhs_reg, rs: rhs_reg }.encode()); }
                }
                BinOpKind::ShrA => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Dsra { rd: dst_reg, rt: lhs_reg, sa: imm as u32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::Dsrav { rd: dst_reg, rt: lhs_reg, rs: rhs_reg }.encode()); }
                }
                BinOpKind::Add => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Daddiu { rt: dst_reg, rs: lhs_reg, imm: imm as i32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::Daddu { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); }
                }
                BinOpKind::Sub => {
                    if use_imm { if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Daddiu { rt: dst_reg, rs: lhs_reg, imm: (-imm) as i32 }.encode()); } }
                    else { code.extend_from_slice(&Instruction::Dsubu { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); }
                }
                BinOpKind::Mul => { code.extend_from_slice(&Instruction::Dmult { rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Mflo { rd: dst_reg }.encode()); code.extend_from_slice(&Instruction::Nop.encode()); }
                _ => code.extend_from_slice(&Instruction::Daddu { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()),
            }
            reads.push(phys(lhs_reg));
            if !use_imm { reads.push(phys(rhs_reg)); }
            writes.push(phys(dst_reg));
            "binop".to_string()
        }

        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let src_reg = load_to_reg(operand, alloc, code);
            match op {
                UnaryOpKind::Neg => { code.extend_from_slice(&Instruction::Dsubu { rd: dst_reg, rs: Gpr::Zero, rt: src_reg }.encode()); }
                UnaryOpKind::Not => { code.extend_from_slice(&Instruction::Nor { rd: dst_reg, rs: src_reg, rt: Gpr::Zero }.encode()); }
                _ => { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: Gpr::Zero, rt: Gpr::Zero }.encode()); }
            }
            reads.push(phys(src_reg)); writes.push(phys(dst_reg));
            "unaryop".to_string()
        }

        IRInstr::Load { dst, addr, offset, ty } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let off = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => { if matches!(ty, IRType::I8) { code.extend_from_slice(&Instruction::Lb { rt: dst_reg, base: base_reg, offset: off }.encode()); } else { code.extend_from_slice(&Instruction::Lbu { rt: dst_reg, base: base_reg, offset: off }.encode()); } }
                IRType::U16 | IRType::I16 => { if matches!(ty, IRType::I16) { code.extend_from_slice(&Instruction::Lh { rt: dst_reg, base: base_reg, offset: off }.encode()); } else { code.extend_from_slice(&Instruction::Lhu { rt: dst_reg, base: base_reg, offset: off }.encode()); } }
                IRType::U32 | IRType::I32 => { if matches!(ty, IRType::I32) { code.extend_from_slice(&Instruction::Lw { rt: dst_reg, base: base_reg, offset: off }.encode()); } else { code.extend_from_slice(&Instruction::Lwu { rt: dst_reg, base: base_reg, offset: off }.encode()); } }
                _ => code.extend_from_slice(&Instruction::Ld { rt: dst_reg, base: base_reg, offset: off }.encode()),
            }
            reads.push(phys(base_reg)); writes.push(phys(dst_reg));
            "load".to_string()
        }

        IRInstr::Store { value, addr, offset, ty } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let off = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => code.extend_from_slice(&Instruction::Sb { rt: val_reg, base: base_reg, offset: off }.encode()),
                IRType::U16 | IRType::I16 => code.extend_from_slice(&Instruction::Sh { rt: val_reg, base: base_reg, offset: off }.encode()),
                IRType::U32 | IRType::I32 => code.extend_from_slice(&Instruction::Sw { rt: val_reg, base: base_reg, offset: off }.encode()),
                _ => code.extend_from_slice(&Instruction::Sd { rt: val_reg, base: base_reg, offset: off }.encode()),
            }
            reads.push(phys(val_reg)); reads.push(phys(base_reg));
            "store".to_string()
        }

        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            match kind {
                CmpKind::Eq => { code.extend_from_slice(&Instruction::Xor { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Sltiu { rt: dst_reg, rs: dst_reg, imm: 1 }.encode()); }
                CmpKind::Ne => { code.extend_from_slice(&Instruction::Xor { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Sltu { rd: dst_reg, rs: Gpr::Zero, rt: dst_reg }.encode()); }
                CmpKind::SLt => code.extend_from_slice(&Instruction::Slt { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()),
                CmpKind::SLe => { code.extend_from_slice(&Instruction::Slt { rd: dst_reg, rs: rhs_reg, rt: lhs_reg }.encode()); code.extend_from_slice(&Instruction::Xori { rt: dst_reg, rs: dst_reg, imm: 1 }.encode()); }
                CmpKind::SGt => { code.extend_from_slice(&Instruction::Slt { rd: dst_reg, rs: rhs_reg, rt: lhs_reg }.encode()); }
                CmpKind::SGe => { code.extend_from_slice(&Instruction::Slt { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Xori { rt: dst_reg, rs: dst_reg, imm: 1 }.encode()); }
                CmpKind::ULt => code.extend_from_slice(&Instruction::Sltu { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()),
                CmpKind::ULe => { code.extend_from_slice(&Instruction::Sltu { rd: dst_reg, rs: rhs_reg, rt: lhs_reg }.encode()); code.extend_from_slice(&Instruction::Xori { rt: dst_reg, rs: dst_reg, imm: 1 }.encode()); }
                CmpKind::UGt => { code.extend_from_slice(&Instruction::Sltu { rd: dst_reg, rs: rhs_reg, rt: lhs_reg }.encode()); }
                CmpKind::UGe => { code.extend_from_slice(&Instruction::Sltu { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode()); code.extend_from_slice(&Instruction::Xori { rt: dst_reg, rs: dst_reg, imm: 1 }.encode()); }
            }
            reads.push(phys(lhs_reg)); reads.push(phys(rhs_reg)); writes.push(phys(dst_reg));
            "cmp".to_string()
        }

        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: false_reg, rt: Gpr::Zero }.encode()); // mov dst, false
            code.extend_from_slice(&Instruction::Movn { rd: dst_reg, rs: true_reg, rt: cond_reg }.encode()); // if cond!=0, dst=true
            reads.push(phys(cond_reg)); reads.push(phys(false_reg)); reads.push(phys(true_reg)); writes.push(phys(dst_reg));
            "select".to_string()
        }

        IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: false_reg, rt: Gpr::Zero }.encode());
            code.extend_from_slice(&Instruction::Movn { rd: dst_reg, rs: true_reg, rt: cond_reg }.encode());
            reads.push(phys(cond_reg)); reads.push(phys(false_reg)); reads.push(phys(true_reg)); writes.push(phys(dst_reg));
            "ct_select".to_string()
        }

        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&Instruction::Xor { rd: dst_reg, rs: lhs_reg, rt: rhs_reg }.encode());
            code.extend_from_slice(&Instruction::Sltiu { rt: dst_reg, rs: dst_reg, imm: 1 }.encode());
            reads.push(phys(lhs_reg)); reads.push(phys(rhs_reg)); writes.push(phys(dst_reg));
            "ct_eq".to_string()
        }

        IRInstr::Cast { kind, dst, src, from_ty, to_ty, .. } => {
            let src_reg = load_to_reg(src, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            match kind {
                CastKind::ZExt => { match from_ty { Some(IRType::U8)|Some(IRType::I8) => code.extend_from_slice(&Instruction::Andi { rt: dst_reg, rs: src_reg, imm: 0xFF }.encode()), Some(IRType::U16)|Some(IRType::I16) => code.extend_from_slice(&Instruction::Andi { rt: dst_reg, rs: src_reg, imm: 0xFFFF }.encode()), _ => { if src_reg != dst_reg { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: src_reg, rt: Gpr::Zero }.encode()); } } } }
                CastKind::SExt => { match from_ty { Some(IRType::I8)|Some(IRType::U8) => { code.extend_from_slice(&Instruction::Dsll { rd: dst_reg, rt: src_reg, sa: 56 }.encode()); code.extend_from_slice(&Instruction::Dsra { rd: dst_reg, rt: dst_reg, sa: 56 }.encode()); } Some(IRType::I16)|Some(IRType::U16) => { code.extend_from_slice(&Instruction::Dsll { rd: dst_reg, rt: src_reg, sa: 48 }.encode()); code.extend_from_slice(&Instruction::Dsra { rd: dst_reg, rt: dst_reg, sa: 48 }.encode()); } Some(IRType::I32)|Some(IRType::U32) => { code.extend_from_slice(&Instruction::Dsll { rd: dst_reg, rt: src_reg, sa: 32 }.encode()); code.extend_from_slice(&Instruction::Dsra { rd: dst_reg, rt: dst_reg, sa: 32 }.encode()); } _ => { if src_reg != dst_reg { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: src_reg, rt: Gpr::Zero }.encode()); } } } }
                CastKind::Trunc => { if src_reg != dst_reg { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: src_reg, rt: Gpr::Zero }.encode()); } else if let Some(tt) = to_ty { match tt { IRType::U8|IRType::I8 => code.extend_from_slice(&Instruction::Andi { rt: dst_reg, rs: dst_reg, imm: 0xFF }.encode()), IRType::U16|IRType::I16 => code.extend_from_slice(&Instruction::Andi { rt: dst_reg, rs: dst_reg, imm: 0xFFFF }.encode()), IRType::U32|IRType::I32 => { code.extend_from_slice(&Instruction::Dsll { rd: dst_reg, rt: dst_reg, sa: 32 }.encode()); code.extend_from_slice(&Instruction::Dsrl { rd: dst_reg, rt: dst_reg, sa: 32 }.encode()); } _ => {} } } }
                _ => { if src_reg != dst_reg { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: src_reg, rt: Gpr::Zero }.encode()); } }
            }
            reads.push(phys(src_reg)); writes.push(phys(dst_reg));
            "cast".to_string()
        }

        IRInstr::Alloc { dst, size, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let aligned = ((*size as i32 + 15) & !15) as i32;
            code.extend_from_slice(&Instruction::Daddiu { rt: Gpr::Sp, rs: Gpr::Sp, imm: -aligned }.encode());
            code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: Gpr::Sp, rt: Gpr::Zero }.encode());
            writes.push(phys(dst_reg));
            "alloc".to_string()
        }

        IRInstr::Free { ptr, .. } => { let _ = load_to_reg(ptr, alloc, code); code.extend_from_slice(&Instruction::Nop.encode()); "free".to_string() }

        IRInstr::GetAddress { dst, name: _ } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&Instruction::Nop.encode()); // TODO: proper GOT/relocation
            writes.push(phys(dst_reg));
            "getaddr".to_string()
        }

        IRInstr::Offset { dst, base, offset, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(base, alloc, code);
            match resolve_value(offset, alloc) {
                ResolvedVal::Imm(imm) => { if imm >= -32768 && imm <= 32767 { code.extend_from_slice(&Instruction::Daddiu { rt: dst_reg, rs: base_reg, imm: imm as i32 }.encode()); } else { let s = load_to_reg(offset, alloc, code); code.extend_from_slice(&Instruction::Daddu { rd: dst_reg, rs: base_reg, rt: s }.encode()); } }
                ResolvedVal::Reg(off_reg) => { code.extend_from_slice(&Instruction::Daddu { rd: dst_reg, rs: base_reg, rt: off_reg }.encode()); reads.push(phys(off_reg)); }
            }
            reads.push(phys(base_reg)); writes.push(phys(dst_reg));
            "offset".to_string()
        }

        IRInstr::Phi { dst, .. } => { let dst_reg = load_to_reg(dst, alloc, code); code.extend_from_slice(&Instruction::Nop.encode()); writes.push(phys(dst_reg)); "phi".to_string() }

        IRInstr::Ret { values } => { if let Some(first) = values.first() { let ret_reg = load_to_reg(first, alloc, code); if ret_reg != Gpr::V0 { code.extend_from_slice(&Instruction::Or { rd: Gpr::V0, rs: ret_reg, rt: Gpr::Zero }.encode()); } } code.extend_from_slice(&Instruction::Nop.encode()); "ret".to_string() }

        IRInstr::Branch { target } => {
            let offset_pos = code.len();
            code.extend_from_slice(&Instruction::Beq { rs: Gpr::Zero, rt: Gpr::Zero, offset: 0 }.encode()); // b 0 (always taken)
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            fixups.push(BranchFixup { offset: offset_pos, target: target.clone() });
            "branch".to_string()
        }

        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let offset_pos1 = code.len();
            code.extend_from_slice(&Instruction::Bne { rs: cond_reg, rt: Gpr::Zero, offset: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            fixups.push(BranchFixup { offset: offset_pos1, target: true_target.clone() });
            let offset_pos2 = code.len();
            code.extend_from_slice(&Instruction::Beq { rs: Gpr::Zero, rt: Gpr::Zero, offset: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            fixups.push(BranchFixup { offset: offset_pos2, target: false_target.clone() });
            reads.push(phys(cond_reg));
            "cond_branch".to_string()
        }

        IRInstr::Syscall { nr, args, dst } => {
            let native_nr = crate::syscall_abi::translate_or_warn(crate::backend::BackendKind::Mips64, *nr);
            code.extend_from_slice(&Instruction::Daddiu { rt: Gpr::V0, rs: Gpr::Zero, imm: native_nr as i32 }.encode());
            let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::T0, Gpr::T1];
            for (i, arg) in args.iter().enumerate().take(6) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] { code.extend_from_slice(&Instruction::Or { rd: arg_regs[i], rs: arg_reg, rt: Gpr::Zero }.encode()); }
            }
            code.extend_from_slice(&Instruction::Syscall { code: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            if let Some(dst_val) = dst { let dst_reg = load_to_reg(dst_val, alloc, code); if dst_reg != Gpr::V0 { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: Gpr::V0, rt: Gpr::Zero }.encode()); } writes.push(phys(dst_reg)); }
            "syscall".to_string()
        }

        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3];
            for (i, arg) in args.iter().enumerate().take(4) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] { code.extend_from_slice(&Instruction::Or { rd: arg_regs[i], rs: arg_reg, rt: Gpr::Zero }.encode()); }
            }
            let offset_pos = code.len();
            code.extend_from_slice(&Instruction::Jal { target: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            relocations.push(RelocationEntry { offset: offset_pos as u64, symbol: fname.clone(), reloc_type: "R_MIPS_26".to_string() });
            if let Some(dst_val) = dst { let dst_reg = load_to_reg(dst_val, alloc, code); if dst_reg != Gpr::V0 { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs: Gpr::V0, rt: Gpr::Zero }.encode()); } writes.push(phys(dst_reg)); }
            if *is_extern { "call_extern".to_string() } else { "call".to_string() }
        }

        IRInstr::AtomicLoad { dst, addr, .. } => { let dst_reg = load_to_reg(dst, alloc, code); let base_reg = load_to_reg(addr, alloc, code); code.extend_from_slice(&Instruction::Ld { rt: dst_reg, base: base_reg, offset: 0 }.encode()); reads.push(phys(base_reg)); writes.push(phys(dst_reg)); "atomic_load".to_string() }
        IRInstr::AtomicStore { value, addr, .. } => { let val_reg = load_to_reg(value, alloc, code); let base_reg = load_to_reg(addr, alloc, code); code.extend_from_slice(&Instruction::Sd { rt: val_reg, base: base_reg, offset: 0 }.encode()); reads.push(phys(val_reg)); reads.push(phys(base_reg)); "atomic_store".to_string() }
        IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
            let expected_reg = load_to_reg(expected, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let new_reg = load_to_reg(desired, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&Instruction::Lld { rt: dst_reg, base: base_reg, offset: 0 }.encode());
            code.extend_from_slice(&Instruction::Bne { rs: dst_reg, rt: expected_reg, offset: 12 }.encode()); // skip sc
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            code.extend_from_slice(&Instruction::Scd { rt: new_reg, base: base_reg, offset: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            reads.push(phys(expected_reg)); reads.push(phys(base_reg)); reads.push(phys(new_reg)); writes.push(phys(dst_reg));
            "atomic_cas".to_string()
        }

        _ => { code.extend_from_slice(&Instruction::Nop.encode()); "unhandled".to_string() }
    };
    Ok((opcode, reads, writes))
}

fn emit_terminator(code: &mut Vec<u8>, term: &IRTerminator, alloc: &RegAllocResult, frame_size: i32, callee_saved_gprs: &[Gpr], fixups: &mut Vec<BranchFixup>) {
    match term {
        IRTerminator::Jump(label) => {
            let offset_pos = code.len();
            code.extend_from_slice(&Instruction::Beq { rs: Gpr::Zero, rt: Gpr::Zero, offset: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            fixups.push(BranchFixup { offset: offset_pos, target: label.clone() });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let offset_pos1 = code.len();
            code.extend_from_slice(&Instruction::Bne { rs: cond_reg, rt: Gpr::Zero, offset: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            fixups.push(BranchFixup { offset: offset_pos1, target: true_block.clone() });
            let offset_pos2 = code.len();
            code.extend_from_slice(&Instruction::Beq { rs: Gpr::Zero, rt: Gpr::Zero, offset: 0 }.encode());
            code.extend_from_slice(&Instruction::Nop.encode()); // delay slot
            fixups.push(BranchFixup { offset: offset_pos2, target: false_block.clone() });
        }
        IRTerminator::Return(vals) => {
            if let Some(first) = vals.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::V0 { code.extend_from_slice(&Instruction::Or { rd: Gpr::V0, rs: ret_reg, rt: Gpr::Zero }.encode()); }
            }
            code.extend(emit_epilogue_bytes(frame_size, callee_saved_gprs));
        }
        IRTerminator::Unreachable => { code.extend_from_slice(&Instruction::Break { code: 0 }.encode()); code.extend_from_slice(&Instruction::Nop.encode()); }
        _ => { code.extend_from_slice(&Instruction::Nop.encode()); }
    }
}

fn phys(g: Gpr) -> PhysicalReg { PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32) }

fn emit_fp_fallback(instr: &IRInstr) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed { isa: "mips64", reason: format!("FP not yet supported: {:?}", instr) })
}
