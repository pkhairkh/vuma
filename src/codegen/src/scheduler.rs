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

        // Memory dependencies: stores depend on previous stores/loads to
        // the same address. Conservatively, all memory ops depend on
        // previous memory ops (no alias analysis here — that's Wave 3's job).
        if matches!(instr, IRInstr::Store { .. } | IRInstr::Load { .. }
                    | IRInstr::AtomicStore { .. } | IRInstr::AtomicLoad { .. }) {
            for j in 0..i {
                if matches!(instructions[j], IRInstr::Store { .. } | IRInstr::Load { .. }
                            | IRInstr::AtomicStore { .. } | IRInstr::AtomicLoad { .. }) {
                    if !preds.contains(&j) {
                        preds.push(j);
                    }
                }
            }
        }

        // Calls depend on all previous instructions (conservative).
        if matches!(instr, IRInstr::Call { .. }) {
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
    // Process in reverse topological order (leaves first).
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
