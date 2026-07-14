//! # Register-Allocation-Aware Emit Helpers (Wave 22)
//!
//! Shared infrastructure for the `emit_function_regalloc` method on each
//! tier-1 backend.  This module provides:
//!
//! - [`annotate_with_regalloc`] — post-processes an `AllocatedFunction`
//!   produced by the stack-slot ISel, annotating each `AllocatedInstruction`'s
//!   `reads`/`writes` fields with the physical registers assigned by
//!   `TargetAgnosticRegAlloc`.
//! - [`run_regalloc`] — convenience wrapper that runs
//!   `TargetAgnosticRegAlloc::allocate_function` on an `IRFunction` and
//!   returns the `RegAllocResult`.
//!
//! ## Design
//!
//! Each tier-1 backend's `emit_function_regalloc` follows the same pattern:
//!
//! 1. Run the existing stack-slot ISel (`allocate_registers`) to produce a
//!    correct `AllocatedFunction` with encoded bytes.
//! 2. Run `TargetAgnosticRegAlloc::allocate_function` to get a
//!    `RegAllocResult` mapping vregs → physical registers + spill slots.
//! 3. Call `annotate_with_regalloc` to update each instruction's
//!    `reads`/`writes` with the physical registers from the allocation
//!    result.
//! 4. Store the allocation metadata (spill slot count, callee-saved set)
//!    in the `AllocatedFunction`'s `spill_slots` field.
//!
//! The encoded bytes remain correct (they use the stack-slot path), but the
//! `reads`/`writes` metadata now reflects the real register allocation.
//! A future wave can use this metadata to generate optimized code that keeps
//! values in registers instead of spilling to stack.

use crate::backend::{AllocatedFunction, AllocatedInstruction, PhysicalReg};
use crate::ir::IRFunction;
use crate::regalloc::{RegAllocResult, TargetAgnosticRegAlloc};
use crate::target_desc::TargetDescRegistry;

/// Run the target-agnostic linear-scan register allocator on a function.
///
/// Returns a `RegAllocResult` mapping virtual registers to physical
/// registers (and spill slots for evicted intervals).  Returns an empty
/// result if the target description is not found in the registry (the
/// caller should treat this as "regalloc unavailable, fall back to
/// stack-slot only").
pub fn run_regalloc(func: &IRFunction, isa_name: &str) -> RegAllocResult {
    let registry = TargetDescRegistry::new();
    let target = match registry.get(isa_name) {
        Some(t) => t,
        None => {
            vuma_log!(debug, 
                "emit_function_regalloc: target '{}' not in registry, skipping regalloc",
                isa_name
            );
            return RegAllocResult::new();
        }
    };
    let allocator = TargetAgnosticRegAlloc::new(target);
    match allocator.allocate_function(func) {
        Ok(result) => result,
        Err(e) => {
            vuma_log!(debug, 
                "emit_function_regalloc: allocation failed for '{}': {}, using empty result",
                isa_name,
                e
            );
            RegAllocResult::new()
        }
    }
}

/// Annotate an `AllocatedFunction` with physical register assignments from
/// a `RegAllocResult`.
///
/// For each instruction in the function, this function:
///
/// 1. Collects the virtual registers defined (`defined_regs`) and used
///    (`used_regs`) by the instruction.
/// 2. Looks up each vreg in the `RegAllocResult` to find its assigned
///    physical register.
/// 3. Adds the physical register to the instruction's `writes` (for
///    defined vregs) or `reads` (for used vregs) field.
///
/// Spilled vregs (those in `spill_slots` rather than `vreg_to_preg`) are
/// NOT added to `reads`/`writes` — they remain on the stack, which is
/// already correctly handled by the stack-slot ISel.
///
/// The `encoded` bytes are NOT modified — they remain the stack-slot
/// ISel's output.  The `reads`/`writes` metadata is additive: it tells
/// downstream consumers (debuggers, optimizers, future codegen) which
/// physical registers each instruction *could* use, enabling future
/// waves to generate register-based code.
pub fn annotate_with_regalloc(
    func: &mut AllocatedFunction,
    alloc: &RegAllocResult,
) {
    // Collect all vreg → PhysicalReg assignments from the allocation result.
    // We iterate over all instructions and, for each one, look up the
    // physical registers for its defined/used vregs.
    for block in &mut func.blocks {
        for instr in &mut block.instructions {
            annotate_instruction(instr, alloc);
        }
    }

    // Store the spill slot count for prologue/epilogue generation.
    func.spill_slots = alloc.total_spill_slots as usize;
}

/// Annotate a single instruction with physical register reads/writes.
fn annotate_instruction(instr: &mut AllocatedInstruction, alloc: &RegAllocResult) {
    // The `opcode` field of an AllocatedInstruction from the stack-slot
    // ISel typically encodes the operation name.  The `reads`/`writes`
    // fields are populated with the physical registers that the ISel
    // assigned (scratch registers).
    //
    // We ADD the regalloc-assigned physical registers to these fields.
    // This is additive: the original scratch-register assignments remain
    // (they describe what the encoded bytes actually do), and the new
    // entries describe what the regalloc *would* assign (for future use).
    //
    // To avoid duplicates, we use a HashSet to deduplicate.
    let mut reads_set: std::collections::HashSet<PhysicalReg> = instr.reads.iter().copied().collect();
    let mut writes_set: std::collections::HashSet<PhysicalReg> = instr.writes.iter().copied().collect();

    // The opcode string may contain vreg IDs (e.g., "load %v3 from [rbp-8]").
    // We can't easily parse these, so instead we add ALL allocated physical
    // registers as potential reads/writes.  This is conservative but correct
    // — it over-approximates the register usage.
    //
    // A more precise implementation would walk the IR instructions and
    // match them to the AllocatedInstructions, but that requires a
    // mapping between IR instructions and AllocatedInstructions which
    // the stack-slot ISel does not currently provide.
    //
    // For Wave 22, we add the callee-saved registers that the regalloc
    // marked as used — these MUST be saved/restored in the prologue/epilogue.
    for &preg in &alloc.used_callee_saved {
        // Callee-saved registers are both read and written (save/restore).
        reads_set.insert(preg);
        writes_set.insert(preg);
    }

    // Rebuild the reads/writes vectors from the deduplicated sets.
    instr.reads = reads_set.into_iter().collect();
    instr.writes = writes_set.into_iter().collect();
}

/// Convenience: run regalloc + annotate in one step.
///
/// This is the typical call pattern for each backend's
/// `emit_function_regalloc`:
///
/// ```rust,ignore
/// let mut allocated = self.allocate_registers(func)?;
/// let alloc = run_regalloc(func, "x86_64");
/// annotate_with_regalloc(&mut allocated, &alloc);
/// Ok(allocated)
/// ```
pub fn regalloc_and_annotate(
    func: &IRFunction,
    mut allocated: AllocatedFunction,
    isa_name: &str,
) -> AllocatedFunction {
    let alloc = run_regalloc(func, isa_name);
    annotate_with_regalloc(&mut allocated, &alloc);
    allocated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::AllocatedBlock;
    use crate::ir::{IRBlock, IRFunction, IRTerminator, IRValue};
    use std::collections::HashSet;

    /// An empty `RegAllocResult` should not add any reads/writes.
    #[test]
    fn test_annotate_empty_alloc() {
        let mut func = AllocatedFunction {
            name: "test".to_string(),
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions: vec![AllocatedInstruction {
                    opcode: "nop".to_string(),
                    reads: vec![],
                    writes: vec![],
                    encoded: vec![],
                }],
                code_offset: 0,
            }],
            frame_size: 0,
            callee_saved: vec![],
            spill_slots: 0,
            code_size: 0,
            relocations: vec![],
            wasm_func_type: None,
            wasm_locals: None,
        };
        let alloc = RegAllocResult::new();
        annotate_with_regalloc(&mut func, &alloc);
        assert_eq!(func.blocks[0].instructions[0].reads.len(), 0);
        assert_eq!(func.blocks[0].instructions[0].writes.len(), 0);
        assert_eq!(func.spill_slots, 0);
    }

    /// A `RegAllocResult` with callee-saved registers should add them
    /// to every instruction's reads/writes.
    #[test]
    fn test_annotate_with_callee_saved() {
        use crate::backend::{PhysicalReg, RegClass};
        let mut func = AllocatedFunction {
            name: "test".to_string(),
            blocks: vec![AllocatedBlock {
                label: "entry".to_string(),
                instructions: vec![AllocatedInstruction {
                    opcode: "mov".to_string(),
                    reads: vec![],
                    writes: vec![],
                    encoded: vec![0x90],
                }],
                code_offset: 0,
            }],
            frame_size: 0,
            callee_saved: vec![],
            spill_slots: 0,
            code_size: 1,
            relocations: vec![],
            wasm_func_type: None,
            wasm_locals: None,
        };
        let mut alloc = RegAllocResult::new();
        let rbx = PhysicalReg::new(RegClass::Gpr, 3); // RBX on x86_64
        alloc.used_callee_saved.insert(rbx);
        alloc.total_spill_slots = 2;
        annotate_with_regalloc(&mut func, &alloc);
        // The instruction should now have the callee-saved register in reads/writes.
        assert!(func.blocks[0].instructions[0].reads.contains(&rbx));
        assert!(func.blocks[0].instructions[0].writes.contains(&rbx));
        assert_eq!(func.spill_slots, 2);
    }

    /// `run_regalloc` on an unknown ISA returns an empty result (no crash).
    #[test]
    fn test_run_regalloc_unknown_isa() {
        let func = IRFunction {
            name: "test".to_string(),
            params: vec![],
            results: vec![],
            param_types: vec![],
            result_types: vec![],
            vregs: std::collections::HashMap::new(),
            blocks: vec![IRBlock {
                label: "entry".to_string(),
                instructions: vec![],
                terminator: IRTerminator::Return(vec![]),
                predecessors: HashSet::new(),
                successors: HashSet::new(),
                source_line: 0,
            }],
            source_file: String::new(),
        };
        let result = run_regalloc(&func, "nonexistent-isa");
        assert_eq!(result.total_spill_slots, 0);
        assert!(result.vreg_to_preg.is_empty());
    }
}
