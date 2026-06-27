//! Compilation speed benchmark: measure parse→SCG→IR→codegen time for
//! programs of varying size.

use super::{BenchmarkResult, measure};
use vuma_codegen::scg_to_ir::{
    Scg, ScgNode, ScgFunction, ScgParam, ScgStatement, ScgType, ScgExpr,
    ComputationNode,
};
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::ScgToIr;

/// Run compilation speed benchmarks at varying program sizes.
pub fn run_benchmarks() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    for &stmt_count in &[10, 50, 100, 500, 1000] {
        let scg = build_program(stmt_count);

        // Measure IR construction time
        let (ir_mean, ir_median) = measure(|| {
            let mut ir_builder = ScgToIr::new();
            let _ = ir_builder.convert(&scg);
        }, 10);

        results.push(
            BenchmarkResult::new(
                format!("compilation_speed/ir_lowering/{}_stmts", stmt_count),
                ir_mean,
                ir_median,
                10,
            )
            .with_extra("statement_count", stmt_count as u64)
        );

        // Measure full pipeline (IR + regalloc + encode for aarch64)
        let (full_mean, full_median) = measure(|| {
            let mut ir_builder = ScgToIr::new();
            if let Ok(ir_program) = ir_builder.convert(&scg) {
                if let Ok(backend) = vuma_codegen::backend::create_backend(BackendKind::AArch64) {
                    for func in &ir_program.functions {
                        let _ = backend.allocate_registers(func);
                    }
                }
            }
        }, 10);

        results.push(
            BenchmarkResult::new(
                format!("compilation_speed/full_pipeline/{}_stmts", stmt_count),
                full_mean,
                full_median,
                10,
            )
            .with_extra("statement_count", stmt_count as u64)
        );
    }

    results
}

/// Build a synthetic SCG program with the given number of statements.
fn build_program(stmt_count: usize) -> Scg {
    let mut body = Vec::new();
    for i in 0..stmt_count {
        body.push(ScgStatement::Computation(ComputationNode {
            dst: format!("x{}", i),
            op: if i % 4 == 0 { BinOpKind::Add } else if i % 4 == 1 { BinOpKind::Sub } else if i % 4 == 2 { BinOpKind::Mul } else { BinOpKind::And },
            lhs: if i == 0 { ScgExpr::Int(i as i64) } else { ScgExpr::Var(format!("x{}", i - 1)) },
            rhs: ScgExpr::Int((i + 1) as i64),
            tail_call: false,
            reassigns: None,
        }));
    }
    body.push(ScgStatement::Return(vec![ScgExpr::Var(format!("x{}", stmt_count - 1))]));

    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![],
            results: vec![ScgType::U32],
            body,
        })],
    }
}

use vuma_codegen::backend::BackendKind;

#[cfg(test)]
mod tests {
    #[test]
    fn test_build_program() {
        let scg = super::build_program(10);
        assert_eq!(scg.nodes.len(), 1);
    }

    #[test]
    fn test_compilation_speed_benchmarks() {
        let results = super::run_benchmarks();
        assert!(!results.is_empty());
        for r in &results {
            assert!(r.name.starts_with("compilation_speed/"));
            assert!(r.mean_ns > 0);
        }
    }
}
