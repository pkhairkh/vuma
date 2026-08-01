//! Full register-based instruction selection for loongarch64.
//!
//! LoongArch is very similar to RISC-V (fixed 32-bit, 3-operand, no delay slots).
//! This emitter mirrors the riscv64 reg_isel.rs template with LoongArch-specific
//! instruction names and field names.

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CastKind, CmpKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::loongarch64::*;

enum ResolvedVal { Reg(Gpr), Imm(i64) }
struct BranchFixup { offset: usize, target: String, is_branch16: bool }

pub fn emit_function_regalloc_full(func: &IRFunction, alloc: &RegAllocResult) -> Result<AllocatedFunction, BackendError> {
    let callee_saved_gprs: Vec<Gpr> = alloc.used_callee_saved.iter()
        .filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::Fp && *g != Gpr::Sp && *g != Gpr::R0 && *g != Gpr::Ra)
        .collect();
    let cs_count = 2 + callee_saved_gprs.len();
    let callee_saved_size = cs_count * 8;
    let spill_size = alloc.total_spill_slots as usize * 8;
    let frame_size = ((callee_saved_size + spill_size + 15) & !15) as i32;

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Prologue
    let prologue_start = all_code.len();
    all_code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -frame_size }.encode());
    all_code.extend_from_slice(&Instruction::StD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: frame_size - 8 }.encode());
    all_code.extend_from_slice(&Instruction::StD { rd: Gpr::Fp, rj: Gpr::Sp, imm12: frame_size - 16 }.encode());
    all_code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Fp, rj: Gpr::Sp, imm12: frame_size }.encode());
    let mut cs_off = frame_size - 24;
    for &g in &callee_saved_gprs {
        if cs_off < 0 { break; }
        all_code.extend_from_slice(&Instruction::StD { rd: g, rj: Gpr::Sp, imm12: cs_off }.encode());
        cs_off -= 8;
    }
    let prologue_end = all_code.len();
    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(), reads: vec![],
        writes: callee_saved_gprs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect(),
        encoded: all_code[prologue_start..prologue_end].to_vec(),
    };

    // Argument shuffle
    let arg_shuffle_start = all_code.len();
    let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new();
    for (i, param) in func.params.iter().enumerate() {
        if i >= 8 { break; }
        if let IRValue::Register(vid) = param {
            let root = alloc.coalesced_map.get(vid).unwrap_or(vid);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(dst) = preg_to_gpr(preg) {
                    let src = arg_regs[i];
                    if dst != src { pending.push((src, dst)); }
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
            for (j, (_, od)) in pending.iter().enumerate() { if i != j && *od == src { conflict = true; break; } }
            if !conflict {
                all_code.extend_from_slice(&Instruction::Or { rd: dst, rj: src, rk: Gpr::R0 }.encode());
                pending.remove(i); progress = true;
            } else { i += 1; }
        }
    }
    for (src, dst) in pending {
        all_code.extend_from_slice(&Instruction::Or { rd: Gpr::T8, rj: src, rk: Gpr::R0 }.encode());
        all_code.extend_from_slice(&Instruction::Or { rd: dst, rj: Gpr::T8, rk: Gpr::R0 }.encode());
    }
    let arg_shuffle_end = all_code.len();
    let has_arg_shuffle = arg_shuffle_end > arg_shuffle_start;

    // Body
    let mut global_pos: u32 = 0;
    for block in &func.blocks {
        let block_offset = all_code.len();
        label_offsets.insert(block.label.clone(), block_offset);
        let mut instrs: Vec<AllocatedInstruction> = Vec::new();
        for instr in &block.instructions {
            if let Some(spills) = alloc.spill_code.get(&global_pos) {
                for spill in spills {
                    let s = all_code.len();
                    emit_spill_code(&mut all_code, spill);
                    if all_code.len() > s { instrs.push(AllocatedInstruction { opcode: match spill { GenericSpillCode::Spill { .. } => "spill", _ => "reload" }.to_string(), reads: vec![], writes: vec![], encoded: all_code[s..].to_vec() }); }
                }
            }
            let s = all_code.len();
            let (op, r, w) = emit_instruction(&mut all_code, instr, alloc, &mut fixups, &mut relocations)?;
            let e = all_code.len();
            if e > s { instrs.push(AllocatedInstruction { opcode: op, reads: r, writes: w, encoded: all_code[s..e].to_vec() }); }
            global_pos += 2;
        }
        if let Some(spills) = alloc.spill_code.get(&global_pos) {
            for spill in spills {
                let s = all_code.len();
                emit_spill_code(&mut all_code, spill);
                if all_code.len() > s { instrs.push(AllocatedInstruction { opcode: match spill { GenericSpillCode::Spill { .. } => "spill", _ => "reload" }.to_string(), reads: vec![], writes: vec![], encoded: all_code[s..].to_vec() }); }
            }
        }
        let s = all_code.len();
        emit_terminator(&mut all_code, &block.terminator, alloc, frame_size, &callee_saved_gprs, &mut fixups);
        let e = all_code.len();
        if e > s { instrs.push(AllocatedInstruction { opcode: "terminator".to_string(), reads: vec![], writes: vec![], encoded: all_code[s..e].to_vec() }); }
        global_pos += 2;
        blocks.push(AllocatedBlock { label: block.label.clone(), instructions: instrs, code_offset: block_offset });
    }

    let epilogue_start = all_code.len();
    all_code.extend(emit_epilogue_bytes(frame_size, &callee_saved_gprs));
    let epilogue_end = all_code.len();

    if let Some(fb) = blocks.first_mut() {
        if has_arg_shuffle { fb.instructions.insert(0, AllocatedInstruction { opcode: "arg_shuffle".to_string(), reads: vec![], writes: vec![], encoded: all_code[arg_shuffle_start..arg_shuffle_end].to_vec() }); }
        fb.instructions.insert(0, prologue_instr);
    }
    if let Some(lb) = blocks.last_mut() {
        lb.instructions.push(AllocatedInstruction { opcode: "epilogue_trailing".to_string(), reads: vec![], writes: vec![], encoded: all_code[epilogue_start..epilogue_end].to_vec() });
    }

    // Resolve branch fixups
    for fixup in &fixups {
        if let Some(&target) = label_offsets.get(&fixup.target) {
            let rel = target as i32 - fixup.offset as i32;
            let instr = u32::from_le_bytes([all_code[fixup.offset], all_code[fixup.offset+1], all_code[fixup.offset+2], all_code[fixup.offset+3]]);
            if fixup.is_branch16 {
                // 2RI16 format: offs16 is in bits [15:0], but the actual encoding
                // splits it: offs[15:0] mapped to bits [25:10]
                let offs16 = (rel >> 2) as u32 & 0xFFFF;
                let patched = (instr & 0xFC0003FF) | ((offs16 & 0xFFFF) << 10);
                all_code[fixup.offset..fixup.offset+4].copy_from_slice(&patched.to_le_bytes());
            } else {
                // I26 format (B/Bl): LoongArch splits the 26-bit offset as:
                // bits [25:10] = imm[15:0] (LOWER 16 bits)
                // bits [9:0] = imm[25:16] (UPPER 10 bits)
                let offs26 = (rel >> 2) as u32 & 0x3FFFFFF;
                let low16 = offs26 & 0xFFFF;
                let high10 = (offs26 >> 16) & 0x3FF;
                let patched = (instr & 0xFC000000) | (low16 << 10) | high10;
                all_code[fixup.offset..fixup.offset+4].copy_from_slice(&patched.to_le_bytes());
            }
        }
    }

    // Re-slice
    let mut offset = 0usize;
    for block in &mut blocks {
        block.code_offset = offset;
        for instr in &mut block.instructions {
            let len = instr.encoded.len();
            if len > 0 && offset + len <= all_code.len() { instr.encoded = all_code[offset..offset+len].to_vec(); }
            offset += len;
        }
    }

    let cs_phys: Vec<PhysicalReg> = callee_saved_gprs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect();
    Ok(AllocatedFunction { name: func.name.clone(), blocks, frame_size: frame_size as usize, callee_saved: cs_phys, spill_slots: alloc.total_spill_slots as usize, code_size: all_code.len(), relocations, wasm_func_type: None, wasm_locals: None })
}

fn preg_to_gpr(preg: &PhysicalReg) -> Option<Gpr> {
    if preg.class != crate::backend::RegClass::Gpr { return None; }
    if preg.index > 31 { return None; }
    Some(Gpr::from_encoding(preg.index as u32))
}

fn resolve_value(val: &IRValue, alloc: &RegAllocResult) -> ResolvedVal {
    match val {
        IRValue::Register(vid) => {
            let root = alloc.coalesced_map.get(vid).unwrap_or(vid);
            if let Some(p) = alloc.vreg_to_preg.get(root) { if let Some(g) = preg_to_gpr(p) { return ResolvedVal::Reg(g); } }
            ResolvedVal::Reg(Gpr::A0)
        }
        IRValue::Immediate(i) => ResolvedVal::Imm(*i),
        IRValue::Address(a) => ResolvedVal::Imm(*a as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Gpr::A0),
    }
}

fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Gpr {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => { let s = Gpr::T8; emit_load_imm(code, s, imm); s }
    }
}

fn emit_load_imm(code: &mut Vec<u8>, rd: Gpr, imm: i64) {
    if imm >= -2048 && imm <= 2047 {
        code.extend_from_slice(&Instruction::AddiD { rd, rj: Gpr::R0, imm12: imm as i32 }.encode());
        return;
    }
    let val = imm as i32;
    let upper = (val + 0x800) >> 12;
    let lower = val - (upper << 12);
    code.extend_from_slice(&Instruction::Lu12iW { rd, imm20: upper }.encode());
    if lower != 0 {
        code.extend_from_slice(&Instruction::AddiD { rd, rj: rd, imm12: lower }.encode());
    }
}

fn emit_spill_code(code: &mut Vec<u8>, spill: &GenericSpillCode) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => { if let Some(g) = preg_to_gpr(preg) { code.extend_from_slice(&Instruction::StD { rd: g, rj: Gpr::Fp, imm12: slot.offset }.encode()); } }
        GenericSpillCode::Reload { preg, slot, .. } => { if let Some(g) = preg_to_gpr(preg) { code.extend_from_slice(&Instruction::LdD { rd: g, rj: Gpr::Fp, imm12: slot.offset }.encode()); } }
    }
}

fn emit_epilogue_bytes(frame_size: i32, callee_saved_gprs: &[Gpr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(48 + callee_saved_gprs.len() * 4);
    out.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Fp, imm12: -frame_size }.encode());
    let mut cs_off = frame_size - 24;
    let mut saved: Vec<(Gpr, i32)> = Vec::new();
    for &g in callee_saved_gprs { saved.push((g, cs_off)); cs_off -= 8; }
    for (g, off) in saved.iter().rev() { out.extend_from_slice(&Instruction::LdD { rd: *g, rj: Gpr::Sp, imm12: *off }.encode()); }
    out.extend_from_slice(&Instruction::LdD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: frame_size - 8 }.encode());
    out.extend_from_slice(&Instruction::LdD { rd: Gpr::Fp, rj: Gpr::Sp, imm12: frame_size - 16 }.encode());
    out.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: frame_size }.encode());
    out.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode()); // ret
    out
}

fn emit_instruction(code: &mut Vec<u8>, instr: &IRInstr, alloc: &RegAllocResult, fixups: &mut Vec<BranchFixup>, relocations: &mut Vec<RelocationEntry>) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    let opcode = match instr {
        IRInstr::Add { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let d = load_to_reg(dst, alloc, code); let l = load_to_reg(lhs, alloc, code);
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(r) => { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: r }.encode()); reads.push(phys(r)); }
                ResolvedVal::Imm(i) => { if i >= -2048 && i <= 2047 { code.extend_from_slice(&Instruction::AddiD { rd: d, rj: l, imm12: i as i32 }.encode()); } else { let s = load_to_reg(rhs, alloc, code); code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: s }.encode()); } }
            }
            reads.push(phys(l)); writes.push(phys(d)); "add".to_string()
        }
        IRInstr::Sub { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let d = load_to_reg(dst, alloc, code); let l = load_to_reg(lhs, alloc, code); let r = load_to_reg(rhs, alloc, code);
            code.extend_from_slice(&Instruction::SubD { rd: d, rj: l, rk: r }.encode());
            reads.push(phys(l)); reads.push(phys(r)); writes.push(phys(d)); "sub".to_string()
        }
        IRInstr::Mul { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let d = load_to_reg(dst, alloc, code); let l = load_to_reg(lhs, alloc, code); let r = load_to_reg(rhs, alloc, code);
            code.extend_from_slice(&Instruction::MulD { rd: d, rj: l, rk: r }.encode());
            reads.push(phys(l)); reads.push(phys(r)); writes.push(phys(d)); "mul".to_string()
        }
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let d = load_to_reg(dst, alloc, code); let l = load_to_reg(lhs, alloc, code); let r = load_to_reg(rhs, alloc, code);
            match op {
                BinOpKind::SDiv => code.extend_from_slice(&Instruction::DivD { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::UDiv => code.extend_from_slice(&Instruction::DivDu { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::SRem => code.extend_from_slice(&Instruction::ModD { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::URem => code.extend_from_slice(&Instruction::ModDu { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::And => code.extend_from_slice(&Instruction::And { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::Or => code.extend_from_slice(&Instruction::Or { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::Xor => code.extend_from_slice(&Instruction::Xor { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::Shl => code.extend_from_slice(&Instruction::SllD { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::ShrL => code.extend_from_slice(&Instruction::SrlD { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::ShrA => code.extend_from_slice(&Instruction::SraD { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::Add => code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::Sub => code.extend_from_slice(&Instruction::SubD { rd: d, rj: l, rk: r }.encode()),
                BinOpKind::Mul => code.extend_from_slice(&Instruction::MulD { rd: d, rj: l, rk: r }.encode()),
                _ => code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: r }.encode()),
            }
            reads.push(phys(l)); reads.push(phys(r)); writes.push(phys(d)); "binop".to_string()
        }
        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let d = load_to_reg(dst, alloc, code); let s = load_to_reg(operand, alloc, code);
            match op {
                UnaryOpKind::Neg => code.extend_from_slice(&Instruction::SubD { rd: d, rj: Gpr::R0, rk: s }.encode()),
                UnaryOpKind::Not => code.extend_from_slice(&Instruction::Nor { rd: d, rj: s, rk: Gpr::R0 }.encode()),
                _ => code.extend_from_slice(&Instruction::AddiD { rd: d, rj: Gpr::R0, imm12: 0 }.encode()),
            }
            reads.push(phys(s)); writes.push(phys(d)); "unaryop".to_string()
        }
        IRInstr::Load { dst, addr, offset, ty } => {
            let d = load_to_reg(dst, alloc, code); let b = load_to_reg(addr, alloc, code); let o = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => { if matches!(ty, IRType::I8) { code.extend_from_slice(&Instruction::LdB { rd: d, rj: b, imm12: o }.encode()); } else { code.extend_from_slice(&Instruction::LdBu { rd: d, rj: b, imm12: o }.encode()); } }
                IRType::U16 | IRType::I16 => { if matches!(ty, IRType::I16) { code.extend_from_slice(&Instruction::LdH { rd: d, rj: b, imm12: o }.encode()); } else { code.extend_from_slice(&Instruction::LdHu { rd: d, rj: b, imm12: o }.encode()); } }
                IRType::U32 | IRType::I32 => { if matches!(ty, IRType::I32) { code.extend_from_slice(&Instruction::LdW { rd: d, rj: b, imm12: o }.encode()); } else { code.extend_from_slice(&Instruction::LdWu { rd: d, rj: b, imm12: o }.encode()); } }
                _ => code.extend_from_slice(&Instruction::LdD { rd: d, rj: b, imm12: o }.encode()),
            }
            reads.push(phys(b)); writes.push(phys(d)); "load".to_string()
        }
        IRInstr::Store { value, addr, offset, ty } => {
            let v = load_to_reg(value, alloc, code); let b = load_to_reg(addr, alloc, code); let o = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => code.extend_from_slice(&Instruction::StB { rd: v, rj: b, imm12: o }.encode()),
                IRType::U16 | IRType::I16 => code.extend_from_slice(&Instruction::StH { rd: v, rj: b, imm12: o }.encode()),
                IRType::U32 | IRType::I32 => code.extend_from_slice(&Instruction::StW { rd: v, rj: b, imm12: o }.encode()),
                _ => code.extend_from_slice(&Instruction::StD { rd: v, rj: b, imm12: o }.encode()),
            }
            reads.push(phys(v)); reads.push(phys(b)); "store".to_string()
        }
        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => {
            let l = load_to_reg(lhs, alloc, code); let r = load_to_reg(rhs, alloc, code); let d = load_to_reg(dst, alloc, code);
            match kind {
                CmpKind::Eq => { code.extend_from_slice(&Instruction::Xor { rd: d, rj: l, rk: r }.encode()); code.extend_from_slice(&Instruction::Sltui { rd: d, rj: d, imm12: 1 }.encode()); }
                CmpKind::Ne => { code.extend_from_slice(&Instruction::Xor { rd: d, rj: l, rk: r }.encode()); code.extend_from_slice(&Instruction::Sltu { rd: d, rj: Gpr::R0, rk: d }.encode()); }
                CmpKind::SLt => code.extend_from_slice(&Instruction::Slt { rd: d, rj: l, rk: r }.encode()),
                CmpKind::SLe => { code.extend_from_slice(&Instruction::Slt { rd: d, rj: r, rk: l }.encode()); code.extend_from_slice(&Instruction::Xori { rd: d, rj: d, imm12: 1 }.encode()); }
                CmpKind::SGt => code.extend_from_slice(&Instruction::Slt { rd: d, rj: r, rk: l }.encode()),
                CmpKind::SGe => { code.extend_from_slice(&Instruction::Slt { rd: d, rj: l, rk: r }.encode()); code.extend_from_slice(&Instruction::Xori { rd: d, rj: d, imm12: 1 }.encode()); }
                CmpKind::ULt => code.extend_from_slice(&Instruction::Sltu { rd: d, rj: l, rk: r }.encode()),
                CmpKind::ULe => { code.extend_from_slice(&Instruction::Sltu { rd: d, rj: r, rk: l }.encode()); code.extend_from_slice(&Instruction::Xori { rd: d, rj: d, imm12: 1 }.encode()); }
                CmpKind::UGt => code.extend_from_slice(&Instruction::Sltu { rd: d, rj: r, rk: l }.encode()),
                CmpKind::UGe => { code.extend_from_slice(&Instruction::Sltu { rd: d, rj: l, rk: r }.encode()); code.extend_from_slice(&Instruction::Xori { rd: d, rj: d, imm12: 1 }.encode()); }
            }
            reads.push(phys(l)); reads.push(phys(r)); writes.push(phys(d)); "cmp".to_string()
        }
        IRInstr::Select { dst, cond, true_val, false_val, .. } | IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let c = load_to_reg(cond, alloc, code); let d = load_to_reg(dst, alloc, code);
            let f = load_to_reg(false_val, alloc, code); let t = load_to_reg(true_val, alloc, code);
            code.extend_from_slice(&Instruction::Or { rd: d, rj: f, rk: Gpr::R0 }.encode());
            code.extend_from_slice(&Instruction::Maskeqz { rd: d, rj: d, rk: c }.encode()); // if c==0, clear d
            code.extend_from_slice(&Instruction::Or { rd: Gpr::T8, rj: t, rk: Gpr::R0 }.encode());
            code.extend_from_slice(&Instruction::Masknez { rd: d, rj: d, rk: c }.encode()); // if c!=0, clear d
            code.extend_from_slice(&Instruction::Or { rd: d, rj: d, rk: Gpr::T8 }.encode());
            reads.push(phys(c)); reads.push(phys(f)); reads.push(phys(t)); writes.push(phys(d)); "select".to_string()
        }
        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let l = load_to_reg(lhs, alloc, code); let r = load_to_reg(rhs, alloc, code); let d = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&Instruction::Xor { rd: d, rj: l, rk: r }.encode());
            code.extend_from_slice(&Instruction::Sltui { rd: d, rj: d, imm12: 1 }.encode());
            reads.push(phys(l)); reads.push(phys(r)); writes.push(phys(d)); "ct_eq".to_string()
        }
        IRInstr::Cast { kind, dst, src, from_ty, to_ty, .. } => {
            let s = load_to_reg(src, alloc, code); let d = load_to_reg(dst, alloc, code);
            match kind {
                CastKind::ZExt => { match from_ty { Some(IRType::U8)|Some(IRType::I8) => code.extend_from_slice(&Instruction::Andi { rd: d, rj: s, imm12: 0xFF }.encode()), Some(IRType::U16)|Some(IRType::I16) => code.extend_from_slice(&Instruction::Andi { rd: d, rj: s, imm12: 0xFFFF }.encode()), _ => { if s != d { code.extend_from_slice(&Instruction::Or { rd: d, rj: s, rk: Gpr::R0 }.encode()); } } } }
                CastKind::SExt => { match from_ty { Some(IRType::I8)|Some(IRType::U8) => { code.extend_from_slice(&Instruction::SlliD { rd: d, rj: s, imm8: 56 }.encode()); code.extend_from_slice(&Instruction::SraiD { rd: d, rj: d, imm8: 56 }.encode()); } Some(IRType::I16)|Some(IRType::U16) => { code.extend_from_slice(&Instruction::SlliD { rd: d, rj: s, imm8: 48 }.encode()); code.extend_from_slice(&Instruction::SraiD { rd: d, rj: d, imm8: 48 }.encode()); } Some(IRType::I32)|Some(IRType::U32) => { code.extend_from_slice(&Instruction::SlliW { rd: d, rj: s, imm8: 0 }.encode()); code.extend_from_slice(&Instruction::SraiW { rd: d, rj: d, imm8: 0 }.encode()); } _ => { if s != d { code.extend_from_slice(&Instruction::Or { rd: d, rj: s, rk: Gpr::R0 }.encode()); } } } }
                CastKind::Trunc => { if s != d { code.extend_from_slice(&Instruction::Or { rd: d, rj: s, rk: Gpr::R0 }.encode()); } else if let Some(tt) = to_ty { match tt { IRType::U8|IRType::I8 => code.extend_from_slice(&Instruction::Andi { rd: d, rj: d, imm12: 0xFF }.encode()), IRType::U16|IRType::I16 => code.extend_from_slice(&Instruction::Andi { rd: d, rj: d, imm12: 0xFFFF }.encode()), _ => {} } } }
                _ => { if s != d { code.extend_from_slice(&Instruction::Or { rd: d, rj: s, rk: Gpr::R0 }.encode()); } }
            }
            reads.push(phys(s)); writes.push(phys(d)); "cast".to_string()
        }
        IRInstr::Alloc { dst, size, .. } => {
            let d = load_to_reg(dst, alloc, code); let a = ((*size as i32 + 15) & !15) as i32;
            code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -a }.encode());
            code.extend_from_slice(&Instruction::Or { rd: d, rj: Gpr::Sp, rk: Gpr::R0 }.encode());
            writes.push(phys(d)); "alloc".to_string()
        }
        IRInstr::Free { ptr, .. } => { let _ = load_to_reg(ptr, alloc, code); code.extend_from_slice(&Instruction::Nop.encode()); "free".to_string() }
        IRInstr::GetAddress { dst, name: _ } => { let d = load_to_reg(dst, alloc, code); code.extend_from_slice(&Instruction::Nop.encode()); writes.push(phys(d)); "getaddr".to_string() }
        IRInstr::Offset { dst, base, offset, .. } => {
            let d = load_to_reg(dst, alloc, code); let b = load_to_reg(base, alloc, code);
            match resolve_value(offset, alloc) {
                ResolvedVal::Imm(i) => { if i >= -2048 && i <= 2047 { code.extend_from_slice(&Instruction::AddiD { rd: d, rj: b, imm12: i as i32 }.encode()); } else { let s = load_to_reg(offset, alloc, code); code.extend_from_slice(&Instruction::AddD { rd: d, rj: b, rk: s }.encode()); } }
                ResolvedVal::Reg(o) => { code.extend_from_slice(&Instruction::AddD { rd: d, rj: b, rk: o }.encode()); reads.push(phys(o)); }
            }
            reads.push(phys(b)); writes.push(phys(d)); "offset".to_string()
        }
        IRInstr::Phi { dst, .. } => { let d = load_to_reg(dst, alloc, code); code.extend_from_slice(&Instruction::Nop.encode()); writes.push(phys(d)); "phi".to_string() }
        IRInstr::Ret { values } => { if let Some(f) = values.first() { let r = load_to_reg(f, alloc, code); if r != Gpr::A0 { code.extend_from_slice(&Instruction::Or { rd: Gpr::A0, rj: r, rk: Gpr::R0 }.encode()); } } code.extend_from_slice(&Instruction::Nop.encode()); "ret".to_string() }
        IRInstr::Branch { target } => {
            let pos = code.len();
            code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
            fixups.push(BranchFixup { offset: pos, target: target.clone(), is_branch16: false });
            "branch".to_string()
        }
        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let c = load_to_reg(cond, alloc, code);
            let pos1 = code.len();
            code.extend_from_slice(&Instruction::Bne { rj: c, rd: Gpr::R0, offs16: 0 }.encode());
            fixups.push(BranchFixup { offset: pos1, target: true_target.clone(), is_branch16: true });
            let pos2 = code.len();
            code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
            fixups.push(BranchFixup { offset: pos2, target: false_target.clone(), is_branch16: false });
            reads.push(phys(c)); "cond_branch".to_string()
        }
        IRInstr::Syscall { nr, args, dst } => {
            let n = crate::syscall_abi::translate_or_warn(crate::backend::BackendKind::LoongArch64, *nr);
            code.extend_from_slice(&Instruction::AddiD { rd: Gpr::A7, rj: Gpr::R0, imm12: n as i32 }.encode());
            let ar = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5];
            for (i, a) in args.iter().enumerate().take(6) { let r = load_to_reg(a, alloc, code); if r != ar[i] { code.extend_from_slice(&Instruction::Or { rd: ar[i], rj: r, rk: Gpr::R0 }.encode()); } }
            code.extend_from_slice(&Instruction::Syscall.encode());
            if let Some(dv) = dst { let d = load_to_reg(dv, alloc, code); if d != Gpr::A0 { code.extend_from_slice(&Instruction::Or { rd: d, rj: Gpr::A0, rk: Gpr::R0 }.encode()); } writes.push(phys(d)); }
            "syscall".to_string()
        }
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            let ar = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
            for (i, a) in args.iter().enumerate().take(8) { let r = load_to_reg(a, alloc, code); if r != ar[i] { code.extend_from_slice(&Instruction::Or { rd: ar[i], rj: r, rk: Gpr::R0 }.encode()); } }
            let pos = code.len();
            code.extend_from_slice(&Instruction::Bl { offs26: 0 }.encode());
            relocations.push(RelocationEntry { offset: pos as u64, symbol: fname.clone(), reloc_type: "R_LARCH_B26".to_string() });
            if let Some(dv) = dst { let d = load_to_reg(dv, alloc, code); if d != Gpr::A0 { code.extend_from_slice(&Instruction::Or { rd: d, rj: Gpr::A0, rk: Gpr::R0 }.encode()); } writes.push(phys(d)); }
            if *is_extern { "call_extern".to_string() } else { "call".to_string() }
        }
        IRInstr::AtomicLoad { dst, addr, .. } => { let d = load_to_reg(dst, alloc, code); let b = load_to_reg(addr, alloc, code); code.extend_from_slice(&Instruction::LdD { rd: d, rj: b, imm12: 0 }.encode()); reads.push(phys(b)); writes.push(phys(d)); "atomic_load".to_string() }
        IRInstr::AtomicStore { value, addr, .. } => { let v = load_to_reg(value, alloc, code); let b = load_to_reg(addr, alloc, code); code.extend_from_slice(&Instruction::StD { rd: v, rj: b, imm12: 0 }.encode()); reads.push(phys(v)); reads.push(phys(b)); "atomic_store".to_string() }
        IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
            let e = load_to_reg(expected, alloc, code); let b = load_to_reg(addr, alloc, code); let n = load_to_reg(desired, alloc, code); let d = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&Instruction::LlD { rd: d, rj: b, imm14: 0 }.encode());
            code.extend_from_slice(&Instruction::Bne { rj: d, rd: e, offs16: 12 }.encode());
            code.extend_from_slice(&Instruction::ScD { rd: n, rj: b, imm14: 0 }.encode());
            reads.push(phys(e)); reads.push(phys(b)); reads.push(phys(n)); writes.push(phys(d)); "atomic_cas".to_string()
        }
        _ => { code.extend_from_slice(&Instruction::Nop.encode()); "unhandled".to_string() }
    };
    Ok((opcode, reads, writes))
}

fn emit_terminator(code: &mut Vec<u8>, term: &IRTerminator, alloc: &RegAllocResult, frame_size: i32, cs: &[Gpr], fixups: &mut Vec<BranchFixup>) {
    match term {
        IRTerminator::Jump(label) => {
            let pos = code.len();
            code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
            fixups.push(BranchFixup { offset: pos, target: label.clone(), is_branch16: false });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let c = load_to_reg(cond, alloc, code);
            let pos1 = code.len();
            code.extend_from_slice(&Instruction::Bne { rj: c, rd: Gpr::R0, offs16: 0 }.encode());
            fixups.push(BranchFixup { offset: pos1, target: true_block.clone(), is_branch16: true });
            let pos2 = code.len();
            code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
            fixups.push(BranchFixup { offset: pos2, target: false_block.clone(), is_branch16: false });
        }
        IRTerminator::Return(vals) => {
            if let Some(f) = vals.first() { let r = load_to_reg(f, alloc, code); if r != Gpr::A0 { code.extend_from_slice(&Instruction::Or { rd: Gpr::A0, rj: r, rk: Gpr::R0 }.encode()); } }
            code.extend(emit_epilogue_bytes(frame_size, cs));
        }
        IRTerminator::Unreachable => { code.extend_from_slice(&Instruction::Break.encode()); }
        _ => { code.extend_from_slice(&Instruction::Nop.encode()); }
    }
}

fn phys(g: Gpr) -> PhysicalReg { PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32) }
fn emit_fp_fallback(instr: &IRInstr) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed { isa: "loongarch64", reason: format!("FP not yet supported: {:?}", instr) })
}
