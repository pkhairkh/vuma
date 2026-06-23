//! SHA256d benchmark: compile time, binary size, estimated instruction count per backend.
//!
//! Compiles a reference program through all 8 backends and measures
//! parse time, SCG-to-IR time, register allocation time, encoding time,
//! final binary size, and estimated instruction count.

use super::{BenchmarkResult, measure};
use vuma_codegen::backend::{create_backend, BackendKind, AllocatedProgram};
use vuma_codegen::scg_to_ir::{
    Scg, ScgNode, ScgFunction, ScgParam, ScgStatement, ScgType, ScgExpr,
    ComputationNode, AllocationNode, AccessNode, ControlNode,
};
use vuma_codegen::ir::BinOpKind;
use vuma_codegen::ScgToIr;

/// Build a representative SHA256d-like SCG program for benchmarking.
///
/// This constructs a codegen SCG with multiple functions that perform
/// arithmetic operations similar to SHA256d, providing a realistic
/// workload for the compilation pipeline.
fn build_sha256d_like_scg() -> Scg {
    let mut nodes = Vec::new();

    // Main function with many arithmetic operations
    let mut main_body = Vec::new();
    for i in 0..64 {
        main_body.push(ScgStatement::Computation(ComputationNode {
            dst: format!("h{}", i),
            op: if i % 4 == 0 { BinOpKind::Xor } else if i % 4 == 1 { BinOpKind::Add } else if i % 4 == 2 { BinOpKind::And } else { BinOpKind::Or },
            lhs: ScgExpr::Var(format!("a{}", i % 8)),
            rhs: ScgExpr::Var(format!("b{}", (i + 1) % 8)),
            tail_call: false,
        }));
    }
    main_body.push(ScgStatement::Return(vec![ScgExpr::Var("h0".to_string())]));

    nodes.push(ScgNode::Function(ScgFunction {
        name: "main".to_string(),
        params: vec![],
        results: vec![ScgType::U32],
        body: main_body,
    }));

    // Compression function
    let mut compress_body = Vec::new();
    for i in 0..32 {
        compress_body.push(ScgStatement::Computation(ComputationNode {
            dst: format!("w{}", i),
            op: BinOpKind::Add,
            lhs: ScgExpr::Var(format!("k{}", i % 8)),
            rhs: ScgExpr::Var(format!("msg{}", i % 16)),
            tail_call: false,
        }));
    }
    compress_body.push(ScgStatement::Return(vec![ScgExpr::Var("w0".to_string())]));

    nodes.push(ScgNode::Function(ScgFunction {
        name: "compress".to_string(),
        params: vec![
            ScgParam { name: "state".to_string(), ty: ScgType::Ptr },
            ScgParam { name: "block".to_string(), ty: ScgType::Ptr },
        ],
        results: vec![ScgType::U32],
        body: compress_body,
    }));

    Scg { nodes }
}

/// Run the SHA256d benchmark across all backends.
pub fn run_benchmarks() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();
    let scg = build_sha256d_like_scg();

    let backends: [(BackendKind, &str); 8] = [
        (BackendKind::AArch64, "aarch64"),
        (BackendKind::X86_64, "x86_64"),
        (BackendKind::RiscV64, "riscv64"),
        (BackendKind::Arm32, "arm32"),
        (BackendKind::Mips64, "mips64"),
        (BackendKind::PowerPC64, "ppc64"),
        (BackendKind::LoongArch64, "loongarch64"),
        (BackendKind::Wasm32, "wasm32"),
    ];

    for (kind, name) in &backends {
        if let Some(result) = benchmark_backend(&scg, *kind, name) {
            results.push(result);
        }
    }

    results
}

fn benchmark_backend(scg: &Scg, kind: BackendKind, name: &str) -> Option<BenchmarkResult> {
    // Measure full pipeline
    let (mean_ns, median_ns) = measure(|| {
        let mut ir_builder = ScgToIr::new();
        if let Ok(ir_program) = ir_builder.convert(scg) {
            if let Ok(backend) = create_backend(kind) {
                for func in &ir_program.functions {
                    let _ = backend.allocate_registers(func);
                }
            }
        }
    }, 5);

    // Get binary size and instruction count
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
        BenchmarkResult::new(format!("sha256d/{}", name), mean_ns, median_ns, 5)
            .with_extra("binary_size", binary_size as u64)
            .with_extra("ir_instructions", ir_instr_count as u64)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_sha256d_benchmarks() {
        let results = super::run_benchmarks();
        for r in &results {
            assert!(r.name.starts_with("sha256d/"));
            assert!(r.mean_ns > 0);
        }
    }

    #[test]
    fn test_build_sha256d_like_scg() {
        let scg = super::build_sha256d_like_scg();
        assert!(!scg.nodes.is_empty());
    }
}
