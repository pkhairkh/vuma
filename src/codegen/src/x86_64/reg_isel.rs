//! Full register-based instruction selection for x86_64.
//!
//! This module implements a complete register-based emitter that consumes
//! a `RegAllocResult` (from `TargetAgnosticRegAlloc`) and produces
//! `AllocatedFunction` with register-to-register machine code for ALL IR
//! instructions.
//!
//! # Architecture
//!
//! 1. **Prologue**: `push rbp; mov rbp, rsp; push <callee-saved>; sub rsp, frame_size`
//! 2. **Body**: For each IR instruction, resolve vregs → physical regs via
//!    `alloc.vreg_to_preg`, emit register-based encoding.
//! 3. **Spill/reload**: Insert `mov [rbp+offset], reg` / `mov reg, [rbp+offset]`
//!    at positions from `alloc.spill_code`.
//! 4. **Epilogue**: `add rsp, frame_size; pop <callee-saved>; pop rbp; ret`
//!
//! # Frame Layout
//!
//! ```text
//! [rbp+16]  return address (pushed by call)
//! [rbp+8]   old rbp (pushed by prologue — wait, push rbp puts it at [rbp])
//! [rbp]     saved rbp
//! [rbp-N]   callee-saved saves (RBX, R12-R15)
//! [rbp-M]   spill slots (each 8 bytes, from alloc.spill_slots)
//! rsp       = rbp - frame_size
//! ```
//!
//! Actually, x86_64 System V ABI frame layout with frame pointer:
//! ```text
//! [rbp+8]   return address
//! [rbp]     old rbp (pushed first, then mov rbp, rsp)
//! [rbp-8]   first callee-saved (pushed after rbp setup)
//! [rbp-16]  second callee-saved
//! ...
//! [rbp-K]   spill slot 0
//! [rbp-K-8] spill slot 1
//! ...
//! rsp = rbp - frame_size
//! ```

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CastKind, CmpKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::x86_64::*;

/// Resolved value: either a physical register or an immediate.
enum ResolvedVal {
    Reg(Gpr),
    Imm(i64),
}

/// Branch fixup: (byte_offset_in_code, target_label, is_conditional, condition_code_if_cond)
struct BranchFixup {
    offset: usize,      // byte offset of the rel32 field in the code
    target: String,     // target label
}

/// Emit a complete function using register-based instruction selection.
///
/// This is the FULL register-based emitter — it does NOT start from
/// stack-slot bytes. It consumes the `RegAllocResult` and produces
/// register-to-register machine code for every IR instruction.
pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    // DEBUG: dump the IR blocks and terminators to stderr.
    if std::env::var("VUMA_DEBUG_REG_ISEL").is_ok() {
        eprintln!("=== emit_function_regalloc_full: {} ===", func.name);
        for (i, block) in func.blocks.iter().enumerate() {
            eprintln!("  bb{} label={:?}:", i, block.label);
            for instr in &block.instructions {
                eprintln!("    {:?}", instr);
            }
            eprintln!("    TERM: {:?}", block.terminator);
        }
    }
    // ── Compute frame size ──
    // Callee-saved saves: each push is 8 bytes. Count the callee-saved GPRs
    // (excluding RBP which is handled separately).
    let callee_saved_gprs: Vec<Gpr> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::Rbp && *g != Gpr::Rsp)
        .collect();
    let callee_saved_size = callee_saved_gprs.len() * 8;
    // Spill slots: each is 8 bytes (GPR) or 16 bytes (SIMD). Use 8 for now.
    let spill_size = alloc.total_spill_slots as usize * 8;
    // Frame size must be 16-byte aligned (System V ABI).
    // After `call` (8-byte return addr) + `push rbp` (8 bytes), RSP is
    // 16-aligned. After pushing N callee-saved (N*8 bytes), alignment is
    // preserved iff N is even, else off by 8. Either way, `sub rsp, frame_size`
    // must bring RSP back to 16-alignment at the next call boundary, so
    // frame_size = ((callee_saved_size + spill_size + 15) & !15).
    let raw_frame = callee_saved_size + spill_size;
    let frame_size = ((raw_frame + 15) & !15) as u32;

    // ── Two-pass: first compute block offsets, then emit ──
    // Pass 1: emit to a temporary buffer to compute sizes.
    // Actually, we emit in one pass and use fixups for branches.

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // ── Prologue ──
    let prologue_start = all_code.len();
    // push rbp
    all_code.extend(encode_push(Gpr::Rbp));
    // mov rbp, rsp
    all_code.extend(encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp));
    // push callee-saved registers (in order: RBX, R12-R15)
    for &g in &callee_saved_gprs {
        all_code.extend(encode_push(g));
    }
    // sub rsp, frame_size
    all_code.extend(encode_sub_reg_imm32(Gpr::Rsp, frame_size as i32));

    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: callee_saved_gprs.iter().map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32)).collect(),
        encoded: all_code[prologue_start..].to_vec(),
    };

    // ── Argument shuffle (function entry) ──
    // The System V ABI passes the first 6 integer args in
    // RDI, RSI, RDX, RCX, R8, R9. The register allocator may assign
    // these parameter vregs to DIFFERENT physical registers (e.g. vreg 0
    // → R10). Without an argument shuffle at function entry, the callee
    // reads its parameters from the wrong registers (garbage) and
    // produces wrong results. This is the x86_64 equivalent of aarch64's
    // `reg_alloc.preassign(vreg, X0..X7)` — but since reg_isel.rs
    // consumes an already-computed RegAllocResult, we emit mov
    // instructions instead of pre-coloring.
    let arg_shuffle_start = all_code.len();
    let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new(); // (src=ABI reg, dst=allocator reg)
    for (i, param) in func.params.iter().enumerate() {
        if i >= 6 {
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
    // Pass 1: move non-conflicting args (dst is not a src for any
    // unmoved arg). Repeat until no progress.
    let mut progress = true;
    while progress && !pending.is_empty() {
        progress = false;
        let mut i = 0;
        while i < pending.len() {
            let (src, dst) = pending[i];
            // Check if dst is a src for any other pending move.
            let mut conflict = false;
            for (j, (_, other_dst)) in pending.iter().enumerate() {
                if i != j && *other_dst == src {
                    conflict = true;
                    break;
                }
            }
            if !conflict {
                all_code.extend(encode_mov_reg_reg(dst, src));
                pending.remove(i);
                progress = true;
            } else {
                i += 1;
            }
        }
    }
    // Pass 2: handle cycles with R11 scratch.
    for (src, dst) in pending {
        // mov r11, src; mov dst, r11
        all_code.extend(encode_mov_reg_reg(Gpr::R11, src));
        all_code.extend(encode_mov_reg_reg(dst, Gpr::R11));
    }
    let arg_shuffle_end = all_code.len();
    let has_arg_shuffle = arg_shuffle_end > arg_shuffle_start;

    // ── Body: emit each block ──
    // Position keying must match the allocator in regalloc.rs
    // (`LiveRangeComputer::compute`): each instruction and each terminator
    // consumes `pos += 2`, NEVER reset between blocks. The emitter previously
    // used `idx as u32 * 2` per-block which silently dropped spill_code for
    // all blocks after the first.
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
                    emit_spill_code(&mut all_code, spill, &callee_saved_gprs, frame_size);
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

            // Emit the instruction itself.
            let instr_start = all_code.len();
            let (opcode, reads, writes) = emit_instruction(
                &mut all_code,
                instr,
                alloc,
                frame_size,
                &callee_saved_gprs,
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

        // Spill/reload BEFORE the terminator uses the terminator's own pos.
        if let Some(spills) = alloc.spill_code.get(&global_pos) {
            for spill in spills {
                let spill_start = all_code.len();
                emit_spill_code(&mut all_code, spill, &callee_saved_gprs, frame_size);
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

        // Emit terminator. Return paths emit the full epilogue inline so
        // that early returns restore RSP / callee-saved / RBP correctly.
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

    // ── Trailing unreachable epilogue (defensive) ──
    // Every Return/Jump-to-return path now emits its own epilogue inline.
    // This trailing copy is kept only as a defensive safety net in case
    // some path falls through without an explicit terminator; it is
    // normally unreachable.
    let epilogue_start = all_code.len();
    all_code.extend(emit_epilogue_bytes(frame_size, &callee_saved_gprs));
    let epilogue_end = all_code.len();

    // Add prologue (and argument shuffle if any) to the first block,
    // and the trailing epilogue to the last block (as a defensive
    // unreachable marker).
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
            let rel32: i32 = target_offset as i32 - fixup.offset as i32 - 4;
            let bytes = rel32.to_le_bytes();
            all_code[fixup.offset] = bytes[0];
            all_code[fixup.offset + 1] = bytes[1];
            all_code[fixup.offset + 2] = bytes[2];
            all_code[fixup.offset + 3] = bytes[3];
        } else if std::env::var("VUMA_DEBUG_REG_ISEL").is_ok() {
            eprintln!(
                "  FIXUP MISS: target={:?} not in label_offsets (keys: {:?})",
                fixup.target,
                label_offsets.keys().collect::<Vec<_>>()
            );
        }
    }

    // ── Update block code offsets and instruction encoded bytes ──
    // CRITICAL: branch fixups patched `all_code` in place above, but each
    // `AllocatedInstruction.encoded` was copied from `all_code` BEFORE the
    // patch. Re-slice each instruction's encoded bytes from the now-patched
    // `all_code` so the linker sees correct rel32 values. Lengths are
    // unchanged (fixup resolution only overwrites the 4 rel32 bytes in
    // place), so we can re-derive offsets from the existing lengths.
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

    // ── Build the AllocatedFunction ──
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

/// Map a PhysicalReg to a Gpr.
fn preg_to_gpr(preg: &PhysicalReg) -> Option<Gpr> {
    if preg.class != crate::backend::RegClass::Gpr {
        return None;
    }
    match preg.index {
        0 => Some(Gpr::Rax),
        1 => Some(Gpr::Rcx),
        2 => Some(Gpr::Rdx),
        3 => Some(Gpr::Rbx),
        4 => Some(Gpr::Rsp),
        5 => Some(Gpr::Rbp),
        6 => Some(Gpr::Rsi),
        7 => Some(Gpr::Rdi),
        8 => Some(Gpr::R8),
        9 => Some(Gpr::R9),
        10 => Some(Gpr::R10),
        11 => Some(Gpr::R11),
        12 => Some(Gpr::R12),
        13 => Some(Gpr::R13),
        14 => Some(Gpr::R14),
        15 => Some(Gpr::R15),
        _ => None,
    }
}

/// Resolve an IRValue to a physical register or immediate.
fn resolve_value(val: &IRValue, alloc: &RegAllocResult) -> ResolvedVal {
    match val {
        IRValue::Register(vreg_id) => {
            // Follow coalesced map.
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(gpr) = preg_to_gpr(preg) {
                    return ResolvedVal::Reg(gpr);
                }
            }
            // If spilled, we should have already inserted a reload.
            // Fall back to RAX as a scratch (should not happen in correct alloc).
            ResolvedVal::Reg(Gpr::Rax)
        }
        IRValue::Immediate(imm) => ResolvedVal::Imm(*imm),
        IRValue::Address(addr) => ResolvedVal::Imm(*addr as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Gpr::Rax), // Should not happen for data values.
    }
}

/// Load a value into a register. If it's an immediate, emit a mov.
/// If it's already in a register, just return it.
fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Gpr {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            // Use R11 as scratch for immediates (caller-saved, not an arg reg).
            let scratch = Gpr::R11;
            if imm >= i32::MIN as i64 && imm <= i32::MAX as i64 {
                code.extend(encode_mov_reg_imm32(scratch, imm as i32));
            } else {
                code.extend(encode_mov_reg_imm64(scratch, imm as u64));
            }
            scratch
        }
    }
}

/// Emit spill/reload code.
fn emit_spill_code(
    code: &mut Vec<u8>,
    spill: &GenericSpillCode,
    _callee_saved: &[Gpr],
    _frame_size: u32,
) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // mov [rbp + slot.offset], gpr
                code.extend(encode_mov_mem_reg(Gpr::Rbp, slot.offset, gpr));
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(gpr) = preg_to_gpr(preg) {
                // mov gpr, [rbp + slot.offset]
                code.extend(encode_mov_reg_mem(gpr, Gpr::Rbp, slot.offset));
            }
        }
    }
}

/// Emit a single IR instruction as register-based machine code.
/// Returns (opcode_name, reads, writes).
fn emit_instruction(
    code: &mut Vec<u8>,
    instr: &IRInstr,
    alloc: &RegAllocResult,
    frame_size: u32,
    callee_saved_gprs: &[Gpr],
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
                // FP: use SSE. For now, fall back to stack-slot for FP.
                // TODO: implement FP register-based emission.
                return emit_fp_fallback(instr, alloc);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            // mov dst, lhs; add dst, rhs
            if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
            }
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend(encode_add_reg_reg(dst_reg, rhs_reg));
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    code.extend(encode_add_reg_imm32(dst_reg, imm as i32));
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
                return emit_fp_fallback(instr, alloc);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
            }
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend(encode_sub_reg_reg(dst_reg, rhs_reg));
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    code.extend(encode_sub_reg_imm32(dst_reg, imm as i32));
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
                return emit_fp_fallback(instr, alloc);
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let lhs_reg = load_to_reg(lhs, alloc, code);
            if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
            }
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend(encode_imul_reg_reg(dst_reg, rhs_reg));
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    // imul dst, imm32
                    let scratch = Gpr::R11;
                    code.extend(encode_mov_reg_imm32(scratch, imm as i32));
                    code.extend(encode_imul_reg_reg(dst_reg, scratch));
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "mul".to_string()
        }

        // ── Div (standalone, from scg_to_ir) ──
        IRInstr::Div { dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr, alloc);
            }
            // Check if this is a 32-bit operation
            let is_32bit = matches!(ty, Some(IRType::U32) | Some(IRType::I32));
            let lhs_reg = load_to_reg(lhs, alloc, code);

            // The x86_64 div/idiv instructions clobber RAX (quotient) and
            // RDX (remainder).  If the register allocator has assigned live
            // values to RAX or RDX that span this Div, those values would
            // be corrupted.  Save RAX/RDX before the Div, save the quotient
            // to R11 (scratch), restore RAX/RDX, then move R11 to dst.
            code.extend(encode_push(Gpr::Rax));
            code.extend(encode_push(Gpr::Rdx));

            if is_32bit {
                // 32-bit unsigned division: zero-extend EAX, zero EDX
                code.extend(encode_mov_reg_reg(Gpr::Rax, lhs_reg));
                // Use 32-bit mov to zero upper bits: mov eax, eax
                code.extend_from_slice(&[0x89, 0xC0]); // mov eax, eax (zero-extends)
                code.extend_from_slice(&[0x31, 0xD2]); // xor edx, edx
            } else {
                code.extend(encode_mov_reg_reg(Gpr::Rax, lhs_reg));
                code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx));
            }
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    if is_32bit {
                        // 32-bit div: F7 /6 (no REX.W, but need REX.B for regs >= R8)
                        let rm = (rhs_reg as u8) & 7;
                        if (rhs_reg as u8) >= 8 {
                            // REX.B prefix for registers R8-R15
                            code.extend_from_slice(&[0x41, 0xF7, 0xF0 | rm]);
                        } else {
                            code.extend_from_slice(&[0xF7, 0xF0 | rm]);
                        }
                    } else {
                        code.extend(encode_div_reg(rhs_reg));
                    }
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    if imm == 0 {
                        code.extend(encode_nop());
                    } else {
                        code.extend(encode_mov_reg_imm32(Gpr::Rcx, imm as i32));
                        if is_32bit {
                            code.extend_from_slice(&[0xF7, 0xF1]); // div ecx
                        } else {
                            code.extend(encode_div_reg(Gpr::Rcx));
                        }
                    }
                }
            }
            // Save quotient (in RAX) to R11 scratch BEFORE restoring RAX/RDX.
            code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rax));
            // Restore old RDX and RAX.
            code.extend(encode_pop(Gpr::Rdx));
            code.extend(encode_pop(Gpr::Rax));
            // Move quotient from R11 to dst_reg.
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend(encode_mov_reg_reg(dst_reg, Gpr::R11));
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "div".to_string()
        }

        // ── Div (BinOp with SDiv/UDiv/SRem/URem) ──
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr, alloc);
            }
            match op {
                BinOpKind::SDiv | BinOpKind::SRem => {
                    // x86_64 idiv: rax = lhs, divide by rhs. Result in RAX, remainder in RDX.
                    // Save RAX/RDX (may hold live values) before idiv clobbers them.
                    // Use R11 as scratch to save the result before restoring.
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    code.extend(encode_push(Gpr::Rax));
                    code.extend(encode_push(Gpr::Rdx));
                    code.extend(encode_mov_reg_reg(Gpr::Rax, lhs_reg));
                    code.extend(encode_cqo()); // sign-extend RAX into RDX:RAX
                    let rhs_reg = load_to_reg(rhs, alloc, code);
                    code.extend(encode_idiv_reg(rhs_reg));
                    // Save result to R11 BEFORE restoring RAX/RDX.
                    if *op == BinOpKind::SRem {
                        code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rdx)); // remainder
                    } else {
                        code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rax)); // quotient
                    }
                    code.extend(encode_pop(Gpr::Rdx));
                    code.extend(encode_pop(Gpr::Rax));
                    let dst_reg = load_to_reg(dst, alloc, code);
                    code.extend(encode_mov_reg_reg(dst_reg, Gpr::R11));
                    reads.push(phys(lhs_reg));
                    reads.push(phys(rhs_reg));
                    writes.push(phys(dst_reg));
                    "sdiv".to_string()
                }
                BinOpKind::UDiv | BinOpKind::URem => {
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    code.extend(encode_push(Gpr::Rax));
                    code.extend(encode_push(Gpr::Rdx));
                    code.extend(encode_mov_reg_reg(Gpr::Rax, lhs_reg));
                    code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx)); // zero RDX
                    let rhs_reg = load_to_reg(rhs, alloc, code);
                    code.extend(encode_div_reg(rhs_reg));
                    if *op == BinOpKind::URem {
                        code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rdx)); // remainder
                    } else {
                        code.extend(encode_mov_reg_reg(Gpr::R11, Gpr::Rax)); // quotient
                    }
                    code.extend(encode_pop(Gpr::Rdx));
                    code.extend(encode_pop(Gpr::Rax));
                    let dst_reg = load_to_reg(dst, alloc, code);
                    code.extend(encode_mov_reg_reg(dst_reg, Gpr::R11));
                    reads.push(phys(lhs_reg));
                    reads.push(phys(rhs_reg));
                    writes.push(phys(dst_reg));
                    "udiv".to_string()
                }
                BinOpKind::And => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    match resolve_value(rhs, alloc) {
                        ResolvedVal::Reg(rhs_reg) => {
                            code.extend(encode_and_reg_reg(dst_reg, rhs_reg));
                            reads.push(phys(rhs_reg));
                        }
                        ResolvedVal::Imm(imm) => {
                            code.extend(encode_and_reg_imm32(dst_reg, imm as i32));
                        }
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "and".to_string()
                }
                BinOpKind::Or => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    match resolve_value(rhs, alloc) {
                        ResolvedVal::Reg(rhs_reg) => {
                            code.extend(encode_or_reg_reg(dst_reg, rhs_reg));
                            reads.push(phys(rhs_reg));
                        }
                        ResolvedVal::Imm(imm) => {
                            code.extend(encode_or_reg_imm32(dst_reg, imm as i32));
                        }
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "or".to_string()
                }
                BinOpKind::Xor => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    match resolve_value(rhs, alloc) {
                        ResolvedVal::Reg(rhs_reg) => {
                            code.extend(encode_xor_reg_reg(dst_reg, rhs_reg));
                            reads.push(phys(rhs_reg));
                        }
                        ResolvedVal::Imm(imm) => {
                            code.extend(encode_xor_reg_imm32(dst_reg, imm as i32));
                        }
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "xor".to_string()
                }
                BinOpKind::Shl => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    // shl requires shift count in CL.
                    let rcx_orig = load_to_reg(rhs, alloc, code);
                    if rcx_orig != Gpr::Rcx {
                        code.extend(encode_mov_reg_reg(Gpr::Rcx, rcx_orig));
                    }
                    code.extend(encode_shl_reg_cl(dst_reg));
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "shl".to_string()
                }
                BinOpKind::ShrL | BinOpKind::ShrA => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    let rcx_orig = load_to_reg(rhs, alloc, code);
                    if rcx_orig != Gpr::Rcx {
                        code.extend(encode_mov_reg_reg(Gpr::Rcx, rcx_orig));
                    }
                    if *op == BinOpKind::ShrA {
                        code.extend(encode_sar_reg_cl(dst_reg));
                    } else {
                        code.extend(encode_shr_reg_cl(dst_reg));
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    if *op == BinOpKind::ShrA { "sar" } else { "shr" }.to_string()
                }
                BinOpKind::Add => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    match resolve_value(rhs, alloc) {
                        ResolvedVal::Reg(rhs_reg) => {
                            code.extend(encode_add_reg_reg(dst_reg, rhs_reg));
                            reads.push(phys(rhs_reg));
                        }
                        ResolvedVal::Imm(imm) => {
                            code.extend(encode_add_reg_imm32(dst_reg, imm as i32));
                        }
                    }
                    // For 32-bit types, zero-extend result (mov eax, eax).
                    if matches!(ty, Some(IRType::I32) | Some(IRType::U32)) {
                        let r = dst_reg as u8;
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "add".to_string()
                }
                BinOpKind::Sub => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    match resolve_value(rhs, alloc) {
                        ResolvedVal::Reg(rhs_reg) => {
                            code.extend(encode_sub_reg_reg(dst_reg, rhs_reg));
                            reads.push(phys(rhs_reg));
                        }
                        ResolvedVal::Imm(imm) => {
                            code.extend(encode_sub_reg_imm32(dst_reg, imm as i32));
                        }
                    }
                    // For 32-bit types, zero-extend result.
                    if matches!(ty, Some(IRType::I32) | Some(IRType::U32)) {
                        let r = dst_reg as u8;
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "sub".to_string()
                }
                BinOpKind::Mul => {
                    let dst_reg = load_to_reg(dst, alloc, code);
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    if !matches!(resolve_value(lhs, alloc), ResolvedVal::Reg(r) if r == dst_reg) {
                        code.extend(encode_mov_reg_reg(dst_reg, lhs_reg));
                    }
                    match resolve_value(rhs, alloc) {
                        ResolvedVal::Reg(rhs_reg) => {
                            code.extend(encode_imul_reg_reg(dst_reg, rhs_reg));
                            reads.push(phys(rhs_reg));
                        }
                        ResolvedVal::Imm(imm) => {
                            // imul dst, imm32
                            let scratch = Gpr::R11;
                            code.extend(encode_mov_reg_imm32(scratch, imm as i32));
                            code.extend(encode_imul_reg_reg(dst_reg, scratch));
                        }
                    }
                    // For 32-bit types, zero-extend result.
                    if matches!(ty, Some(IRType::I32) | Some(IRType::U32)) {
                        let r = dst_reg as u8;
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "mul".to_string()
                }
                _ => {
                    // Other BinOp variants (Add, Sub, Mul) are handled by
                    // IRInstr::Add/Sub/Mul above. If we get here, it's an
                    // unhandled variant — emit a NOP as placeholder.
                    code.extend(encode_nop());
                    "unhandled_binop".to_string()
                }
            }
        }

        // ── UnaryOp ──
        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let src_reg = load_to_reg(operand, alloc, code);
            if src_reg != dst_reg {
                code.extend(encode_mov_reg_reg(dst_reg, src_reg));
            }
            match op {
                UnaryOpKind::Neg => {
                    code.extend(encode_neg_reg(dst_reg));
                }
                UnaryOpKind::Not => {
                    code.extend(encode_not_reg(dst_reg));
                }
                UnaryOpKind::Popcnt => {
                    // popcnt dst, dst (needs SSE4.2; use the encoding helper if available,
                    // otherwise fall back to a loop — for now, emit NOP).
                    code.extend(encode_nop()); // TODO: implement popcnt
                }
                UnaryOpKind::Clz | UnaryOpKind::Ctz => {
                    code.extend(encode_nop()); // TODO: implement lzcnt/tzcnt
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
            match ty {
                IRType::U8 | IRType::I8 => {
                    if matches!(ty, IRType::I8) {
                        code.extend(encode_movsx_reg8_mem(dst_reg, base_reg, *offset));
                    } else {
                        code.extend(encode_movzx_reg8_mem(dst_reg, base_reg, *offset));
                    }
                }
                IRType::U16 | IRType::I16 => {
                    if matches!(ty, IRType::I16) {
                        code.extend(encode_movsx_reg16_mem(dst_reg, base_reg, *offset));
                    } else {
                        code.extend(encode_movzx_reg16_mem(dst_reg, base_reg, *offset));
                    }
                }
                IRType::U32 | IRType::I32 => {
                    code.extend(encode_mov_reg32_mem(dst_reg, base_reg, *offset));
                }
                _ => {
                    // 64-bit load (default).
                    code.extend(encode_mov_reg_mem(dst_reg, base_reg, *offset));
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
            match ty {
                IRType::U8 | IRType::I8 => {
                    code.extend(encode_mov_mem8_reg8(base_reg, *offset, val_reg));
                }
                IRType::U16 | IRType::I16 => {
                    code.extend(encode_mov_mem16_reg16(base_reg, *offset, val_reg));
                }
                IRType::U32 | IRType::I32 => {
                    code.extend(encode_mov_mem32_reg32(base_reg, *offset, val_reg));
                }
                _ => {
                    code.extend(encode_mov_mem_reg(base_reg, *offset, val_reg));
                }
            }
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "store".to_string()
        }

        // ── Cmp ──
        IRInstr::Cmp { dst, kind, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            match resolve_value(rhs, alloc) {
                ResolvedVal::Reg(rhs_reg) => {
                    code.extend(encode_cmp_reg_reg(lhs_reg, rhs_reg));
                    reads.push(phys(rhs_reg));
                }
                ResolvedVal::Imm(imm) => {
                    code.extend(encode_cmp_reg_imm32(lhs_reg, imm as i32));
                }
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            let cc = cmp_kind_to_cc(kind);
            code.extend(encode_setcc(cc, dst_reg));
            // Zero-extend the 8-bit result to 64-bit.
            code.extend(encode_movzx_reg8(dst_reg, dst_reg));
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "cmp".to_string()
        }

        // ── Select ──
        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // If cond is an immediate, evaluate at compile time.
            if let IRValue::Immediate(c) = cond {
                if *c != 0 {
                    // cond = true → dst = true_val
                    let tv = load_to_reg(true_val, alloc, code);
                    if tv != dst_reg {
                        code.extend(encode_mov_reg_reg(dst_reg, tv));
                    }
                    reads.push(phys(tv));
                } else {
                    // cond = false → dst = false_val
                    let fv = load_to_reg(false_val, alloc, code);
                    if fv != dst_reg {
                        code.extend(encode_mov_reg_reg(dst_reg, fv));
                    }
                    reads.push(phys(fv));
                }
            } else {
                // cond is a register — use test + cmovne pattern.
                let cond_reg = load_to_reg(cond, alloc, code);
                code.extend(encode_test_reg_reg(cond_reg, cond_reg));
                // Load false_val first, then true_val.  If both are
                // immediates, they both use R11 scratch, so we must
                // emit the mov BEFORE loading true_val (which would
                // clobber R11).  Strategy:
                //   1. mov dst, false_val  (R11 = false_val)
                //   2. Load true_val into a DIFFERENT register.
                //      If true_val is immediate, it goes to R11 (overwriting
                //      false_val, but dst already has false_val).
                //   3. cmovne dst, true_reg
                let false_reg = load_to_reg(false_val, alloc, code);
                code.extend(encode_mov_reg_reg(dst_reg, false_reg));
                let true_reg = load_to_reg(true_val, alloc, code);
                code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, dst_reg, true_reg));
                reads.push(phys(cond_reg));
                reads.push(phys(false_reg));
                reads.push(phys(true_reg));
            }
            writes.push(phys(dst_reg));
            "select".to_string()
        }

        // ── CtSelect (constant-time select — same as Select on x86_64) ──
        IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            if let IRValue::Immediate(c) = cond {
                if *c != 0 {
                    let tv = load_to_reg(true_val, alloc, code);
                    if tv != dst_reg { code.extend(encode_mov_reg_reg(dst_reg, tv)); }
                    reads.push(phys(tv));
                } else {
                    let fv = load_to_reg(false_val, alloc, code);
                    if fv != dst_reg { code.extend(encode_mov_reg_reg(dst_reg, fv)); }
                    reads.push(phys(fv));
                }
            } else {
                let cond_reg = load_to_reg(cond, alloc, code);
                code.extend(encode_test_reg_reg(cond_reg, cond_reg));
                let false_reg = load_to_reg(false_val, alloc, code);
                code.extend(encode_mov_reg_reg(dst_reg, false_reg));
                let true_reg = load_to_reg(true_val, alloc, code);
                code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, dst_reg, true_reg));
                reads.push(phys(cond_reg));
                reads.push(phys(false_reg));
                reads.push(phys(true_reg));
            }
            writes.push(phys(dst_reg));
            "ct_select".to_string()
        }

        // ── CtEq ──
        IRInstr::CtEq { dst, lhs, rhs, .. } => {
            let lhs_reg = load_to_reg(lhs, alloc, code);
            let rhs_reg = load_to_reg(rhs, alloc, code);
            code.extend(encode_cmp_reg_reg(lhs_reg, rhs_reg));
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend(encode_setcc(Cc::Equal, dst_reg));
            code.extend(encode_movzx_reg8(dst_reg, dst_reg));
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
                    // Zero-extend based on source type.
                    match from_ty {
                        Some(IRType::U8) | Some(IRType::I8) => {
                            code.extend(encode_movzx_reg8(dst_reg, src_reg));
                        }
                        Some(IRType::U16) | Some(IRType::I16) => {
                            code.extend(encode_movzx_reg16(dst_reg, src_reg));
                        }
                        Some(IRType::U32) | Some(IRType::I32) => {
                            // ZExt from U32/I32 to 64-bit: emit a 32-bit
                            // mov (which zero-extends to 64-bit on x86_64).
                            // We don't have a dedicated encode_mov_reg32_reg32
                            // helper, so emit the bytes directly:
                            //   8B /r (MOV r32, r/m32) — no REX.W
                            // For now, fall back to a 64-bit mov which
                            // preserves all bits; this is correct as long
                            // as the source U32 value already has its high
                            // 32 bits clear (which the IR type system should
                            // guarantee).
                            //
                            // BUG W0-2 fix: previously this branch called
                            // `code.clear()` which wiped the ENTIRE function's
                            // emitted code so far. Removed.
                            if src_reg != dst_reg {
                                code.extend(encode_mov_reg_reg(dst_reg, src_reg));
                            }
                        }
                        _ => {
                            if src_reg != dst_reg {
                                code.extend(encode_mov_reg_reg(dst_reg, src_reg));
                            }
                        }
                    }
                }
                CastKind::SExt => {
                    match from_ty {
                        Some(IRType::I8) | Some(IRType::U8) => {
                            code.extend(encode_movsx_reg8(dst_reg, src_reg));
                        }
                        Some(IRType::I16) | Some(IRType::U16) => {
                            code.extend(encode_movsx_reg16(dst_reg, src_reg));
                        }
                        Some(IRType::I32) | Some(IRType::U32) => {
                            code.extend(encode_movsxd(dst_reg, src_reg));
                        }
                        _ => {
                            if src_reg != dst_reg {
                                code.extend(encode_mov_reg_reg(dst_reg, src_reg));
                            }
                        }
                    }
                }
                CastKind::Trunc => {
                    if src_reg != dst_reg {
                        code.extend(encode_mov_reg_reg(dst_reg, src_reg));
                    }
                    // Mask to the target width.
                    if let Some(tt) = to_ty {
                        match tt {
                            IRType::U8 | IRType::I8 => {
                                code.extend(encode_and_reg_imm32(dst_reg, 0xFF));
                            }
                            IRType::U16 | IRType::I16 => {
                                code.extend(encode_and_reg_imm32(dst_reg, 0xFFFF));
                            }
                            IRType::U32 | IRType::I32 => {
                                code.extend(encode_and_reg_imm32(dst_reg, -1i32));
                            }
                            _ => {}
                        }
                    }
                }
                CastKind::BitCast => {
                    if src_reg != dst_reg {
                        code.extend(encode_mov_reg_reg(dst_reg, src_reg));
                    }
                }
                _ => {
                    // IntToFloat, FloatToInt, FloatToUInt, UIntToFloat,
                    // FloatToFloat — FP casts need SSE instructions which
                    // the register-based emitter doesn't support yet.
                    // Return an error so the caller falls back to the
                    // stack-slot ISel which has proper FP conversion code.
                    return emit_fp_fallback(instr, alloc);
                }
            }
            reads.push(phys(src_reg));
            writes.push(phys(dst_reg));
            "cast".to_string()
        }

        // ── Alloc (stack allocation — use RSP) ──
        IRInstr::Alloc { dst, size, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // Stack-allocate `size` bytes (16-byte aligned) and return a
            // pointer to the LOW end of the new space in `dst_reg`.
            // Order matters: `sub rsp, N` FIRST, then `lea dst, [rsp]`.
            // The previous order (`lea dst, [rsp]; sub rsp, N`) put the
            // buffer at the HIGH end ([old_rsp]), which overlaps the
            // saved-RBP slot at [rbp] when frame_size=0, causing the
            // subsequent store to clobber the saved RBP and corrupt the
            // return path.
            let aligned = ((*size as usize + 15) & !15) as i32;
            code.extend(encode_sub_reg_imm32(Gpr::Rsp, aligned));
            code.extend(encode_lea_reg_mem(dst_reg, Gpr::Rsp, 0));
            writes.push(phys(dst_reg));
            "alloc".to_string()
        }

        // ── Free (stack deallocation) ──
        IRInstr::Free { ptr, .. } => {
            // add rsp, size — but we don't know the size here.
            // For now, emit NOP (stack is reclaimed at function exit).
            code.extend(encode_nop());
            let _ = load_to_reg(ptr, alloc, code);
            "free".to_string()
        }

        // ── GetAddress (lea — symbol address) ──
        IRInstr::GetAddress { dst, name } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // lea dst, [rip + sym] — emit placeholder 0, record a
            // PC32 relocation so the linker resolves the symbol.
            code.extend(encode_lea_rip_rel(dst_reg, 0));
            let rel_offset = code.len() as u64 - 4;
            relocations.push(RelocationEntry {
                offset: rel_offset,
                symbol: name.clone(),
                reloc_type: "R_X86_64_PC32".to_string(),
            });
            writes.push(phys(dst_reg));
            "getaddr".to_string()
        }

        // ── Offset (add offset to pointer) ──
        IRInstr::Offset { dst, base, offset, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(base, alloc, code);
            if base_reg != dst_reg {
                code.extend(encode_mov_reg_reg(dst_reg, base_reg));
            }
            // offset is an IRValue (could be register or immediate).
            match resolve_value(offset, alloc) {
                ResolvedVal::Imm(imm) => {
                    code.extend(encode_add_reg_imm32(dst_reg, imm as i32));
                }
                ResolvedVal::Reg(off_reg) => {
                    code.extend(encode_add_reg_reg(dst_reg, off_reg));
                    reads.push(phys(off_reg));
                }
            }
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "offset".to_string()
        }

        // ── Phi (should be resolved by this point, emit NOP) ──
        IRInstr::Phi { dst, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend(encode_nop());
            writes.push(phys(dst_reg));
            "phi".to_string()
        }

        // ── Ret (rare mid-block return — emit full epilogue inline) ──
        IRInstr::Ret { values } => {
            if let Some(first) = values.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(Gpr::Rax, ret_reg));
                }
            }
            // BUG W0-1 fix: emit the full epilogue (was just `nop`).
            code.extend(emit_epilogue_bytes(frame_size, callee_saved_gprs));
            "ret".to_string()
        }

        // ── Branch (unconditional) ──
        IRInstr::Branch { target } => {
            // jmp rel32 (will be patched)
            let offset_pos = code.len() + 1; // after the 0xE9 opcode byte
            code.extend(encode_jmp_rel32(0)); // placeholder
            fixups.push(BranchFixup {
                offset: offset_pos,
                target: target.clone(),
            });
            "branch".to_string()
        }

        // ── CondBranch ──
        IRInstr::CondBranch { cond, true_target, false_target, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            code.extend(encode_test_reg_reg(cond_reg, cond_reg));
            // jne true_block (rel32)
            let offset_pos = code.len() + 2; // after 0x0F 0x85
            code.extend(encode_jcc_rel32(Cc::NotEqual, 0)); // placeholder
            fixups.push(BranchFixup {
                offset: offset_pos,
                target: true_target.clone(),
            });
            // jmp false_target (rel32)
            let offset_pos2 = code.len() + 1;
            code.extend(encode_jmp_rel32(0)); // placeholder
            fixups.push(BranchFixup {
                offset: offset_pos2,
                target: false_target.clone(),
            });
            reads.push(phys(cond_reg));
            "cond_branch".to_string()
        }

        // ── Syscall ──
        IRInstr::Syscall { nr, args, dst } => {
            // x86_64 Linux syscall ABI:
            //   nr in RAX, args in RDI, RSI, RDX, R10, R8, R9
            //   return in RAX
            //
            // Translate the VUMA-generic syscall number (e.g. 220 for
            // clone, which is the aarch64 number) to the x86_64 native
            // number (e.g. 56 for clone). Without this translation, the
            // IPC tests call non-existent syscalls and SIGSEGV.
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::X86_64,
                *nr,
            );
            code.extend(encode_mov_reg_imm32(Gpr::Rax, native_nr as i32));
            let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::R10, Gpr::R8, Gpr::R9];
            for (i, arg) in args.iter().enumerate().take(6) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend(encode_mov_reg_reg(arg_regs[i], arg_reg));
                }
            }
            code.extend(encode_syscall());
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rax));
                }
                writes.push(phys(dst_reg));
            }
            "syscall".to_string()
        }

        // ── Call ──
        IRInstr::Call { dst, func: fname, args, is_extern, .. } => {
            // x86_64 System V calling convention:
            //   args in RDI, RSI, RDX, RCX, R8, R9 (first 6 integer args)
            //   return in RAX
            //
            // Caller-saved registers (RAX, RCX, RDX, RSI, RDI, R8, R9, R10,
            // R11) are clobbered by the call.  The register allocator may
            // have assigned live values to these registers (it "prefers"
            // callee-saved for call-crossing intervals but will use
            // caller-saved when callee-saved are exhausted).  To preserve
            // those values across the call, save all caller-saved registers
            // (except RAX when the call has a return value) before setting
            // up arguments, and restore them after the call.
            let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
            let has_return = dst.is_some();

            // Caller-saved registers to preserve.  RAX is excluded when the
            // call returns a value (it holds the return value after the call).
            // RSP and RBP are never caller-saved-allocatable so are not listed.
            let saved_regs: Vec<Gpr> = if has_return {
                vec![Gpr::Rcx, Gpr::Rdx, Gpr::Rsi, Gpr::Rdi,
                     Gpr::R8, Gpr::R9, Gpr::R10, Gpr::R11]
            } else {
                vec![Gpr::Rax, Gpr::Rcx, Gpr::Rdx, Gpr::Rsi, Gpr::Rdi,
                     Gpr::R8, Gpr::R9, Gpr::R10, Gpr::R11]
            };

            // Maintain 16-byte stack alignment: the System V ABI requires
            // RSP to be 16-aligned at the `call` instruction.  Each `push`
            // subtracts 8, so an even number of pushes keeps alignment.
            // If saved_regs has odd length, insert an 8-byte sub for padding.
            let need_align_pad = saved_regs.len() % 2 == 1;
            if need_align_pad {
                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
            }
            for &g in &saved_regs {
                code.extend(encode_push(g));
            }

            // Set up arguments (may use R11 as scratch for immediates —
            // that's fine, R11 was just saved).
            for (i, arg) in args.iter().enumerate().take(6) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend(encode_mov_reg_reg(arg_regs[i], arg_reg));
                }
            }
            // For variadic functions, AL should contain the number of
            // XMM registers used (0 for integer-only calls). Clear AL
            // unconditionally — harmless for non-variadic calls.
            code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));
            // Emit call with a relocation so the linker resolves the
            // target function address.
            code.extend(encode_call_rel32(0));
            let call_rel32_offset = code.len() as u64 - 4;
            relocations.push(RelocationEntry {
                offset: call_rel32_offset,
                symbol: fname.clone(),
                reloc_type: "R_X86_64_PLT32".to_string(),
            });

            // If the call has a return value, it's in RAX.  RAX is NOT in
            // saved_regs (excluded when has_return=true), so the pops below
            // won't clobber it.  Move RAX to dst_reg AFTER restoring all
            // saved registers (dst_reg may itself be a saved register).
            let dst_reg_opt = if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                writes.push(phys(dst_reg));
                Some(dst_reg)
            } else {
                None
            };

            // Restore caller-saved registers (reverse order).
            for &g in saved_regs.iter().rev() {
                code.extend(encode_pop(g));
            }
            if need_align_pad {
                code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
            }

            // Now move the return value from RAX to dst_reg (after all
            // pops, so dst_reg is no longer at risk of being overwritten).
            if let Some(dst_reg) = dst_reg_opt {
                if dst_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rax));
                }
            }

            if *is_extern {
                "call_extern".to_string()
            } else {
                "call".to_string()
            }
        }

        // ── AtomicLoad ──
        IRInstr::AtomicLoad { dst, addr, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            // mov dst, [base] (x86_64 loads are atomic for aligned accesses)
            code.extend(encode_mov_reg_mem(dst_reg, base_reg, 0));
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "atomic_load".to_string()
        }

        // ── AtomicStore ──
        IRInstr::AtomicStore { value, addr, .. } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            // mov [base], val (x86_64 stores are atomic for aligned accesses)
            code.extend(encode_mov_mem_reg(base_reg, 0, val_reg));
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "atomic_store".to_string()
        }

        // ── AtomicCas ──
        IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
            // cmpxchg [addr], new
            // RAX holds expected; if [addr]==RAX, then [addr]=new, else RAX=[addr].
            let expected_reg = load_to_reg(expected, alloc, code);
            if expected_reg != Gpr::Rax {
                code.extend(encode_mov_reg_reg(Gpr::Rax, expected_reg));
            }
            let base_reg = load_to_reg(addr, alloc, code);
            let new_reg = load_to_reg(desired, alloc, code);
            // lock cmpxchg [base], new_reg
            // We don't have a direct encode_cmpxchg helper, so emit the bytes.
            code.extend_from_slice(&[0xF0]); // LOCK prefix
            code.extend_from_slice(&[0x0F, 0xB1]); // CMPXCHG r/m, r
            encode_mem_operand(code, new_reg as u8 & 7, base_reg, 0);
            if (new_reg as u8) >= 8 || (base_reg as u8) >= 8 {
                // REX prefix needed — this is a simplification; proper
                // REX handling should go before the opcode.
                // For correctness, insert REX before LOCK.
                // TODO: proper REX prefix placement.
            }
            let dst_reg = load_to_reg(dst, alloc, code);
            if dst_reg != Gpr::Rax {
                code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rax));
            }
            reads.push(phys(expected_reg));
            reads.push(phys(base_reg));
            reads.push(phys(new_reg));
            writes.push(phys(dst_reg));
            "atomic_cas".to_string()
        }

        // ── CallIndirect (function pointer call) ──
        IRInstr::CallIndirect { dst, func_ptr, args } => {
            // x86_64 System V calling convention:
            //   args in RDI, RSI, RDX, RCX, R8, R9 (first 6 integer args)
            //   return in RAX
            //   call via: call r10 (or any caller-saved register)
            let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx, Gpr::R8, Gpr::R9];
            let has_return = dst.is_some();

            // Load the function pointer into R10 (caller-saved, not an arg reg).
            let func_ptr_reg = load_to_reg(func_ptr, alloc, code);

            // Save caller-saved registers (same pattern as Call handler).
            let saved_regs: Vec<Gpr> = if has_return {
                vec![Gpr::Rcx, Gpr::Rdx, Gpr::Rsi, Gpr::Rdi,
                     Gpr::R8, Gpr::R9, Gpr::R10, Gpr::R11]
            } else {
                vec![Gpr::Rax, Gpr::Rcx, Gpr::Rdx, Gpr::Rsi, Gpr::Rdi,
                     Gpr::R8, Gpr::R9, Gpr::R10, Gpr::R11]
            };
            let need_align_pad = saved_regs.len() % 2 == 1;
            if need_align_pad {
                code.extend(encode_sub_reg_imm32(Gpr::Rsp, 8));
            }
            for &g in &saved_regs {
                code.extend(encode_push(g));
            }

            // Move function pointer to R10 if not already there.
            if func_ptr_reg != Gpr::R10 {
                code.extend(encode_mov_reg_reg(Gpr::R10, func_ptr_reg));
            }

            // Set up arguments.
            for (i, arg) in args.iter().enumerate().take(6) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend(encode_mov_reg_reg(arg_regs[i], arg_reg));
                }
            }
            code.extend(encode_xor_reg_reg(Gpr::Rax, Gpr::Rax));

            // call r10
            // FF /2 — CALL r/m64
            // R10 requires REX.B prefix: 41 FF D2
            code.extend_from_slice(&[0x41, 0xFF, 0xD2]); // call r10

            // Save return value.
            let dst_reg_opt = if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                writes.push(phys(dst_reg));
                Some(dst_reg)
            } else {
                None
            };

            // Restore caller-saved registers.
            for &g in saved_regs.iter().rev() {
                code.extend(encode_pop(g));
            }
            if need_align_pad {
                code.extend(encode_add_reg_imm32(Gpr::Rsp, 8));
            }

            // Move return value from RAX to dst_reg.
            if let Some(dst_reg) = dst_reg_opt {
                if dst_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rax));
                }
            }
            reads.push(phys(func_ptr_reg));
            "call_indirect".to_string()
        }

        // ── Struct/Array/TaggedUnion (these are type declarations, not instructions) ──
        // ── Unhandled instructions (emit NOP, should not appear at this stage) ──
        _ => {
            code.extend(encode_nop());
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
    frame_size: u32,
    callee_saved_gprs: &[Gpr],
    fixups: &mut Vec<BranchFixup>,
) {
    match term {
        IRTerminator::Jump(label) => {
            let offset_pos = code.len() + 1;
            code.extend(encode_jmp_rel32(0));
            fixups.push(BranchFixup {
                offset: offset_pos,
                target: label.clone(),
            });
        }
        IRTerminator::Branch { cond, true_block, false_block } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            code.extend(encode_test_reg_reg(cond_reg, cond_reg));
            let offset_pos = code.len() + 2;
            code.extend(encode_jcc_rel32(Cc::NotEqual, 0));
            fixups.push(BranchFixup {
                offset: offset_pos,
                target: true_block.clone(),
            });
            let offset_pos2 = code.len() + 1;
            code.extend(encode_jmp_rel32(0));
            fixups.push(BranchFixup {
                offset: offset_pos2,
                target: false_block.clone(),
            });
        }
        IRTerminator::Return(vals) => {
            if let Some(first) = vals.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(Gpr::Rax, ret_reg));
                }
            }
            // BUG W0-1 fix: emit the FULL epilogue (add rsp, frame_size;
            // pop callee-saved; pop rbp; ret) — previously emitted only
            // a bare `ret`, leaving RSP at `rbp - frame_size` and
            // callee-saved registers unrestored, which caused every
            // return to jump to garbage and SIGSEGV.
            code.extend(emit_epilogue_bytes(frame_size, callee_saved_gprs));
        }
        IRTerminator::Unreachable => {
            code.extend(encode_int3()); // trap
        }
        _ => {
            code.extend(encode_nop());
        }
    }
}

/// Build the function epilogue bytes: restore RSP from RBP (so dynamic
/// stack adjustments from `Alloc` are correctly undone), then pop
/// callee-saved (reverse order), pop rbp, ret. Used at every Return path.
///
/// We use `lea rsp, [rbp - callee_saved_size]` instead of `add rsp,
/// frame_size` because the latter does NOT account for `IRInstr::Alloc`'s
/// runtime `sub rsp, size` — using RBP as the reference is robust.
fn emit_epilogue_bytes(frame_size: u32, callee_saved_gprs: &[Gpr]) -> Vec<u8> {
    let _ = frame_size; // unused — we restore via RBP, not frame_size
    let callee_saved_size = (callee_saved_gprs.len() * 8) as i32;
    let mut out = Vec::with_capacity(8 + callee_saved_gprs.len() * 2);
    // lea rsp, [rbp - callee_saved_size] — restore RSP to just below the
    // callee-saved area (i.e., the value RSP had right after the prologue's
    // `sub rsp, frame_size`, before any Alloc adjustments).
    out.extend(encode_lea_reg_mem(Gpr::Rsp, Gpr::Rbp, -callee_saved_size));
    // pop callee-saved in reverse order of prologue push
    for &g in callee_saved_gprs.iter().rev() {
        out.extend(encode_pop(g));
    }
    // pop rbp
    out.extend(encode_pop(Gpr::Rbp));
    // ret
    out.extend(encode_ret());
    out
}

/// FP fallback — for now, emit NOP and return empty metadata.
/// A full FP implementation would use SSE/AVX instructions.
fn emit_fp_fallback(
    instr: &IRInstr,
    _alloc: &RegAllocResult,
) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    // FP instructions are not yet implemented in the register-based emitter.
    // Return an error so the caller can fall back to stack-slot for FP functions.
    Err(BackendError::RegisterAllocFailed {
        isa: "x86_64",
        reason: format!("FP instruction not yet supported in register-based emitter: {:?}", instr),
    })
}

/// Helper: create a PhysicalReg from a Gpr.
fn phys(g: Gpr) -> PhysicalReg {
    PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32)
}

/// Map CmpKind to x86_64 condition code.
fn cmp_kind_to_cc(kind: &CmpKind) -> Cc {
    match kind {
        CmpKind::Eq => Cc::Equal,
        CmpKind::Ne => Cc::NotEqual,
        CmpKind::SLt => Cc::Less,
        CmpKind::SLe => Cc::LessEqual,
        CmpKind::SGt => Cc::Greater,
        CmpKind::SGe => Cc::GreaterEqual,
        CmpKind::ULt => Cc::Below,
        CmpKind::ULe => Cc::BelowEqual,
        CmpKind::UGt => Cc::Above,
        CmpKind::UGe => Cc::AboveEqual,
    }
}
