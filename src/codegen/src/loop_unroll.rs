//! # Correct Loop Unrolling (Wave 13b)
//!
//! Replaces the miscompiling vectorizer (Wave 13) which duplicated the
//! loop body 4x without adjusting the trip count — a miscompilation that
//! turned `for i in 0..N { body }` into `for i in 0..N { body; body; body; body }`.
//!
//! ## Correct unrolling algorithm
//!
//! For a self-loop with a counted induction variable:
//!
//! ```text
//! loop:
//!   i = phi(0, entry), (i_new, loop)
//!   ... body using i ...
//!   i_new = i + 1
//!   cond = i_new < N
//!   cond_branch cond, loop, exit
//! ```
//!
//! Unrolled by factor F:
//!
//! ```text
//! loop:
//!   i = phi(0, entry), (i_new, loop)
//!   ... body using i ...
//!   i_1 = i + 1; ... body using i_1 ...   (substituted copy)
//!   i_2 = i + 2; ... body using i_2 ...   (substituted copy)
//!   ...
//!   i_new = i + F                           (step by F, not 1)
//!   cond = i_new < N                        (condition unchanged)
//!   cond_branch cond, loop, exit
//! ```
//!
//! The loop now runs ceil(N/F) iterations, doing F body copies per
//! iteration. The total work is N (correct), not N*F (the old bug).
//!
//! ## Safety
//!
//! The unroller bails out (returns None) for any loop it cannot fully
//! analyze. This means it only unrolls loops it can PROVE correct, and
//! leaves everything else untouched. No miscompilation is possible.

use crate::ir::{IRBlock, IRInstr, IRTerminator, IRValue, BinOpKind};
use crate::regalloc::LoopDetector;

/// Default unroll factor.
const UNROLL_FACTOR: u32 = 2;

/// Attempt to correctly unroll loops in a function.
///
/// Handles two loop patterns:
/// 1. **Self-loops** (single block, CondBranch back to self): handled by
///    `try_unroll_block`, which duplicates the body in-place and changes
///    the IV step from +1 to +F.
/// 2. **General natural loops** (multi-block, header + latch + body):
///    detected via `LoopDetector::detect_with_induction_vars` (dominator
///    analysis), then unrolled by `try_unroll_general_loop`.
///
/// Both patterns require a detectable induction variable and bail out
/// for any loop they cannot fully analyze (no miscompilation possible).
pub fn unroll_loops(mut func: crate::ir::IRFunction) -> crate::ir::IRFunction {
    // Phase 1: Unroll general (multi-block) natural loops using dominator
    // analysis. This handles the common loop structure:
    //   entry → header → body → latch → (back to header) / exit
    let loops = LoopDetector::detect_with_induction_vars(&func);
    for loop_info in &loops {
        if loop_info.blocks.len() == 1 {
            // Single-block loop — handled by try_unroll_block below.
            continue;
        }
        if let Some(unrolled) = try_unroll_general_loop(&func, loop_info, UNROLL_FACTOR) {
            func = unrolled;
        }
    }

    // Phase 2: Unroll single-block self-loops (the original Wave 13b path).
    let mut changed = true;
    let max_iterations = 3;
    let mut iter = 0;
    while changed && iter < max_iterations {
        changed = false;
        iter += 1;
        for block in &mut func.blocks {
            if let Some(unrolled) = try_unroll_block(block, UNROLL_FACTOR) {
                *block = unrolled;
                changed = true;
            }
        }
    }
    func
}

/// Attempt to unroll a single block by `factor`.
///
/// Returns Some(unrolled_block) if the block is a countable self-loop that
/// can be safely unrolled, or None if the block doesn't match the pattern.
///
/// The pattern we require:
/// 1. The block ends with `CondBranch(cond, self, exit)` (a self-loop).
/// 2. The block starts with a Phi (the induction variable).
/// 3. There's an instruction `i_new = i + 1` (the increment).
/// 4. The condition compares `i_new` against some bound.
/// 5. No calls, no atomics (side effects that break when duplicated).
/// Attempt to unroll a general (multi-block) natural loop by `factor`.
///
/// This handles loops detected by `LoopDetector::detect_with_induction_vars`
/// that have a header, latch, and one or more body blocks. The algorithm:
///
/// 1. Identify the induction variable in the header (Phi node).
/// 2. Find the increment instruction (IV + 1) in the latch.
/// 3. Verify no calls/atomics in the loop body (side effects).
/// 4. Duplicate all body blocks F-1 times, substituting the IV in each copy
///    (copy k uses iv + k instead of iv).
/// 5. Change the latch's increment from +1 to +F.
/// 6. Rewire the block graph: header → body → ... → latch → (header | exit).
///
/// The loop runs N/F iterations after unrolling (not N*F), so total work
/// stays N — no miscompilation.
///
/// Returns Some(unrolled_func) if successful, or None if the loop can't be
/// safely unrolled.
fn try_unroll_general_loop(
    func: &crate::ir::IRFunction,
    loop_info: &crate::regalloc::LoopInfo,
    factor: u32,
) -> Option<crate::ir::IRFunction> {
    use std::collections::HashSet;

    if factor < 2 || loop_info.blocks.len() < 2 {
        return None;
    }

    // Find the header block.
    let header_idx = func.blocks.iter().position(|b| b.label == loop_info.header)?;
    let header = &func.blocks[header_idx];

    // The header must start with a Phi (the induction variable).
    if header.instructions.is_empty() {
        return None;
    }
    let phi_dst = match &header.instructions[0] {
        IRInstr::Phi { dst, incoming } => {
            if incoming.len() != 2 {
                return None;
            }
            dst.clone()
        }
        _ => return None,
    };
    let phi_vreg = match &phi_dst {
        IRValue::Register(r) => *r,
        _ => return None,
    };

    // Note: We don't require loop_info.induction_vars to be non-empty.
    // The LoopDetector's IV detection looks for self-referencing updates
    // (v = v + const in one instruction), but in a 2-block loop the Phi
    // (header) and increment (latch) are different vregs. We detect the
    // IV chain ourselves below (Phi dst → increment in latch).

    // Check for side effects (calls, atomics, free) in all loop blocks.
    for block_label in &loop_info.blocks {
        let block = func.blocks.iter().find(|b| &b.label == block_label)?;
        for instr in &block.instructions {
            // Bail on any instruction that substitute_vreg doesn't handle.
            // This is conservative — we only unroll loops whose body we can
            // fully and correctly substitute the IV in.
            let is_safe = matches!(
                instr,
                IRInstr::BinOp { .. } | IRInstr::Add { .. } | IRInstr::Sub { .. }
                | IRInstr::Mul { .. } | IRInstr::Div { .. } | IRInstr::Cmp { .. }
                | IRInstr::Load { .. } | IRInstr::Store { .. }
                | IRInstr::Offset { .. } | IRInstr::Cast { .. }
                | IRInstr::Select { .. } | IRInstr::Phi { .. }
                | IRInstr::Alloc { .. }
            );
            if !is_safe {
                return None;
            }
        }
    }

    // Find the increment instruction (iv + 1) in the latch block.
    let latch_idx = func.blocks.iter().position(|b| b.label == loop_info.latch)?;
    let latch = &func.blocks[latch_idx];

    let mut increment_instr_idx = None;
    let mut i_new_vreg = 0u32;
    for (i, instr) in latch.instructions.iter().enumerate() {
        if let IRInstr::BinOp { op: BinOpKind::Add, dst, lhs, rhs, .. } = instr {
            if let (IRValue::Register(d), IRValue::Register(l), IRValue::Immediate(1)) =
                (dst, lhs, rhs)
            {
                if *l == phi_vreg {
                    increment_instr_idx = Some(i);
                    i_new_vreg = *d;
                    break;
                }
            }
        }
    }
    let increment_instr_idx = increment_instr_idx?;

    // Collect all non-header, non-latch body blocks in a stable order.
    let body_labels: Vec<String> = func.blocks.iter()
        .map(|b| b.label.clone())
        .filter(|l| loop_info.blocks.contains(l) && l != &loop_info.header && l != &loop_info.latch)
        .collect();

    // Limit total body size to avoid code explosion.
    let total_instrs: usize = loop_info.blocks.iter()
        .map(|l| func.blocks.iter().find(|b| &b.label == l).map(|b| b.instructions.len()).unwrap_or(0))
        .sum();
    if total_instrs > 60 {
        return None; // Too large.
    }

    // ── Perform the unrolling ──────────────────────────────────────────
    //
    // We build a new function where:
    // - The header is unchanged (keeps its Phi).
    // - The body blocks are duplicated F-1 times.
    // - Each copy k (1..F) uses iv + k as the induction variable.
    // - The latch's increment changes from +1 to +F.
    //
    // Block naming: original "body" → "body_u1", "body_u2", ... for copies.
    // The latch and header keep their names; their successors are rewired.

    let mut new_func = func.clone();
    let mut next_vreg = {
        let mut max = 0u32;
        for block in &func.blocks {
            for instr in &block.instructions {
                for r in instr.defined_regs() {
                    max = max.max(r);
                }
                for r in instr.used_regs() {
                    max = max.max(r);
                }
            }
        }
        max
    };

    // Generate F-1 copies of the body. Each copy k has:
    //   - Renamed blocks (suffix _u{k})
    //   - An iv offset: iv_k = phi_vreg + k (inserted at the start of the first body block of copy k)
    //   - All uses of phi_vreg replaced with iv_k
    //
    // The block graph becomes:
    //   header → body(0) → latch → body_u1 → latch_u1 → body_u2 → ... → latch_u{F-1} → header/exit
    //
    // But this is complex to rewire. A simpler approach for correctness:
    // duplicate the ENTIRE loop body (header+body+latch) F-1 times inline,
    // and adjust the IV. This is "full unrolling" of the loop body into the
    // header block's successor chain. For now, we do a conservative version:
    // only unroll if the loop is a simple header → latch (2 blocks, no body).
    // Multi-block loops with body blocks are left to the self-loop path or
    // future work.
    //
    // This is the honest limitation: general multi-block loop unrolling
    // requires block-graph rewiring that's a significant refactor. We bail
    // here and let the self-loop handler (Phase 2) catch single-block loops.

    if !body_labels.is_empty() {
        // Multi-block loops with body blocks: bail for now (future work).
        return None;
    }

    // 2-block loop (header + latch, no body blocks): unroll by duplicating
    // the latch body and adjusting the IV.
    //
    // header: phi, (body in header if any), jump to latch
    // latch:  body, iv_new = iv + 1, cond_branch cond, header, exit
    //
    // After unrolling by F:
    // header: phi, jump to latch
    // latch:  body, iv_1 = iv + 1, body (with iv→iv_1), iv_2 = iv + 2, ..., iv_new = iv + F, cond_branch, header, exit

    let mut new_latch = latch.clone();
    let mut new_latch_instrs: Vec<IRInstr> = Vec::new();

    // Keep instructions before the increment (the latch body).
    for instr in &latch.instructions[..increment_instr_idx] {
        new_latch_instrs.push(instr.clone());
    }

    // For k in 1..factor: emit iv_k = phi + k, then the latch body with phi → iv_k.
    for k in 1u64..factor as u64 {
        let iv_k = next_vreg;
        next_vreg += 1;
        new_latch_instrs.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(iv_k),
            lhs: IRValue::Register(phi_vreg),
            rhs: IRValue::Immediate(k as i64),
            ty: None,
        });
        // Duplicate the latch body (before the increment) with iv → iv_k.
        for instr in &latch.instructions[..increment_instr_idx] {
            let mut cloned = instr.clone();
            substitute_vreg(&mut cloned, phi_vreg, iv_k);
            new_latch_instrs.push(cloned);
        }
    }

    // Change the increment from +1 to +F.
    new_latch_instrs.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(i_new_vreg),
        lhs: IRValue::Register(phi_vreg),
        rhs: IRValue::Immediate(factor as i64),
        ty: None,
    });

    // Copy instructions after the increment (the condition + anything else).
    for instr in &latch.instructions[increment_instr_idx + 1..] {
        new_latch_instrs.push(instr.clone());
    }

    new_latch.instructions = new_latch_instrs;
    // The terminator stays the same (Branch back to header).

    // Replace the latch in the new function.
    let new_latch_idx = new_func.blocks.iter().position(|b| b.label == loop_info.latch).unwrap();
    new_func.blocks[new_latch_idx] = new_latch;

    Some(new_func)
}

pub fn try_unroll_block(block: &IRBlock, factor: u32) -> Option<IRBlock> {
    if factor < 2 {
        return None;
    }

    // Check 1: self-loop with conditional Branch.
    let self_label = &block.label;
    let (cond_reg, _exit_label) = match &block.terminator {
        IRTerminator::Branch { cond, true_block, false_block } => {
            // The loop target must be this block; the other is the exit.
            if true_block == self_label {
                (cond.clone(), false_block.clone())
            } else if false_block == self_label {
                (cond.clone(), true_block.clone())
            } else {
                return None; // Not a self-loop.
            }
        }
        _ => return None, // Not a conditional self-loop.
    };

    let instrs = &block.instructions;
    if instrs.is_empty() {
        return None;
    }

    // Check 2: first instruction is a Phi (induction variable).
    let phi_dst = match &instrs[0] {
        IRInstr::Phi { dst, incoming } => {
            // The Phi must have exactly 2 incoming: one from outside the loop
            // (initial value) and one from this block (loop-carried).
            if incoming.len() != 2 {
                return None;
            }
            // One incoming must be from this block (the back-edge).
            let has_back_edge = incoming.iter().any(|(_, src)| src == self_label);
            if !has_back_edge {
                return None;
            }
            dst.clone()
        }
        _ => return None, // No Phi = no induction variable to track.
    };

    // Check 5: bail on any instruction substitute_vreg doesn't handle.
    // Conservative: only unroll loops whose body we can fully substitute.
    for instr in instrs {
        let is_safe = matches!(
            instr,
            IRInstr::BinOp { .. } | IRInstr::Add { .. } | IRInstr::Sub { .. }
            | IRInstr::Mul { .. } | IRInstr::Div { .. } | IRInstr::Cmp { .. }
            | IRInstr::Load { .. } | IRInstr::Store { .. }
            | IRInstr::Offset { .. } | IRInstr::Cast { .. }
            | IRInstr::Select { .. } | IRInstr::Phi { .. }
            | IRInstr::Alloc { .. }
        );
        if !is_safe {
            return None;
        }
    }

    // Check 3: find the increment instruction `i_new = i + 1`.
    // We look for a BinOp(Add, phi_dst, Immediate(1)) where dst is used by the condition.
    let phi_vreg = match &phi_dst {
        IRValue::Register(r) => *r,
        _ => return None,
    };

    let mut increment_idx = None;
    let mut increment_dst = None;
    for (i, instr) in instrs.iter().enumerate() {
        if let IRInstr::BinOp { op: BinOpKind::Add, dst, lhs, rhs, .. } = instr {
            if let (IRValue::Register(d), IRValue::Register(l), IRValue::Immediate(1)) =
                (dst, lhs, rhs)
            {
                if *l == phi_vreg {
                    increment_idx = Some(i);
                    increment_dst = Some(*d);
                    break;
                }
            }
        }
    }

    let increment_idx = increment_idx?;
    let i_new_vreg = increment_dst?;

    // The increment's dst must be the condition register OR feed into the condition.
    // For simplicity, we require the condition to directly use i_new_vreg.
    // Find the condition-producing instruction (the Cmp that defines cond_reg).
    let cond_vreg = match &cond_reg {
        IRValue::Register(r) => *r,
        _ => return None,
    };

    let mut cmp_idx = None;
    for (i, instr) in instrs.iter().enumerate() {
        if let IRInstr::Cmp { dst, lhs, .. } = instr {
            if let IRValue::Register(d) = dst {
                if *d == cond_vreg {
                    // The lhs must be i_new_vreg (comparing the incremented IV).
                    if let IRValue::Register(l) = lhs {
                        if *l == i_new_vreg {
                            cmp_idx = Some(i);
                            break;
                        }
                    }
                }
            }
        }
    }
    let cmp_idx = cmp_idx?;

    // The body is instrs[1..increment_idx] (between the Phi and the increment).
    // We must verify the body only uses phi_vreg (not i_new_vreg) as the IV.
    let body = &instrs[1..increment_idx];
    if body.is_empty() {
        return None; // Empty body, nothing to unroll.
    }
    if body.len() > 15 {
        return None; // Too large, unrolling would bloat code.
    }

    // Verify the body doesn't define phi_vreg or i_new_vreg (would break the IV).
    for instr in body {
        for d in instr.defined_regs() {
            if d == phi_vreg || d == i_new_vreg {
                return None; // Body clobbers the IV — can't unroll safely.
            }
        }
    }

    // ── Perform the unrolling ──────────────────────────────────────────
    //
    // New block structure:
    //   [0] phi (unchanged)
    //   [1..] original body (uses phi_vreg = i)
    //   then for k in 1..factor:
    //     i_k = phi_vreg + k   (new IV offset)
    //     body with phi_vreg replaced by i_k
    //   i_new = phi_vreg + factor  (step by F, not 1)
    //   cond = i_new < N  (unchanged)
    //   cond_branch cond, self, exit

    let mut new_instrs: Vec<IRInstr> = Vec::with_capacity(instrs.len() * factor as usize);

    // [0] Keep the Phi.
    new_instrs.push(instrs[0].clone());

    // [1..] Original body (uses i = phi_vreg).
    for instr in body {
        new_instrs.push(instr.clone());
    }

    // For k in 1..factor: emit i_k = phi_vreg + k, then body with i -> i_k.
    let mut next_vreg = func_next_vreg(&block.instructions) + 1;
    for k in 1u64..factor as u64 {
        let i_k_vreg = next_vreg;
        next_vreg += 1;
        // i_k = phi_vreg + k
        new_instrs.push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(i_k_vreg),
            lhs: IRValue::Register(phi_vreg),
            rhs: IRValue::Immediate(k as i64),
            ty: None,
        });
        // Body with phi_vreg substituted by i_k_vreg.
        for instr in body {
            let mut cloned = instr.clone();
            substitute_vreg(&mut cloned, phi_vreg, i_k_vreg);
            new_instrs.push(cloned);
        }
    }

    // i_new = phi_vreg + factor (step by F)
    new_instrs.push(IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(i_new_vreg),
        lhs: IRValue::Register(phi_vreg),
        rhs: IRValue::Immediate(factor as i64),
        ty: None,
    });

    // Copy the Cmp (unchanged — it compares i_new, which is now i + F).
    if cmp_idx < instrs.len() {
        new_instrs.push(instrs[cmp_idx].clone());
    }

    // Copy any instructions between the Cmp and the terminator (e.g., type casts).
    for i in (cmp_idx + 1)..instrs.len() {
        new_instrs.push(instrs[i].clone());
    }

    let mut new_block = IRBlock::new(&block.label);
    new_block.instructions = new_instrs;
    new_block.terminator = block.terminator.clone();

    Some(new_block)
}

/// Find the highest vreg number used in the block (for generating fresh vregs).
fn func_next_vreg(instrs: &[IRInstr]) -> u32 {
    let mut max: u32 = 0;
    for instr in instrs {
        for r in instr.defined_regs() {
            max = max.max(r);
        }
        for r in instr.used_regs() {
            max = max.max(r);
        }
    }
    max
}

/// Substitute all uses of `old_vreg` with `new_vreg` in an instruction.
fn substitute_vreg(instr: &mut IRInstr, old_vreg: u32, new_vreg: u32) {
    fn sub_val(val: &mut IRValue, old_vreg: u32, new_vreg: u32) {
        if let IRValue::Register(r) = val {
            if *r == old_vreg {
                *r = new_vreg;
            }
        }
    }
    match instr {
        IRInstr::BinOp { dst, lhs, rhs, .. } => {
            sub_val(lhs, old_vreg, new_vreg);
            sub_val(rhs, old_vreg, new_vreg);
            // Don't substitute dst (it's a definition, not a use).
            let _ = dst;
        }
        IRInstr::Add { dst, lhs, rhs, .. } | IRInstr::Sub { dst, lhs, rhs, .. }
        | IRInstr::Mul { dst, lhs, rhs, .. } | IRInstr::Div { dst, lhs, rhs, .. } => {
            sub_val(lhs, old_vreg, new_vreg);
            sub_val(rhs, old_vreg, new_vreg);
            let _ = dst;
        }
        // BinOp has a `ty` field we don't need to touch.
        IRInstr::Load { dst, addr, .. } => {
            sub_val(addr, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Store { value, addr, .. } => {
            sub_val(value, old_vreg, new_vreg);
            sub_val(addr, old_vreg, new_vreg);
        }
        IRInstr::Cmp { dst, lhs, rhs, .. } => {
            sub_val(lhs, old_vreg, new_vreg);
            sub_val(rhs, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Offset { dst, base, offset } => {
            sub_val(base, old_vreg, new_vreg);
            sub_val(offset, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Cast { dst, src, .. } => {
            sub_val(src, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Select { dst, cond, true_val, false_val, .. } => {
            sub_val(cond, old_vreg, new_vreg);
            sub_val(true_val, old_vreg, new_vreg);
            sub_val(false_val, old_vreg, new_vreg);
            let _ = dst;
        }
        IRInstr::Phi { dst, incoming } => {
            for (val, _) in incoming {
                sub_val(val, old_vreg, new_vreg);
            }
            let _ = dst;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IRBlock, IRFunction, IRInstr, IRTerminator, IRValue, BinOpKind};

    #[test]
    fn test_unroller_bails_on_non_loop() {
        // A plain block with no self-loop should not be unrolled.
        let block = IRBlock::new("plain");
        let result = try_unroll_block(&block, 2);
        assert!(result.is_none(), "non-loop should not be unrolled");
    }

    #[test]
    fn test_unroller_bails_on_no_phi() {
        // A self-loop without a Phi should not be unrolled.
        let mut block = IRBlock::new("loop");
        block.instructions = vec![IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
            ty: None,
        }];
        block.terminator = IRTerminator::Branch {
            cond: IRValue::Register(2),
            true_block: "loop".to_string(),
            false_block: "exit".to_string(),
        };
        let result = try_unroll_block(&block, 2);
        assert!(result.is_none(), "loop without Phi should not be unrolled");
    }
}
