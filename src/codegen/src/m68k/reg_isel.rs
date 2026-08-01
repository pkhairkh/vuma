//! Full register-based instruction selection for m68k (Motorola 68000).
//! 2-operand (dst = dst op src), variable-length, big-endian, no delay slots.

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CmpKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::m68k::*;

enum ResolvedVal { Reg(Gpr), Imm(i64) }
struct BranchFixup { offset: usize, target: String }

pub fn emit_function_regalloc_full(func: &IRFunction, alloc: &RegAllocResult) -> Result<AllocatedFunction, BackendError> {
    let cs: Vec<Gpr> = alloc.used_callee_saved.iter().filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::A6 && *g != Gpr::A7).collect();
    let spill_size = alloc.total_spill_slots as usize * 4;
    let frame_size = ((spill_size + cs.len() * 4 + 3) & !3) as i32; // 4-byte aligned

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Prologue: LINK A6, #-frame_size; MOVEM.L D3-D7/A2-A5, -(SP)
    let p_start = all_code.len();
    all_code.extend(Instruction::Link { reg: Gpr::A6, disp: (-frame_size) as i16 }.encode());
    // Save callee-saved (MOVEM.L is complex; use individual MOVE.L for simplicity)
    let mut cs_off = -4i16;
    for &g in &cs {
        if g as u8 <= 7 {
            // Data register Dn — store to (cs_off, A6)
            all_code.extend(Instruction::Store { src: g, base: Gpr::A6, offset: cs_off }.encode());
        } else {
            // Address register An — store to (cs_off, A6)
            all_code.extend(Instruction::Store { src: g, base: Gpr::A6, offset: cs_off }.encode());
        }
        cs_off -= 4;
    }
    let p_end = all_code.len();
    let prologue = AllocatedInstruction { opcode: "prologue".to_string(), reads: vec![], writes: vec![], encoded: all_code[p_start..p_end].to_vec() };

    // Arg shuffle: D1-D5 → allocator regs
    let as_start = all_code.len();
    let arg_regs = [Gpr::D1, Gpr::D2, Gpr::D3, Gpr::D4, Gpr::D5];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new();
    for (i, p) in func.params.iter().enumerate() { if i >= 5 { break; } if let IRValue::Register(vid) = p { let r = alloc.coalesced_map.get(vid).unwrap_or(vid); if let Some(pg) = alloc.vreg_to_preg.get(r) { if let Some(d) = preg_to_gpr(pg) { let s = arg_regs[i]; if d != s { pending.push((s, d)); } } } } }
    let mut prog = true; while prog && !pending.is_empty() { prog = false; let mut i = 0; while i < pending.len() { let (s, d) = pending[i]; let mut c = false; for (j, (_, od)) in pending.iter().enumerate() { if i != j && *od == s { c = true; break; } } if !c { all_code.extend(Instruction::Move { src: s, dst: d }.encode()); pending.remove(i); prog = true; } else { i += 1; } } }
    for (s, d) in pending { all_code.extend(Instruction::Move { src: s, dst: Gpr::D2 }.encode()); all_code.extend(Instruction::Move { src: Gpr::D2, dst: d }.encode()); }
    let as_end = all_code.len();
    let has_as = as_end > as_start;

    // Body
    let mut gp: u32 = 0;
    for block in &func.blocks {
        let bo = all_code.len(); label_offsets.insert(block.label.clone(), bo);
        let mut instrs: Vec<AllocatedInstruction> = Vec::new();
        for instr in &block.instructions {
            if let Some(spills) = alloc.spill_code.get(&gp) { for sp in spills { let s = all_code.len(); emit_spill(&mut all_code, sp); if all_code.len() > s { instrs.push(AllocatedInstruction { opcode: match sp { GenericSpillCode::Spill { .. } => "spill", _ => "reload" }.to_string(), reads: vec![], writes: vec![], encoded: all_code[s..].to_vec() }); } } }
            let s = all_code.len(); let (op, r, w) = emit_instr(&mut all_code, instr, alloc, &mut fixups, &mut relocations)?; let e = all_code.len();
            if e > s { instrs.push(AllocatedInstruction { opcode: op, reads: r, writes: w, encoded: all_code[s..e].to_vec() }); }
            gp += 2;
        }
        if let Some(spills) = alloc.spill_code.get(&gp) { for sp in spills { let s = all_code.len(); emit_spill(&mut all_code, sp); if all_code.len() > s { instrs.push(AllocatedInstruction { opcode: match sp { GenericSpillCode::Spill { .. } => "spill", _ => "reload" }.to_string(), reads: vec![], writes: vec![], encoded: all_code[s..].to_vec() }); } } }
        let s = all_code.len(); emit_term(&mut all_code, &block.terminator, alloc, frame_size, &cs, &mut fixups); let e = all_code.len();
        if e > s { instrs.push(AllocatedInstruction { opcode: "terminator".to_string(), reads: vec![], writes: vec![], encoded: all_code[s..e].to_vec() }); }
        gp += 2; blocks.push(AllocatedBlock { label: block.label.clone(), instructions: instrs, code_offset: bo });
    }
    let ep_s = all_code.len(); all_code.extend(emit_epilogue(frame_size, &cs)); let ep_e = all_code.len();
    if let Some(fb) = blocks.first_mut() { if has_as { fb.instructions.insert(0, AllocatedInstruction { opcode: "arg_shuffle".to_string(), reads: vec![], writes: vec![], encoded: all_code[as_start..as_end].to_vec() }); } fb.instructions.insert(0, prologue); }
    if let Some(lb) = blocks.last_mut() { lb.instructions.push(AllocatedInstruction { opcode: "epilogue_trailing".to_string(), reads: vec![], writes: vec![], encoded: all_code[ep_s..ep_e].to_vec() }); }

    // Fixups: m68k bra/bcc offset is relative to the instruction start + 2
    for f in &fixups { if let Some(&t) = label_offsets.get(&f.target) {
        let rel = t as i32 - f.offset as i32 - 2;
        if rel >= -32768 && rel <= 32767 {
            let off16 = rel as i16;
            all_code[f.offset+2..f.offset+4].copy_from_slice(&off16.to_be_bytes());
        }
    } }

    let mut off = 0; for b in &mut blocks { b.code_offset = off; for i in &mut b.instructions { let l = i.encoded.len(); if l > 0 && off + l <= all_code.len() { i.encoded = all_code[off..off+l].to_vec(); } off += l; } }
    let cs_phys: Vec<PhysicalReg> = cs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect();
    Ok(AllocatedFunction { name: func.name.clone(), blocks, frame_size: frame_size as usize, callee_saved: cs_phys, spill_slots: alloc.total_spill_slots as usize, code_size: all_code.len(), relocations, wasm_func_type: None, wasm_locals: None })
}

fn preg_to_gpr(p: &PhysicalReg) -> Option<Gpr> { if p.class != crate::backend::RegClass::Gpr { return None; } if p.index > 15 { return None; } Gpr::from_encoding(p.index as u8) }
fn resolve(v: &IRValue, a: &RegAllocResult) -> ResolvedVal { match v { IRValue::Register(id) => { let r = a.coalesced_map.get(id).unwrap_or(id); if let Some(p) = a.vreg_to_preg.get(r) { if let Some(g) = preg_to_gpr(p) { return ResolvedVal::Reg(g); } } ResolvedVal::Reg(Gpr::D0) } IRValue::Immediate(i) => ResolvedVal::Imm(*i), IRValue::Address(a) => ResolvedVal::Imm(*a as i64), IRValue::Label(_) => ResolvedVal::Reg(Gpr::D0) } }
fn load_to_reg(v: &IRValue, a: &RegAllocResult, c: &mut Vec<u8>) -> Gpr { match resolve(v, a) { ResolvedVal::Reg(g) => g, ResolvedVal::Imm(i) => { let s = Gpr::D2; c.extend(Instruction::MoveImm32 { dst: s, imm: i as i32 }.encode()); s } } }
fn emit_spill(c: &mut Vec<u8>, s: &GenericSpillCode) { match s { GenericSpillCode::Spill { preg, slot, .. } => { if let Some(g) = preg_to_gpr(preg) { c.extend(Instruction::Store { src: g, base: Gpr::A6, offset: slot.offset as i16 }.encode()); } } GenericSpillCode::Reload { preg, slot, .. } => { if let Some(g) = preg_to_gpr(preg) { c.extend(Instruction::Load { base: Gpr::A6, offset: slot.offset as i16, dst: g }.encode()); } } } }
#[allow(unused_variables)]
fn emit_epilogue(fs: i32, cs: &[Gpr]) -> Vec<u8> { let mut o = Vec::new(); let mut co = -4i16; for &g in cs.iter().rev() { o.extend(Instruction::Load { base: Gpr::A6, offset: co, dst: g }.encode()); co -= 4; } o.extend(Instruction::Unlk { reg: Gpr::A6 }.encode()); o.extend(Instruction::Rts.encode()); o }

#[allow(clippy::possible_missing_else, unused_variables, unreachable_patterns)]
fn emit_instr(c: &mut Vec<u8>, instr: &IRInstr, a: &RegAllocResult, fx: &mut Vec<BranchFixup>, rel: &mut Vec<RelocationEntry>) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    let mut reads = Vec::new(); let mut writes = Vec::new();
    let op = match instr {
        IRInstr::Add { dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); if l != d { c.extend(Instruction::Move { src: l, dst: d }.encode()); } match resolve(rhs, a) { ResolvedVal::Reg(r) => { c.extend(Instruction::Add { src: r, dst: d }.encode()); reads.push(ph(r)); } ResolvedVal::Imm(i) => { let s = load_to_reg(rhs, a, c); c.extend(Instruction::Add { src: s, dst: d }.encode()); } } reads.push(ph(l)); writes.push(ph(d)); "add".to_string() }
        IRInstr::Sub { dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); if l != d { c.extend(Instruction::Move { src: l, dst: d }.encode()); } let r = load_to_reg(rhs, a, c); c.extend(Instruction::Sub { src: r, dst: d }.encode()); reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "sub".to_string() }
        IRInstr::Mul { dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); if l != d { c.extend(Instruction::Move { src: l, dst: d }.encode()); } let r = load_to_reg(rhs, a, c); c.extend(Instruction::Mulu { src: r, dst: d }.encode()); reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "mul".to_string() }
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); if l != d { c.extend(Instruction::Move { src: l, dst: d }.encode()); } let r = load_to_reg(rhs, a, c); match op { BinOpKind::And => c.extend(Instruction::And { src: r, dst: d }.encode()), BinOpKind::Or => c.extend(Instruction::Or { src: r, dst: d }.encode()), BinOpKind::Xor => { c.extend(Instruction::Nop.encode()); }, BinOpKind::Add => c.extend(Instruction::Add { src: r, dst: d }.encode()), BinOpKind::Sub => c.extend(Instruction::Sub { src: r, dst: d }.encode()), BinOpKind::Mul => c.extend(Instruction::Mulu { src: r, dst: d }.encode()), _ => return Err(BackendError::RegisterAllocFailed { isa: "m68k", reason: format!("BinOp {:?} not supported", op) }), } reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "binop".to_string() }
        IRInstr::UnaryOp { op, dst, operand, .. } => { let d = load_to_reg(dst, a, c); let s = load_to_reg(operand, a, c); if s != d { c.extend(Instruction::Move { src: s, dst: d }.encode()); } match op { UnaryOpKind::Neg => { c.extend(Instruction::Nop.encode()); } UnaryOpKind::Not => { c.extend(Instruction::Nop.encode()); } _ => {} } reads.push(ph(s)); writes.push(ph(d)); "unaryop".to_string() }
        IRInstr::Load { dst, addr, offset, ty } => { let d = load_to_reg(dst, a, c); let b = load_to_reg(addr, a, c); let o = *offset as i16; let base_reg = if b as u8 >= 8 { b } else { let mv = 0x2040u16 | (b as u16 & 7); c.extend_from_slice(&mv.to_be_bytes()); Gpr::A0 }; let base_idx = (base_reg as u16 - 8) & 7; match ty { IRType::U8|IRType::I8 => { let w = 0x1000u16 | ((d as u16 & 7) << 9) | (0b101 << 3) | base_idx; let mut bytes = w.to_be_bytes().to_vec(); bytes.extend_from_slice(&o.to_be_bytes()); c.extend_from_slice(&bytes); }, IRType::U16|IRType::I16 => { let w = 0x3000u16 | ((d as u16 & 7) << 9) | (0b101 << 3) | base_idx; let mut bytes = w.to_be_bytes().to_vec(); bytes.extend_from_slice(&o.to_be_bytes()); c.extend_from_slice(&bytes); }, _ => c.extend(Instruction::Load { base: base_reg, offset: o, dst: d }.encode()), } reads.push(ph(b)); writes.push(ph(d)); "load".to_string() }
        IRInstr::Store { value, addr, offset, ty } => { let v = load_to_reg(value, a, c); let b = load_to_reg(addr, a, c); let o = *offset as i16; let base_reg = if b as u8 >= 8 { b } else { let mv = 0x2040u16 | (b as u16 & 7); c.extend_from_slice(&mv.to_be_bytes()); Gpr::A0 }; let base_idx = (base_reg as u16 - 8) & 7; match ty { IRType::U8|IRType::I8 => { let w = 0x1000u16 | (base_idx << 9) | (0b101 << 6) | (0b000 << 3) | (v as u16 & 7); let mut bytes = w.to_be_bytes().to_vec(); bytes.extend_from_slice(&o.to_be_bytes()); c.extend_from_slice(&bytes); }, IRType::U16|IRType::I16 => { let w = 0x3000u16 | (base_idx << 9) | (0b101 << 6) | (0b000 << 3) | (v as u16 & 7); let mut bytes = w.to_be_bytes().to_vec(); bytes.extend_from_slice(&o.to_be_bytes()); c.extend_from_slice(&bytes); }, _ => c.extend(Instruction::Store { src: v, base: base_reg, offset: o }.encode()), } reads.push(ph(v)); reads.push(ph(b)); "store".to_string() }
        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => { let l = load_to_reg(lhs, a, c); let r = load_to_reg(rhs, a, c); let d = load_to_reg(dst, a, c); c.extend(Instruction::Moveq { dst: d, imm: 0 }.encode()); c.extend(Instruction::Cmp { src: r, dst: l }.encode()); c.extend(Instruction::Bcc { cond: match kind { CmpKind::Eq => 6, CmpKind::Ne => 7, CmpKind::SLt => 12, CmpKind::SLe => 14, CmpKind::SGt => 15, CmpKind::SGe => 13, CmpKind::ULt => 4, CmpKind::ULe => 2, CmpKind::UGt => 3, CmpKind::UGe => 5, _ => 6 }, offset: 4 }.encode()); c.extend(Instruction::Moveq { dst: d, imm: 1 }.encode()); reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "cmp".to_string() }
        IRInstr::Select { dst, cond, true_val, false_val, .. } | IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => { let c_reg = load_to_reg(cond, a, c); let d = load_to_reg(dst, a, c); let f = load_to_reg(false_val, a, c); let t = load_to_reg(true_val, a, c); c.extend(Instruction::Move { src: f, dst: d }.encode()); c.extend(Instruction::Tst { dst: c_reg }.encode()); c.extend(Instruction::Bcc { cond: 6, offset: 4 }.encode()); c.extend(Instruction::Move { src: t, dst: d }.encode()); reads.push(ph(c_reg)); reads.push(ph(f)); reads.push(ph(t)); writes.push(ph(d)); "select".to_string() }
        IRInstr::CtEq { dst, lhs, rhs, .. } => { let l = load_to_reg(lhs, a, c); let r = load_to_reg(rhs, a, c); let d = load_to_reg(dst, a, c); c.extend(Instruction::Moveq { dst: d, imm: 0 }.encode()); c.extend(Instruction::Cmp { src: r, dst: l }.encode()); c.extend(Instruction::Bcc { cond: 6, offset: 4 }.encode()); c.extend(Instruction::Moveq { dst: d, imm: 1 }.encode()); reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "ct_eq".to_string() }
        IRInstr::Cast { dst, src, .. } => { let s = load_to_reg(src, a, c); let d = load_to_reg(dst, a, c); if s != d { c.extend(Instruction::Move { src: s, dst: d }.encode()); } reads.push(ph(s)); writes.push(ph(d)); "cast".to_string() }
        IRInstr::Alloc { dst, size, .. } => { let d = load_to_reg(dst, a, c); let al = ((*size as i32 + 3) & !3) as i16; c.extend_from_slice(&[0x4Fu8, 0xEF]); c.extend_from_slice(&(-al).to_be_bytes()); let dn = d as u8 & 7; let mv = 0x2000u16 | ((dn as u16) << 9) | (0b001 << 3) | 7; c.extend_from_slice(&mv.to_be_bytes()); writes.push(ph(d)); "alloc".to_string() }
        IRInstr::Free { ptr, .. } => { let _ = load_to_reg(ptr, a, c); c.extend(Instruction::Nop.encode()); "free".to_string() }
        IRInstr::GetAddress { dst, name: _ } => { let d = load_to_reg(dst, a, c); c.extend(Instruction::Nop.encode()); writes.push(ph(d)); "getaddr".to_string() }
        IRInstr::Offset { dst, base, offset, .. } => { let d = load_to_reg(dst, a, c); let b = load_to_reg(base, a, c); if b != d { c.extend(Instruction::Move { src: b, dst: d }.encode()); } match resolve(offset, a) { ResolvedVal::Imm(i) => { if i != 0 { c.extend(Instruction::MoveImm32 { dst: Gpr::D2, imm: i as i32 }.encode()); c.extend(Instruction::Add { src: Gpr::D2, dst: d }.encode()); } } ResolvedVal::Reg(o) => c.extend(Instruction::Add { src: o, dst: d }.encode()), } reads.push(ph(b)); writes.push(ph(d)); "offset".to_string() }
        IRInstr::Phi { dst, .. } => { let d = load_to_reg(dst, a, c); c.extend(Instruction::Nop.encode()); writes.push(ph(d)); "phi".to_string() }
        IRInstr::Ret { values } => { if let Some(f) = values.first() { let r = load_to_reg(f, a, c); if r != Gpr::D0 { c.extend(Instruction::Move { src: r, dst: Gpr::D0 }.encode()); } } c.extend(Instruction::Nop.encode()); "ret".to_string() }
        IRInstr::Branch { target } => { let pos = c.len(); c.extend(Instruction::Bra { offset: 0 }.encode()); fx.push(BranchFixup { offset: pos, target: target.clone() }); "branch".to_string() }
        IRInstr::CondBranch { cond, true_target, false_target, .. } => { let c_reg = load_to_reg(cond, a, c); c.extend(Instruction::Tst { dst: c_reg }.encode()); let p1 = c.len(); c.extend(Instruction::Bcc { cond: 6, offset: 0 }.encode()); fx.push(BranchFixup { offset: p1, target: true_target.clone() }); let p2 = c.len(); c.extend(Instruction::Bra { offset: 0 }.encode()); fx.push(BranchFixup { offset: p2, target: false_target.clone() }); reads.push(ph(c_reg)); "cond_branch".to_string() }
        IRInstr::Syscall { nr, args, dst } => { let n = crate::syscall_abi::translate_or_warn(crate::backend::BackendKind::M68k, *nr); c.extend(Instruction::MoveImm32 { dst: Gpr::D0, imm: n as i32 }.encode()); let ar = [Gpr::D1, Gpr::D2, Gpr::D3, Gpr::D4, Gpr::D5]; for (i, arg) in args.iter().enumerate().take(5) { let r = load_to_reg(arg, a, c); if r != ar[i] { c.extend(Instruction::Move { src: r, dst: ar[i] }.encode()); } } c.extend(Instruction::Trap0.encode()); if let Some(dv) = dst { let d = load_to_reg(dv, a, c); if d != Gpr::D0 { c.extend(Instruction::Move { src: Gpr::D0, dst: d }.encode()); } writes.push(ph(d)); } "syscall".to_string() }
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => { let ar = [Gpr::D1, Gpr::D2, Gpr::D3, Gpr::D4, Gpr::D5]; for (i, arg) in args.iter().enumerate().take(5) { let r = load_to_reg(arg, a, c); if r != ar[i] { c.extend(Instruction::Move { src: r, dst: ar[i] }.encode()); } } let pos = c.len(); c.extend_from_slice(&[0x61u8, 0xFF, 0x00, 0x00, 0x00, 0x00]); rel.push(RelocationEntry { offset: pos as u64, symbol: fname.clone(), reloc_type: "R_68K_PC32".to_string() }); if let Some(dv) = dst { let d = load_to_reg(dv, a, c); if d != Gpr::D0 { c.extend(Instruction::Move { src: Gpr::D0, dst: d }.encode()); } writes.push(ph(d)); } if *is_extern { "call_extern".to_string() } else { "call".to_string() } }
        IRInstr::AtomicLoad { dst, addr, .. } => { let d = load_to_reg(dst, a, c); let b = load_to_reg(addr, a, c); c.extend(Instruction::Load { base: b, offset: 0, dst: d }.encode()); reads.push(ph(b)); writes.push(ph(d)); "atomic_load".to_string() }
        IRInstr::AtomicStore { value, addr, .. } => { let v = load_to_reg(value, a, c); let b = load_to_reg(addr, a, c); c.extend(Instruction::Store { src: v, base: b, offset: 0 }.encode()); reads.push(ph(v)); reads.push(ph(b)); "atomic_store".to_string() }
        IRInstr::AtomicCas { .. } => { c.extend(Instruction::Nop.encode()); "atomic_cas".to_string() }
        _ => { c.extend(Instruction::Nop.encode()); "unhandled".to_string() }
    };
    Ok((op, reads, writes))
}

fn emit_term(c: &mut Vec<u8>, term: &IRTerminator, a: &RegAllocResult, fs: i32, cs: &[Gpr], fx: &mut Vec<BranchFixup>) {
    match term {
        IRTerminator::Jump(label) => { let pos = c.len(); c.extend(Instruction::Bra { offset: 0 }.encode()); fx.push(BranchFixup { offset: pos, target: label.clone() }); }
        IRTerminator::Branch { cond, true_block, false_block } => { let c_reg = load_to_reg(cond, a, c); c.extend(Instruction::Tst { dst: c_reg }.encode()); let p1 = c.len(); c.extend(Instruction::Bcc { cond: 6, offset: 0 }.encode()); fx.push(BranchFixup { offset: p1, target: true_block.clone() }); let p2 = c.len(); c.extend(Instruction::Bra { offset: 0 }.encode()); fx.push(BranchFixup { offset: p2, target: false_block.clone() }); }
        IRTerminator::Return(vals) => { if let Some(f) = vals.first() { let r = load_to_reg(f, a, c); if r != Gpr::D0 { c.extend(Instruction::Move { src: r, dst: Gpr::D0 }.encode()); } } c.extend(emit_epilogue(fs, cs)); }
        IRTerminator::Unreachable => { c.extend(Instruction::Trap0.encode()); }
        _ => { c.extend(Instruction::Nop.encode()); }
    }
}

fn ph(g: Gpr) -> PhysicalReg { PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32) }
fn fp_fb(instr: &IRInstr) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> { Err(BackendError::RegisterAllocFailed { isa: "m68k", reason: format!("FP not supported: {:?}", instr) }) }
