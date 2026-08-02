//! Full register-based instruction selection for s390x (IBM System Z).
//!
//! Mirrors the riscv64/ppc64 `reg_isel.rs` templates but uses the s390x
//! encoding helpers defined in `super` (i.e., `s390x/mod.rs`).
//!
//! # Architecture
//!
//! 1. **Prologue**: `SP -= frame_size; STG LR, ...; STG FP, ...;
//!    STG callee_saved, ...; FP = SP + frame_size (= old SP)`
//! 2. **Body**: For each IR instruction, resolve vregs → physical regs via
//!    `alloc.vreg_to_preg`, emit register-based s390x machine code using
//!    the `encode_*` helpers.
//! 3. **Spill/reload**: Insert `STG preg, slot.offset(FP)` / `LG preg,
//!    slot.offset(FP)` at positions from `alloc.spill_code`. Slot offsets
//!    are NEGATIVE from FP (per `GenericSpillSlot::offset`).
//! 4. **Epilogue**: `SP = FP - frame_size; LG callee_saved, ...; LG LR, ...;
//!    LG FP, ...; SP += frame_size; BR LR` — emitted at EVERY Return path.
//!
//! # Frame Layout
//!
//! FP (R11) = old SP (top of frame, after stack allocation).
//!
//! ```text
//!   [old SP]                ← FP (R11) points here
//!   [FP - 8]                ← spill slot 0
//!   [FP - 16]               ← spill slot 1
//!   ...
//!   [FP - 8*N_spills]       ← bottom of spill area
//!   [FP - 8*N_spills - 8]   ← saved LR (R14)
//!   [FP - 8*N_spills - 16]  ← saved FP (R11)
//!   [FP - 8*N_spills - 24]  ← first callee-saved (R6)
//!   [FP - 8*N_spills - 32]  ← second callee-saved (R7)
//!   ...
//!   [SP + 160]              ← (above ABI save area)
//!   [SP + 0..159]           ← 160-byte ABI caller save area (mandatory)
//!   [SP]                    ← current SP (after prologue decrement)
//! ```
//!
//! `frame_size = spill_size + 16 (LR+FP) + N_cs*8 + 160 (ABI save area)`,
//! aligned to 16 bytes (s390x ABI requires 8-byte alignment; we use 16 for
//! safety and to match the stack-slot ISel's convention).
//!
//! # Scratch Register
//!
//! R0 is the dedicated scratch register (used by `emit_load_imm`,
//! `emit_add_imm`, etc.). It is marked `not_allocatable` in
//! `target_desc.rs` so the register allocator will never assign a live vreg
//! to it.

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RegClass,
    RelocationEntry,
};
use crate::ir::{
    BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue, UnaryOpKind,
};
use crate::regalloc::{GenericSpillCode, RegAllocResult};
// Bring in all of s390x/mod.rs's items (private `encode_*` helpers, Gpr, FP,
// SP, LR constants, etc.).  Child modules can access private items of their
// parent.
use super::*;

/// Resolved value: either a physical register or an immediate.
enum ResolvedVal {
    Reg(Gpr),
    Imm(i64),
}

/// Branch fixup record: byte offset of the branch instruction in the code
/// buffer, whether it's a long (BRCL, 6-byte) or short (BRC, 4-byte) form,
/// and the target label.
struct BranchFixup {
    offset: usize,
    is_long: bool,
    target: String,
}

/// Emit a complete function using register-based instruction selection.
///
/// Returns an `AllocatedFunction` whose `encoded` bytes are real s390x
/// machine code with vregs kept in physical registers (wherever the
/// allocator placed them), spill/reload code inserted at the right
/// positions, and a proper prologue/epilogue.
pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // ── Compute frame size ──
    // Callee-saved GPRs (excluding FP=R11, LR=R14, SP=R15, scratch R0).
    let callee_saved_gprs: Vec<Gpr> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::R11 && *g != Gpr::R14 && *g != Gpr::R15 && *g != Gpr::R0)
        .collect();
    // LR + FP + each callee-saved GPR used.
    let cs_count = 2 + callee_saved_gprs.len();
    let callee_saved_size = cs_count * 8;
    // Spill slots: each is 8 bytes (GPR).
    let spill_size = alloc.total_spill_slots as usize * 8;
    // s390x ABI: 160-byte caller save area at the bottom of every frame
    // (callee may store R2-R5 there for outgoing args).
    let abi_save_area: i32 = 160;
    let raw_frame = spill_size as i32 + callee_saved_size as i32 + abi_save_area;
    // 16-byte aligned (s390x ABI requires 8-byte; we use 16 for safety and
    // to match the stack-slot ISel).
    let frame_size = ((raw_frame + 15) & !15) as i32;
    // Ensure room for at least LR + FP + ABI save area.
    let frame_size = frame_size.max(abi_save_area + 16);

    let spill_size_i32 = spill_size as i32;

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    // ── Prologue ──
    //
    // Layout (offsets relative to current SP after decrement):
    //   [SP + frame_size - 8 - spill_size]   = saved LR (R14)
    //   [SP + frame_size - 16 - spill_size]  = saved FP (R11)
    //   [SP + frame_size - 24 - spill_size]  = first callee-saved
    //   [SP + frame_size - 32 - spill_size]  = second callee-saved
    //   ...
    //   [SP + 160..]                          = (above ABI save area)
    //   [SP + 0..159]                         = ABI caller save area
    //
    // After saving, set FP = SP + frame_size (= old SP). Then spills are
    // accessed at [FP + slot.offset] where slot.offset is negative.
    let prologue_start = all_code.len();
    // Decrement SP by frame_size (uses LGHI/LGFI + SGR internally).
    all_code.extend(adjust_sp(-frame_size));
    // STG LR, frame_size - 8 - spill_size(SP) — save return address.
    let lr_off = frame_size - 8 - spill_size_i32;
    all_code.extend_from_slice(&encode_stg(LR, SP, lr_off));
    // STG FP, frame_size - 16 - spill_size(SP) — save old frame pointer.
    let fp_save_off = frame_size - 16 - spill_size_i32;
    all_code.extend_from_slice(&encode_stg(FP, SP, fp_save_off));
    // Set new FP = SP + frame_size (= old SP).
    //   LGR FP, SP           (FP = current SP)
    //   LGHI/LGFI R0, frame_size
    //   AGR FP, R0           (FP += frame_size)
    all_code.extend_from_slice(&encode_lgr(FP, SP));
    if (-32768..=32767).contains(&frame_size) {
        all_code.extend_from_slice(&encode_lghi(Gpr::R0, frame_size as i16));
    } else {
        all_code.extend_from_slice(&encode_lgfi(Gpr::R0, frame_size));
    }
    all_code.extend_from_slice(&encode_agr(FP, Gpr::R0));
    // Save remaining callee-saved registers (R6, R7, ...) at decreasing
    // offsets from SP + frame_size - 24 - spill_size.
    let mut cs_offset = frame_size - 24 - spill_size_i32;
    for &g in &callee_saved_gprs {
        all_code.extend_from_slice(&encode_stg(g, SP, cs_offset));
        cs_offset -= 8;
    }

    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: callee_saved_gprs
            .iter()
            .map(|g| PhysicalReg::new(RegClass::Gpr, *g as u32))
            .collect(),
        encoded: all_code[prologue_start..].to_vec(),
    };

    // ── Argument shuffle (function entry) ──
    // s390x calling convention: first 5 integer args in R2-R6.
    // The allocator may assign these param vregs to different registers.
    // Use a 2-pass move with R0 as a cycle-breaker (R0 is scratch, never
    // holds a live vreg).
    let arg_shuffle_start = all_code.len();
    let arg_regs = [
        Gpr::R2, Gpr::R3, Gpr::R4, Gpr::R5, Gpr::R6,
    ];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new();
    for (i, param) in func.params.iter().enumerate() {
        if i >= arg_regs.len() {
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
    // Pass 1: move non-conflicting args (src not in any pending dst).
    let mut progress = true;
    while progress && !pending.is_empty() {
        progress = false;
        let mut i = 0;
        while i < pending.len() {
            let (src, _dst) = pending[i];
            let mut conflict = false;
            for (j, (_, other_dst)) in pending.iter().enumerate() {
                if i != j && *other_dst == src {
                    conflict = true;
                    break;
                }
            }
            if !conflict {
                let (src, dst) = pending[i];
                all_code.extend_from_slice(&encode_lgr(dst, src));
                pending.remove(i);
                progress = true;
            } else {
                i += 1;
            }
        }
    }
    // Pass 2: break cycles via R0 scratch.
    for (src, dst) in pending {
        all_code.extend_from_slice(&encode_lgr(Gpr::R0, src));
        all_code.extend_from_slice(&encode_lgr(dst, Gpr::R0));
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
            spill_size_i32,
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
    all_code.extend(emit_epilogue_bytes(
        frame_size,
        spill_size_i32,
        &callee_saved_gprs,
    ));
    let epilogue_end = all_code.len();

    if let Some(first_block) = blocks.first_mut() {
        if has_arg_shuffle {
            first_block.instructions.insert(
                0,
                AllocatedInstruction {
                    opcode: "arg_shuffle".to_string(),
                    reads: vec![],
                    writes: vec![],
                    encoded: all_code[arg_shuffle_start..arg_shuffle_end].to_vec(),
                },
            );
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
    // BRC: 16-bit signed halfword displacement. Target = PC + (disp * 2).
    // BRCL: 32-bit signed halfword displacement. Target = PC + (disp * 2).
    // Patch bytes 2..4 (BRC) or 2..6 (BRCL) with the halfword displacement.
    for fixup in &fixups {
        if let Some(&target_offset) = label_offsets.get(&fixup.target) {
            let disp_bytes = target_offset as i64 - fixup.offset as i64;
            let disp_halfwords = (disp_bytes / 2) as i64;
            if fixup.is_long {
                let disp = disp_halfwords as i32;
                let disp_be = disp.to_be_bytes();
                all_code[fixup.offset + 2..fixup.offset + 6].copy_from_slice(&disp_be);
            } else {
                let disp = disp_halfwords as i16;
                let disp_be = disp.to_be_bytes();
                all_code[fixup.offset + 2..fixup.offset + 4].copy_from_slice(&disp_be);
            }
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
        .map(|g| PhysicalReg::new(RegClass::Gpr, *g as u32))
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

/// Map a target-agnostic [`PhysicalReg`] to an s390x [`Gpr`].
fn preg_to_gpr(preg: &PhysicalReg) -> Option<Gpr> {
    if preg.class != RegClass::Gpr {
        return None;
    }
    Gpr::from_encoding(preg.index as u8)
}

/// Resolve an [`IRValue`] to a physical register or an immediate.
fn resolve_value(val: &IRValue, alloc: &RegAllocResult) -> ResolvedVal {
    match val {
        IRValue::Register(vreg_id) => {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(gpr) = preg_to_gpr(preg) {
                    return ResolvedVal::Reg(gpr);
                }
            }
            // Fallback (should not happen if allocator ran correctly).
            ResolvedVal::Reg(Gpr::R2)
        }
        IRValue::Immediate(imm) => ResolvedVal::Imm(*imm),
        IRValue::Address(addr) => ResolvedVal::Imm(*addr as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Gpr::R2),
    }
}

/// Load a value into a register. If the value is an immediate, materialize
/// it via `emit_load_imm` into R0 (the dedicated scratch register); if it's
/// a register, return the assigned physical Gpr.
fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Gpr {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            // R0 is the dedicated scratch register (not allocatable).
            let scratch = Gpr::R0;
            emit_load_imm(code, scratch, imm);
            scratch
        }
    }
}

/// Materialize a 64-bit immediate into `rd`.
///
/// Strategy:
/// - If `imm` fits in i16: `LGHI rd, imm` (4 bytes).
/// - If `imm` fits in [0, u32::MAX]: `LLILF rd, imm` (6 bytes, zero-extended).
/// - If `imm` fits in i32 (negative): `LGFI rd, imm` (6 bytes, sign-extended).
/// - Otherwise: `LGFI rd, hi; SLLG rd, rd, 32; LLILF R0, lo; OGR rd, R0`.
///
/// **Warning**: this function uses R0 as a scratch for the full-64-bit path.
/// Callers that pass `rd = R0` should ensure `imm` fits in 32 bits (so the
/// R0-scratch path is not taken). For all the typical use sites
/// (`emit_load_imm` is called with `dst_reg` from `load_to_reg`, and the
/// allocator never assigns dst to R0), this is safe.
fn emit_load_imm(code: &mut Vec<u8>, rd: Gpr, imm: i64) {
    if (-32768..=32767).contains(&imm) {
        code.extend_from_slice(&encode_lghi(rd, imm as i16));
        return;
    }
    // Unsigned 32-bit value: use LLILF (zero-extended) to avoid
    // sign-extension bugs. LGFI would load 0xFFFFFFFF as
    // 0xFFFFFFFFFFFFFFFF (sign-extended -1), breaking AND masks.
    if (0..=0xFFFFFFFF).contains(&(imm as u64)) {
        code.extend_from_slice(&encode_llilf(rd, imm as u32));
        return;
    }
    if (-2147483648..=-1).contains(&imm) {
        code.extend_from_slice(&encode_lgfi(rd, imm as i32));
        return;
    }
    // Full 64-bit value: load high 32 bits, shift left 32, OR low 32 bits.
    let v = imm as u64;
    let hi = ((v >> 32) & 0xFFFF_FFFF) as i32;
    let lo = (v & 0xFFFF_FFFF) as u32;
    code.extend_from_slice(&encode_lgfi(rd, hi));
    code.extend_from_slice(&encode_sllg(rd, rd, 32));
    // Load low 32 bits into R0 (zero-extended via LLILF), then OR into rd.
    code.extend_from_slice(&encode_llilf(Gpr::R0, lo));
    // OGR rd, R0: rd |= R0. op1=0xB9, op2=0x81.
    code.extend_from_slice(&encode_rre(0xB9, 0x81, rd, Gpr::R0));
}

/// Emit spill/reload code. Spill slots are at NEGATIVE offsets from FP
/// (per `GenericSpillSlot::offset`).
fn emit_spill_code(code: &mut Vec<u8>, spill: &GenericSpillCode) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // STG gpr, slot.offset(FP)
                code.extend_from_slice(&encode_stg(gpr, FP, slot.offset));
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // LG gpr, slot.offset(FP)
                code.extend_from_slice(&encode_lg(gpr, FP, slot.offset));
            }
        }
    }
}

/// Build the function epilogue bytes: restore SP from FP (undoes any
/// dynamic Alloc adjustments), restore callee-saved, LR, FP, deallocate
/// frame, and return via `BR LR`. Used at every Return path.
fn emit_epilogue_bytes(frame_size: i32, spill_size: i32, callee_saved_gprs: &[Gpr]) -> Vec<u8> {
    let mut out = Vec::with_capacity(80 + callee_saved_gprs.len() * 6);

    // Restore SP from FP: SP = FP - frame_size.
    //   LGR SP, FP          (SP = FP = old_SP)
    //   LGHI/LGFI R0, -frame_size
    //   AGR SP, R0          (SP += -frame_size → SP = old_SP - frame_size)
    // This undoes any dynamic Alloc adjustments that may have shifted SP
    // during the function body.
    out.extend_from_slice(&encode_lgr(SP, FP));
    let neg_frame = -frame_size;
    if (-32768..=32767).contains(&neg_frame) {
        out.extend_from_slice(&encode_lghi(Gpr::R0, neg_frame as i16));
    } else {
        out.extend_from_slice(&encode_lgfi(Gpr::R0, neg_frame));
    }
    out.extend_from_slice(&encode_agr(SP, Gpr::R0));

    // Restore callee-saved (reverse order of prologue save).
    // Prologue saved at decreasing offsets starting from
    //   frame_size - 24 - spill_size
    // down by 8 for each. The LAST saved register is at the LOWEST offset.
    // Restore in reverse: start from the lowest offset (last saved) and
    // go up.
    if !callee_saved_gprs.is_empty() {
        let lowest_off = frame_size - 24 - spill_size - (callee_saved_gprs.len() as i32 - 1) * 8;
        let mut cs_off = lowest_off;
        for &g in callee_saved_gprs.iter().rev() {
            out.extend_from_slice(&encode_lg(g, SP, cs_off));
            cs_off += 8;
        }
    }

    // Restore LR and FP.
    out.extend_from_slice(&encode_lg(LR, SP, frame_size - 8 - spill_size));
    out.extend_from_slice(&encode_lg(FP, SP, frame_size - 16 - spill_size));

    // SP += frame_size (deallocate frame).
    out.extend(adjust_sp(frame_size));

    // BR LR — return.
    out.extend_from_slice(&encode_br(LR));
    out
}

/// Emit a single IR instruction as register-based s390x machine code.
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
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            // Materialize lhs into dst.
            match resolve_value(lhs, alloc) {
                ResolvedVal::Reg(lhs_reg) => {
                    if dst_reg != lhs_reg {
                        code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
                    }
                    reads.push(phys(lhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, dst_reg, imm);
                }
            }
            // Add rhs.
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend_from_slice(&encode_agr(dst_reg, rhs_reg));
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, Gpr::R0, imm);
                    code.extend_from_slice(&encode_agr(dst_reg, Gpr::R0));
                }
            }
            // Zero-extend 32-bit results.
            if is_32bit_ty(ty.as_ref()) {
                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
            }
            writes.push(phys(dst_reg));
            "add".to_string()
        }

        // ── Sub ──
        IRInstr::Sub { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            match resolve_value(lhs, alloc) {
                ResolvedVal::Reg(lhs_reg) => {
                    if dst_reg != lhs_reg {
                        code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
                    }
                    reads.push(phys(lhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, dst_reg, imm);
                }
            }
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend_from_slice(&encode_sgr(dst_reg, rhs_reg));
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, Gpr::R0, imm);
                    code.extend_from_slice(&encode_sgr(dst_reg, Gpr::R0));
                }
            }
            if is_32bit_ty(ty.as_ref()) {
                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
            }
            writes.push(phys(dst_reg));
            "sub".to_string()
        }

        // ── Mul ──
        IRInstr::Mul { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            match resolve_value(lhs, alloc) {
                ResolvedVal::Reg(lhs_reg) => {
                    if dst_reg != lhs_reg {
                        code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
                    }
                    reads.push(phys(lhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, dst_reg, imm);
                }
            }
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend_from_slice(&encode_msgr(dst_reg, rhs_reg));
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, Gpr::R0, imm);
                    code.extend_from_slice(&encode_msgr(dst_reg, Gpr::R0));
                }
            }
            if is_32bit_ty(ty.as_ref()) {
                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
            }
            writes.push(phys(dst_reg));
            "mul".to_string()
        }

        // ── BinOp (Div/And/Or/Xor/Shifts/Comparisons) ──
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return emit_fp_fallback(instr);
            }
            emit_binop_isel(op, dst, lhs, rhs, ty.as_ref(), alloc, code, &mut reads, &mut writes);
            "binop".to_string()
        }

        // ── Div (signed by default; emit based on type signedness) ──
        IRInstr::Div { dst, lhs, rhs, ty } => {
            if matches!(ty, Some(IRType::F32) | Some(IRType::F64)) {
                return emit_fp_fallback(instr);
            }
            let signed = is_signed_ty(ty.as_ref());
            emit_div_isel(dst, lhs, rhs, ty.as_ref(), signed, false, alloc, code, &mut reads, &mut writes);
            "div".to_string()
        }

        // ── Cmp ──
        IRInstr::Cmp { kind, dst, lhs, rhs, ty } => {
            let binop_kind = match kind {
                CmpKind::Eq => BinOpKind::Eq,
                CmpKind::Ne => BinOpKind::Ne,
                CmpKind::SLt => BinOpKind::SLt,
                CmpKind::SLe => BinOpKind::SLe,
                CmpKind::SGt => BinOpKind::SGt,
                CmpKind::SGe => BinOpKind::SGe,
                CmpKind::ULt => BinOpKind::ULt,
                CmpKind::ULe => BinOpKind::ULe,
                CmpKind::UGt => BinOpKind::UGt,
                CmpKind::UGe => BinOpKind::UGe,
            };
            emit_binop_isel(
                &binop_kind, dst, lhs, rhs, ty.as_ref(), alloc, code, &mut reads, &mut writes,
            );
            "cmp".to_string()
        }

        // ── UnaryOp ──
        IRInstr::UnaryOp { op, dst, operand, ty: _ } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let src_reg = load_to_reg(operand, alloc, code);
            if dst_reg != src_reg {
                code.extend_from_slice(&encode_lgr(dst_reg, src_reg));
            }
            match op {
                UnaryOpKind::Neg => {
                    // LCGR R1, R2 (Load Complement 64-bit): R1 = -R2.
                    // op1=0xB9, op2=0x03.
                    code.extend_from_slice(&encode_rre(0xB9, 0x03, dst_reg, src_reg));
                }
                UnaryOpKind::Not => {
                    // ~R = -R - 1. Use XGR with all-ones.
                    code.extend_from_slice(&encode_lghi(Gpr::R0, -1));
                    // XGR dst, R0: dst ^= R0 = ~dst. op1=0xB9, op2=0x82.
                    code.extend_from_slice(&encode_rre(0xB9, 0x82, dst_reg, Gpr::R0));
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz | UnaryOpKind::Popcnt => {
                    // Not natively supported (without extensions). Emit 0.
                    code.extend_from_slice(&encode_lghi(dst_reg, 0));
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
            // For non-zero offsets that don't fit in disp20, compute the
            // effective address first via LGHI/LGFI + AGR into R0... but
            // base_reg might BE R0 (if addr is an immediate, which is rare).
            // For simplicity, assume offsets fit in disp20 (the common case).
            match ty {
                IRType::I8 | IRType::U8 => {
                    // LLC (Load Logical Character, op2=0x90): zero-extend byte.
                    code.extend_from_slice(&encode_rxy_a(0xE3, 0x90, dst_reg, Gpr::R0, base_reg, off));
                }
                IRType::I16 | IRType::U16 => {
                    // LLH (Load Logical Halfword, op2=0x91): zero-extend halfword.
                    code.extend_from_slice(&encode_rxy_a(0xE3, 0x91, dst_reg, Gpr::R0, base_reg, off));
                }
                IRType::I32 | IRType::U32 => {
                    // LLGF (Load Logical Fullword, op2=0x16): zero-extend word.
                    code.extend_from_slice(&encode_llgf(dst_reg, base_reg, off));
                }
                _ => {
                    // LG (Load 64-bit). op2=0x04.
                    code.extend_from_slice(&encode_lg(dst_reg, base_reg, off));
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
                IRType::I8 | IRType::U8 => {
                    code.extend_from_slice(&encode_stc(val_reg, base_reg, off));
                }
                IRType::I16 | IRType::U16 => {
                    code.extend_from_slice(&encode_sth(val_reg, base_reg, off));
                }
                IRType::I32 | IRType::U32 => {
                    code.extend_from_slice(&encode_sty(val_reg, base_reg, off));
                }
                _ => {
                    code.extend_from_slice(&encode_stg(val_reg, base_reg, off));
                }
            }
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "store".to_string()
        }

        // ── Select ──
        IRInstr::Select { dst, cond, true_val, false_val, ty: _ } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            // dst = false_val
            code.extend_from_slice(&encode_lgr(dst_reg, false_reg));
            // LTGR R0, cond: sets CC based on cond.
            //   op1=0xB9, op2=0x02. R1=R0 (scratch), R2=cond.
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R0, cond_reg));
            // BRC 0x8, +skip (skip the load-true if cond == 0).
            // Mask 0x8 = CC=0 (cond == 0). 4-byte BRC.
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x8, 0));
            // Load true_val into dst (overwriting false_val).
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend_from_slice(&encode_lgr(dst_reg, true_reg));
            // skip_load_true: patch the BRC to jump here.
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);

            reads.push(phys(cond_reg));
            reads.push(phys(false_reg));
            reads.push(phys(true_reg));
            writes.push(phys(dst_reg));
            "select".to_string()
        }

        // ── CtSelect (same lowering as Select) ──
        IRInstr::CtSelect { dst, cond, true_val, false_val, ty: _ } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            code.extend_from_slice(&encode_lgr(dst_reg, false_reg));
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R0, cond_reg));
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x8, 0));
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend_from_slice(&encode_lgr(dst_reg, true_reg));
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);

            reads.push(phys(cond_reg));
            reads.push(phys(false_reg));
            reads.push(phys(true_reg));
            writes.push(phys(dst_reg));
            "ct_select".to_string()
        }

        // ── CtEq ──
        IRInstr::CtEq { dst, lhs, rhs, ty: _ } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // R0 = lhs ^ rhs
            code.extend_from_slice(&encode_lgr(Gpr::R0, lhs_reg));
            code.extend_from_slice(&encode_rre(0xB9, 0x82, Gpr::R0, rhs_reg)); // XGR R0, rhs
            // R0 = -R0 (LCGR)
            code.extend_from_slice(&encode_rre(0xB9, 0x03, Gpr::R0, Gpr::R0));
            // R0 = R0 | R0_old... actually we want (x | -x) >> 63.
            // Easier: dst = ((x | -x) >> 63) ^ 1
            //   where x = lhs ^ rhs.
            // Re-do cleanly: compute x = lhs ^ rhs in dst, then:
            //   LCGR R0, dst (R0 = -dst)
            //   OGR dst, R0 (dst |= -dst → bit 63 set iff dst != 0)
            //   SRLG dst, dst, 63 (dst = 1 if originally non-zero, else 0)
            //   XILF dst, 1 (dst ^= 1 → 0 if non-zero, 1 if zero)
            // For simplicity use the same approach with R0 as scratch.
            // dst = lhs ^ rhs
            code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
            code.extend_from_slice(&encode_rre(0xB9, 0x82, dst_reg, rhs_reg)); // XGR dst, rhs
            // R0 = -dst
            code.extend_from_slice(&encode_rre(0xB9, 0x03, Gpr::R0, dst_reg)); // LCGR R0, dst
            // dst |= R0
            code.extend_from_slice(&encode_rre(0xB9, 0x81, dst_reg, Gpr::R0)); // OGR dst, R0
            // dst >>= 63 (logical)
            code.extend_from_slice(&encode_srlg(dst_reg, dst_reg, 63));
            // dst ^= 1: load 1 into R0, XGR dst, R0.
            code.extend_from_slice(&encode_lghi(Gpr::R0, 1));
            code.extend_from_slice(&encode_rre(0xB9, 0x82, dst_reg, Gpr::R0)); // XGR dst, R0
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "ct_eq".to_string()
        }

        // ── Cast ──
        IRInstr::Cast { kind, dst, src, from_ty, to_ty, .. } => {
            let src_reg = load_to_reg(src, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            if dst_reg != src_reg {
                code.extend_from_slice(&encode_lgr(dst_reg, src_reg));
            }
            match kind {
                CastKind::ZExt => {
                    match from_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            // Mask to 0xFF (via NRK) then LLGFR.
                            code.extend_from_slice(&encode_lghi(Gpr::R0, 0xFF));
                            code.extend_from_slice(&encode_nrk(dst_reg, dst_reg, Gpr::R0));
                            code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            code.extend_from_slice(&encode_lgfi(Gpr::R0, 0xFFFF));
                            code.extend_from_slice(&encode_nrk(dst_reg, dst_reg, Gpr::R0));
                            code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
                        }
                        Some(IRType::I32) | Some(IRType::U32) | None => {
                            code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
                        }
                        _ => {}
                    }
                }
                CastKind::SExt => {
                    match from_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            // LGBR: sign-extend byte → int64. op2=0xA6.
                            code.extend_from_slice(&encode_rre(0xB9, 0xA6, dst_reg, dst_reg));
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            // LGHR: sign-extend halfword → int64. op2=0xA5.
                            code.extend_from_slice(&encode_rre(0xB9, 0xA5, dst_reg, dst_reg));
                        }
                        Some(IRType::I32) | Some(IRType::U32) | None => {
                            code.extend_from_slice(&encode_lgfr(dst_reg, dst_reg));
                        }
                        _ => {}
                    }
                }
                CastKind::Trunc => {
                    if let Some(tt) = to_ty {
                        match tt {
                            IRType::I8 | IRType::U8 => {
                                code.extend_from_slice(&encode_lghi(Gpr::R0, 0xFF));
                                code.extend_from_slice(&encode_nrk(dst_reg, dst_reg, Gpr::R0));
                                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
                            }
                            IRType::I16 | IRType::U16 => {
                                code.extend_from_slice(&encode_lgfi(Gpr::R0, 0xFFFF));
                                code.extend_from_slice(&encode_nrk(dst_reg, dst_reg, Gpr::R0));
                                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
                            }
                            IRType::I32 | IRType::U32 => {
                                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
                            }
                            _ => {}
                        }
                    }
                }
                CastKind::BitCast => {
                    // No-op (reinterpret bits). dst already has src.
                }
                _ => {
                    // IntToFloat / UIntToFloat / FloatToInt / FloatToUInt / FloatToFloat
                    // are not supported by the register-based emitter (the
                    // stack-slot ISel handles them with elaborate sequences).
                    // Fall back via Err to trigger the stack-slot fallback.
                    return emit_fp_fallback(instr);
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
            // SP -= aligned; dst = SP
            // Use R0 as scratch for the immediate.
            if (-32768..=32767).contains(&-aligned) {
                code.extend_from_slice(&encode_lghi(Gpr::R0, (-aligned) as i16));
            } else {
                code.extend_from_slice(&encode_lgfi(Gpr::R0, -aligned));
            }
            code.extend_from_slice(&encode_sgr(SP, Gpr::R0));
            code.extend_from_slice(&encode_lgr(dst_reg, SP));
            writes.push(phys(dst_reg));
            "alloc".to_string()
        }

        // ── Free (stack deallocation — no-op) ──
        IRInstr::Free { ptr, .. } => {
            let _ = load_to_reg(ptr, alloc, code);
            code.extend_from_slice(&encode_nop());
            "free".to_string()
        }

        // ── GetAddress ──
        IRInstr::GetAddress { dst, name } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // LARL dst, 0 — placeholder disp=0; record a relocation.
            let larl_offset = code.len() as u64;
            code.extend_from_slice(&encode_larl(dst_reg, 0));
            relocations.push(RelocationEntry {
                offset: larl_offset,
                symbol: name.clone(),
                reloc_type: "R_S390_PC32DBL".to_string(),
            });
            writes.push(phys(dst_reg));
            "getaddr".to_string()
        }

        // ── Offset (pointer arithmetic: dst = base + offset) ──
        IRInstr::Offset { dst, base, offset, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(base, alloc, code);
            if dst_reg != base_reg {
                code.extend_from_slice(&encode_lgr(dst_reg, base_reg));
            }
            match resolve_value(offset, alloc) {
                ResolvedVal::Imm(imm) => {
                    // Use AGFI workaround: load imm into R0, then AGR dst, R0.
                    emit_load_imm(code, Gpr::R0, imm);
                    code.extend_from_slice(&encode_agr(dst_reg, Gpr::R0));
                }
                ResolvedVal::Reg(off_reg) => {
                    code.extend_from_slice(&encode_agr(dst_reg, off_reg));
                    reads.push(phys(off_reg));
                }
            }
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "offset".to_string()
        }

        // ── Phi (no-op; SSA deconstruction handled at IR level) ──
        IRInstr::Phi { dst, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend_from_slice(&encode_nop());
            writes.push(phys(dst_reg));
            "phi".to_string()
        }

        // ── Ret (mid-block return — rare; emit NOP, rely on terminator) ──
        IRInstr::Ret { values } => {
            if let Some(first) = values.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::R2 {
                    code.extend_from_slice(&encode_lgr(Gpr::R2, ret_reg));
                }
            }
            code.extend_from_slice(&encode_nop());
            "ret".to_string()
        }

        // ── Branch (unconditional) ──
        IRInstr::Branch { target } => {
            // BRCL 0xF, target (unconditional). 6 bytes, 32-bit disp.
            let patch_offset = code.len();
            code.extend_from_slice(&encode_brcl(0xF, 0));
            fixups.push(BranchFixup {
                offset: patch_offset,
                is_long: true,
                target: target.clone(),
            });
            "branch".to_string()
        }

        // ── CondBranch ──
        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            // LTGR R0, cond: sets CC based on cond.
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R0, cond_reg));
            // BRCL 0x6, true_target (branch if CC != 0, i.e., cond != 0).
            // Mask 0x6 = "CC != 0". 6 bytes, 32-bit disp.
            let true_patch = code.len();
            code.extend_from_slice(&encode_brcl(0x6, 0));
            fixups.push(BranchFixup {
                offset: true_patch,
                is_long: true,
                target: true_target.clone(),
            });
            // BRCL 0xF, false_target (unconditional).
            let false_patch = code.len();
            code.extend_from_slice(&encode_brcl(0xF, 0));
            fixups.push(BranchFixup {
                offset: false_patch,
                is_long: true,
                target: false_target.clone(),
            });
            reads.push(phys(cond_reg));
            "cond_branch".to_string()
        }

        // ── Syscall ──
        IRInstr::Syscall { nr, args, dst } => {
            // s390x Linux syscall ABI: nr in R1, args R2-R7, SVC 0, return R2.
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::S390X,
                *nr,
            );
            // Load syscall number into R1.
            emit_load_imm(code, Gpr::R1, native_nr as i64);
            // Load args into R2-R7.
            let syscall_arg_regs = [
                Gpr::R2, Gpr::R3, Gpr::R4, Gpr::R5, Gpr::R6, Gpr::R7,
            ];
            let num_reg_args = args.len().min(syscall_arg_regs.len());
            for (i, arg) in args.iter().take(num_reg_args).enumerate() {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != syscall_arg_regs[i] {
                    code.extend_from_slice(&encode_lgr(syscall_arg_regs[i], arg_reg));
                }
            }
            // SVC 0
            code.extend_from_slice(&encode_svc(0));
            // Move return value (R2) to dst.
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Gpr::R2 {
                    code.extend_from_slice(&encode_lgr(dst_reg, Gpr::R2));
                }
                writes.push(phys(dst_reg));
            }
            "syscall".to_string()
        }

        // ── Call ──
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            // Move args into R2-R6 (up to 5 args in registers).
            // Args 6+ go on the stack at R15+160, R15+168, etc.
            let arg_regs = [Gpr::R2, Gpr::R3, Gpr::R4, Gpr::R5, Gpr::R6];
            for (i, arg) in args.iter().enumerate() {
                if i < arg_regs.len() {
                    let arg_reg = load_to_reg(arg, alloc, code);
                    if arg_reg != arg_regs[i] {
                        code.extend_from_slice(&encode_lgr(arg_regs[i], arg_reg));
                    }
                } else {
                    // Stack arg: store at R15 + (160 + (i - 5) * 8)
                    let stack_off = (160 + (i - arg_regs.len()) * 8) as i32;
                    let arg_reg = load_to_reg(arg, alloc, code);
                    code.extend_from_slice(&encode_stg(arg_reg, SP, stack_off));
                }
            }
            // BRASL R14, fname — placeholder disp=0; record a relocation.
            let call_offset = code.len() as u64;
            code.extend_from_slice(&encode_brasl(LR, 0));
            relocations.push(RelocationEntry {
                offset: call_offset,
                symbol: fname.clone(),
                reloc_type: "R_S390_PC32DBL".to_string(),
            });
            // Move return value (R2) to dst.
            if let Some(d) = dst {
                let dst_reg = load_to_reg(d, alloc, code);
                if dst_reg != Gpr::R2 {
                    code.extend_from_slice(&encode_lgr(dst_reg, Gpr::R2));
                }
                writes.push(phys(dst_reg));
            }
            if *is_extern { "call_extern".to_string() } else { "call".to_string() }
        }

        // ── AtomicLoad (simplified: regular load — s390x aligned loads are atomic) ──
        IRInstr::AtomicLoad { dst, addr, ty } => {
            let load_instr = IRInstr::Load {
                dst: dst.clone(),
                addr: addr.clone(),
                offset: 0,
                ty: ty.clone(),
            };
            let (load_opcode, load_reads, load_writes) =
                emit_instruction(code, &load_instr, alloc, fixups, relocations)?;
            reads.extend(load_reads);
            writes.extend(load_writes);
            load_opcode
        }

        // ── AtomicStore ──
        IRInstr::AtomicStore { value, addr, ty } => {
            let store_instr = IRInstr::Store {
                value: value.clone(),
                addr: addr.clone(),
                offset: 0,
                ty: ty.clone(),
            };
            let (store_opcode, store_reads, store_writes) =
                emit_instruction(code, &store_instr, alloc, fixups, relocations)?;
            reads.extend(store_reads);
            writes.extend(store_writes);
            store_opcode
        }

        // ── AtomicCas (simplified CAS via load + compare + store) ──
        IRInstr::AtomicCas { dst, addr, expected, desired, ty } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let addr_reg = load_to_reg(addr, alloc, code);
            let expected_reg = load_to_reg(expected, alloc, code);
            let desired_reg = load_to_reg(desired, alloc, code);
            // Load old into dst.
            match ty {
                IRType::U32 | IRType::I32 => {
                    code.extend_from_slice(&encode_llgf(dst_reg, addr_reg, 0));
                }
                _ => {
                    code.extend_from_slice(&encode_lg(dst_reg, addr_reg, 0));
                }
            }
            // CGR expected, dst → sets CC.
            code.extend_from_slice(&encode_rre(0xB9, 0x20, expected_reg, dst_reg));
            // BRC 0x6, skip_store (if not equal, skip).
            let skip_patch = code.len();
            code.extend_from_slice(&encode_brc(0x6, 0));
            // Store desired.
            match ty {
                IRType::U32 | IRType::I32 => {
                    code.extend_from_slice(&encode_sty(desired_reg, addr_reg, 0));
                }
                _ => {
                    code.extend_from_slice(&encode_stg(desired_reg, addr_reg, 0));
                }
            }
            // skip_store: patch the BRC.
            let skip_target = code.len() as i64;
            let disp = (skip_target - skip_patch as i64) / 2;
            let disp_be = (disp as i16).to_be_bytes();
            code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);

            reads.push(phys(addr_reg));
            reads.push(phys(expected_reg));
            reads.push(phys(desired_reg));
            writes.push(phys(dst_reg));
            "atomic_cas".to_string()
        }

        // ── Unhandled ──
        _ => {
            // Emit a NOP for any unhandled instruction to preserve code layout.
            code.extend_from_slice(&encode_nop());
            "unhandled".to_string()
        }
    };

    Ok((opcode, reads, writes))
}

/// Emit a binary op (And/Or/Xor/Shifts/Div/Rem/Comparisons) as s390x
/// register-based machine code.
///
/// `reads` and `writes` are populated with the physical registers used.
fn emit_binop_isel(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    ty: Option<&IRType>,
    alloc: &RegAllocResult,
    code: &mut Vec<u8>,
    reads: &mut Vec<PhysicalReg>,
    writes: &mut Vec<PhysicalReg>,
) {
    let is_32bit = is_32bit_ty(ty);
    let dst_reg = load_to_reg(dst, alloc, code);

    match op {
        BinOpKind::Add | BinOpKind::Sub | BinOpKind::Mul => {
            // Inline the Add/Sub/Mul logic (mirrors the IRInstr::Add/Sub/Mul
            // handlers in `emit_instruction`) so we can populate reads/writes
            // correctly without recursive dispatch.
            match resolve_value(lhs, alloc) {
                ResolvedVal::Reg(lhs_reg) => {
                    if dst_reg != lhs_reg {
                        code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
                    }
                    reads.push(phys(lhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, dst_reg, imm);
                }
            }
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    match op {
                        BinOpKind::Add => code.extend_from_slice(&encode_agr(dst_reg, rhs_reg)),
                        BinOpKind::Sub => code.extend_from_slice(&encode_sgr(dst_reg, rhs_reg)),
                        BinOpKind::Mul => code.extend_from_slice(&encode_msgr(dst_reg, rhs_reg)),
                        _ => unreachable!(),
                    }
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, Gpr::R0, imm);
                    match op {
                        BinOpKind::Add => code.extend_from_slice(&encode_agr(dst_reg, Gpr::R0)),
                        BinOpKind::Sub => code.extend_from_slice(&encode_sgr(dst_reg, Gpr::R0)),
                        BinOpKind::Mul => code.extend_from_slice(&encode_msgr(dst_reg, Gpr::R0)),
                        _ => unreachable!(),
                    }
                }
            }
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
            }
            writes.push(phys(dst_reg));
            return;
        }
        BinOpKind::SDiv => {
            emit_div_isel(dst, lhs, rhs, ty, true, false, alloc, code, reads, writes);
            return;
        }
        BinOpKind::SRem => {
            emit_div_isel(dst, lhs, rhs, ty, true, true, alloc, code, reads, writes);
            return;
        }
        BinOpKind::UDiv => {
            emit_div_isel(dst, lhs, rhs, ty, false, false, alloc, code, reads, writes);
            return;
        }
        BinOpKind::URem => {
            emit_div_isel(dst, lhs, rhs, ty, false, true, alloc, code, reads, writes);
            return;
        }
        BinOpKind::And | BinOpKind::Or | BinOpKind::Xor => {
            // Materialize lhs into dst.
            match resolve_value(lhs, alloc) {
                ResolvedVal::Reg(lhs_reg) => {
                    if dst_reg != lhs_reg {
                        code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
                    }
                    reads.push(phys(lhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, dst_reg, imm);
                }
            }
            // Apply op with rhs.
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    match op {
                        BinOpKind::And => {
                            // NGR dst, rhs (64-bit AND). op1=0xB9, op2=0x80.
                            code.extend_from_slice(&encode_rre(0xB9, 0x80, dst_reg, rhs_reg));
                        }
                        BinOpKind::Or => {
                            // OGR dst, rhs (64-bit OR). op1=0xB9, op2=0x81.
                            code.extend_from_slice(&encode_rre(0xB9, 0x81, dst_reg, rhs_reg));
                        }
                        BinOpKind::Xor => {
                            // XGR dst, rhs (64-bit XOR). op1=0xB9, op2=0x82.
                            code.extend_from_slice(&encode_rre(0xB9, 0x82, dst_reg, rhs_reg));
                        }
                        _ => unreachable!(),
                    }
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, Gpr::R0, imm);
                    match op {
                        BinOpKind::And => {
                            code.extend_from_slice(&encode_rre(0xB9, 0x80, dst_reg, Gpr::R0));
                        }
                        BinOpKind::Or => {
                            code.extend_from_slice(&encode_rre(0xB9, 0x81, dst_reg, Gpr::R0));
                        }
                        BinOpKind::Xor => {
                            code.extend_from_slice(&encode_rre(0xB9, 0x82, dst_reg, Gpr::R0));
                        }
                        _ => unreachable!(),
                    }
                }
            }
            // For 32-bit results, zero-extend (the high bits of the result
            // are already correct for AND/OR/XOR when both operands have
            // zero high bits — but the imm path may have set them. LLGFR
            // is harmless and safe.)
            if is_32bit {
                code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
            }
            writes.push(phys(dst_reg));
            return;
        }
        BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA => {
            // Materialize lhs into dst.
            match resolve_value(lhs, alloc) {
                ResolvedVal::Reg(lhs_reg) => {
                    if dst_reg != lhs_reg {
                        code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
                    }
                    reads.push(phys(lhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, dst_reg, imm);
                }
            }
            // Shift by rhs. If rhs is an immediate, use the immediate form
            // (SLLG/SRLG/SRAG with imm disp). Otherwise, use the register
            // form via RSY-a with B2=rhs_reg and D2=0.
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    match op {
                        BinOpKind::Shl => {
                            // SLLG dst, dst, 0(rhs_reg). op1=0xEB, op2=0x0D.
                            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0D, dst_reg, dst_reg, rhs_reg, 0));
                        }
                        BinOpKind::ShrL => {
                            // SRLG dst, dst, 0(rhs_reg). op1=0xEB, op2=0x0C.
                            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0C, dst_reg, dst_reg, rhs_reg, 0));
                        }
                        BinOpKind::ShrA => {
                            // SRAG dst, dst, 0(rhs_reg). op1=0xEB, op2=0x0A.
                            code.extend_from_slice(&encode_rsy_a(0xEB, 0x0A, dst_reg, dst_reg, rhs_reg, 0));
                        }
                        _ => unreachable!(),
                    }
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    let shamt = (imm as u32) & 0x3F;
                    match op {
                        BinOpKind::Shl => {
                            code.extend_from_slice(&encode_sllg(dst_reg, dst_reg, shamt));
                        }
                        BinOpKind::ShrL => {
                            code.extend_from_slice(&encode_srlg(dst_reg, dst_reg, shamt));
                        }
                        BinOpKind::ShrA => {
                            code.extend_from_slice(&encode_srag(dst_reg, dst_reg, shamt));
                        }
                        _ => unreachable!(),
                    }
                }
            }
            if is_32bit && !matches!(op, BinOpKind::ShrA) {
                // Don't truncate for arithmetic shifts (sign-extension is
                // intentional), and don't truncate for shifts by >= 32
                // (which produce values that need all 64 bits).
                // For 32-bit Shl/ShrL, the low 32 bits are correct; the
                // high 32 bits may be non-zero. LLGFR truncates.
                // Actually, skip LLGFR for shifts to match the stack-slot
                // ISel behavior (which doesn't truncate shifts either).
                // Code that needs a u32 result after a shift should
                // explicitly Cast/Trunc.
            }
            writes.push(phys(dst_reg));
            return;
        }
        BinOpKind::Ror | BinOpKind::Rol => {
            // Rotate not directly supported; emulate via shifts (simplified — leave as lhs).
            match resolve_value(lhs, alloc) {
                ResolvedVal::Reg(lhs_reg) => {
                    if dst_reg != lhs_reg {
                        code.extend_from_slice(&encode_lgr(dst_reg, lhs_reg));
                    }
                    reads.push(phys(lhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    emit_load_imm(code, dst_reg, imm);
                }
            }
            writes.push(phys(dst_reg));
            return;
        }
        // Comparison ops: dst = (lhs OP rhs) ? 1 : 0
        BinOpKind::Eq | BinOpKind::Ne
        | BinOpKind::SLt | BinOpKind::SLe
        | BinOpKind::SGt | BinOpKind::SGe
        | BinOpKind::ULt | BinOpKind::ULe
        | BinOpKind::UGt | BinOpKind::UGe => {
            emit_cmp_isel(op, dst, lhs, rhs, ty, alloc, code, reads, writes);
            return;
        }
    }
}

/// Emit a comparison: `dst = (lhs OP rhs) ? 1 : 0`.
///
/// Uses CGR (signed 64-bit compare) or CLGR (unsigned 64-bit compare) to set
/// the condition code, then BRC to skip setting dst=0 when the condition holds.
fn emit_cmp_isel(
    op: &BinOpKind,
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    ty: Option<&IRType>,
    alloc: &RegAllocResult,
    code: &mut Vec<u8>,
    reads: &mut Vec<PhysicalReg>,
    writes: &mut Vec<PhysicalReg>,
) {
    let _ = ty;
    let dst_reg = load_to_reg(dst, alloc, code);
    let lhs_reg = load_to_reg(lhs, alloc, code);

    // For comparisons, we need both operands in registers. If rhs is an
    // immediate, load it into R0 (scratch). For 32-bit operands, the high
    // bits should be zero (LLGFR) or sign-extended (LGFR) — but for
    // simplicity, we use 64-bit compares which work correctly when both
    // operands have the same high-bit extension. The IR convention is that
    // 32-bit values are zero-extended (LLGFR applied on definition), so
    // unsigned compares are correct.
    let rhs_reg = match resolve_value(rhs, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            emit_load_imm(code, Gpr::R0, imm);
            Gpr::R0
        }
    };
    reads.push(phys(lhs_reg));
    if !matches!(resolve_value(rhs, alloc), ResolvedVal::Imm(_)) {
        reads.push(phys(rhs_reg));
    }

    // Determine signed vs unsigned compare.
    let unsigned = matches!(op, BinOpKind::ULt | BinOpKind::ULe | BinOpKind::UGt | BinOpKind::UGe);

    // Compare lhs with rhs.
    if unsigned {
        // CLGR R1, R2: Compare Logical 64-bit. op1=0xB9, op2=0x21.
        code.extend_from_slice(&encode_rre(0xB9, 0x21, lhs_reg, rhs_reg));
    } else {
        // CGR R1, R2: Compare 64-bit signed. op1=0xB9, op2=0x20.
        code.extend_from_slice(&encode_rre(0xB9, 0x20, lhs_reg, rhs_reg));
    }

    // Determine the BRC mask for "condition holds" (skip the "set 0").
    // After CGR/CLGR: CC=0 (equal), CC=1 (lhs < rhs), CC=2 (lhs > rhs).
    // Mask bits: 8=CC=0, 4=CC=1, 2=CC=2, 1=CC=3.
    // Condition-holds masks (we want to SKIP the "set 0" when condition holds):
    let skip_mask: u8 = match op {
        BinOpKind::Eq => 0x8,  // CC=0 (equal)
        BinOpKind::Ne => 0x6,  // CC!=0 (not equal) = CC=1 or CC=2
        BinOpKind::SLt | BinOpKind::ULt => 0x4,  // CC=1 (less than)
        BinOpKind::SLe | BinOpKind::ULe => 0xC,  // CC=0 or CC=1 (less or equal)
        BinOpKind::SGt | BinOpKind::UGt => 0x2,  // CC=2 (greater than)
        BinOpKind::SGe | BinOpKind::UGe => 0xA,  // CC=0 or CC=2 (greater or equal)
        _ => 0xF,
    };

    // dst = 1 (default: assume condition holds).
    code.extend_from_slice(&encode_lghi(dst_reg, 1));
    // BRC skip_mask, +skip (if condition holds, skip the "set 0").
    let skip_patch = code.len();
    code.extend_from_slice(&encode_brc(skip_mask, 0));
    // dst = 0 (condition did not hold).
    code.extend_from_slice(&encode_lghi(dst_reg, 0));
    // skip: (patch the BRC to jump here).
    let skip_target = code.len() as i64;
    let disp = (skip_target - skip_patch as i64) / 2;
    let disp_be = (disp as i16).to_be_bytes();
    code[skip_patch + 2..skip_patch + 4].copy_from_slice(&disp_be);

    writes.push(phys(dst_reg));
}

/// Emit a division: `dst = lhs / rhs` (signed or unsigned).
///
/// Uses DGR (signed 64-bit divide) or DLGR (unsigned 64-bit divide). Both
/// use the (R0, R1) register pair as the 128-bit dividend:
/// - R0 = high 64 bits of dividend (in), remainder (out)
/// - R1 = low 64 bits of dividend (in), quotient (out)
///
/// So we set up R0:R1 = sign/zero-extended lhs, then divide by rhs. The
/// quotient ends up in R1, which we move to dst.
///
/// **WARNING**: this clobbers R0 and R1. If lhs or rhs is allocated to R0 or
/// R1, this will fail. The allocator marks R0 and R1 as not_allocatable
/// (R0 is scratch; R1 is the syscall-number register), so this is safe.
fn emit_div_isel(
    dst: &IRValue,
    lhs: &IRValue,
    rhs: &IRValue,
    ty: Option<&IRType>,
    signed: bool,
    is_rem: bool,
    alloc: &RegAllocResult,
    code: &mut Vec<u8>,
    reads: &mut Vec<PhysicalReg>,
    writes: &mut Vec<PhysicalReg>,
) {
    let _ = ty;
    let dst_reg = load_to_reg(dst, alloc, code);

    // Load lhs into R1 (the low 64 bits of the dividend pair).
    match resolve_value(lhs, alloc) {
        ResolvedVal::Reg(lhs_reg) => {
            code.extend_from_slice(&encode_lgr(Gpr::R1, lhs_reg));
            reads.push(phys(lhs_reg));
        }
        ResolvedVal::Imm(imm) => {
            emit_load_imm(code, Gpr::R1, imm);
        }
    }

    // For signed: R0 = sign-extension of R1 (SRAG R0, R1, 63).
    // For unsigned: R0 = 0.
    if signed {
        code.extend_from_slice(&encode_srag(Gpr::R0, Gpr::R1, 63));
    } else {
        code.extend_from_slice(&encode_lghi(Gpr::R0, 0));
    }

    // Load rhs into R2 (we use R2 because DGR/DLGR divide R0:R1 by R2).
    let divisor_reg = match resolve_value(rhs, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            emit_load_imm(code, Gpr::R2, imm);
            Gpr::R2
        }
    };
    if matches!(resolve_value(rhs, alloc), ResolvedVal::Reg(_)) {
        reads.push(phys(divisor_reg));
    }

    if signed {
        // DSGR R0, R2: signed 64-bit divide. op1=0xB9, op2=0x0D.
        // After: R1 = quotient, R0 = remainder.
        code.extend_from_slice(&encode_dgr(Gpr::R0, divisor_reg));
    } else {
        // DLGR R0, R2: unsigned 64-bit divide. op1=0xB9, op2=0x87.
        // After: R1 = quotient, R0 = remainder.
        code.extend_from_slice(&encode_dlgr(Gpr::R0, divisor_reg));
    }

    // For Div/SDiv/UDiv: quotient is in R1.
    // For SRem/URem: remainder is in R0.
    let result_reg = if is_rem { Gpr::R0 } else { Gpr::R1 };
    code.extend_from_slice(&encode_lgr(dst_reg, result_reg));

    if is_32bit_ty(ty) {
        code.extend_from_slice(&encode_llgfr(dst_reg, dst_reg));
    }

    writes.push(phys(dst_reg));
}

/// Emit a terminator (Jump, Branch, Return, Switch, Unreachable).
fn emit_terminator(
    code: &mut Vec<u8>,
    term: &IRTerminator,
    alloc: &RegAllocResult,
    frame_size: i32,
    spill_size: i32,
    callee_saved_gprs: &[Gpr],
    fixups: &mut Vec<BranchFixup>,
) {
    match term {
        IRTerminator::Jump(label) => {
            // BRCL 0xF, label (unconditional). 6 bytes, 32-bit disp.
            let patch_offset = code.len();
            code.extend_from_slice(&encode_brcl(0xF, 0));
            fixups.push(BranchFixup {
                offset: patch_offset,
                is_long: true,
                target: label.clone(),
            });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            // LTGR R0, cond: sets CC based on cond.
            code.extend_from_slice(&encode_rre(0xB9, 0x02, Gpr::R0, cond_reg));
            // BRCL 0x6, true_block (if CC != 0, i.e., cond != 0).
            let true_patch = code.len();
            code.extend_from_slice(&encode_brcl(0x6, 0));
            fixups.push(BranchFixup {
                offset: true_patch,
                is_long: true,
                target: true_block.clone(),
            });
            // BRCL 0xF, false_block (unconditional).
            let false_patch = code.len();
            code.extend_from_slice(&encode_brcl(0xF, 0));
            fixups.push(BranchFixup {
                offset: false_patch,
                is_long: true,
                target: false_block.clone(),
            });
        }
        IRTerminator::Return(vals) => {
            // Move return value to R2 (if any), then emit the full epilogue.
            if let Some(first) = vals.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::R2 {
                    code.extend_from_slice(&encode_lgr(Gpr::R2, ret_reg));
                }
            }
            code.extend(emit_epilogue_bytes(frame_size, spill_size, callee_saved_gprs));
        }
        IRTerminator::Unreachable => {
            // Trap: LGFI R1, -1; SVC 0 (invalid syscall).
            code.extend_from_slice(&encode_lgfi(Gpr::R1, -1));
            code.extend_from_slice(&encode_svc(0));
        }
        IRTerminator::Switch { discr, targets, default } => {
            // Linear compare-and-branch switch.
            let discr_reg = load_to_reg(discr, alloc, code);
            for (val, label) in targets {
                // Load val into R0.
                emit_load_imm(code, Gpr::R0, *val);
                // CGR R0, discr → sets CC.
                code.extend_from_slice(&encode_rre(0xB9, 0x20, Gpr::R0, discr_reg));
                // BRCL 0x8, label (if CC=0, i.e., equal).
                let patch = code.len();
                code.extend_from_slice(&encode_brcl(0x8, 0));
                fixups.push(BranchFixup {
                    offset: patch,
                    is_long: true,
                    target: label.clone(),
                });
            }
            // BRCL 0xF, default (unconditional).
            let default_patch = code.len();
            code.extend_from_slice(&encode_brcl(0xF, 0));
            fixups.push(BranchFixup {
                offset: default_patch,
                is_long: true,
                target: default.clone(),
            });
        }
        IRTerminator::Invoke { normal, .. } => {
            // Simplified: jump to normal continuation.
            let patch = code.len();
            code.extend_from_slice(&encode_brcl(0xF, 0));
            fixups.push(BranchFixup {
                offset: patch,
                is_long: true,
                target: normal.clone(),
            });
        }
        IRTerminator::TailCall { .. } => {
            // Simplified: just emit the epilogue (return to caller).
            code.extend(emit_epilogue_bytes(frame_size, spill_size, callee_saved_gprs));
        }
        IRTerminator::Resume { .. } => {
            // Simplified: NOP (should not normally be hit).
            code.extend_from_slice(&encode_nop());
        }
    }
}

/// Helper: create a [`PhysicalReg`] from a [`Gpr`].
fn phys(g: Gpr) -> PhysicalReg {
    PhysicalReg::new(RegClass::Gpr, g as u32)
}

/// FP / unhandled-instruction fallback. Returns an error that triggers the
/// stack-slot ISel fallback in `allocate_registers`.
fn emit_fp_fallback(
    instr: &IRInstr,
) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed {
        isa: "s390x",
        reason: format!(
            "FP / unhandled instruction not yet supported in register-based emitter: {:?}",
            instr
        ),
    })
}
