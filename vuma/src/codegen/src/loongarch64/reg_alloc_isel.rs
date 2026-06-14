//! # Register-Allocating ISel for LoongArch64
//!
//! Replacement for the stack-slot ISel that keeps values in physical registers
//! as much as possible, drastically reducing memory traffic.
//!
//! ## Key optimization vs stack-slot ISel
//!
//! Under QEMU TCG user-mode emulation, every load/store is ~10-100x slower
//! than a register operation. The old stack-slot ISel did:
//!   load lhs from stack → load rhs from stack → compute → store result to stack
//! That's 3 memory ops per computation.
//!
//! This new ISel uses a **register cache** to keep values in physical registers.
//! Within a basic block, no intermediate stores occur. Values are only flushed
//! to their stack slots at block boundaries and before function calls.

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction,
    BackendError, PhysicalReg, RegClass, RelocationEntry,
};
use crate::ir::{BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRType, IRValue, UnaryOpKind};
use std::collections::HashMap;

use super::{Gpr, Instruction};

// =============================================================================
// Constants
// =============================================================================

/// Allocatable registers in priority order.
const ALLOC_REGS: &[Gpr] = &[
    Gpr::S0, Gpr::S1, Gpr::S2, Gpr::S3, Gpr::S4,
    Gpr::S5, Gpr::S6, Gpr::S7, Gpr::S8,
    Gpr::T0, Gpr::T1, Gpr::T2, Gpr::T3, Gpr::T4,
    Gpr::T5, Gpr::T6, Gpr::T7, Gpr::T8,
    Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3,
    Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7,
];

const CALLER_SAVED: &[Gpr] = &[
    Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3,
    Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7,
    Gpr::T0, Gpr::T1, Gpr::T2, Gpr::T3, Gpr::T4,
    Gpr::T5, Gpr::T6, Gpr::T7, Gpr::T8,
];

const CALLEE_SAVED_ALLOC: &[Gpr] = &[
    Gpr::S0, Gpr::S1, Gpr::S2, Gpr::S3, Gpr::S4,
    Gpr::S5, Gpr::S6, Gpr::S7, Gpr::S8,
];

// =============================================================================
// Helpers
// =============================================================================

fn fits_si12(val: i64) -> bool {
    (-2048..=2047).contains(&val)
}

fn encode_load_imm(rd: Gpr, imm: i64) -> Vec<u8> {
    let val = imm as u64;
    let mut code = Vec::with_capacity(16);
    if val == 0 {
        code.extend_from_slice(&Instruction::AddD { rd, rj: Gpr::R0, rk: Gpr::R0 }.encode());
        return code;
    }
    let hi20 = ((val >> 12) & 0xFFFFF) as i32;
    code.extend_from_slice(&Instruction::Lu12iW { rd, imm20: hi20 }.encode());
    let lo12 = (val & 0xFFF) as u32;
    if lo12 != 0 || (val >> 12) == 0 {
        code.extend_from_slice(&Instruction::Ori { rd, rj: rd, imm12: lo12 }.encode());
    }
    let lower32 = val & 0xFFFFFFFF;
    let sign_ext = if lower32 & 0x80000000 != 0 { 0xFFFFFFFF00000000u64 } else { 0u64 };
    if val >> 32 == sign_ext >> 32 { return code; }
    if val >> 32 == 0 && lower32 & 0x80000000 != 0 {
        code.extend_from_slice(&Instruction::SlliD { rd, rj: rd, imm8: 32 }.encode());
        code.extend_from_slice(&Instruction::SrliD { rd, rj: rd, imm8: 32 }.encode());
        return code;
    }
    let hi32 = ((val >> 32) & 0xFFFFF) as i32;
    code.extend_from_slice(&Instruction::Lu32iD { rd, imm20: hi32 }.encode());
    let hi52 = ((val >> 52) & 0xFFF) as i32;
    code.extend_from_slice(&Instruction::Lu52iD { rd, rj: rd, imm12: hi52 }.encode());
    code
}

fn emit_ai(code: Vec<u8>, name: &str) -> AllocatedInstruction {
    AllocatedInstruction { opcode: name.to_string(), reads: vec![], writes: vec![], encoded: code }
}

// =============================================================================
// Register Cache
// =============================================================================

#[derive(Clone, Copy, Debug)]
enum VregLoc {
    Stack(i32),
    Reg(Gpr, bool), // (register, dirty)
    Undef,
}

#[derive(Clone, Copy, Debug)]
struct RegState {
    vreg: Option<u32>,
    dirty: bool,
    last_used: u32,
}

struct RegCache {
    vreg_loc: HashMap<u32, VregLoc>,
    reg_state: [RegState; 32],
    vreg_slots: HashMap<u32, i32>,
    timestamp: u32,
}

impl RegCache {
    fn new(vreg_slots: HashMap<u32, i32>) -> Self {
        let rs = RegState { vreg: None, dirty: false, last_used: 0 };
        Self { vreg_loc: HashMap::new(), reg_state: [rs; 32], vreg_slots, timestamp: 0 }
    }

    fn slot_offset(&self, vid: u32) -> i32 { self.vreg_slots.get(&vid).copied().unwrap_or(-24) }

    fn touch(&mut self, reg: Gpr) {
        self.timestamp += 1;
        self.reg_state[reg.encoding() as usize].last_used = self.timestamp;
    }

    fn vreg_in_reg(&self, vid: u32) -> Option<Gpr> {
        match self.vreg_loc.get(&vid) {
            Some(VregLoc::Reg(reg, _)) => Some(*reg),
            _ => None,
        }
    }

    fn read_vreg(&mut self, vid: u32, fp: Gpr) -> (Gpr, Vec<u8>) {
        if let Some(reg) = self.vreg_in_reg(vid) {
            self.touch(reg);
            return (reg, Vec::new());
        }
        let (reg, evict_code) = self.alloc_reg(None, fp);
        let offset = self.slot_offset(vid);
        let mut code = evict_code;
        if fits_si12(offset as i64) {
            code.extend_from_slice(&Instruction::LdD { rd: reg, rj: fp, imm12: offset }.encode());
        } else {
            code.extend(encode_load_imm(Gpr::T0, offset as i64));
            code.extend_from_slice(&Instruction::AddD { rd: Gpr::T0, rj: fp, rk: Gpr::T0 }.encode());
            code.extend_from_slice(&Instruction::LdD { rd: reg, rj: Gpr::T0, imm12: 0 }.encode());
        }
        self.assign_vreg(vid, reg, false);
        self.touch(reg);
        (reg, code)
    }

    fn alloc_vreg(&mut self, vid: u32, hint: Option<Gpr>, fp: Gpr) -> (Gpr, Vec<u8>) {
        if let Some(reg) = self.vreg_in_reg(vid) { self.touch(reg); return (reg, Vec::new()); }
        if let Some(h) = hint { if self.reg_state[h.encoding() as usize].vreg.is_none() { self.assign_vreg(vid, h, true); self.touch(h); return (h, Vec::new()); } }
        let (reg, evict_code) = self.alloc_reg(hint, fp);
        self.assign_vreg(vid, reg, true);
        self.touch(reg);
        (reg, evict_code)
    }

    fn alloc_reg(&mut self, hint: Option<Gpr>, fp: Gpr) -> (Gpr, Vec<u8>) {
        if let Some(h) = hint { if self.reg_state[h.encoding() as usize].vreg.is_none() { return (h, Vec::new()); } }
        for &reg in ALLOC_REGS { if self.reg_state[reg.encoding() as usize].vreg.is_none() { return (reg, Vec::new()); } }
        // Evict LRU, preferring caller-saved
        let mut best = ALLOC_REGS[0];
        let mut best_ts = u32::MAX;
        let mut best_prio = 0;
        for &reg in ALLOC_REGS {
            let idx = reg.encoding() as usize;
            if self.reg_state[idx].vreg.is_some() {
                let prio = if CALLEE_SAVED_ALLOC.contains(&reg) { 2 } else { 1 };
                let ts = self.reg_state[idx].last_used;
                if prio < best_prio || (prio == best_prio && ts < best_ts) { best = reg; best_ts = ts; best_prio = prio; }
            }
        }
        let code = self.evict_reg(best, fp);
        (best, code)
    }

    fn evict_reg(&mut self, reg: Gpr, fp: Gpr) -> Vec<u8> {
        let idx = reg.encoding() as usize;
        let old_vid = self.reg_state[idx].vreg;
        let dirty = self.reg_state[idx].dirty;
        let mut code = Vec::new();
        if let Some(vid) = old_vid {
            if dirty {
                let offset = self.slot_offset(vid);
                if fits_si12(offset as i64) {
                    code.extend_from_slice(&Instruction::StD { rd: reg, rj: fp, imm12: offset }.encode());
                } else {
                    // Rare: use $t0 as temp
                    code.extend(encode_load_imm(Gpr::T0, offset as i64));
                    code.extend_from_slice(&Instruction::AddD { rd: Gpr::T0, rj: fp, rk: Gpr::T0 }.encode());
                    code.extend_from_slice(&Instruction::StD { rd: reg, rj: Gpr::T0, imm12: 0 }.encode());
                }
            }
            self.vreg_loc.insert(vid, VregLoc::Stack(self.slot_offset(vid)));
        }
        self.reg_state[idx] = RegState { vreg: None, dirty: false, last_used: 0 };
        code
    }

    fn assign_vreg(&mut self, vid: u32, reg: Gpr, dirty: bool) {
        let idx = reg.encoding() as usize;
        if let Some(old_vid) = self.reg_state[idx].vreg {
            if old_vid != vid { self.vreg_loc.insert(old_vid, VregLoc::Stack(self.slot_offset(old_vid))); }
        }
        self.reg_state[idx] = RegState { vreg: Some(vid), dirty, last_used: 0 };
        self.vreg_loc.insert(vid, VregLoc::Reg(reg, dirty));
    }

    fn mark_dirty(&mut self, vid: u32) {
        if let Some(VregLoc::Reg(reg, _)) = self.vreg_loc.get_mut(&vid) {
            let idx = reg.encoding() as usize;
            self.reg_state[idx].dirty = true;
            *self.vreg_loc.get_mut(&vid).unwrap() = VregLoc::Reg(*reg, true);
        }
    }

    fn flush_all(&mut self, fp: Gpr) -> Vec<u8> {
        let mut code = Vec::new();
        for &reg in ALLOC_REGS {
            let idx = reg.encoding() as usize;
            if self.reg_state[idx].dirty {
                if let Some(vid) = self.reg_state[idx].vreg {
                    let offset = self.slot_offset(vid);
                    if fits_si12(offset as i64) {
                        code.extend_from_slice(&Instruction::StD { rd: reg, rj: fp, imm12: offset }.encode());
                    } else {
                        code.extend(encode_load_imm(Gpr::T0, offset as i64));
                        code.extend_from_slice(&Instruction::AddD { rd: Gpr::T0, rj: fp, rk: Gpr::T0 }.encode());
                        code.extend_from_slice(&Instruction::StD { rd: reg, rj: Gpr::T0, imm12: 0 }.encode());
                    }
                    self.reg_state[idx].dirty = false;
                    self.vreg_loc.insert(vid, VregLoc::Reg(reg, false));
                }
            }
        }
        code
    }

    fn flush_caller_saved(&mut self, fp: Gpr) -> Vec<u8> {
        let mut code = Vec::new();
        for &reg in CALLER_SAVED {
            let idx = reg.encoding() as usize;
            if self.reg_state[idx].dirty {
                if let Some(vid) = self.reg_state[idx].vreg {
                    let offset = self.slot_offset(vid);
                    if fits_si12(offset as i64) {
                        code.extend_from_slice(&Instruction::StD { rd: reg, rj: fp, imm12: offset }.encode());
                    } else {
                        code.extend(encode_load_imm(Gpr::S0, offset as i64));
                        code.extend_from_slice(&Instruction::AddD { rd: Gpr::S0, rj: fp, rk: Gpr::S0 }.encode());
                        code.extend_from_slice(&Instruction::StD { rd: reg, rj: Gpr::S0, imm12: 0 }.encode());
                    }
                    self.reg_state[idx].dirty = false;
                    self.vreg_loc.insert(vid, VregLoc::Reg(reg, false));
                }
            }
        }
        code
    }

    fn invalidate_caller_saved(&mut self) {
        for &reg in CALLER_SAVED {
            let idx = reg.encoding() as usize;
            if let Some(vid) = self.reg_state[idx].vreg {
                self.vreg_loc.insert(vid, VregLoc::Stack(self.slot_offset(vid)));
            }
            self.reg_state[idx] = RegState { vreg: None, dirty: false, last_used: 0 };
        }
    }

    fn process_phi(&mut self, old_vid: u32, new_vid: u32) -> Vec<u8> {
        if old_vid == new_vid { return Vec::new(); }
        if let Some(VregLoc::Reg(reg, dirty)) = self.vreg_loc.get(&old_vid).copied() {
            let idx = reg.encoding() as usize;
            self.reg_state[idx].vreg = Some(new_vid);
            self.vreg_loc.insert(new_vid, VregLoc::Reg(reg, dirty));
            self.vreg_loc.insert(old_vid, VregLoc::Stack(self.slot_offset(old_vid)));
        } else {
            self.vreg_loc.insert(new_vid, VregLoc::Stack(self.slot_offset(new_vid)));
        }
        Vec::new()
    }
}

// =============================================================================
// Comparison lowering
// =============================================================================

fn encode_cmp(kind: &CmpKind, dst: Gpr, lhs: Gpr, rhs: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    match kind {
        CmpKind::Eq => { code.extend_from_slice(&Instruction::Xor { rd: dst, rj: lhs, rk: rhs }.encode()); code.extend_from_slice(&Instruction::Sltui { rd: dst, rj: dst, imm12: 1 }.encode()); }
        CmpKind::Ne => { code.extend_from_slice(&Instruction::Xor { rd: dst, rj: lhs, rk: rhs }.encode()); code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: Gpr::R0, rk: dst }.encode()); }
        CmpKind::SLt => { code.extend_from_slice(&Instruction::Slt { rd: dst, rj: lhs, rk: rhs }.encode()); }
        CmpKind::SLe => { code.extend_from_slice(&Instruction::Slt { rd: dst, rj: rhs, rk: lhs }.encode()); code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode()); }
        CmpKind::SGt => { code.extend_from_slice(&Instruction::Slt { rd: dst, rj: rhs, rk: lhs }.encode()); }
        CmpKind::SGe => { code.extend_from_slice(&Instruction::Slt { rd: dst, rj: lhs, rk: rhs }.encode()); code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode()); }
        CmpKind::ULt => { code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: lhs, rk: rhs }.encode()); }
        CmpKind::ULe => { code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: rhs, rk: lhs }.encode()); code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode()); }
        CmpKind::UGt => { code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: rhs, rk: lhs }.encode()); }
        CmpKind::UGe => { code.extend_from_slice(&Instruction::Sltu { rd: dst, rj: lhs, rk: rhs }.encode()); code.extend_from_slice(&Instruction::Xori { rd: dst, rj: dst, imm12: 1 }.encode()); }
    }
    code
}

fn binop_kind_to_cmp_kind(op: &BinOpKind) -> CmpKind {
    match op {
        BinOpKind::SLt => CmpKind::SLt, BinOpKind::SLe => CmpKind::SLe,
        BinOpKind::SGt => CmpKind::SGt, BinOpKind::SGe => CmpKind::SGe,
        BinOpKind::ULt => CmpKind::ULt, BinOpKind::ULe => CmpKind::ULe,
        BinOpKind::UGt => CmpKind::UGt, BinOpKind::UGe => CmpKind::UGe,
        BinOpKind::Eq => CmpKind::Eq, BinOpKind::Ne => CmpKind::Ne,
        other => unreachable!("BinOpKind::{:?} is not a comparison", other),
    }
}

/// Resolve an IRValue to a physical register.
fn resolve_val(val: &IRValue, cache: &mut RegCache, fp: Gpr) -> (Gpr, Vec<u8>) {
    match val {
        IRValue::Register(vid) => cache.read_vreg(*vid, fp),
        IRValue::Immediate(imm) => {
            let (reg, ac) = cache.alloc_reg(None, fp);
            let mut code = ac;
            code.extend(encode_load_imm(reg, *imm));
            (reg, code)
        }
        IRValue::Address(addr) => {
            let (reg, ac) = cache.alloc_reg(None, fp);
            let mut code = ac;
            code.extend(encode_load_imm(reg, *addr as i64));
            (reg, code)
        }
        IRValue::Label(_) => {
            let (reg, ac) = cache.alloc_reg(None, fp);
            let mut code = ac;
            code.extend(encode_load_imm(reg, 0));
            (reg, code)
        }
    }
}

/// Free a temp register that was allocated for a non-vreg value (immediate/address).
fn free_temp_reg(cache: &mut RegCache, reg: Gpr) {
    // Only free if it's not currently holding a named vreg
    let idx = reg.encoding() as usize;
    // If it holds a vreg, don't free. Otherwise, clear it.
    // We track this: if we allocated this for an immediate, it shouldn't
    // have a vreg assigned. But resolve_val doesn't assign one.
    // Actually, resolve_val calls alloc_reg which doesn't assign a vreg.
    // But the register could have been assigned by something else.
    // For safety, just leave it — the eviction logic will handle it.
    let _ = (cache, reg);
}

// =============================================================================
// Main allocation function
// =============================================================================

pub fn allocate_registers(func: &IRFunction) -> Result<AllocatedFunction, BackendError> {
    let func_name = func.name.clone();

    // ── Phase 1: Collect all vreg IDs and compute stack layout ──
    let mut all_vreg_ids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for &id in func.vregs.keys() { all_vreg_ids.insert(id); }
    for param in &func.params { if let Some(id) = param.as_register() { all_vreg_ids.insert(id); } }
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() { all_vreg_ids.insert(id); }
            for id in instr.used_regs() { all_vreg_ids.insert(id); }
        }
        match &block.terminator {
            crate::ir::IRTerminator::Branch { cond, .. } => { if let Some(id) = cond.as_register() { all_vreg_ids.insert(id); } }
            crate::ir::IRTerminator::Return(vals) => { for val in vals { if let Some(id) = val.as_register() { all_vreg_ids.insert(id); } } }
            _ => {}
        }
    }

    let mut stack_alloc_vregs: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut alloc_sizes: HashMap<u32, i32> = HashMap::new();
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Alloc { dst, size } = instr {
                if let Some(id) = dst.as_register() {
                    stack_alloc_vregs.insert(id);
                    alloc_sizes.insert(id, ((*size as i32 + 15) & !15));
                }
            }
        }
    }

    let mut vreg_slots: HashMap<u32, i32> = HashMap::new();
    let mut sorted: Vec<u32> = all_vreg_ids.iter().copied().collect();
    sorted.sort();
    for (i, &id) in sorted.iter().enumerate() { vreg_slots.insert(id, -(24 + 8 * i as i32)); }

    let num_vregs = sorted.len() as i32;
    let vreg_area_end = 24 + 8 * num_vregs;
    let mut alloc_offsets: HashMap<u32, i32> = HashMap::new();
    let mut alloc_running: i32 = vreg_area_end;
    let mut alloc_ids: Vec<u32> = stack_alloc_vregs.iter().copied().collect();
    alloc_ids.sort();
    for &id in &alloc_ids { let s = alloc_sizes[&id]; alloc_offsets.insert(id, -(alloc_running + s)); alloc_running += s; }

    let frame_size = ((alloc_running + 15) & !15) as usize;

    // ── Phase 2: Generate code ──
    let mut instrs: Vec<AllocatedInstruction> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut cache = RegCache::new(vreg_slots.clone());
    let mut byte_offset: usize = 0;
    let fp = Gpr::Fp;
    let fs = frame_size as i32;

    // Helper to emit code
    let mut emit_code = |code: Vec<u8>, name: &str| -> usize {
        let len = code.len();
        if !code.is_empty() {
            instrs.push(emit_ai(code, name));
        }
        len
    };

    // ── Prologue ──
    if fits_si12(-(fs as i64)) {
        byte_offset += emit_code(Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: -fs }.encode().to_vec(), "addi.d sp, sp, -fs");
    } else {
        let mut c = encode_load_imm(Gpr::S0, -(fs as i64));
        c.extend_from_slice(&Instruction::AddD { rd: Gpr::Sp, rj: Gpr::Sp, rk: Gpr::S0 }.encode());
        byte_offset += emit_code(c, "sub sp, sp, fs");
    }

    let ra_off = fs - 8;
    if fits_si12(ra_off as i64) {
        byte_offset += emit_code(Instruction::StD { rd: Gpr::Ra, rj: Gpr::Sp, imm12: ra_off }.encode().to_vec(), "st.d ra");
    } else {
        let mut c = encode_load_imm(Gpr::S0, ra_off as i64);
        c.extend_from_slice(&Instruction::AddD { rd: Gpr::S0, rj: Gpr::Sp, rk: Gpr::S0 }.encode());
        c.extend_from_slice(&Instruction::StD { rd: Gpr::Ra, rj: Gpr::S0, imm12: 0 }.encode());
        byte_offset += emit_code(c, "st.d ra");
    }

    let fp_off = fs - 16;
    if fits_si12(fp_off as i64) {
        byte_offset += emit_code(Instruction::StD { rd: fp, rj: Gpr::Sp, imm12: fp_off }.encode().to_vec(), "st.d fp");
    } else {
        let mut c = encode_load_imm(Gpr::S0, fp_off as i64);
        c.extend_from_slice(&Instruction::AddD { rd: Gpr::S0, rj: Gpr::Sp, rk: Gpr::S0 }.encode());
        c.extend_from_slice(&Instruction::StD { rd: fp, rj: Gpr::S0, imm12: 0 }.encode());
        byte_offset += emit_code(c, "st.d fp");
    }

    if fits_si12(fs as i64) {
        byte_offset += emit_code(Instruction::AddiD { rd: fp, rj: Gpr::Sp, imm12: fs }.encode().to_vec(), "addi.d fp, sp, fs");
    } else {
        let mut c = encode_load_imm(Gpr::S0, fs as i64);
        c.extend_from_slice(&Instruction::AddD { rd: fp, rj: Gpr::Sp, rk: Gpr::S0 }.encode());
        byte_offset += emit_code(c, "add fp, sp, fs");
    }

    // Save callee-saved registers at $sp+0 through $sp+64
    let callee_save_slots: [(Gpr, i32); 9] = [
        (Gpr::S0, 0), (Gpr::S1, 8), (Gpr::S2, 16),
        (Gpr::S3, 24), (Gpr::S4, 32), (Gpr::S5, 40),
        (Gpr::S6, 48), (Gpr::S7, 56), (Gpr::S8, 64),
    ];
    for &(reg, off) in &callee_save_slots {
        byte_offset += emit_code(Instruction::StD { rd: reg, rj: Gpr::Sp, imm12: off }.encode().to_vec(), &format!("st.d {}", reg.asm_name()));
    }

    // Store params to stack and register cache
    let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
    for (i, param) in func.params.iter().enumerate() {
        if let Some(id) = param.as_register() {
            if i < arg_regs.len() {
                let off = vreg_slots.get(&id).copied().unwrap_or(-24);
                let mut c = Vec::new();
                if fits_si12(off as i64) {
                    c.extend_from_slice(&Instruction::StD { rd: arg_regs[i], rj: fp, imm12: off }.encode());
                } else {
                    c.extend(encode_load_imm(Gpr::T0, off as i64));
                    c.extend_from_slice(&Instruction::AddD { rd: Gpr::T0, rj: fp, rk: Gpr::T0 }.encode());
                    c.extend_from_slice(&Instruction::StD { rd: arg_regs[i], rj: Gpr::T0, imm12: 0 }.encode());
                }
                byte_offset += emit_code(c, "store_param");
                cache.assign_vreg(id, arg_regs[i], false);
            }
        }
    }

    // ── Phase 3: Encode each IR instruction ──
    let mut block_offsets: HashMap<String, usize> = HashMap::new();
    let mut branch_patches: Vec<(usize, String)> = Vec::new();

    // Drop the emit_code closure so we can use instrs/byte_offset directly
    drop(emit_code);

    for block in &func.blocks {
        block_offsets.insert(block.label.clone(), byte_offset);

        // Flush at block entry
        let flush_code = cache.flush_all(fp);
        if !flush_code.is_empty() { byte_offset += flush_code.len(); instrs.push(emit_ai(flush_code, "flush")); }

        for instr in &block.instructions {
            let code = lower_instr(instr, &mut cache, fp, &vreg_slots, &alloc_offsets, &mut relocations);
            if !code.is_empty() { byte_offset += code.len(); instrs.push(emit_ai(code, "instr")); }
        }

        match &block.terminator {
            crate::ir::IRTerminator::Jump(target) => {
                let mut code = cache.flush_all(fp);
                let patch_off = byte_offset + code.len();
                code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
                branch_patches.push((patch_off, target.clone()));
                byte_offset += code.len(); instrs.push(emit_ai(code, "jump"));
            }
            crate::ir::IRTerminator::Branch { cond, true_block, false_block } => {
                let mut code = cache.flush_all(fp);
                let (c, pre) = if let Some(vid) = cond.as_register() { cache.read_vreg(vid, fp) } else { (Gpr::T0, encode_load_imm(Gpr::T0, 1)) };
                code.extend(pre);
                let bnez_off = byte_offset + code.len();
                code.extend_from_slice(&Instruction::Bnez { rj: c, offs21: 0 }.encode());
                branch_patches.push((bnez_off, true_block.clone()));
                let b_off = byte_offset + code.len();
                code.extend_from_slice(&Instruction::B { offs26: 0 }.encode());
                branch_patches.push((b_off, false_block.clone()));
                byte_offset += code.len(); instrs.push(emit_ai(code, "cond_br"));
            }
            crate::ir::IRTerminator::Return(vals) => {
                let mut code = Vec::new();
                if let Some(val) = vals.first() {
                    if let Some(vid) = val.as_register() {
                        let (reg, pre) = cache.read_vreg(vid, fp);
                        code.extend(pre);
                        if reg != Gpr::A0 { code.extend_from_slice(&Instruction::AddD { rd: Gpr::A0, rj: reg, rk: Gpr::R0 }.encode()); }
                    } else if let IRValue::Immediate(imm) = val { code.extend(encode_load_imm(Gpr::A0, *imm)); }
                }
                for &(reg, off) in &callee_save_slots { code.extend_from_slice(&Instruction::LdD { rd: reg, rj: Gpr::Sp, imm12: off }.encode()); }
                code.extend_from_slice(&Instruction::LdD { rd: Gpr::Ra, rj: fp, imm12: -8 }.encode());
                code.extend_from_slice(&Instruction::LdD { rd: fp, rj: fp, imm12: -16 }.encode());
                if fits_si12(fs as i64) { code.extend_from_slice(&Instruction::AddiD { rd: Gpr::Sp, rj: Gpr::Sp, imm12: fs }.encode()); }
                else { code.extend(encode_load_imm(Gpr::T0, fs as i64)); code.extend_from_slice(&Instruction::AddD { rd: Gpr::Sp, rj: Gpr::Sp, rk: Gpr::T0 }.encode()); }
                code.extend_from_slice(&Instruction::Jirl { rd: Gpr::R0, rj: Gpr::Ra, offs16: 0 }.encode());
                byte_offset += code.len(); instrs.push(emit_ai(code, "return"));
            }
            crate::ir::IRTerminator::Unreachable => {
                let c = Instruction::Break.encode().to_vec(); byte_offset += c.len(); instrs.push(emit_ai(c, "unreachable"));
            }
            _ => { let c = Instruction::Break.encode().to_vec(); byte_offset += c.len(); instrs.push(emit_ai(c, "unimpl_term")); }
        }
    }

    // ── Phase 4: Patch branch targets ──
    let mut instr_offsets: Vec<usize> = Vec::with_capacity(instrs.len());
    let mut cur: usize = 0;
    for instr in &instrs { instr_offsets.push(cur); cur += instr.encoded.len(); }

    for (patch_offset, target_label) in &branch_patches {
        if let Some(&target_offset) = block_offsets.get(target_label) {
            for (i, &start) in instr_offsets.iter().enumerate() {
                let end = start + instrs[i].encoded.len();
                if *patch_offset >= start && *patch_offset < end {
                    let within = *patch_offset - start;
                    if within + 4 <= instrs[i].encoded.len() {
                        let word = u32::from_le_bytes([instrs[i].encoded[within], instrs[i].encoded[within+1], instrs[i].encoded[within+2], instrs[i].encoded[within+3]]);
                        let opcode = (word >> 26) & 0x3F;
                        let off_bytes = target_offset as i64 - *patch_offset as i64;
                        let off_instrs = off_bytes / 4;
                        let new_word = if opcode == 0x14 || opcode == 0x15 {
                            let o = (off_instrs as u32) & 0x3FFFFFF; (word & 0xFC000000) | ((o & 0xFFFF) << 10) | ((o >> 16) & 0x3FF)
                        } else if opcode == 0x10 || opcode == 0x11 {
                            let o = (off_instrs as u32) & 0x1FFFFF; let rj = (word >> 5) & 0x1F; ((opcode & 0x3F) << 26) | ((o & 0xFFFF) << 10) | (rj << 5) | ((o >> 16) & 0x1F)
                        } else { word };
                        instrs[i].encoded[within..within+4].copy_from_slice(&new_word.to_le_bytes());
                    }
                    break;
                }
            }
        }
    }

    let code_size: usize = instrs.iter().map(|i| i.encoded.len()).sum();
    let callee_saved: Vec<PhysicalReg> = CALLEE_SAVED_ALLOC.iter().map(|&r| PhysicalReg::new(RegClass::Gpr, r.encoding())).collect();

    Ok(AllocatedFunction {
        name: func_name,
        blocks: vec![AllocatedBlock { label: "entry".to_string(), instructions: instrs, code_offset: 0 }],
        frame_size, callee_saved, spill_slots: 0, code_size, relocations,
        wasm_func_type: None, wasm_locals: None,
    })
}

// =============================================================================
// Instruction lowering
// =============================================================================

fn lower_instr(
    instr: &IRInstr, cache: &mut RegCache, fp: Gpr,
    vreg_slots: &HashMap<u32, i32>, alloc_offsets: &HashMap<u32, i32>,
    relocations: &mut Vec<RelocationEntry>,
) -> Vec<u8> {
    match instr {
        IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
            lower_binop(op, dst, lhs, rhs, cache, fp)
        }
        IRInstr::Add { dst, lhs, rhs, .. } => {
            lower_binop(&BinOpKind::Add, dst, lhs, rhs, cache, fp)
        }
        IRInstr::Sub { dst, lhs, rhs, .. } => lower_binop(&BinOpKind::Sub, dst, lhs, rhs, cache, fp),
        IRInstr::Mul { dst, lhs, rhs, .. } => lower_binop(&BinOpKind::Mul, dst, lhs, rhs, cache, fp),
        IRInstr::Div { dst, lhs, rhs, .. } => lower_binop(&BinOpKind::SDiv, dst, lhs, rhs, cache, fp),
        IRInstr::Cmp { kind, dst, lhs, rhs, .. } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre) = resolve_val(rhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend(encode_cmp(kind, d, l, r));
            cache.mark_dirty(dst_id);
            code
        }
        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (s, pre) = resolve_val(operand, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(s), fp); code.extend(ac);
            match op {
                UnaryOpKind::Neg => { if d != s { code.extend_from_slice(&Instruction::SubD { rd: d, rj: Gpr::R0, rk: s }.encode()); } else { code.extend_from_slice(&Instruction::SubD { rd: d, rj: Gpr::R0, rk: d }.encode()); } }
                UnaryOpKind::Not => { if d != s { code.extend_from_slice(&Instruction::Nor { rd: d, rj: Gpr::R0, rk: s }.encode()); } else { code.extend_from_slice(&Instruction::Nor { rd: d, rj: Gpr::R0, rk: d }.encode()); } }
                UnaryOpKind::Clz => { code.extend_from_slice(&Instruction::Nor { rd: d, rj: Gpr::R0, rk: s }.encode()); code.extend_from_slice(&Instruction::CloD { rd: d, rj: d }.encode()); }
                UnaryOpKind::Ctz | UnaryOpKind::Popcnt => { if d != s { code.extend_from_slice(&Instruction::AddD { rd: d, rj: s, rk: Gpr::R0 }.encode()); } }
            }
            cache.mark_dirty(dst_id); code
        }
        IRInstr::Load { dst, addr, offset, ty } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (a, pre) = resolve_val(addr, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            let addr_reg = if *offset != 0 {
                let (tmp, ac2) = cache.alloc_reg(None, fp); code.extend(ac2);
                if fits_si12(*offset as i64) { code.extend_from_slice(&Instruction::AddiD { rd: tmp, rj: a, imm12: *offset }.encode()); }
                else { code.extend(encode_load_imm(tmp, *offset as i64)); code.extend_from_slice(&Instruction::AddD { rd: tmp, rj: a, rk: tmp }.encode()); }
                tmp
            } else { a };
            let ld = match ty {
                IRType::I8 => Instruction::LdB { rd: d, rj: addr_reg, imm12: 0 },
                IRType::U8 => Instruction::LdBu { rd: d, rj: addr_reg, imm12: 0 },
                IRType::I16 => Instruction::LdH { rd: d, rj: addr_reg, imm12: 0 },
                IRType::U16 => Instruction::LdHu { rd: d, rj: addr_reg, imm12: 0 },
                IRType::I32 => Instruction::LdW { rd: d, rj: addr_reg, imm12: 0 },
                IRType::U32 => Instruction::LdWu { rd: d, rj: addr_reg, imm12: 0 },
                _ => Instruction::LdD { rd: d, rj: addr_reg, imm12: 0 },
            };
            code.extend_from_slice(&ld.encode());
            cache.mark_dirty(dst_id); code
        }
        IRInstr::Store { value, addr, offset, ty } => {
            let mut code = Vec::new();
            let (v, pre) = resolve_val(value, cache, fp); code.extend(pre);
            let (a, pre) = resolve_val(addr, cache, fp); code.extend(pre);
            let final_addr = if *offset != 0 {
                let (tmp, ac) = cache.alloc_reg(None, fp); code.extend(ac);
                if fits_si12(*offset as i64) { code.extend_from_slice(&Instruction::AddiD { rd: tmp, rj: a, imm12: *offset }.encode()); }
                else { code.extend(encode_load_imm(tmp, *offset as i64)); code.extend_from_slice(&Instruction::AddD { rd: tmp, rj: a, rk: tmp }.encode()); }
                tmp
            } else { a };
            let st = match ty {
                IRType::I8 | IRType::U8 => Instruction::StB { rd: v, rj: final_addr, imm12: 0 },
                IRType::I16 | IRType::U16 => Instruction::StH { rd: v, rj: final_addr, imm12: 0 },
                IRType::I32 | IRType::U32 => Instruction::StW { rd: v, rj: final_addr, imm12: 0 },
                _ => Instruction::StD { rd: v, rj: final_addr, imm12: 0 },
            };
            code.extend_from_slice(&st.encode()); code
        }
        IRInstr::Alloc { dst, .. } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            if let Some(&aoff) = alloc_offsets.get(&dst_id) {
                if fits_si12(aoff as i64) { code.extend_from_slice(&Instruction::AddiD { rd: d, rj: fp, imm12: aoff }.encode()); }
                else { code.extend(encode_load_imm(Gpr::T0, aoff as i64)); code.extend_from_slice(&Instruction::AddD { rd: d, rj: fp, rk: Gpr::T0 }.encode()); }
            } else { code.extend_from_slice(&Instruction::AddiD { rd: d, rj: Gpr::Sp, imm12: 0 }.encode()); }
            cache.mark_dirty(dst_id); code
        }
        IRInstr::Ret { values } => {
            let mut code = Vec::new();
            if let Some(val) = values.first() {
                if let Some(vid) = val.as_register() {
                    let (reg, pre) = cache.read_vreg(vid, fp); code.extend(pre);
                    if reg != Gpr::A0 { code.extend_from_slice(&Instruction::AddD { rd: Gpr::A0, rj: reg, rk: Gpr::R0 }.encode()); }
                } else if let IRValue::Immediate(imm) = val { code.extend(encode_load_imm(Gpr::A0, *imm)); }
            }
            code
        }
        IRInstr::Call { dst, func: target, args } => {
            let mut code = cache.flush_caller_saved(fp);
            let call_arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
            for (i, arg) in args.iter().enumerate() {
                if i < call_arg_regs.len() {
                    if let Some(vid) = arg.as_register() {
                        let (reg, pre) = cache.read_vreg(vid, fp); code.extend(pre);
                        if reg != call_arg_regs[i] { code.extend_from_slice(&Instruction::AddD { rd: call_arg_regs[i], rj: reg, rk: Gpr::R0 }.encode()); }
                    } else if let IRValue::Immediate(imm) = arg { code.extend(encode_load_imm(call_arg_regs[i], *imm)); }
                    else if let IRValue::Address(addr) = arg { code.extend(encode_load_imm(call_arg_regs[i], *addr as i64)); }
                }
            }
            let bl_off = code.len(); // offset within this instruction's code
            code.extend_from_slice(&Instruction::Bl { offs26: 0 }.encode());
            relocations.push(RelocationEntry { offset: bl_off as u64, symbol: target.clone(), reloc_type: "R_LARCH_B26".to_string() });
            cache.invalidate_caller_saved();
            if let Some(d) = dst { cache.assign_vreg(d.as_register().unwrap_or(0), Gpr::A0, true); }
            code
        }
        IRInstr::Cast { kind, dst, src, .. } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (s, pre) = resolve_val(src, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(s), fp); code.extend(ac);
            if d != s { code.extend_from_slice(&Instruction::AddD { rd: d, rj: s, rk: Gpr::R0 }.encode()); }
            match kind {
                CastKind::ZExt => { code.extend_from_slice(&Instruction::SlliD { rd: d, rj: d, imm8: 32 }.encode()); code.extend_from_slice(&Instruction::SrliD { rd: d, rj: d, imm8: 32 }.encode()); }
                CastKind::SExt => { code.extend_from_slice(&Instruction::SlliW { rd: d, rj: d, imm8: 0 }.encode()); }
                CastKind::Trunc | CastKind::BitCast => {}
            }
            cache.mark_dirty(dst_id); code
        }
        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (fv, pre) = resolve_val(false_val, cache, fp); code.extend(pre);
            let (tv, pre) = resolve_val(true_val, cache, fp); code.extend(pre);
            let (c, pre) = resolve_val(cond, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            if fv != d { code.extend_from_slice(&Instruction::AddD { rd: d, rj: fv, rk: Gpr::R0 }.encode()); }
            code.extend_from_slice(&Instruction::Beqz { rj: c, offs21: 2 }.encode());
            code.extend_from_slice(&Instruction::AddD { rd: d, rj: tv, rk: Gpr::R0 }.encode());
            cache.mark_dirty(dst_id); code
        }
        IRInstr::Offset { dst, base, offset } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (b, pre) = resolve_val(base, cache, fp); code.extend(pre);
            let (o, pre) = resolve_val(offset, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend_from_slice(&Instruction::AddD { rd: d, rj: b, rk: o }.encode());
            cache.mark_dirty(dst_id); code
        }
        IRInstr::GetAddress { dst, name } => {
            let mut code = Vec::new();
            let dst_id = dst.as_register().unwrap_or(0);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend(encode_load_imm(d, 0));
            relocations.push(RelocationEntry { offset: 0, symbol: name.clone(), reloc_type: "R_LARCH_64".to_string() });
            cache.mark_dirty(dst_id); code
        }
        IRInstr::Free { .. } => Vec::new(),
        IRInstr::Branch { .. } | IRInstr::CondBranch { .. } => Vec::new(), // handled by terminator
        IRInstr::Phi { dst, incoming, .. } => {
            let dst_id = dst.as_register().unwrap_or(0);
            let non_self: Vec<_> = incoming.iter().filter(|(v, _)| v != dst).collect();
            if non_self.len() == 1 {
                let (val, _) = non_self[0];
                if let Some(src_id) = val.as_register() { cache.process_phi(src_id, dst_id) }
                else {
                    let mut code = Vec::new();
                    let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
                    match val { IRValue::Immediate(imm) => code.extend(encode_load_imm(d, *imm)), IRValue::Address(addr) => code.extend(encode_load_imm(d, *addr as i64)), _ => {} }
                    cache.mark_dirty(dst_id); code
                }
            } else if non_self.is_empty() { Vec::new() }
            else {
                let (val, _) = non_self[0];
                if let Some(src_id) = val.as_register() { cache.process_phi(src_id, dst_id) }
                else {
                    let mut code = Vec::new();
                    let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
                    match val { IRValue::Immediate(imm) => code.extend(encode_load_imm(d, *imm)), IRValue::Address(addr) => code.extend(encode_load_imm(d, *addr as i64)), _ => {} }
                    cache.mark_dirty(dst_id); code
                }
            }
        }
        _ => Vec::new(),
    }
}

fn lower_binop(op: &BinOpKind, dst: &IRValue, lhs: &IRValue, rhs: &IRValue, cache: &mut RegCache, fp: Gpr) -> Vec<u8> {
    let mut code = Vec::new();
    let dst_id = dst.as_register().unwrap_or(0);

    match op {
        BinOpKind::Add => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                if fits_si12(*imm) { code.extend_from_slice(&Instruction::AddiD { rd: d, rj: d, imm12: *imm as i32 }.encode()); }
                else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::AddD { rd: d, rj: d, rk: r }.encode()); }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::AddD { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::Sub => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                if fits_si12(-(*imm)) { code.extend_from_slice(&Instruction::AddiD { rd: d, rj: d, imm12: -(*imm as i32) }.encode()); }
                else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::SubD { rd: d, rj: d, rk: r }.encode()); }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::SubD { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::And => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                let u = *imm as u64;
                if u < 4096 { code.extend_from_slice(&Instruction::Andi { rd: d, rj: d, imm12: u as u32 }.encode()); }
                else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::And { rd: d, rj: d, rk: r }.encode()); }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::And { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::Or => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                let u = *imm as u64;
                if u < 4096 { code.extend_from_slice(&Instruction::Ori { rd: d, rj: d, imm12: u as u32 }.encode()); }
                else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::Or { rd: d, rj: d, rk: r }.encode()); }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::Or { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::Xor => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                if *imm == -1 { code.extend_from_slice(&Instruction::Nor { rd: d, rj: d, rk: Gpr::R0 }.encode()); }
                else { let u = *imm as u64; if u < 4096 { code.extend_from_slice(&Instruction::Xori { rd: d, rj: d, imm12: u as u32 }.encode()); } else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::Xor { rd: d, rj: d, rk: r }.encode()); } }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::Xor { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::Shl => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                if *imm >= 0 && *imm < 64 { code.extend_from_slice(&Instruction::SlliD { rd: d, rj: d, imm8: *imm as u32 }.encode()); }
                else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::SllD { rd: d, rj: d, rk: r }.encode()); }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::SllD { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::ShrL => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                if *imm >= 0 && *imm < 64 { code.extend_from_slice(&Instruction::SrliD { rd: d, rj: d, imm8: *imm as u32 }.encode()); }
                else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::SrlD { rd: d, rj: d, rk: r }.encode()); }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::SrlD { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::ShrA => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (d, ac) = cache.alloc_vreg(dst_id, Some(l), fp); code.extend(ac);
            if d != l { code.extend_from_slice(&Instruction::AddD { rd: d, rj: l, rk: Gpr::R0 }.encode()); }
            if let IRValue::Immediate(imm) = rhs {
                if *imm >= 0 && *imm < 64 { code.extend_from_slice(&Instruction::SraiD { rd: d, rj: d, imm8: *imm as u32 }.encode()); }
                else { let (r, pre2) = cache.alloc_reg(None, fp); code.extend(pre2); code.extend(encode_load_imm(r, *imm)); code.extend_from_slice(&Instruction::SraD { rd: d, rj: d, rk: r }.encode()); }
            } else { let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2); code.extend_from_slice(&Instruction::SraD { rd: d, rj: d, rk: r }.encode()); }
            cache.mark_dirty(dst_id);
        }
        BinOpKind::Ror => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend_from_slice(&Instruction::RotrD { rd: d, rj: l, rk: r }.encode());
            cache.mark_dirty(dst_id);
        }
        BinOpKind::Rol => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            let (tmp, ac2) = cache.alloc_reg(None, fp); code.extend(ac2);
            code.extend_from_slice(&Instruction::AddiD { rd: tmp, rj: Gpr::R0, imm12: 64 }.encode());
            code.extend_from_slice(&Instruction::SubD { rd: tmp, rj: tmp, rk: r }.encode());
            code.extend_from_slice(&Instruction::RotrD { rd: d, rj: l, rk: tmp }.encode());
            cache.mark_dirty(dst_id);
        }
        BinOpKind::Mul => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend_from_slice(&Instruction::MulD { rd: d, rj: l, rk: r }.encode());
            cache.mark_dirty(dst_id);
        }
        BinOpKind::SDiv => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend_from_slice(&Instruction::DivD { rd: d, rj: l, rk: r }.encode());
            cache.mark_dirty(dst_id);
        }
        BinOpKind::UDiv => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend_from_slice(&Instruction::DivDu { rd: d, rj: l, rk: r }.encode());
            cache.mark_dirty(dst_id);
        }
        BinOpKind::SRem => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend_from_slice(&Instruction::ModD { rd: d, rj: l, rk: r }.encode());
            cache.mark_dirty(dst_id);
        }
        BinOpKind::URem => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend_from_slice(&Instruction::ModDu { rd: d, rj: l, rk: r }.encode());
            cache.mark_dirty(dst_id);
        }
        BinOpKind::SLt | BinOpKind::SLe | BinOpKind::SGt | BinOpKind::SGe
        | BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe
        | BinOpKind::Eq | BinOpKind::Ne => {
            let (l, pre) = resolve_val(lhs, cache, fp); code.extend(pre);
            let (r, pre2) = resolve_val(rhs, cache, fp); code.extend(pre2);
            let (d, ac) = cache.alloc_vreg(dst_id, None, fp); code.extend(ac);
            code.extend(encode_cmp(&binop_kind_to_cmp_kind(op), d, l, r));
            cache.mark_dirty(dst_id);
        }
    }
    code
}
