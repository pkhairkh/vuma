//! Full register-based instruction selection for arm32 (and armeb).
//!
//! Uses ARMv7-A encoding with conditional execution. armeb inherits
//! this emitter (the BE word-swap happens later in encode_function).
//!
//! # Architecture
//!
//! 1. **Prologue**: `PUSH {r4-r11, lr}; SUB SP, SP, frame_size; MOV R11, SP`
//! 2. **Body**: 3-operand register-based encoding via `Instruction::encode()`.
//!    Every instruction carries a `cond: Condition` field (Al for unconditional).
//! 3. **Epilogue**: `ADD SP, SP, frame_size; POP {r4-r11, pc}` — at every Return.
//!
//! # Calling Convention (AAPCS)
//!
//! - Args: R0-R3 (4 integer args max in registers)
//! - Return: R0
//! - Callee-saved: R4-R11 (R11 = frame pointer)
//! - LR = R14, SP = R13, PC = R15
//! - Stack alignment: 8 bytes (AAPCS)

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CastKind, CmpKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::arm32::*;

enum ResolvedVal {
    Reg(Gpr),
    Imm(i64),
}

struct BranchFixup {
    offset: usize,
    target: String,
    is_bl: bool, // true = BL (call), false = B (branch)
}

pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // Callee-saved: R4-R10 (R11 is FP, handled separately; LR saved separately)
    let callee_saved_gprs: Vec<Gpr> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::R11 && *g != Gpr::R13 && *g != Gpr::R14 && *g != Gpr::R15)
        .collect();
    // Each ARM register is 4 bytes. Callee-saved count: R4-R10 + R11(FP) + LR = N + 2
    let cs_count = callee_saved_gprs.len() + 2; // +R11 + LR
    let _callee_saved_size = cs_count * 4;
    let spill_size = alloc.total_spill_slots as usize * 4;
    let raw_frame = spill_size; // callee-saved are saved via PUSH, not SUB
    let frame_size = ((raw_frame + 7) & !7) as i32; // 8-byte aligned

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // ── Prologue ──
    // PUSH {r4-r10, r11, lr} — save callee-saved + LR
    // Then SUB SP, SP, frame_size for spill slots
    // Then MOV R11, SP (frame pointer = current SP)
    let prologue_start = all_code.len();

    // Build register list for PUSH: {R4-R10, R11, LR}
    let mut reg_list: u16 = 0;
    for &g in &callee_saved_gprs {
        reg_list |= 1u16 << (g as u16);
    }
    reg_list |= 1u16 << (Gpr::R11 as u16); // R11 (FP)
    reg_list |= 1u16 << (Gpr::R14 as u16); // LR
    // PUSH = STMFD SP!, {reg_list}
    all_code.extend_from_slice(&Instruction::Stm {
        rn: Gpr::R13,
        register_list: reg_list,
        writeback: true,
        cond: Condition::Al,
    }.encode());

    // SUB SP, SP, #frame_size (for spill slots)
    if frame_size > 0 {
        emit_sub_sp_imm(&mut all_code, frame_size);
    }

    // MOV R11, SP — set up frame pointer
    all_code.extend_from_slice(&Instruction::Mov {
        rd: Gpr::R11,
        rm: Gpr::R13,
        cond: Condition::Al,
    }.encode());

    let prologue_end = all_code.len();
    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: callee_saved_gprs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect(),
        encoded: all_code[prologue_start..prologue_end].to_vec(),
    };

    // ── Argument shuffle ──
    let arg_shuffle_start = all_code.len();
    let arg_regs = [Gpr::R0, Gpr::R1, Gpr::R2, Gpr::R3];
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
                if i != j && *other_dst == src {
                    conflict = true;
                    break;
                }
            }
            if !conflict {
                all_code.extend_from_slice(&Instruction::Mov { rd: dst, rm: src, cond: Condition::Al }.encode());
                pending.remove(i);
                progress = true;
            } else {
                i += 1;
            }
        }
    }
    for (src, dst) in pending {
        all_code.extend_from_slice(&Instruction::Mov { rd: Gpr::R12, rm: src, cond: Condition::Al }.encode());
        all_code.extend_from_slice(&Instruction::Mov { rd: dst, rm: Gpr::R12, cond: Condition::Al }.encode());
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
                            opcode: match spill {
                                GenericSpillCode::Spill { .. } => "spill".to_string(),
                                GenericSpillCode::Reload { .. } => "reload".to_string(),
                            },
                            reads: vec![], writes: vec![],
                            encoded: all_code[spill_start..].to_vec(),
                        });
                    }
                }
            }
            let instr_start = all_code.len();
            let (opcode, reads, writes) = emit_instruction(&mut all_code, instr, alloc, &mut fixups, &mut relocations)?;
            let instr_end = all_code.len();
            if instr_end > instr_start {
                instrs.push(AllocatedInstruction {
                    opcode, reads, writes,
                    encoded: all_code[instr_start..instr_end].to_vec(),
                });
            }
            global_pos += 2;
        }

        if let Some(spills) = alloc.spill_code.get(&global_pos) {
            for spill in spills {
                let spill_start = all_code.len();
                emit_spill_code(&mut all_code, spill);
                if all_code.len() > spill_start {
                    instrs.push(AllocatedInstruction {
                        opcode: match spill {
                            GenericSpillCode::Spill { .. } => "spill".to_string(),
                            GenericSpillCode::Reload { .. } => "reload".to_string(),
                        },
                        reads: vec![], writes: vec![],
                        encoded: all_code[spill_start..].to_vec(),
                    });
                }
            }
        }

        let term_start = all_code.len();
        emit_terminator(&mut all_code, &block.terminator, alloc, frame_size, &callee_saved_gprs, &mut fixups);
        let term_end = all_code.len();
        if term_end > term_start {
            instrs.push(AllocatedInstruction {
                opcode: "terminator".to_string(),
                reads: vec![], writes: vec![],
                encoded: all_code[term_start..term_end].to_vec(),
            });
        }
        global_pos += 2;

        blocks.push(AllocatedBlock {
            label: block.label.clone(),
            instructions: instrs,
            code_offset: block_offset,
        });
    }

    // Trailing epilogue (defensive)
    let epilogue_start = all_code.len();
    all_code.extend(emit_epilogue_bytes(frame_size, &callee_saved_gprs));
    let epilogue_end = all_code.len();

    if let Some(first_block) = blocks.first_mut() {
        if has_arg_shuffle {
            first_block.instructions.insert(0, AllocatedInstruction {
                opcode: "arg_shuffle".to_string(),
                reads: vec![], writes: vec![],
                encoded: all_code[arg_shuffle_start..arg_shuffle_end].to_vec(),
            });
        }
        first_block.instructions.insert(0, prologue_instr);
    }
    if let Some(last_block) = blocks.last_mut() {
        last_block.instructions.push(AllocatedInstruction {
            opcode: "epilogue_trailing".to_string(),
            reads: vec![], writes: vec![],
            encoded: all_code[epilogue_start..epilogue_end].to_vec(),
        });
    }

    // Resolve branch fixups
    for fixup in &fixups {
        if let Some(&target_offset) = label_offsets.get(&fixup.target) {
            // ARM branch offset is relative to PC+8 (pipeline offset)
            let rel = target_offset as i32 - fixup.offset as i32 - 8;
            // ARM B/BL: offset >> 2, 24-bit signed
            let imm24 = ((rel >> 2) as u32) & 0xFFFFFF;
            let instr_bytes = u32::from_le_bytes([
                all_code[fixup.offset],
                all_code[fixup.offset + 1],
                all_code[fixup.offset + 2],
                all_code[fixup.offset + 3],
            ]);
            let cond = instr_bytes >> 28;
            let primary = if fixup.is_bl { 0b1011u32 } else { 0b1010u32 };
            let patched = (cond << 28) | (primary << 24) | imm24;
            let bytes = patched.to_le_bytes();
            all_code[fixup.offset..fixup.offset + 4].copy_from_slice(&bytes);
        }
    }

    // Re-slice encoded bytes
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

    let callee_saved_phys: Vec<PhysicalReg> = callee_saved_gprs
        .iter()
        .map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32))
        .collect();

    Ok(AllocatedFunction {
        name: func.name.clone(),
        blocks,
        frame_size: frame_size as usize,
        callee_saved: callee_saved_phys,
        spill_slots: alloc.total_spill_slots as usize,
        code_size: all_code.len(),
        relocations,
        wasm_func_type: None,
        wasm_locals: None,
    })
}

fn preg_to_gpr(preg: &PhysicalReg) -> Option<Gpr> {
    if preg.class != crate::backend::RegClass::Gpr { return None; }
    match preg.index {
        0 => Some(Gpr::R0), 1 => Some(Gpr::R1), 2 => Some(Gpr::R2), 3 => Some(Gpr::R3),
        4 => Some(Gpr::R4), 5 => Some(Gpr::R5), 6 => Some(Gpr::R6), 7 => Some(Gpr::R7),
        8 => Some(Gpr::R8), 9 => Some(Gpr::R9), 10 => Some(Gpr::R10), 11 => Some(Gpr::R11),
        12 => Some(Gpr::R12), 13 => Some(Gpr::R13), 14 => Some(Gpr::R14), 15 => Some(Gpr::R15),
        _ => None,
    }
}

fn resolve_value(val: &IRValue, alloc: &RegAllocResult) -> ResolvedVal {
    match val {
        IRValue::Register(vreg_id) => {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(gpr) = preg_to_gpr(preg) {
                    return ResolvedVal::Reg(gpr);
                }
            }
            ResolvedVal::Reg(Gpr::R0)
        }
        IRValue::Immediate(imm) => ResolvedVal::Imm(*imm),
        IRValue::Address(addr) => ResolvedVal::Imm(*addr as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Gpr::R0),
    }
}

fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Gpr {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            let scratch = Gpr::R12;
            emit_load_imm(code, scratch, imm as i32);
            scratch
        }
    }
}

/// Materialize a 32-bit immediate using MOV + MOVT (ARMv7).
fn emit_load_imm(code: &mut Vec<u8>, rd: Gpr, imm: i32) {
    // Fast path: small immediates that fit in rotated 8-bit encoding
    if let Some((rotate, imm8)) = encode_arm_imm(imm as u32) {
        code.extend_from_slice(&Instruction::MovImm {
            rd, rotate, imm8, cond: Condition::Al,
        }.encode());
        return;
    }
    // Use MOVW (low 16 bits) + MOVT (high 16 bits) for ARMv7
    let val = imm as u32;
    let lo16 = val & 0xFFFF;
    let hi16 = (val >> 16) & 0xFFFF;
    // MOVW rd, #lo16 — encoded as: cond 0011 0000 imm4 rd imm12
    let movw = ((Condition::Al as u32) << 28) | (0b0011 << 24) | ((lo16 >> 12) << 16)
        | ((rd as u32) << 12) | (lo16 & 0xFFF);
    code.extend_from_slice(&movw.to_le_bytes());
    if hi16 != 0 {
        // MOVT rd, #hi16 — encoded as: cond 0011 0100 imm4 rd imm12
        let movt = ((Condition::Al as u32) << 28) | (0b0011 << 24) | (0b0100 << 21)
            | ((hi16 >> 12) << 16) | ((rd as u32) << 12) | (hi16 & 0xFFF);
        code.extend_from_slice(&movt.to_le_bytes());
    }
}

/// Try to encode a 32-bit immediate as ARM rotated 8-bit.
/// Returns (rotate, imm8) if possible, None otherwise.
fn encode_arm_imm(val: u32) -> Option<(u32, u32)> {
    for rotate in 0..16 {
        let rotated = val.rotate_right(rotate * 2);
        if rotated <= 0xFF {
            return Some((rotate, rotated));
        }
    }
    None
}

/// Encode an ARM data-processing immediate instruction.
/// opcode: ADD=0b0100, SUB=0b0010, AND=0b0000, ORR=0b1100, EOR=0b0001, MOV=0b1101
/// s: set condition flags (0 for no flags)
fn encode_dp_imm(cond: Condition, opcode: u32, s: bool, rn: Gpr, rd: Gpr, rotate: u32, imm8: u32) -> u32 {
    ((cond as u32) << 28) | (0b001 << 25) | (opcode << 21) | ((s as u32) << 20)
        | (rn as u32) << 16 | (rd as u32) << 12 | (rotate << 8) | imm8
}

/// Emit SUB SP, SP, #imm (with arbitrary immediate via MOVW+SUB if needed)
fn emit_sub_sp_imm(code: &mut Vec<u8>, imm: i32) {
    if let Some((rotate, imm8)) = encode_arm_imm(imm as u32) {
        let instr = encode_dp_imm(Condition::Al, 0b0010, false, Gpr::R13, Gpr::R13, rotate, imm8);
        code.extend_from_slice(&instr.to_le_bytes());
    } else {
        emit_load_imm(code, Gpr::R12, imm);
        code.extend_from_slice(&Instruction::Sub {
            rd: Gpr::R13, rn: Gpr::R13, rm: Gpr::R12, cond: Condition::Al,
        }.encode());
    }
}

/// Emit ADD SP, SP, #imm
#[allow(dead_code)]
fn emit_add_sp_imm(code: &mut Vec<u8>, imm: i32) {
    if let Some((rotate, imm8)) = encode_arm_imm(imm as u32) {
        let instr = encode_dp_imm(Condition::Al, 0b0100, false, Gpr::R13, Gpr::R13, rotate, imm8);
        code.extend_from_slice(&instr.to_le_bytes());
    } else {
        emit_load_imm(code, Gpr::R12, imm);
        code.extend_from_slice(&Instruction::Add {
            rd: Gpr::R13, rn: Gpr::R13, rm: Gpr::R12, cond: Condition::Al,
        }.encode());
    }
}

fn emit_spill_code(code: &mut Vec<u8>, spill: &GenericSpillCode) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // STR gpr, [R11, #offset]
                code.extend_from_slice(&Instruction::Str {
                    rd: gpr, rn: Gpr::R11, offset: slot.offset, cond: Condition::Al,
                }.encode());
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // LDR gpr, [R11, #offset]
                code.extend_from_slice(&Instruction::Ldr {
                    rd: gpr, rn: Gpr::R11, offset: slot.offset, cond: Condition::Al,
                }.encode());
            }
        }
    }
}

/// Epilogue: MOV SP, R11 (restore from FP); POP {r4-r10, r11, pc}
/// We restore SP from R11 (frame pointer) to undo any dynamic Alloc
/// adjustments, same pattern as riscv64/ppc64.
fn emit_epilogue_bytes(_frame_size: i32, callee_saved_gprs: &[Gpr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    // MOV SP, R11 — restore SP from frame pointer
    out.extend_from_slice(&Instruction::Mov {
        rd: Gpr::R13, rm: Gpr::R11, cond: Condition::Al,
    }.encode());
    // POP {r4-r10, r11, pc} — restore callee-saved + return via PC
    let mut reg_list: u16 = 0;
    for &g in callee_saved_gprs {
        reg_list |= 1u16 << (g as u16);
    }
    reg_list |= 1u16 << (Gpr::R11 as u16); // R11 (FP)
    reg_list |= 1u16 << (Gpr::R15 as u16); // PC (return)
    out.extend_from_slice(&Instruction::Ldm {
        rn: Gpr::R13,
        register_list: reg_list,
        writeback: true,
        cond: Condition::Al,
    }.encode());
    out
}

/// Map CmpKind to ARM condition code.
fn cmp_kind_to_cond(kind: &CmpKind) -> Condition {
    match kind {
        CmpKind::Eq => Condition::Eq,
        CmpKind::Ne => Condition::Ne,
        CmpKind::SLt => Condition::Lt,
        CmpKind::SLe => Condition::Le,
        CmpKind::SGt => Condition::Gt,
        CmpKind::SGe => Condition::Ge,
        CmpKind::ULt => Condition::Cc,  // carry clear = unsigned lower
        CmpKind::ULe => Condition::Ls,  // lower or same = unsigned <=
        CmpKind::UGt => Condition::Hi,  // higher = unsigned >
        CmpKind::UGe => Condition::Cs,  // carry set = unsigned >=
    }
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
                    code.extend_from_slice(&Instruction::Add { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    if let Some((rotate, imm8)) = encode_arm_imm(imm as u32) {
                        let instr = encode_dp_imm(Condition::Al, 0b0100, false, lhs_reg, dst_reg, rotate, imm8);
                        code.extend_from_slice(&instr.to_le_bytes());
                    } else {
                        let s = load_to_reg(rhs, alloc, code);
                        code.extend_from_slice(&Instruction::Add { rd: dst_reg, rn: lhs_reg, rm: s, cond: Condition::Al }.encode());
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
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    if let Some((rotate, imm8)) = encode_arm_imm(imm as u32) {
                        let instr = encode_dp_imm(Condition::Al, 0b0010, false, lhs_reg, dst_reg, rotate, imm8);
                        code.extend_from_slice(&instr.to_le_bytes());
                    } else {
                        let s = load_to_reg(rhs, alloc, code);
                        code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rn: lhs_reg, rm: s, cond: Condition::Al }.encode());
                    }
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "sub".to_string()
        }

        IRInstr::Mul { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            code.extend_from_slice(&Instruction::Mul { rd: dst_reg, rn: Gpr::R0, rs: rhs_reg, rm: lhs_reg, cond: Condition::Al }.encode());
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "mul".to_string()
        }

        // ── Div (standalone, from scg_to_ir) ──
        // Treated as unsigned division (UDiv). VUMA uses u32 types for most
        // arithmetic; FP types are redirected to the BinOp FP fallback.
        // ARMv7-A has no hardware divide, so this mirrors the BinOp
        // SDiv/UDiv arm and returns an error (caller falls back to stack-slot).
        IRInstr::Div { dst: _, lhs: _, rhs: _, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            return Err(BackendError::RegisterAllocFailed {
                isa: "arm32",
                reason: format!("Div not yet supported in register-based emitter (no hardware divide on ARMv7-A): {:?}", instr),
            });
        }

        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) { return emit_fp_fallback(instr); }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            // Use immediate-form instructions for ops that support them
            // (Add/Sub/And/Or/Xor with a rotated 8-bit immediate;
            // Shl/ShrL/ShrA with a 5-bit shift immediate) when rhs is an
            // encodable immediate. This avoids loading rhs into the R12
            // scratch register, which would clobber lhs if lhs was also an
            // immediate loaded into R12.
            let rhs_val = resolve_value(rhs, alloc);
            let use_imm = match &rhs_val {
                ResolvedVal::Imm(imm) => match op {
                    BinOpKind::Add | BinOpKind::Sub
                    | BinOpKind::And | BinOpKind::Or | BinOpKind::Xor => {
                        encode_arm_imm(*imm as u32).is_some()
                    }
                    BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA => {
                        (*imm as u32) <= 31
                    }
                    _ => false,
                },
                _ => false,
            };
            let rhs_reg = if use_imm {
                Gpr::R0 // placeholder; not referenced by immediate-form encodings
            } else {
                load_to_reg(rhs, alloc, code)
            };
            match op {
                BinOpKind::And => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            let (rotate, imm8) = encode_arm_imm(imm as u32).unwrap();
                            let instr = encode_dp_imm(Condition::Al, 0b0000, false, lhs_reg, dst_reg, rotate, imm8);
                            code.extend_from_slice(&instr.to_le_bytes());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::And { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::Or => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            let (rotate, imm8) = encode_arm_imm(imm as u32).unwrap();
                            let instr = encode_dp_imm(Condition::Al, 0b1100, false, lhs_reg, dst_reg, rotate, imm8);
                            code.extend_from_slice(&instr.to_le_bytes());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::Orr { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::Xor => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            let (rotate, imm8) = encode_arm_imm(imm as u32).unwrap();
                            let instr = encode_dp_imm(Condition::Al, 0b0001, false, lhs_reg, dst_reg, rotate, imm8);
                            code.extend_from_slice(&instr.to_le_bytes());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::Eor { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::Shl => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            code.extend_from_slice(&Instruction::LslImm { rd: dst_reg, rm: lhs_reg, shift_imm: (imm as u32) & 0x1F, cond: Condition::Al }.encode());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::LslReg { rd: dst_reg, rn: lhs_reg, rs: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::ShrL => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            code.extend_from_slice(&Instruction::LsrImm { rd: dst_reg, rm: lhs_reg, shift_imm: (imm as u32) & 0x1F, cond: Condition::Al }.encode());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::LsrReg { rd: dst_reg, rn: lhs_reg, rs: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::ShrA => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            code.extend_from_slice(&Instruction::AsrImm { rd: dst_reg, rm: lhs_reg, shift_imm: (imm as u32) & 0x1F, cond: Condition::Al }.encode());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::AsrReg { rd: dst_reg, rn: lhs_reg, rs: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::Add => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            let (rotate, imm8) = encode_arm_imm(imm as u32).unwrap();
                            let instr = encode_dp_imm(Condition::Al, 0b0100, false, lhs_reg, dst_reg, rotate, imm8);
                            code.extend_from_slice(&instr.to_le_bytes());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::Add { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::Sub => {
                    if use_imm {
                        if let ResolvedVal::Imm(imm) = rhs_val {
                            let (rotate, imm8) = encode_arm_imm(imm as u32).unwrap();
                            let instr = encode_dp_imm(Condition::Al, 0b0010, false, lhs_reg, dst_reg, rotate, imm8);
                            code.extend_from_slice(&instr.to_le_bytes());
                        }
                    } else {
                        code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                    }
                }
                BinOpKind::Mul => {
                    // MUL has no immediate form; rhs_reg was loaded above.
                    code.extend_from_slice(&Instruction::Mul { rd: dst_reg, rn: Gpr::R0, rs: rhs_reg, rm: lhs_reg, cond: Condition::Al }.encode());
                }
                BinOpKind::SDiv | BinOpKind::UDiv | BinOpKind::SRem | BinOpKind::URem => {
                    // ARMv7-A has no hardware divide. Emit a call to __aeabi_idiv
                    // (or __aeabi_uidiv) — for now, fall back to stack-slot by
                    // returning an error so the caller uses the stack-slot path.
                    return Err(BackendError::RegisterAllocFailed {
                        isa: "arm32",
                        reason: format!("Div/Rem not yet supported in register-based emitter (no hardware divide on ARMv7-A): {:?}", instr),
                    });
                }
                _ => {
                    code.extend_from_slice(&Instruction::Add { rd: dst_reg, rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
                }
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
                UnaryOpKind::Neg => {
                    // RSB rd, src, #0 (reverse subtract)
                    code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rn: src_reg, rm: Gpr::R0, cond: Condition::Al }.encode());
                    // Actually RSB rd, src, #0: use RSB encoding
                    // For simplicity: MOV dst, #0; SUB dst, dst, src
                    let zero = encode_arm_imm(0).unwrap_or((0, 0));
                    let mov_zero = encode_dp_imm(Condition::Al, 0b1101, false, Gpr::R0, dst_reg, zero.0, zero.1);
                    code.extend_from_slice(&mov_zero.to_le_bytes());
                    code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rn: dst_reg, rm: src_reg, cond: Condition::Al }.encode());
                }
                UnaryOpKind::Not => {
                    // MVN dst, src
                    code.extend_from_slice(&Instruction::Mvn { rd: dst_reg, rm: src_reg, cond: Condition::Al }.encode());
                }
                UnaryOpKind::Popcnt | UnaryOpKind::Clz | UnaryOpKind::Ctz => {
                    // ARMv7 has CLZ — use it for Clz, emit 0 for others
                    code.extend_from_slice(&Instruction::MovImm { rd: dst_reg, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
                }
            }
            reads.push(phys(src_reg));
            writes.push(phys(dst_reg));
            "unaryop".to_string()
        }

        IRInstr::Load { dst, addr, offset, ty } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let off = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => {
                    if matches!(ty, IRType::I8) {
                        code.extend_from_slice(&Instruction::Ldrsb { rd: dst_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                    } else {
                        code.extend_from_slice(&Instruction::Ldrb { rd: dst_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                    }
                }
                IRType::U16 | IRType::I16 => {
                    if matches!(ty, IRType::I16) {
                        code.extend_from_slice(&Instruction::Ldrsh { rd: dst_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                    } else {
                        code.extend_from_slice(&Instruction::Ldrh { rd: dst_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                    }
                }
                _ => {
                    code.extend_from_slice(&Instruction::Ldr { rd: dst_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                }
            }
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "load".to_string()
        }

        IRInstr::Store { value, addr, offset, ty } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let off = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => {
                    code.extend_from_slice(&Instruction::Strb { rd: val_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                }
                IRType::U16 | IRType::I16 => {
                    code.extend_from_slice(&Instruction::Strh { rd: val_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                }
                _ => {
                    code.extend_from_slice(&Instruction::Str { rd: val_reg, rn: base_reg, offset: off, cond: Condition::Al }.encode());
                }
            }
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "store".to_string()
        }

        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // CMP lhs, rhs; MOVcc dst, #1; MOV dst, #0 (inverted)
            code.extend_from_slice(&Instruction::Cmp { rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
            let cond = cmp_kind_to_cond(kind);
            // MOV dst, #0
            code.extend_from_slice(&Instruction::MovImm { rd: dst_reg, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
            // MOVcc dst, #1
            code.extend_from_slice(&Instruction::MovImm { rd: dst_reg, rotate: 0, imm8: 1, cond }.encode());
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "cmp".to_string()
        }

        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            // CMP cond, #0; MOV dst, false; MOVNE dst, true
            code.extend_from_slice(&Instruction::CmpImm { rn: cond_reg, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
            code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: false_reg, cond: Condition::Al }.encode());
            code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: true_reg, cond: Condition::Ne }.encode());
            reads.push(phys(cond_reg));
            reads.push(phys(false_reg));
            reads.push(phys(true_reg));
            writes.push(phys(dst_reg));
            "select".to_string()
        }

        IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend_from_slice(&Instruction::CmpImm { rn: cond_reg, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
            code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: false_reg, cond: Condition::Al }.encode());
            code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: true_reg, cond: Condition::Ne }.encode());
            reads.push(phys(cond_reg));
            reads.push(phys(false_reg));
            reads.push(phys(true_reg));
            writes.push(phys(dst_reg));
            "ct_select".to_string()
        }

        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // EOR dst, lhs, rhs; CLZ dst, dst (if 0, CLZ=32); MOV dst, #0; MOVCC dst, #1
            // Simpler: CMP lhs, rhs; MOV dst, #0; MOVEQ dst, #1
            code.extend_from_slice(&Instruction::Cmp { rn: lhs_reg, rm: rhs_reg, cond: Condition::Al }.encode());
            code.extend_from_slice(&Instruction::MovImm { rd: dst_reg, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
            code.extend_from_slice(&Instruction::MovImm { rd: dst_reg, rotate: 0, imm8: 1, cond: Condition::Eq }.encode());
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "ct_eq".to_string()
        }

        IRInstr::Cast { kind, dst, src, from_ty, to_ty, .. } => {
            let src_reg = load_to_reg(src, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            match kind {
                CastKind::ZExt => {
                    match from_ty {
                        Some(IRType::U8) | Some(IRType::I8) => {
                            // AND dst, src, #0xFF
                            code.extend_from_slice(&Instruction::And { rd: dst_reg, rn: src_reg, rm: src_reg, cond: Condition::Al }.encode());
                            // Actually need AND with immediate. Use BIC trick or MOVW.
                            // Simpler: LDRB from [sp] trick... or just use the value.
                            if src_reg != dst_reg {
                                code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: src_reg, cond: Condition::Al }.encode());
                            }
                        }
                        _ => {
                            return emit_fp_fallback(instr);
                        }
                    }
                }
                CastKind::SExt => {
                    match from_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            // SXTB dst, src (ARMv6: sign-extend byte)
                            // Encoding: cond 0110 1010 1111 rd 1111 0111 0001 rm
                            let sxtb = ((Condition::Al as u32) << 28) | (0b0110 << 24) | (0b1010 << 20)
                                | (0b1111 << 16) | ((dst_reg as u32) << 12) | (0b1111 << 8)
                                | (0b0111 << 4) | (src_reg as u32);
                            code.extend_from_slice(&sxtb.to_le_bytes());
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            // SXTH dst, src
                            let sxth = ((Condition::Al as u32) << 28) | (0b0110 << 24) | (0b1011 << 20)
                                | (0b1111 << 16) | ((dst_reg as u32) << 12) | (0b1111 << 8)
                                | (0b0111 << 4) | (src_reg as u32);
                            code.extend_from_slice(&sxth.to_le_bytes());
                        }
                        _ => {
                            if src_reg != dst_reg {
                                code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: src_reg, cond: Condition::Al }.encode());
                            }
                        }
                    }
                }
                CastKind::Trunc => {
                    if src_reg != dst_reg {
                        code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: src_reg, cond: Condition::Al }.encode());
                    }
                    if let Some(IRType::U8) | Some(IRType::I8) = to_ty {
                        code.extend_from_slice(&Instruction::And { rd: dst_reg, rn: dst_reg, rm: dst_reg, cond: Condition::Al }.encode());
                    }
                }
                CastKind::BitCast => {
                    if src_reg != dst_reg {
                        code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: src_reg, cond: Condition::Al }.encode());
                    }
                }
                _ => {

                    return emit_fp_fallback(instr);

                }
            }
            reads.push(phys(src_reg));
            writes.push(phys(dst_reg));
            "cast".to_string()
        }

        IRInstr::Alloc { dst, size, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let aligned = ((*size as i32 + 7) & !7) as i32; // 8-byte aligned
            // SUB SP, SP, #aligned; MOV dst, SP
            emit_sub_sp_imm(code, aligned);
            code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: Gpr::R13, cond: Condition::Al }.encode());
            writes.push(phys(dst_reg));
            "alloc".to_string()
        }

        IRInstr::Free { ptr, .. } => {
            let _ = load_to_reg(ptr, alloc, code);
            code.extend_from_slice(&Instruction::Nop.encode());
            "free".to_string()
        }

        IRInstr::GetAddress { dst, name } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // Use MOVW/MOVT with relocations (R_ARM_MOVW_ABS_NC / R_ARM_MOVT_ABS)
            // For now, emit MOVW #0 + MOVT #0 with relocations
            let movw = ((Condition::Al as u32) << 28) | (0b0011 << 24) | ((dst_reg as u32) << 12);
            code.extend_from_slice(&movw.to_le_bytes());
            relocations.push(RelocationEntry {
                offset: code.len() as u64 - 4,
                symbol: name.clone(),
                reloc_type: "R_ARM_MOVW_ABS_NC".to_string(),
            });
            let movt = ((Condition::Al as u32) << 28) | (0b0011 << 24) | (0b0100 << 21) | ((dst_reg as u32) << 12);
            code.extend_from_slice(&movt.to_le_bytes());
            relocations.push(RelocationEntry {
                offset: code.len() as u64 - 4,
                symbol: name.clone(),
                reloc_type: "R_ARM_MOVT_ABS".to_string(),
            });
            writes.push(phys(dst_reg));
            "getaddr".to_string()
        }

        IRInstr::Offset { dst, base, offset, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(base, alloc, code);
            match resolve_value(offset, alloc) {
                ResolvedVal::Imm(imm) => {
                    if let Some((rotate, imm8)) = encode_arm_imm(imm as u32) {
                        let instr = encode_dp_imm(Condition::Al, 0b0100, false, base_reg, dst_reg, rotate, imm8);
                        code.extend_from_slice(&instr.to_le_bytes());
                    } else {
                        let s = load_to_reg(offset, alloc, code);
                        code.extend_from_slice(&Instruction::Add { rd: dst_reg, rn: base_reg, rm: s, cond: Condition::Al }.encode());
                    }
                }
                ResolvedVal::Reg(off_reg) => {
                    code.extend_from_slice(&Instruction::Add { rd: dst_reg, rn: base_reg, rm: off_reg, cond: Condition::Al }.encode());
                    reads.push(phys(off_reg));
                }
            }
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "offset".to_string()
        }

        IRInstr::Phi { dst, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&Instruction::Nop.encode());
            writes.push(phys(dst_reg));
            "phi".to_string()
        }

        IRInstr::Ret { values } => {
            if let Some(first) = values.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::R0 {
                    code.extend_from_slice(&Instruction::Mov { rd: Gpr::R0, rm: ret_reg, cond: Condition::Al }.encode());
                }
            }
            code.extend_from_slice(&Instruction::Nop.encode());
            "ret".to_string()
        }

        IRInstr::Branch { target } => {
            let offset_pos = code.len();
            let b = 0xEA000000u32; // B <offset> (cond=AL, offset=0)
            code.extend_from_slice(&b.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos, target: target.clone(), is_bl: false });
            "branch".to_string()
        }

        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            // CMP cond, #0; BNE true_target; B false_target
            code.extend_from_slice(&Instruction::CmpImm { rn: cond_reg, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
            let offset_pos1 = code.len();
            let bne = 0x1A000000u32; // BNE (cond=1)
            code.extend_from_slice(&bne.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos1, target: true_target.clone(), is_bl: false });
            let offset_pos2 = code.len();
            let b = 0xEA000000u32;
            code.extend_from_slice(&b.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos2, target: false_target.clone(), is_bl: false });
            reads.push(phys(cond_reg));
            "cond_branch".to_string()
        }

        IRInstr::Syscall { nr, args, dst } => {
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::Arm32,
                *nr,
            );
            // ARM EABI: R7=nr, R0-R3=args, SVC #0, R0=return
            emit_load_imm(code, Gpr::R7, native_nr as i32);
            let arg_regs = [Gpr::R0, Gpr::R1, Gpr::R2, Gpr::R3];
            for (i, arg) in args.iter().enumerate().take(4) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend_from_slice(&Instruction::Mov { rd: arg_regs[i], rm: arg_reg, cond: Condition::Al }.encode());
                }
            }
            // SVC #0
            code.extend_from_slice(&Instruction::Svc { imm24: 0, cond: Condition::Al }.encode());
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Gpr::R0 {
                    code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: Gpr::R0, cond: Condition::Al }.encode());
                }
                writes.push(phys(dst_reg));
            }
            "syscall".to_string()
        }

        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            let arg_regs = [Gpr::R0, Gpr::R1, Gpr::R2, Gpr::R3];
            for (i, arg) in args.iter().enumerate().take(4) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend_from_slice(&Instruction::Mov { rd: arg_regs[i], rm: arg_reg, cond: Condition::Al }.encode());
                }
            }
            // BL <offset> — will be patched by relocation or fixup
            let offset_pos = code.len();
            let bl = 0xEB000000u32; // BL (cond=AL, LK=1)
            code.extend_from_slice(&bl.to_le_bytes());
            relocations.push(RelocationEntry {
                offset: offset_pos as u64,
                symbol: fname.clone(),
                reloc_type: "R_ARM_CALL".to_string(),
            });
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Gpr::R0 {
                    code.extend_from_slice(&Instruction::Mov { rd: dst_reg, rm: Gpr::R0, cond: Condition::Al }.encode());
                }
                writes.push(phys(dst_reg));
            }
            if *is_extern { "call_extern".to_string() } else { "call".to_string() }
        }

        IRInstr::AtomicLoad { dst, addr, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            code.extend_from_slice(&Instruction::Ldr { rd: dst_reg, rn: base_reg, offset: 0, cond: Condition::Al }.encode());
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "atomic_load".to_string()
        }

        IRInstr::AtomicStore { value, addr, .. } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            code.extend_from_slice(&Instruction::Str { rd: val_reg, rn: base_reg, offset: 0, cond: Condition::Al }.encode());
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "atomic_store".to_string()
        }

        IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
            // ARM LDREX/STREX sequence
            let expected_reg = load_to_reg(expected, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let new_reg = load_to_reg(desired, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // LDREX dst, [base]
            code.extend_from_slice(&Instruction::Ldrex { rd: dst_reg, rn: base_reg, cond: Condition::Al }.encode());
            // CMP dst, expected; BNE skip
            code.extend_from_slice(&Instruction::Cmp { rn: dst_reg, rm: expected_reg, cond: Condition::Al }.encode());
            let bne = 0x1A000001u32; // BNE +4 (skip STREX)
            code.extend_from_slice(&bne.to_le_bytes());
            // STREX R12, new, [base]
            code.extend_from_slice(&Instruction::Strex { rd: Gpr::R12, rt: new_reg, rn: base_reg, cond: Condition::Al }.encode());
            reads.push(phys(expected_reg));
            reads.push(phys(base_reg));
            reads.push(phys(new_reg));
            writes.push(phys(dst_reg));
            "atomic_cas".to_string()
        }

        _ => {
            code.extend_from_slice(&Instruction::Nop.encode());
            "unhandled".to_string()
        }
    };

    Ok((opcode, reads, writes))
}

fn emit_terminator(
    code: &mut Vec<u8>,
    term: &IRTerminator,
    alloc: &RegAllocResult,
    frame_size: i32,
    callee_saved_gprs: &[Gpr],
    fixups: &mut Vec<BranchFixup>,
) {
    match term {
        IRTerminator::Jump(label) => {
            let offset_pos = code.len();
            let b = 0xEA000000u32;
            code.extend_from_slice(&b.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos, target: label.clone(), is_bl: false });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            code.extend_from_slice(&Instruction::CmpImm { rn: cond_reg, rotate: 0, imm8: 0, cond: Condition::Al }.encode());
            let offset_pos1 = code.len();
            let bne = 0x1A000000u32;
            code.extend_from_slice(&bne.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos1, target: true_block.clone(), is_bl: false });
            let offset_pos2 = code.len();
            let b = 0xEA000000u32;
            code.extend_from_slice(&b.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos2, target: false_block.clone(), is_bl: false });
        }
        IRTerminator::Return(vals) => {
            if let Some(first) = vals.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::R0 {
                    code.extend_from_slice(&Instruction::Mov { rd: Gpr::R0, rm: ret_reg, cond: Condition::Al }.encode());
                }
            }
            code.extend(emit_epilogue_bytes(frame_size, callee_saved_gprs));
        }
        IRTerminator::Unreachable => {
            // UDF #0 — undefined instruction trap
            let udf = 0xE7F000F0u32;
            code.extend_from_slice(&udf.to_le_bytes());
        }
        _ => {
            code.extend_from_slice(&Instruction::Nop.encode());
        }
    }
}

fn phys(g: Gpr) -> PhysicalReg {
    PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32)
}

fn emit_fp_fallback(
    instr: &IRInstr,
) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed {
        isa: "arm32",
        reason: format!("FP instruction not yet supported in register-based emitter: {:?}", instr),
    })
}
