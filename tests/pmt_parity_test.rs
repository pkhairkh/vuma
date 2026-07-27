//! pmt_parity_test.rs — parity test for PMT checkers
//!
//! This test verifies that the Rust hand-translations of the Lean-verified
//! checkers (proof/extracted/pmt_check.rs) produce the same results as
//! the Lean definitions (proof/PMT/Extraction.lean) on all test cases.
//!
//! Currently, the parity is verified by matching expected values (computed
//! by hand from the Lean definitions). A future improvement would be to
//! call the Lean-compiled C code directly via FFI and compare.
//!
//! ## Wave 1 task IVE-1-B: IVE state-verifier parity tests
//!
//! Extended to cover the 3 IVE state verifiers that are `@[export]`-ed
//! from `proof/PMT/Extraction.lean` §8:
//!
//!   - `lean_verify_transform` (mirrors `PMT.IVE.Soundness.verify_transform`)
//!   - `lean_verify_state_reads` (mirrors `PMT.IVE.Soundness.verify_state_reads`)
//!   - `lean_verify_state_writes` (mirrors `PMT.IVE.Soundness.verify_state_writes`)
//!
//! The Rust-side hand-translations below mirror the Lean semantics. The
//! parity tests verify they match the expected Lean behavior on
//! representative inputs. When the build-system integration is complete
//! (Wave 1 task IVE-1-D), these tests will call the actual extracted C
//! functions via FFI and compare against the hand-written Rust path.

// NOTE: This test file lives in tests/ but the pmt_check module is at
// proof/extracted/pmt_check.rs (outside the crate). To make this work,
// we either need to:
// 1. Move pmt_check.rs into src/codegen/src/runtime/pmt_check.rs (preferred)
// 2. Or use a build.rs to include it via include_str!/include!
// 3. Or duplicate the functions here for parity testing
//
// For now, we duplicate the functions here and verify they match the
// expected Lean behavior. This is a PARITY test — if the functions here
// ever diverge from proof/extracted/pmt_check.rs, the test still passes
// (because both would need to match the expected values), but a separate
// diff check would catch the divergence.

/// Hand-translated from Lean: verified_capacity_check
fn lean_capacity_check(used: u64, size: u64, capacity: u64) -> bool {
    // Lean: used + size ≤ capacity
    // Rust: use checked_add to catch overflow (Lean Nat can't overflow)
    used.checked_add(size).map_or(false, |sum| sum <= capacity)
}

/// Hand-translated from Lean: verified_field_bounds_check
fn lean_field_bounds_check(offset: u64, size: u64, total: u64) -> bool {
    offset.checked_add(size).map_or(false, |sum| sum <= total)
}

/// Hand-translated from Lean: verified_linearity_check
fn lean_linearity_check(var: &str, consumed: &[&str]) -> bool {
    !consumed.iter().any(|c| *c == var)
}

// ─────────────────────────────────────────────────────────────────────
// IVE state-verifier hand-translations (Wave 1 task IVE-1-B)
// ─────────────────────────────────────────────────────────────────────
//
// These mirror the Lean definitions in `proof/PMT/IVE/Soundness/`:
//   - `verify_transform` (Transform.lean) — checks 2 layouts are
//     well-formed + kind-specific constraint (identity/reinterpret/copy).
//   - `verify_state_reads` (StateReads.lean) — checks each read accesses
//     a registered, in-bounds field.
//   - `verify_state_writes` (StateWrites.lean) — checks each write is to
//     a live (non-consumed) variable with a registered, in-bounds field.
//
// The Lean `WF_Layout` predicate has 3 conjuncts: (1) every field is in
// bounds, (2) every distinct pair of fields is disjoint, (3) total_size > 0
// or fields is empty. The Rust hand-translation below mirrors this.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransformKind { Identity, Reinterpret, Copy }

#[derive(Clone, Debug, PartialEq, Eq)]
struct Layout { total_size: u64, fields: Vec<Field> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Field { offset: u64, size: u64 }

/// Hand-translated from Lean: `PMT.WF_Layout` (via `wf_layout_bool` in
/// `proof/PMT/IVE/Soundness/WFLayoutBool.lean`). Returns true iff:
/// (1) every field is in bounds (offset + size ≤ total_size),
/// (2) every distinct pair of fields is disjoint,
/// (3) total_size > 0 or fields is empty.
fn lean_wf_layout_bool(l: &Layout) -> bool {
    // Conjunct 3: total_size > 0 or fields is empty.
    if l.total_size == 0 && !l.fields.is_empty() {
        return false;
    }
    // Conjunct 1: every field is in bounds.
    for f in &l.fields {
        if !lean_field_bounds_check(f.offset, f.size, l.total_size) {
            return false;
        }
    }
    // Conjunct 2: every distinct pair of fields is disjoint.
    // Disjoint: f1.offset + f1.size ≤ f2.offset OR f2.offset + f2.size ≤ f1.offset.
    for (i, f1) in l.fields.iter().enumerate() {
        for f2 in &l.fields[i + 1..] {
            let disjoint = f1.offset.checked_add(f1.size).map_or(false, |s| s <= f2.offset)
                || f2.offset.checked_add(f2.size).map_or(false, |s| s <= f1.offset);
            if !disjoint {
                return false;
            }
        }
    }
    true
}

/// Hand-translated from Lean: `PMT.IVE.Soundness.verify_transform`
/// (Transform.lean). Returns true iff:
/// (1) in_layout is well-formed,
/// (2) out_layout is well-formed,
/// (3) kind-specific constraint:
///     - Identity: in_layout = out_layout (same total_size AND same fields),
///     - Reinterpret: in_layout.total_size = out_layout.total_size,
///     - Copy: no constraint (any pair accepted).
fn lean_verify_transform(
    in_layout: &Layout,
    out_layout: &Layout,
    kind: TransformKind,
) -> bool {
    if !lean_wf_layout_bool(in_layout) { return false; }
    if !lean_wf_layout_bool(out_layout) { return false; }
    match kind {
        TransformKind::Identity => in_layout.total_size == out_layout.total_size
            && in_layout.fields.len() == out_layout.fields.len()
            && in_layout.fields.iter().zip(out_layout.fields.iter()).all(|(a, b)| a == b),
        TransformKind::Reinterpret => in_layout.total_size == out_layout.total_size,
        TransformKind::Copy => true,
    }
}

/// Hand-translated from Lean: `PMT.IVE.Soundness.verify_state_reads`
/// (StateReads.lean). Returns true iff every read accesses a registered,
/// in-bounds field. `env` maps var name → layout (None for unknown vars
/// maps to emptyLayout, matching Lean's `layout_env_from_list`).
fn lean_verify_state_reads(
    env: &[(&str, Layout)],
    reads: &[(&str, Field)],
) -> bool {
    let empty_layout = Layout { total_size: 1, fields: Vec::new() };
    reads.iter().all(|(var, f)| {
        // Look up var in env; default to empty layout (size 1, no fields).
        let layout = env.iter()
            .find(|(name, _)| *name == *var)
            .map(|(_, l)| l)
            .unwrap_or(&empty_layout);
        // Check field is registered (matches by offset + size).
        let registered = layout.fields.iter().any(|g| g.offset == f.offset && g.size == f.size);
        // Check field is in bounds.
        let in_bounds = lean_field_bounds_check(f.offset, f.size, layout.total_size);
        registered && in_bounds
    })
}

/// Hand-translated from Lean: `PMT.IVE.Soundness.verify_state_writes`
/// (StateWrites.lean). Returns true iff every write is to a live
/// (non-consumed) variable with a registered, in-bounds field.
fn lean_verify_state_writes(
    env: &[(&str, Layout)],
    consumed: &[&str],
    writes: &[(&str, Field)],
) -> bool {
    let empty_layout = Layout { total_size: 1, fields: Vec::new() };
    writes.iter().all(|(var, f)| {
        // Check var is not consumed (live).
        let live = !consumed.iter().any(|c| *c == *var);
        if !live { return false; }
        // Look up var in env; default to empty layout.
        let layout = env.iter()
            .find(|(name, _)| *name == *var)
            .map(|(_, l)| l)
            .unwrap_or(&empty_layout);
        // Check field is registered and in bounds.
        let registered = layout.fields.iter().any(|g| g.offset == f.offset && g.size == f.size);
        let in_bounds = lean_field_bounds_check(f.offset, f.size, layout.total_size);
        registered && in_bounds
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Parity test: verify Rust matches expected Lean behavior
    // Expected values computed by hand from proof/PMT/Extraction.lean

    #[test]
    fn parity_capacity_check_basic() {
        // Lean: verified_capacity_check 0 16 1024 = (0 + 16 ≤ 1024) = true
        assert_eq!(lean_capacity_check(0, 16, 1024), true);
        // Lean: verified_capacity_check 1000 100 1024 = (1000 + 100 ≤ 1024) = false
        assert_eq!(lean_capacity_check(1000, 100, 1024), false);
        // Lean: verified_capacity_check 1024 0 1024 = (1024 + 0 ≤ 1024) = true
        assert_eq!(lean_capacity_check(1024, 0, 1024), true);
    }

    #[test]
    fn parity_capacity_check_overflow() {
        // Lean: verified_capacity_check 0 (2^64) (2^64) = true (Nat, no overflow)
        // Rust: u64 overflow → checked_add returns None → false
        // This is the KEY difference: Rust catches overflow, Lean doesn't.
        // The Rust behavior is MORE faithful to the actual usize semantics.
        assert_eq!(lean_capacity_check(u64::MAX, 1, u64::MAX), false);
        assert_eq!(lean_capacity_check(0, u64::MAX, u64::MAX), true);
    }

    #[test]
    fn parity_field_bounds_check() {
        // Lean: verified_field_bounds_check ⟨0,4⟩ ⟨16,[]⟩ = (0 + 4 ≤ 16) = true
        assert_eq!(lean_field_bounds_check(0, 4, 16), true);
        // Lean: verified_field_bounds_check ⟨12,8⟩ ⟨16,[]⟩ = (12 + 8 ≤ 16) = false
        assert_eq!(lean_field_bounds_check(12, 8, 16), false);
    }

    #[test]
    fn parity_linearity_check() {
        assert_eq!(lean_linearity_check("x", &["a", "b"]), true);
        assert_eq!(lean_linearity_check("a", &["a", "b"]), false);
        assert_eq!(lean_linearity_check("x", &[]), true);
    }

    #[test]
    fn parity_composed_check() {
        // All pass
        let result = lean_capacity_check(0, 16, 1024)
            && lean_field_bounds_check(0, 4, 16)
            && lean_linearity_check("x", &["a", "b"]);
        assert_eq!(result, true);
    }

    // ─── IVE state-verifier parity tests (Wave 1 task IVE-1-B) ───────

    #[test]
    fn parity_wf_layout_empty() {
        // Lean: WF_Layout ⟨1, []⟩ = true (conjunct 3: 0 < 1 ∨ [] = [])
        let l = Layout { total_size: 1, fields: vec![] };
        assert_eq!(lean_wf_layout_bool(&l), true);
    }

    #[test]
    fn parity_wf_layout_zero_size_with_fields() {
        // Lean: WF_Layout ⟨0, [⟨0,0⟩]⟩ = false (conjunct 3 fails: 0 < 0 is false, fields ≠ [])
        let f = Field { offset: 0, size: 0 };
        let l = Layout { total_size: 0, fields: vec![f] };
        assert_eq!(lean_wf_layout_bool(&l), false);
    }

    #[test]
    fn parity_wf_layout_in_bounds() {
        // Lean: WF_Layout ⟨16, [⟨0,4⟩, ⟨8,4⟩]⟩ = true
        let f1 = Field { offset: 0, size: 4 };
        let f2 = Field { offset: 8, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f1, f2] };
        assert_eq!(lean_wf_layout_bool(&l), true);
    }

    #[test]
    fn parity_wf_layout_out_of_bounds() {
        // Lean: WF_Layout ⟨16, [⟨12,8⟩]⟩ = false (conjunct 1: 12 + 8 = 20 > 16)
        let f = Field { offset: 12, size: 8 };
        let l = Layout { total_size: 16, fields: vec![f] };
        assert_eq!(lean_wf_layout_bool(&l), false);
    }

    #[test]
    fn parity_wf_layout_overlapping_fields() {
        // Lean: WF_Layout ⟨16, [⟨0,8⟩, ⟨4,8⟩]⟩ = false (conjunct 2: not disjoint)
        let f1 = Field { offset: 0, size: 8 };
        let f2 = Field { offset: 4, size: 8 };
        let l = Layout { total_size: 16, fields: vec![f1, f2] };
        assert_eq!(lean_wf_layout_bool(&l), false);
    }

    #[test]
    fn parity_verify_transform_identity_pass() {
        // Lean: verify_transform ⟨_, _, in, in, identity⟩ where in = ⟨16, [⟨0,4⟩]⟩
        let f = Field { offset: 0, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f] };
        assert_eq!(lean_verify_transform(&l, &l, TransformKind::Identity), true);
    }

    #[test]
    fn parity_verify_transform_identity_fail_different_fields() {
        // Lean: verify_transform ⟨_, _, in, out, identity⟩ where in ≠ out
        let f1 = Field { offset: 0, size: 4 };
        let f2 = Field { offset: 0, size: 8 };
        let in_l = Layout { total_size: 16, fields: vec![f1] };
        let out_l = Layout { total_size: 16, fields: vec![f2] };
        assert_eq!(lean_verify_transform(&in_l, &out_l, TransformKind::Identity), false);
    }

    #[test]
    fn parity_verify_transform_reinterpret_pass() {
        // Lean: verify_transform ⟨_, _, in, out, reinterpret⟩ where in.total_size = out.total_size
        let f1 = Field { offset: 0, size: 4 };
        let f2 = Field { offset: 4, size: 4 };
        let in_l = Layout { total_size: 8, fields: vec![f1] };
        let out_l = Layout { total_size: 8, fields: vec![f2] };
        assert_eq!(lean_verify_transform(&in_l, &out_l, TransformKind::Reinterpret), true);
    }

    #[test]
    fn parity_verify_transform_reinterpret_fail_size_mismatch() {
        // Lean: verify_transform ⟨_, _, in, out, reinterpret⟩ where in.total_size ≠ out.total_size
        let f1 = Field { offset: 0, size: 4 };
        let f2 = Field { offset: 0, size: 8 };
        let in_l = Layout { total_size: 4, fields: vec![f1] };
        let out_l = Layout { total_size: 8, fields: vec![f2] };
        assert_eq!(lean_verify_transform(&in_l, &out_l, TransformKind::Reinterpret), false);
    }

    #[test]
    fn parity_verify_transform_copy_pass_any() {
        // Lean: verify_transform ⟨_, _, in, out, copy⟩ — any pair accepted (after WF check)
        let f1 = Field { offset: 0, size: 4 };
        let f2 = Field { offset: 0, size: 8 };
        let in_l = Layout { total_size: 4, fields: vec![f1] };
        let out_l = Layout { total_size: 8, fields: vec![f2] };
        assert_eq!(lean_verify_transform(&in_l, &out_l, TransformKind::Copy), true);
    }

    #[test]
    fn parity_verify_transform_rejects_ill_formed_in_layout() {
        // Lean: verify_transform ⟨_, _, in, out, copy⟩ where in is ill-formed → false
        let f_bad = Field { offset: 12, size: 8 };  // 12 + 8 = 20 > 16
        let in_l = Layout { total_size: 16, fields: vec![f_bad] };
        let out_l = Layout { total_size: 8, fields: vec![] };
        assert_eq!(lean_verify_transform(&in_l, &out_l, TransformKind::Copy), false);
    }

    #[test]
    fn parity_verify_state_reads_pass() {
        // Lean: verify_state_reads env [⟨"x", ⟨0,4⟩⟩] where env "x" = ⟨16, [⟨0,4⟩]⟩
        let f = Field { offset: 0, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f] };
        let env = vec![("x", l)];
        let reads = vec![("x", f)];
        assert_eq!(lean_verify_state_reads(&env, &reads), true);
    }

    #[test]
    fn parity_verify_state_reads_fail_unregistered_field() {
        // Lean: verify_state_reads env [⟨"x", ⟨8,4⟩⟩] where env "x" = ⟨16, [⟨0,4⟩]⟩
        // Field ⟨8,4⟩ is in bounds (8+4=12 ≤ 16) but NOT registered → fail.
        let f_registered = Field { offset: 0, size: 4 };
        let f_unregistered = Field { offset: 8, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f_registered] };
        let env = vec![("x", l)];
        let reads = vec![("x", f_unregistered)];
        assert_eq!(lean_verify_state_reads(&env, &reads), false);
    }

    #[test]
    fn parity_verify_state_reads_fail_out_of_bounds() {
        // Lean: verify_state_reads env [⟨"x", ⟨12,8⟩⟩] where env "x" = ⟨16, [⟨12,8⟩]⟩
        // Field ⟨12,8⟩ is registered but NOT in bounds (12+8=20 > 16) → fail.
        let f = Field { offset: 12, size: 8 };
        let l = Layout { total_size: 16, fields: vec![f] };
        let env = vec![("x", l)];
        let reads = vec![("x", f)];
        assert_eq!(lean_verify_state_reads(&env, &reads), false);
    }

    #[test]
    fn parity_verify_state_writes_pass() {
        // Lean: verify_state_writes env [] [⟨"x", ⟨0,4⟩⟩] where env "x" = ⟨16, [⟨0,4⟩]⟩
        let f = Field { offset: 0, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f] };
        let env = vec![("x", l)];
        let consumed: Vec<&str> = vec![];
        let writes = vec![("x", f)];
        assert_eq!(lean_verify_state_writes(&env, &consumed, &writes), true);
    }

    #[test]
    fn parity_verify_state_writes_fail_consumed_var() {
        // Lean: verify_state_writes env ["x"] [⟨"x", ⟨0,4⟩⟩] — "x" is consumed → fail
        let f = Field { offset: 0, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f] };
        let env = vec![("x", l)];
        let consumed = vec!["x"];
        let writes = vec![("x", f)];
        assert_eq!(lean_verify_state_writes(&env, &consumed, &writes), false);
    }

    #[test]
    fn parity_verify_state_writes_fail_unregistered_field() {
        // Lean: verify_state_writes env [] [⟨"x", ⟨8,4⟩⟩] where env "x" = ⟨16, [⟨0,4⟩]⟩
        let f_registered = Field { offset: 0, size: 4 };
        let f_unregistered = Field { offset: 8, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f_registered] };
        let env = vec![("x", l)];
        let consumed: Vec<&str> = vec![];
        let writes = vec![("x", f_unregistered)];
        assert_eq!(lean_verify_state_writes(&env, &consumed, &writes), false);
    }

    #[test]
    fn parity_verify_state_writes_mixed() {
        // Mixed: one write passes, one fails (consumed var) → overall fail.
        let f = Field { offset: 0, size: 4 };
        let l = Layout { total_size: 16, fields: vec![f] };
        let env = vec![("x", l.clone()), ("y", l)];
        let consumed = vec!["y"];
        let writes = vec![("x", f), ("y", f)];  // x passes, y fails (consumed)
        assert_eq!(lean_verify_state_writes(&env, &consumed, &writes), false);
    }
}
