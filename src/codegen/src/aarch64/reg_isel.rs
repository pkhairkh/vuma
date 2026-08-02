//! Full register-based instruction selection for AArch64.
//!
//! Mirrors the `reg_isel.rs` template used by every other VUMA backend
//! (riscv64, x86_64, ppc64, s390x, loongarch64, mips64, sparc64, hppa,
//! arm32, m68k, alpha, x86_32, riscv32). Consumes a target-agnostic
//! [`RegAllocResult`] from [`crate::regalloc::TargetAgnosticRegAlloc`] and
//! emits real AArch64 machine code (prologue / body / epilogue) with
//! virtual registers replaced by the allocator's assigned physical
//! registers.
//!
//! # Architecture
//!
//! 1. **Prologue**: `SUB SP, SP, #frame_size; ADD X29, SP, #0` (FP = SP,
//!    bottom of frame, so spill slots use positive FP-relative offsets);
//!    `STP X29, X30, [SP, #frame_size-16]` saves the FP/LR pair at the
//!    top of the frame; `STP Xn, Xn+1, [SP, #...]` saves each callee-saved
//!    GPR pair just below the FP/LR save.
//! 2. **Body**: For each IR instruction, resolve vregs → physical regs via
//!    `alloc.vreg_to_preg`, emit register-based encoding using the
//!    [`Instruction`] enum's `encode()` method (returns `u32` → little-endian
//!    `Vec<u8>`).
//! 3. **Spill/reload**: Insert `STR Xt, [X29, #fp_off]` / `LDR Xt, [X29,
//!    #fp_off]` at positions from `alloc.spill_code`. The
//!    target-agnostic `GenericSpillSlot.offset` is negative (FP-relative);
//!    we translate it to a positive FP-relative offset via
//!    `fp_off = -slot.offset - 8` so the unsigned-offset LDR/STR encoder
//!    accepts it.
//! 4. **Epilogue**: `ADD SP, X29, #0` (restore SP from FP — needed because
//!    `IRInstr::Alloc` may have decremented SP during the body); `LDP` for
//!    each callee-saved pair (in reverse order of the prologue); `LDP X29,
//!    X30, [SP, #frame_size-16]`; `ADD SP, SP, #frame_size`; `RET` —
//!    emitted at EVERY `IRTerminator::Return` path.

use crate::backend::{
    AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RegClass,
    RelocationEntry,
};
use crate::ir::{
    BinOpKind, CastKind, CmpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue, UnaryOpKind,
};
use crate::regalloc::{GenericSpillCode, RegAllocResult};
use crate::aarch64::{Condition, Instruction, Operand, Register};

/// Resolved value: either a physical register or an immediate.
enum ResolvedVal {
    Reg(Register),
    Imm(i64),
}

/// Branch fixup: (byte_offset_in_code, target_label, is_conditional).
///
/// `is_conditional = true` selects the 19-bit imm field (B.cond / CBZ /
/// CBNZ); `false` selects the 26-bit imm field (B / BL). The displacement
/// is `(target - PC) >> 2` per the ARM ARM.
struct BranchFixup {
    offset: usize,
    target: String,
    is_cond: bool,
}

/// Emit a complete function using register-based instruction selection.
///
/// Consumes a target-agnostic [`RegAllocResult`] (produced by
/// [`crate::regalloc::TargetAgnosticRegAlloc`]) and returns an
/// [`AllocatedFunction`] whose `encoded` bytes are real AArch64 machine
/// code with every vreg replaced by its assigned physical register.
pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // ── Compute frame size ──
    //
    // Callee-saved slots: each used callee-saved GPR (X19–X28 from the
    // allocator) occupies 8 bytes; we save them in pairs via STP, so the
    // area is `ceil(count/2) * 16` bytes. X29 (FP) and X30 (LR) are NOT
    // in `used_callee_saved` because they are marked `not_allocatable`
    // in `target_desc.rs` — the allocator cannot assign a vreg to them.
    // They are always saved in their own dedicated STP at the top of the
    // frame.
    let mut callee_saved_gprs: Vec<Register> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_reg(p))
        .filter(|r| !matches!(*r, Register::X29 | Register::X30 | Register::SP | Register::XZR))
        .collect();
    // Sort by encoding for deterministic STP pairing (X19+X20, X21+X22, ...).
    callee_saved_gprs.sort_by_key(|r| r.encoding());
    // Pad to an even count by appending XZR (the second register of the
    // final STP is unused — its store is discarded).
    let cs_count = callee_saved_gprs.len();
    let cs_pairs = cs_count.div_ceil(2);
    let cs_size = (cs_pairs * 16) as i32;

    // Spill area: 8 bytes per GPR spill slot ( SIMD uses 16 bytes, but the
    // current path only emits GPR spills — see `emit_spill_code`).
    let spill_bytes = (alloc.total_spill_slots as i32) * 8;
    let spill_size = (spill_bytes + 15) & !15;

    // +16 for the dedicated FP/LR save pair at the top of the frame.
    let raw_frame = cs_size + spill_size + 16;
    let frame_size = (raw_frame + 15) & !15;

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // ── Prologue ──
    //
    // Frame layout (after `SUB SP, SP, #frame_size`):
    //
    //   [SP + 0]                = spill slot 0  (slot.offset = -8)
    //   [SP + 8]                = spill slot 1  (slot.offset = -16)
    //   ...
    //   [SP + spill_size - 8]   = spill slot N-1
    //   [SP + spill_size]       = first callee-saved STP pair (X19+X20)
    //   [SP + spill_size + 16]  = second pair (X21+X22)
    //   ...
    //   [SP + frame_size - 16]  = X29 (FP) and X30 (LR) — top of frame
    //   [SP + frame_size]       = old SP (caller's frame)
    //
    // FP (X29) is set to SP (bottom of frame) so that spill slots have
    // POSITIVE FP-relative offsets — the existing
    // `Instruction::LDR/STR` encoders only accept non-negative multiples
    // of the access size. The translation is `fp_off = -slot.offset - 8`
    // (so slot.offset = -8 → fp_off = 0, slot.offset = -16 → fp_off = 8,
    // etc.).
    let prologue_start = all_code.len();
    // SUB SP, SP, #frame_size
    emit_imm12_or_long(
        &mut all_code,
        Instruction::SUB {
            rd: Register::SP,
            rn: Register::SP,
            rm: Operand::Imm12(frame_size as u16),
        },
        frame_size,
        |code, imm| {
            emit_load_imm(code, Register::X9, imm as i64);
            emit_instr(code, Instruction::ADD { rd: Register::X10, rn: Register::SP, rm: Operand::Imm12(0) });
            emit_instr(code, Instruction::SUB { rd: Register::X10, rn: Register::X10, rm: Operand::Reg { reg: Register::X9, shift: None } });
            emit_instr(code, Instruction::ADD { rd: Register::SP, rn: Register::X10, rm: Operand::Imm12(0) });
        },
    );
    // STP X29, X30, [SP, #frame_size-16]  — save CALLER's FP/LR at top
    // of frame BEFORE setting new FP. This is critical: if we set
    // MOV X29, SP first, then STP would save the NEW X29 (our own
    // frame pointer), not the CALLER's X29. On return, the epilogue
    // would restore our own FP instead of the caller's, causing the
    // caller to use the wrong SP and read the wrong saved LR → infinite
    // loop on multi-function call chains.
    emit_instr(
        &mut all_code,
        Instruction::STP {
            rt1: Register::X29,
            rt2: Register::X30,
            rn: Register::SP,
            offset: frame_size - 16,
        },
    );
    // ADD X29, SP, #0  (FP = SP = bottom of frame)
    emit_instr(
        &mut all_code,
        Instruction::ADD {
            rd: Register::X29,
            rn: Register::SP,
            rm: Operand::Imm12(0),
        },
    );
    // STP X19, X20, [SP, #frame_size-32]; STP X21, X22, [SP, #frame_size-48]; ...
    // Walk the callee-saved list in pairs; the second slot of an odd
    // count is XZR (no-op store).
    let mut cs_offset = frame_size - 32;
    let mut idx = 0;
    while idx < callee_saved_gprs.len() {
        let r1 = callee_saved_gprs[idx];
        let r2 = if idx + 1 < callee_saved_gprs.len() {
            callee_saved_gprs[idx + 1]
        } else {
            Register::XZR
        };
        emit_instr(
            &mut all_code,
            Instruction::STP {
                rt1: r1,
                rt2: r2,
                rn: Register::SP,
                offset: cs_offset,
            },
        );
        cs_offset -= 16;
        idx += 2;
    }
    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: callee_saved_gprs
            .iter()
            .copied()
            .map(phys)
            .collect(),
        encoded: all_code[prologue_start..].to_vec(),
    };

    // ── Argument shuffle (function entry) ──
    //
    // AAPCS64: first 8 integer args in X0–X7. The allocator may assign
    // these param vregs to different physical registers, so we emit a
    // sequence of MOV (ORR Xd, XZR, Xm) instructions to move each arg
    // from its AAPCS64 register to the allocator-assigned register.
    //
    // Cycles (e.g. arg0 → X1, arg1 → X0) are broken via the X9 scratch
    // (X9 is caller-saved and not used as an arg register).
    let arg_shuffle_start = all_code.len();
    let arg_regs = [
        Register::X0, Register::X1, Register::X2, Register::X3,
        Register::X4, Register::X5, Register::X6, Register::X7,
    ];
    let mut pending: Vec<(Register, Register)> = Vec::new();
    for (i, param) in func.params.iter().enumerate() {
        if i >= 8 {
            break;
        }
        if let IRValue::Register(vreg_id) = param {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(dst) = preg_to_reg(preg) {
                    let src = arg_regs[i];
                    if dst != src {
                        pending.push((src, dst));
                    }
                }
            }
        }
    }
    // Pass 1: move non-conflicting args (dst is not the src of any
    // other pending move).
    let mut progress = true;
    while progress && !pending.is_empty() {
        progress = false;
        let mut i = 0;
        while i < pending.len() {
            let (src, _dst) = pending[i];
            let mut conflict = false;
            for (j, (other_src, _)) in pending.iter().enumerate() {
                if i != j && *other_src == pending[i].1 {
                    // Another move's src is our dst — would clobber it.
                    let _ = (src, other_src);
                    conflict = true;
                    break;
                }
            }
            if !conflict {
                let (s, d) = pending.remove(i);
                emit_instr(&mut all_code, Instruction::MOV { rd: d, rm: s });
                progress = true;
            } else {
                i += 1;
            }
        }
    }
    // Pass 2: break cycles via X9 scratch.
    for (src, dst) in pending {
        emit_instr(&mut all_code, Instruction::MOV { rd: Register::X9, rm: src });
        emit_instr(&mut all_code, Instruction::MOV { rd: dst, rm: Register::X9 });
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
            // Insert spill/reload code BEFORE this instruction.
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
    //
    // If the function's CFG does not end in a Return (e.g. ends in
    // Unreachable), this epilogue acts as a safety net so the binary
    // never falls off the end of the function into the next symbol.
    let epilogue_start = all_code.len();
    all_code.extend(emit_epilogue_bytes(frame_size, &callee_saved_gprs));
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
    //
    // AArch64 branch displacements are `(target - PC) >> 2`, encoded in
    // 26 bits (B / BL) or 19 bits (B.cond / CBZ / CBNZ). The fixup's
    // `offset` is the byte offset of the branch instruction; the target
    // is the byte offset of the destination label.
    for fixup in &fixups {
        if let Some(&target_offset) = label_offsets.get(&fixup.target) {
            let rel: i32 = target_offset as i32 - fixup.offset as i32;
            patch_branch_displacement(&mut all_code, fixup.offset, rel, fixup.is_cond);
        }
    }

    // ── Re-slice AllocatedInstruction.encoded from patched all_code ──
    //
    // The branch fixups above patched bytes in `all_code` that are
    // already referenced by `instr.encoded` slices. Re-copy the patched
    // bytes into each instruction so the AllocatedFunction reflects the
    // final machine code.
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

    let callee_saved_phys: Vec<PhysicalReg> =
        callee_saved_gprs.iter().copied().map(phys).collect();

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

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Emit an [`Instruction`] as 4 little-endian bytes.
///
/// `Instruction::encode()` returns `Result<u32>`; we propagate the error
/// as a `BackendError::EncodingError` so callers can fall back to the
/// stack-slot ISel path.
fn emit_instr(code: &mut Vec<u8>, instr: Instruction) {
    let word = instr.encode().unwrap_or_else(|e| {
        panic!("aarch64 reg_isel: encoding failed for {:?}: {}", instr, e);
    });
    code.extend_from_slice(&word.to_le_bytes());
}

/// Try to emit an `Instruction::SUB`/`ADD` with a 12-bit immediate; if the
/// immediate is too large, fall back to the long-form sequence provided by
/// `fallback`.
fn emit_imm12_or_long<F: FnOnce(&mut Vec<u8>, i32)>(
    code: &mut Vec<u8>,
    short: Instruction,
    imm: i32,
    fallback: F,
) {
    if (0..=4095).contains(&imm) {
        emit_instr(code, short);
    } else {
        fallback(code, imm);
    }
}

/// Map a target-agnostic [`PhysicalReg`] to an AArch64 [`Register`].
fn preg_to_reg(preg: &PhysicalReg) -> Option<Register> {
    if preg.class != RegClass::Gpr {
        return None;
    }
    Register::from_encoding(preg.index)
}

/// Create a [`PhysicalReg`] (GPR) from a [`Register`].
fn phys(r: Register) -> PhysicalReg {
    PhysicalReg::new(RegClass::Gpr, r.encoding())
}

/// Resolve an [`IRValue`] to a physical register or an immediate.
fn resolve_value(val: &IRValue, alloc: &RegAllocResult) -> ResolvedVal {
    match val {
        IRValue::Register(vreg_id) => {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(reg) = preg_to_reg(preg) {
                    return ResolvedVal::Reg(reg);
                }
            }
            ResolvedVal::Reg(Register::X0) // fallback (should not happen)
        }
        IRValue::Immediate(imm) => ResolvedVal::Imm(*imm),
        IRValue::Address(addr) => ResolvedVal::Imm(*addr as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Register::X0),
    }
}

/// Load a value into a register. If the value is an immediate, materialise
/// it via `emit_load_imm` using X16 (IP0) as scratch.
///
/// X16 is chosen because it is `not_allocatable` in `target_desc.rs`
/// (intra-procedure-call scratch, IP0), so the register allocator never
/// assigns a live vreg to it. Using an allocatable register (e.g. X9)
/// would clobber any vreg the allocator assigned there, breaking later
/// instructions that read that vreg (e.g. Select loading an immediate
/// true_val into X9, then a Store using X9 as a base address).
///
/// X16 is safe within instruction emission because the value is consumed
/// immediately (no BL between the load and the use). The linker only
/// clobbers X16/X17 at BL veneer sites, not between instructions.
fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Register {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(r) => r,
        ResolvedVal::Imm(imm) => {
            let scratch = Register::X16;
            emit_load_imm(code, scratch, imm);
            scratch
        }
    }
}

/// Materialise a 64-bit immediate into `rd` using MOVZ / MOVK.
///
/// Mirrors `Emitter::emit_load_immediate` in `emit.rs:3336`:
///   - 0..=0xFFFF            → MOVZ
///   - 0..=0xFFFF_FFFF       → MOVZ + MOVK
///   - full 64-bit           → MOVZ + 3× MOVK (skipping zero words)
fn emit_load_imm(code: &mut Vec<u8>, rd: Register, value: i64) {
    if (0..=65535).contains(&value) {
        emit_instr(code, Instruction::MOVZ { rd, imm16: value as u16, shift: 0 });
        return;
    }
    if (0..=0xFFFF_FFFF).contains(&value) {
        let lo = (value & 0xFFFF) as u16;
        let hi = ((value >> 16) & 0xFFFF) as u16;
        emit_instr(code, Instruction::MOVZ { rd, imm16: lo, shift: 0 });
        emit_instr(code, Instruction::MOVK { rd, imm16: hi, shift: 16 });
        return;
    }
    let w0 = (value & 0xFFFF) as u16;
    let w1 = ((value >> 16) & 0xFFFF) as u16;
    let w2 = ((value >> 32) & 0xFFFF) as u16;
    let w3 = ((value >> 48) & 0xFFFF) as u16;
    emit_instr(code, Instruction::MOVZ { rd, imm16: w0, shift: 0 });
    if w1 != 0 {
        emit_instr(code, Instruction::MOVK { rd, imm16: w1, shift: 16 });
    }
    if w2 != 0 {
        emit_instr(code, Instruction::MOVK { rd, imm16: w2, shift: 32 });
    }
    if w3 != 0 {
        emit_instr(code, Instruction::MOVK { rd, imm16: w3, shift: 48 });
    }
}

/// Emit spill/reload code for a [`GenericSpillCode`].
///
/// The spill slot's `offset` is negative (FP-relative, convention from
/// the target-agnostic allocator: `offset = -(idx+1) * 8` for GPRs). We
/// translate it to a positive FP-relative offset via `fp_off =
/// -slot.offset - 8` so the existing unsigned-offset `LDR`/`STR`
/// encoders accept it. This works because we set `FP = SP` (bottom of
/// frame) in the prologue, placing spill slots at `[FP, #0..spill_size]`.
fn emit_spill_code(code: &mut Vec<u8>, spill: &GenericSpillCode) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(reg) = preg_to_reg(preg) {
                let fp_off = -slot.offset - 8;
                if fp_off >= 0 && fp_off % 8 == 0 && fp_off / 8 <= 4095 {
                    emit_instr(
                        code,
                        Instruction::STR {
                            rt: reg,
                            rn: Register::X29,
                            offset: fp_off,
                        },
                    );
                } else {
                    // Out of range: materialise address in X9, then STR [X9].
                    emit_load_imm(code, Register::X9, fp_off as i64);
                    emit_instr(
                        code,
                        Instruction::ADD {
                            rd: Register::X9,
                            rn: Register::X29,
                            rm: Operand::Reg { reg: Register::X9, shift: None },
                        },
                    );
                    emit_instr(
                        code,
                        Instruction::STR {
                            rt: reg,
                            rn: Register::X9,
                            offset: 0,
                        },
                    );
                }
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(reg) = preg_to_reg(preg) {
                let fp_off = -slot.offset - 8;
                if fp_off >= 0 && fp_off % 8 == 0 && fp_off / 8 <= 4095 {
                    emit_instr(
                        code,
                        Instruction::LDR {
                            rt: reg,
                            rn: Register::X29,
                            offset: fp_off,
                        },
                    );
                } else {
                    emit_load_imm(code, Register::X9, fp_off as i64);
                    emit_instr(
                        code,
                        Instruction::ADD {
                            rd: Register::X9,
                            rn: Register::X29,
                            rm: Operand::Reg { reg: Register::X9, shift: None },
                        },
                    );
                    emit_instr(
                        code,
                        Instruction::LDR {
                            rt: reg,
                            rn: Register::X9,
                            offset: 0,
                        },
                    );
                }
            }
        }
    }
}

/// Build the function epilogue bytes.
///
/// Restores SP from FP (in case `IRInstr::Alloc` decremented SP during
/// the body), restores callee-saved registers in reverse order, restores
/// FP/LR, deallocates the frame, and returns via `RET` (X30).
fn emit_epilogue_bytes(frame_size: i32, callee_saved_gprs: &[Register]) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + callee_saved_gprs.len() * 4);

    // Restore SP from FP: SP = X29 + 0.
    emit_instr(
        &mut out,
        Instruction::ADD {
            rd: Register::SP,
            rn: Register::X29,
            rm: Operand::Imm12(0),
        },
    );

    // Restore callee-saved (in reverse order of prologue save).
    // Walk the (possibly odd-length) list in pairs; emit LDP for each pair.
    let n = callee_saved_gprs.len();
    let mut pair_indices: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < n {
        pair_indices.push(i);
        i += 2;
    }
    // Reverse: restore the last-saved pair first.
    let mut cs_offset = frame_size - 32 - 16 * ((pair_indices.len() as i32) - 1);
    for &start in pair_indices.iter().rev() {
        let r1 = callee_saved_gprs[start];
        let r2 = if start + 1 < n {
            callee_saved_gprs[start + 1]
        } else {
            Register::XZR
        };
        emit_instr(
            &mut out,
            Instruction::LDP {
                rt1: r1,
                rt2: r2,
                rn: Register::SP,
                offset: cs_offset,
            },
        );
        cs_offset += 16;
    }

    // LDP X29, X30, [SP, #frame_size-16]  — restore FP/LR.
    emit_instr(
        &mut out,
        Instruction::LDP {
            rt1: Register::X29,
            rt2: Register::X30,
            rn: Register::SP,
            offset: frame_size - 16,
        },
    );

    // ADD SP, SP, #frame_size  — deallocate frame.
    if (0..=4095).contains(&frame_size) {
        emit_instr(
            &mut out,
            Instruction::ADD {
                rd: Register::SP,
                rn: Register::SP,
                rm: Operand::Imm12(frame_size as u16),
            },
        );
    } else {
        emit_load_imm(&mut out, Register::X9, frame_size as i64);
        emit_instr(
            &mut out,
            Instruction::ADD {
                rd: Register::X10,
                rn: Register::SP,
                rm: Operand::Imm12(0),
            },
        );
        emit_instr(
            &mut out,
            Instruction::ADD {
                rd: Register::X10,
                rn: Register::X10,
                rm: Operand::Reg { reg: Register::X9, shift: None },
            },
        );
        emit_instr(
            &mut out,
            Instruction::ADD {
                rd: Register::SP,
                rn: Register::X10,
                rm: Operand::Imm12(0),
            },
        );
    }

    // RET  (returns via X30).
    emit_instr(&mut out, Instruction::RET { rn: None });
    out
}

/// Patch a branch instruction's displacement in `code` at `offset`.
///
/// `is_cond = true` selects the 19-bit imm field (B.cond / CBZ / CBNZ at
/// bits [23:5]); `false` selects the 26-bit imm field (B / BL at bits
/// [25:0]). The displacement `rel` is a signed byte offset; the encoded
/// field is `rel >> 2` (word offset).
fn patch_branch_displacement(code: &mut [u8], offset: usize, rel: i32, is_cond: bool) {
    if offset + 4 > code.len() {
        return;
    }
    let instr = u32::from_le_bytes([
        code[offset],
        code[offset + 1],
        code[offset + 2],
        code[offset + 3],
    ]);
    let new_instr = if is_cond {
        // BCond / CBZ / CBNZ: imm19 at bits [23:5].
        let imm19 = ((rel >> 2) as u32) & 0x7FFFF;
        (instr & !(0x7FFFF << 5)) | (imm19 << 5)
    } else {
        // B / BL: imm26 at bits [25:0].
        let imm26 = ((rel >> 2) as u32) & 0x03FFFFFF;
        (instr & !0x03FFFFFF) | imm26
    };
    let bytes = new_instr.to_le_bytes();
    code[offset..offset + 4].copy_from_slice(&bytes);
}

/// Get the current code offset (length).
fn all_code_offset(code: &[u8]) -> usize {
    code.len()
}

/// Map a [`CmpKind`] to the corresponding AArch64 [`Condition`] code.
fn cmp_kind_to_cond(kind: &CmpKind) -> Condition {
    match kind {
        CmpKind::Eq => Condition::EQ,
        CmpKind::Ne => Condition::NE,
        CmpKind::SLt => Condition::LT,
        CmpKind::SLe => Condition::LE,
        CmpKind::SGt => Condition::GT,
        CmpKind::SGe => Condition::GE,
        CmpKind::ULt => Condition::CC,
        CmpKind::ULe => Condition::LS,
        CmpKind::UGt => Condition::HI,
        CmpKind::UGe => Condition::CS,
    }
}

/// Emit a comparison + CSET sequence that materialises the boolean
/// result of `cmp kind lhs, rhs` into `dst`.
fn emit_cmp_isel(code: &mut Vec<u8>, dst: Register, lhs: Register, rhs: Register, kind: &CmpKind) {
    // CMP lhs, rhs  (= SUBS XZR, lhs, rhs)
    emit_instr(
        code,
        Instruction::CMP {
            rn: lhs,
            rm: Operand::Reg { reg: rhs, shift: None },
        },
    );
    // CSET dst, cond  (= CSINC dst, XZR, XZR, invert(cond))
    emit_instr(code, Instruction::CSET { rd: dst, cond: cmp_kind_to_cond(kind) });
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
            if let Some(IRType::F32) | Some(IRType::F64) = ty {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    emit_instr(code, Instruction::ADD {
                        rd: dst_reg,
                        rn: lhs_reg,
                        rm: Operand::Reg { reg: rhs_reg, shift: None },
                    });
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    if (0..=4095).contains(&imm) {
                        emit_instr(code, Instruction::ADD {
                            rd: dst_reg,
                            rn: lhs_reg,
                            rm: Operand::Imm12(imm as u16),
                        });
                    } else {
                        let scratch = load_to_reg(rhs, alloc, code);
                        emit_instr(code, Instruction::ADD {
                            rd: dst_reg,
                            rn: lhs_reg,
                            rm: Operand::Reg { reg: scratch, shift: None },
                        });
                    }
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "add".to_string()
        }

        // ── Sub ──
        IRInstr::Sub { dst, lhs, rhs, ty } => {
            if let Some(IRType::F32) | Some(IRType::F64) = ty {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    emit_instr(code, Instruction::SUB {
                        rd: dst_reg,
                        rn: lhs_reg,
                        rm: Operand::Reg { reg: rhs_reg, shift: None },
                    });
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    if (0..=4095).contains(&imm) {
                        emit_instr(code, Instruction::SUB {
                            rd: dst_reg,
                            rn: lhs_reg,
                            rm: Operand::Imm12(imm as u16),
                        });
                    } else {
                        let scratch = load_to_reg(rhs, alloc, code);
                        emit_instr(code, Instruction::SUB {
                            rd: dst_reg,
                            rn: lhs_reg,
                            rm: Operand::Reg { reg: scratch, shift: None },
                        });
                    }
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "sub".to_string()
        }

        // ── Mul ──
        IRInstr::Mul { dst, lhs, rhs, ty } => {
            if let Some(IRType::F32) | Some(IRType::F64) = ty {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            emit_instr(code, Instruction::MUL { rd: dst_reg, rn: lhs_reg, rm: rhs_reg });
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "mul".to_string()
        }

        // ── Div ──
        IRInstr::Div { dst, lhs, rhs, ty } => {
            let is_signed = !matches!(ty, Some(IRType::U32) | Some(IRType::U64) | Some(IRType::U8) | Some(IRType::U16));
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            if is_signed {
                emit_instr(code, Instruction::SDIV { rd: dst_reg, rn: lhs_reg, rm: rhs_reg });
            } else {
                emit_instr(code, Instruction::UDIV { rd: dst_reg, rn: lhs_reg, rm: rhs_reg });
            }
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "div".to_string()
        }

        // ── BinOp (Div/And/Or/Xor/Shifts/Rem) ──
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            if let Some(IRType::F32) | Some(IRType::F64) = ty {
                return emit_fp_fallback(instr);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            // For ops that support immediate operands (Add, Sub, Shl, ShrL,
            // ShrA), use Imm12 when rhs is a small immediate. This avoids
            // loading the immediate into a scratch register (which would
            // clobber X9 if lhs was also an immediate loaded into X9).
            let supports_imm = matches!(
                op,
                BinOpKind::Add | BinOpKind::Sub | BinOpKind::Shl | BinOpKind::ShrL | BinOpKind::ShrA
            );
            let (rhs_reg, rhs_operand): (Register, Operand) = if supports_imm {
                match resolve_value(rhs, alloc) {
                    ResolvedVal::Imm(imm) if (0..=4095).contains(&imm) => {
                        (Register::XZR, Operand::Imm12(imm as u16))
                    }
                    _ => {
                        let r = load_to_reg(rhs, alloc, code);
                        (r, Operand::Reg { reg: r, shift: None })
                    }
                }
            } else {
                let r = load_to_reg(rhs, alloc, code);
                (r, Operand::Reg { reg: r, shift: None })
            };
            match op {
                BinOpKind::SDiv => {
                    let rm_reg = match rhs_operand { Operand::Reg { reg, .. } => reg, _ => rhs_reg };
                    emit_instr(code, Instruction::SDIV { rd: dst_reg, rn: lhs_reg, rm: rm_reg });
                }
                BinOpKind::UDiv => {
                    let rm_reg = match rhs_operand { Operand::Reg { reg, .. } => reg, _ => rhs_reg };
                    emit_instr(code, Instruction::UDIV { rd: dst_reg, rn: lhs_reg, rm: rm_reg });
                }
                BinOpKind::SRem => {
                    let rm_reg = match rhs_operand { Operand::Reg { reg, .. } => reg, _ => rhs_reg };
                    emit_instr(code, Instruction::SDIV { rd: dst_reg, rn: lhs_reg, rm: rm_reg });
                    emit_instr(code, Instruction::MSUB { rd: dst_reg, rn: rm_reg, rm: dst_reg, ra: lhs_reg });
                }
                BinOpKind::URem => {
                    let rm_reg = match rhs_operand { Operand::Reg { reg, .. } => reg, _ => rhs_reg };
                    emit_instr(code, Instruction::UDIV { rd: dst_reg, rn: lhs_reg, rm: rm_reg });
                    emit_instr(code, Instruction::MSUB { rd: dst_reg, rn: rm_reg, rm: dst_reg, ra: lhs_reg });
                }
                BinOpKind::And => emit_instr(code, Instruction::AND { rd: dst_reg, rn: lhs_reg, rm: rhs_reg }),
                BinOpKind::Or  => emit_instr(code, Instruction::ORR { rd: dst_reg, rn: lhs_reg, rm: rhs_reg }),
                BinOpKind::Xor => emit_instr(code, Instruction::EOR { rd: dst_reg, rn: lhs_reg, rm: rhs_reg }),
                BinOpKind::Shl => emit_instr(code, Instruction::LSL { rd: dst_reg, rn: lhs_reg, rm: rhs_operand }),
                BinOpKind::ShrL => emit_instr(code, Instruction::LSR { rd: dst_reg, rn: lhs_reg, rm: rhs_operand }),
                BinOpKind::ShrA => emit_instr(code, Instruction::ASR { rd: dst_reg, rn: lhs_reg, rm: rhs_operand }),
                BinOpKind::Add => emit_instr(code, Instruction::ADD { rd: dst_reg, rn: lhs_reg, rm: rhs_operand }),
                BinOpKind::Sub => emit_instr(code, Instruction::SUB { rd: dst_reg, rn: lhs_reg, rm: rhs_operand }),
                BinOpKind::Mul => emit_instr(code, Instruction::MUL { rd: dst_reg, rn: lhs_reg, rm: rhs_reg }),
                _ => {
                    emit_instr(code, Instruction::ADD { rd: dst_reg, rn: lhs_reg, rm: Operand::Reg { reg: rhs_reg, shift: None } });
                }
            }
            // For 32-bit types, zero-extend result (UBFM Xd, Xd, #0, #31).
            if matches!(ty, Some(IRType::I32) | Some(IRType::U32)) {
                emit_instr(code, Instruction::UBFM { rd: dst_reg, rn: dst_reg, immr: 0, imms: 31 });
            }
            reads.push(phys(lhs_reg));
            reads.push(phys(rhs_reg));
            writes.push(phys(dst_reg));
            "binop".to_string()
        }

        // ── UnaryOp ──
        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let src_reg = load_to_reg(operand, alloc, code);
            match op {
                UnaryOpKind::Neg => {
                    // SUB dst, XZR, src  →  NEG dst, src
                    // Note: rd=XZR would set S bit (CMP alias); use a non-XZR dst.
                    emit_instr(code, Instruction::SUB {
                        rd: dst_reg,
                        rn: Register::XZR,
                        rm: Operand::Reg { reg: src_reg, shift: None },
                    });
                }
                UnaryOpKind::Not => {
                    // ORR dst, XZR, src  then NOT = MVN. AArch64 has no MVN in
                    // our Instruction enum; emit EOR dst, src, #-1 would need
                    // an immediate form. Use: ORR dst, XZR, XZR = 0; EOR dst,
                    // src, dst_all_ones. Simpler: SUB dst, XZR, src, #0 then
                    // EOR with XZR-shifted is not available. Fall back to:
                    //   MOVN dst, #0  (dst = ~0 = all ones)  -- not in enum
                    // Use: EOR dst, src, XZR-shifted-1 is invalid.
                    // Practical: emit SUB X9, XZR, #1 (X9 = -1 = all ones),
                    // then EOR dst, src, X9.
                    emit_instr(code, Instruction::SUB {
                        rd: Register::X9,
                        rn: Register::XZR,
                        rm: Operand::Imm12(1),
                    });
                    emit_instr(code, Instruction::EOR {
                        rd: dst_reg,
                        rn: src_reg,
                        rm: Register::X9,
                    });
                }
                UnaryOpKind::Popcnt => {
                    // Population count: RBIT dst, src; then count bits via CLZ.
                    // RBIT reverses bits; then CLZ gives leading zeros = 64 -
                    // (position of highest set bit) - 1... this is not popcnt.
                    // AArch64 does not have a scalar popcnt instruction (CNT is
                    // SIMD only). Emit a NOP placeholder; correct popcnt would
                    // need a SIMD sequence (CNT + ADDV + UMOV).
                    emit_instr(code, Instruction::NOP);
                }
                UnaryOpKind::Clz => {
                    emit_instr(code, Instruction::CLZ { rd: dst_reg, rn: src_reg });
                }
                UnaryOpKind::Ctz => {
                    // ctz = clz(rbit(x)) — count trailing zeros.
                    emit_instr(code, Instruction::RBIT { rd: dst_reg, rn: src_reg });
                    emit_instr(code, Instruction::CLZ { rd: dst_reg, rn: dst_reg });
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
            let off = *offset;
            match ty {
                IRType::U8 | IRType::I8 => {
                    emit_instr(code, Instruction::LDRB { rt: dst_reg, rn: base_reg, offset: off });
                }
                IRType::U16 | IRType::I16 => {
                    emit_instr(code, Instruction::LDRH { rt: dst_reg, rn: base_reg, offset: off });
                }
                IRType::U32 => {
                    emit_instr(code, Instruction::LDR_W { rt: dst_reg, rn: base_reg, offset: off });
                }
                IRType::I32 => {
                    emit_instr(code, Instruction::LDRSW { rt: dst_reg, rn: base_reg, offset: off });
                }
                _ => {
                    emit_instr(code, Instruction::LDR { rt: dst_reg, rn: base_reg, offset: off });
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
            let off = *offset;
            match ty {
                IRType::U8 | IRType::I8 => {
                    emit_instr(code, Instruction::STRB { rt: val_reg, rn: base_reg, offset: off });
                }
                IRType::U16 | IRType::I16 => {
                    emit_instr(code, Instruction::STRH { rt: val_reg, rn: base_reg, offset: off });
                }
                IRType::U32 | IRType::I32 => {
                    emit_instr(code, Instruction::STR_W { rt: val_reg, rn: base_reg, offset: off });
                }
                _ => {
                    emit_instr(code, Instruction::STR { rt: val_reg, rn: base_reg, offset: off });
                }
            }
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "store".to_string()
        }

        // ── Cmp ──
        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            // Use CMP with immediate (Imm12) when rhs is a small immediate.
            // This avoids loading rhs into X9 scratch which could clobber
            // a live value assigned to X9 by the regalloc.
            let (rhs_reg, rhs_operand, is_imm) = match resolve_value(rhs, alloc) {
                ResolvedVal::Imm(imm) if (0..=4095).contains(&imm) => {
                    (Register::XZR, Operand::Imm12(imm as u16), true)
                }
                _ => {
                    let r = load_to_reg(rhs, alloc, code);
                    (r, Operand::Reg { reg: r, shift: None }, false)
                }
            };
            let dst_reg = load_to_reg(dst, alloc, code);
            // CMP lhs, rhs_operand
            emit_instr(code, Instruction::CMP { rn: lhs_reg, rm: rhs_operand });
            emit_instr(code, Instruction::CSET { rd: dst_reg, cond: cmp_kind_to_cond(kind) });
            reads.push(phys(lhs_reg));
            if !is_imm {
                reads.push(phys(rhs_reg));
            }
            writes.push(phys(dst_reg));
            "cmp".to_string()
        }

        // ── Select (cond ? true_val : false_val) ──
        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // If cond is an immediate, evaluate at compile time.
            if let IRValue::Immediate(c) = cond {
                if *c != 0 {
                    // cond = true → dst = true_val
                    let tv = load_to_reg(true_val, alloc, code);
                    if tv != dst_reg {
                        emit_instr(code, Instruction::MOV { rd: dst_reg, rm: tv });
                    }
                    reads.push(phys(tv));
                } else {
                    // cond = false → dst = false_val
                    let fv = load_to_reg(false_val, alloc, code);
                    if fv != dst_reg {
                        emit_instr(code, Instruction::MOV { rd: dst_reg, rm: fv });
                    }
                    reads.push(phys(fv));
                }
            } else {
                // cond is a register — use CMP + CSEL pattern.
                let cond_reg = load_to_reg(cond, alloc, code);
                // Load false_val first, then true_val.  If both are
                // immediates, they both use the load_to_reg scratch
                // (X16), so we must emit the MOV BEFORE loading
                // true_val (which would clobber the scratch).  Strategy:
                //   1. mov dst, false_val  (scratch = false_val)
                //   2. Load true_val into a DIFFERENT register.
                //      If true_val is immediate, it goes to scratch
                //      (overwriting false_val, but dst already has
                //      false_val).
                //   3. CMP cond, #0; CSEL dst, true_reg, dst, NE
                let false_reg = load_to_reg(false_val, alloc, code);
                if false_reg != dst_reg {
                    emit_instr(code, Instruction::MOV { rd: dst_reg, rm: false_reg });
                }
                let true_reg = load_to_reg(true_val, alloc, code);
                emit_instr(code, Instruction::CMP { rn: cond_reg, rm: Operand::Imm12(0) });
                emit_instr(code, Instruction::CSEL { rd: dst_reg, rn: true_reg, rm: dst_reg, cond: Condition::NE });
                reads.push(phys(cond_reg));
                reads.push(phys(false_reg));
                reads.push(phys(true_reg));
            }
            writes.push(phys(dst_reg));
            "select".to_string()
        }

        // ── CtSelect (same as Select on aarch64 — CSEL is branchless) ──
        IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            if let IRValue::Immediate(c) = cond {
                if *c != 0 {
                    let tv = load_to_reg(true_val, alloc, code);
                    if tv != dst_reg {
                        emit_instr(code, Instruction::MOV { rd: dst_reg, rm: tv });
                    }
                    reads.push(phys(tv));
                } else {
                    let fv = load_to_reg(false_val, alloc, code);
                    if fv != dst_reg {
                        emit_instr(code, Instruction::MOV { rd: dst_reg, rm: fv });
                    }
                    reads.push(phys(fv));
                }
            } else {
                let cond_reg = load_to_reg(cond, alloc, code);
                let false_reg = load_to_reg(false_val, alloc, code);
                if false_reg != dst_reg {
                    emit_instr(code, Instruction::MOV { rd: dst_reg, rm: false_reg });
                }
                let true_reg = load_to_reg(true_val, alloc, code);
                emit_instr(code, Instruction::CMP { rn: cond_reg, rm: Operand::Imm12(0) });
                emit_instr(code, Instruction::CSEL { rd: dst_reg, rn: true_reg, rm: dst_reg, cond: Condition::NE });
                reads.push(phys(cond_reg));
                reads.push(phys(false_reg));
                reads.push(phys(true_reg));
            }
            writes.push(phys(dst_reg));
            "ct_select".to_string()
        }

        // ── CtEq (dst = (lhs == rhs) ? 1 : 0, branchless) ──
        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // CMP lhs, rhs; CSET dst, EQ
            emit_instr(code, Instruction::CMP { rn: lhs_reg, rm: Operand::Reg { reg: rhs_reg, shift: None } });
            emit_instr(code, Instruction::CSET { rd: dst_reg, cond: Condition::EQ });
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
                            // UBFM dst, src, #0, #7  (= UXTB)
                            emit_instr(code, Instruction::UBFM { rd: dst_reg, rn: src_reg, immr: 0, imms: 7 });
                        }
                        Some(IRType::U16) | Some(IRType::I16) => {
                            emit_instr(code, Instruction::UBFM { rd: dst_reg, rn: src_reg, immr: 0, imms: 15 });
                        }
                        Some(IRType::U32) | Some(IRType::I32) => {
                            // UBFM Xd, Xn, #0, #31  (= UXTW)
                            emit_instr(code, Instruction::UBFM { rd: dst_reg, rn: src_reg, immr: 0, imms: 31 });
                        }
                        _ => {
                            return emit_fp_fallback(instr);
                        }
                    }
                }
                CastKind::SExt => {
                    match from_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            // SBFM dst, src, #0, #7  (= SXTB)
                            emit_instr(code, Instruction::SBFM { rd: dst_reg, rn: src_reg, immr: 0, imms: 7 });
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            emit_instr(code, Instruction::SBFM { rd: dst_reg, rn: src_reg, immr: 0, imms: 15 });
                        }
                        Some(IRType::I32) | Some(IRType::U32) => {
                            // SXTW dst, src  (= SBFM Xd, Xn, #0, #31)
                            emit_instr(code, Instruction::SXTW { rd: dst_reg, rn: src_reg });
                        }
                        _ => {
                            if src_reg != dst_reg {
                                emit_instr(code, Instruction::MOV { rd: dst_reg, rm: src_reg });
                            }
                        }
                    }
                }
                CastKind::Trunc => {
                    if src_reg != dst_reg {
                        emit_instr(code, Instruction::MOV { rd: dst_reg, rm: src_reg });
                    }
                    // Truncation is implicit — the upper bits will be ignored
                    // by the consumer's 32-bit / 16-bit / 8-bit access.
                    if let Some(tt) = to_ty {
                        match tt {
                            IRType::U8 | IRType::I8 => {
                                emit_instr(code, Instruction::UBFM { rd: dst_reg, rn: dst_reg, immr: 0, imms: 7 });
                            }
                            IRType::U16 | IRType::I16 => {
                                emit_instr(code, Instruction::UBFM { rd: dst_reg, rn: dst_reg, immr: 0, imms: 15 });
                            }
                            IRType::U32 | IRType::I32 => {
                                emit_instr(code, Instruction::UBFM { rd: dst_reg, rn: dst_reg, immr: 0, imms: 31 });
                            }
                            _ => {}
                        }
                    }
                }
                CastKind::BitCast => {
                    if src_reg != dst_reg {
                        emit_instr(code, Instruction::MOV { rd: dst_reg, rm: src_reg });
                    }
                }
                _ => {
                    // IntToFloat, FloatToInt, FloatToUInt, UIntToFloat,
                    // FloatToFloat — FP casts need FP instructions which
                    // the register-based emitter doesn't support yet.
                    // Return an error so the caller falls back to the
                    // stack-slot ISel which has proper FP conversion code.
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
            // SUB SP, SP, #aligned; MOV dst, SP
            if (0..=4095).contains(&aligned) {
                emit_instr(code, Instruction::SUB {
                    rd: Register::SP,
                    rn: Register::SP,
                    rm: Operand::Imm12(aligned as u16),
                });
            } else {
                emit_load_imm(code, Register::X9, aligned as i64);
                emit_instr(code, Instruction::ADD { rd: Register::X10, rn: Register::SP, rm: Operand::Imm12(0) });
                emit_instr(code, Instruction::SUB { rd: Register::X10, rn: Register::X10, rm: Operand::Reg { reg: Register::X9, shift: None } });
                emit_instr(code, Instruction::ADD { rd: Register::SP, rn: Register::X10, rm: Operand::Imm12(0) });
            }
            emit_instr(code, Instruction::MOV { rd: dst_reg, rm: Register::SP });
            writes.push(phys(dst_reg));
            "alloc".to_string()
        }

        // ── Free (stack deallocation — NOP, stack reclaimed at epilogue) ──
        IRInstr::Free { .. } => {
            emit_instr(code, Instruction::NOP);
            "free".to_string()
        }

        // ── GetAddress (load address of a symbol) ──
        IRInstr::GetAddress { dst, name } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // Emit MOVZ + 3× MOVK placeholder (16 bytes). encode_program
            // patches these with the symbol's absolute address via the
            // R_VUMA_GETADDR relocation.
            let reloc_offset = all_code_offset(code) as u64;
            emit_instr(code, Instruction::MOVZ { rd: dst_reg, imm16: 0x1111, shift: 0 });
            emit_instr(code, Instruction::MOVK { rd: dst_reg, imm16: 0x1111, shift: 16 });
            emit_instr(code, Instruction::MOVK { rd: dst_reg, imm16: 0x1111, shift: 32 });
            emit_instr(code, Instruction::MOVK { rd: dst_reg, imm16: 0x1111, shift: 48 });
            relocations.push(RelocationEntry {
                offset: reloc_offset,
                symbol: name.clone(),
                reloc_type: "R_VUMA_GETADDR".to_string(),
            });
            writes.push(phys(dst_reg));
            "getaddr".to_string()
        }

        // ── Offset (pointer arithmetic: dst = base + offset) ──
        IRInstr::Offset { dst, base, offset, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(base, alloc, code);
            match resolve_value(offset, alloc) {
                ResolvedVal::Imm(imm) => {
                    if (0..=4095).contains(&imm) {
                        emit_instr(code, Instruction::ADD {
                            rd: dst_reg,
                            rn: base_reg,
                            rm: Operand::Imm12(imm as u16),
                        });
                    } else {
                        let scratch = Register::X9;
                        emit_load_imm(code, scratch, imm);
                        emit_instr(code, Instruction::ADD {
                            rd: dst_reg,
                            rn: base_reg,
                            rm: Operand::Reg { reg: scratch, shift: None },
                        });
                    }
                }
                ResolvedVal::Reg(off_reg) => {
                    emit_instr(code, Instruction::ADD {
                        rd: dst_reg,
                        rn: base_reg,
                        rm: Operand::Reg { reg: off_reg, shift: None },
                    });
                    reads.push(phys(off_reg));
                }
            }
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "offset".to_string()
        }

        // ── Phi (no-op at this stage — values are resolved via coalescing) ──
        IRInstr::Phi { dst, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // Emit a NOP to keep the destination register "live" for the
            // allocator's liveness tracking.
            let _ = dst_reg;
            emit_instr(code, Instruction::NOP);
            writes.push(phys(dst_reg));
            "phi".to_string()
        }

        // ── Ret (mid-block return — usually lowered to a Branch to the
        //     return block before reaching reg_isel; we still handle it
        //     defensively by moving the return value to X0.) ──
        IRInstr::Ret { values } => {
            if let Some(first) = values.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Register::X0 {
                    emit_instr(code, Instruction::MOV { rd: Register::X0, rm: ret_reg });
                }
            }
            emit_instr(code, Instruction::NOP);
            "ret".to_string()
        }

        // ── Branch (unconditional) ──
        IRInstr::Branch { target } => {
            let offset_pos = all_code_offset(code);
            // B 0  — placeholder, patched in fixup pass.
            let instr = Instruction::B { offset: 0 };
            let word = instr.encode().unwrap_or(0x14000000);
            code.extend_from_slice(&word.to_le_bytes());
            fixups.push(BranchFixup {
                offset: offset_pos,
                target: target.clone(),
                is_cond: false,
            });
            "branch".to_string()
        }

        // ── CondBranch (if cond then true_target else false_target) ──
        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            // CBNZ cond, true_target  (if cond != 0, jump to true)
            let offset_pos1 = all_code_offset(code);
            let cbnz = Instruction::CBNZ { rt: cond_reg, offset: 0 };
            let word = cbnz.encode().unwrap_or(0xB5000000);
            code.extend_from_slice(&word.to_le_bytes());
            fixups.push(BranchFixup {
                offset: offset_pos1,
                target: true_target.clone(),
                is_cond: true,
            });
            // B false_target  (fall-through to false)
            let offset_pos2 = all_code_offset(code);
            let b = Instruction::B { offset: 0 };
            let word = b.encode().unwrap_or(0x14000000);
            code.extend_from_slice(&word.to_le_bytes());
            fixups.push(BranchFixup {
                offset: offset_pos2,
                target: false_target.clone(),
                is_cond: false,
            });
            reads.push(phys(cond_reg));
            "cond_branch".to_string()
        }

        // ── Syscall (AArch64 Linux: x8=nr, x0-x5=args, x0=return, SVC #0) ──
        IRInstr::Syscall { nr, args, dst } => {
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::AArch64,
                *nr,
            );
            // MOV X8, #nr
            emit_load_imm(code, Register::X8, native_nr as i64);
            let arg_regs = [
                Register::X0, Register::X1, Register::X2,
                Register::X3, Register::X4, Register::X5,
            ];
            for (i, arg) in args.iter().enumerate().take(6) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    emit_instr(code, Instruction::MOV { rd: arg_regs[i], rm: arg_reg });
                }
            }
            // SVC #0
            emit_instr(code, Instruction::SVC { imm16: 0 });
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Register::X0 {
                    emit_instr(code, Instruction::MOV { rd: dst_reg, rm: Register::X0 });
                }
                writes.push(phys(dst_reg));
            }
            "syscall".to_string()
        }

        // ── Call (BL with R_AARCH64_CALL26 relocation) ──
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            // AAPCS64: calls clobber X0-X18 (caller-saved). The register
            // allocator may have assigned live values to X0-X14 (the
            // allocatable caller-saved range; X15 is the spill-scratch and
            // X16-X18 are not allocatable). Save all allocatable
            // caller-saved registers before the call and restore after,
            // preserving the return value (in X0) if the call has one.
            let arg_regs = [
                Register::X0, Register::X1, Register::X2, Register::X3,
                Register::X4, Register::X5, Register::X6, Register::X7,
            ];
            let has_return = dst.is_some();

            // Caller-saved allocatable registers: X0-X14 (X15 is
            // spill-scratch, not allocatable). Exclude X0 when the call
            // returns a value (X0 holds the return value after BL).
            let saved_regs: Vec<Register> = if has_return {
                vec![Register::X1, Register::X2, Register::X3, Register::X4,
                     Register::X5, Register::X6, Register::X7, Register::X8,
                     Register::X9, Register::X10, Register::X11, Register::X12,
                     Register::X13, Register::X14]
            } else {
                vec![Register::X0, Register::X1, Register::X2, Register::X3,
                     Register::X4, Register::X5, Register::X6, Register::X7,
                     Register::X8, Register::X9, Register::X10, Register::X11,
                     Register::X12, Register::X13, Register::X14]
            };

            // Allocate stack space for saved registers (16-byte aligned).
            // saved_regs.len() is 14 (has_return) or 15 (no return).
            // 14*8 = 112 (already 16-aligned); 15*8 = 120 → round to 128.
            let save_bytes = ((saved_regs.len() * 8 + 15) & !15) as u16;
            emit_instr(code, Instruction::SUB {
                rd: Register::SP,
                rn: Register::SP,
                rm: Operand::Imm12(save_bytes),
            });

            // Store saved_regs in pairs via STP. Pairing: (r0, r1), (r2, r3),
            // ... The last pair of an odd count uses XZR as the second reg.
            let n_pairs = (saved_regs.len() + 1) / 2;
            for pair in 0..n_pairs {
                let i = pair * 2;
                let r1 = saved_regs[i];
                let r2 = if i + 1 < saved_regs.len() {
                    saved_regs[i + 1]
                } else {
                    Register::XZR
                };
                emit_instr(code, Instruction::STP {
                    rt1: r1,
                    rt2: r2,
                    rn: Register::SP,
                    offset: (pair * 16) as i32,
                });
            }

            // Set up arguments.
            for (i, arg) in args.iter().enumerate().take(8) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    emit_instr(code, Instruction::MOV { rd: arg_regs[i], rm: arg_reg });
                }
            }

            // BL 0  — placeholder, patched by encode_program via
            // R_AARCH64_CALL26 relocation.
            let offset_pos = all_code_offset(code);
            let bl = Instruction::BL { offset: 0 };
            let word = bl.encode().unwrap_or(0x94000000);
            code.extend_from_slice(&word.to_le_bytes());
            relocations.push(RelocationEntry {
                offset: offset_pos as u64,
                symbol: fname.clone(),
                reloc_type: "R_AARCH64_CALL26".to_string(),
            });

            // Restore caller-saved registers (in reverse pair order, using
            // the same pairing as the store loop).
            for pair in (0..n_pairs).rev() {
                let i = pair * 2;
                let r1 = saved_regs[i];
                let r2 = if i + 1 < saved_regs.len() {
                    saved_regs[i + 1]
                } else {
                    Register::XZR
                };
                emit_instr(code, Instruction::LDP {
                    rt1: r1,
                    rt2: r2,
                    rn: Register::SP,
                    offset: (pair * 16) as i32,
                });
            }

            // Deallocate stack space.
            emit_instr(code, Instruction::ADD {
                rd: Register::SP,
                rn: Register::SP,
                rm: Operand::Imm12(save_bytes),
            });

            // Move return value from X0 to dst_reg AFTER restoring (dst_reg
            // may be one of the restored caller-saved registers; X0 was not
            // restored when has_return=true, so it still holds the return).
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Register::X0 {
                    emit_instr(code, Instruction::MOV { rd: dst_reg, rm: Register::X0 });
                }
                writes.push(phys(dst_reg));
            }
            if *is_extern { "call_extern".to_string() } else { "call".to_string() }
        }

        // ── AtomicLoad (LDAR — load-acquire) ──
        IRInstr::AtomicLoad { dst, addr, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            emit_instr(code, Instruction::LDAR { rt: dst_reg, rn: base_reg });
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "atomic_load".to_string()
        }

        // ── AtomicStore (STLR — store-release) ──
        IRInstr::AtomicStore { value, addr, .. } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            emit_instr(code, Instruction::STLR { rt: val_reg, rn: base_reg });
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "atomic_store".to_string()
        }

        // ── AtomicCas (LDAXR / CMP / B.NE / STLXR loop) ──
        IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
            let expected_reg = load_to_reg(expected, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            let new_reg = load_to_reg(desired, alloc, code);
            let dst_reg = load_to_reg(dst, alloc, code);
            // Loop:
            //   LDAXR X9, [base]      ; load-acquire exclusive
            //   CMP X9, expected      ; compare with expected
            //   B.NE done             ; if not equal, exit with old value
            //   STLXR X10, new, [base]; store-release exclusive
            //   CBNZ X10, loop        ; if store failed (X10 != 0), retry
            // done:
            //   MOV dst, X9           ; return old value
            let loop_pos = all_code_offset(code);
            emit_instr(code, Instruction::LDAXR { rt: Register::X9, rn: base_reg });
            emit_instr(code, Instruction::CMP {
                rn: Register::X9,
                rm: Operand::Reg { reg: expected_reg, shift: None },
            });
            // B.NE done  — skip the STLXR + CBNZ retry (2 instructions = 8 bytes ahead).
            emit_instr(code, Instruction::BCond { cond: Condition::NE, offset: 8 });
            emit_instr(code, Instruction::STLXR { rs: Register::X10, rt: new_reg, rn: base_reg });
            // CBNZ X10, loop  (backward branch to LDAXR — relative offset
            // is loop_pos - cbnz_pos, which is negative).
            let cbnz_pos = all_code_offset(code);
            emit_instr(code, Instruction::CBNZ { rt: Register::X10, offset: 0 });
            let rel: i32 = loop_pos as i32 - cbnz_pos as i32;
            patch_branch_displacement(code, cbnz_pos, rel, true);
            // done:  (the B.NE above lands here.)
            emit_instr(code, Instruction::MOV { rd: dst_reg, rm: Register::X9 });
            reads.push(phys(expected_reg));
            reads.push(phys(base_reg));
            reads.push(phys(new_reg));
            writes.push(phys(dst_reg));
            "atomic_cas".to_string()
        }

        // ── Unhandled (emit NOP, fall through) ──
        _ => {
            emit_instr(code, Instruction::NOP);
            "unhandled".to_string()
        }
    };

    Ok((opcode, reads, writes))
}

/// Emit a terminator (Jump, Branch, Return, Unreachable).
fn emit_terminator(
    code: &mut Vec<u8>,
    term: &IRTerminator,
    alloc: &RegAllocResult,
    frame_size: i32,
    callee_saved_gprs: &[Register],
    fixups: &mut Vec<BranchFixup>,
) {
    match term {
        IRTerminator::Jump(label) => {
            let offset_pos = all_code_offset(code);
            let b = Instruction::B { offset: 0 };
            let word = b.encode().unwrap_or(0x14000000);
            code.extend_from_slice(&word.to_le_bytes());
            fixups.push(BranchFixup {
                offset: offset_pos,
                target: label.clone(),
                is_cond: false,
            });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            // CBNZ cond, true_block  (if cond != 0, branch to true)
            let offset_pos1 = all_code_offset(code);
            let cbnz = Instruction::CBNZ { rt: cond_reg, offset: 0 };
            let word = cbnz.encode().unwrap_or(0xB5000000);
            code.extend_from_slice(&word.to_le_bytes());
            fixups.push(BranchFixup {
                offset: offset_pos1,
                target: true_block.clone(),
                is_cond: true,
            });
            // B false_block  (fall-through to false)
            let offset_pos2 = all_code_offset(code);
            let b = Instruction::B { offset: 0 };
            let word = b.encode().unwrap_or(0x14000000);
            code.extend_from_slice(&word.to_le_bytes());
            fixups.push(BranchFixup {
                offset: offset_pos2,
                target: false_block.clone(),
                is_cond: false,
            });
        }
        IRTerminator::Return(vals) => {
            // Move return values into X0 (first value) — AAPCS64.
            if let Some(first) = vals.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Register::X0 {
                    emit_instr(code, Instruction::MOV { rd: Register::X0, rm: ret_reg });
                }
            }
            // Emit the full epilogue (restore SP from FP, restore
            // callee-saved, restore FP/LR, deallocate frame, RET).
            code.extend(emit_epilogue_bytes(frame_size, callee_saved_gprs));
        }
        IRTerminator::Unreachable => {
            // BRK #1  — trap (SIGTRAP on Linux).
            // Encoding: 0xD4200020  (BRK #1 = 1101_0100_001_0000000000000000000_00001)
            let brk: u32 = 0xD4200020;
            code.extend_from_slice(&brk.to_le_bytes());
        }
        _ => {
            emit_instr(code, Instruction::NOP);
        }
    }
}

/// FP fallback — returns a `BackendError` so the caller (the dispatch in
/// `AArch64Backend::allocate_registers`) can fall back to the stack-slot
/// ISel path.
fn emit_fp_fallback(
    instr: &IRInstr,
) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed {
        isa: "aarch64",
        reason: format!(
            "FP instruction not yet supported in register-based emitter: {:?}",
            instr
        ),
    })
}
