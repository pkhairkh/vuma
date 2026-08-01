//! Full register-based instruction selection for riscv64.
//!
//! Mirrors the x86_64 `reg_isel.rs` template (Wave 1-6) but uses RISC-V's
//! simpler encoding: fixed 32-bit instructions, 3-operand format
//! (`op rd, rs1, rs2`), no REX prefixes, no two-operand constraint.
//!
//! # Architecture
//!
//! 1. **Prologue**: `addi sp, sp, -N; sw ra, N-8(sp); sw s0, N(sp);
//!    addi s0, sp, N; sw s1, ...; sw s11, ...`
//! 2. **Body**: For each IR instruction, resolve vregs → physical regs via
//!    `alloc.vreg_to_preg`, emit register-based encoding using the
//!    `Instruction` enum's `encode()` method.
//! 3. **Spill/reload**: Insert `sw preg, slot(s0)` / `lw preg, slot(s0)`
//!    at positions from `alloc.spill_code`.
//! 4. **Epilogue**: `lw ra, N-8(sp); lw s0, N(sp); ...; ld s11, ...;
//!    addi sp, sp, N; ret` — emitted at EVERY Return path.

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CastKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::riscv32::*;

/// Resolved value: either a physical register or an immediate.
enum ResolvedVal {
    Reg(Gpr),
    Imm(i64),
}

/// Branch fixup: (byte_offset_in_code, target_label).
struct BranchFixup {
    offset: usize, // byte offset of the rel21/rel13 field in the code
    target: String,
}

/// Emit a complete function using register-based instruction selection.
pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // ── Compute frame size ──
    // RV32: registers are 4 bytes. Callee-saved slots:
    //   - ra (1 slot, always saved)
    //   - s0 (1 slot, always saved as frame pointer)
    //   - each callee-saved GPR from alloc.used_callee_saved (s1-s11)
    // Each slot is 4 bytes on RV32. Total = (2 + N) * 4.
    let callee_saved_gprs: Vec<Gpr> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::S0 && *g != Gpr::Sp && *g != Gpr::Zero)
        .collect();
    let callee_saved_count = 2 + callee_saved_gprs.len();
    let callee_saved_size = callee_saved_count * 4; // RV32: 4 bytes per slot
    // Spill slots: each is 4 bytes on RV32.
    let spill_size = alloc.total_spill_slots as usize * 4;
    // Frame size must be 16-byte aligned (RISC-V ABI).
    let raw_frame = callee_saved_size + spill_size;
    let frame_size = ((raw_frame + 15) & !15) as i32;

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // ── Prologue ──
    // RV32 frame layout (after `addi sp, sp, -frame_size`):
    //   [sp + frame_size - 4]  = ra
    //   [sp + frame_size - 8]  = s0 (frame pointer)
    //   [sp + frame_size - 12] = first callee-saved (s1)
    //   [sp + frame_size - 16] = second callee-saved (s2)
    //   ...
    //   [sp + 0]               = bottom of spill area
    // All offsets POSITIVE from sp (sp was decremented by frame_size).
    // RV32: 4-byte slots.
    let prologue_start = all_code.len();
    // addi sp, sp, -frame_size
    all_code.extend_from_slice(&Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: -frame_size }.encode());
    // sw ra, frame_size-4(sp)  — save return address
    all_code.extend_from_slice(&Instruction::Sw { rs1: Gpr::Sp, rs2: Gpr::Ra, imm: frame_size - 4 }.encode());
    // sw s0, frame_size-8(sp) — save frame pointer
    all_code.extend_from_slice(&Instruction::Sw { rs1: Gpr::Sp, rs2: Gpr::S0, imm: frame_size - 8 }.encode());
    // addi s0, sp, frame_size — set up frame pointer (s0 = old sp)
    all_code.extend_from_slice(&Instruction::Addi { rd: Gpr::S0, rs1: Gpr::Sp, imm: frame_size }.encode());
    // sw s1, frame_size-12(sp); sw s2, frame_size-16(sp); ...
    let mut cs_offset = frame_size - 12;
    for &g in &callee_saved_gprs {
        if cs_offset < 0 {
            break;
        }
        all_code.extend_from_slice(&Instruction::Sw { rs1: Gpr::Sp, rs2: g, imm: cs_offset }.encode());
        cs_offset -= 4;
    }

    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: callee_saved_gprs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect(),
        encoded: all_code[prologue_start..].to_vec(),
    };

    // ── Argument shuffle (function entry) ──
    // RISC-V calling convention: first 8 integer args in a0-a7.
    // The allocator may assign these param vregs to different registers.
    let arg_shuffle_start = all_code.len();
    let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new();
    for (i, param) in func.params.iter().enumerate() {
        if i >= 8 {
            break;
        }
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
    // Pass 1: move non-conflicting args.
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
                all_code.extend_from_slice(&Instruction::Addi { rd: dst, rs1: src, imm: 0 }.encode()); // mv dst, src
                pending.remove(i);
                progress = true;
            } else {
                i += 1;
            }
        }
    }
    // Pass 2: cycles via t6 scratch.
    for (src, dst) in pending {
        all_code.extend_from_slice(&Instruction::Addi { rd: Gpr::T6, rs1: src, imm: 0 }.encode()); // mv t6, src
        all_code.extend_from_slice(&Instruction::Addi { rd: dst, rs1: Gpr::T6, imm: 0 }.encode()); // mv dst, t6
    }
    let arg_shuffle_end = all_code.len();
    let has_arg_shuffle = arg_shuffle_end > arg_shuffle_start;

    // ── Body: emit each block ──
    let mut global_pos: u32 = 0;

    for block in &func.blocks {
        let block_offset = all_code.len();
        label_offsets.insert(block.label.clone(), block_offset);

        let mut instrs: Vec<AllocatedInstruction> = Vec::new();

        for instr in &block.instructions {
            // Insert spill/reload code before this instruction.
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
                            reads: vec![],
                            writes: vec![],
                            encoded: all_code[spill_start..].to_vec(),
                        });
                    }
                }
            }

            let instr_start = all_code.len();
            let (opcode, reads, writes) = emit_instruction(
                &mut all_code,
                instr,
                alloc,
                &mut fixups,
                &mut relocations,
            )?;
            let instr_end = all_code.len();

            if instr_end > instr_start {
                instrs.push(AllocatedInstruction {
                    opcode,
                    reads,
                    writes,
                    encoded: all_code[instr_start..instr_end].to_vec(),
                });
            }
            global_pos += 2;
        }

        // Spill/reload before the terminator.
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
                        reads: vec![],
                        writes: vec![],
                        encoded: all_code[spill_start..].to_vec(),
                    });
                }
            }
        }

        let term_start = all_code.len();
        emit_terminator(
            &mut all_code,
            &block.terminator,
            alloc,
            frame_size,
            &callee_saved_gprs,
            &mut fixups,
        );
        let term_end = all_code.len();

        if term_end > term_start {
            instrs.push(AllocatedInstruction {
                opcode: "terminator".to_string(),
                reads: vec![],
                writes: vec![],
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

    // ── Trailing defensive epilogue (normally unreachable) ──
    let epilogue_start = all_code.len();
    all_code.extend(emit_epilogue_bytes(frame_size, &callee_saved_gprs));
    let epilogue_end = all_code.len();

    if let Some(first_block) = blocks.first_mut() {
        if has_arg_shuffle {
            first_block.instructions.insert(0, AllocatedInstruction {
                opcode: "arg_shuffle".to_string(),
                reads: vec![],
                writes: vec![],
                encoded: all_code[arg_shuffle_start..arg_shuffle_end].to_vec(),
            });
        }
        first_block.instructions.insert(0, prologue_instr);
    }
    if let Some(last_block) = blocks.last_mut() {
        last_block.instructions.push(AllocatedInstruction {
            opcode: "epilogue_trailing".to_string(),
            reads: vec![],
            writes: vec![],
            encoded: all_code[epilogue_start..epilogue_end].to_vec(),
        });
    }

    // ── Resolve branch fixups ──
    for fixup in &fixups {
        if let Some(&target_offset) = label_offsets.get(&fixup.target) {
            // Each instruction is 4 bytes. The rel offset is from the
            // instruction's own address (branch target semantics).
            let rel: i32 = target_offset as i32 - fixup.offset as i32;
            // Re-encode the instruction at fixup.offset with the correct
            // branch displacement. We need to know which branch instruction
            // it is — but since we wrote it, we can re-derive. Simpler:
            // store the BranchFixup with enough info to patch.
            // For RISC-V, branches (B-type) and JAL (J-type) encode the
            // displacement in specific bit fields. We'll re-encode by
            // reading the opcode bits and applying the displacement.
            patch_branch_displacement(&mut all_code, fixup.offset, rel);
        }
    }

    // ── Re-slice AllocatedInstruction.encoded from patched all_code ──
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

/// Patch a branch instruction's displacement in `code` at `offset`.
/// Reads the opcode to determine if it's a B-type (branch) or J-type (jal)
/// and re-encodes the displacement field.
fn patch_branch_displacement(code: &mut [u8], offset: usize, rel: i32) {
    if offset + 4 > code.len() {
        return;
    }
    let instr = u32::from_le_bytes([code[offset], code[offset+1], code[offset+2], code[offset+3]]);
    let opcode = instr & 0x7F;
    if opcode == 0x6F {
        // JAL (J-type): imm[20|10:1|11|19:12]
        let new_jal = encode_jal_imm(rel) | (instr & 0xFFF); // keep rd bits
        let bytes = new_jal.to_le_bytes();
        code[offset..offset+4].copy_from_slice(&bytes);
    } else if opcode == 0x63 {
        // Branch (B-type): imm[12|10:5] | rs2 | rs1 | funct3 | imm[4:1|11] | opcode
        // Preserve rs2[24:20], rs1[19:15], funct3[14:12] = bits [24:12] = mask 0x1FFF000.
        let new_br = encode_branch_imm(rel) | (instr & 0x1FFF000);
        let bytes = new_br.to_le_bytes();
        code[offset..offset+4].copy_from_slice(&bytes);
    }
    // else: not a branch, leave alone
}

/// Encode a JAL immediate (J-type format).
fn encode_jal_imm(imm: i32) -> u32 {
    // imm is a signed offset, must be multiple of 2
    let imm = imm as u32;
    let bit20 = (imm >> 20) & 0x1;
    let bits10_1 = (imm >> 1) & 0x3FF;
    let bit11 = (imm >> 11) & 0x1;
    let bits19_12 = (imm >> 12) & 0xFF;
    (bit20 << 31) | (bits10_1 << 21) | (bit11 << 20) | (bits19_12 << 12) | 0x6F // JAL opcode
}

/// Encode a branch immediate (B-type format).
fn encode_branch_imm(imm: i32) -> u32 {
    let imm = imm as u32;
    let bit12 = (imm >> 12) & 0x1;
    let bits10_5 = (imm >> 5) & 0x3F;
    let bits4_1 = (imm >> 1) & 0xF;
    let bit11 = (imm >> 11) & 0x1;
    (bit12 << 31) | (bits10_5 << 25) | (bits4_1 << 8) | (bit11 << 7) | 0x63 // BRANCH opcode
}

/// Map a PhysicalReg to a Gpr.
fn preg_to_gpr(preg: &PhysicalReg) -> Option<Gpr> {
    if preg.class != crate::backend::RegClass::Gpr {
        return None;
    }
    Gpr::from_encoding(preg.index)
}

/// Resolve an IRValue to a physical register or immediate.
fn resolve_value(val: &IRValue, alloc: &RegAllocResult) -> ResolvedVal {
    match val {
        IRValue::Register(vreg_id) => {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(gpr) = preg_to_gpr(preg) {
                    return ResolvedVal::Reg(gpr);
                }
            }
            ResolvedVal::Reg(Gpr::A0) // fallback (should not happen)
        }
        IRValue::Immediate(imm) => ResolvedVal::Imm(*imm),
        IRValue::Address(addr) => ResolvedVal::Imm(*addr as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Gpr::A0),
    }
}

/// Load a value into a register. If immediate, materialize via LUI+ADDI.
fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Gpr {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            // Use T6 as scratch for immediates.
            let scratch = Gpr::T6;
            emit_load_imm(code, scratch, imm);
            scratch
        }
    }
}

/// Materialize a 32-bit immediate into `rd` using LUI+ADDI.
/// On RV32, all immediates fit in 32 bits.
fn emit_load_imm(code: &mut Vec<u8>, rd: Gpr, imm: i64) {
    // Fast path: small immediates fit in a single ADDI.
    if imm >= -2048 && imm <= 2047 {
        code.extend_from_slice(&Instruction::Addi { rd, rs1: Gpr::Zero, imm: imm as i32 }.encode());
        return;
    }
    // For 32-bit range, use LUI+ADDI.
    let val = imm as i32;
    let upper = (val + 0x800) >> 12;
    let lower = val - (upper << 12);
    code.extend_from_slice(&Instruction::Lui { rd, imm: upper as u32 }.encode());
    if lower != 0 {
        code.extend_from_slice(&Instruction::Addi { rd, rs1: rd, imm: lower }.encode());
    }
}

/// Emit spill/reload code.
fn emit_spill_code(code: &mut Vec<u8>, spill: &GenericSpillCode) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // sd gpr, slot.offset(s0)
                code.extend_from_slice(&Instruction::Sw { rs1: Gpr::S0, rs2: gpr, imm: slot.offset }.encode());
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // ld gpr, slot.offset(s0)
                code.extend_from_slice(&Instruction::Lw { rd: gpr, rs1: Gpr::S0, imm: slot.offset }.encode());
            }
        }
    }
}

/// Build the function epilogue bytes: restore sp from s0 (frame pointer)
/// to undo any dynamic Alloc adjustments, then restore callee-saved, ra,
/// s0, ret. Used at every Return path.
///
/// We use `addi sp, s0, -frame_size` to restore sp to its post-prologue
/// value, because IRInstr::Alloc may have decremented sp further during
/// the function body. Without this, the epilogue's `ld` instructions
/// would read from the Alloc'd buffer (which overwrites the saved
/// callee-saved slots), loading garbage.
fn emit_epilogue_bytes(frame_size: i32, callee_saved_gprs: &[Gpr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + callee_saved_gprs.len() * 4);
    // Restore sp from s0: sp = s0 - frame_size.
    // s0 was set to (old_sp) in the prologue via `addi s0, sp, frame_size`.
    // So s0 - frame_size = old_sp - frame_size = post-prologue sp.
    out.extend_from_slice(&Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::S0, imm: -frame_size }.encode());
    // Now sp is at its post-prologue value, so the saved registers are
    // at positive offsets from sp.
    // Restore callee-saved (in reverse order of prologue save).
    let mut cs_offset = frame_size - 12;
    let mut saved_list: Vec<(Gpr, i32)> = Vec::new();
    for &g in callee_saved_gprs {
        saved_list.push((g, cs_offset));
        cs_offset -= 4;
    }
    for (g, off) in saved_list.iter().rev() {
        out.extend_from_slice(&Instruction::Lw { rd: *g, rs1: Gpr::Sp, imm: *off }.encode());
    }
    // lw ra, frame_size-4(sp)
    out.extend_from_slice(&Instruction::Lw { rd: Gpr::Ra, rs1: Gpr::Sp, imm: frame_size - 4 }.encode());
    // lw s0, frame_size-8(sp)
    out.extend_from_slice(&Instruction::Lw { rd: Gpr::S0, rs1: Gpr::Sp, imm: frame_size - 8 }.encode());
    // addi sp, sp, frame_size  — deallocate frame
    out.extend_from_slice(&Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: frame_size }.encode());
    // ret (jalr zero, ra, 0)
    out.extend_from_slice(&Instruction::Jalr { rd: Gpr::Zero, rs1: Gpr::Ra, imm: 0 }.encode());
    out
}

/// Emit a single IR instruction as register-based machine code.
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
        // ── Add ──
        IRInstr::Add { dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend_from_slice(&Instruction::Add { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode());
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    if imm >= -2048 && imm <= 2047 {
                        code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: lhs_reg, imm: imm as i32 }.encode());
                    } else {
                        let scratch = load_to_reg(rhs, alloc, code);
                        code.extend_from_slice(&Instruction::Add { rd: dst_reg, rs1: lhs_reg, rs2: scratch }.encode());
                    }
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "add".to_string()
        }

        // ── Sub ──
        IRInstr::Sub { dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode());
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    let scratch = Gpr::T6;
                    emit_load_imm(code, scratch, imm);
                    code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rs1: lhs_reg, rs2: scratch }.encode());
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "sub".to_string()
        }

        // ── Mul ──
        IRInstr::Mul { dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            code.extend_from_slice(&Instruction::Mul { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode());
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "mul".to_string()
        }

        // ── Div (standalone, from scg_to_ir) ──
        // Treated as unsigned division (UDiv). VUMA uses u32 types for most
        // arithmetic; FP types are redirected to the BinOp FP fallback.
        IRInstr::Div { dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            code.extend_from_slice(&Instruction::Divu { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode());
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "div".to_string()
        }

        // ── BinOp (Div/And/Or/Xor/Shifts) ──
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            // Use immediate form for ops that support it when rhs is a small
            // immediate. Avoids loading rhs into T6 scratch which would
            // clobber lhs if lhs was also an immediate loaded into T6.
            let rhs_val = resolve_value(rhs, alloc);
            let use_imm = match &rhs_val {
                ResolvedVal::Imm(imm) => *imm >= -2048 && *imm <= 2047,
                _ => false,
            };
            let rhs_reg = if use_imm { Gpr::Zero } else { load_to_reg(rhs, alloc, code) };
            match op {
                BinOpKind::SDiv => code.extend_from_slice(&Instruction::Div { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()),
                BinOpKind::UDiv => code.extend_from_slice(&Instruction::Divu { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()),
                BinOpKind::SRem => code.extend_from_slice(&Instruction::Rem { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()),
                BinOpKind::URem => code.extend_from_slice(&Instruction::Remu { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()),
                BinOpKind::And => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Andi { rd: dst_reg, rs1: lhs_reg, imm: imm as i32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::And { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::Or => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Ori { rd: dst_reg, rs1: lhs_reg, imm: imm as i32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::Or { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::Xor => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Xori { rd: dst_reg, rs1: lhs_reg, imm: imm as i32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::Xor { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::Shl => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Slli { rd: dst_reg, rs1: lhs_reg, shamt: (imm & 31) as u32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::Sll { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::ShrL => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Srli { rd: dst_reg, rs1: lhs_reg, shamt: (imm & 31) as u32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::Srl { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::ShrA => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Srai { rd: dst_reg, rs1: lhs_reg, shamt: (imm & 31) as u32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::Sra { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::Add => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: lhs_reg, imm: imm as i32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::Add { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::Sub => {
                    if let ResolvedVal::Imm(imm) = rhs_val { code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: lhs_reg, imm: (-imm) as i32 }.encode()) }
                    else { code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()) }
                }
                BinOpKind::Mul => code.extend_from_slice(&Instruction::Mul { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode()),
                _ => {
                    code.extend_from_slice(&Instruction::Add { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode());
                }
            }
            reads.push(phys(lhs_reg));
            if !use_imm { reads.push(phys(rhs_reg)); }
            writes.push(phys(dst_reg));
            "binop".to_string()
        }

        // ── UnaryOp ──
        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let src_reg = load_to_reg(operand, alloc, code);
            match op {
                UnaryOpKind::Neg => {
                    // sub rd, zero, src
                    code.extend_from_slice(&Instruction::Sub { rd: dst_reg, rs1: Gpr::Zero, rs2: src_reg }.encode());
                }
                UnaryOpKind::Not => {
                    // xori rd, src, -1
                    code.extend_from_slice(&Instruction::Xori { rd: dst_reg, rs1: src_reg, imm: -1 }.encode());
                }
                UnaryOpKind::Popcnt => {
                    // Not implemented in scalar RISC-V; emit a nop.
                    // TODO: use Zbb extension cpop once available.
                    code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: Gpr::Zero, imm: 0 }.encode());
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz => {
                    code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: Gpr::Zero, imm: 0 }.encode());
                }
            }
            reads.push(phys(src_reg));
            writes.push(phys(dst_reg));
            "unaryop".to_string()
        }

        // ── Load ──
        IRInstr::Load { dst, addr, offset, ty } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let off = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => {
                    if matches!(ty, IRType::I8) {
                        code.extend_from_slice(&Instruction::Lb { rd: dst_reg, rs1: base_reg, imm: off }.encode());
                    } else {
                        code.extend_from_slice(&Instruction::Lbu { rd: dst_reg, rs1: base_reg, imm: off }.encode());
                    }
                }
                IRType::U16 | IRType::I16 => {
                    if matches!(ty, IRType::I16) {
                        code.extend_from_slice(&Instruction::Lh { rd: dst_reg, rs1: base_reg, imm: off }.encode());
                    } else {
                        code.extend_from_slice(&Instruction::Lhu { rd: dst_reg, rs1: base_reg, imm: off }.encode());
                    }
                }
                IRType::U32 | IRType::I32 => {
                    if matches!(ty, IRType::I32) {
                        code.extend_from_slice(&Instruction::Lw { rd: dst_reg, rs1: base_reg, imm: off }.encode());
                    } else {
                        code.extend_from_slice(&Instruction::Lw { rd: dst_reg, rs1: base_reg, imm: off }.encode());
                    }
                }
                _ => {
                    code.extend_from_slice(&Instruction::Lw { rd: dst_reg, rs1: base_reg, imm: off }.encode());
                }
            }
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "load".to_string()
        }

        // ── Store ──
        IRInstr::Store { value, addr, offset, ty } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let off = *offset as i32;
            match ty {
                IRType::U8 | IRType::I8 => {
                    code.extend_from_slice(&Instruction::Sb { rs1: base_reg, rs2: val_reg, imm: off }.encode());
                }
                IRType::U16 | IRType::I16 => {
                    code.extend_from_slice(&Instruction::Sh { rs1: base_reg, rs2: val_reg, imm: off }.encode());
                }
                IRType::U32 | IRType::I32 => {
                    code.extend_from_slice(&Instruction::Sw { rs1: base_reg, rs2: val_reg, imm: off }.encode());
                }
                _ => {
                    code.extend_from_slice(&Instruction::Sw { rs1: base_reg, rs2: val_reg, imm: off }.encode());
                }
            }
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "store".to_string()
        }

        // ── Cmp ──
        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let cmp_code = emit_cmp_isel(kind, dst_reg, lhs_reg, rhs_reg, Gpr::T6);
            code.extend_from_slice(&cmp_code);
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "cmp".to_string()
        }

        // ── Select ──
        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            // mv dst, false_val; bne cond, zero, +8; mv dst, true_val
            code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: false_reg, imm: 0 }.encode()); // mv dst, false
            code.extend_from_slice(&Instruction::Bne { rs1: cond_reg, rs2: Gpr::Zero, offset: 8 }.encode()); // skip next if cond!=0
            code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: true_reg, imm: 0 }.encode()); // mv dst, true
            reads.push(phys(cond_reg));
            reads.push(phys(false_reg));
            reads.push(phys(true_reg));
            writes.push(phys(dst_reg));
            "select".to_string()
        }

        // ── CtSelect (same as Select on riscv64) ──
        IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: false_reg, imm: 0 }.encode());
            code.extend_from_slice(&Instruction::Bne { rs1: cond_reg, rs2: Gpr::Zero, offset: 8 }.encode());
            code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: true_reg, imm: 0 }.encode());
            reads.push(phys(cond_reg));
            reads.push(phys(false_reg));
            reads.push(phys(true_reg));
            writes.push(phys(dst_reg));
            "ct_select".to_string()
        }

        // ── CtEq ──
        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // xor dst, lhs, rhs; sltiu dst, dst, 1
            code.extend_from_slice(&Instruction::Xor { rd: dst_reg, rs1: lhs_reg, rs2: rhs_reg }.encode());
            code.extend_from_slice(&Instruction::Sltiu { rd: dst_reg, rs1: dst_reg, imm: 1 }.encode());
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "ct_eq".to_string()
        }

        // ── Cast ──
        IRInstr::Cast { kind, dst, src, from_ty, to_ty, .. } => {
            let src_reg = load_to_reg(src, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            match kind {
                CastKind::ZExt => {
                    match from_ty {
                        Some(IRType::U8) | Some(IRType::I8) => {
                            // andi dst, src, 0xFF
                            code.extend_from_slice(&Instruction::Andi { rd: dst_reg, rs1: src_reg, imm: 0xFF }.encode());
                        }
                        Some(IRType::U16) | Some(IRType::I16) => {
                            code.extend_from_slice(&Instruction::Andi { rd: dst_reg, rs1: src_reg, imm: 0xFFFF }.encode());
                        }
                        Some(IRType::U32) | Some(IRType::I32) => {
                            // On RV32, 32-bit values are native width — no zero-extension needed.
                            if src_reg != dst_reg {
                                code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: src_reg, imm: 0 }.encode());
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
                            // sign-extend byte: slli rd, src, 24; srai rd, rd, 24
                            code.extend_from_slice(&Instruction::Slli { rd: dst_reg, rs1: src_reg, shamt: 24 }.encode());
                            code.extend_from_slice(&Instruction::Srai { rd: dst_reg, rs1: dst_reg, shamt: 24 }.encode());
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            code.extend_from_slice(&Instruction::Slli { rd: dst_reg, rs1: src_reg, shamt: 16 }.encode());
                            code.extend_from_slice(&Instruction::Srai { rd: dst_reg, rs1: dst_reg, shamt: 16 }.encode());
                        }
                        Some(IRType::I32) | Some(IRType::U32) => {
                            // On RV32, 32-bit values are native width — no extension needed.
                            if src_reg != dst_reg {
                                code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: src_reg, imm: 0 }.encode());
                            }
                        }
                        _ => {
                            if src_reg != dst_reg {
                                code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: src_reg, imm: 0 }.encode());
                            }
                        }
                    }
                }
                CastKind::Trunc => {
                    if src_reg != dst_reg {
                        code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: src_reg, imm: 0 }.encode());
                    }
                    if let Some(tt) = to_ty {
                        match tt {
                            IRType::U8 | IRType::I8 => {
                                code.extend_from_slice(&Instruction::Andi { rd: dst_reg, rs1: dst_reg, imm: 0xFF }.encode());
                            }
                            IRType::U16 | IRType::I16 => {
                                code.extend_from_slice(&Instruction::Andi { rd: dst_reg, rs1: dst_reg, imm: 0xFFFF }.encode());
                            }
                            IRType::U32 | IRType::I32 => {
                                // On RV32, no truncation needed — registers are 32-bit.
                            }
                            _ => {}
                        }
                    }
                }
                CastKind::BitCast => {
                    if src_reg != dst_reg {
                        code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: src_reg, imm: 0 }.encode());
                    }
                }
                _ => {
                    if src_reg != dst_reg {
                        code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: src_reg, imm: 0 }.encode());
                    }
                }
            }
            reads.push(phys(src_reg));
            writes.push(phys(dst_reg));
            "cast".to_string()
        }

        // ── Alloc (stack allocation) ──
        IRInstr::Alloc { dst, size, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let aligned = ((*size as i32 + 15) & !15) as i32;
            // sub sp, sp, size FIRST, then mv dst, sp
            code.extend_from_slice(&Instruction::Addi { rd: Gpr::Sp, rs1: Gpr::Sp, imm: -aligned }.encode());
            code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: Gpr::Sp, imm: 0 }.encode()); // mv dst, sp
            writes.push(phys(dst_reg));
            "alloc".to_string()
        }

        // ── Free (stack deallocation) ──
        IRInstr::Free { ptr, .. } => {
            let _ = load_to_reg(ptr, alloc, code);
            // nop — stack reclaimed at function exit
            code.extend_from_slice(&Instruction::Addi { rd: Gpr::Zero, rs1: Gpr::Zero, imm: 0 }.encode());
            "free".to_string()
        }

        // ── GetAddress ──
        IRInstr::GetAddress { dst, name } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // auipc dst, 0 — placeholder, relocation will patch
            code.extend_from_slice(&Instruction::Auipc { rd: dst_reg, imm: 0 }.encode());
            let rel_offset = code.len() as u64 - 4;
            relocations.push(RelocationEntry {
                offset: rel_offset,
                symbol: name.clone(),
                reloc_type: "R_RISCV_PCREL_HI20".to_string(),
            });
            // addi dst, dst, 0 — paired with auipc for the full address
            code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: dst_reg, imm: 0 }.encode());
            let rel_offset2 = code.len() as u64 - 4;
            relocations.push(RelocationEntry {
                offset: rel_offset2,
                symbol: name.clone(),
                reloc_type: "R_RISCV_PCREL_LO12_I".to_string(),
            });
            writes.push(phys(dst_reg));
            "getaddr".to_string()
        }

        // ── Offset ──
        IRInstr::Offset { dst, base, offset, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(base, alloc, code);
            match resolve_value(offset, alloc) {
                ResolvedVal::Imm(imm) => {
                    if imm >= -2048 && imm <= 2047 {
                        code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: base_reg, imm: imm as i32 }.encode());
                    } else {
                        let scratch = Gpr::T6;
                        emit_load_imm(code, scratch, imm);
                        code.extend_from_slice(&Instruction::Add { rd: dst_reg, rs1: base_reg, rs2: scratch }.encode());
                    }
                }
                ResolvedVal::Reg(off_reg) => {
                    code.extend_from_slice(&Instruction::Add { rd: dst_reg, rs1: base_reg, rs2: off_reg }.encode());
                    reads.push(phys(off_reg));
                }
            }
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "offset".to_string()
        }

        // ── Phi ──
        IRInstr::Phi { dst, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&Instruction::Addi { rd: Gpr::Zero, rs1: Gpr::Zero, imm: 0 }.encode());
            writes.push(phys(dst_reg));
            "phi".to_string()
        }

        // ── Ret (mid-block return — emit full epilogue) ──
        // Note: IRInstr::Ret is rare; the common path is IRTerminator::Return
        // which calls emit_terminator -> emit_epilogue_bytes with the correct
        // frame_size. For IRInstr::Ret we don't have frame_size here, so we
        // emit a NOP — the function-level Return terminator will emit the
        // real epilogue. (If a mid-block Ret appears, it should be lowered
        // to a Branch to the return block before reaching reg_isel.)
        IRInstr::Ret { values } => {
            if let Some(first) = values.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::A0 {
                    code.extend_from_slice(&Instruction::Addi { rd: Gpr::A0, rs1: ret_reg, imm: 0 }.encode());
                }
            }
            code.extend_from_slice(&Instruction::Addi { rd: Gpr::Zero, rs1: Gpr::Zero, imm: 0 }.encode()); // nop
            "ret".to_string()
        }

        // ── Branch (unconditional) ──
        IRInstr::Branch { target } => {
            // jal zero, target (offset patched later)
            let offset_pos = all_code_offset(code);
            // Use jal x0, 0 — will be patched
            let instr = 0x6Fu32; // JAL x0, 0
            code.extend_from_slice(&instr.to_le_bytes());
            fixups.push(BranchFixup {
                offset: offset_pos,
                target: target.clone(),
            });
            "branch".to_string()
        }

        // ── CondBranch ──
        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            // bne cond, zero, true_target  (if cond != 0, jump to true)
            let offset_pos1 = all_code_offset(code);
            let bne_instr = encode_branch(1, cond_reg, Gpr::Zero, 0); // funct3=1 = BNE
            code.extend_from_slice(&bne_instr.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos1, target: true_target.clone() });
            // jal x0, false_target  (fall-through to false)
            let offset_pos2 = all_code_offset(code);
            let jal = 0x6Fu32; // JAL x0, 0
            code.extend_from_slice(&jal.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos2, target: false_target.clone() });
            reads.push(phys(cond_reg));
            "cond_branch".to_string()
        }

        // ── Syscall ──
        IRInstr::Syscall { nr, args, dst } => {
            // RISC-V syscall ABI: a7=nr, a0-a5=args, a0=return.
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::RiscV32,
                *nr,
            );
            code.extend_from_slice(&Instruction::Addi { rd: Gpr::A7, rs1: Gpr::Zero, imm: native_nr as i32 }.encode());
            let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5];
            for (i, arg) in args.iter().enumerate().take(6) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend_from_slice(&Instruction::Addi { rd: arg_regs[i], rs1: arg_reg, imm: 0 }.encode());
                }
            }
            code.extend_from_slice(&[0x73, 0x00, 0x00, 0x00]); // ecall
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Gpr::A0 {
                    code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: Gpr::A0, imm: 0 }.encode());
                }
                writes.push(phys(dst_reg));
            }
            "syscall".to_string()
        }

        // ── Call ──
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            let arg_regs = [Gpr::A0, Gpr::A1, Gpr::A2, Gpr::A3, Gpr::A4, Gpr::A5, Gpr::A6, Gpr::A7];
            for (i, arg) in args.iter().enumerate().take(8) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend_from_slice(&Instruction::Addi { rd: arg_regs[i], rs1: arg_reg, imm: 0 }.encode());
                }
            }
            // jal ra, 0 — placeholder, relocation will patch
            let offset_pos = all_code_offset(code);
            let jal_ra = 0xEFu32; // JAL ra, 0
            code.extend_from_slice(&jal_ra.to_le_bytes());
            relocations.push(RelocationEntry {
                offset: offset_pos as u64,
                symbol: fname.clone(),
                reloc_type: "R_RISCV_JAL".to_string(),
            });
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Gpr::A0 {
                    code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: Gpr::A0, imm: 0 }.encode());
                }
                writes.push(phys(dst_reg));
            }
            if *is_extern { "call_extern".to_string() } else { "call".to_string() }
        }

        // ── AtomicLoad ──
        IRInstr::AtomicLoad { dst, addr, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            code.extend_from_slice(&Instruction::Lw { rd: dst_reg, rs1: base_reg, imm: 0 }.encode());
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "atomic_load".to_string()
        }

        // ── AtomicStore ──
        IRInstr::AtomicStore { value, addr, .. } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            code.extend_from_slice(&Instruction::Sw { rs1: base_reg, rs2: val_reg, imm: 0 }.encode());
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "atomic_store".to_string()
        }

        // ── AtomicCas ──
        IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
            // RISC-V LR/SC sequence:
            //   lr.d t6, [addr]
            //   bne t6, expected, fail
            //   sc.d t6, desired, [addr]
            //   bnez t6, retry (or fail)
            // fail: mv dst, t6 (return loaded value)
            let expected_reg = load_to_reg(expected, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let new_reg = load_to_reg(desired, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // lr.d t6, [base]  (0x100027ab with rd=t6, rs1=base)
            let lr_d = encode_atomic_lr(Gpr::T6, base_reg);
            code.extend_from_slice(&lr_d.to_le_bytes());
            // bne t6, expected, +12 (skip sc and exit)
            code.extend_from_slice(&encode_branch(0, Gpr::T6, expected_reg, 12).to_le_bytes());
            // sc.d t6, new_reg, [base]
            let sc_d = encode_atomic_sc(Gpr::T6, base_reg, new_reg);
            code.extend_from_slice(&sc_d.to_le_bytes());
            // mv dst, t6 (loaded value)
            code.extend_from_slice(&Instruction::Addi { rd: dst_reg, rs1: Gpr::T6, imm: 0 }.encode());
            reads.push(phys(expected_reg));
            reads.push(phys(base_reg));
            reads.push(phys(new_reg));
            writes.push(phys(dst_reg));
            "atomic_cas".to_string()
        }

        // ── Unhandled ──
        _ => {
            code.extend_from_slice(&Instruction::Addi { rd: Gpr::Zero, rs1: Gpr::Zero, imm: 0 }.encode());
            "unhandled".to_string()
        }
    };

    Ok((opcode, reads, writes))
}

/// Emit a terminator (Jump, Branch, Return).
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
            let offset_pos = all_code_offset(code);
            let jal = 0x6Fu32; // JAL x0, 0
            code.extend_from_slice(&jal.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos, target: label.clone() });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            // bne cond, zero, true_block
            let offset_pos1 = all_code_offset(code);
            let bne = encode_branch(1, cond_reg, Gpr::Zero, 0); // funct3=1 for BNE
            code.extend_from_slice(&bne.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos1, target: true_block.clone() });
            // jal x0, false_block
            let offset_pos2 = all_code_offset(code);
            let jal = 0x6Fu32;
            code.extend_from_slice(&jal.to_le_bytes());
            fixups.push(BranchFixup { offset: offset_pos2, target: false_block.clone() });
        }
        IRTerminator::Return(vals) => {
            if let Some(first) = vals.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::A0 {
                    code.extend_from_slice(&Instruction::Addi { rd: Gpr::A0, rs1: ret_reg, imm: 0 }.encode());
                }
            }
            code.extend(emit_epilogue_bytes(frame_size, callee_saved_gprs));
        }
        IRTerminator::Unreachable => {
            code.extend_from_slice(&[0x73, 0x10, 0x00, 0x00]); // ebreak
        }
        _ => {
            code.extend_from_slice(&Instruction::Addi { rd: Gpr::Zero, rs1: Gpr::Zero, imm: 0 }.encode());
        }
    }
}

/// Helper: get the current code offset (length).
fn all_code_offset(code: &[u8]) -> usize {
    code.len()
}

/// Encode a branch instruction (B-type).
/// funct3: 0=BEQ, 1=BNE, 4=BLT, 5=BGE, 6=BLTU, 7=BGEU
fn encode_branch(funct3: u32, rs1: Gpr, rs2: Gpr, imm: i32) -> u32 {
    let imm = imm as u32;
    let bit12 = (imm >> 12) & 0x1;
    let bits10_5 = (imm >> 5) & 0x3F;
    let bits4_1 = (imm >> 1) & 0xF;
    let bit11 = (imm >> 11) & 0x1;
    (bit12 << 31) | (bits10_5 << 25) | (rs2.encoding() << 20) | (rs1.encoding() << 15)
        | (funct3 << 12) | (bits4_1 << 8) | (bit11 << 7) | 0x63
}

/// Encode LR.D (load-reserved doubleword).
fn encode_atomic_lr(rd: Gpr, rs1: Gpr) -> u32 {
    // opcode=0x2F (AMO), funct3=0x3 (64-bit), funct5=0x02 (LR)
    0x2F | (0x3 << 12) | (rs1.encoding() << 15) | (rd.encoding() << 7)
        | (0x02 << 27) // rs2=0 for LR
}

/// Encode SC.D (store-conditional doubleword).
fn encode_atomic_sc(rd: Gpr, rs1: Gpr, rs2: Gpr) -> u32 {
    0x2F | (0x3 << 12) | (rs1.encoding() << 15) | (rd.encoding() << 7)
        | (0x03 << 27) | (rs2.encoding() << 20)
}

/// Helper: create a PhysicalReg from a Gpr.
fn phys(g: Gpr) -> PhysicalReg {
    PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32)
}

/// FP fallback.
fn emit_fp_fallback(
    instr: &IRInstr,
) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed {
        isa: "riscv64",
        reason: format!("FP instruction not yet supported in register-based emitter: {:?}", instr),
    })
}
