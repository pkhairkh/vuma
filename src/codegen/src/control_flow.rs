//! # Control Flow Lowering
//!
//! This module handles complex control flow lowering for multi-target codegen. It
//! translates high-level control flow patterns — switch/match dispatch, tail call
//! optimization, and loop optimization — into IR-level representations that the
//! emitter can process.
//!
//! ## Components
//!
//! - **SwitchLowerer** — Lowers `IRTerminator::Switch` into jump tables,
//!   binary search trees, or if-else chains depending on target density.
//! - **TailCallLowerer** — Detects and lowers eligible tail calls into
//!   frame-discarding jumps.
//! - **LoopOptimizer** — Identifies natural loops, checks unroll eligibility,
//!   and performs loop unrolling.
//!
//! ## Wave 35 decision: DELETE exception & coroutine lowerers
//!
//! The previous `ExceptionLowerer` (lowers `IRTerminator::Invoke`) and
//! `CoroutineLowerer` (transforms coroutine functions into state-machine IR)
//! were removed in Wave 35 because the `.vuma` language has **no syntax** that
//! can feed them:
//!
//! - **Exceptions**: The lexer (`src/parser/src/lexer.rs`) has no `try`,
//!   `catch`, `raise`, `throw`, or equivalent token kinds. The parser
//!   (`src/parser/src/parser.rs`) and AST (`src/parser/src/ast.rs`) define no
//!   exception AST nodes. The lowering pass `to_scg.rs` never emits an
//!   `IRTerminator::Invoke`. A repo-wide sweep of every `.vuma` file shows the
//!   words "try"/"catch"/"raise"/"throw" only inside English-language comments,
//!   never as language constructs.
//! - **Coroutines**: The lexer has no `yield`, `resume`, or `coroutine` token
//!   kinds. The `async`/`await` keywords DO exist in the lexer, and
//!   `Expr::Async`/`Expr::Await` AST nodes exist, but they are lowered through
//!   the parallel-region path in `to_scg.rs` (line ~1626: "Async block →
//!   Parallel region") — NOT through `CoroutineLowerer`. `CoroutineLowerer`
//!   scanned for IR blocks whose label starts with `"yield_"`, a convention
//!   that no parser/AST producer ever emits. It was therefore dead code.
//!
//! The `IRTerminator::Invoke` and `IRTerminator::Resume` enum variants still
//! exist in `src/codegen/src/ir.rs` (outside this file's ownership) and are
//! handled defensively by `successor_indices` for IR-walking safety; they are
//! simply never produced. Removing the enum variants is out of scope for Wave 35.
//!
//! Adding speculative language syntax is risky and explicitly out of scope, so
//! the decision is to DELETE both lowerers rather than wire them.

use crate::backend::{AArch64TargetInfo, TargetInfo};
use crate::ir::{BinOpKind, CmpKind, IRBlock, IRFunction, IRInstr, IRTerminator, IRValue};
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Allocate a new virtual register ID and advance the counter.
fn next_vreg(counter: &mut u32) -> IRValue {
    let id = *counter;
    *counter += 1;
    IRValue::Register(id)
}

/// Allocate a new unique label and advance the counter.
fn next_label(counter: &mut u32, prefix: &str) -> String {
    let id = *counter;
    *counter += 1;
    format!("{}{}", prefix, id)
}

// ===========================================================================
// SwitchLowerer
// ===========================================================================

/// Strategy for lowering a switch/match terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchStrategy {
    /// Jump table: dense range of values, use target-specific table addressing
    /// (ADRP+ADD on ARM64, PC-relative on x86_64, TOC-relative on PPC64).
    JumpTable,
    /// Binary search: sorted comparisons, log2(n) branches.
    BinarySearch,
    /// If-else chain: linear comparisons, good for few targets.
    IfElseChain,
    /// Wasm br_table: use the native `br_table` instruction (Wasm targets only).
    BrTable,
}

/// Lowers `IRTerminator::Switch` into a sequence of IR blocks using the
/// best strategy for the given target distribution.
pub struct SwitchLowerer;

/// Minimum number of targets to consider a jump table.
const JUMP_TABLE_MIN_TARGETS: usize = 6;
/// Maximum ratio of (range / count) to still consider dense enough for a
/// jump table. E.g. a ratio of 2.0 means at most half the table entries
/// are holes.
const DENSITY_THRESHOLD: f64 = 2.5;
/// Maximum number of targets where an if-else chain is preferred.
const IFELSE_MAX_TARGETS: usize = 4;

impl SwitchLowerer {
    /// Analyze switch targets and choose the best lowering strategy.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::choose_strategy_for_target`] with `AArch64TargetInfo`.
    pub fn choose_strategy(targets: &[(i64, String)], default: &str) -> SwitchStrategy {
        Self::choose_strategy_for_target(targets, default, &AArch64TargetInfo)
    }

    /// Analyze switch targets and choose the best lowering strategy for
    /// the given target.
    ///
    /// The decision is based on:
    /// - **Wasm targets**: Use `br_table` (the native Wasm switch instruction).
    /// - **Few targets (≤ 4)**: If-else chain is simplest and fastest.
    /// - **Dense range**: Jump table gives O(1) dispatch.
    /// - **Sparse / many targets**: Binary search gives O(log n) dispatch.
    pub fn choose_strategy_for_target(
        targets: &[(i64, String)],
        _default: &str,
        target: &dyn TargetInfo,
    ) -> SwitchStrategy {
        if targets.is_empty() {
            return SwitchStrategy::IfElseChain;
        }

        // Wasm targets use the native br_table instruction.
        if !target.has_registers() {
            log::debug!(
                "SwitchLowerer: {} targets → BrTable (Wasm stack machine)",
                targets.len()
            );
            return SwitchStrategy::BrTable;
        }

        let count = targets.len();

        // Few targets → linear chain is best (less overhead than table setup).
        if count <= IFELSE_MAX_TARGETS {
            log::debug!(
                "SwitchLowerer: {} targets → IfElseChain (few targets)",
                count
            );
            return SwitchStrategy::IfElseChain;
        }

        // Check density for jump table eligibility.
        if count >= JUMP_TABLE_MIN_TARGETS && Self::is_dense_range(targets) {
            log::debug!("SwitchLowerer: {} targets → JumpTable (dense range)", count);
            return SwitchStrategy::JumpTable;
        }

        // Fall back to binary search.
        log::debug!(
            "SwitchLowerer: {} targets → BinarySearch (sparse range)",
            count
        );
        SwitchStrategy::BinarySearch
    }

    /// Lower a switch to IR blocks using the chosen strategy.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::lower_switch_for_target`] with `AArch64TargetInfo`.
    ///
    /// Returns a list of new IR blocks to insert. The first block is the
    /// entry point that should replace the original switch terminator.
    pub fn lower_switch(
        discr: IRValue,
        targets: &[(i64, String)],
        default: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
    ) -> Vec<IRBlock> {
        Self::lower_switch_for_target(
            discr,
            targets,
            default,
            vreg_counter,
            label_counter,
            &AArch64TargetInfo,
        )
    }

    /// Lower a switch to IR blocks using the best strategy for the given target.
    ///
    /// Returns a list of new IR blocks to insert. The first block is the
    /// entry point that should replace the original switch terminator.
    pub fn lower_switch_for_target(
        discr: IRValue,
        targets: &[(i64, String)],
        default: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
        target: &dyn TargetInfo,
    ) -> Vec<IRBlock> {
        if targets.is_empty() {
            // Degenerate: just jump to default.
            let mut entry = IRBlock::new(next_label(label_counter, "switch_entry_"));
            entry.terminator = IRTerminator::Jump(default.to_string());
            return vec![entry];
        }

        let strategy = Self::choose_strategy_for_target(targets, default, target);
        match strategy {
            SwitchStrategy::JumpTable => {
                Self::lower_jump_table(discr, targets, default, vreg_counter, label_counter)
            }
            SwitchStrategy::BinarySearch => {
                Self::lower_binary_search(discr, targets, default, vreg_counter, label_counter)
            }
            SwitchStrategy::IfElseChain => {
                Self::lower_if_else_chain(discr, targets, default, vreg_counter, label_counter)
            }
            SwitchStrategy::BrTable => {
                // For Wasm, the br_table is a native instruction that the
                // Wasm backend handles directly. At the IR level we represent
                // it as an if-else chain (the Wasm emitter will emit br_table
                // when it sees this pattern).
                Self::lower_if_else_chain(discr, targets, default, vreg_counter, label_counter)
            }
        }
    }

    /// Lower using jump table strategy.
    ///
    /// Generates code to:
    /// 1. Subtract the minimum value from the discriminator.
    /// 2. Compare the adjusted discriminator against the range size.
    /// 3. If out of range, jump to default.
    /// 4. Otherwise, use the adjusted value as an index into a jump table
    ///    (represented as a series of comparisons simulating table lookup).
    fn lower_jump_table(
        discr: IRValue,
        targets: &[(i64, String)],
        default: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
    ) -> Vec<IRBlock> {
        let mut blocks = Vec::new();

        // Sort targets by value.
        let mut sorted = targets.to_vec();
        sorted.sort_by_key(|(v, _)| *v);

        let min_val = sorted[0].0;
        let max_val = sorted.last().unwrap().0;
        let range = (max_val - min_val) as u64;

        // Build a map from value to target label.
        let target_map: HashMap<i64, String> = sorted.iter().cloned().collect();

        // Entry block: compute adjusted index and bounds check.
        let entry_label = next_label(label_counter, "jt_entry_");
        let mut entry_block = IRBlock::new(&entry_label);

        let offset_val = IRValue::Immediate(min_val);
        let adj = next_vreg(vreg_counter);
        entry_block.push(IRInstr::BinOp {
            op: BinOpKind::Sub,
            dst: adj.clone(),
            lhs: discr.clone(),
            rhs: offset_val,
            ty: None,
        });

        // Bounds check: if adj > range, go to default.
        let range_val = IRValue::Immediate(range as i64);
        let oob = next_vreg(vreg_counter);
        entry_block.push(IRInstr::Cmp {
            kind: CmpKind::UGt,
            dst: oob.clone(),
            lhs: adj.clone(),
            rhs: range_val,
            ty: None,
        });

        let dispatch_label = next_label(label_counter, "jt_dispatch_");
        entry_block.terminator = IRTerminator::Branch {
            cond: oob,
            true_block: default.to_string(),
            false_block: dispatch_label.clone(),
        };
        blocks.push(entry_block);

        // Dispatch block: generate a chain of equality comparisons that
        // simulates jump table lookup. For each index in [0, range], check
        // if adj == index and branch to the corresponding target or default.
        //
        // In a real emitter this would become target-specific addressing:
        // - ARM64: ADRP+LDR from a table in .rodata
        // - x86_64: PC-relative lea + jmp indirect
        // - PPC64: TOC-relative add + mtctr + bctr
        // - MIPS64: Load address + jr (with NOP in branch delay slot)
        // At the IR level we represent it as sequential comparisons for
        // correctness.
        let mut dispatch_block = IRBlock::new(&dispatch_label);

        for idx in 0..=range {
            let idx_i64 = idx as i64;
            let value = min_val + idx_i64;
            let is_last = idx == range;

            let cmp_result = next_vreg(vreg_counter);
            dispatch_block.push(IRInstr::Cmp {
                kind: CmpKind::Eq,
                dst: cmp_result.clone(),
                lhs: adj.clone(),
                rhs: IRValue::Immediate(idx_i64),
            ty: None,
            });

            let target_label = target_map
                .get(&value)
                .cloned()
                .unwrap_or_else(|| default.to_string());

            if is_last {
                // Last index — if it matches, go to target; otherwise default.
                dispatch_block.terminator = IRTerminator::Branch {
                    cond: cmp_result,
                    true_block: target_label,
                    false_block: default.to_string(),
                };
            } else {
                // Not the last — if it matches, go to target; otherwise
                // continue to the next comparison in a new block.
                let next_cmp_label = next_label(label_counter, "jt_cmp_");
                dispatch_block.terminator = IRTerminator::Branch {
                    cond: cmp_result,
                    true_block: target_label,
                    false_block: next_cmp_label.clone(),
                };
                blocks.push(dispatch_block);
                dispatch_block = IRBlock::new(&next_cmp_label);
            }
        }

        blocks.push(dispatch_block);
        log::debug!(
            "SwitchLowerer: jump table with range {} ({} blocks)",
            range,
            blocks.len()
        );
        blocks
    }

    /// Lower using binary search strategy.
    ///
    /// Recursively partitions the sorted target list into halves, comparing
    /// the discriminator against the median value and branching accordingly.
    /// This yields O(log n) comparison depth.
    fn lower_binary_search(
        discr: IRValue,
        targets: &[(i64, String)],
        default: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
    ) -> Vec<IRBlock> {
        let mut sorted = targets.to_vec();
        sorted.sort_by_key(|(v, _)| *v);
        let mut blocks = Vec::new();

        let entry_label = next_label(label_counter, "bs_entry_");
        Self::lower_binary_search_recursive(
            discr,
            &sorted,
            default,
            vreg_counter,
            label_counter,
            &entry_label,
            &mut blocks,
        );

        log::debug!(
            "SwitchLowerer: binary search with {} targets ({} blocks)",
            sorted.len(),
            blocks.len()
        );
        blocks
    }

    /// Recursive helper for binary search lowering.
    fn lower_binary_search_recursive(
        discr: IRValue,
        targets: &[(i64, String)],
        default: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
        current_label: &str,
        blocks: &mut Vec<IRBlock>,
    ) {
        if targets.is_empty() {
            let mut block = IRBlock::new(current_label);
            block.terminator = IRTerminator::Jump(default.to_string());
            blocks.push(block);
            return;
        }

        if targets.len() == 1 {
            // Single target: compare and branch.
            let mut block = IRBlock::new(current_label);
            let cmp = next_vreg(vreg_counter);
            block.push(IRInstr::Cmp {
                kind: CmpKind::Eq,
                dst: cmp.clone(),
                lhs: discr.clone(),
                rhs: IRValue::Immediate(targets[0].0),
            ty: None,
            });
            block.terminator = IRTerminator::Branch {
                cond: cmp,
                true_block: targets[0].1.clone(),
                false_block: default.to_string(),
            };
            blocks.push(block);
            return;
        }

        // Find median.
        let mid = targets.len() / 2;
        let median_val = targets[mid].0;

        let mut block = IRBlock::new(current_label);
        let cmp = next_vreg(vreg_counter);
        block.push(IRInstr::Cmp {
            kind: CmpKind::SLt,
            dst: cmp.clone(),
            lhs: discr.clone(),
            rhs: IRValue::Immediate(median_val),
            ty: None,
        });

        // Left side: values < median_val → targets[0..mid]
        let left_label = next_label(label_counter, "bs_left_");
        // Right side: values >= median_val → targets[mid..]
        let right_label = next_label(label_counter, "bs_right_");

        block.terminator = IRTerminator::Branch {
            cond: cmp,
            true_block: left_label.clone(),
            false_block: right_label.clone(),
        };
        blocks.push(block);

        // Recurse into left half.
        Self::lower_binary_search_recursive(
            discr.clone(),
            &targets[..mid],
            default,
            vreg_counter,
            label_counter,
            &left_label,
            blocks,
        );

        // Recurse into right half.
        Self::lower_binary_search_recursive(
            discr,
            &targets[mid..],
            default,
            vreg_counter,
            label_counter,
            &right_label,
            blocks,
        );
    }

    /// Lower using if-else chain strategy.
    ///
    /// Generates a linear sequence of equality comparisons, one per target.
    /// Each comparison either branches to the corresponding target label
    /// or falls through to the next comparison. If no target matches,
    /// control falls through to the default block.
    fn lower_if_else_chain(
        discr: IRValue,
        targets: &[(i64, String)],
        default: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
    ) -> Vec<IRBlock> {
        let mut blocks = Vec::new();
        let entry_label = next_label(label_counter, "ie_entry_");
        let mut current_label = entry_label;

        for (i, (value, target)) in targets.iter().enumerate() {
            let is_last = i == targets.len() - 1;
            let mut block = IRBlock::new(&current_label);

            let cmp = next_vreg(vreg_counter);
            block.push(IRInstr::Cmp {
                kind: CmpKind::Eq,
                dst: cmp.clone(),
                lhs: discr.clone(),
                rhs: IRValue::Immediate(*value),
            ty: None,
            });

            if is_last {
                // Last comparison: match → target, no match → default.
                block.terminator = IRTerminator::Branch {
                    cond: cmp,
                    true_block: target.clone(),
                    false_block: default.to_string(),
                };
            } else {
                // Match → target, no match → next comparison block.
                let next_label = next_label(label_counter, "ie_cmp_");
                block.terminator = IRTerminator::Branch {
                    cond: cmp,
                    true_block: target.clone(),
                    false_block: next_label.clone(),
                };
                current_label = next_label;
            }

            blocks.push(block);
        }

        // If there were no targets, just jump to default.
        if targets.is_empty() {
            let mut block = IRBlock::new(&current_label);
            block.terminator = IRTerminator::Jump(default.to_string());
            blocks.push(block);
        }

        log::debug!(
            "SwitchLowerer: if-else chain with {} targets ({} blocks)",
            targets.len(),
            blocks.len()
        );
        blocks
    }

    /// Check if targets form a dense range suitable for a jump table.
    ///
    /// A range is "dense" when the ratio of the span (max - min) to the
    /// number of targets is below the [`DENSITY_THRESHOLD`]. This ensures
    /// the jump table doesn't have too many holes.
    fn is_dense_range(targets: &[(i64, String)]) -> bool {
        if targets.len() < 2 {
            return true;
        }

        let mut min_val = i64::MAX;
        let mut max_val = i64::MIN;
        for (v, _) in targets {
            min_val = min_val.min(*v);
            max_val = max_val.max(*v);
        }

        let span = (max_val - min_val) as f64;
        let count = targets.len() as f64;

        if count == 0.0 {
            return false;
        }

        let density = span / count;
        density <= DENSITY_THRESHOLD
    }
}

// ===========================================================================
// TailCallLowerer
// ===========================================================================

/// Analyzes whether a call can be tail-call optimized and lowers eligible
/// calls into frame-discarding jumps.
///
/// Tail call optimization avoids creating a new stack frame when the last
/// action of a function is to call another function and immediately return
/// its result. The specific mechanism varies by target:
/// - ARM64: move args into X0–X7, restore callee-saved, then BLR/BR
/// - x86_64: move args into RDI/RSI/RDX/RCX/R8/R9, then JMP
/// - RISC-V: move args into a0–a7, then JALR
/// - MIPS: move args into $a0–$a3 (or $a0–$a7 in N64), then JR (with NOP in delay slot)
pub struct TailCallLowerer;

// ARM64_MAX_REG_ARGS is no longer used directly; TailCallLowerer uses
// target.num_int_arg_regs() instead. Kept for documentation reference.
// const ARM64_MAX_REG_ARGS: usize = 8;

impl TailCallLowerer {
    /// Check if a call at the end of a function can be converted to a tail call.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::is_tail_call_eligible_for_target`] with `AArch64TargetInfo`.
    pub fn is_tail_call_eligible(
        call_dst: &Option<IRValue>,
        return_vals: &[IRValue],
        func: &IRFunction,
    ) -> bool {
        Self::is_tail_call_eligible_for_target(call_dst, return_vals, func, &AArch64TargetInfo)
    }

    /// Check if a call at the end of a function can be converted to a tail call,
    /// using target-specific calling convention information.
    ///
    /// A call is eligible for tail call optimization if:
    /// - The call's return value is immediately returned by the caller.
    /// - The caller has no stack-allocated values that need cleanup.
    /// - The calling convention is compatible (all params fit in registers).
    /// - The caller and callee return the same number of values.
    pub fn is_tail_call_eligible_for_target(
        call_dst: &Option<IRValue>,
        return_vals: &[IRValue],
        func: &IRFunction,
        target: &dyn TargetInfo,
    ) -> bool {
        let max_reg_args = target.num_int_arg_regs();

        // Rule 1: The call's destination must match the return values exactly.
        // For a single return value, the call dst must be the returned value.
        // For void calls (dst=None), the return must also be void.
        match (call_dst, return_vals) {
            (None, []) => {
                // Void tail call: call returns nothing, function returns nothing.
            }
            (Some(dst), [ret_val]) => {
                // The call result must be directly returned.
                if dst != ret_val {
                    log::debug!(
                        "TailCallLowerer: ineligible — call dst {:?} != return val {:?}",
                        dst,
                        ret_val
                    );
                    return false;
                }
            }
            _ => {
                // Multiple return values or mismatched count.
                log::debug!(
                    "TailCallLowerer: ineligible — return count mismatch (dst={:?}, rets={})",
                    call_dst,
                    return_vals.len()
                );
                return false;
            }
        }

        // Rule 2: No stack allocations that require cleanup.
        for block in &func.blocks {
            for instr in &block.instructions {
                if let IRInstr::Alloc { .. } = instr {
                    log::debug!("TailCallLowerer: ineligible — function has stack allocations");
                    return false;
                }
            }
        }

        // Rule 3: No stack arguments in the caller (all params must fit in
        // registers). The number of available argument registers depends on
        // the target's calling convention.
        if func.params.len() > max_reg_args {
            log::debug!(
                "TailCallLowerer: ineligible — caller has {} params (exceeds {} register args for {})",
                func.params.len(),
                max_reg_args,
                target.isa_name()
            );
            return false;
        }

        // Rule 4: The function must not have any invokes (exception handling
        // interacts poorly with tail calls).
        for block in &func.blocks {
            if let IRTerminator::Invoke { .. } = &block.terminator {
                log::debug!(
                    "TailCallLowerer: ineligible — function has invoke (exception handling)"
                );
                return false;
            }
        }

        // Rule 5: On targets without a link register (e.g. x86_64), tail
        // calls are still possible but require the return address to be
        // restored from the stack first. This is handled by the emitter.
        // We don't block eligibility here.
        let _ = target.has_link_register();

        log::debug!("TailCallLowerer: call is eligible for tail call optimization");
        true
    }

    /// Lower a tail call: move args into argument registers, restore
    /// callee-saved regs, then jump to the callee.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::lower_tail_call_for_target`] with `AArch64TargetInfo`.
    pub fn lower_tail_call(func: &str, args: &[IRValue], vreg_counter: &mut u32) -> Vec<IRInstr> {
        Self::lower_tail_call_for_target(func, args, vreg_counter, &AArch64TargetInfo)
    }

    /// Lower a tail call for the given target, using the target's number of
    /// argument registers.
    ///
    /// At the IR level we represent this as a `TailCall` terminator which
    /// the emitter will translate into frame deallocation + indirect branch.
    /// However, we also generate the argument-shuffling instructions here
    /// for cases where arguments are not already in the right registers.
    pub fn lower_tail_call_for_target(
        func: &str,
        args: &[IRValue],
        vreg_counter: &mut u32,
        target: &dyn TargetInfo,
    ) -> Vec<IRInstr> {
        let mut instrs = Vec::new();
        let max_reg_args = target.num_int_arg_regs();

        // If we have more args than register capacity, we can't tail-call
        // optimize in the standard way. The caller should have checked
        // eligibility first.
        if args.len() > max_reg_args {
            log::warn!(
                "TailCallLowerer: {} args exceed {} register capacity for {}; \
                 tail call may not be correct",
                args.len(),
                max_reg_args,
                target.isa_name()
            );
        }

        // Generate argument moves. At the IR level we don't know which
        // physical register each vreg is in, so we emit copy instructions
        // that the register allocator / emitter will resolve. For each
        // argument, we create a "move" using Select with a true condition
        // (effectively a copy) or we rely on the TailCall terminator
        // carrying the argument list.
        //
        // In practice, the TailCall terminator already carries the args,
        // so the emitter can handle the moves. We emit explicit copies only
        // for cases where we need to free up a source register that would
        // be clobbered by a prior move.

        // Detect overlapping argument moves: if any arg i is a register
        // that will be overwritten by the move for arg j (j < i), we need
        // to copy it to a temp first.
        let arg_regs: Vec<Option<u32>> = args.iter().map(|a| a.as_register()).collect();

        // Simple check: if any arg register index equals the target
        // position of a prior arg, we have a conflict.
        let mut needs_temp = vec![false; args.len()];
        for i in 1..args.len() {
            if let Some(src_reg) = arg_regs[i] {
                for arg_reg in arg_regs.iter().take(i) {
                    // If src_reg is the register for arg j and j's destination
                    // would overwrite it before we read arg i.
                    if *arg_reg == Some(src_reg) && src_reg != i as u32 {
                        needs_temp[i] = true;
                        break;
                    }
                }
            }
        }

        for (i, arg) in args.iter().enumerate() {
            if needs_temp[i] {
                // Copy to a temporary vreg to avoid clobbering.
                let temp = next_vreg(vreg_counter);
                instrs.push(IRInstr::Select {
                    dst: temp,
                    cond: IRValue::Immediate(1),
                    true_val: arg.clone(),
                    false_val: arg.clone(),
            ty: None,
                });
                // Note: we don't replace arg[i] here because the TailCall
                // terminator carries the original args. The emitter should
                // use the temp vreg instead. A more complete implementation
                // would track this mapping.
                let _ = temp; // Suppress unused warning; in a full impl this
                              // would be stored in a replacement map.
            }
        }

        log::debug!(
            "TailCallLowerer: lowered tail call to @{} with {} args (target={})",
            func,
            args.len(),
            target.isa_name()
        );

        instrs
    }

    /// Convenience: create a `TailCall` terminator for the given function
    /// and arguments.
    pub fn make_tail_call_terminator(func: &str, args: &[IRValue]) -> IRTerminator {
        IRTerminator::TailCall {
            func: func.to_string(),
            args: args.to_vec(),
        }
    }
}

// ===========================================================================
// LoopOptimizer
// ===========================================================================

/// Loop information extracted from IR blocks.
#[derive(Debug, Clone)]
pub struct LoopInfo {
    /// Label of the loop header block (the target of the back edge).
    pub header_block: String,
    /// Labels of blocks that form the loop body.
    pub body_blocks: Vec<String>,
    /// Labels of exit blocks (blocks outside the loop that are successors
    /// of blocks inside the loop).
    pub exit_blocks: Vec<String>,
    /// Label of the block that contains the back edge (the branch back to
    /// the header).
    pub back_edge_block: String,
    /// Estimated trip count, if statically known.
    pub trip_count: Option<u64>,
}

/// Identifies natural loops, checks unroll eligibility, and performs loop
/// unrolling on IR functions.
pub struct LoopOptimizer;

/// Maximum loop body size (in instructions) to consider for unrolling.
const MAX_UNROLL_BODY_SIZE: usize = 64;
/// Default maximum unroll factor.
const _DEFAULT_MAX_UNROLL_FACTOR: u32 = 8;

impl LoopOptimizer {
    /// Identify natural loops in an IR function by finding back edges.
    ///
    /// A natural loop is defined by a back edge: an edge from some block B
    /// to a dominator block H (the header). The loop body consists of all
    /// blocks reachable from H without going through H's dominator.
    pub fn identify_loops(func: &IRFunction) -> Vec<LoopInfo> {
        let label_to_idx: HashMap<String, usize> = func
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.clone(), i))
            .collect();

        // Compute dominators using the iterative algorithm.
        let doms = compute_dominators(func, &label_to_idx);

        // Find back edges: edges (B → H) where H dominates B.
        let mut loops = Vec::new();

        for (i, block) in func.blocks.iter().enumerate() {
            for succ in successor_indices(&block.terminator, &label_to_idx) {
                // Does the successor dominate this block?
                if dominates(&doms, succ, i) {
                    // Back edge found: block i → succ (header).
                    let header = succ;

                    // Collect all blocks in the natural loop.
                    let body = collect_loop_body(func, &label_to_idx, header, i);

                    // Find exit blocks: successors of body blocks that are
                    // not themselves in the body.
                    let _body_set: HashSet<String> = body
                        .iter()
                        .map(|idx| func.blocks[*idx].label.clone())
                        .collect();

                    let mut exit_blocks = HashSet::new();
                    for &bi in &body {
                        for exit_succ in
                            successor_indices(&func.blocks[bi].terminator, &label_to_idx)
                        {
                            if !body.contains(&exit_succ) {
                                exit_blocks.insert(func.blocks[exit_succ].label.clone());
                            }
                        }
                    }

                    // Estimate trip count.
                    let trip_count = estimate_trip_count(func, header, &body, &label_to_idx);

                    loops.push(LoopInfo {
                        header_block: func.blocks[header].label.clone(),
                        body_blocks: body
                            .iter()
                            .map(|&idx| func.blocks[idx].label.clone())
                            .collect(),
                        exit_blocks: exit_blocks.into_iter().collect(),
                        back_edge_block: func.blocks[i].label.clone(),
                        trip_count,
                    });
                }
            }
        }

        log::debug!(
            "LoopOptimizer: identified {} loops in @{}",
            loops.len(),
            func.name
        );

        loops
    }

    /// Check if a loop is eligible for unrolling.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::is_unrollable_for_target`] with `AArch64TargetInfo`.
    pub fn is_unrollable(loop_info: &LoopInfo, max_unroll_factor: u32) -> bool {
        Self::is_unrollable_for_target(loop_info, max_unroll_factor, &AArch64TargetInfo)
    }

    /// Check if a loop is eligible for unrolling, using the target's
    /// instruction cost model.
    ///
    /// A loop is eligible if:
    /// - It has a known trip count.
    /// - The trip count is divisible by the unroll factor (or we allow
    ///   remainder iterations).
    /// - The loop body is small enough that unrolling won't bloat code
    ///   excessively (using the target's instruction width for cost estimation).
    /// - The loop has exactly one exit block.
    pub fn is_unrollable_for_target(
        loop_info: &LoopInfo,
        max_unroll_factor: u32,
        target: &dyn TargetInfo,
    ) -> bool {
        // Must have a known trip count.
        let trip = match loop_info.trip_count {
            Some(t) => t,
            None => {
                log::debug!(
                    "LoopOptimizer: loop @{} not unrollable — unknown trip count",
                    loop_info.header_block
                );
                return false;
            }
        };

        // Trip count must be at least 2 (unrolling a single-iteration loop
        // is pointless).
        if trip < 2 {
            log::debug!(
                "LoopOptimizer: loop @{} not unrollable — trip count {} < 2",
                loop_info.header_block,
                trip
            );
            return false;
        }

        // Body size check: estimate instruction count based on target.
        // For fixed-width ISAs (ARM64, RISC-V, MIPS), each IR instruction
        // is roughly one machine instruction. For variable-width ISAs
        // (x86_64), IR instructions may expand to multiple bytes, but the
        // count of IR instructions is still a reasonable proxy.
        let instr_size = target.instruction_alignment();
        let body_size_estimate = loop_info.body_blocks.len() * instr_size * 4;
        if body_size_estimate > MAX_UNROLL_BODY_SIZE {
            log::debug!(
                "LoopOptimizer: loop @{} not unrollable — body too large (est. {} bytes, target={})",
                loop_info.header_block,
                body_size_estimate,
                target.isa_name()
            );
            return false;
        }

        // Unroll factor must be reasonable.
        if max_unroll_factor < 2 {
            return false;
        }

        // The effective unroll factor should not exceed the trip count.
        let effective_factor = max_unroll_factor.min(trip as u32);
        if effective_factor < 2 {
            return false;
        }

        log::debug!(
            "LoopOptimizer: loop @{} is unrollable (trip={}, factor={})",
            loop_info.header_block,
            trip,
            effective_factor
        );

        true
    }

    /// Unroll a loop by the given factor. Returns new blocks replacing
    /// the original loop body.
    ///
    /// Unrolling works by:
    /// 1. Cloning the loop body N times (where N = factor).
    /// 2. Rewiring the cloned bodies: the back edge of copy i jumps to
    ///    the header of copy i+1.
    /// 3. The last copy's back edge jumps back to the original header
    ///    (for the next iteration of the outer loop, if trip_count >
    ///    factor).
    /// 4. Adjusting the trip counter by dividing by the factor.
    pub fn unroll_loop(
        loop_info: &LoopInfo,
        factor: u32,
        func: &mut IRFunction,
    ) -> Result<(), String> {
        if factor < 2 {
            return Err("Unroll factor must be at least 2".to_string());
        }

        if loop_info.body_blocks.is_empty() {
            return Err("Cannot unroll a loop with an empty body".to_string());
        }

        // Find the indices of the loop body blocks.
        let label_to_idx: HashMap<String, usize> = func
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.clone(), i))
            .collect();

        let body_indices: Vec<usize> = loop_info
            .body_blocks
            .iter()
            .filter_map(|label| label_to_idx.get(label).copied())
            .collect();

        if body_indices.is_empty() {
            return Err("Loop body blocks not found in function".to_string());
        }

        // Clone the loop body `factor` times, creating uniquely labeled
        // copies.
        let original_labels: Vec<String> = body_indices
            .iter()
            .map(|&idx| func.blocks[idx].label.clone())
            .collect();

        let mut all_copies: Vec<Vec<IRBlock>> = Vec::new();
        let mut label_map: HashMap<String, String> = HashMap::new();

        for copy_num in 0..factor {
            let mut copy_blocks = Vec::new();
            let mut local_label_map: HashMap<String, String> = HashMap::new();

            // Generate new labels for this copy.
            for label in &original_labels {
                let new_label = format!("{}_unroll{}_{}", label, copy_num, factor);
                local_label_map.insert(label.clone(), new_label);
            }

            // The first copy maps original labels to their unrolled labels.
            // Subsequent copies map the previous copy's labels.
            if copy_num == 0 {
                // First copy keeps original labels (we'll rename them).
                for label in &original_labels {
                    label_map.insert(label.clone(), local_label_map[label].clone());
                }
            }

            // Clone each body block.
            for &idx in &body_indices {
                let original = &func.blocks[idx];
                let new_label = local_label_map[&original.label].clone();
                let mut new_block = original.clone();
                new_block.label = new_label;

                // Rewrite branch targets in the terminator to point to
                // the corresponding blocks in this copy.
                rewrite_terminator_targets(
                    &mut new_block.terminator,
                    &local_label_map,
                    &loop_info.header_block,
                    &loop_info.back_edge_block,
                    copy_num,
                    factor,
                );

                copy_blocks.push(new_block);
            }

            all_copies.push(copy_blocks);
        }

        // Insert the unrolled copies into the function after the original
        // loop body. We replace the original body blocks with the first
        // copy, then append the remaining copies.
        //
        // Find the insertion point: right after the last body block.
        let last_body_idx = *body_indices.last().unwrap();

        // Remove original body blocks and insert copies.
        // We need to be careful with indices — remove in reverse order.
        let first_body_idx = body_indices[0];
        let _body_count = body_indices.len();

        // Collect the blocks to keep before and after the loop body.
        let mut new_blocks = Vec::new();

        // Blocks before the loop body.
        for (i, block) in func.blocks.iter().enumerate() {
            if i < first_body_idx {
                new_blocks.push(block.clone());
            }
        }

        // Insert all copies.
        for copy in &all_copies {
            for block in copy {
                new_blocks.push(block.clone());
            }
        }

        // Blocks after the loop body.
        for (i, block) in func.blocks.iter().enumerate() {
            if i > last_body_idx {
                new_blocks.push(block.clone());
            }
        }

        // Also rewrite the predecessor of the loop header to jump to the
        // first copy's header instead.
        let first_copy_header = format!("{}_unroll0_{}", loop_info.header_block, factor);
        for block in &mut new_blocks {
            rewrite_terminator_to_target(
                &mut block.terminator,
                &loop_info.header_block,
                &first_copy_header,
            );
        }

        func.blocks = new_blocks;

        log::debug!(
            "LoopOptimizer: unrolled loop @{} by factor {} ({} copies)",
            loop_info.header_block,
            factor,
            all_copies.len()
        );

        Ok(())
    }

    /// Choose a good unroll factor for the given loop.
    ///
    /// Tries powers of 2 up to `max_factor`, picking the largest one that
    /// evenly divides the trip count (if known) and doesn't make the body
    /// too large.
    pub fn choose_unroll_factor(loop_info: &LoopInfo, max_factor: u32) -> u32 {
        let trip = match loop_info.trip_count {
            Some(t) => t,
            None => return 1,
        };

        let mut best = 1u32;
        let mut factor = 2u32;
        while factor <= max_factor && factor as u64 <= trip {
            if trip % factor as u64 == 0 {
                best = factor;
            }
            factor *= 2;
        }

        log::debug!(
            "LoopOptimizer: chose unroll factor {} for loop @{} (trip={})",
            best,
            loop_info.header_block,
            trip
        );

        best
    }
}

// ===========================================================================
// Internal Helpers
// ===========================================================================

/// Get the successor block indices for a terminator.
fn successor_indices(
    terminator: &IRTerminator,
    label_to_idx: &HashMap<String, usize>,
) -> Vec<usize> {
    match terminator {
        IRTerminator::Jump(target) => label_to_idx.get(target).copied().into_iter().collect(),
        IRTerminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            let mut succs = Vec::new();
            if let Some(&idx) = label_to_idx.get(true_block) {
                succs.push(idx);
            }
            if let Some(&idx) = label_to_idx.get(false_block) {
                succs.push(idx);
            }
            succs
        }
        IRTerminator::Switch {
            targets, default, ..
        } => {
            let mut succs = Vec::new();
            for (_, label) in targets {
                if let Some(&idx) = label_to_idx.get(label) {
                    succs.push(idx);
                }
            }
            if let Some(&idx) = label_to_idx.get(default) {
                succs.push(idx);
            }
            succs
        }
        IRTerminator::Invoke { normal, unwind, .. } => {
            let mut succs = Vec::new();
            if let Some(&idx) = label_to_idx.get(normal) {
                succs.push(idx);
            }
            if let Some(&idx) = label_to_idx.get(unwind) {
                succs.push(idx);
            }
            succs
        }
        IRTerminator::Return(_) | IRTerminator::Unreachable | IRTerminator::Resume { .. } => {
            Vec::new()
        }
        IRTerminator::TailCall { .. } => Vec::new(),
    }
}

/// Compute dominators for each block using the iterative algorithm.
///
/// Returns a vector where `doms[i]` is the set of block indices that
/// dominate block i (including block i itself).
fn compute_dominators(
    func: &IRFunction,
    label_to_idx: &HashMap<String, usize>,
) -> Vec<HashSet<usize>> {
    let n = func.blocks.len();
    if n == 0 {
        return Vec::new();
    }

    // Build predecessor map.
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, block) in func.blocks.iter().enumerate() {
        for succ in successor_indices(&block.terminator, label_to_idx) {
            predecessors[succ].push(i);
        }
    }

    // Initialize: entry block is dominated only by itself.
    let all_blocks: HashSet<usize> = (0..n).collect();
    let mut doms: Vec<HashSet<usize>> = vec![all_blocks; n];
    doms[0] = HashSet::from([0]);

    // Iterate until convergence.
    let mut changed = true;
    while changed {
        changed = false;
        for i in 1..n {
            if predecessors[i].is_empty() {
                // Unreachable block — dominated by itself only.
                continue;
            }

            // Intersect dominators of all predecessors.
            let mut new_dom: HashSet<usize> = if let Some(&first_pred) = predecessors[i].first() {
                doms[first_pred].clone()
            } else {
                HashSet::new()
            };

            for &pred in &predecessors[i][1..] {
                new_dom = new_dom.intersection(&doms[pred]).copied().collect();
            }

            // Every block dominates itself.
            new_dom.insert(i);

            if new_dom != doms[i] {
                doms[i] = new_dom;
                changed = true;
            }
        }
    }

    doms
}

/// Check if block `a` dominates block `b`.
fn dominates(doms: &[HashSet<usize>], a: usize, b: usize) -> bool {
    doms.get(b).is_some_and(|d| d.contains(&a))
}

/// Collect all blocks in the natural loop defined by a back edge
/// from `tail` to `header`.
fn collect_loop_body(
    func: &IRFunction,
    label_to_idx: &HashMap<String, usize>,
    header: usize,
    tail: usize,
) -> Vec<usize> {
    let mut loop_blocks = HashSet::new();
    loop_blocks.insert(header);
    loop_blocks.insert(tail);

    // Build predecessor map.
    let mut predecessors: Vec<Vec<usize>> = vec![Vec::new(); func.blocks.len()];
    for (i, block) in func.blocks.iter().enumerate() {
        for succ in successor_indices(&block.terminator, label_to_idx) {
            predecessors[succ].push(i);
        }
    }

    // Worklist algorithm: start from tail, walk predecessors until we
    // reach the header.
    let mut worklist = vec![tail];
    while let Some(node) = worklist.pop() {
        for &pred in &predecessors[node] {
            if !loop_blocks.contains(&pred) {
                loop_blocks.insert(pred);
                worklist.push(pred);
            }
        }
    }

    let mut result: Vec<usize> = loop_blocks.into_iter().collect();
    result.sort();
    result
}

/// Estimate the trip count of a loop by examining the header block for
/// comparison patterns against loop-invariant values.
fn estimate_trip_count(
    func: &IRFunction,
    header: usize,
    body: &[usize],
    label_to_idx: &HashMap<String, usize>,
) -> Option<u64> {
    let header_block = &func.blocks[header];
    let body_set: HashSet<usize> = body.iter().copied().collect();

    // Look for a Cmp instruction in the header that compares a phi node
    // against an immediate. This is a common pattern for loop counters.
    for instr in &header_block.instructions {
        if let IRInstr::Cmp {
            kind,
            dst: _,
            lhs,
            rhs: IRValue::Immediate(upper_bound),
            ty: _,
        } = instr
        {
            // Find the initial value of the phi source.
            if let IRValue::Register(_phi_reg) = lhs {
                // Search for a Phi instruction that defines this register.
                for block in &func.blocks {
                    for inner_instr in &block.instructions {
                        if let IRInstr::Phi { dst, incoming } = inner_instr {
                            if dst == lhs {
                                // Found the phi. Look for an initial value
                                // that comes from outside the loop.
                                for (val, src_block) in incoming {
                                    if let Some(&src_idx) = label_to_idx.get(src_block) {
                                        if !body_set.contains(&src_idx) {
                                            // Initial value from outside the loop.
                                            if let IRValue::Immediate(init) = val {
                                                let range = (*upper_bound - init) as u64;
                                                // Adjust based on comparison kind.
                                                let trip = match kind {
                                                    CmpKind::SLt | CmpKind::ULt | CmpKind::Ne => {
                                                        range
                                                    }
                                                    CmpKind::SLe | CmpKind::ULe => range + 1,
                                                    _ => range,
                                                };
                                                if trip > 0 && trip < 1_000_000 {
                                                    return Some(trip);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

/// Helper: rewrite a single label based on the unrolling rules.
fn rewrite_label(
    label: &mut String,
    header_label: &str,
    label_map: &HashMap<String, String>,
    copy_num: u32,
    factor: u32,
) {
    if label == header_label {
        if copy_num == factor - 1 {
            *label = header_label.to_string();
        } else {
            *label = format!("{}_unroll{}_{}", header_label, copy_num + 1, factor);
        }
    } else if let Some(new_label) = label_map.get(label) {
        *label = new_label.clone();
    }
}

/// Rewrite branch targets in a terminator for an unrolled copy.
///
/// - Internal targets (within the loop body) are mapped to the current copy.
/// - The back edge to the header is rewired: the last copy jumps back to
///   the original header; other copies jump to the next copy's header.
fn rewrite_terminator_targets(
    terminator: &mut IRTerminator,
    label_map: &HashMap<String, String>,
    header_label: &str,
    back_edge_label: &str,
    copy_num: u32,
    factor: u32,
) {
    match terminator {
        IRTerminator::Jump(target) => {
            rewrite_label(target, header_label, label_map, copy_num, factor);
        }
        IRTerminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            rewrite_label(true_block, header_label, label_map, copy_num, factor);
            rewrite_label(false_block, header_label, label_map, copy_num, factor);
        }
        IRTerminator::Switch {
            targets, default, ..
        } => {
            for (_, label) in targets.iter_mut() {
                rewrite_label(label, header_label, label_map, copy_num, factor);
            }
            rewrite_label(default, header_label, label_map, copy_num, factor);
        }
        _ => {
            // Return, Unreachable, Resume, TailCall, Invoke — no branch
            // targets to rewrite.
        }
    }

    let _ = back_edge_label; // Suppress unused warning.
}

/// Rewrite any branch target in a terminator that matches `old_target`
/// to `new_target`. Used to redirect the pre-header edge to the first
/// unrolled copy.
fn rewrite_terminator_to_target(terminator: &mut IRTerminator, old_target: &str, new_target: &str) {
    match terminator {
        IRTerminator::Jump(target) if target == old_target => {
            *target = new_target.to_string();
        }
        IRTerminator::Jump(_) => {}
        IRTerminator::Branch {
            true_block,
            false_block,
            ..
        } => {
            if *true_block == old_target {
                *true_block = new_target.to_string();
            }
            if *false_block == old_target {
                *false_block = new_target.to_string();
            }
        }
        IRTerminator::Switch {
            targets, default, ..
        } => {
            for (_, label) in targets.iter_mut() {
                if label == old_target {
                    *label = new_target.to_string();
                }
            }
            if *default == old_target {
                *default = new_target.to_string();
            }
        }
        _ => {}
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_strategy_few_targets() {
        let targets = vec![(1i64, "one".to_string()), (2, "two".to_string())];
        assert_eq!(
            SwitchLowerer::choose_strategy(&targets, "default"),
            SwitchStrategy::IfElseChain
        );
    }

    #[test]
    fn test_switch_strategy_dense() {
        let targets: Vec<(i64, String)> = (0..20).map(|i| (i, format!("case_{}", i))).collect();
        assert_eq!(
            SwitchLowerer::choose_strategy(&targets, "default"),
            SwitchStrategy::JumpTable
        );
    }

    #[test]
    fn test_switch_strategy_sparse() {
        let targets = vec![
            (0i64, "a".to_string()),
            (100, "b".to_string()),
            (200, "c".to_string()),
            (300, "d".to_string()),
            (400, "e".to_string()),
            (500, "f".to_string()),
            (600, "g".to_string()),
        ];
        assert_eq!(
            SwitchLowerer::choose_strategy(&targets, "default"),
            SwitchStrategy::BinarySearch
        );
    }

    #[test]
    fn test_is_dense_range() {
        // Dense: 0..10
        let dense: Vec<(i64, String)> = (0..10).map(|i| (i, format!("c{}", i))).collect();
        assert!(SwitchLowerer::is_dense_range(&dense));

        // Sparse: 0, 100, 200
        let sparse = vec![
            (0i64, "a".to_string()),
            (100, "b".to_string()),
            (200, "c".to_string()),
        ];
        assert!(!SwitchLowerer::is_dense_range(&sparse));
    }

    #[test]
    fn test_lower_if_else_chain() {
        let targets = vec![
            (1i64, "one".to_string()),
            (2, "two".to_string()),
            (3, "three".to_string()),
        ];
        let mut vreg = 100u32;
        let mut label = 100u32;

        let blocks = SwitchLowerer::lower_if_else_chain(
            IRValue::Register(0),
            &targets,
            "default",
            &mut vreg,
            &mut label,
        );

        // Should have 3 blocks (one per target).
        assert_eq!(blocks.len(), 3);

        // First block should compare against value 1.
        assert!(matches!(
            &blocks[0].instructions[0],
            IRInstr::Cmp {
                kind: CmpKind::Eq,
                rhs: IRValue::Immediate(1),
                ..
            }
        ));
    }

    #[test]
    fn test_lower_binary_search() {
        let targets = vec![
            (0i64, "a".to_string()),
            (10, "b".to_string()),
            (20, "c".to_string()),
            (30, "d".to_string()),
            (40, "e".to_string()),
            (50, "f".to_string()),
            (60, "g".to_string()),
        ];
        let mut vreg = 100u32;
        let mut label = 100u32;

        let blocks = SwitchLowerer::lower_binary_search(
            IRValue::Register(0),
            &targets,
            "default",
            &mut vreg,
            &mut label,
        );

        // Should produce a non-trivial number of blocks.
        assert!(blocks.len() > 3);

        // First block should compare against the median value.
        let first_instr = &blocks[0].instructions[0];
        assert!(matches!(
            first_instr,
            IRInstr::Cmp {
                kind: CmpKind::SLt,
                ..
            }
        ));
    }

    #[test]
    fn test_tail_call_eligibility_simple() {
        let mut func = IRFunction::new("caller");
        func.params.push(IRValue::Register(0));
        func.results.push(IRValue::Register(1));

        let call_dst = Some(IRValue::Register(1));
        let return_vals = vec![IRValue::Register(1)];

        assert!(TailCallLowerer::is_tail_call_eligible(
            &call_dst,
            &return_vals,
            &func
        ));
    }

    #[test]
    fn test_tail_call_ineligible_with_alloc() {
        let mut func = IRFunction::new("caller");
        func.params.push(IRValue::Register(0));
        func.results.push(IRValue::Register(2));

        // Add an alloc instruction.
        let block = func.current_block();
        block.push(IRInstr::Alloc {
            dst: IRValue::Register(10),
            size: 32,
        });

        let call_dst = Some(IRValue::Register(2));
        let return_vals = vec![IRValue::Register(2)];

        assert!(!TailCallLowerer::is_tail_call_eligible(
            &call_dst,
            &return_vals,
            &func
        ));
    }

    #[test]
    fn test_tail_call_ineligible_mismatch() {
        let func = IRFunction::new("caller");
        let call_dst = Some(IRValue::Register(1));
        let return_vals = vec![IRValue::Register(2)]; // Different register!

        assert!(!TailCallLowerer::is_tail_call_eligible(
            &call_dst,
            &return_vals,
            &func
        ));
    }

    #[test]
    fn test_tail_call_terminator() {
        let term = TailCallLowerer::make_tail_call_terminator(
            "callee",
            &[IRValue::Register(0), IRValue::Register(1)],
        );
        assert!(matches!(term, IRTerminator::TailCall { .. }));
    }

    #[test]
    fn test_loop_identification() {
        // Create a simple loop function:
        // entry → loop_header → loop_body → loop_header (back edge)
        //                       → exit
        let mut func = IRFunction::new("loop_func");
        func.blocks[0].label = "entry".to_string();
        func.blocks[0].terminator = IRTerminator::Jump("loop_header".to_string());

        func.append_block("loop_header");
        func.blocks[1].terminator = IRTerminator::Branch {
            cond: IRValue::Register(0),
            true_block: "loop_body".to_string(),
            false_block: "exit".to_string(),
        };

        func.append_block("loop_body");
        func.blocks[2].push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        func.blocks[2].terminator = IRTerminator::Jump("loop_header".to_string());

        func.append_block("exit");
        func.blocks[3].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let loops = LoopOptimizer::identify_loops(&func);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].header_block, "loop_header");
        assert_eq!(loops[0].back_edge_block, "loop_body");
        assert!(loops[0].exit_blocks.contains(&"exit".to_string()));
    }

    #[test]
    fn test_loop_unroll_eligibility() {
        let loop_info = LoopInfo {
            header_block: "header".to_string(),
            body_blocks: vec!["header".to_string(), "body".to_string()],
            exit_blocks: vec!["exit".to_string()],
            back_edge_block: "body".to_string(),
            trip_count: Some(8),
        };

        assert!(LoopOptimizer::is_unrollable(&loop_info, 4));
    }

    #[test]
    fn test_loop_unroll_ineligible_unknown_trip() {
        let loop_info = LoopInfo {
            header_block: "header".to_string(),
            body_blocks: vec!["header".to_string(), "body".to_string()],
            exit_blocks: vec!["exit".to_string()],
            back_edge_block: "body".to_string(),
            trip_count: None,
        };

        assert!(!LoopOptimizer::is_unrollable(&loop_info, 4));
    }

    #[test]
    fn test_choose_unroll_factor() {
        let loop_info = LoopInfo {
            header_block: "header".to_string(),
            body_blocks: vec!["body".to_string()],
            exit_blocks: vec!["exit".to_string()],
            back_edge_block: "body".to_string(),
            trip_count: Some(16),
        };
        assert_eq!(LoopOptimizer::choose_unroll_factor(&loop_info, 8), 8);
        assert_eq!(LoopOptimizer::choose_unroll_factor(&loop_info, 32), 16);

        let odd_trip = LoopInfo {
            header_block: "header".to_string(),
            body_blocks: vec!["body".to_string()],
            exit_blocks: vec!["exit".to_string()],
            back_edge_block: "body".to_string(),
            trip_count: Some(7),
        };
        // 7 is odd, so factor 2 doesn't divide it... actually 7%2 != 0 so
        // we won't pick 2. Best remains 1.
        // Actually let me re-check: the algorithm tries factor = 2, 4, 8...
        // 7 % 2 != 0 → skip. 7 % 4 != 0 → skip. Best stays 1.
        assert_eq!(LoopOptimizer::choose_unroll_factor(&odd_trip, 4), 1);
    }

    /// Wave 35 regression / decision test.
    ///
    /// `ExceptionLowerer` and `CoroutineLowerer` were deleted in Wave 35
    /// because `.vuma` has no syntax that feeds them (see the module-level
    /// "Wave 35 decision" doc comment for the full audit). This test
    /// verifies that the surviving lowerers — `SwitchLowerer`,
    /// `TailCallLowerer`, and `LoopOptimizer` — still work end-to-end on a
    /// hand-built IR after the deletion, and that the file still compiles
    /// (which transitively proves the deleted types left no dangling
    /// references inside `control_flow.rs`).
    ///
    /// The audit evidence:
    /// - `src/parser/src/lexer.rs` has token kinds for `Async`, `Await`,
    ///   `Spawn`, `Lock`, `Unlock`, `Channel`, `Send`, `Recv` — but NOT for
    ///   `try`, `catch`, `raise`, `throw`, `yield`, `resume`, or
    ///   `coroutine`.
    /// - `src/parser/src/ast.rs` defines `Expr::Async` and `Expr::Await`
    ///   but no `Try`/`Catch`/`Raise`/`Throw`/`Yield`/`Resume` nodes.
    /// - `src/parser/src/to_scg.rs` lowers `Expr::Async` into a parallel
    ///   region (line ~1626) — NOT into `IRTerminator::Invoke` or any
    ///   coroutine IR. The `IRTerminator::Invoke` and `IRTerminator::Resume`
    ///   enum variants (defined in `src/codegen/src/ir.rs`, outside this
    ///   file's ownership) are therefore unreachable from any `.vuma`
    ///   source.
    /// - A repo-wide `.vuma` file sweep finds the words "try"/"catch"/
    ///   "raise"/"throw"/"yield" only inside English-language comments,
    ///   never as language constructs.
    #[test]
    fn test_wave35_exception_coroutine_removed() {
        // --- SwitchLowerer still works ---
        let targets = vec![
            (0i64, "zero".to_string()),
            (1, "one".to_string()),
            (2, "two".to_string()),
        ];
        let strategy = SwitchLowerer::choose_strategy(&targets, "default");
        assert_eq!(strategy, SwitchStrategy::IfElseChain);

        let mut vreg = 100u32;
        let mut label = 100u32;
        let blocks = SwitchLowerer::lower_if_else_chain(
            IRValue::Register(0),
            &targets,
            "default",
            &mut vreg,
            &mut label,
        );
        assert_eq!(blocks.len(), targets.len());

        // --- TailCallLowerer still works ---
        let mut func = IRFunction::new("wave35_caller");
        func.params.push(IRValue::Register(0));
        func.results.push(IRValue::Register(1));
        let call_dst = Some(IRValue::Register(1));
        let return_vals = vec![IRValue::Register(1)];
        assert!(TailCallLowerer::is_tail_call_eligible(
            &call_dst,
            &return_vals,
            &func
        ));

        // --- LoopOptimizer still works ---
        let mut func = IRFunction::new("wave35_loop");
        func.blocks[0].label = "entry".to_string();
        func.blocks[0].terminator = IRTerminator::Jump("loop_header".to_string());

        func.append_block("loop_header");
        func.blocks[1].terminator = IRTerminator::Branch {
            cond: IRValue::Register(0),
            true_block: "loop_body".to_string(),
            false_block: "exit".to_string(),
        };

        func.append_block("loop_body");
        func.blocks[2].push(IRInstr::BinOp {
            op: BinOpKind::Add,
            dst: IRValue::Register(1),
            lhs: IRValue::Register(1),
            rhs: IRValue::Immediate(1),
            ty: None,
        });
        func.blocks[2].terminator = IRTerminator::Jump("loop_header".to_string());

        func.append_block("exit");
        func.blocks[3].terminator = IRTerminator::Return(vec![IRValue::Register(1)]);

        let loops = LoopOptimizer::identify_loops(&func);
        assert_eq!(loops.len(), 1);
        assert_eq!(loops[0].header_block, "loop_header");

        // If this test compiles AND runs, the deletion is sound:
        // `ExceptionLowerer` and `CoroutineLowerer` (and their supporting
        // types `LandingPad`, `ExceptionAction`, `ExceptionTableEntry`,
        // `InvokeLowering`, `CoroutineState`, `YieldPoint`, `CoroutineFrame`,
        // and the helpers `align_to`, `terminator_used_regs`,
        // `compute_live_in`, `find_yield_points`, `collect_local_vars`) are
        // gone from this module, with no dangling references.
    }

}
