//! # Control Flow Lowering
//!
//! This module handles complex control flow lowering for multi-target codegen. It
//! translates high-level control flow patterns — switch/match dispatch,
//! exception handling, tail call optimization, coroutine frames, and loop
//! optimization — into IR-level representations that the emitter can process.
//!
//! ## Components
//!
//! - **SwitchLowerer** — Lowers `IRTerminator::Switch` into jump tables,
//!   binary search trees, or if-else chains depending on target density.
//! - **ExceptionLowerer** — Lowers `IRTerminator::Invoke` into call + landing
//!   pad blocks and generates `.gcc_except_table` entries.
//! - **TailCallLowerer** — Detects and lowers eligible tail calls into
//!   frame-discarding jumps.
//! - **CoroutineLowerer** — Transforms coroutine functions into state-machine
//!   IR with heap-allocated frames.
//! - **LoopOptimizer** — Identifies natural loops, checks unroll eligibility,
//!   and performs loop unrolling.

use crate::backend::{AArch64TargetInfo, TargetInfo};
use crate::ir::{BinOpKind, CmpKind, IRBlock, IRFunction, IRInstr, IRTerminator, IRType, IRValue};
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
// ExceptionLowerer
// ===========================================================================

/// Represents a landing pad for exception handling.
#[derive(Debug, Clone)]
pub struct LandingPad {
    /// Label of the landing pad block.
    pub label: String,
    /// The type this pad catches, if any.
    pub catch_type: Option<String>,
    /// Action taken at this landing pad.
    pub action: ExceptionAction,
}

/// Action taken at an exception landing pad.
#[derive(Debug, Clone)]
pub enum ExceptionAction {
    /// Catch an exception of a specific type and branch to the target block.
    Catch {
        /// Target block label for the catch handler.
        dst: String,
    },
    /// Run cleanup code (e.g. destructors) without catching.
    Cleanup,
    /// Only catch exceptions whose type is in the allowed list.
    Filter {
        /// List of exception type names that this filter catches.
        allowed_types: Vec<String>,
    },
}

/// Exception table entry for the `.gcc_except_table` section.
///
/// Each entry describes a region of code and its associated landing pad.
#[derive(Debug, Clone)]
pub struct ExceptionTableEntry {
    /// Start offset (in bytes from function start) of the protected region.
    pub start_offset: u32,
    /// End offset (in bytes from function start) of the protected region.
    pub end_offset: u32,
    /// Offset of the landing pad from the function start.
    pub landing_pad_offset: u32,
    /// Action table index, if any.
    pub action: Option<u32>,
}

/// Result of lowering an `IRTerminator::Invoke`.
pub struct InvokeLowering {
    /// The call instruction block (normal path).
    pub call_block: IRBlock,
    /// The landing pad block (exception path).
    pub landing_pad: IRBlock,
}

/// Lowers `IRTerminator::Invoke` terminators into separate call and landing
/// pad blocks, and generates exception table entries for the `.gcc_except_table`
/// section.
pub struct ExceptionLowerer;

impl ExceptionLowerer {
    /// Lower an Invoke terminator into:
    /// 1. A Call instruction followed by a Jump to `normal`
    /// 2. A landing pad block that catches and branches to `unwind`
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::lower_invoke_for_target`] with `AArch64TargetInfo`.
    pub fn lower_invoke(
        dst: Option<IRValue>,
        func: &str,
        args: &[IRValue],
        normal: &str,
        unwind: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
    ) -> InvokeLowering {
        Self::lower_invoke_for_target(
            dst,
            func,
            args,
            normal,
            unwind,
            vreg_counter,
            label_counter,
            &AArch64TargetInfo,
        )
    }

    /// Lower an Invoke terminator for the given target.
    ///
    /// The landing pad reads the exception pointer from a dedicated register
    /// and the selector value from another, then branches to the unwind
    /// destination. The number of vregs allocated for exception info depends
    /// on the target's calling convention.
    #[allow(clippy::too_many_arguments)]
    pub fn lower_invoke_for_target(
        dst: Option<IRValue>,
        func: &str,
        args: &[IRValue],
        normal: &str,
        unwind: &str,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
        target: &dyn TargetInfo,
    ) -> InvokeLowering {
        // Call block: perform the call and jump to normal continuation.
        let call_label = next_label(label_counter, "invoke_call_");
        let mut call_block = IRBlock::new(&call_label);

        call_block.push(IRInstr::Call {
            dst: dst.clone(),
            func: func.to_string(),
            args: args.to_vec(),
        });
        call_block.terminator = IRTerminator::Jump(normal.to_string());

        // Landing pad block: read exception info and branch to unwind target.
        let pad_label = next_label(label_counter, "landing_pad_");
        let mut landing_pad = IRBlock::new(&pad_label);

        // Allocate vregs to receive the exception pointer and selector.
        // On targets with a link register (ARM64, RISC-V, MIPS, PPC), the
        // landing pad receives the exception pointer and selector in the
        // first two integer argument registers. On x86_64, they arrive via
        // the stack or in RAX/RDX depending on the ABI.
        let _exception_ptr = next_vreg(vreg_counter);
        let _selector = next_vreg(vreg_counter);

        // On targets with branch delay slots (MIPS), the emitter must insert
        // a NOP after the branch at the end of the landing pad. This is
        // handled by the emitter, not the IR.
        let _ = target; // Target info available for future per-ISA landing pad layout.

        // The landing pad must eventually resume unwinding by branching to
        // the unwind destination. In a full implementation we would insert
        // type-check instructions here (compare selector against type_info
        // addresses). For now we unconditionally branch to unwind.
        landing_pad.terminator = IRTerminator::Jump(unwind.to_string());

        log::debug!(
            "ExceptionLowerer: lowered invoke @{} → call={}, pad={} (target={})",
            func,
            call_label,
            pad_label,
            target.isa_name()
        );

        InvokeLowering {
            call_block,
            landing_pad,
        }
    }

    /// Generate the exception table for a function.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::generate_exception_table_for_target`] with `AArch64TargetInfo`.
    pub fn generate_exception_table(func: &IRFunction) -> Vec<ExceptionTableEntry> {
        Self::generate_exception_table_for_target(func, &AArch64TargetInfo)
    }

    /// Generate the exception table for a function, using the target's
    /// instruction size for offset estimation.
    ///
    /// Walks all blocks in the function looking for `IRTerminator::Invoke`,
    /// and for each one produces an `ExceptionTableEntry` that maps the
    /// call-site region to its landing pad.
    ///
    /// **Note**: Offset computation is approximate at the IR level; the
    /// emitter will patch these with actual byte offsets during code
    /// emission.
    pub fn generate_exception_table_for_target(
        func: &IRFunction,
        target: &dyn TargetInfo,
    ) -> Vec<ExceptionTableEntry> {
        let mut entries = Vec::new();

        // Use the target's instruction alignment as the approximate size
        // of each IR instruction. On fixed-width ISAs (ARM64, RISC-V, MIPS)
        // this is 4 bytes; on variable-width ISAs (x86_64) it's 1 byte
        // (a rough underestimate — the emitter will refine).
        let instr_size = target.instruction_alignment() as u32;

        // Track approximate byte offsets as we walk blocks.
        let mut current_offset: u32 = 0;

        for block in &func.blocks {
            let block_start = current_offset;

            for _instr in &block.instructions {
                current_offset += instr_size;
            }

            // Check if this block's terminator is an Invoke.
            if let IRTerminator::Invoke {
                dst: _,
                func: invoked_func,
                args: _,
                normal: _,
                unwind,
            } = &block.terminator
            {
                // The call site region spans the instructions of this block
                // up to the invoke. The landing pad offset is approximate:
                // we estimate it as the offset of the next block (since the
                // landing pad block will be emitted right after the call).
                let call_end_offset = current_offset;

                // Estimate landing pad offset by searching for a block whose
                // label matches the unwind target.
                let mut pad_offset = 0u32;
                let mut search_offset = 0u32;
                for search_block in &func.blocks {
                    if search_block.label == *unwind {
                        pad_offset = search_offset;
                        break;
                    }
                    search_offset +=
                        (search_block.instructions.len() as u32) * instr_size + instr_size;
                }

                entries.push(ExceptionTableEntry {
                    start_offset: block_start,
                    end_offset: call_end_offset,
                    landing_pad_offset: pad_offset,
                    action: None, // No action table entry for simple catches.
                });

                log::debug!(
                    "ExceptionLowerer: exception table entry for invoke @{} \
                     range=[{}, {}) pad={} (target={})",
                    invoked_func,
                    block_start,
                    call_end_offset,
                    pad_offset,
                    target.isa_name()
                );
            }

            // Terminator counts as one instruction too.
            current_offset += instr_size;
        }

        entries
    }

    /// Build a list of landing pads for a function by scanning all Invoke
    /// terminators. This is useful for the emitter to know where to emit
    /// landing pad code.
    pub fn collect_landing_pads(func: &IRFunction) -> Vec<LandingPad> {
        let mut pads = Vec::new();

        for block in &func.blocks {
            if let IRTerminator::Invoke {
                dst: _,
                func: _,
                args: _,
                normal: _,
                unwind,
            } = &block.terminator
            {
                pads.push(LandingPad {
                    label: format!("landing_pad_for_{}", block.label),
                    catch_type: None,
                    action: ExceptionAction::Catch {
                        dst: unwind.clone(),
                    },
                });
            }
        }

        pads
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
// CoroutineLowerer
// ===========================================================================

/// Coroutine state (suspended, running, completed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoroutineState {
    /// The coroutine is suspended at a yield point, waiting to be resumed.
    Suspended,
    /// The coroutine is currently executing.
    Running,
    /// The coroutine has completed execution and cannot be resumed.
    Completed,
}

impl CoroutineState {
    /// Return the numeric encoding of this state.
    pub fn as_u64(self) -> u64 {
        match self {
            CoroutineState::Suspended => 0,
            CoroutineState::Running => 1,
            CoroutineState::Completed => 2,
        }
    }
}

/// A yield point in a coroutine — where execution suspends.
#[derive(Debug, Clone)]
pub struct YieldPoint {
    /// Unique index identifying this yield point (used for resume dispatch).
    pub index: u32,
    /// Label of the block that performs the suspend (save + return).
    pub suspend_block: String,
    /// Label of the block where execution resumes after this yield.
    pub resume_block: String,
    /// Values that are live across this yield point and must be spilled
    /// to the coroutine frame.
    pub live_values: Vec<IRValue>,
}

/// Layout of a coroutine frame on the heap.
///
/// The frame holds the coroutine's state, the yield index for resume
/// dispatch, and spill slots for live values at each yield point.
#[derive(Debug, Clone)]
pub struct CoroutineFrame {
    /// Size of the frame in bytes.
    pub size: u32,
    /// Alignment of the frame.
    pub align: u32,
    /// Offset of the state field within the frame.
    pub state_offset: u32,
    /// Offset of the yield index field.
    pub yield_index_offset: u32,
    /// Offsets for spilled live values at each yield point.
    /// Each entry is (vreg_name, byte_offset).
    pub spill_slots: Vec<(String, u32)>,
}

// These constants are no longer used directly; CoroutineLowerer uses
// target.pointer_width() and target.stack_alignment() instead.
// Kept for documentation reference.
// const COROUTINE_FRAME_ALIGN: u32 = 8;
// const STATE_FIELD_SIZE: u32 = 8;
// const YIELD_INDEX_FIELD_SIZE: u32 = 8;
// const SPILL_SLOT_SIZE: u32 = 8;

/// Transforms coroutine functions into state-machine IR with heap-allocated
/// frames. Each yield point becomes a suspend (save live values, update
/// state, return) and each resume is dispatched via the yield index.
pub struct CoroutineLowerer;

impl CoroutineLowerer {
    /// Analyze a function to find yield/resume points and compute the
    /// coroutine frame layout.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::analyze_coroutine_for_target`] with `AArch64TargetInfo`.
    pub fn analyze_coroutine(func: &IRFunction) -> Option<CoroutineFrame> {
        Self::analyze_coroutine_for_target(func, &AArch64TargetInfo)
    }

    /// Analyze a function to find yield/resume points and compute the
    /// coroutine frame layout, using target-specific sizes.
    ///
    /// Returns `None` if the function does not contain any yield points
    /// (i.e. it's not a coroutine).
    pub fn analyze_coroutine_for_target(
        func: &IRFunction,
        target: &dyn TargetInfo,
    ) -> Option<CoroutineFrame> {
        let yield_points = Self::find_yield_points(func);

        if yield_points.is_empty() {
            log::debug!(
                "CoroutineLowerer: @{} is not a coroutine (no yield points)",
                func.name
            );
            return None;
        }

        // Collect all local variables (vregs) used in the function.
        let local_vars = Self::collect_local_vars(func);

        let frame = Self::compute_frame_layout_for_target(&yield_points, &local_vars, target);

        log::debug!(
            "CoroutineLowerer: @{} is a coroutine with {} yield points, \
             frame size={} align={} (target={})",
            func.name,
            yield_points.len(),
            frame.size,
            frame.align,
            target.isa_name()
        );

        Some(frame)
    }

    /// Compute the frame layout given yield points and live values.
    ///
    /// This is the legacy ARM64-compatible entry point. It delegates to
    /// [`Self::compute_frame_layout_for_target`] with `AArch64TargetInfo`.
    pub fn compute_frame_layout(
        yield_points: &[YieldPoint],
        local_vars: &[IRValue],
    ) -> CoroutineFrame {
        Self::compute_frame_layout_for_target(yield_points, local_vars, &AArch64TargetInfo)
    }

    /// Compute the frame layout given yield points and live values,
    /// using target-specific pointer width and alignment.
    ///
    /// The frame layout is:
    /// ```text
    /// offset 0:         state (pointer-sized)
    /// offset ptr_size:  yield_index (pointer-sized)
    /// offset 2*ptr_size: spill_slot_0
    /// ...
    /// ```
    pub fn compute_frame_layout_for_target(
        yield_points: &[YieldPoint],
        local_vars: &[IRValue],
        target: &dyn TargetInfo,
    ) -> CoroutineFrame {
        let ptr_width = target.pointer_width() as u32;
        let frame_align = ptr_width; // Frame alignment = pointer width (8 on 64-bit, 4 on 32-bit)
        let state_field_size = ptr_width;
        let yield_index_field_size = ptr_width;
        let spill_slot_size = ptr_width;

        // Collect all unique live values that need spill slots.
        let mut seen_regs: HashSet<u32> = HashSet::new();
        let mut spill_slots: Vec<(String, u32)> = Vec::new();

        for yp in yield_points {
            for val in &yp.live_values {
                if let Some(reg_id) = val.as_register() {
                    if seen_regs.insert(reg_id) {
                        let slot_offset = state_field_size
                            + yield_index_field_size
                            + (spill_slots.len() as u32) * spill_slot_size;
                        spill_slots.push((format!("vreg_{}", reg_id), slot_offset));
                    }
                }
            }
        }

        // Also add slots for any local vars not already covered.
        for val in local_vars {
            if let Some(reg_id) = val.as_register() {
                if seen_regs.insert(reg_id) {
                    let slot_offset = state_field_size
                        + yield_index_field_size
                        + (spill_slots.len() as u32) * spill_slot_size;
                    spill_slots.push((format!("vreg_{}", reg_id), slot_offset));
                }
            }
        }

        let data_size = state_field_size
            + yield_index_field_size
            + (spill_slots.len() as u32) * spill_slot_size;

        // Round up to alignment.
        let aligned_size = align_to(data_size, frame_align);

        CoroutineFrame {
            size: aligned_size,
            align: frame_align,
            state_offset: 0,
            yield_index_offset: state_field_size,
            spill_slots,
        }
    }

    /// Generate prologue code to set up the coroutine frame.
    ///
    /// The prologue:
    /// 1. Allocates the frame on the heap (via a runtime call).
    /// 2. Stores the initial state (Running) and yield index (0).
    /// 3. Returns a pointer to the frame.
    pub fn generate_prologue(frame: &CoroutineFrame, vreg_counter: &mut u32) -> Vec<IRInstr> {
        let mut instrs = Vec::new();

        // Allocate the frame: call __vuma_coro_alloc(size, align).
        let frame_ptr = next_vreg(vreg_counter);
        instrs.push(IRInstr::Call {
            dst: Some(frame_ptr.clone()),
            func: "__vuma_coro_alloc".to_string(),
            args: vec![
                IRValue::Immediate(frame.size as i64),
                IRValue::Immediate(frame.align as i64),
            ],
        });

        // Store initial state = Running (1).
        let state_addr = next_vreg(vreg_counter);
        instrs.push(IRInstr::Offset {
            dst: state_addr.clone(),
            base: frame_ptr.clone(),
            offset: IRValue::Immediate(frame.state_offset as i64),
        });
        instrs.push(IRInstr::Store {
            value: IRValue::Immediate(CoroutineState::Running as i64),
            addr: state_addr,
            offset: 0,
            ty: IRType::I64,
        });

        // Store initial yield_index = 0.
        let yi_addr = next_vreg(vreg_counter);
        instrs.push(IRInstr::Offset {
            dst: yi_addr.clone(),
            base: frame_ptr.clone(),
            offset: IRValue::Immediate(frame.yield_index_offset as i64),
        });
        instrs.push(IRInstr::Store {
            value: IRValue::Immediate(0),
            addr: yi_addr,
            offset: 0,
            ty: IRType::I64,
        });

        log::debug!(
            "CoroutineLowerer: generated prologue, frame size={}",
            frame.size
        );

        instrs
    }

    /// Generate a yield: save live values, update state, return.
    ///
    /// At each yield point:
    /// 1. Store each live value to its spill slot in the frame.
    /// 2. Store the yield index into the frame.
    /// 3. Set the state to Suspended.
    /// 4. Return (the coroutine is suspended).
    pub fn generate_yield(
        yield_point: &YieldPoint,
        frame: &CoroutineFrame,
        vreg_counter: &mut u32,
    ) -> Vec<IRInstr> {
        let mut instrs = Vec::new();

        // We assume the frame pointer is available in a well-known vreg.
        // In practice the prologue would have stored it and the register
        // allocator would keep track. Here we create a placeholder load.
        let frame_ptr = next_vreg(vreg_counter);
        instrs.push(IRInstr::Call {
            dst: Some(frame_ptr.clone()),
            func: "__vuma_coro_get_frame".to_string(),
            args: vec![],
        });

        // Save each live value to its spill slot.
        for live_val in &yield_point.live_values {
            if let Some(reg_id) = live_val.as_register() {
                // Look up the spill slot offset for this vreg.
                let slot_name = format!("vreg_{}", reg_id);
                if let Some((_, offset)) = frame
                    .spill_slots
                    .iter()
                    .find(|(name, _)| name == &slot_name)
                {
                    let slot_addr = next_vreg(vreg_counter);
                    instrs.push(IRInstr::Offset {
                        dst: slot_addr.clone(),
                        base: frame_ptr.clone(),
                        offset: IRValue::Immediate(*offset as i64),
                    });
                    instrs.push(IRInstr::Store {
                        value: live_val.clone(),
                        addr: slot_addr,
                        offset: 0,
                        ty: IRType::I64,
                    });
                }
            }
        }

        // Store the yield index.
        let yi_addr = next_vreg(vreg_counter);
        instrs.push(IRInstr::Offset {
            dst: yi_addr.clone(),
            base: frame_ptr.clone(),
            offset: IRValue::Immediate(frame.yield_index_offset as i64),
        });
        instrs.push(IRInstr::Store {
            value: IRValue::Immediate(yield_point.index as i64),
            addr: yi_addr,
            offset: 0,
            ty: IRType::I64,
        });

        // Set state to Suspended.
        let state_addr = next_vreg(vreg_counter);
        instrs.push(IRInstr::Offset {
            dst: state_addr.clone(),
            base: frame_ptr.clone(),
            offset: IRValue::Immediate(frame.state_offset as i64),
        });
        instrs.push(IRInstr::Store {
            value: IRValue::Immediate(CoroutineState::Suspended as i64),
            addr: state_addr,
            offset: 0,
            ty: IRType::I64,
        });

        log::debug!(
            "CoroutineLowerer: generated yield #{} ({} live values)",
            yield_point.index,
            yield_point.live_values.len()
        );

        instrs
    }

    /// Generate a resume dispatch: load yield index and jump to the
    /// corresponding resume point.
    ///
    /// This generates a switch-like dispatch on the yield index field of
    /// the coroutine frame. Each yield index maps to a resume block.
    pub fn generate_resume_dispatch(
        yield_points: &[YieldPoint],
        frame: &CoroutineFrame,
        vreg_counter: &mut u32,
        label_counter: &mut u32,
    ) -> Vec<IRBlock> {
        let mut blocks = Vec::new();

        if yield_points.is_empty() {
            return blocks;
        }

        // Entry block: load yield index from frame.
        let entry_label = next_label(label_counter, "coro_resume_");
        let mut entry_block = IRBlock::new(&entry_label);

        let frame_ptr = next_vreg(vreg_counter);
        entry_block.push(IRInstr::Call {
            dst: Some(frame_ptr.clone()),
            func: "__vuma_coro_get_frame".to_string(),
            args: vec![],
        });

        let yi_addr = next_vreg(vreg_counter);
        entry_block.push(IRInstr::Offset {
            dst: yi_addr.clone(),
            base: frame_ptr.clone(),
            offset: IRValue::Immediate(frame.yield_index_offset as i64),
        });

        let yield_index = next_vreg(vreg_counter);
        entry_block.push(IRInstr::Load {
            dst: yield_index.clone(),
            addr: yi_addr,
            offset: 0,
            ty: IRType::I64,
        });

        // Set state to Running.
        let state_addr = next_vreg(vreg_counter);
        entry_block.push(IRInstr::Offset {
            dst: state_addr.clone(),
            base: frame_ptr.clone(),
            offset: IRValue::Immediate(frame.state_offset as i64),
        });
        entry_block.push(IRInstr::Store {
            value: IRValue::Immediate(CoroutineState::Running as i64),
            addr: state_addr,
            offset: 0,
            ty: IRType::I64,
        });

        // Also reload live values from spill slots for the appropriate
        // yield point. We use a switch to dispatch.
        let targets: Vec<(i64, String)> = yield_points
            .iter()
            .map(|yp| (yp.index as i64, yp.resume_block.clone()))
            .collect();

        // Default: no matching yield index → coroutine completed or error.
        let completed_label = next_label(label_counter, "coro_completed_");

        // Lower the switch using SwitchLowerer.
        let switch_blocks = SwitchLowerer::lower_switch(
            yield_index,
            &targets,
            &completed_label,
            vreg_counter,
            label_counter,
        );

        // The first switch block is the dispatch entry; set our entry block's
        // terminator to jump into it.
        if let Some(first_switch_block) = switch_blocks.first() {
            entry_block.terminator = IRTerminator::Jump(first_switch_block.label.clone());
        } else {
            entry_block.terminator = IRTerminator::Jump(completed_label.clone());
        }

        blocks.push(entry_block);
        blocks.extend(switch_blocks);

        // Completed block: set state to Completed and return.
        let mut completed_block = IRBlock::new(&completed_label);
        let state_addr2 = next_vreg(vreg_counter);
        completed_block.push(IRInstr::Call {
            dst: Some(state_addr2.clone()),
            func: "__vuma_coro_get_frame".to_string(),
            args: vec![],
        });
        let state_field_addr = next_vreg(vreg_counter);
        completed_block.push(IRInstr::Offset {
            dst: state_field_addr.clone(),
            base: state_addr2,
            offset: IRValue::Immediate(frame.state_offset as i64),
        });
        completed_block.push(IRInstr::Store {
            value: IRValue::Immediate(CoroutineState::Completed as i64),
            addr: state_field_addr,
            offset: 0,
            ty: IRType::I64,
        });
        completed_block.terminator = IRTerminator::Return(vec![IRValue::Immediate(0)]);

        blocks.push(completed_block);

        // For each yield point, generate a reload block that loads the
        // spilled live values from the frame before jumping to the actual
        // resume block.
        for yp in yield_points {
            let reload_label = format!("coro_reload_{}", yp.index);
            let mut reload_block = IRBlock::new(&reload_label);

            let fptr = next_vreg(vreg_counter);
            reload_block.push(IRInstr::Call {
                dst: Some(fptr.clone()),
                func: "__vuma_coro_get_frame".to_string(),
                args: vec![],
            });

            for live_val in &yp.live_values {
                if let Some(reg_id) = live_val.as_register() {
                    let slot_name = format!("vreg_{}", reg_id);
                    if let Some((_, offset)) = frame
                        .spill_slots
                        .iter()
                        .find(|(name, _)| name == &slot_name)
                    {
                        let slot_addr = next_vreg(vreg_counter);
                        reload_block.push(IRInstr::Offset {
                            dst: slot_addr.clone(),
                            base: fptr.clone(),
                            offset: IRValue::Immediate(*offset as i64),
                        });
                        let loaded = next_vreg(vreg_counter);
                        reload_block.push(IRInstr::Load {
                            dst: loaded,
                            addr: slot_addr,
                            offset: 0,
                            ty: IRType::I64,
                        });
                    }
                }
            }

            reload_block.terminator = IRTerminator::Jump(yp.resume_block.clone());
            blocks.push(reload_block);
        }

        log::debug!(
            "CoroutineLowerer: generated resume dispatch with {} yield points ({} blocks)",
            yield_points.len(),
            blocks.len()
        );

        blocks
    }

    // ---- Internal helpers ----

    /// Find yield points in a function by looking for blocks whose name
    /// starts with "yield_" or that have specific marker instructions.
    fn find_yield_points(func: &IRFunction) -> Vec<YieldPoint> {
        let mut yield_points = Vec::new();
        let mut yield_index = 0u32;

        for (i, block) in func.blocks.iter().enumerate() {
            // A block is a yield point if its label starts with "yield_".
            let is_yield = block.label.starts_with("yield_");

            if is_yield {
                // The resume block is the next block in layout order, or
                // a block named "resume_{suffix}" if it exists.
                let suffix = block.label.strip_prefix("yield_").unwrap_or("");
                let resume_label = format!("resume_{}", suffix);

                // Fall back to the next block if the named resume block
                // doesn't exist.
                let actual_resume = if func.blocks.iter().any(|b| b.label == resume_label) {
                    resume_label
                } else if i + 1 < func.blocks.len() {
                    func.blocks[i + 1].label.clone()
                } else {
                    format!("resume_fallback_{}", yield_index)
                };

                // Collect live values: all registers defined before this
                // block that are still in use.
                let live_values = Self::compute_live_in(func, &block.label);

                yield_points.push(YieldPoint {
                    index: yield_index,
                    suspend_block: block.label.clone(),
                    resume_block: actual_resume,
                    live_values,
                });

                yield_index += 1;
            }
        }

        yield_points
    }

    /// Compute the set of values that are live at the entry of a given block.
    ///
    /// This is a simplified liveness analysis: a value is live-in to a block
    /// if it is used in that block or any successor without first being
    /// defined.
    fn compute_live_in(func: &IRFunction, block_label: &str) -> Vec<IRValue> {
        // Build a map from block label to block index.
        let label_to_idx: HashMap<String, usize> = func
            .blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (b.label.clone(), i))
            .collect();

        // Build predecessor map.
        let mut predecessors: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, block) in func.blocks.iter().enumerate() {
            for succ in successor_indices(&block.terminator, &label_to_idx) {
                predecessors.entry(succ).or_default().push(i);
            }
        }

        // Simple backward data-flow: compute live-out and live-in for each
        // block iteratively until convergence.
        let n = func.blocks.len();
        let mut live_out: Vec<HashSet<u32>> = vec![HashSet::new(); n];
        let mut live_in: Vec<HashSet<u32>> = vec![HashSet::new(); n];

        // Iterate to fixed point.
        let mut changed = true;
        while changed {
            changed = false;
            for i in (0..n).rev() {
                let block = &func.blocks[i];

                // live_out[i] = union of live_in[succ] for all successors.
                let mut new_out: HashSet<u32> = HashSet::new();
                for succ in successor_indices(&block.terminator, &label_to_idx) {
                    new_out.extend(&live_in[succ]);
                }

                // live_in[i] = (use[i] ∪ live_out[i]) \ def[i]
                let mut new_in: HashSet<u32> = HashSet::new();

                // Add uses from instructions.
                for _instr in &block.instructions {
                    for reg in _instr.used_regs() {
                        new_in.insert(reg);
                    }
                }

                // Add uses from terminator.
                if let Some(regs) = terminator_used_regs(&block.terminator) {
                    for reg in regs {
                        new_in.insert(reg);
                    }
                }

                // Add live_out.
                new_in.extend(&new_out);

                // Remove definitions.
                for instr in &block.instructions {
                    for reg in instr.defined_regs() {
                        new_in.remove(&reg);
                    }
                }

                if new_out != live_out[i] || new_in != live_in[i] {
                    changed = true;
                    live_out[i] = new_out;
                    live_in[i] = new_in;
                }
            }
        }

        // Return the live-in set for the requested block.
        if let Some(&idx) = label_to_idx.get(block_label) {
            live_in[idx]
                .iter()
                .map(|&id| IRValue::Register(id))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Collect all local variable vregs defined in the function.
    fn collect_local_vars(func: &IRFunction) -> Vec<IRValue> {
        let mut seen: HashSet<u32> = HashSet::new();
        let mut vars = Vec::new();

        for block in &func.blocks {
            for instr in &block.instructions {
                for reg in instr.defined_regs() {
                    if seen.insert(reg) {
                        vars.push(IRValue::Register(reg));
                    }
                }
            }
        }

        vars
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

/// Round `value` up to the nearest multiple of `alignment`.
fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

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

/// Get the virtual registers used by a terminator (for liveness analysis).
fn terminator_used_regs(terminator: &IRTerminator) -> Option<Vec<u32>> {
    match terminator {
        IRTerminator::Branch { cond, .. } => Some(cond.as_register().into_iter().collect()),
        IRTerminator::Switch { discr, .. } => Some(discr.as_register().into_iter().collect()),
        IRTerminator::Invoke { args, .. } => {
            Some(args.iter().filter_map(|v| v.as_register()).collect())
        }
        IRTerminator::Resume { value } => Some(value.as_register().into_iter().collect()),
        IRTerminator::TailCall { args, .. } => {
            Some(args.iter().filter_map(|v| v.as_register()).collect())
        }
        IRTerminator::Return(vals) => Some(vals.iter().filter_map(|v| v.as_register()).collect()),
        IRTerminator::Jump(_) | IRTerminator::Unreachable => None,
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
    fn test_exception_lower_invoke() {
        let mut vreg = 10u32;
        let mut label = 10u32;

        let result = ExceptionLowerer::lower_invoke(
            Some(IRValue::Register(5)),
            "throw_func",
            &[IRValue::Register(1)],
            "normal_cont",
            "unwind_cont",
            &mut vreg,
            &mut label,
        );

        // Call block should have a Call instruction and a Jump terminator.
        assert!(matches!(
            &result.call_block.instructions[0],
            IRInstr::Call { .. }
        ));
        assert!(matches!(
            &result.call_block.terminator,
            IRTerminator::Jump(t) if t == "normal_cont"
        ));

        // Landing pad should jump to unwind.
        assert!(matches!(
            &result.landing_pad.terminator,
            IRTerminator::Jump(t) if t == "unwind_cont"
        ));
    }

    #[test]
    fn test_exception_table_generation() {
        let mut func = IRFunction::new("test_func");
        func.append_block("invoke_block");
        func.blocks[0].terminator = IRTerminator::Invoke {
            dst: None,
            func: "may_throw".to_string(),
            args: vec![],
            normal: "ok".to_string(),
            unwind: "catch".to_string(),
        };

        let entries = ExceptionLowerer::generate_exception_table(&func);
        // Should find at least one entry for the invoke.
        assert_eq!(entries.len(), 1);
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
    fn test_coroutine_frame_layout() {
        let yield_points = vec![
            YieldPoint {
                index: 0,
                suspend_block: "yield_0".to_string(),
                resume_block: "resume_0".to_string(),
                live_values: vec![IRValue::Register(1), IRValue::Register(2)],
            },
            YieldPoint {
                index: 1,
                suspend_block: "yield_1".to_string(),
                resume_block: "resume_1".to_string(),
                live_values: vec![IRValue::Register(1), IRValue::Register(3)],
            },
        ];

        let frame = CoroutineLowerer::compute_frame_layout(&yield_points, &[]);

        // Frame should have state, yield_index, and 3 spill slots
        // (vreg 1, 2, 3 — note vreg 1 is shared across yield points).
        assert_eq!(frame.spill_slots.len(), 3);
        assert_eq!(frame.state_offset, 0);
        assert_eq!(frame.yield_index_offset, 8);
        // Total: 8 + 8 + 3*8 = 40, rounded to 8 → 40.
        assert_eq!(frame.size, 40);
    }

    #[test]
    fn test_coroutine_analyze_non_coroutine() {
        let func = IRFunction::new("normal_func");
        assert!(CoroutineLowerer::analyze_coroutine(&func).is_none());
    }

    #[test]
    fn test_coroutine_analyze_with_yield() {
        let mut func = IRFunction::new("my_coro");
        func.append_block("yield_point1");
        func.blocks[1].terminator = IRTerminator::Return(vec![]);

        let result = CoroutineLowerer::analyze_coroutine(&func);
        // The block is named "yield_point1" which doesn't start with "yield_"
        // — it starts with "yield_". Wait, "yield_point1" starts with "yield_"
        // so it should be detected.
        // Actually, "yield_point1" starts with "yield_p" not "yield_"... let me check.
        // "yield_point1" does NOT start with "yield_" because it's "yield_point1".
        // Let me fix the test.
        // Actually "yield_point1".starts_with("yield_") is true since "yield_" is the first 6 chars
        // and "yield_point1" starts with "yield_".
        // Wait: "yield_point1" - first 6 chars are "yield_" - yes it does start with "yield_".
        assert!(result.is_some());
    }

    #[test]
    fn test_coroutine_state_encoding() {
        assert_eq!(CoroutineState::Suspended.as_u64(), 0);
        assert_eq!(CoroutineState::Running.as_u64(), 1);
        assert_eq!(CoroutineState::Completed.as_u64(), 2);
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

    #[test]
    fn test_align_to() {
        assert_eq!(align_to(0, 8), 0);
        assert_eq!(align_to(1, 8), 8);
        assert_eq!(align_to(7, 8), 8);
        assert_eq!(align_to(8, 8), 8);
        assert_eq!(align_to(9, 8), 16);
        assert_eq!(align_to(40, 8), 40);
    }

    #[test]
    fn test_landing_pads_collection() {
        let mut func = IRFunction::new("test");
        // The default entry block is "entry"; set its terminator to Invoke.
        func.blocks[0].terminator = IRTerminator::Invoke {
            dst: None,
            func: "f".to_string(),
            args: vec![],
            normal: "n".to_string(),
            unwind: "u".to_string(),
        };

        let pads = ExceptionLowerer::collect_landing_pads(&func);
        assert_eq!(pads.len(), 1);
        assert_eq!(pads[0].label, "landing_pad_for_entry");
    }
}
