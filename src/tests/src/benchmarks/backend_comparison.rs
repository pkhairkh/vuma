//! Backend comparison benchmark: compile the same program and measure
//! binary sizes across all 10 backends.

use super::{BenchmarkResult, measure};
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::{
    Scg, ScgNode, ScgFunction, ScgStatement, ScgType, ScgExpr,
    ComputationNode, AllocationNode, AccessNode, ControlNode,
};
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::ScgToIr;

/// Run backend comparison benchmarks.
///
/// Compiles a representative reference program through all 10 backends
/// and reports binary size, IR instruction count, and compilation time.
pub fn run_benchmarks() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();
    let scg = build_reference_program();

    let backends: [(BackendKind, &str); 10] = [
        (BackendKind::AArch64, "aarch64"),
        (BackendKind::X86_64, "x86_64"),
        (BackendKind::RiscV64, "riscv64"),
        (BackendKind::Arm32, "arm32"),
        (BackendKind::Mips64, "mips64"),
        (BackendKind::PowerPC64, "ppc64"),
        (BackendKind::LoongArch64, "loongarch64"),
        (BackendKind::Wasm32, "wasm32"),
        (BackendKind::X86_32, "x86_32"),
        (BackendKind::RiscV32, "riscv32"),
    ];

    for (kind, name) in &backends {
        if let Some(result) = benchmark_backend(&scg, *kind, name) {
            results.push(result);
        }
    }

    results
}

fn benchmark_backend(scg: &Scg, kind: BackendKind, name: &str) -> Option<BenchmarkResult> {
    let (mean_ns, median_ns) = measure(|| {
        let mut ir_builder = ScgToIr::new();
        if let Ok(ir_program) = ir_builder.convert(scg) {
            if let Ok(backend) = create_backend(kind) {
                for func in &ir_program.functions {
                    let _ = backend.allocate_registers(func);
                }
            }
        }
    }, 10);

    // Get binary size
    let mut ir_builder = ScgToIr::new();
    let ir_program = ir_builder.convert(scg).ok()?;
    let ir_instr_count: usize = ir_program.functions.iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();

    let backend = create_backend(kind).ok()?;
    let mut allocated = Vec::new();
    for func in &ir_program.functions {
        if let Ok(a) = backend.allocate_registers(func) {
            allocated.push(a);
        }
    }

    let binary_size = if !allocated.is_empty() {
        let prog = AllocatedProgram {
            functions: allocated,
            total_code_size: 0,
            total_data_size: 0,
        };
        backend.encode_program(&prog).map(|b| b.len()).unwrap_or(0)
    } else {
        0
    };

    Some(
        BenchmarkResult::new(format!("backend_comparison/{}", name), mean_ns, median_ns, 10)
            .with_extra("binary_size", binary_size as u64)
            .with_extra("ir_instructions", ir_instr_count as u64)
            .with_extra("isa", name)
    )
}

/// Build a reference program for consistent backend comparison.
fn build_reference_program() -> Scg {
    let mut body = Vec::new();

    // Arithmetic operations
    body.push(ScgStatement::Computation(ComputationNode {
        dst: "a".to_string(),
        op: BinOpKind::Add,
        lhs: ScgExpr::Int(1),
        rhs: ScgExpr::Int(2),
        tail_call: false,
        reassigns: None,
    }));
    body.push(ScgStatement::Computation(ComputationNode {
        dst: "b".to_string(),
        op: BinOpKind::Mul,
        lhs: ScgExpr::Var("a".to_string()),
        rhs: ScgExpr::Int(3),
        tail_call: false,
        reassigns: None,
    }));
    body.push(ScgStatement::Computation(ComputationNode {
        dst: "c".to_string(),
        op: BinOpKind::Sub,
        lhs: ScgExpr::Var("b".to_string()),
        rhs: ScgExpr::Var("a".to_string()),
        tail_call: false,
        reassigns: None,
    }));

    // Memory operations
    body.push(ScgStatement::Allocation(AllocationNode::Stack {
        name: "buf".to_string(),
        size: 256,
        ty: ScgType::U32,
    }));
    body.push(ScgStatement::Access(AccessNode::Store {
        ptr: ScgExpr::Var("buf".to_string()),
        offset: None,
        value: ScgExpr::Var("c".to_string()),
        ty: None,
    }));
    body.push(ScgStatement::Access(AccessNode::Load {
        dst: "val".to_string(),
        ptr: ScgExpr::Var("buf".to_string()),
        offset: None,
        ty: None,
    }));

    // Control flow
    body.push(ScgStatement::Control(ControlNode::If {
        cond: ScgExpr::Var("a".to_string()),
        then_body: vec![ScgStatement::Computation(ComputationNode {
            dst: "d".to_string(),
            op: BinOpKind::Add,
            lhs: ScgExpr::Var("val".to_string()),
            rhs: ScgExpr::Int(1),
            tail_call: false,
            reassigns: None,
        })],
        else_body: Some(vec![ScgStatement::Computation(ComputationNode {
            dst: "d".to_string(),
            op: BinOpKind::Sub,
            lhs: ScgExpr::Var("val".to_string()),
            rhs: ScgExpr::Int(1),
            tail_call: false,
            reassigns: None,
        })]),
    }));

    body.push(ScgStatement::Return(vec![ScgExpr::Var("d".to_string())]));

    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![],
            results: vec![ScgType::U32],
            body,
        })],
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_backend_comparison_benchmarks() {
        let results = super::run_benchmarks();
        for r in &results {
            assert!(r.name.starts_with("backend_comparison/"));
            assert!(r.mean_ns > 0);
        }
    }
}
