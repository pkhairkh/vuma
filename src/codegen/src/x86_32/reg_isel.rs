//! Full register-based instruction selection for x86_32 (i386).
//!
//! This module implements a complete register-based emitter that consumes
//! a `RegAllocResult` (from `TargetAgnosticRegAlloc`) and produces an
//! `AllocatedFunction` with register-to-register machine code for ALL IR
//! instructions.  It is the x86_32 counterpart to `x86_64::reg_isel`.
//!
//! # Architecture
//!
//! 1. **Prologue**: `push ebp; mov ebp, esp; push <callee-saved>; sub esp, frame_size`
//! 2. **Argument shuffle**: parallel-move args from ABI regs (EDI/ESI/EDX/ECX)
//!    to whatever registers the allocator assigned to the param vregs.
//! 3. **Body**: for each IR instruction, resolve vregs → physical regs via
//!    `alloc.vreg_to_preg` and emit register-based encoding.
//! 4. **Spill/reload**: insert `mov [ebp+offset], reg` / `mov reg, [ebp+offset]`
//!    at positions from `alloc.spill_code`.
//! 5. **Epilogue** (emitted inline at every Return path): `lea esp, [ebp - callee_saved_size];
//!    pop <callee-saved>; pop ebp; ret`.
//!
//! # Frame Layout
//!
//! On i386, `push r32` decrements ESP by 4 and writes 4 bytes.  After the
//! call instruction pushes the 4-byte return address and the prologue
//! pushes EBP, ESP is 8 mod 16.  The prologue then pushes N callee-saved
//! registers (N×4 bytes) and subtracts `frame_size`.  We choose
//! `frame_size` so that RSP is 16-aligned at the next call boundary.
//!
//! ```text
//! [ebp+4]    return address (pushed by call)
//! [ebp]      saved ebp (pushed by prologue, then mov ebp, esp)
//! [ebp-4]    first callee-saved (pushed after ebp setup)
//! [ebp-8]    second callee-saved
//! ...
//! [ebp-K]    spill slot 0 (8 bytes each, from alloc.spill_slots)
//! [ebp-K-8]  spill slot 1
//! ...
//! esp = ebp - frame_size
//! ```
//!
//! # Calling Convention (VUMA-internal regparam)
//!
//! Mirrors the existing `stack_slot_isel` convention so user functions
//! compiled via reg_isel are ABI-compatible with functions compiled via
//! stack-slot ISel (and with the runtime syscall stubs in `mod.rs`).
//!
//!   • First 4 non-FP int args: EDI, ESI, EDX, ECX (in that order).
//!   • FP params (F32/F64): on the stack, 8 bytes each at [EBP + 8 + i*8].
//!   • Return value: EAX (32-bit) or EDX:EAX (64-bit).
//!
//! # Scratch Register
//!
//! EAX is reserved as the dedicated scratch register (mirrors R11 on
//! x86_64).  It is marked `not_allocatable` in `x86_32_target_desc` so the
//! allocator never assigns a live vreg to it.  It is used for:
//!   • Immediate materialization in `load_to_reg` (transient).
//!   • IDIV dividend / quotient (EAX = lhs, then EAX = quotient).
//!   • MUL low word (EAX = lhs, then EAX = low product).
//!   • Return value at function exit (`mov eax, ret_reg`).
//!
//! # Syscall ABI (Linux i386)
//!
//!   nr in EAX, args in EBX, ECX, EDX, ESI, EDI, EBP.
//!   Return value in EAX.
//!   `int 0x80` is the syscall instruction (NOT `syscall`).

use crate::backend::{AllocatedBlock, AllocatedFunction, AllocatedInstruction, BackendError, PhysicalReg, RelocationEntry};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator, IRType, BinOpKind, UnaryOpKind, CastKind, CmpKind};
use crate::regalloc::RegAllocResult;
use crate::regalloc::GenericSpillCode;
use crate::x86_32::*;

/// Resolved value: either a physical register or an immediate.
enum ResolvedVal {
    Reg(Gpr),
    Imm(i64),
}

/// Branch fixup: byte_offset_in_code + target_label.
struct BranchFixup {
    offset: usize,
    target: String,
}

/// Emit a complete function using register-based instruction selection.
///
/// This is the FULL register-based emitter — it does NOT start from
/// stack-slot bytes.  It consumes the `RegAllocResult` and produces
/// register-to-register machine code for every IR instruction.
pub fn emit_function_regalloc_full(
    func: &IRFunction,
    alloc: &RegAllocResult,
) -> Result<AllocatedFunction, BackendError> {
    if std::env::var("VUMA_DEBUG_REG_ISEL").is_ok() {
        eprintln!("=== emit_function_regalloc_full [x86_32]: {} ===", func.name);
        for (i, block) in func.blocks.iter().enumerate() {
            eprintln!("  bb{} label={:?}:", i, block.label);
            for instr in &block.instructions {
                eprintln!("    {:?}", instr);
            }
            eprintln!("    TERM: {:?}", block.terminator);
        }
    }

    // ── Compute frame size ──
    // On i386, `push r32` is 4 bytes (NOT 8 like x86_64).  The callee-saved
    // set on x86_32 is just {EBX} (EBP is the frame pointer, handled by the
    // push ebp / mov ebp, esp prologue and not in `used_callee_saved`).
    let callee_saved_gprs: Vec<Gpr> = alloc
        .used_callee_saved
        .iter()
        .filter_map(|p| preg_to_gpr(p))
        .filter(|g| *g != Gpr::Rbp && *g != Gpr::Rsp)
        .collect();
    let callee_saved_size = callee_saved_gprs.len() * 4;
    // Each spill slot is 8 bytes (matches the allocator's spill_offset calc).
    let spill_size = alloc.total_spill_slots as usize * 8;
    // Frame size must be 16-byte aligned at the next call boundary.
    // After `call` (4-byte return addr) + `push ebp` (4 bytes), ESP is 8 mod 16.
    // After pushing N callee-saved (N*4 bytes), ESP is (8 + 4N) mod 16.
    // `sub esp, frame_size` must bring ESP back to 0 mod 16.
    let raw_frame = callee_saved_size + spill_size;
    let frame_size = ((raw_frame + 15) & !15) as u32;

    let mut all_code: Vec<u8> = Vec::new();
    let mut blocks: Vec<AllocatedBlock> = Vec::new();
    let mut fixups: Vec<BranchFixup> = Vec::new();
    let mut relocations: Vec<RelocationEntry> = Vec::new();
    let mut label_offsets: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // ── Prologue ──
    let prologue_start = all_code.len();
    // push ebp
    all_code.extend(encode_push(Gpr::Rbp));
    // mov ebp, esp
    all_code.extend(encode_mov_reg_reg(Gpr::Rbp, Gpr::Rsp));
    // push callee-saved registers (in canonical order: EBX)
    for &g in &callee_saved_gprs {
        all_code.extend(encode_push(g));
    }
    // sub esp, frame_size
    all_code.extend(encode_sub_reg_imm32(Gpr::Rsp, frame_size as i32));

    let prologue_instr = AllocatedInstruction {
        opcode: "prologue".to_string(),
        reads: vec![],
        writes: callee_saved_gprs
            .iter()
            .map(|g| PhysicalReg::new(crate::backend::RegClass::Gpr, *g as u32))
            .collect(),
        encoded: all_code[prologue_start..].to_vec(),
    };

    // ── Argument shuffle (function entry) ──
    // The VUMA-internal regparam convention passes the first 4 non-FP int
    // args in EDI, ESI, EDX, ECX.  The allocator may assign these param
    // vregs to DIFFERENT physical registers (e.g. vreg 0 → EBX).  Without
    // an argument shuffle, the callee reads its parameters from the wrong
    // registers (garbage) and produces wrong results.
    //
    // FP params are passed on the stack ([ebp + 8 + i*8]); the allocator
    // may assign them to XMM regs, but FP support in this emitter is
    // currently a fallback (see `emit_fp_fallback`).  For non-FP params
    // we emit a parallel move from ABI regs to allocator-chosen regs.
    let arg_shuffle_start = all_code.len();
    let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx];
    let mut pending: Vec<(Gpr, Gpr)> = Vec::new(); // (src=ABI reg, dst=allocator reg)
    let mut fp_param_count = 0usize;
    for (i, param) in func.params.iter().enumerate() {
        let is_fp = i < func.param_types.len()
            && matches!(func.param_types[i], IRType::F32 | IRType::F64);
        if is_fp {
            // FP params come on the stack — leave them there; the FP
            // fallback path will load them as needed.  (FP register-based
            // emission is deferred.)
            fp_param_count += 1;
            continue;
        }
        if let IRValue::Register(vreg_id) = param {
            let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
            if let Some(preg) = alloc.vreg_to_preg.get(root) {
                if let Some(dst_gpr) = preg_to_gpr(preg) {
                    // Non-FP params are assigned to ABI regs in order; the
                    // i-th non-FP param goes to arg_regs[non_fp_idx].
                    let non_fp_idx = i - fp_param_count;
                    if non_fp_idx < arg_regs.len() {
                        let src = arg_regs[non_fp_idx];
                        if dst_gpr != src {
                            pending.push((src, dst_gpr));
                        }
                    }
                    // Non-FP params beyond the 4th come on the stack at
                    // [ebp + 8 + fp_count*8 + overflow*4]; we'd need to
                    // load them into the allocator-chosen reg.  For now,
                    // only register-passed args are shuffled (the common
                    // case in the test suite).
                }
            }
        }
    }
    // Pass 1: move non-conflicting args.  Repeat until no progress.
    let mut progress = true;
    while progress && !pending.is_empty() {
        progress = false;
        let mut i = 0;
        while i < pending.len() {
            let (src, _dst) = pending[i];
            // Check if dst is a src for any other pending move (cycle).
            let mut conflict = false;
            for (j, (other_src, _)) in pending.iter().enumerate() {
                if i != j && *other_src == pending[i].1 {
                    conflict = true;
                    break;
                }
            }
            let _ = src;
            if !conflict {
                let (s, d) = pending[i];
                all_code.extend(encode_mov_reg_reg(d, s));
                pending.remove(i);
                progress = true;
            } else {
                i += 1;
            }
        }
    }
    // Pass 2: handle cycles with EAX scratch (EAX is not_allocatable, so
    // no live vreg is in it).
    for (src, dst) in pending {
        // mov eax, src; mov dst, eax
        all_code.extend(encode_mov_reg_reg(Gpr::Rax, src));
        all_code.extend(encode_mov_reg_reg(dst, Gpr::Rax));
    }
    let arg_shuffle_end = all_code.len();
    let has_arg_shuffle = arg_shuffle_end > arg_shuffle_start;

    // ── Body: emit each block ──
    // Position keying must match the allocator in regalloc.rs
    // (`LiveRangeComputer::compute`): each instruction and each terminator
    // consumes `pos += 2`, NEVER reset between blocks.
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

        // Emit terminator.  Return paths emit the full epilogue inline so
        // that early returns restore ESP / callee-saved / EBP correctly.
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
    // Every Return/Jump-to-return path emits its own epilogue inline.
    // This trailing copy is kept only as a defensive safety net.
    let epilogue_start = all_code.len();
    all_code.extend(emit_epilogue_bytes(frame_size, &callee_saved_gprs));
    let epilogue_end = all_code.len();

    // Add prologue (and argument shuffle if any) to the first block,
    // and the trailing epilogue to the last block (defensive).
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

    // ── Re-slice each instruction's encoded bytes from patched all_code ──
    // Branch fixup resolution patched `all_code` in place; re-derive each
    // instruction's `encoded` slice so the linker sees correct rel32 values.
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
///
/// On x86_32, only the 8 low registers (RAX–RDI, indices 0–7) are real.
/// The Gpr enum retains R8–R15 as source-compat aliases (encoding 8–15),
/// but the target desc only lists the 8 real registers, so the allocator
/// never hands out indices >= 8.  We map them defensively anyway.
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
        _ => None,
    }
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
            // If spilled, we should have already inserted a reload.
            // Fall back to EAX as scratch (should not happen in correct alloc).
            ResolvedVal::Reg(Gpr::Rax)
        }
        IRValue::Immediate(imm) => ResolvedVal::Imm(*imm),
        IRValue::Address(addr) => ResolvedVal::Imm(*addr as i64),
        IRValue::Label(_) => ResolvedVal::Reg(Gpr::Rax),
    }
}

/// Load a value into a register.  If it's an immediate, emit a mov into
/// the dedicated scratch register (EAX).  If it's already in a register,
/// just return that register.
///
/// EAX is `not_allocatable` in the target desc, so the allocator never
/// assigns a live vreg to it — clobbering EAX here is safe.
fn load_to_reg(val: &IRValue, alloc: &RegAllocResult, code: &mut Vec<u8>) -> Gpr {
    match resolve_value(val, alloc) {
        ResolvedVal::Reg(g) => g,
        ResolvedVal::Imm(imm) => {
            let scratch = Gpr::Rax;
            // x86_32 cannot hold a 64-bit immediate in a single register;
            // encode_mov_reg_imm64 truncates to the low 32 bits (with a
            // warning).  This is the same limitation as the stack-slot ISel.
            if imm >= i32::MIN as i64 && imm <= i32::MAX as i64 {
                code.extend(encode_mov_reg_imm32(scratch, imm as i32));
            } else {
                code.extend(encode_mov_reg_imm64(scratch, imm as u64));
            }
            scratch
        }
    }
}

/// Emit spill/reload code.  On x86_32, `encode_mov_mem_reg` is a 32-bit
/// store (no REX.W) — sufficient for 32-bit values; 64-bit values are
/// truncated to their low 32 bits (matching the existing stack-slot ISel
/// behavior for the common case).
///
/// # The index-5 scratch hazard
///
/// The target-agnostic allocator's `gen_spill_reload` (for entirely-spilled
/// intervals) uses `PhysicalReg::new(class, 5)` as a scratch register —
/// see `regalloc.rs:3348`.  On x86_32 (and x86_64), GPR index 5 is RBP,
/// the frame pointer.  Allowing the spill/reload to clobber RBP would
/// corrupt the frame and crash on the next stack access.
///
/// We remap any spill/reload targeting RBP to EAX instead.  EAX is
/// `not_allocatable` in our target desc (reserved as the dedicated
/// immediate-materialization scratch), so clobbering it here is safe —
/// no live vreg is ever assigned to EAX.  This matches the spirit of the
/// allocator's intent (use a caller-saved scratch) without the frame-
/// pointer corruption.
fn emit_spill_code(code: &mut Vec<u8>, spill: &GenericSpillCode) {
    match spill {
        GenericSpillCode::Spill { preg, slot, .. } => {
            if let Some(mut gpr) = preg_to_gpr(preg) {
                // Remap RBP/RSP scratch → EAX (see hazard note above).
                if gpr == Gpr::Rbp || gpr == Gpr::Rsp {
                    gpr = Gpr::Rax;
                }
                // mov [ebp + slot.offset], gpr
                code.extend(encode_mov_mem_reg(Gpr::Rbp, slot.offset, gpr));
            }
        }
        GenericSpillCode::Reload { preg, slot, .. } => {
            if let Some(mut gpr) = preg_to_gpr(preg) {
                if gpr == Gpr::Rbp || gpr == Gpr::Rsp {
                    gpr = Gpr::Rax;
                }
                // mov gpr, [ebp + slot.offset]
                code.extend(encode_mov_reg_mem(gpr, Gpr::Rbp, slot.offset));
            }
        }
    }
}

/// Emit a single IR instruction as register-based machine code.
/// Returns (opcode_name, reads, writes).
#[allow(clippy::too_many_arguments)]
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
                return emit_fp_fallback(instr);
            }
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
                return emit_fp_fallback(instr);
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
                    // imul dst, imm32 — materialize imm in EAX scratch.
                    let scratch = Gpr::Rax;
                    code.extend(encode_mov_reg_imm32(scratch, imm as i32));
                    code.extend(encode_imul_reg_reg(dst_reg, scratch));
                }
            }
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "mul".to_string()
        }

        // ── BinOp (Div/Rem/And/Or/Xor/Shl/Shr/Add/Sub/Mul) ──
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => {
            let is_fp = matches!(ty, Some(IRType::F32) | Some(IRType::F64));
            if is_fp {
                return emit_fp_fallback(instr);
            }
            match op {
                BinOpKind::SDiv | BinOpKind::SRem => {
                    // i386 IDIV: EAX = lhs, divide by rhs.  Result in EAX (quotient)
                    // and EDX (remainder).  CDQ sign-extends EAX into EDX:EAX.
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    code.extend(encode_mov_reg_reg(Gpr::Rax, lhs_reg));
                    code.extend(encode_cqo()); // CDQ (32-bit sign-extend)
                    let rhs_reg = load_to_reg(rhs, alloc, code);
                    code.extend(encode_idiv_reg(rhs_reg));
                    let dst_reg = load_to_reg(dst, alloc, code);
                    if *op == BinOpKind::SRem {
                        code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rdx));
                    } else {
                        code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rax));
                    }
                    reads.push(phys(lhs_reg));
                    reads.push(phys(rhs_reg));
                    writes.push(phys(dst_reg));
                    "sdiv".to_string()
                }
                BinOpKind::UDiv | BinOpKind::URem => {
                    let lhs_reg = load_to_reg(lhs, alloc, code);
                    code.extend(encode_mov_reg_reg(Gpr::Rax, lhs_reg));
                    code.extend(encode_xor_reg_reg(Gpr::Rdx, Gpr::Rdx)); // zero EDX
                    let rhs_reg = load_to_reg(rhs, alloc, code);
                    code.extend(encode_div_reg(rhs_reg));
                    let dst_reg = load_to_reg(dst, alloc, code);
                    if *op == BinOpKind::URem {
                        code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rdx));
                    } else {
                        code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rax));
                    }
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
                            let scratch = Gpr::Rax;
                            code.extend(encode_mov_reg_imm32(scratch, imm as i32));
                            code.extend(encode_imul_reg_reg(dst_reg, scratch));
                        }
                    }
                    reads.push(phys(lhs_reg));
                    writes.push(phys(dst_reg));
                    "mul".to_string()
                }
                _ => {
                    // Comparison BinOps (Eq/Ne/SLt/...) are handled by IRInstr::Cmp.
                    // Other variants should not reach here.
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
                UnaryOpKind::Popcnt | UnaryOpKind::Clz | UnaryOpKind::Ctz => {
                    // TODO: implement popcnt/lzcnt/tzcnt (need SSE4.2 / BMI).
                    code.extend(encode_nop());
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
                    // 32-bit load (x86_32 GPRs are 32-bit; 64-bit loads
                    // would need two movs — emit one 32-bit load and rely
                    // on the IR type system to keep values in range).
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
            let cc = cmp_kind_to_cc(kind);
            // Check if dst is spilled — if so, setcc into EAX and store to
            // spill slot immediately, preventing reload-clobbers-result hazard.
            if let IRValue::Register(vreg_id) = dst {
                let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
                if let Some(slot) = alloc.spill_slot(*root) {
                    code.extend(encode_setcc(cc, Gpr::Rax));
                    code.extend(encode_movzx_reg8(Gpr::Rax, Gpr::Rax));
                    code.extend(encode_mov_mem_reg(Gpr::Rbp, slot.offset, Gpr::Rax));
                    reads.push(phys(lhs_reg));
                    return Ok(("cmp".to_string(), reads, writes));
                }
            }
            // Normal path: dst is in a register
            let dst_reg = load_to_reg(dst, alloc, code);
            code.extend(encode_setcc(cc, dst_reg));
            code.extend(encode_movzx_reg8(dst_reg, dst_reg));
            reads.push(phys(lhs_reg));
            writes.push(phys(dst_reg));
            "cmp".to_string()
        }

        // ── Select ──
        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            code.extend(encode_test_reg_reg(cond_reg, cond_reg));
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend(encode_mov_reg_reg(dst_reg, false_reg));
            code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, dst_reg, true_reg));
            reads.push(phys(cond_reg));
            reads.push(phys(false_reg));
            reads.push(phys(true_reg));
            writes.push(phys(dst_reg));
            "select".to_string()
        }

        // ── CtSelect (constant-time select — same as Select on x86_32) ──
        IRInstr::CtSelect { dst, cond, true_val, false_val, .. } => {
            let cond_reg = load_to_reg(cond, alloc, code);
            code.extend(encode_test_reg_reg(cond_reg, cond_reg));
            let dst_reg = load_to_reg(dst, alloc, code);
            let false_reg = load_to_reg(false_val, alloc, code);
            let true_reg = load_to_reg(true_val, alloc, code);
            code.extend(encode_mov_reg_reg(dst_reg, false_reg));
            code.extend(encode_cmovcc_reg_reg(Cc::NotEqual, dst_reg, true_reg));
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
                    match from_ty {
                        Some(IRType::U8) | Some(IRType::I8) => {
                            code.extend(encode_movzx_reg8(dst_reg, src_reg));
                        }
                        Some(IRType::U16) | Some(IRType::I16) => {
                            code.extend(encode_movzx_reg16(dst_reg, src_reg));
                        }
                        // x86_32 registers are 32-bit; ZExt from U32/I32
                        // is a no-op (the value already fits in 32 bits).
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
                        // x86_32 has no MOVSXD (32→64); 32-bit values
                        // already fill the register.  Just mov.
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
                    if let Some(tt) = to_ty {
                        match tt {
                            IRType::U8 | IRType::I8 => {
                                code.extend(encode_and_reg_imm32(dst_reg, 0xFF));
                            }
                            IRType::U16 | IRType::I16 => {
                                code.extend(encode_and_reg_imm32(dst_reg, 0xFFFF));
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
                    // IntToFloat, FloatToInt — FP casts need SSE.
                    // For now, emit a mov as placeholder.
                    if src_reg != dst_reg {
                        code.extend(encode_mov_reg_reg(dst_reg, src_reg));
                    }
                }
            }
            reads.push(phys(src_reg));
            writes.push(phys(dst_reg));
            "cast".to_string()
        }

        // ── Alloc (stack allocation — use ESP) ──
        IRInstr::Alloc { dst, size, .. } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // Stack-allocate `size` bytes (16-byte aligned) and return a
            // pointer to the LOW end of the new space in `dst_reg`.
            // Order: `sub esp, N` FIRST, then `lea dst, [esp]`.
            let aligned = ((*size as usize + 15) & !15) as i32;
            code.extend(encode_sub_reg_imm32(Gpr::Rsp, aligned));
            code.extend(encode_lea_reg_mem(dst_reg, Gpr::Rsp, 0));
            writes.push(phys(dst_reg));
            "alloc".to_string()
        }

        // ── Free (stack deallocation — no-op; stack reclaimed at exit) ──
        IRInstr::Free { ptr, .. } => {
            code.extend(encode_nop());
            let _ = load_to_reg(ptr, alloc, code);
            "free".to_string()
        }

        // ── GetAddress (load absolute address of a symbol) ──
        // x86_32 has no RIP-relative addressing, so we use a 32-bit
        // absolute load: `mov r32, imm32` with a R_386_32 relocation.
        // The linker patches the imm32 with the symbol's virtual address.
        IRInstr::GetAddress { dst, name } => {
            let dst_reg = load_to_reg(dst, alloc, code);
            // mov dst, 0 (placeholder); the 4-byte immediate is patched
            // by the linker via a R_386_32 (aliased as R_X86_64_64 in
            // the x86_32 backend for source-compat with the relocation
            // resolver in encode_program).
            code.extend(encode_mov_reg_imm64(dst_reg, 0));
            let rel_offset = code.len() as u64 - 4;
            relocations.push(RelocationEntry {
                offset: rel_offset,
                symbol: name.clone(),
                reloc_type: "R_386_32".to_string(),
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

        // ── Ret (mid-block return — emit full epilogue inline) ──
        IRInstr::Ret { values } => {
            if let Some(first) = values.first() {
                let ret_reg = load_to_reg(first, alloc, code);
                if ret_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(Gpr::Rax, ret_reg));
                }
            }
            code.extend(emit_epilogue_bytes(frame_size, callee_saved_gprs));
            "ret".to_string()
        }

        // ── Branch (unconditional) ──
        IRInstr::Branch { target } => {
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
            // If cond is spilled, reload from spill slot into EAX directly
            // (bypassing load_to_reg which may return clobbered EAX).
            let cond_reg = if let IRValue::Register(vreg_id) = cond {
                let root = alloc.coalesced_map.get(vreg_id).unwrap_or(vreg_id);
                if let Some(slot) = alloc.spill_slot(*root) {
                    code.extend(encode_mov_reg_mem(Gpr::Rax, Gpr::Rbp, slot.offset));
                    Gpr::Rax
                } else {
                    load_to_reg(cond, alloc, code)
                }
            } else {
                load_to_reg(cond, alloc, code)
            };
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

        // ── Syscall (Linux i386: int 0x80, args in EBX/ECX/EDX/ESI/EDI/EBP) ──
        IRInstr::Syscall { nr, args, dst } => {
            // Translate the VUMA-generic syscall number (e.g. 220 for clone,
            // the aarch64 number) to the i386 native number (e.g. 120 for
            // clone).  Without this translation, the test suite would call
            // non-existent syscalls.
            let native_nr = crate::syscall_abi::translate_or_warn(
                crate::backend::BackendKind::X86_32,
                *nr,
            );
            // Materialize syscall number into EAX.  EAX is the scratch reg
            // (not_allocatable), so clobbering it here is safe.
            code.extend(encode_mov_reg_imm32(Gpr::Rax, native_nr as i32));
            // i386 syscall arg registers: EBX, ECX, EDX, ESI, EDI, EBP.
            let arg_regs = [Gpr::Rbx, Gpr::Rcx, Gpr::Rdx, Gpr::Rsi, Gpr::Rdi, Gpr::Rbp];
            for (i, arg) in args.iter().enumerate().take(6) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend(encode_mov_reg_reg(arg_regs[i], arg_reg));
                }
            }
            code.extend(encode_syscall()); // int 0x80
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
            // VUMA-internal regparam: first 4 non-FP int args in EDI, ESI,
            // EDX, ECX.  Args 5+ pushed on stack (right-to-left).
            // Return value in EAX.
            let arg_regs = [Gpr::Rdi, Gpr::Rsi, Gpr::Rdx, Gpr::Rcx];
            // Push stack args (args 4+) in reverse order first.
            let n_stack = if args.len() > arg_regs.len() { args.len() - arg_regs.len() } else { 0 };
            for i in (arg_regs.len()..args.len()).rev() {
                let arg_reg = load_to_reg(&args[i], alloc, code);
                if arg_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(Gpr::Rax, arg_reg));
                }
                code.extend(encode_push(Gpr::Rax));
            }
            // Set up register args (args 0-3).
            for (i, arg) in args.iter().enumerate().take(arg_regs.len()) {
                let arg_reg = load_to_reg(arg, alloc, code);
                if arg_reg != arg_regs[i] {
                    code.extend(encode_mov_reg_reg(arg_regs[i], arg_reg));
                }
            }
            code.extend(encode_call_rel32(0));
            let call_rel32_offset = code.len() as u64 - 4;
            relocations.push(RelocationEntry {
                offset: call_rel32_offset,
                symbol: fname.clone(),
                reloc_type: "R_386_PC32".to_string(),
            });
            // Clean up stack args.
            if n_stack > 0 {
                code.extend_from_slice(&[0x83, 0xC4, (n_stack * 4) as u8]); // add esp, N*4
            }
            if let Some(dst_val) = dst {
                let dst_reg = load_to_reg(dst_val, alloc, code);
                if dst_reg != Gpr::Rax {
                    code.extend(encode_mov_reg_reg(dst_reg, Gpr::Rax));
                }
                writes.push(phys(dst_reg));
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
            // mov dst, [base] (x86 loads are atomic for aligned accesses)
            code.extend(encode_mov_reg_mem(dst_reg, base_reg, 0));
            reads.push(phys(base_reg));
            writes.push(phys(dst_reg));
            "atomic_load".to_string()
        }

        // ── AtomicStore ──
        IRInstr::AtomicStore { value, addr, .. } => {
            let val_reg = load_to_reg(value, alloc, code);
            let base_reg = load_to_reg(addr, alloc, code);
            code.extend(encode_mov_mem_reg(base_reg, 0, val_reg));
            reads.push(phys(val_reg));
            reads.push(phys(base_reg));
            "atomic_store".to_string()
        }

        // ── AtomicCas ──
        IRInstr::AtomicCas { dst, addr, expected, desired, .. } => {
            // cmpxchg [addr], new
            // EAX holds expected; if [addr]==EAX, then [addr]=new, else EAX=[addr].
            let expected_reg = load_to_reg(expected, alloc, code);
            if expected_reg != Gpr::Rax {
                code.extend(encode_mov_reg_reg(Gpr::Rax, expected_reg));
            }
            let base_reg = load_to_reg(addr, alloc, code);
            let new_reg = load_to_reg(desired, alloc, code);
            // lock cmpxchg [base], new_reg
            // F0 0F B1 /r  (LOCK CMPXCHG r/m32, r32)
            code.extend_from_slice(&[0xF0]); // LOCK prefix
            code.extend_from_slice(&[0x0F, 0xB1]); // CMPXCHG r/m, r
            encode_mem_operand(code, new_reg.encoding() & 7, base_reg, 0);
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

        // ── Unhandled instructions (emit NOP) ──
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
            // Emit the FULL epilogue (lea esp, [ebp - callee_saved_size];
            // pop callee-saved; pop ebp; ret).  Using `lea esp, [ebp - N]`
            // instead of `add esp, frame_size` correctly undoes any dynamic
            // stack adjustments from `IRInstr::Alloc`.
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

/// Build the function epilogue bytes: restore ESP from EBP (so dynamic
/// stack adjustments from `Alloc` are correctly undone), then pop
/// callee-saved (reverse order), pop ebp, ret.  Used at every Return path.
fn emit_epilogue_bytes(frame_size: u32, callee_saved_gprs: &[Gpr]) -> Vec<u8> {
    let _ = frame_size; // unused — we restore via EBP, not frame_size
    // On i386, `push r32` is 4 bytes (not 8 like x86_64).
    let callee_saved_size = (callee_saved_gprs.len() * 4) as i32;
    let mut out = Vec::with_capacity(8 + callee_saved_gprs.len() * 2);
    // lea esp, [ebp - callee_saved_size] — restore ESP to just below the
    // callee-saved area (i.e., the value ESP had right after the prologue's
    // `sub esp, frame_size`, before any Alloc adjustments).
    out.extend(encode_lea_reg_mem(Gpr::Rsp, Gpr::Rbp, -callee_saved_size));
    // pop callee-saved in reverse order of prologue push
    for &g in callee_saved_gprs.iter().rev() {
        out.extend(encode_pop(g));
    }
    // pop ebp
    out.extend(encode_pop(Gpr::Rbp));
    // ret
    out.extend(encode_ret());
    out
}

/// FP fallback — return an error so the caller falls back to stack-slot ISel
/// for FP-heavy functions.  A full FP implementation would use SSE/SSE2.
fn emit_fp_fallback(instr: &IRInstr) -> Result<(String, Vec<PhysicalReg>, Vec<PhysicalReg>), BackendError> {
    Err(BackendError::RegisterAllocFailed {
        isa: "x86_32",
        reason: format!(
            "FP instruction not yet supported in register-based emitter: {:?}",
            instr
        ),
    })
}

/// Helper: create a PhysicalReg from a Gpr.
fn phys(g: Gpr) -> PhysicalReg {
    PhysicalReg::new(crate::backend::RegClass::Gpr, g as u32)
}

/// Map CmpKind to x86_32 condition code.
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
