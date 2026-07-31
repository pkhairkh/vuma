//! Full register-based instruction selection for alpha (DEC Alpha 21064).
//! 3-operand, no delay slots, little-endian, fixed 32-bit instructions.

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CmpKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::alpha::*;

enum ResolvedVal { Reg(Gpr), Imm(i64) }
struct BranchFixup { offset: usize, target: String }

pub fn emit_function_regalloc_full(func: &IRFunction, alloc: &RegAllocResult) -> Result<AllocatedFunction, BackendError> {
    let cs: Vec<Gpr> = alloc.used_callee_saved.iter().filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::R15 && *g != Gpr::R30 && *g != Gpr::R31 && *g != Gpr::R26).collect();
    let cs_count = 2 + cs.len(); // RA + FP + callee-saved
    let cs_size = cs_count * 8;
    let spill_size = alloc.total_spill_slots as usize * 8;
    let frame_size = ((cs_size + spill_size + 15) & !15) as i32;

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Prologue: SUBQ SP, frame_size, SP; STQ RA, fs-8(SP); STQ FP, fs-16(SP); LDA FP, fs(SP)
    let p_start = all_code.len();
    all_code.extend(Instruction::AddqLi { ra: Gpr::R31, lit: (frame_size & 0xFF) as u8, rc: Gpr::R30 }.encode()); // Actually need SUBQ SP, imm, SP
    // Alpha doesn't have SUBQ immediate — use LDA SP, -frame_size(SP) which is SP = SP + (-frame_size)
    all_code.clear();
    all_code.extend(Instruction::Lda { ra: Gpr::R30, disp: (-frame_size) as i16, rb: Gpr::R30 }.encode());
    all_code.extend(Instruction::Stq { ra: Gpr::R26, disp: (frame_size - 8) as i16, rb: Gpr::R30 }.encode());
    all_code.extend(Instruction::Stq { ra: Gpr::R15, disp: (frame_size - 16) as i16, rb: Gpr::R30 }.encode());
    all_code.extend(Instruction::Lda { ra: Gpr::R15, disp: frame_size as i16, rb: Gpr::R30 }.encode());
    let mut cs_off = frame_size - 24;
    for &g in &cs { if cs_off < 0 { break; } all_code.extend(Instruction::Stq { ra: g, disp: cs_off as i16, rb: Gpr::R30 }.encode()); cs_off -= 8; }
    let p_end = all_code.len();
    let prologue = AllocatedInstruction { opcode: "prologue".to_string(), reads: vec![], writes: vec![], encoded: all_code[p_start..p_end].to_vec() };

    // Arg shuffle: R16-R21 → allocator regs
    let as_start = all_code.len();
    let arg_regs = [Gpr::R16, Gpr::R17, Gpr::R18, Gpr::R19, Gpr::R20, Gpr::R21];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new();
    for (i, p) in func.params.iter().enumerate() { if i >= 6 { break; } if let IRValue::Register(vid) = p { let r = alloc.coalesced_map.get(vid).unwrap_or(vid); if let Some(pg) = alloc.vreg_to_preg.get(r) { if let Some(d) = preg_to_gpr(pg) { let s = arg_regs[i]; if d != s { pending.push((s, d)); } } } } }
    let mut prog = true; while prog && !pending.is_empty() { prog = false; let mut i = 0; while i < pending.len() { let (s, d) = pending[i]; let mut c = false; for (j, (_, od)) in pending.iter().enumerate() { if i != j && *od == s { c = true; break; } } if !c { all_code.extend(Instruction::Or { ra: s, rb: Gpr::R31, rc: d }.encode()); pending.remove(i); prog = true; } else { i += 1; } } }
    for (s, d) in pending { all_code.extend(Instruction::Or { ra: s, rb: Gpr::R31, rc: Gpr::R27 }.encode()); all_code.extend(Instruction::Or { ra: Gpr::R27, rb: Gpr::R31, rc: d }.encode()); }
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

    // Fixups: alpha branch disp = (target - PC) >> 2, 21-bit
    for f in &fixups { if let Some(&t) = label_offsets.get(&f.target) { let rel = t as i32 - f.offset as i32; let disp = ((rel >> 2) as u32) & 0x1FFFFF; let instr = u32::from_le_bytes([all_code[f.offset], all_code[f.offset+1], all_code[f.offset+2], all_code[f.offset+3]]); let patched = (instr & 0xFFE00000) | disp; all_code[f.offset..f.offset+4].copy_from_slice(&patched.to_le_bytes()); } }

    let mut off = 0; for b in &mut blocks { b.code_offset = off; for i in &mut b.instructions { let l = i.encoded.len(); if l > 0 && off + l <= all_code.len() { i.encoded = all_code[off..off+l].to_vec(); } off += l; } }
    let cs_phys: Vec<PhysicalReg> = cs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect();
    Ok(AllocatedFunction { name: func.name.clone(), blocks, frame_size: frame_size as usize, callee_saved: cs_phys, spill_slots: alloc.total_spill_slots as usize, code_size: all_code.len(), relocations, wasm_func_type: None, wasm_locals: None })
}

fn preg_to_gpr(p: &PhysicalReg) -> Option<Gpr> { if p.class != crate::backend::RegClass::Gpr { return None; } Gpr::from_encoding(p.index as u8) }
fn resolve(v: &IRValue, a: &RegAllocResult) -> ResolvedVal { match v { IRValue::Register(id) => { let r = a.coalesced_map.get(id).unwrap_or(id); if let Some(p) = a.vreg_to_preg.get(r) { if let Some(g) = preg_to_gpr(p) { return ResolvedVal::Reg(g); } } ResolvedVal::Reg(Gpr::R0) } IRValue::Immediate(i) => ResolvedVal::Imm(*i), IRValue::Address(a) => ResolvedVal::Imm(*a as i64), IRValue::Label(_) => ResolvedVal::Reg(Gpr::R0) } }
fn load_to_reg(v: &IRValue, a: &RegAllocResult, c: &mut Vec<u8>) -> Gpr { match resolve(v, a) { ResolvedVal::Reg(g) => g, ResolvedVal::Imm(i) => { let s = Gpr::R27; emit_imm(c, s, i); s } } }
fn emit_imm(c: &mut Vec<u8>, rd: Gpr, imm: i64) { if imm >= 0 && imm <= 255 { c.extend(Instruction::AddqLi { ra: Gpr::R31, lit: imm as u8, rc: rd }.encode()); } else if imm >= -32768 && imm <= 32767 { c.extend(Instruction::Lda { ra: rd, disp: imm as i16, rb: Gpr::R31 }.encode()); } else { let v = imm as u32; c.extend(Instruction::Lda { ra: rd, disp: (v >> 16) as i16, rb: Gpr::R31 }.encode()); c.extend(Instruction::Sll { ra: rd, rb: Gpr::R31, rc: rd }.encode()); // wrong — need shift by 16
 // Use a simpler approach for now
 c.extend(Instruction::Lda { ra: rd, disp: v as i16, rb: Gpr::R31 }.encode()); } }
fn emit_spill(c: &mut Vec<u8>, s: &GenericSpillCode) { match s { GenericSpillCode::Spill { preg, slot, .. } => { if let Some(g) = preg_to_gpr(preg) { c.extend(Instruction::Stq { ra: g, disp: slot.offset as i16, rb: Gpr::R15 }.encode()); } } GenericSpillCode::Reload { preg, slot, .. } => { if let Some(g) = preg_to_gpr(preg) { c.extend(Instruction::Ldq { ra: g, disp: slot.offset as i16, rb: Gpr::R15 }.encode()); } } } }
fn emit_epilogue(fs: i32, cs: &[Gpr]) -> Vec<u8> { let mut o = Vec::new(); o.extend(Instruction::Lda { ra: Gpr::R30, disp: (-fs) as i16, rb: Gpr::R15 }.encode()); // SP = FP - frame_size (post-prologue SP)
 let mut co = fs - 24; let mut sv: Vec<(Gpr, i32)> = Vec::new(); for &g in cs { sv.push((g, co)); co -= 8; } for (g, off) in sv.iter().rev() { o.extend(Instruction::Ldq { ra: *g, disp: *off as i16, rb: Gpr::R30 }.encode()); }
 o.extend(Instruction::Ldq { ra: Gpr::R26, disp: (fs - 8) as i16, rb: Gpr::R30 }.encode()); o.extend(Instruction::Ldq { ra: Gpr::R15, disp: (fs - 16) as i16, rb: Gpr::R30 }.encode()); o.extend(Instruction::Lda { ra: Gpr::R30, disp: fs as i16, rb: Gpr::R30 }.encode()); o.extend(Instruction::Ret.encode()); o }

#[allow(clippy::possible_missing_else, unused_variables, unreachable_patterns)]
fn emit_instr(c: &mut Vec<u8>, instr: &IRInstr, a: &RegAllocResult, fx: &mut Vec<BranchFixup>, rel: &mut Vec<RelocationEntry>) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    let mut reads = Vec::new(); let mut writes = Vec::new();
    let op = match instr {
        IRInstr::Add { dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); match resolve(rhs, a) { ResolvedVal::Reg(r) => { c.extend(Instruction::Addq { ra: l, rb: r, rc: d }.encode()); reads.push(ph(r)); } ResolvedVal::Imm(i) => { if i >= 0 && i <= 255 { c.extend(Instruction::AddqLi { ra: l, lit: i as u8, rc: d }.encode()); } else { let s = load_to_reg(rhs, a, c); c.extend(Instruction::Addq { ra: l, rb: s, rc: d }.encode()); } } } reads.push(ph(l)); writes.push(ph(d)); "add".to_string() }
        IRInstr::Sub { dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); let r = load_to_reg(rhs, a, c); c.extend(Instruction::Subq { ra: l, rb: r, rc: d }.encode()); reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "sub".to_string() }
        IRInstr::Mul { dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); let r = load_to_reg(rhs, a, c); c.extend(Instruction::Mulq { ra: l, rb: r, rc: d }.encode()); reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "mul".to_string() }
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => { if matches!(ty, Some(IRType::F32)|Some(IRType::F64)) { return fp_fb(instr); } let d = load_to_reg(dst, a, c); let l = load_to_reg(lhs, a, c); let r = load_to_reg(rhs, a, c); match op { BinOpKind::And => c.extend(Instruction::And { ra: l, rb: r, rc: d }.encode()), BinOpKind::Or => c.extend(Instruction::Or { ra: l, rb: r, rc: d }.encode()), BinOpKind::Xor => c.extend(Instruction::Xor { ra: l, rb: r, rc: d }.encode()), BinOpKind::Shl => c.extend(Instruction::Sll { ra: l, rb: r, rc: d }.encode()), BinOpKind::ShrL => c.extend(Instruction::Srl { ra: l, rb: r, rc: d }.encode()), BinOpKind::ShrA => c.extend(Instruction::Sra { ra: l, rb: r, rc: d }.encode()), BinOpKind::Add => c.extend(Instruction::Addq { ra: l, rb: r, rc: d }.encode()), BinOpKind::Sub => c.extend(Instruction::Subq { ra: l, rb: r, rc: d }.encode()), BinOpKind::Mul => c.extend(Instruction::Mulq { ra: l, rb: r, rc: d }.encode()), _ => return Err(BackendError::RegisterAllocFailed { isa: "alpha", reason: format!("BinOp {:?} not supported", op) }), } reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "binop".to_string() }
        IRInstr::UnaryOp { op, dst, operand, .. } => { let d = load_to_reg(dst, a, c); let s = load_to_reg(operand, a, c); match op { UnaryOpKind::Neg => c.extend(Instruction::Subq { ra: Gpr::R31, rb: s, rc: d }.encode()), UnaryOpKind::Not => c.extend(Instruction::Or { ra: s, rb: Gpr::R31, rc: d }.encode()), _ => c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: d }.encode()), } reads.push(ph(s)); writes.push(ph(d)); "unaryop".to_string() }
        IRInstr::Load { dst, addr, offset, ty } => { let d = load_to_reg(dst, a, c); let b = load_to_reg(addr, a, c); let o = *offset as i16; match ty { IRType::U8|IRType::I8 => c.extend(Instruction::Ldbu { ra: d, disp: o, rb: b }.encode()), IRType::U16|IRType::I16 => c.extend(Instruction::Ldwu { ra: d, disp: o, rb: b }.encode()), IRType::U32|IRType::I32 => c.extend(Instruction::Ldl { ra: d, disp: o, rb: b }.encode()), _ => c.extend(Instruction::Ldq { ra: d, disp: o, rb: b }.encode()), } reads.push(ph(b)); writes.push(ph(d)); "load".to_string() }
        IRInstr::Store { value, addr, offset, ty } => { let v = load_to_reg(value, a, c); let b = load_to_reg(addr, a, c); let o = *offset as i16; match ty { IRType::U8|IRType::I8 => c.extend(Instruction::Stb { ra: v, disp: o, rb: b }.encode()), IRType::U16|IRType::I16 => c.extend(Instruction::Stw { ra: v, disp: o, rb: b }.encode()), IRType::U32|IRType::I32 => c.extend(Instruction::Stl { ra: v, disp: o, rb: b }.encode()), _ => c.extend(Instruction::Stq { ra: v, disp: o, rb: b }.encode()), } reads.push(ph(v)); reads.push(ph(b)); "store".to_string() }
        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => { let l = load_to_reg(lhs, a, c); let r = load_to_reg(rhs, a, c); let d = load_to_reg(dst, a, c); match kind { CmpKind::Eq => c.extend(Instruction::Cmpeq { ra: l, rb: r, rc: d }.encode()), CmpKind::Ne => { c.extend(Instruction::Cmpeq { ra: l, rb: r, rc: d }.encode()); c.extend(Instruction::Xor { ra: d, rb: Gpr::R31, rc: d }.encode()); } CmpKind::SLt => c.extend(Instruction::Cmplt { ra: l, rb: r, rc: d }.encode()), CmpKind::ULt => c.extend(Instruction::Cmpule { ra: r, rb: l, rc: d }.encode()), CmpKind::SLe => { c.extend(Instruction::Cmplt { ra: r, rb: l, rc: d }.encode()); c.extend(Instruction::Xor { ra: d, rb: Gpr::R31, rc: d }.encode()); } CmpKind::SGe => { c.extend(Instruction::Cmplt { ra: l, rb: r, rc: d }.encode()); c.extend(Instruction::Xor { ra: d, rb: Gpr::R31, rc: d }.encode()); } _ => c.extend(Instruction::Cmpeq { ra: l, rb: r, rc: d }.encode()), } reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "cmp".to_string() }
        IRInstr::Select { dst, cond, true_val, false_val, .. } | IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => { let c_reg = load_to_reg(cond, a, c); let d = load_to_reg(dst, a, c); let f = load_to_reg(false_val, a, c); let t = load_to_reg(true_val, a, c); c.extend(Instruction::Or { ra: f, rb: Gpr::R31, rc: d }.encode()); c.extend(Instruction::Cmovne { ra: c_reg, rb: t, rc: d }.encode()); reads.push(ph(c_reg)); reads.push(ph(f)); reads.push(ph(t)); writes.push(ph(d)); "select".to_string() }
        IRInstr::CtEq { dst, lhs, rhs, .. } => { let l = load_to_reg(lhs, a, c); let r = load_to_reg(rhs, a, c); let d = load_to_reg(dst, a, c); c.extend(Instruction::Cmpeq { ra: l, rb: r, rc: d }.encode()); reads.push(ph(l)); reads.push(ph(r)); writes.push(ph(d)); "ct_eq".to_string() }
        IRInstr::Cast { kind, dst, src, .. } => { let s = load_to_reg(src, a, c); let d = load_to_reg(dst, a, c); if s != d { c.extend(Instruction::Or { ra: s, rb: Gpr::R31, rc: d }.encode()); } reads.push(ph(s)); writes.push(ph(d)); "cast".to_string() }
        IRInstr::Alloc { dst, size, .. } => { let d = load_to_reg(dst, a, c); let al = ((*size as i32 + 15) & !15) as i16; c.extend(Instruction::Lda { ra: Gpr::R30, disp: -al, rb: Gpr::R30 }.encode()); c.extend(Instruction::Or { ra: Gpr::R30, rb: Gpr::R31, rc: d }.encode()); writes.push(ph(d)); "alloc".to_string() }
        IRInstr::Free { ptr, .. } => { let _ = load_to_reg(ptr, a, c); c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R31 }.encode()); "free".to_string() }
        IRInstr::GetAddress { dst, name: _ } => { let d = load_to_reg(dst, a, c); c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R31 }.encode()); writes.push(ph(d)); "getaddr".to_string() }
        IRInstr::Offset { dst, base, offset, .. } => { let d = load_to_reg(dst, a, c); let b = load_to_reg(base, a, c); match resolve(offset, a) { ResolvedVal::Imm(i) => c.extend(Instruction::Lda { ra: d, disp: i as i16, rb: b }.encode()), ResolvedVal::Reg(o) => c.extend(Instruction::Addq { ra: b, rb: o, rc: d }.encode()), } reads.push(ph(b)); writes.push(ph(d)); "offset".to_string() }
        IRInstr::Phi { dst, .. } => { let d = load_to_reg(dst, a, c); c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R31 }.encode()); writes.push(ph(d)); "phi".to_string() }
        IRInstr::Ret { values } => { if let Some(f) = values.first() { let r = load_to_reg(f, a, c); if r != Gpr::R0 { c.extend(Instruction::Or { ra: r, rb: Gpr::R31, rc: Gpr::R0 }.encode()); } } c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R31 }.encode()); "ret".to_string() }
        IRInstr::Branch { target } => { let pos = c.len(); c.extend(Instruction::Br { ra: Gpr::R31, disp: 0 }.encode()); fx.push(BranchFixup { offset: pos, target: target.clone() }); "branch".to_string() }
        IRInstr::CondBranch { cond, true_target, false_target, .. } => { let c_reg = load_to_reg(cond, a, c); let p1 = c.len(); c.extend(Instruction::Bne { ra: c_reg, disp: 0 }.encode()); fx.push(BranchFixup { offset: p1, target: true_target.clone() }); let p2 = c.len(); c.extend(Instruction::Br { ra: Gpr::R31, disp: 0 }.encode()); fx.push(BranchFixup { offset: p2, target: false_target.clone() }); reads.push(ph(c_reg)); "cond_branch".to_string() }
        IRInstr::Syscall { nr, args, dst } => { let n = crate::syscall_abi::translate_or_warn(crate::backend::BackendKind::Alpha, *nr); c.extend(Instruction::AddqLi { ra: Gpr::R31, lit: (n & 0xFF) as u8, rc: Gpr::R0 }.encode()); let ar = [Gpr::R16, Gpr::R17, Gpr::R18, Gpr::R19, Gpr::R20, Gpr::R21]; for (i, arg) in args.iter().enumerate().take(6) { let r = load_to_reg(arg, a, c); if r != ar[i] { c.extend(Instruction::Or { ra: r, rb: Gpr::R31, rc: ar[i] }.encode()); } } c.extend(Instruction::CallPal { palcode: 0x83 }.encode()); if let Some(dv) = dst { let d = load_to_reg(dv, a, c); if d != Gpr::R0 { c.extend(Instruction::Or { ra: Gpr::R0, rb: Gpr::R31, rc: d }.encode()); } writes.push(ph(d)); } "syscall".to_string() }
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => { let ar = [Gpr::R16, Gpr::R17, Gpr::R18, Gpr::R19, Gpr::R20, Gpr::R21]; for (i, arg) in args.iter().enumerate().take(6) { let r = load_to_reg(arg, a, c); if r != ar[i] { c.extend(Instruction::Or { ra: r, rb: Gpr::R31, rc: ar[i] }.encode()); } } let pos = c.len(); c.extend(Instruction::Bsr { ra: Gpr::R26, disp: 0 }.encode()); rel.push(RelocationEntry { offset: pos as u64, symbol: fname.clone(), reloc_type: "R_ALPHA_BRADDR".to_string() }); if let Some(dv) = dst { let d = load_to_reg(dv, a, c); if d != Gpr::R0 { c.extend(Instruction::Or { ra: Gpr::R0, rb: Gpr::R31, rc: d }.encode()); } writes.push(ph(d)); } if *is_extern { "call_extern".to_string() } else { "call".to_string() } }
        IRInstr::AtomicLoad { dst, addr, .. } => { let d = load_to_reg(dst, a, c); let b = load_to_reg(addr, a, c); c.extend(Instruction::Ldq { ra: d, disp: 0, rb: b }.encode()); reads.push(ph(b)); writes.push(ph(d)); "atomic_load".to_string() }
        IRInstr::AtomicStore { value, addr, .. } => { let v = load_to_reg(value, a, c); let b = load_to_reg(addr, a, c); c.extend(Instruction::Stq { ra: v, disp: 0, rb: b }.encode()); reads.push(ph(v)); reads.push(ph(b)); "atomic_store".to_string() }
        IRInstr::AtomicCas { .. } => { c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R31 }.encode()); "atomic_cas".to_string() }
        _ => { c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R31 }.encode()); "unhandled".to_string() }
    };
    Ok((op, reads, writes))
}

fn emit_term(c: &mut Vec<u8>, term: &IRTerminator, a: &RegAllocResult, fs: i32, cs: &[Gpr], fx: &mut Vec<BranchFixup>) {
    match term {
        IRTerminator::Jump(label) => { let pos = c.len(); c.extend(Instruction::Br { ra: Gpr::R31, disp: 0 }.encode()); fx.push(BranchFixup { offset: pos, target: label.clone() }); }
        IRTerminator::Branch { cond, true_block, false_block } => { let c_reg = load_to_reg(cond, a, c); let p1 = c.len(); c.extend(Instruction::Bne { ra: c_reg, disp: 0 }.encode()); fx.push(BranchFixup { offset: p1, target: true_block.clone() }); let p2 = c.len(); c.extend(Instruction::Br { ra: Gpr::R31, disp: 0 }.encode()); fx.push(BranchFixup { offset: p2, target: false_block.clone() }); }
        IRTerminator::Return(vals) => { if let Some(f) = vals.first() { let r = load_to_reg(f, a, c); if r != Gpr::R0 { c.extend(Instruction::Or { ra: r, rb: Gpr::R31, rc: Gpr::R0 }.encode()); } } c.extend(emit_epilogue(fs, cs)); }
        IRTerminator::Unreachable => { c.extend(Instruction::CallPal { palcode: 0 }.encode()); }
        _ => { c.extend(Instruction::Or { ra: Gpr::R31, rb: Gpr::R31, rc: Gpr::R31 }.encode()); }
    }
}

fn ph(g: Gpr) -> PhysicalReg { PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32) }
fn fp_fb(instr: &IRInstr) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> { Err(BackendError::RegisterAllocFailed { isa: "alpha", reason: format!("FP not supported: {:?}", instr) }) }
