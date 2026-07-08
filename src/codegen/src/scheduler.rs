//! # Instruction Scheduler (Wave 5)
//!
//! List-scheduling algorithm that reorders instructions within a basic block
//! to minimize pipeline stalls. Uses `LatencyTable` from `target_desc.rs`
//! to model functional-unit pressure and load-use latency.
//!
//! ## Algorithm
//!
//! 1. Build a data-dependence graph (DDG) for the block.
//! 2. Compute earliest-start times based on predecessor latencies.
//! 3. List-schedule: at each cycle, issue ready instructions (all deps satisfied)
//!    prioritized by critical-path length (longest path to a leaf).
//! 4. Emit instructions in scheduled order.
//!
//! ## Limitations
//!
//! - Currently operates on raw IR instructions (pre-codegen).
//! - Does not model register pressure (handled by regalloc after scheduling).
//! - Cross-block scheduling not supported (basic block scope only).

use std::collections::{HashMap, HashSet, BinaryHeap};
use std::cmp::Ordering;
use crate::ir::{IRInstr, IRValue, IRTerminator};
use crate::target_desc::LatencyTable;

/// A node in the data-dependence graph.
#[derive(Debug, Clone)]
struct DDGNode {
    /// Index in the original instruction list.
    idx: usize,
    /// Instruction category for latency lookup.
    category: &'static str,
    /// Latency of this instruction.
    latency: u8,
    /// Predecessors (instructions that must complete before this one).
    preds: Vec<usize>,
    /// Successors (instructions that depend on this one).
    succs: Vec<usize>,
    /// Critical-path length: longest latency path from this node to any leaf.
    critical_path: u32,
    /// Earliest cycle this instruction can be issued.
    earliest_start: u32,
}

/// Priority queue entry: higher critical_path = higher priority.
#[derive(Debug, Clone, Eq)]
struct ReadyEntry {
    idx: usize,
    critical_path: u32,
}

impl Ord for ReadyEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher critical_path first (max-heap)
        other.critical_path.cmp(&self.critical_path)
            .then_with(|| self.idx.cmp(&other.idx))
    }
}

impl PartialEq for ReadyEntry {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}

impl PartialOrd for ReadyEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Classify an IR instruction into a latency category.
fn classify_instr(instr: &IRInstr) -> &'static str {
    match instr {
        IRInstr::Add { .. } | IRInstr::Sub { .. } | IRInstr::BinOp { op: _, .. } => "arithmetic",
        IRInstr::Mul { .. } => "multiply",
        IRInstr::Div { .. } => "divide",
        IRInstr::Load { .. } | IRInstr::AtomicLoad { .. } => "load",
        IRInstr::Store { .. } | IRInstr::AtomicStore { .. } => "store",
        IRInstr::Cmp { .. } => "arithmetic",
        IRInstr::Call { .. } => "branch", // Conservative: calls are serializing
        IRInstr::Cast { .. } | IRInstr::Offset { .. } | IRInstr::Alloc { .. } => "arithmetic",
        IRInstr::Branch { .. } | IRInstr::CondBranch { .. } => "branch",
        _ => "arithmetic",
    }
}

/// Extract the register IDs that an instruction defines (writes).
fn defined_regs(instr: &IRInstr) -> Vec<u32> {
    instr.defined_regs()
}

/// Extract the register IDs that an instruction uses (reads).
fn used_regs(instr: &IRInstr) -> Vec<u32> {
    instr.used_regs()
}

/// Schedule instructions in a single basic block using list-scheduling.
///
/// Returns a permutation of instruction indices in scheduled order.
/// If scheduling fails or is not beneficial, returns the identity permutation.
pub fn schedule_block(
    instructions: &[IRInstr],
    latency_table: &LatencyTable,
) -> Vec<usize> {
    let n = instructions.len();
    if n <= 2 {
        return (0..n).collect();
    }

    // If the block contains ANY memory operations (Load, Store, Alloc, Free,
    // AtomicLoad, AtomicStore), skip scheduling entirely and return the
    // identity permutation. This is extremely conservative but provably
    // sound — the scheduler's memory dependency tracking is incomplete
    // (it doesn't model address aliasing, and it misses some cross-block
    // memory effects after inlining/CSE/LICM). Rather than risk
    // miscompilation, we preserve the original instruction order for any
    // block that touches memory.
    //
    // The scheduler still runs on pure-computation blocks (no memory ops),
    // where it can safely reorder independent ALU operations.
    let has_memory_ops = instructions.iter().any(|i| {
        matches!(i,
            IRInstr::Load { .. } | IRInstr::Store { .. }
            | IRInstr::AtomicLoad { .. } | IRInstr::AtomicStore { .. }
            | IRInstr::Alloc { .. } | IRInstr::Free { .. }
            | IRInstr::Call { .. }
        )
    });
    if has_memory_ops {
        return (0..n).collect();
    }

    // ── Phi-node handling (Wave 5 SSA fix) ─────────────────────────────
    //
    // SSA semantics require:
    //   1. All Phi nodes appear at the TOP of their block, before any
    //      non-Phi instruction.
    //   2. Phi nodes execute "concurrently" at block entry — their incoming
    //      values come from predecessor blocks, not from intra-block defs.
    //      Therefore Phis have NO intra-block data dependencies.
    //   3. The relative order of Phi nodes among themselves doesn't matter
    //      semantically, but we preserve it to avoid surprising debug output.
    //
    // Implementation: we partition instructions into Phis (prefix) and
    // non-Phis (rest). We schedule only the non-Phis, then prepend the
    // Phis in their original order. This guarantees Phi nodes stay at the
    // top and non-Phis are scheduled among themselves.

    // Find the Phi prefix: all leading Phi instructions.
    let phi_count = instructions.iter().take_while(|i| matches!(i, IRInstr::Phi { .. })).count();
    if phi_count == n {
        // All instructions are Phis — nothing to schedule.
        return (0..n).collect();
    }

    // The non-Phi instructions are instructions[phi_count..].
    // We schedule only these, then prepend the Phi indices.
    let non_phi_instrs = &instructions[phi_count..];
    let non_phi_order = schedule_block_inner(non_phi_instrs, latency_table);

    // Build the full order: Phi indices (0..phi_count) in original order,
    // then scheduled non-Phi indices (offset by phi_count).
    let mut full_order: Vec<usize> = (0..phi_count).collect();
    for &idx in &non_phi_order {
        full_order.push(idx + phi_count);
    }
    full_order
}

/// Inner scheduler: schedules a slice of instructions that contains NO Phi
/// nodes. This is the original list-scheduling algorithm.
fn schedule_block_inner(
    instructions: &[IRInstr],
    latency_table: &LatencyTable,
) -> Vec<usize> {
    let n = instructions.len();
    if n <= 2 {
        return (0..n).collect();
    }

    // Build the data-dependence graph.
    let mut nodes: Vec<DDGNode> = Vec::with_capacity(n);
    let mut last_def: HashMap<u32, usize> = HashMap::new();

    for (i, instr) in instructions.iter().enumerate() {
        let category = classify_instr(instr);
        let (latency, _, _) = latency_table.lookup(category);

        // Find predecessors: any instruction that defines a register we use.
        let mut preds = Vec::new();
        for reg in used_regs(instr) {
            if let Some(&def_idx) = last_def.get(&reg) {
                if !preds.contains(&def_idx) {
                    preds.push(def_idx);
                }
            }
        }

        // Memory dependencies: all memory ops (Load, Store, AtomicLoad,
        // AtomicStore, Alloc, Free) depend on all previous memory ops.
        // This is conservative but SOUND — it prevents the scheduler from
        // reordering a Free before a Load/Store (use-after-free) or
        // reordering an Alloc after a Load that depends on it.
        // Free is also a barrier: it depends on ALL previous instructions
        // (not just memory ops), because freeing memory invalidates
        // everything that references it.
        if matches!(instr, IRInstr::Store { .. } | IRInstr::Load { .. }
                    | IRInstr::AtomicStore { .. } | IRInstr::AtomicLoad { .. }
                    | IRInstr::Alloc { .. } | IRInstr::Free { .. }) {
            for j in 0..i {
                if matches!(instructions[j], IRInstr::Store { .. } | IRInstr::Load { .. }
                            | IRInstr::AtomicStore { .. } | IRInstr::AtomicLoad { .. }
                            | IRInstr::Alloc { .. } | IRInstr::Free { .. }
                            | IRInstr::Call { .. }) {
                    if !preds.contains(&j) {
                        preds.push(j);
                    }
                }
            }
        }

        // Free depends on ALL previous instructions (it's a barrier —
        // nothing after it can use the freed memory, and everything
        // before it must have completed).
        // Calls also depend on all previous instructions (conservative).
        if matches!(instr, IRInstr::Free { .. } | IRInstr::Call { .. }) {
            for j in 0..i {
                if !preds.contains(&j) {
                    preds.push(j);
                }
            }
        }

        nodes.push(DDGNode {
            idx: i,
            category,
            latency,
            preds: preds.clone(),
            succs: Vec::new(),
            critical_path: 0,
            earliest_start: 0,
        });

        // Update last_def for this instruction's defined registers.
        for reg in defined_regs(instr) {
            last_def.insert(reg, i);
        }
    }

    // Build successor lists.
    for i in 0..n {
        let preds = nodes[i].preds.clone();
        for &pred in &preds {
            nodes[pred].succs.push(i);
        }
    }

    // Compute critical-path lengths (longest path from each node to a leaf).
    let mut visited = vec![false; n];
    fn compute_critical_path(
        nodes: &mut Vec<DDGNode>,
        idx: usize,
        visited: &mut Vec<bool>,
    ) {
        if visited[idx] {
            return;
        }
        visited[idx] = true;
        let succs = nodes[idx].succs.clone();
        let mut max_succ_path = 0u32;
        for &succ in &succs {
            compute_critical_path(nodes, succ, visited);
            max_succ_path = max_succ_path.max(nodes[succ].critical_path);
        }
        nodes[idx].critical_path = nodes[idx].latency as u32 + max_succ_path;
    }
    for i in 0..n {
        compute_critical_path(&mut nodes, i, &mut visited);
    }

    // List-schedule: at each step, pick the ready instruction with the
    // highest critical-path length.
    let mut scheduled: Vec<usize> = Vec::with_capacity(n);
    let mut scheduled_set: HashSet<usize> = HashSet::new();
    let mut ready: BinaryHeap<ReadyEntry> = BinaryHeap::new();
    let mut remaining_preds: Vec<usize> = nodes.iter().map(|n| n.preds.len()).collect();

    // Initialize ready queue with instructions that have no predecessors.
    for i in 0..n {
        if remaining_preds[i] == 0 {
            ready.push(ReadyEntry { idx: i, critical_path: nodes[i].critical_path });
        }
    }

    while let Some(entry) = ready.pop() {
        let idx = entry.idx;
        if scheduled_set.contains(&idx) {
            continue;
        }
        scheduled.push(idx);
        scheduled_set.insert(idx);

        // Decrement remaining predecessors for successors.
        for &succ in &nodes[idx].succs {
            if remaining_preds[succ] > 0 {
                remaining_preds[succ] -= 1;
                if remaining_preds[succ] == 0 {
                    ready.push(ReadyEntry {
                        idx: succ,
                        critical_path: nodes[succ].critical_path,
                    });
                }
            }
        }
    }

    // If we couldn't schedule everything (cycle?), fall back to original order.
    if scheduled.len() != n {
        return (0..n).collect();
    }

    scheduled
}

/// Schedule all blocks in a function.
///
/// Reorders instructions within each block to minimize stalls.
/// The terminator is always kept last.
pub fn schedule_function(
    blocks: &mut [crate::ir::IRBlock],
    latency_table: &LatencyTable,
) {
    for block in blocks.iter_mut() {
        if block.instructions.len() <= 2 {
            continue;
        }

        // Skip scheduling for blocks that contain ANY memory operations.
        // The scheduler's memory dependency tracking is not sound when
        // combined with CSE/inliner/LICM — it misses some aliasing cases
        // that cause miscompilation. Rather than risk incorrect reordering,
        // preserve the original instruction order for memory-touching blocks.
        // The scheduler still runs on pure-computation blocks.
        let has_memory_ops = block.instructions.iter().any(|i| {
            matches!(i,
                IRInstr::Load { .. } | IRInstr::Store { .. }
                | IRInstr::AtomicLoad { .. } | IRInstr::AtomicStore { .. }
                | IRInstr::Alloc { .. } | IRInstr::Free { .. }
                | IRInstr::Call { .. }
            )
        });
        if has_memory_ops {
            continue;
        }

        let order = schedule_block(&block.instructions, latency_table);
        let mut new_instrs = Vec::with_capacity(block.instructions.len());
        for &idx in &order {
            new_instrs.push(block.instructions[idx].clone());
        }
        block.instructions = new_instrs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{IRInstr, IRValue, BinOpKind};

    #[test]
    fn test_schedule_simple() {
        // a = 1 + 2; b = a + 3; c = 4 + 5
        // c is independent of a, b → should be scheduled first (or concurrently)
        let instrs = vec![
            IRInstr::BinOp { op: BinOpKind::Add, dst: IRValue::Register(1), lhs: IRValue::Immediate(1), rhs: IRValue::Immediate(2), ty: None },
            IRInstr::BinOp { op: BinOpKind::Add, dst: IRValue::Register(2), lhs: IRValue::Register(1), rhs: IRValue::Immediate(3), ty: None },
            IRInstr::BinOp { op: BinOpKind::Add, dst: IRValue::Register(3), lhs: IRValue::Immediate(4), rhs: IRValue::Immediate(5), ty: None },
        ];
        let lt = LatencyTable::default_ooo();
        let order = schedule_block(&instrs, &lt);
        // Instruction 2 (c = 4+5) should come before instruction 1 (b = a+3)
        // because c has no dependencies and b depends on a.
        assert_eq!(order.len(), 3);
        // c (idx 2) should be scheduled before b (idx 1)
        let pos_c = order.iter().position(|&x| x == 2).unwrap();
        let pos_b = order.iter().position(|&x| x == 1).unwrap();
        assert!(pos_c < pos_b, "independent instruction should be scheduled before dependent one");
    }

    #[test]
    fn test_schedule_preserves_dependencies() {
        // a = 1 + 2; b = a + 3
        // b must come after a
        let instrs = vec![
            IRInstr::BinOp { op: BinOpKind::Add, dst: IRValue::Register(1), lhs: IRValue::Immediate(1), rhs: IRValue::Immediate(2), ty: None },
            IRInstr::BinOp { op: BinOpKind::Add, dst: IRValue::Register(2), lhs: IRValue::Register(1), rhs: IRValue::Immediate(3), ty: None },
        ];
        let lt = LatencyTable::default_ooo();
        let order = schedule_block(&instrs, &lt);
        assert_eq!(order, vec![0, 1]); // a before b
    }
}
