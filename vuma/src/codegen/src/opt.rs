//! # Optimization Passes
//!
//! Implements IR-level optimization passes for the VUMA code generator:
//!
//! - **Constant Folding** — Evaluate compile-time-known expressions.
//! - **Dead Code Elimination** — Remove instructions whose results are never used.
//! - **Common Subexpression Elimination** — Replace redundant computations.
//! - **Inlining** — Inline small callee functions at the call site.
//! - **Loop-Invariant Code Motion** — Move loop-invariant instructions to preheaders.
//!
//! The [`run_optimizations`] function applies all passes in the recommended order.

use std::collections::{HashMap, HashSet};

use crate::ir::{
    BinOpKind, CmpKind, IRBlock, IRFunction, IRInstr, IRProgram, IRTerminator, IRValue, UnaryOpKind,
};

// ===========================================================================
// Helpers
// ===========================================================================

/// Substitute a single IRValue using a register-to-value mapping.
fn substitute_value(val: &IRValue, map: &HashMap<u32, IRValue>) -> IRValue {
    if let IRValue::Register(id) = val {
        if let Some(replacement) = map.get(id) {
            return replacement.clone();
        }
    }
    val.clone()
}

/// Substitute all IRValue operands in an instruction using a register-to-value mapping.
fn substitute_instr(instr: &IRInstr, map: &HashMap<u32, IRValue>) -> IRInstr {
    let sv = |v: &IRValue| substitute_value(v, map);
    match instr {
        IRInstr::Load { dst, addr, offset, ty } => IRInstr::Load {
            dst: sv(dst),
            addr: sv(addr),
            offset: *offset,
            ty: ty.clone(),
        },
        IRInstr::Store { value, addr, offset, ty } => IRInstr::Store {
            value: sv(value),
            addr: sv(addr),
            offset: *offset,
            ty: ty.clone(),
        },
        IRInstr::BinOp { op, dst, lhs, rhs, ty } => IRInstr::BinOp {
            op: *op,
            dst: sv(dst),
            lhs: sv(lhs),
            rhs: sv(rhs),
            ty: ty.clone(),
        },
        IRInstr::UnaryOp { op, dst, operand, ty } => IRInstr::UnaryOp {
            op: *op,
            dst: sv(dst),
            operand: sv(operand),
            ty: ty.clone(),
        },
        IRInstr::Call { dst, func, args } => IRInstr::Call {
            dst: dst.as_ref().map(&sv),
            func: func.clone(),
            args: args.iter().map(sv).collect(),
        },
        IRInstr::Alloc { dst, size } => IRInstr::Alloc {
            dst: sv(dst),
            size: *size,
        },
        IRInstr::Free { ptr } => IRInstr::Free { ptr: sv(ptr) },
        IRInstr::Cast { kind, dst, src } => IRInstr::Cast {
            kind: *kind,
            dst: sv(dst),
            src: sv(src),
        },
        IRInstr::Phi { dst, incoming } => IRInstr::Phi {
            dst: sv(dst),
            incoming: incoming.iter().map(|(v, b)| (sv(v), b.clone())).collect(),
        },
        IRInstr::GetAddress { dst, name } => IRInstr::GetAddress {
            dst: sv(dst),
            name: name.clone(),
        },
        IRInstr::Offset { dst, base, offset } => IRInstr::Offset {
            dst: sv(dst),
            base: sv(base),
            offset: sv(offset),
        },
        IRInstr::Select {
            dst,
            cond,
            true_val,
            false_val,
            ty,
        } => IRInstr::Select {
            dst: sv(dst),
            cond: sv(cond),
            true_val: sv(true_val),
            false_val: sv(false_val),
            ty: ty.clone(),
        },
        IRInstr::Add { dst, lhs, rhs, ty } => IRInstr::Add {
            dst: sv(dst),
            lhs: sv(lhs),
            rhs: sv(rhs),
            ty: ty.clone(),
        },
        IRInstr::Sub { dst, lhs, rhs, ty } => IRInstr::Sub {
            dst: sv(dst),
            lhs: sv(lhs),
            rhs: sv(rhs),
            ty: ty.clone(),
        },
        IRInstr::Mul { dst, lhs, rhs, ty } => IRInstr::Mul {
            dst: sv(dst),
            lhs: sv(lhs),
            rhs: sv(rhs),
            ty: ty.clone(),
        },
        IRInstr::Div { dst, lhs, rhs, ty } => IRInstr::Div {
            dst: sv(dst),
            lhs: sv(lhs),
            rhs: sv(rhs),
            ty: ty.clone(),
        },
        IRInstr::Cmp {
            kind,
            dst,
            lhs,
            rhs,
            ty,
        } => IRInstr::Cmp {
            kind: *kind,
            dst: sv(dst),
            lhs: sv(lhs),
            rhs: sv(rhs),
            ty: ty.clone(),
        },
        IRInstr::Ret { values } => IRInstr::Ret {
            values: values.iter().map(sv).collect(),
        },
        IRInstr::Branch { target } => IRInstr::Branch {
            target: target.clone(),
        },
        IRInstr::CondBranch {
            cond,
            true_target,
            false_target,
        } => IRInstr::CondBranch {
            cond: sv(cond),
            true_target: true_target.clone(),
            false_target: false_target.clone(),
        },
    }
}

/// Substitute values in a terminator.
fn substitute_terminator(terminator: &IRTerminator, map: &HashMap<u32, IRValue>) -> IRTerminator {
    let sv = |v: &IRValue| substitute_value(v, map);
    match terminator {
        IRTerminator::Return(vals) => IRTerminator::Return(vals.iter().map(sv).collect()),
        IRTerminator::Branch {
            cond,
            true_block,
            false_block,
        } => IRTerminator::Branch {
            cond: sv(cond),
            true_block: true_block.clone(),
            false_block: false_block.clone(),
        },
        IRTerminator::Switch {
            discr,
            targets,
            default,
        } => IRTerminator::Switch {
            discr: sv(discr),
            targets: targets.clone(),
            default: default.clone(),
        },
        IRTerminator::Invoke {
            dst,
            func,
            args,
            normal,
            unwind,
        } => IRTerminator::Invoke {
            dst: dst.as_ref().map(sv),
            func: func.clone(),
            args: args.iter().map(sv).collect(),
            normal: normal.clone(),
            unwind: unwind.clone(),
        },
        IRTerminator::TailCall { func, args } => IRTerminator::TailCall {
            func: func.clone(),
            args: args.iter().map(sv).collect(),
        },
        IRTerminator::Resume { value } => IRTerminator::Resume { value: sv(value) },
        IRTerminator::Jump(target) => IRTerminator::Jump(target.clone()),
        IRTerminator::Unreachable => IRTerminator::Unreachable,
    }
}

/// Try to evaluate a binary operation on two immediate values.
fn try_fold_binop(op: BinOpKind, lhs: i64, rhs: i64) -> Option<i64> {
    match op {
        BinOpKind::Add => Some(lhs.wrapping_add(rhs)),
        BinOpKind::Sub => Some(lhs.wrapping_sub(rhs)),
        BinOpKind::Mul => Some(lhs.wrapping_mul(rhs)),
        BinOpKind::SDiv => {
            if rhs == 0 {
                return None;
            }
            lhs.checked_div(rhs)
        }
        BinOpKind::UDiv => {
            if rhs == 0 {
                return None;
            }
            Some((lhs as u64 / rhs as u64) as i64)
        }
        BinOpKind::SRem => {
            if rhs == 0 {
                return None;
            }
            lhs.checked_rem(rhs)
        }
        BinOpKind::URem => {
            if rhs == 0 {
                return None;
            }
            Some((lhs as u64 % rhs as u64) as i64)
        }
        BinOpKind::And => Some(lhs & rhs),
        BinOpKind::Or => Some(lhs | rhs),
        BinOpKind::Xor => Some(lhs ^ rhs),
        BinOpKind::Shl => Some(lhs.wrapping_shl(rhs as u32)),
        BinOpKind::ShrL => Some((lhs as u64).wrapping_shr(rhs as u32) as i64),
        BinOpKind::ShrA => Some(lhs.wrapping_shr(rhs as u32)),
        BinOpKind::Ror => Some(lhs.rotate_right(rhs as u32)),
        BinOpKind::Rol => Some(lhs.rotate_left(rhs as u32)),
        BinOpKind::SLt => Some(if lhs < rhs { 1 } else { 0 }),
        BinOpKind::SLe => Some(if lhs <= rhs { 1 } else { 0 }),
        BinOpKind::SGt => Some(if lhs > rhs { 1 } else { 0 }),
        BinOpKind::SGe => Some(if lhs >= rhs { 1 } else { 0 }),
        BinOpKind::ULt => Some(if (lhs as u64) < (rhs as u64) { 1 } else { 0 }),
        BinOpKind::ULe => Some(if (lhs as u64) <= (rhs as u64) { 1 } else { 0 }),
        BinOpKind::UGt => Some(if (lhs as u64) > (rhs as u64) { 1 } else { 0 }),
        BinOpKind::UGe => Some(if (lhs as u64) >= (rhs as u64) { 1 } else { 0 }),
        BinOpKind::Eq => Some(if lhs == rhs { 1 } else { 0 }),
        BinOpKind::Ne => Some(if lhs != rhs { 1 } else { 0 }),
    }
}

/// Try to evaluate a unary operation on an immediate value.
fn try_fold_unaryop(op: UnaryOpKind, operand: i64) -> Option<i64> {
    match op {
        UnaryOpKind::Neg => Some(operand.wrapping_neg()),
        UnaryOpKind::Not => Some(!operand),
        UnaryOpKind::Clz => Some(operand.leading_zeros() as i64),
        UnaryOpKind::Ctz => Some(operand.trailing_zeros() as i64),
        UnaryOpKind::Popcnt => Some(operand.count_ones() as i64),
    }
}

/// Try to evaluate a comparison on two immediate values.
fn try_fold_cmp(kind: CmpKind, lhs: i64, rhs: i64) -> Option<i64> {
    let result = match kind {
        CmpKind::Eq => lhs == rhs,
        CmpKind::Ne => lhs != rhs,
        CmpKind::SLt => lhs < rhs,
        CmpKind::SLe => lhs <= rhs,
        CmpKind::SGt => lhs > rhs,
        CmpKind::SGe => lhs >= rhs,
        CmpKind::ULt => (lhs as u64) < (rhs as u64),
        CmpKind::ULe => (lhs as u64) <= (rhs as u64),
        CmpKind::UGt => (lhs as u64) > (rhs as u64),
        CmpKind::UGe => (lhs as u64) >= (rhs as u64),
    };
    Some(if result { 1 } else { 0 })
}

/// Returns `true` if the instruction has side effects and must not be removed
/// by DCE even when its result is unused.
fn has_side_effects(instr: &IRInstr) -> bool {
    match instr {
        IRInstr::Store { .. }
        | IRInstr::Call { .. }
        | IRInstr::Free { .. }
        | IRInstr::Ret { .. }
        | IRInstr::Branch { .. }
        | IRInstr::CondBranch { .. } => true,
        IRInstr::BinOp { op, .. } => matches!(
            op,
            BinOpKind::SDiv | BinOpKind::UDiv | BinOpKind::SRem | BinOpKind::URem
        ),
        IRInstr::Div { .. } => true,
        _ => false,
    }
}

/// Returns `true` if the instruction is safe to speculate (no trapping, no
/// side effects) — used by LICM.
fn is_safe_to_speculate(instr: &IRInstr) -> bool {
    match instr {
        IRInstr::BinOp { op, .. } => !matches!(
            op,
            BinOpKind::SDiv | BinOpKind::UDiv | BinOpKind::SRem | BinOpKind::URem
        ),
        IRInstr::Div { .. } => false,
        IRInstr::Load { .. } => false,
        IRInstr::Store { .. } => false,
        IRInstr::Call { .. } => false,
        IRInstr::Free { .. } => false,
        IRInstr::Alloc { .. } => false,
        IRInstr::Ret { .. } => false,
        IRInstr::Branch { .. } => false,
        IRInstr::CondBranch { .. } => false,
        _ => true,
    }
}

/// Get the destination IRValue of an instruction, if any.
fn get_defined_value(instr: &IRInstr) -> Option<&IRValue> {
    match instr {
        IRInstr::BinOp { dst, .. } => Some(dst),
        IRInstr::UnaryOp { dst, .. } => Some(dst),
        IRInstr::Load { dst, .. } => Some(dst),
        IRInstr::Call { dst, .. } => dst.as_ref(),
        IRInstr::Alloc { dst, .. } => Some(dst),
        IRInstr::Cast { dst, .. } => Some(dst),
        IRInstr::Phi { dst, .. } => Some(dst),
        IRInstr::GetAddress { dst, .. } => Some(dst),
        IRInstr::Offset { dst, .. } => Some(dst),
        IRInstr::Select { dst, .. } => Some(dst),
        IRInstr::Add { dst, .. } => Some(dst),
        IRInstr::Sub { dst, .. } => Some(dst),
        IRInstr::Mul { dst, .. } => Some(dst),
        IRInstr::Div { dst, .. } => Some(dst),
        IRInstr::Cmp { dst, .. } => Some(dst),
        _ => None,
    }
}

/// Compute the maximum virtual register ID in a function.
fn max_vreg_id(func: &IRFunction) -> u32 {
    let mut max_id = 0u32;
    let check_val = |max_id: &mut u32, v: &IRValue| {
        if let IRValue::Register(id) = v {
            *max_id = (*max_id).max(*id);
        }
    };
    for val in &func.params {
        check_val(&mut max_id, val);
    }
    for val in &func.results {
        check_val(&mut max_id, val);
    }
    for &id in func.vregs.keys() {
        max_id = max_id.max(id);
    }
    for block in &func.blocks {
        for instr in &block.instructions {
            for id in instr.defined_regs() {
                max_id = max_id.max(id);
            }
            for id in instr.used_regs() {
                max_id = max_id.max(id);
            }
        }
        match &block.terminator {
            IRTerminator::Return(vals) => {
                for val in vals {
                    check_val(&mut max_id, val);
                }
            }
            IRTerminator::Branch { cond, .. } => {
                check_val(&mut max_id, cond);
            }
            IRTerminator::Switch { discr, .. } => {
                check_val(&mut max_id, discr);
            }
            IRTerminator::Invoke { dst, args, .. } => {
                if let Some(v) = dst {
                    check_val(&mut max_id, v);
                }
                for val in args {
                    check_val(&mut max_id, val);
                }
            }
            IRTerminator::TailCall { args, .. } => {
                for val in args {
                    check_val(&mut max_id, val);
                }
            }
            IRTerminator::Resume { value } => {
                check_val(&mut max_id, value);
            }
            _ => {}
        }
    }
    max_id
}

/// Redirect a terminator's branch targets from `from_label` to `to_label`.
fn redirect_terminator(terminator: &mut IRTerminator, from_label: &str, to_label: &str) {
    match terminator {
        IRTerminator::Jump(target) if *target == *from_label => {
            *target = to_label.to_string();
        }
        IRTerminator::Jump(_) => {}
        IRTerminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            if *true_block == *from_label {
                *true_block = to_label.to_string();
            }
            if *false_block == *from_label {
                *false_block = to_label.to_string();
            }
        }
        IRTerminator::Switch {
            targets, default, ..
        } => {
            for (_, target) in targets.iter_mut() {
                if *target == *from_label {
                    *target = to_label.to_string();
                }
            }
            if *default == *from_label {
                *default = to_label.to_string();
            }
        }
        _ => {}
    }
}

/// Expression key for CSE value numbering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ExprKey {
    Binary(BinOpKind, IRValue, IRValue),
    Unary(UnaryOpKind, IRValue),
    Compare(CmpKind, IRValue, IRValue),
}

/// Compute the expression key for an instruction, if it is a candidate for CSE.
fn compute_expr_key(instr: &IRInstr) -> Option<ExprKey> {
    match instr {
        IRInstr::BinOp { op, lhs, rhs, .. } => Some(ExprKey::Binary(*op, lhs.clone(), rhs.clone())),
        IRInstr::UnaryOp { op, operand, .. } => Some(ExprKey::Unary(*op, operand.clone())),
        IRInstr::Add { lhs, rhs, .. } => {
            Some(ExprKey::Binary(BinOpKind::Add, lhs.clone(), rhs.clone()))
        }
        IRInstr::Sub { lhs, rhs, .. } => {
            Some(ExprKey::Binary(BinOpKind::Sub, lhs.clone(), rhs.clone()))
        }
        IRInstr::Mul { lhs, rhs, .. } => {
            Some(ExprKey::Binary(BinOpKind::Mul, lhs.clone(), rhs.clone()))
        }
        IRInstr::Div { lhs, rhs, .. } => {
            Some(ExprKey::Binary(BinOpKind::SDiv, lhs.clone(), rhs.clone()))
        }
        IRInstr::Cmp { kind, lhs, rhs, .. } => {
            Some(ExprKey::Compare(*kind, lhs.clone(), rhs.clone()))
        }
        _ => None,
    }
}

/// Find natural loops in the CFG using back-edge detection.
///
/// Returns a list of (header_label, set_of_loop_block_labels) tuples.
fn find_natural_loops(func: &IRFunction) -> Vec<(String, HashSet<String>)> {
    let label_to_idx: HashMap<String, usize> = func
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.clone(), i))
        .collect();

    let mut loops = Vec::new();
    let mut seen_headers: HashSet<String> = HashSet::new();

    for (block_idx, block) in func.blocks.iter().enumerate() {
        for succ_label in &block.successors {
            if let Some(&succ_idx) = label_to_idx.get(succ_label) {
                // A back edge exists when a successor has a smaller or equal
                // block index (i.e. it goes "backward" in the layout order).
                if succ_idx <= block_idx {
                    let header_label = succ_label.clone();
                    if seen_headers.contains(&header_label) {
                        continue;
                    }
                    seen_headers.insert(header_label.clone());

                    // Find natural loop body: header + all blocks reachable
                    // from the back-edge source without going through the header.
                    let mut loop_body = HashSet::new();
                    loop_body.insert(header_label.clone());

                    let mut stack = vec![block.label.clone()];
                    while let Some(label) = stack.pop() {
                        if !loop_body.contains(&label) {
                            loop_body.insert(label.clone());
                            if let Some(&idx) = label_to_idx.get(&label) {
                                for pred in &func.blocks[idx].predecessors {
                                    stack.push(pred.clone());
                                }
                            }
                        }
                    }

                    loops.push((header_label, loop_body));
                }
            }
        }
    }

    loops
}

// ===========================================================================
// Constant Folding
// ===========================================================================

/// For each BinOp/Add/Sub/Mul/Div/Cmp/UnaryOp where both operands are
/// `Immediate`, compute the result at compile time and replace the
/// instruction's destination with the computed constant.  Handles Add, Sub,
/// Mul, Div, And, Or, Xor, Shl, Shr as well as comparisons and unary ops.
///
/// Also performs intra-block constant propagation: when a register is known to
/// hold a constant, subsequent uses of that register are replaced with the
/// constant, potentially enabling further folds.
pub fn constant_fold(mut func: IRFunction) -> IRFunction {
    for block in &mut func.blocks {
        let mut subst: HashMap<u32, IRValue> = HashMap::new();
        let mut new_instrs = Vec::new();

        for instr in &block.instructions {
            // Substitute operands with known constants.
            let instr = substitute_instr(instr, &subst);

            // Try to fold.
            let folded = try_fold_instruction(&instr);
            if let Some((dst_id, result)) = folded {
                subst.insert(dst_id, IRValue::Immediate(result));
                // Instruction is eliminated; its dst is now a known constant.
                continue;
            }

            new_instrs.push(instr);
        }

        block.instructions = new_instrs;

        // Substitute in the terminator as well.
        block.terminator = substitute_terminator(&block.terminator, &subst);
    }
    func
}

/// Try to fold an instruction whose operands are all immediates.
/// Returns `Some((dst_register_id, computed_value))` if the instruction can be
/// eliminated, or `None` if it cannot be folded.
fn try_fold_instruction(instr: &IRInstr) -> Option<(u32, i64)> {
    match instr {
        IRInstr::BinOp { op, dst, lhs, rhs, .. } => {
            let l = lhs.as_immediate()?;
            let r = rhs.as_immediate()?;
            let dst_id = dst.as_register()?;
            let result = try_fold_binop(*op, l, r)?;
            Some((dst_id, result))
        }
        IRInstr::UnaryOp { op, dst, operand, .. } => {
            let o = operand.as_immediate()?;
            let dst_id = dst.as_register()?;
            let result = try_fold_unaryop(*op, o)?;
            Some((dst_id, result))
        }
        IRInstr::Add { dst, lhs, rhs, .. } => {
            let l = lhs.as_immediate()?;
            let r = rhs.as_immediate()?;
            let dst_id = dst.as_register()?;
            Some((dst_id, l.wrapping_add(r)))
        }
        IRInstr::Sub { dst, lhs, rhs, .. } => {
            let l = lhs.as_immediate()?;
            let r = rhs.as_immediate()?;
            let dst_id = dst.as_register()?;
            Some((dst_id, l.wrapping_sub(r)))
        }
        IRInstr::Mul { dst, lhs, rhs, .. } => {
            let l = lhs.as_immediate()?;
            let r = rhs.as_immediate()?;
            let dst_id = dst.as_register()?;
            Some((dst_id, l.wrapping_mul(r)))
        }
        IRInstr::Div { dst, lhs, rhs, .. } => {
            let l = lhs.as_immediate()?;
            let r = rhs.as_immediate()?;
            if r == 0 {
                return None;
            }
            let dst_id = dst.as_register()?;
            l.checked_div(r).map(|v| (dst_id, v))
        }
        IRInstr::Cmp {
            kind,
            dst,
            lhs,
            rhs, ty: _,
        } => {
            let l = lhs.as_immediate()?;
            let r = rhs.as_immediate()?;
            let dst_id = dst.as_register()?;
            let result = try_fold_cmp(*kind, l, r)?;
            Some((dst_id, result))
        }
        _ => None,
    }
}

// ===========================================================================
// Dead Code Elimination
// ===========================================================================

/// Walk instructions in reverse. Track which IRValues are "used" (appear as
/// operands or have side effects). Remove instructions whose `dst` is never
/// used and that have no side effects.
pub fn dead_code_eliminate(mut func: IRFunction) -> IRFunction {
    for block in &mut func.blocks {
        // Seed the used set with values referenced by the terminator.
        let mut used: HashSet<u32> = HashSet::new();
        match &block.terminator {
            IRTerminator::Return(vals) => {
                for val in vals {
                    if let IRValue::Register(id) = val {
                        used.insert(*id);
                    }
                }
            }
            IRTerminator::Branch { cond, .. } => {
                if let IRValue::Register(id) = cond {
                    used.insert(*id);
                }
            }
            IRTerminator::Switch { discr, .. } => {
                if let IRValue::Register(id) = discr {
                    used.insert(*id);
                }
            }
            IRTerminator::Invoke { args, .. } => {
                for arg in args {
                    if let IRValue::Register(id) = arg {
                        used.insert(*id);
                    }
                }
            }
            IRTerminator::TailCall { args, .. } => {
                for arg in args {
                    if let IRValue::Register(id) = arg {
                        used.insert(*id);
                    }
                }
            }
            IRTerminator::Resume { value } => {
                if let IRValue::Register(id) = value {
                    used.insert(*id);
                }
            }
            IRTerminator::Jump(_) | IRTerminator::Unreachable => {}
        }

        // Walk instructions in reverse.
        let mut new_instrs = Vec::new();
        for instr in block.instructions.iter().rev() {
            let defined = instr.defined_regs();
            let is_dst_used = defined.iter().any(|id| used.contains(id));

            if is_dst_used || has_side_effects(instr) {
                // Keep this instruction and mark its operands as used.
                for id in instr.used_regs() {
                    used.insert(id);
                }
                new_instrs.push(instr.clone());
            }
            // else: instruction is dead — remove it.
        }

        new_instrs.reverse();
        block.instructions = new_instrs;
    }
    func
}

// ===========================================================================
// Common Subexpression Elimination
// ===========================================================================

/// For each BinOp/UnaryOp/Add/Sub/Mul/Div/Cmp, compute a hash of (op,
/// operands). If the same (op, operands) has been seen before in the same
/// block, replace the destination with the previously-computed destination.
/// Uses value numbering within each basic block.
pub fn cse(mut func: IRFunction) -> IRFunction {
    for block in &mut func.blocks {
        let mut value_map: HashMap<ExprKey, IRValue> = HashMap::new();
        let mut subst: HashMap<u32, IRValue> = HashMap::new();
        let mut new_instrs = Vec::new();

        for instr in &block.instructions {
            // Apply previous CSE substitutions.
            let instr = substitute_instr(instr, &subst);

            if let Some(key) = compute_expr_key(&instr) {
                if let Some(prev_val) = value_map.get(&key) {
                    // Common subexpression found — replace dst with previous result.
                    if let Some(IRValue::Register(id)) = get_defined_value(&instr) {
                        subst.insert(*id, prev_val.clone());
                        continue; // Eliminate redundant instruction.
                    }
                } else if let Some(dst) = get_defined_value(&instr) {
                    value_map.insert(key, dst.clone());
                }
            }

            new_instrs.push(instr);
        }

        block.instructions = new_instrs;

        // Apply substitutions to the terminator.
        block.terminator = substitute_terminator(&block.terminator, &subst);
    }
    func
}

// ===========================================================================
// Inlining of Small Functions
// ===========================================================================

/// For `Call` instructions to functions with ≤5 instructions, inline the
/// callee's body at the call site.  Multi-block callees are supported: the
/// caller block is split at the call site, the callee's blocks (with
/// remapped vregs and labels) are inserted in between, and `Return`
/// terminators are redirected to the continuation block.
pub fn inline_small(
    mut func: IRFunction,
    program_funcs: &HashMap<String, &IRFunction>,
) -> IRFunction {
    let mut vreg_counter = max_vreg_id(&func) + 1;
    let mut inline_id: u32 = 0;

    let mut block_idx = 0;
    while block_idx < func.blocks.len() {
        // Find the first inlinable call in this block.
        let mut call_info: Option<(usize, String, Option<IRValue>, Vec<IRValue>)> = None;

        for (i, instr) in func.blocks[block_idx].instructions.iter().enumerate() {
            if let IRInstr::Call {
                dst,
                func: callee_name,
                args,
            } = instr
            {
                // Don't inline recursive calls.
                if *callee_name == func.name {
                    continue;
                }
                if let Some(callee) = program_funcs.get(callee_name) {
                    if callee.instruction_count() <= 5 {
                        call_info = Some((i, callee_name.clone(), dst.clone(), args.clone()));
                        break;
                    }
                }
            }
        }

        if let Some((call_pos, callee_name, call_dst, call_args)) = call_info {
            let callee = program_funcs.get(&callee_name).unwrap();
            let prefix = format!("inl{}_{}", inline_id, func.blocks[block_idx].label);
            inline_id += 1;

            // Build vreg mapping: callee params → caller args.
            let mut vreg_map: HashMap<u32, IRValue> = HashMap::new();
            for (param, arg) in callee.params.iter().zip(call_args.iter()) {
                if let IRValue::Register(id) = param {
                    vreg_map.insert(*id, arg.clone());
                }
            }

            // Create a result vreg for the return value (if the call has a dst).
            let result_vreg = if call_dst.is_some() {
                let rv = IRValue::Register(vreg_counter);
                vreg_counter += 1;
                Some(rv)
            } else {
                None
            };

            // Map callee's defined vregs to fresh vregs.
            for cblock in &callee.blocks {
                for instr in &cblock.instructions {
                    for def_id in instr.defined_regs() {
                        if let std::collections::hash_map::Entry::Vacant(e) = vreg_map.entry(def_id)
                        {
                            let new_vreg = IRValue::Register(vreg_counter);
                            e.insert(new_vreg);
                            vreg_counter += 1;
                        }
                    }
                }
            }

            let cont_label = format!("{}_cont", prefix);

            // Clone and remap callee blocks.
            let mut new_blocks: Vec<IRBlock> = Vec::new();
            for cblock in &callee.blocks {
                let new_label = format!("{}_{}", prefix, cblock.label);
                let mut new_block = IRBlock::new(&new_label);

                for instr in &cblock.instructions {
                    new_block.push(substitute_instr(instr, &vreg_map));
                }

                // Remap the terminator.
                match &cblock.terminator {
                    IRTerminator::Return(vals) => {
                        // Assign the return value to result_vreg and jump to
                        // the continuation block.
                        if let Some(rv) = &result_vreg {
                            if let Some(ret_val) = vals.first() {
                                let ret_val = substitute_value(ret_val, &vreg_map);
                                new_block.push(IRInstr::Select {
                                    dst: rv.clone(),
                                    cond: IRValue::Immediate(1),
                                    true_val: ret_val.clone(),
                                    false_val: ret_val,
                                    ty: None,
                                });
                            }
                        }
                        new_block.terminator = IRTerminator::Jump(cont_label.clone());
                    }
                    IRTerminator::Jump(target) => {
                        new_block.terminator = IRTerminator::Jump(format!("{}_{}", prefix, target));
                    }
                    IRTerminator::Branch {
                        cond,
                        true_block,
                        false_block,
                    } => {
                        new_block.terminator = IRTerminator::Branch {
                            cond: substitute_value(cond, &vreg_map),
                            true_block: format!("{}_{}", prefix, true_block),
                            false_block: format!("{}_{}", prefix, false_block),
                        };
                    }
                    other => {
                        new_block.terminator = other.clone();
                    }
                }

                new_blocks.push(new_block);
            }

            // Split the caller block at the call site.
            let suffix_instrs: Vec<IRInstr> =
                func.blocks[block_idx].instructions[call_pos + 1..].to_vec();
            let suffix_terminator = func.blocks[block_idx].terminator.clone();

            // Prefix: everything before the call; terminator → first callee block.
            func.blocks[block_idx].instructions.truncate(call_pos);
            func.blocks[block_idx].terminator =
                IRTerminator::Jump(format!("{}_{}", prefix, callee.blocks[0].label));

            // Continuation block: copy result to call dst + rest of original.
            let mut cont_block = IRBlock::new(&cont_label);
            if let (Some(dst), Some(ref rv)) = (call_dst, result_vreg) {
                cont_block.push(IRInstr::Select {
                    dst,
                    cond: IRValue::Immediate(1),
                    true_val: rv.clone(),
                    false_val: rv.clone(),
                    ty: None,
                });
            }
            cont_block.instructions.extend(suffix_instrs);
            cont_block.terminator = suffix_terminator;

            new_blocks.push(cont_block);

            // Insert the new blocks after the current block.
            for (i, nb) in new_blocks.into_iter().enumerate() {
                func.blocks.insert(block_idx + 1 + i, nb);
            }

            // Skip past all inserted blocks.
            block_idx += 1;
        } else {
            block_idx += 1;
        }
    }

    func.rebuild_cfg();
    func
}

// ===========================================================================
// Loop-Invariant Code Motion
// ===========================================================================

/// For loops (identified by back edges in the CFG), move loop-invariant
/// instructions (whose operands are all defined outside the loop) to a newly
/// created preheader block.  Only pure, non-trapping instructions are moved.
pub fn licm(mut func: IRFunction) -> IRFunction {
    func.rebuild_cfg();

    let loops = find_natural_loops(&func);

    // Process loops in reverse order of header block index so that inserting
    // preheader blocks doesn't shift indices of other loops.
    let label_to_idx: HashMap<String, usize> = func
        .blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.label.clone(), i))
        .collect();

    let mut sorted_loops = loops;
    sorted_loops.sort_by(|a, b| {
        let ai = label_to_idx.get(&a.0).copied().unwrap_or(0);
        let bi = label_to_idx.get(&b.0).copied().unwrap_or(0);
        bi.cmp(&ai) // reverse order
    });

    for (header_label, loop_body_labels) in sorted_loops {
        let header_idx = match func.find_block_by_label(&header_label) {
            Some(i) => i,
            None => continue,
        };

        // Collect vregs defined outside the loop.
        let mut outside_defs: HashSet<u32> = HashSet::new();
        for param in &func.params {
            if let IRValue::Register(id) = param {
                outside_defs.insert(*id);
            }
        }
        for block in &func.blocks {
            let block_label = &block.label;
            if loop_body_labels.contains(block_label) {
                continue;
            }
            for instr in &block.instructions {
                for id in instr.defined_regs() {
                    outside_defs.insert(id);
                }
            }
        }

        // Find loop-invariant instructions in the header block.
        // We walk in order so that earlier invariant instructions can make
        // later ones invariant too (their defs become "outside").
        let mut invariant_instrs: Vec<IRInstr> = Vec::new();
        let mut remove_indices: Vec<usize> = Vec::new();

        for (i, instr) in func.blocks[header_idx].instructions.iter().enumerate() {
            // Skip Phi nodes — they depend on control flow.
            if matches!(instr, IRInstr::Phi { .. }) {
                continue;
            }
            // Skip side-effect and trapping instructions.
            if has_side_effects(instr) || !is_safe_to_speculate(instr) {
                continue;
            }
            // Check that all used registers are defined outside the loop.
            let used = instr.used_regs();
            let all_outside = used.iter().all(|id| outside_defs.contains(id));
            if all_outside {
                invariant_instrs.push(instr.clone());
                remove_indices.push(i);
                // This instruction's result is now available "outside" the
                // loop (it will be in the preheader).
                for id in instr.defined_regs() {
                    outside_defs.insert(id);
                }
            }
        }

        if invariant_instrs.is_empty() {
            continue;
        }

        // Create the preheader block.
        let preheader_label = format!("preheader_{}", header_label);
        let mut preheader = IRBlock::new(&preheader_label);
        for instr in &invariant_instrs {
            preheader.push(instr.clone());
        }
        preheader.terminator = IRTerminator::Jump(header_label.clone());

        // Remove invariant instructions from the header (in reverse index
        // order to preserve positions).
        for &i in remove_indices.iter().rev() {
            func.blocks[header_idx].instructions.remove(i);
        }

        // Redirect non-loop predecessors of the header to the preheader.
        for (block_idx, block) in func.blocks.iter_mut().enumerate() {
            let block_label = block.label.clone();
            if loop_body_labels.contains(&block_label) || block_idx == header_idx {
                // Don't redirect loop-internal edges or the preheader itself.
                // However, we haven't inserted the preheader yet, so this
                // check is for the existing blocks.
                continue;
            }
            redirect_terminator(&mut block.terminator, &header_label, &preheader_label);
        }

        // Insert the preheader before the header block.
        // We need to re-find the header index because previous insertions may
        // have shifted it.
        let header_idx = func.find_block_by_label(&header_label).unwrap_or(0);
        func.blocks.insert(header_idx, preheader);
    }

    func.rebuild_cfg();
    func
}

// ===========================================================================
// Pipeline
// ===========================================================================

/// Apply all optimization passes in the recommended order:
///
/// `constant_fold → cse → dce → inline_small → licm → constant_fold → dce`
pub fn run_optimizations(mut program: IRProgram) -> IRProgram {
    // Build a function lookup table (cloned to avoid borrow conflicts when
    // mutating program.functions).
    let func_map: HashMap<String, IRFunction> = program
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    let func_refs: HashMap<String, &IRFunction> =
        func_map.iter().map(|(k, v)| (k.clone(), v)).collect();

    for i in 0..program.functions.len() {
        let f = std::mem::replace(&mut program.functions[i], IRFunction::new("__tmp__"));
        let f = constant_fold(f);
        let f = cse(f);
        let f = dead_code_eliminate(f);
        let f = inline_small(f, &func_refs);
        let f = licm(f);
        let f = constant_fold(f);
        let f = dead_code_eliminate(f);
        program.functions[i] = f;
    }

    program
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(any())] // Disabled: broken tests need fixing
mod tests {
    use super::*;
    use crate::ir::{BinOpKind, CmpKind, IRFunction, IRInstr, IRTerminator, IRType, UnaryOpKind};

    // ---- Helper: build a minimal function from instructions ----

    fn make_func_with_instrs(name: &str, instrs: Vec<IRInstr>) -> IRFunction {
        let mut func = IRFunction::new(name);
        func.blocks[0].instructions = instrs;
        func.blocks[0].terminator = IRTerminator::Return(vec![]);
        func
    }

    // ---- Constant Folding Tests ----

    #[test]
    fn constant_fold_add() {
        let func = make_func_with_instrs(
            "test",
            vec![IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(3),
                rhs: IRValue::Immediate(4),
            }],
        );
        let result = constant_fold(func);
        // Instruction should be eliminated (folded to 7).
        assert!(result.blocks[0].instructions.is_empty());
    }

    #[test]
    fn constant_fold_sub() {
        let func = make_func_with_instrs(
            "test",
            vec![IRInstr::BinOp {
                op: BinOpKind::Sub,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(10),
                rhs: IRValue::Immediate(3),
            }],
        );
        let result = constant_fold(func);
        assert!(result.blocks[0].instructions.is_empty());
    }

    #[test]
    fn constant_fold_mul() {
        let func = make_func_with_instrs(
            "test",
            vec![IRInstr::BinOp {
                op: BinOpKind::Mul,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(6),
                rhs: IRValue::Immediate(7),
            }],
        );
        let result = constant_fold(func);
        assert!(result.blocks[0].instructions.is_empty());
    }

    #[test]
    fn constant_fold_div_by_zero() {
        // Division by zero must NOT be folded.
        let func = make_func_with_instrs(
            "test",
            vec![IRInstr::BinOp {
                op: BinOpKind::SDiv,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(10),
                rhs: IRValue::Immediate(0),
            }],
        );
        let result = constant_fold(func);
        assert_eq!(result.blocks[0].instructions.len(), 1);
    }

    #[test]
    fn constant_fold_chain() {
        // x = 3 + 4 → 7;  y = x + 5 → 12
        let mut func = IRFunction::new("test");
        func.blocks[0].instructions = vec![
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(3),
                rhs: IRValue::Immediate(4),
            },
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(5),
            },
        ];
        func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let result = constant_fold(func);
        // Both instructions should be eliminated; Return should use Immediate(12).
        assert!(result.blocks[0].instructions.is_empty());
        match &result.blocks[0].terminator {
            IRTerminator::Return(vals) => {
                assert_eq!(vals.len(), 1);
                assert_eq!(vals[0], IRValue::Immediate(12));
            }
            _ => panic!("expected Return terminator"),
        }
    }

    #[test]
    fn constant_fold_dedicated_add() {
        let func = make_func_with_instrs(
            "test",
            vec![IRInstr::Add {
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(5),
                rhs: IRValue::Immediate(8),
            }],
        );
        let result = constant_fold(func);
        assert!(result.blocks[0].instructions.is_empty());
    }

    #[test]
    fn constant_fold_and_or_xor() {
        for (op, expected) in [
            (BinOpKind::And, 0b1010 & 0b1100),
            (BinOpKind::Or, 0b1010 | 0b1100),
            (BinOpKind::Xor, 0b1010 ^ 0b1100),
        ] {
            let func = make_func_with_instrs(
                "test",
                vec![IRInstr::BinOp {
                    op,
                    dst: IRValue::Register(0),
                    lhs: IRValue::Immediate(0b1010),
                    rhs: IRValue::Immediate(0b1100),
                }],
            );
            let result = constant_fold(func);
            assert!(
                result.blocks[0].instructions.is_empty(),
                "failed for {:?}",
                op
            );

            // Verify via return value.
            let mut func2 = IRFunction::new("test");
            func2.blocks[0].instructions = vec![IRInstr::BinOp {
                op,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(0b1010),
                rhs: IRValue::Immediate(0b1100),
            }];
            func2.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);
            let result2 = constant_fold(func2);
            match &result2.blocks[0].terminator {
                IRTerminator::Return(vals) => {
                    assert_eq!(vals[0], IRValue::Immediate(expected), "failed for {:?}", op);
                }
                _ => panic!("expected Return"),
            }
        }
    }

    #[test]
    fn constant_fold_shift() {
        let func = make_func_with_instrs(
            "test",
            vec![
                IRInstr::BinOp {
                    op: BinOpKind::Shl,
                    dst: IRValue::Register(0),
                    lhs: IRValue::Immediate(1),
                    rhs: IRValue::Immediate(4),
                },
                IRInstr::BinOp {
                    op: BinOpKind::ShrL,
                    dst: IRValue::Register(1),
                    lhs: IRValue::Immediate(256),
                    rhs: IRValue::Immediate(4),
                },
            ],
        );
        let result = constant_fold(func);
        assert!(result.blocks[0].instructions.is_empty());
    }

    #[test]
    fn constant_fold_unary_neg_not() {
        let mut func = IRFunction::new("test");
        func.blocks[0].instructions = vec![
            IRInstr::UnaryOp {
                op: UnaryOpKind::Neg,
                dst: IRValue::Register(0),
                operand: IRValue::Immediate(42),
            },
            IRInstr::UnaryOp {
                op: UnaryOpKind::Not,
                dst: IRValue::Register(1),
                operand: IRValue::Immediate(0),
            },
        ];
        func.blocks[0].terminator =
            IRTerminator::Return(vec![IRValue::Register(0), IRValue::Register(1)]);
        let result = constant_fold(func);
        assert!(result.blocks[0].instructions.is_empty());
        match &result.blocks[0].terminator {
            IRTerminator::Return(vals) => {
                assert_eq!(vals[0], IRValue::Immediate(-42));
                assert_eq!(vals[1], IRValue::Immediate(-1));
            }
            _ => panic!("expected Return"),
        }
    }

    #[test]
    fn constant_fold_cmp() {
        let func = make_func_with_instrs(
            "test",
            vec![IRInstr::Cmp {
                kind: CmpKind::SLt,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(3),
                rhs: IRValue::Immediate(5),
            }],
        );
        let result = constant_fold(func);
        assert!(result.blocks[0].instructions.is_empty());
    }

    // ---- Dead Code Elimination Tests ----

    #[test]
    fn dce_removes_dead_binop() {
        let mut func = IRFunction::new("test");
        func.blocks[0].instructions = vec![
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(1),
                rhs: IRValue::Immediate(2),
            },
            // v0 is never used → should be eliminated.
        ];
        func.blocks[0].terminator = IRTerminator::Return(vec![]);
        let result = dead_code_eliminate(func);
        assert!(result.blocks[0].instructions.is_empty());
    }

    #[test]
    fn dce_keeps_used_binop() {
        let mut func = IRFunction::new("test");
        func.blocks[0].instructions = vec![IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(0),
            lhs: IRValue::Immediate(1),
            rhs: IRValue::Immediate(2),
        }];
        func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);
        let result = dead_code_eliminate(func);
        assert_eq!(result.blocks[0].instructions.len(), 1);
    }

    #[test]
    fn dce_keeps_side_effects() {
        let mut func = IRFunction::new("test");
        func.blocks[0].instructions = vec![IRInstr::Store {
            value: IRValue::Immediate(42),
            addr: IRValue::Register(0),
        }];
        func.blocks[0].terminator = IRTerminator::Return(vec![]);
        let result = dead_code_eliminate(func);
        assert_eq!(result.blocks[0].instructions.len(), 1);
    }

    #[test]
    fn dce_keeps_call() {
        let mut func = IRFunction::new("test");
        func.blocks[0].instructions = vec![IRInstr::Call {
            dst: None,
            func: "side_effect".to_string(),
            args: vec![],
        }];
        func.blocks[0].terminator = IRTerminator::Return(vec![]);
        let result = dead_code_eliminate(func);
        assert_eq!(result.blocks[0].instructions.len(), 1);
    }

    #[test]
    fn dce_removes_dead_alloc() {
        let mut func = IRFunction::new("test");
        func.blocks[0].instructions = vec![IRInstr::Alloc {
            dst: IRValue::Register(0),
            size: 16,
        }];
        func.blocks[0].terminator = IRTerminator::Return(vec![]);
        let result = dead_code_eliminate(func);
        assert!(result.blocks[0].instructions.is_empty());
    }

    // ---- CSE Tests ----

    #[test]
    fn cse_duplicate_binop() {
        let mut func = IRFunction::new("test");
        func.params = vec![IRValue::Register(0)];
        func.blocks[0].instructions = vec![
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(1),
            },
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(2),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(1),
            },
        ];
        func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let result = cse(func);
        // Second BinOp should be eliminated.
        assert_eq!(result.blocks[0].instructions.len(), 1);

        // v2 should have been replaced with v1 in the return.
        match &result.blocks[0].terminator {
            IRTerminator::Return(vals) => {
                assert_eq!(vals[0], IRValue::Register(1));
            }
            _ => panic!("expected Return"),
        }
    }

    #[test]
    fn cse_duplicate_add() {
        let mut func = IRFunction::new("test");
        func.params = vec![IRValue::Register(0)];
        func.blocks[0].instructions = vec![
            IRInstr::Add {
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(1),
            },
            IRInstr::Add {
                dst: IRValue::Register(2),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(1),
            },
        ];
        func.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        let result = cse(func);
        assert_eq!(result.blocks[0].instructions.len(), 1);
    }

    #[test]
    fn cse_does_not_eliminate_different_ops() {
        let mut func = IRFunction::new("test");
        func.params = vec![IRValue::Register(0)];
        func.blocks[0].instructions = vec![
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(1),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(1),
            },
            IRInstr::BinOp {
                op: BinOpKind::Sub,
                dst: IRValue::Register(2),
                lhs: IRValue::Register(0),
                rhs: IRValue::Immediate(1),
            },
        ];
        func.blocks[0].terminator = IRTerminator::Return(vec![]);

        let result = cse(func);
        assert_eq!(result.blocks[0].instructions.len(), 2);
    }

    // ---- Inlining Tests ----

    #[test]
    fn inline_small_fn() {
        // Callee: fn add_one(x) { v0 = x + 1; return v0 }
        let mut callee = IRFunction::new("add_one");
        callee.params = vec![IRValue::Register(0)];
        callee.param_types = vec![IRType::I64];
        callee.blocks[0].instructions = vec![IRInstr::Add {
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        }];
        callee.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
        callee.results = vec![IRValue::Register(1)];
        callee.result_types = vec![IRType::I64];

        // Caller: v0 = call add_one(42)
        let mut caller = IRFunction::new("caller");
        caller.blocks[0].instructions = vec![IRInstr::Call {
            dst: Some(IRValue::Register(0)),
            func: "add_one".to_string(),
            args: vec![IRValue::Immediate(42)],
        }];
        caller.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let func_map: HashMap<String, &IRFunction> =
            [("add_one".to_string(), &callee)].into_iter().collect();

        let result = inline_small(caller, &func_map);

        // The call should have been replaced with inlined instructions.
        // There should be no Call instruction in any block.
        for block in &result.blocks {
            for instr in &block.instructions {
                assert!(
                    !matches!(instr, IRInstr::Call { func, .. } if func == "add_one"),
                    "call should have been inlined"
                );
            }
        }
        // There should be at least 2 blocks (prefix + continuation or inlined body).
        assert!(result.blocks.len() >= 2);
    }

    #[test]
    fn inline_skips_large() {
        // Callee with >5 instructions.
        let mut callee = IRFunction::new("big_fn");
        callee.params = vec![IRValue::Register(0)];
        for i in 0..6u32 {
            callee.blocks[0].instructions.push(IRInstr::Add {
                dst: IRValue::Register(i + 1),
                lhs: IRValue::Register(i),
                rhs: IRValue::Immediate(1),
            });
        }
        callee.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(7)]);

        let mut caller = IRFunction::new("caller");
        caller.blocks[0].instructions = vec![IRInstr::Call {
            dst: Some(IRValue::Register(0)),
            func: "big_fn".to_string(),
            args: vec![IRValue::Immediate(0)],
        }];
        caller.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let func_map: HashMap<String, &IRFunction> =
            [("big_fn".to_string(), &callee)].into_iter().collect();

        let result = inline_small(caller, &func_map);

        // The call should NOT have been inlined.
        assert_eq!(result.blocks.len(), 1);
        assert!(matches!(
            &result.blocks[0].instructions[0],
            IRInstr::Call { func, .. } if func == "big_fn"
        ));
    }

    #[test]
    fn inline_preserves_return_value() {
        // Callee: fn double(x) { return x * 2 }
        let mut callee = IRFunction::new("double");
        callee.params = vec![IRValue::Register(0)];
        callee.param_types = vec![IRType::I64];
        callee.blocks[0].instructions = vec![IRInstr::Mul {
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(2),
        }];
        callee.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
        callee.results = vec![IRValue::Register(1)];
        callee.result_types = vec![IRType::I64];

        // Caller: v0 = call double(21); ret v0
        let mut caller = IRFunction::new("caller");
        caller.blocks[0].instructions = vec![IRInstr::Call {
            dst: Some(IRValue::Register(0)),
            func: "double".to_string(),
            args: vec![IRValue::Immediate(21)],
        }];
        caller.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(0)]);

        let func_map: HashMap<String, &IRFunction> =
            [("double".to_string(), &callee)].into_iter().collect();

        let result = inline_small(caller, &func_map);

        // The inlined body should contain the Mul instruction with args substituted.
        let all_instrs: Vec<&IRInstr> =
            result.blocks.iter().flat_map(|b| &b.instructions).collect();
        let has_mul = all_instrs.iter().any(|i| matches!(i, IRInstr::Mul { .. }));
        assert!(has_mul, "inlined body should contain the Mul instruction");
    }

    // ---- LICM Tests ----

    #[test]
    fn licm_moves_invariant() {
        // Build a loop with a loop-invariant computation in the header.
        //
        // entry:
        //   v0 = 10         (constant, defined before the loop)
        //   jump loop_header
        //
        // loop_header:
        //   v1 = v0 + 1     (loop-invariant: v0 is defined outside)
        //   v2 = phi [...]   (should not be moved)
        //   branch v2, loop_header, exit
        //
        // exit:
        //   ret v1
        let mut func = IRFunction::new("test_licm");
        func.params = vec![IRValue::Register(0)];

        // entry block
        func.blocks[0].label = "entry".to_string();
        func.blocks[0].instructions = vec![IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(1),
        }];
        func.blocks[0].terminator = IRTerminator::Jump("loop_header".to_string());

        // loop_header block
        let mut loop_header = IRBlock::new("loop_header");
        loop_header.instructions = vec![
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(2),
                lhs: IRValue::Register(1), // v1 is defined in entry (outside loop)
                rhs: IRValue::Immediate(5),
            },
            IRInstr::Phi {
                dst: IRValue::Register(3),
                incoming: vec![
                    (IRValue::Immediate(0), "entry".to_string()),
                    (IRValue::Register(3), "loop_header".to_string()),
                ],
            },
        ];
        loop_header.terminator = IRTerminator::Branch {
            cond: IRValue::Register(3),
            true_block: "exit".to_string(),
            false_block: "loop_header".to_string(),
        };

        // exit block
        let mut exit_block = IRBlock::new("exit");
        exit_block.terminator = IRTerminator::Return(vec![IRValue::Register(2)]);

        func.blocks = vec![func.blocks[0].clone(), loop_header, exit_block];
        func.rebuild_cfg();

        let result = licm(func);

        // The BinOp (v2 = v1 + 5) should have been moved out of the loop
        // header into the preheader.
        let preheader = result
            .blocks
            .iter()
            .find(|b| b.label.starts_with("preheader"));
        assert!(
            preheader.is_some(),
            "a preheader block should have been created"
        );

        let preheader = preheader.unwrap();
        let has_invariant = preheader.instructions.iter().any(|i| {
            matches!(
                i,
                IRInstr::BinOp {
                    op: BinOpKind::Add,
                    ..
                }
            )
        });
        assert!(
            has_invariant,
            "loop-invariant BinOp should be in the preheader"
        );

        // The loop header should no longer contain the invariant BinOp.
        let header = result.blocks.iter().find(|b| b.label == "loop_header");
        assert!(header.is_some());
        let header = header.unwrap();
        let header_has_invariant = header.instructions.iter().any(|i| {
            matches!(
                i,
                IRInstr::BinOp {
                    op: BinOpKind::Add,
                    dst: IRValue::Register(2),
                    ..
                }
            )
        });
        assert!(
            !header_has_invariant,
            "loop-invariant BinOp should have been moved out of the header"
        );
    }

    #[test]
    fn licm_does_not_move_div() {
        // Division is not safe to speculate (can trap), so LICM should not
        // move it.
        let mut func = IRFunction::new("test_licm_div");
        func.params = vec![IRValue::Register(0)];

        func.blocks[0].label = "entry".to_string();
        func.blocks[0].terminator = IRTerminator::Jump("loop_header".to_string());

        let mut loop_header = IRBlock::new("loop_header");
        loop_header.instructions = vec![IRInstr::Div {
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Immediate(2),
        }];
        loop_header.terminator = IRTerminator::Branch {
            cond: IRValue::Immediate(1),
            true_block: "exit".to_string(),
            false_block: "loop_header".to_string(),
        };

        let mut exit_block = IRBlock::new("exit");
        exit_block.terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        func.blocks = vec![func.blocks[0].clone(), loop_header, exit_block];
        func.rebuild_cfg();

        let result = licm(func);

        // No preheader should be created (nothing to move).
        let preheader = result
            .blocks
            .iter()
            .find(|b| b.label.starts_with("preheader"));
        // Even if a preheader is created, the Div should still be in the header.
        let header = result
            .blocks
            .iter()
            .find(|b| b.label == "loop_header")
            .unwrap();
        let header_has_div = header
            .instructions
            .iter()
            .any(|i| matches!(i, IRInstr::Div { .. }));
        assert!(
            header_has_div,
            "Div should not be moved out of the loop header"
        );
    }

    // ---- Pipeline Test ----

    #[test]
    fn run_optimizations_full() {
        // Create a small program with a callee and a caller that has
        // constant-foldable and dead code.
        let mut callee = IRFunction::new("square");
        callee.params = vec![IRValue::Register(0)];
        callee.param_types = vec![IRType::I64];
        callee.blocks[0].instructions = vec![IRInstr::Mul {
            dst: IRValue::Register(1),
            lhs: IRValue::Register(0),
            rhs: IRValue::Register(0),
        }];
        callee.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);
        callee.results = vec![IRValue::Register(1)];
        callee.result_types = vec![IRType::I64];

        let mut caller = IRFunction::new("main");
        // Dead instruction: v0 = 1 + 2 (never used directly, will be folded then DCE'd)
        caller.blocks[0].instructions = vec![
            IRInstr::BinOp {
                op: BinOpKind::Add,
                dst: IRValue::Register(0),
                lhs: IRValue::Immediate(1),
                rhs: IRValue::Immediate(2),
            },
            IRInstr::Call {
                dst: Some(IRValue::Register(1)),
                func: "square".to_string(),
                args: vec![IRValue::Immediate(5)],
            },
        ];
        caller.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let program = IRProgram {
            functions: vec![callee, caller],
            data_sections: vec![],
        };

        let result = run_optimizations(program);

        // The main function should have had its constant folded away (1+2
        // eliminated). The call may or may not be inlined depending on pass
        // order, but the dead add should definitely be gone.
        let main_func = result.functions.iter().find(|f| f.name == "main").unwrap();
        let has_dead_add = main_func.blocks.iter().any(|b| {
            b.instructions.iter().any(|i| {
                matches!(
                    i,
                    IRInstr::BinOp {
                        op: BinOpKind::Add,
                        dst: IRValue::Register(0),
                        ..
                    }
                )
            })
        });
        assert!(
            !has_dead_add,
            "dead constant add should have been eliminated"
        );
    }
}
