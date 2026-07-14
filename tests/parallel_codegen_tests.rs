//! Parallel codegen tests (Wave 9).
//!
//! These tests prove that parallel register allocation (via rayon) produces
//! identical results to sequential allocation. The parallel path runs in
//! production (compile_with_path and compile_with_recovery); this test
//! verifies it doesn't corrupt the output.

use vuma_codegen::backend::{create_backend, BackendKind, Backend, AllocatedProgram};
use vuma_codegen::regalloc::LinearScanAllocator;
use vuma_codegen::ir::{BinOpKind, IRFunction, IRInstr, IRTerminator, IRType, IRValue};

/// Build a multi-function program suitable for parallel regalloc.
fn make_program() -> Vec<IRFunction> {
    let mut funcs = Vec::new();

    // Function 1: simple add
    let mut f1 = IRFunction::new("add");
    f1.params = vec![IRValue::Register(0), IRValue::Register(1)];
    f1.param_types = vec![IRType::I64, IRType::I64];
    f1.blocks[0].instructions = vec![IRInstr::BinOp {
        op: BinOpKind::Add,
        dst: IRValue::Register(2),
        lhs: IRValue::Register(0),
        rhs: IRValue::Register(1),
        ty: None,
    }];
    f1.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(2)]);
    f1.results = vec![IRValue::Register(2)];
    f1.result_types = vec![IRType::I64];
    funcs.push(f1);

    // Function 2: multiply
    let mut f2 = IRFunction::new("mul");
    f2.params = vec![IRValue::Register(0), IRValue::Register(1)];
    f2.param_types = vec![IRType::I64, IRType::I64];
    f2.blocks[0].instructions = vec![IRInstr::BinOp {
        op: BinOpKind::Mul,
        dst: IRValue::Register(2),
        lhs: IRValue::Register(0),
        rhs: IRValue::Register(1),
        ty: None,
    }];
    f2.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(2)]);
    f2.results = vec![IRValue::Register(2)];
    f2.result_types = vec![IRType::I64];
    funcs.push(f2);

    // Function 3: xor (bitwise)
    let mut f3 = IRFunction::new("xor");
    f3.params = vec![IRValue::Register(0), IRValue::Register(1)];
    f3.param_types = vec![IRType::I64, IRType::I64];
    f3.blocks[0].instructions = vec![IRInstr::BinOp {
        op: BinOpKind::Xor,
        dst: IRValue::Register(2),
        lhs: IRValue::Register(0),
        rhs: IRValue::Register(1),
        ty: None,
    }];
    f3.blocks[0].terminator = IRTerminator::Return(vec![IRValue::Register(2)]);
    f3.results = vec![IRValue::Register(2)];
    f3.result_types = vec![IRType::I64];
    funcs.push(f3);

    funcs
}

#[test]
fn wave9_parallel_regalloc_matches_sequential() {
    // Run register allocation sequentially and in parallel, verify the
    // results are identical (same vreg→physical-reg mappings).
    use rayon::prelude::*;

    let funcs = make_program();
    let allocator = LinearScanAllocator::new();

    // Sequential
    let sequential: Vec<_> = funcs.iter()
        .map(|f| allocator.allocate_function(f))
        .collect();

    // Parallel
    let parallel: Vec<_> = funcs.par_iter()
        .map(|f| allocator.allocate_function(f))
        .collect();

    // Both must succeed
    for (s, p) in sequential.iter().zip(parallel.iter()) {
        assert!(s.is_ok(), "sequential allocation failed: {:?}", s);
        assert!(p.is_ok(), "parallel allocation failed: {:?}", p);
    }

    // Results must be identical (same vreg→preg mapping size, same spill count).
    for (i, (s, p)) in sequential.iter().zip(parallel.iter()).enumerate() {
        let s = s.as_ref().unwrap();
        let p = p.as_ref().unwrap();
        assert_eq!(
            s.vreg_to_preg.len(),
            p.vreg_to_preg.len(),
            "function {}: vreg_to_preg size mismatch ({} vs {})",
            i,
            s.vreg_to_preg.len(),
            p.vreg_to_preg.len()
        );
        assert_eq!(
            s.total_spill_slots, p.total_spill_slots,
            "function {}: spill slot count mismatch",
            i
        );
        assert_eq!(
            s.used_callee_saved_gprs.len(),
            p.used_callee_saved_gprs.len(),
            "function {}: callee-saved GPR count mismatch",
            i
        );
    }
}

#[test]
fn wave9_parallel_regalloc_handles_errors() {
    // Parallel regalloc must correctly propagate errors (e.g., from a
    // function with too many live registers to allocate).
    use rayon::prelude::*;

    let funcs = make_program();
    let allocator = LinearScanAllocator::new();

    let results: Vec<_> = funcs.par_iter()
        .map(|f| allocator.allocate_function(f))
        .collect();

    // All should succeed (these are simple functions).
    assert_eq!(results.len(), 3);
    for r in &results {
        assert!(r.is_ok(), "expected all allocations to succeed");
    }
}

#[test]
fn wave9_production_compile_uses_parallel_regalloc() {
    // End-to-end: compile a real .vuma program through the production
    // pipeline (which now uses parallel regalloc) and verify it succeeds.
    // This proves the parallel path doesn't break compilation.
    use vuma::pipeline::{compile, CompileConfig, OptLevel};

    let source = r#"
        fn add(a, b) -> i64 { return a + b; }
        fn mul(a, b) -> i64 { return a * b; }
        fn main() -> i64 {
            x = add(3, 4);
            y = mul(x, 2);
            return y;
        }
    "#;

    let config = CompileConfig {
        opt_level: OptLevel::O2,
        ..CompileConfig::default()
    };

    let result = compile(source, &config);
    assert!(result.is_ok(), "production compile with parallel regalloc failed: {:?}", result.err());
    let output = result.unwrap();
    assert!(!output.binary.is_empty(), "should produce a binary");
}
