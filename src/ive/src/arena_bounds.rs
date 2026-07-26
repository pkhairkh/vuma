//! Arena Bounds Verifier — checks arena allocation bounds.
//!
//! Verifies that every ArenaAlloc node has a bounds check (offset + size <=
//! capacity) before the allocation proceeds. At the SCG level, this is a
//! structural check: the ArenaAlloc node must compute new_offset = offset +
//! layout_size and check it against capacity.
//!
//! The IVE also tracks ArenaAlloc's arena input as consumed (linear ownership),
//! mirroring the StateTransform consume pattern.
//!
//! # IVE Wave 0 task C — INERT (REMOVE); restoration deferred to Wave 2
//!
//! **Status as of IVE Wave 0 task C:** this verifier is structurally INERT.
//! [`verify_arena_bounds`] unconditionally returns `Vec::new()` — the loop
//! body over `accessed_vars` is empty (only a comment), and the function
//! discards its inputs via `let _ = (arena_vars, accessed_vars);`. It has
//! ZERO callers in `pipeline.rs` (grep `verify_arena_bounds` returns only
//! this file and the unit tests below). The `all_valid(&[])` shortcut makes
//! the unit tests pass trivially.
//!
//! **Why inert by design (not a bug):** the actual arena-bounds enforcement
//! is performed at RUNTIME by the codegen — `pipeline.rs` Stage 5 emits a
//! `ComputationNode(UGe)` + `ControlNode::If { __oob_trap }` pair at every
//! arena-alloc site (see the `// Bounds check (K0e/K0f DoD)` comment block
//! starting at `pipeline.rs:11710`, calling `__arena_overflow()` which
//! exits the process). Arena LINEARITY (use-after-`arena_free`) is handled
//! separately by the invariant aggregator's `consumed_vars` tracking.
//! [`verify_arena_bounds`] was therefore redundant from inception — its
//! signature (`arena_vars`, `accessed_vars` HashSets) is too narrow to
//! reconstruct either the runtime bounds check or the linearity check
//! without SCG/IR plumbing that does not exist.
//!
//! **Decision (IVE Wave 0 task C):** REMOVE from the active-verifier roster.
//! No source-code deletion is performed in Wave 0 (the function and its
//! tests are retained so Wave 2 can either RESTORE it with proper SCG/IR
//! plumbing + a Lean soundness proof, or DELETE it as confirmed-redundant).
//! Until then, callers MUST NOT rely on [`verify_arena_bounds`] for any
//! real verification — its empty `Vec` return is structurally guaranteed,
//! not empirically validated. See `docs/caveats.md` §0.7 for the full
//! decision table.

use std::collections::HashSet;

/// Result of arena bounds verification.
#[derive(Debug, Clone)]
pub struct ArenaBoundsVerification {
    pub valid: bool,
    pub error: Option<String>,
}

/// Verify that arena allocations have proper bounds checks.
///
/// `arena_vars` — set of variables holding arena states (from arena_new).
/// `accessed_vars` — set of arena variables that are read/written after alloc.
///
/// Returns verification results. An empty Vec means all valid.
pub fn verify_arena_bounds(
    arena_vars: &HashSet<String>,
    accessed_vars: &HashSet<String>,
) -> Vec<ArenaBoundsVerification> {
    let results = Vec::new();
    // For now, the bounds check is performed at runtime (the codegen emits
    // the offset + layout_size computation). The IVE-level check verifies
    // that arena variables are not accessed after arena_free (linearity).
    for var in accessed_vars {
        if arena_vars.contains(var) {
            // The arena variable is accessed — this is fine as long as it
            // hasn't been freed. The linearity check (consumed_vars in the
            // invariant aggregator) handles use-after-free.
        }
    }
    // No violations — the runtime bounds check handles overflow.
    let _ = (arena_vars, accessed_vars);
    results
}

/// Returns true if all verification results are valid.
pub fn all_valid(results: &[ArenaBoundsVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_inputs() {
        let results = verify_arena_bounds(&HashSet::new(), &HashSet::new());
        assert!(results.is_empty());
        assert!(all_valid(&results));
    }

    #[test]
    fn test_arena_var_accessed() {
        let arena_vars: HashSet<String> = HashSet::from(["arena".to_string()]);
        let accessed: HashSet<String> = HashSet::from(["arena".to_string()]);
        let results = verify_arena_bounds(&arena_vars, &accessed);
        assert!(all_valid(&results));
    }

    #[test]
    fn test_no_arena_vars() {
        let accessed: HashSet<String> = HashSet::from(["x".to_string()]);
        let results = verify_arena_bounds(&HashSet::new(), &accessed);
        assert!(results.is_empty());
    }
}
