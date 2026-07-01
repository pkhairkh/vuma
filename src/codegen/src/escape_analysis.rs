//! Escape Analysis for Stack Allocation
//!
//! Determines which allocations don't escape their function and can be
//! stack-allocated instead of heap-allocated.
//!
//! # Algorithm
//!
//! 1. For each Alloc instruction, track the resulting vreg.
//! 2. An allocation ESCAPES if:
//!    - It's returned from the function (Ret with the vreg)
//!    - It's stored to memory (Store with the vreg as value)
//!    - It's passed to a Call as an argument (except to free())
//!    - It's used in a Phi that could propagate to an escape
//! 3. Non-escaping allocations are marked for stack allocation.

use std::collections::{HashMap, HashSet};
use crate::ir::{IRFunction, IRInstr, IRValue, IRTerminator};

/// Result of escape analysis for a single allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscapeResult {
    /// Allocation does not escape — can be stack-allocated.
    DoesNotEscape,
    /// Allocation escapes — must be heap-allocated.
    Escapes,
}

/// Analyze a function for escaping allocations.
///
/// Returns a map from vreg (allocation result) to escape result.
pub fn analyze_escapes(func: &IRFunction) -> HashMap<u32, EscapeResult> {
    let mut allocs: HashSet<u32> = HashSet::new();
    let mut escapes: HashSet<u32> = HashSet::new();

    // Phase 1: Find all allocations
    for block in &func.blocks {
        for instr in &block.instructions {
            if let IRInstr::Alloc { dst, .. } = instr {
                if let Some(vreg) = dst.as_register() {
                    allocs.insert(vreg);
                }
            }
        }
    }

    // Phase 2: Find escape points
    for block in &func.blocks {
        for instr in &block.instructions {
            match instr {
                // Store: if value is an allocation, it escapes
                IRInstr::Store { value, .. } => {
                    if let IRValue::Register(vreg) = value {
                        if allocs.contains(vreg) {
                            escapes.insert(*vreg);
                        }
                    }
                }

                // Call: if any argument is an allocation, it escapes
                // (unless the call is to free())
                IRInstr::Call { args, func: fname, .. } => {
                    if fname != "__vuma_free" && fname != "free" {
                        for arg in args {
                            if let IRValue::Register(vreg) = arg {
                                if allocs.contains(vreg) {
                                    escapes.insert(*vreg);
                                }
                            }
                        }
                    }
                }

                // Phi: if any incoming is an escaping allocation,
                // mark the phi result as escaping
                IRInstr::Phi { dst, incoming } => {
                    if let Some(phi_vreg) = dst.as_register() {
                        for (val, _) in incoming {
                            if let IRValue::Register(src_vreg) = val {
                                if escapes.contains(src_vreg) {
                                    escapes.insert(phi_vreg);
                                }
                            }
                        }
                    }
                }

                _ => {}
            }
        }

        // Check terminator for return
        match &block.terminator {
            IRTerminator::Return(vals) => {
                for val in vals {
                    if let IRValue::Register(vreg) = val {
                        if allocs.contains(vreg) {
                            escapes.insert(*vreg);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Phase 3: Build result map
    let mut result = HashMap::new();
    for &alloc_vreg in &allocs {
        let escape = if escapes.contains(&alloc_vreg) {
            EscapeResult::Escapes
        } else {
            EscapeResult::DoesNotEscape
        };
        result.insert(alloc_vreg, escape);
    }

    result
}

/// Count how many allocations can be stack-allocated.
pub fn count_stack_allocatable(func: &IRFunction) -> (usize, usize) {
    let results = analyze_escapes(func);
    let total = results.len();
    let stack = results
        .values()
        .filter(|r| **r == EscapeResult::DoesNotEscape)
        .count();
    (stack, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_function() {
        let func = IRFunction::new("test".to_string());
        let (stack, total) = count_stack_allocatable(&func);
        assert_eq!(stack, 0);
        assert_eq!(total, 0);
    }
}
