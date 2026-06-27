//! Codegen quality benchmark: count redundant loads/stores in stack-slot output.
//!
//! Analyzes the IR to find potential redundant load/store patterns where
//! a value is stored and then immediately loaded again (or stored again
//! without intervening modifications). This is a measure of codegen quality
//! that helps identify optimization opportunities.

use super::{BenchmarkResult, measure};
use vuma_codegen::scg_to_ir::{
    Scg, ScgNode, ScgFunction, ScgParam, ScgStatement, ScgType, ScgExpr,
    ComputationNode, AllocationNode, AccessNode,
};
use vuma_codegen::ir::{BinOpKind, IRInstr, IRProgram};
use vuma_codegen::ScgToIr;

/// Run codegen quality benchmarks.
pub fn run_benchmarks() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    // Analyze at different program sizes
    for &size in &[10, 50, 100] {
        let scg = build_program_with_memory(size);

        let (mean_ns, median_ns) = measure(|| {
            let mut ir_builder = ScgToIr::new();
            let _ = ir_builder.convert(&scg);
        }, 10);

        let mut ir_builder = ScgToIr::new();
        if let Ok(ir_program) = ir_builder.convert(&scg) {
            let metrics = count_redundant_loads_stores(&ir_program);

            results.push(
                BenchmarkResult::new(
                    format!("codegen_quality/size_{}", size),
                    mean_ns,
                    median_ns,
                    10,
                )
                .with_extra("total_loads", metrics.total_loads as u64)
                .with_extra("total_stores", metrics.total_stores as u64)
                .with_extra("redundant_loads", metrics.redundant_loads as u64)
                .with_extra("redundant_stores", metrics.redundant_stores as u64)
                .with_extra("redundancy_ratio", format!("{:.3}", metrics.redundancy_ratio))
            );
        }
    }

    results
}

/// Results of codegen quality analysis.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CodegenQualityMetrics {
    /// Total number of IR load instructions.
    pub total_loads: usize,
    /// Total number of IR store instructions.
    pub total_stores: usize,
    /// Number of potentially redundant loads.
    pub redundant_loads: usize,
    /// Number of potentially redundant stores.
    pub redundant_stores: usize,
    /// Total number of IR instructions.
    pub total_instructions: usize,
    /// Percentage of loads/stores that are potentially redundant.
    pub redundancy_ratio: f64,
}

/// Count potentially redundant loads and stores in an IR program.
///
/// A load is considered potentially redundant if it follows a store to the
/// same address without any intervening modification. A store is redundant
/// if it writes to the same address as the immediately preceding store.
pub fn count_redundant_loads_stores(ir_program: &IRProgram) -> CodegenQualityMetrics {
    let mut total_loads = 0usize;
    let mut total_stores = 0usize;
    let mut redundant_loads = 0usize;
    let mut redundant_stores = 0usize;
    let mut total_instructions = 0usize;

    for func in &ir_program.functions {
        for block in &func.blocks {
            let mut last_store_addr: Option<String> = None;

            for instr in &block.instructions {
                total_instructions += 1;

                match instr {
                    IRInstr::Load { dst, addr, .. } => {
                        total_loads += 1;
                        let addr_str = format!("{:?}", addr);
                        if let Some(ref last_addr) = last_store_addr {
                            if *last_addr == addr_str {
                                redundant_loads += 1;
                            }
                        }
                        let _ = dst;
                    }
                    IRInstr::Store { addr, value, .. } => {
                        total_stores += 1;
                        let addr_str = format!("{:?}", addr);
                        if let Some(ref last_addr) = last_store_addr {
                            if *last_addr == addr_str {
                                redundant_stores += 1;
                            }
                        }
                        last_store_addr = Some(addr_str);
                        let _ = value;
                    }
                    _ => {}
                }
            }
        }
    }

    let redundancy_ratio = if total_loads + total_stores > 0 {
        (redundant_loads + redundant_stores) as f64 / (total_loads + total_stores) as f64
    } else {
        0.0
    };

    CodegenQualityMetrics {
        total_loads,
        total_stores,
        redundant_loads,
        redundant_stores,
        total_instructions,
        redundancy_ratio,
    }
}

/// Build a program with memory access patterns for quality analysis.
fn build_program_with_memory(size: usize) -> Scg {
    let mut body = Vec::new();

    // Allocate a buffer
    body.push(ScgStatement::Allocation(AllocationNode::Stack {
        name: "buf".to_string(),
        size: 1024,
        ty: ScgType::U32,
    }));

    // Interleave stores and loads (some potentially redundant)
    for i in 0..size {
        body.push(ScgStatement::Access(AccessNode::Store {
            ptr: ScgExpr::Var("buf".to_string()),
            offset: Some(ScgExpr::Int((i * 4) as i64)),
            value: ScgExpr::Int(i as i64),
            ty: None,
        }));
        if i % 3 == 0 {
            // Load immediately after store — potentially redundant
            body.push(ScgStatement::Access(AccessNode::Load {
                dst: format!("val{}", i),
                ptr: ScgExpr::Var("buf".to_string()),
                offset: Some(ScgExpr::Int((i * 4) as i64)),
                ty: None,
            }));
        }
    }

    // Some arithmetic
    for i in 0..size.min(20) {
        body.push(ScgStatement::Computation(ComputationNode {
            dst: format!("r{}", i),
            op: BinOpKind::Add,
            lhs: ScgExpr::Int(i as i64),
            rhs: ScgExpr::Int((i + 1) as i64),
            tail_call: false,
            reassigns: None,
        }));
    }

    body.push(ScgStatement::Return(vec![ScgExpr::Var("buf".to_string())]));

    Scg {
        nodes: vec![ScgNode::Function(ScgFunction {
            name: "main".to_string(),
            params: vec![],
            results: vec![ScgType::Ptr],
            body,
        })],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codegen_quality_benchmarks() {
        let results = run_benchmarks();
        for r in &results {
            assert!(r.name.starts_with("codegen_quality/"));
        }
    }

    #[test]
    fn test_quality_metrics_structure() {
        let metrics = CodegenQualityMetrics {
            total_loads: 10,
            total_stores: 5,
            redundant_loads: 2,
            redundant_stores: 1,
            total_instructions: 50,
            redundancy_ratio: 0.2,
        };
        assert_eq!(metrics.total_loads, 10);
        assert_eq!(metrics.redundancy_ratio, 0.2);
    }
}
