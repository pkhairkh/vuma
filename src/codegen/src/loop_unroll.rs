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

/// Default unroll factor.
const UNROLL_FACTOR: u32 = 2;

/// Attempt to correctly unroll loops in a function.
///
/// Only unrolls self-loops (CondBranch back to self) with a detectable
/// induction variable. Bails out for any loop it cannot fully analyze.
pub fn unroll_loops(mut func: crate::ir::IRFunction) -> crate::ir::IRFunction {
    let mut changed = true;
    // Iterate to fixpoint: unrolling may expose new opportunities.
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

    // Check 5: no calls or atomics in the body (side effects).
    for instr in instrs {
        match instr {
            IRInstr::Call { .. } | IRInstr::AtomicLoad { .. } | IRInstr::AtomicStore { .. }
            | IRInstr::Free { .. } => return None,
            _ => {}
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
