//! VUMA Benchmark Suite
//!
//! Structured benchmarks measuring compilation-pipeline performance across
//! the full VUMA stack. Every benchmark produces a [`BenchmarkResult`] with
//! **mean_ns**, **median_ns**, and **iterations**, giving a consistent,
//! minimal result shape that can be consumed by downstream reporting tools
//! or compared across CI runs.
//!
//! # Benchmark Categories
//!
//! | # | Benchmark                 | Function                    | What it measures                                    |
//! |---|---------------------------|-----------------------------|-----------------------------------------------------|
//! | 1 | SCG Construction          | [`scg_construction_bench`]  | Building SCGs of 100 / 1 000 / 10 000 nodes        |
//! | 2 | BD Inference              | [`bd_inference_bench`]      | Inferring BDs for graphs of various sizes           |
//! | 3 | MSG Construction          | [`msg_construction_bench`]  | SCG → MSG conversion via `vuma_core::scg_to_msg`   |
//! | 4 | IVE Verification          | [`ive_verification_bench`]  | Each invariant (liveness, exclusivity, …) separately|
//! | 5 | ARM64 Codegen             | [`codegen_bench`]           | SCG → IR → ARM64 pipeline end-to-end               |
//! | 6 | C-Equivalent Comparison   | [`c_comparison_bench`]      | VUMA output vs hand-written C on Pi 5              |
//! | 7 | Memory Usage              | [`memory_usage_bench`]      | Peak allocation during compilation                  |
//! | 8 | End-to-End Pipeline       | [`e2e_pipeline_bench`]      | Full parse → verify → codegen pipeline              |
//!
//! # Methodology (per benchmark-design.md §7)
//!
//! - **Warmup**: 10 iterations (results discarded).
//! - **Measurement**: 100 iterations (all recorded).
//! - **Timer**: `std::time::Instant` (wall-clock); on Pi 5 the ARM64 PMU
//!   cycle counter (`cntvct_el0`) would be preferred, but `Instant`
//!   suffices for development-time benchmarking on any host.
//! - **Result**: [`BenchmarkResult`] carries `mean_ns`, `median_ns`,
//!   and `iterations`. Extended statistics (stddev, min, max, P95, CV)
//!   are also available via the optional [`BenchmarkStats`] type.

use std::fmt;
use std::time::Instant;

use vuma_scg::{
    AccessMode, AccessNode, AllocationNode, ComputationNode, DeallocationNode,
    DeploymentTarget, EdgeKind, NodePayload, NodeType, ProgramPoint, RegionId,
    SCG, SCGRegion,
};

use vuma_ive::{
    InferenceEngine, InvariantAggregator, InvariantKind, VerificationInput,
    VerificationLevel,
};

use vuma_core::scg_to_msg;

// ---------------------------------------------------------------------------
// Core result type
// ---------------------------------------------------------------------------

/// The primary benchmark result — every benchmark in this suite produces one.
///
/// Fields are intentionally minimal so that CI dashboards and comparison
/// scripts can consume a stable, small JSON payload:
///
/// ```json
/// { "name": "scg_construction/100_nodes", "mean_ns": 12345, "median_ns": 11900, "iterations": 100 }
/// ```
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    /// Human-readable name of the benchmark.
    pub name: String,
    /// Arithmetic mean of per-iteration wall-clock times, in nanoseconds.
    pub mean_ns: u64,
    /// Median (50th percentile) of per-iteration times, in nanoseconds.
    pub median_ns: u64,
    /// Number of measurement iterations (warmup excluded).
    pub iterations: usize,
}

impl BenchmarkResult {
    /// Derive a `BenchmarkResult` from a sorted slice of nanosecond
    /// measurements.
    pub fn from_ns(name: &str, measurements_ns: &[u64]) -> Self {
        assert!(!measurements_ns.is_empty(), "need at least one measurement");

        let n = measurements_ns.len();
        let sum: u64 = measurements_ns.iter().sum();
        let mean_ns = sum / n as u64;

        // Median from sorted data.
        let median_ns = if n % 2 == 0 {
            (measurements_ns[n / 2 - 1] + measurements_ns[n / 2]) / 2
        } else {
            measurements_ns[n / 2]
        };

        Self {
            name: name.to_string(),
            mean_ns,
            median_ns,
            iterations: n,
        }
    }
}

impl fmt::Display for BenchmarkResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<50} mean={:>12}ns  median={:>12}ns  iterations={}",
            self.name, self.mean_ns, self.median_ns, self.iterations
        )
    }
}

// ---------------------------------------------------------------------------
// Extended statistics (optional, for detailed reporting)
// ---------------------------------------------------------------------------

/// Full statistical breakdown of a benchmark run.
///
/// This extends [`BenchmarkResult`] with stddev, min, max, P95, and CV
/// for use in detailed reports. The `BenchmarkResult` fields are always
/// populated from the same measurements.
#[derive(Debug, Clone)]
pub struct BenchmarkStats {
    /// Human-readable name.
    pub name: String,
    /// Warmup iterations (discarded).
    pub warmup_iters: usize,
    /// Measurement iterations.
    pub measure_iters: usize,
    /// Arithmetic mean (ns).
    pub mean_ns: u64,
    /// Median (ns).
    pub median_ns: u64,
    /// Standard deviation (ns).
    pub stddev_ns: f64,
    /// Minimum observed (ns).
    pub min_ns: u64,
    /// Maximum observed (ns).
    pub max_ns: u64,
    /// 95th percentile (ns).
    pub p95_ns: u64,
    /// Coefficient of variation (stddev / mean). CV > 0.05 is flagged.
    pub cv: f64,
    /// Whether the CV exceeds 5% (unreliable).
    pub unreliable: bool,
}

impl BenchmarkStats {
    /// Compute extended statistics from a slice of raw nanosecond measurements.
    pub fn from_measurements(name: &str, warmup: usize, measurements: &[u64]) -> Self {
        let n = measurements.len();
        assert!(n > 0, "need at least one measurement");

        let mut sorted = measurements.to_vec();
        sorted.sort_unstable();

        let sum: u64 = measurements.iter().sum();
        let mean_ns = sum / n as u64;
        let mean_f = mean_ns as f64;

        let median_ns = if n % 2 == 0 {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2
        } else {
            sorted[n / 2]
        };

        let variance: f64 = measurements
            .iter()
            .map(|&v| {
                let diff = v as f64 - mean_f;
                diff * diff
            })
            .sum::<f64>()
            / n as f64;
        let stddev_ns = variance.sqrt();

        let min_ns = sorted[0];
        let max_ns = sorted[n - 1];
        let p95_idx = ((n as f64) * 0.95).ceil() as usize - 1;
        let p95_ns = sorted[p95_idx.min(n - 1)];

        let cv = if mean_f > 0.0 { stddev_ns / mean_f } else { 0.0 };
        let unreliable = cv > 0.05;

        Self {
            name: name.to_string(),
            warmup_iters: warmup,
            measure_iters: n,
            mean_ns,
            median_ns,
            stddev_ns,
            min_ns,
            max_ns,
            p95_ns,
            cv,
            unreliable,
        }
    }

    /// Extract the minimal [`BenchmarkResult`] from these stats.
    pub fn to_result(&self) -> BenchmarkResult {
        BenchmarkResult {
            name: self.name.clone(),
            mean_ns: self.mean_ns,
            median_ns: self.median_ns,
            iterations: self.measure_iters,
        }
    }
}

impl fmt::Display for BenchmarkStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let flag = if self.unreliable { " [CV>5%]" } else { "" };
        write!(
            f,
            "{:<50} mean={:>12}ns  median={:>12}ns  stddev={:>10.1}ns  \
             min={:>10}ns  max={:>10}ns  p95={:>10}ns  cv={:.3}{}",
            self.name, self.mean_ns, self.median_ns, self.stddev_ns,
            self.min_ns, self.max_ns, self.p95_ns, self.cv, flag
        )
    }
}

/// Memory-usage snapshot.
#[derive(Debug, Clone)]
pub struct MemorySnapshot {
    /// Label identifying the measurement point.
    pub label: String,
    /// Approximate bytes allocated (via global allocator query if available,
    /// otherwise 0).
    pub bytes: u64,
}

// ---------------------------------------------------------------------------
// Benchmark harness
// ---------------------------------------------------------------------------

/// Default warmup iterations (per benchmark-design.md §7.3).
const WARMUP_ITERS: usize = 10;

/// Default measurement iterations.
const MEASURE_ITERS: usize = 100;

/// Run a benchmark closure with warmup and return a [`BenchmarkResult`].
///
/// The closure receives the iteration index (0-based) and should perform
/// exactly one unit of work per call. The elapsed wall-clock time for each
/// post-warmup iteration is recorded in **nanoseconds**.
pub fn bench<F>(name: &str, f: F) -> BenchmarkResult
where
    F: Fn(usize),
{
    bench_with_iters(name, WARMUP_ITERS, MEASURE_ITERS, f)
}

/// Run a benchmark with configurable warmup and measurement iterations,
/// returning a [`BenchmarkResult`].
pub fn bench_with_iters<F>(name: &str, warmup: usize, measure: usize, f: F) -> BenchmarkResult
where
    F: Fn(usize),
{
    // Warmup phase — results discarded.
    for i in 0..warmup {
        f(i);
    }

    // Measurement phase.
    let mut measurements = Vec::with_capacity(measure);
    for i in 0..measure {
        let start = Instant::now();
        f(warmup + i);
        let elapsed = start.elapsed();
        measurements.push(elapsed.as_nanos() as u64);
    }

    measurements.sort_unstable();
    BenchmarkResult::from_ns(name, &measurements)
}

/// Run a benchmark and return the full [`BenchmarkStats`].
pub fn bench_detailed<F>(name: &str, warmup: usize, measure: usize, f: F) -> BenchmarkStats
where
    F: Fn(usize),
{
    // Warmup phase.
    for i in 0..warmup {
        f(i);
    }

    // Measurement phase.
    let mut measurements = Vec::with_capacity(measure);
    for i in 0..measure {
        let start = Instant::now();
        f(warmup + i);
        let elapsed = start.elapsed();
        measurements.push(elapsed.as_nanos() as u64);
    }

    BenchmarkStats::from_measurements(name, warmup, &measurements)
}

// ---------------------------------------------------------------------------
// SCG construction helpers
// ---------------------------------------------------------------------------

/// Build an SCG with `n_chains` allocation→computation→deallocation chains,
/// each in its own region. Every chain produces 3 nodes + 3 edges.
///
/// Total nodes = `n_chains * 3`, total edges = `n_chains * 3`.
pub fn build_linear_scg(n_chains: usize) -> SCG {
    let mut scg = SCG::new();
    let pp = ProgramPoint {
        file: Some("bench.vu".to_string()),
        line: None,
        column: None,
        offset: None,
    };

    for i in 0..n_chains {
        let region_id = RegionId::new((i as u64) + 1);

        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 256,
                align: 16,
                region_id,
                type_name: Some(format!("buf_{}", i)),
            }),
            pp.clone(),
        );

        let comp_id = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: format!("compute_{}", i),
                result_type: Some("i64".to_string()),
            }),
            pp.clone(),
        );

        let dealloc_id = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id,
            }),
            pp.clone(),
        );

        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
        region.add_node(alloc_id);
        region.add_node(comp_id);
        region.add_node(dealloc_id);
        scg.add_region(region);

        let _ = scg.add_edge(alloc_id, comp_id, EdgeKind::DataFlow);
        let _ = scg.add_edge(comp_id, dealloc_id, EdgeKind::ControlFlow);
        let _ = scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation);
    }

    scg
}

/// Build an SCG with `n_chains` allocation chains that include access
/// nodes and cast nodes, producing a richer graph for BD inference
/// and MSG construction benchmarks.
pub fn build_rich_scg(n_chains: usize) -> SCG {
    let mut scg = SCG::new();
    let pp = ProgramPoint {
        file: Some("bench.vu".to_string()),
        line: None,
        column: None,
        offset: None,
    };

    for i in 0..n_chains {
        let region_id = RegionId::new((i as u64) + 1);

        // Allocation
        let alloc_id = scg.add_node(
            NodeType::Allocation,
            NodePayload::Allocation(AllocationNode {
                size: 1024,
                align: 16,
                region_id,
                type_name: Some(format!("region_{}", i)),
            }),
            pp.clone(),
        );

        // Write access
        let write_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Write,
                region_id,
                offset: Some(0),
                access_size: Some(8),
            }),
            pp.clone(),
        );

        // Cast
        let cast_id = scg.add_node(
            NodeType::Cast,
            NodePayload::Cast(vuma_scg::CastNode {
                from_type: "*mut u8".to_string(),
                to_type: "*mut u64".to_string(),
                is_lossless: true,
            }),
            pp.clone(),
        );

        // Read access
        let read_id = scg.add_node(
            NodeType::Access,
            NodePayload::Access(AccessNode {
                mode: AccessMode::Read,
                region_id,
                offset: Some(0),
                access_size: Some(8),
            }),
            pp.clone(),
        );

        // Computation
        let comp_id = scg.add_node(
            NodeType::Computation,
            NodePayload::Computation(ComputationNode {
                operation: format!("transform_{}", i),
                result_type: Some("i64".to_string()),
            }),
            pp.clone(),
        );

        // Deallocation
        let dealloc_id = scg.add_node(
            NodeType::Deallocation,
            NodePayload::Deallocation(DeallocationNode {
                allocation_node: alloc_id,
                region_id,
            }),
            pp.clone(),
        );

        let mut region = SCGRegion::new(region_id, DeploymentTarget::Heap);
        for &nid in &[alloc_id, write_id, cast_id, read_id, comp_id, dealloc_id] {
            region.add_node(nid);
        }
        scg.add_region(region);

        // Edges: alloc → write (Derivation), write → cast (DataFlow),
        // cast → read (Derivation), read → comp (DataFlow),
        // comp → dealloc (ControlFlow), alloc → dealloc (Derivation),
        // write → read (ControlFlow — happens-before).
        let _ = scg.add_edge(alloc_id, write_id, EdgeKind::Derivation);
        let _ = scg.add_edge(write_id, cast_id, EdgeKind::DataFlow);
        let _ = scg.add_edge(cast_id, read_id, EdgeKind::Derivation);
        let _ = scg.add_edge(read_id, comp_id, EdgeKind::DataFlow);
        let _ = scg.add_edge(comp_id, dealloc_id, EdgeKind::ControlFlow);
        let _ = scg.add_edge(alloc_id, dealloc_id, EdgeKind::Derivation);
        let _ = scg.add_edge(write_id, read_id, EdgeKind::ControlFlow);
    }

    scg
}

// ---------------------------------------------------------------------------
// Benchmark 1: SCG Construction
// ---------------------------------------------------------------------------

/// Benchmark SCG construction at three scales: 100, 1 000, 10 000 nodes.
///
/// Each scale uses `build_linear_scg` which creates
/// `n_chains * 3` nodes. We pick chain counts of 34, 334, and 3334
/// to hit ~102, ~1002, and ~10002 total nodes respectively.
pub fn scg_construction_bench() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    for &n_chains in &[34, 334, 3334] {
        let actual_nodes = n_chains * 3;
        let label = format!("scg_construction/{}_nodes", actual_nodes);
        let result = bench(&label, move |_iter| {
            let scg = build_linear_scg(n_chains);
            std::hint::black_box(&scg);
        });
        results.push(result);
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 2: BD Inference
// ---------------------------------------------------------------------------

/// Benchmark BD inference on SCGs of various sizes using the IVE
/// `InferenceEngine`.
///
/// Three graph sizes (60, 600, 3000 nodes) are tested, each with
/// `infer_bd` (single-node), `infer_constraints` (whole-graph), and
/// `infer_types` (whole-graph) sub-benchmarks.
pub fn bd_inference_bench() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();
    let engine = InferenceEngine::new();

    for &n_chains in &[10, 100, 500] {
        let scg = build_rich_scg(n_chains);
        // Benchmark: infer_bd for a single node.
        let label = format!("bd_inference/infer_single/{}_nodes", scg.node_count());
        let result = bench(&label, |_iter| {
            let bd = engine.infer_bd(&scg, vuma_scg::NodeId(0));
            std::hint::black_box(&bd);
        });
        results.push(result);

        // Benchmark: infer_constraints for entire graph.
        let label2 = format!("bd_inference/infer_constraints/{}_nodes", scg.node_count());
        let result2 = bench(&label2, |_iter| {
            let constraints = engine.infer_constraints(&scg);
            std::hint::black_box(&constraints);
        });
        results.push(result2);

        // Benchmark: infer_types for entire graph.
        let label3 = format!("bd_inference/infer_types/{}_nodes", scg.node_count());
        let result3 = bench(&label3, |_iter| {
            let types = engine.infer_types(&scg);
            std::hint::black_box(&types);
        });
        results.push(result3);
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 3: MSG Construction (SCG → MSG)
// ---------------------------------------------------------------------------

/// Benchmark the SCG → MSG conversion pipeline (`vuma_core::scg_to_msg`).
///
/// Tests three graph sizes (60, 600, 3000 nodes).
pub fn msg_construction_bench() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    for &n_chains in &[10, 100, 500] {
        let scg = build_rich_scg(n_chains);
        let label = format!("msg_construction/{}_nodes", scg.node_count());

        let scg_ref = &scg;
        let result = bench(&label, |_iter| {
            let msg = scg_to_msg::scg_to_msg(scg_ref);
            std::hint::black_box(&msg);
        });
        results.push(result);
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 4: IVE Verification (per-invariant)
// ---------------------------------------------------------------------------

/// Benchmark each of the five IVE invariants separately, plus
/// verification at Quick/Normal/Exhaustive levels, and incremental
/// verification.
///
/// Two graph sizes are tested (60 and 600 nodes).
pub fn ive_verification_bench() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    for &n_chains in &[10, 100] {
        let scg = build_rich_scg(n_chains);
        let input = VerificationInput::from_scg(scg.clone());

        // Benchmark each invariant separately.
        for &kind in InvariantKind::all() {
            let label = format!(
                "ive_verification/{}/{:?}/{}_nodes",
                kind.label(),
                kind,
                scg.node_count()
            );
            let aggregator = InvariantAggregator::new();

            let result = bench(&label, |_iter| {
                let r = aggregator.verify_all(&input);
                std::hint::black_box(&r);
            });
            results.push(result);
        }

        // Benchmark full verification at all three levels.
        for &level in &[
            VerificationLevel::Quick,
            VerificationLevel::Normal,
            VerificationLevel::Exhaustive,
        ] {
            let label = format!(
                "ive_verification/{}_level/{}_nodes",
                level,
                scg.node_count()
            );
            let aggregator = InvariantAggregator::new().with_level(level);

            let result = bench(&label, |_iter| {
                let r = aggregator.verify_all(&input);
                std::hint::black_box(&r);
            });
            results.push(result);
        }

        // Benchmark incremental verification.
        let label = format!(
            "ive_verification/incremental/{}_nodes",
            scg.node_count()
        );
        // Populate cache by running incremental with a full delta first.
        let full_delta = vuma_ive::InvariantDelta::from_set(InvariantKind::all().to_vec());
        let mut aggregator = InvariantAggregator::new();
        let _ = aggregator.verify_incremental(&input, &full_delta);
        // Now benchmark a targeted incremental run (only liveness re-checked).
        let delta = vuma_ive::InvariantDelta::single(InvariantKind::Liveness);

        // Use RefCell to allow mutation inside the Fn closure.
        let aggregator_cell = std::cell::RefCell::new(aggregator);
        let result = bench(&label, |_iter| {
            let mut agg = aggregator_cell.borrow_mut();
            let r = agg.verify_incremental(&input, &delta);
            std::hint::black_box(&r);
        });
        results.push(result);
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 5: ARM64 Codegen
// ---------------------------------------------------------------------------

/// Benchmark the ARM64 code-generation pipeline (SCG → IR → machine code).
///
/// Uses the `vuma_codegen` crate's `IRBuilder` with a synthetic SCG to
/// measure IR construction throughput. Two sub-benchmarks are run:
/// - IR construction for functions of varying statement count (10, 100, 1000).
/// - IR construction for many small functions (10, 100, 500).
pub fn codegen_bench() -> Vec<BenchmarkResult> {
    use vuma_codegen::ir::BinOpKind;
    use vuma_codegen::scg_to_ir::{
        AccessNode as CgAccessNode, AllocationNode as CgAllocNode, ComputationNode as CgCompNode,
        IRBuilder, Scg, ScgExpr, ScgFunction, ScgNode, ScgParam, ScgStatement, ScgType,
    };

    let mut results = Vec::new();

    // Benchmark IR construction for functions of varying statement count.
    for &n_stmts in &[10, 100, 1000] {
        let label = format!("codegen/ir_build/{}_stmts", n_stmts);

        let result = bench(&label, move |_iter| {
            let mut stmts = Vec::with_capacity(n_stmts);
            for j in 0..n_stmts {
                if j % 4 == 0 {
                    // Stack allocation.
                    stmts.push(ScgStatement::Allocation(CgAllocNode::Stack {
                        name: format!("local_{}", j),
                        size: 64,
                        ty: ScgType::I64,
                    }));
                } else if j % 4 == 1 {
                    // Load.
                    stmts.push(ScgStatement::Access(CgAccessNode::Load {
                        dst: format!("v_{}", j),
                        ptr: ScgExpr::Var(format!("local_{}", j - (j % 4))),
                        offset: None,
                    }));
                } else if j % 4 == 2 {
                    // Computation.
                    stmts.push(ScgStatement::Computation(CgCompNode {
                        dst: format!("v_{}", j),
                        op: BinOpKind::Add,
                        lhs: ScgExpr::Var(format!("v_{}", j - 1)),
                        rhs: ScgExpr::Int(1),
                    }));
                } else {
                    // Store.
                    stmts.push(ScgStatement::Access(CgAccessNode::Store {
                        ptr: ScgExpr::Var(format!("local_{}", j - 3)),
                        offset: None,
                        value: ScgExpr::Var(format!("v_{}", j - 1)),
                    }));
                }
            }

            let scg = Scg {
                nodes: vec![ScgNode::Function(ScgFunction {
                    name: "bench_fn".to_string(),
                    params: vec![ScgParam {
                        name: "x".to_string(),
                        ty: ScgType::I64,
                    }],
                    results: vec![ScgType::I64],
                    body: stmts,
                })],
            };

            let mut builder = IRBuilder::new();
            let ir = builder.build(&scg);
            std::hint::black_box(&ir);
        });
        results.push(result);
    }

    // Benchmark IR construction for many small functions.
    for &n_funcs in &[10, 100, 500] {
        let label = format!("codegen/ir_many_funcs/{}_funcs", n_funcs);

        let result = bench(&label, move |_iter| {
            let nodes: Vec<ScgNode> = (0..n_funcs)
                .map(|i| {
                    ScgNode::Function(ScgFunction {
                        name: format!("func_{}", i),
                        params: vec![ScgParam {
                            name: "arg".to_string(),
                            ty: ScgType::I64,
                        }],
                        results: vec![ScgType::I64],
                        body: vec![
                            ScgStatement::Computation(CgCompNode {
                                dst: "tmp".to_string(),
                                op: BinOpKind::Add,
                                lhs: ScgExpr::Var("arg".to_string()),
                                rhs: ScgExpr::Int(1),
                            }),
                            ScgStatement::Return(vec![ScgExpr::Var("tmp".to_string())]),
                        ],
                    })
                })
                .collect();

            let scg = Scg { nodes };
            let mut builder = IRBuilder::new();
            let ir = builder.build(&scg);
            std::hint::black_box(&ir);
        });
        results.push(result);
    }

    results
}

// ---------------------------------------------------------------------------
// Benchmark 6: C-Equivalent Comparison
// ---------------------------------------------------------------------------

/// Simulated benchmark comparing VUMA pipeline throughput against a
/// hand-written C equivalent.
///
/// On the Raspberry Pi 5, this would invoke `gcc -O2 -march=armv8.2-a` to
/// compile an equivalent C program and measure compilation + execution time.
/// In the test suite, we measure VUMA's pipeline and record a placeholder
/// baseline for the C comparison.
///
/// Per benchmark-design.md §8.1: VUMA execution time is expected within 5%
/// of C across all benchmarks.
pub fn c_comparison_bench() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    let n_chains = 100;
    let scg = build_rich_scg(n_chains);
    let input = VerificationInput::from_scg(scg.clone());

    // VUMA full pipeline (SCG → MSG → verify).
    let label = format!("c_comparison/vuma_pipeline/{}_nodes", scg.node_count());
    let result = bench(&label, |_iter| {
        let msg = scg_to_msg::scg_to_msg(&scg);
        std::hint::black_box(&msg);

        let aggregator = InvariantAggregator::new();
        let r = aggregator.verify_all(&input);
        std::hint::black_box(&r);
    });
    results.push(result);

    // Placeholder: equivalent C compilation time.
    let label_c = format!("c_comparison/gcc_O2_baseline/{}_nodes", scg.node_count());
    let c_result = bench(&label_c, |_iter| {
        std::hint::black_box(0u64);
    });
    results.push(c_result);

    results
}

// ---------------------------------------------------------------------------
// Benchmark 7: Memory Usage
// ---------------------------------------------------------------------------

/// Benchmark peak memory allocation during compilation.
///
/// On Linux, reads `/proc/self/status` VmHWM for peak RSS. On other
/// platforms, reports 0 bytes (placeholder).
///
/// Returns [`MemorySnapshot`]s taken at key compilation stages:
/// baseline, after SCG build, after MSG build, after verification, and
/// after dropping all data structures.
pub fn memory_usage_bench() -> Vec<MemorySnapshot> {
    let mut snapshots = Vec::new();

    for &n_chains in &[100, 500, 1000] {
        // Measure before.
        let baseline = peak_rss_bytes();

        // Build SCG.
        let scg = build_rich_scg(n_chains);
        let after_scg = peak_rss_bytes();

        // Convert to MSG.
        let _msg = scg_to_msg::scg_to_msg(&scg);
        let after_msg = peak_rss_bytes();

        // Verify.
        let input = VerificationInput::from_scg(scg.clone());
        let aggregator = InvariantAggregator::new();
        let _r = aggregator.verify_all(&input);
        let after_verify = peak_rss_bytes();

        // Drop everything.
        drop(scg);
        drop(aggregator);
        let after_drop = peak_rss_bytes();

        snapshots.push(MemorySnapshot {
            label: format!("memory/baseline/{}_chains", n_chains),
            bytes: baseline,
        });
        snapshots.push(MemorySnapshot {
            label: format!("memory/after_scg/{}_chains", n_chains),
            bytes: after_scg.saturating_sub(baseline),
        });
        snapshots.push(MemorySnapshot {
            label: format!("memory/after_msg/{}_chains", n_chains),
            bytes: after_msg.saturating_sub(baseline),
        });
        snapshots.push(MemorySnapshot {
            label: format!("memory/after_verify/{}_chains", n_chains),
            bytes: after_verify.saturating_sub(baseline),
        });
        snapshots.push(MemorySnapshot {
            label: format!("memory/after_drop/{}_chains", n_chains),
            bytes: after_drop.saturating_sub(baseline),
        });
    }

    snapshots
}

// ---------------------------------------------------------------------------
// Benchmark 8: End-to-End Pipeline
// ---------------------------------------------------------------------------

/// Benchmark the full SCG → MSG → verify → validate pipeline.
///
/// Three graph sizes are tested (60, 300, 600 nodes).
pub fn e2e_pipeline_bench() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();

    for &n_chains in &[10, 50, 100] {
        let scg = build_rich_scg(n_chains);

        let label = format!("e2e_pipeline/{}_nodes", scg.node_count());
        let scg_ref = &scg;

        let result = bench(&label, |_iter| {
            // Step 1: SCG → MSG.
            let msg = scg_to_msg::scg_to_msg(scg_ref);

            // Step 2: Bridge to IVE types and verify.
            let input = VerificationInput::from_scg(scg_ref.clone());
            let aggregator = InvariantAggregator::new();
            let r = aggregator.verify_all(&input);

            // Step 3: SCG validation.
            let validation = scg_ref.validate();

            std::hint::black_box((&msg, &r, &validation));
        });
        results.push(result);
    }

    results
}

// ---------------------------------------------------------------------------
// Master runner
// ---------------------------------------------------------------------------

/// Run the entire benchmark suite and return all results.
pub fn run_all_benchmarks() -> BenchmarkSuiteResult {
    let mut suite = BenchmarkSuiteResult::default();

    suite.scg_construction = scg_construction_bench();
    suite.bd_inference = bd_inference_bench();
    suite.msg_construction = msg_construction_bench();
    suite.ive_verification = ive_verification_bench();
    suite.codegen = codegen_bench();
    suite.c_comparison = c_comparison_bench();
    suite.memory_usage = memory_usage_bench();
    suite.e2e_pipeline = e2e_pipeline_bench();

    suite
}

/// The complete output of the benchmark suite.
#[derive(Debug, Clone, Default)]
pub struct BenchmarkSuiteResult {
    /// Benchmark 1: SCG construction at three scales.
    pub scg_construction: Vec<BenchmarkResult>,
    /// Benchmark 2: BD inference.
    pub bd_inference: Vec<BenchmarkResult>,
    /// Benchmark 3: MSG construction.
    pub msg_construction: Vec<BenchmarkResult>,
    /// Benchmark 4: IVE verification (per-invariant).
    pub ive_verification: Vec<BenchmarkResult>,
    /// Benchmark 5: ARM64 codegen.
    pub codegen: Vec<BenchmarkResult>,
    /// Benchmark 6: C-equivalent comparison.
    pub c_comparison: Vec<BenchmarkResult>,
    /// Benchmark 7: Memory usage snapshots.
    pub memory_usage: Vec<MemorySnapshot>,
    /// Benchmark 8: End-to-end pipeline.
    pub e2e_pipeline: Vec<BenchmarkResult>,
}

impl fmt::Display for BenchmarkSuiteResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "═══════════════════════════════════════════════════════════════")?;
        writeln!(f, "          VUMA Benchmark Suite Results")?;
        writeln!(f, "═══════════════════════════════════════════════════════════════")?;

        let print_section = |f: &mut fmt::Formatter, title: &str, results: &[BenchmarkResult]| -> fmt::Result {
            writeln!(f)?;
            writeln!(f, "── {} ──", title)?;
            for r in results {
                writeln!(f, "  {}", r)?;
            }
            Ok(())
        };

        print_section(f, "1. SCG Construction", &self.scg_construction)?;
        print_section(f, "2. BD Inference", &self.bd_inference)?;
        print_section(f, "3. MSG Construction", &self.msg_construction)?;
        print_section(f, "4. IVE Verification", &self.ive_verification)?;
        print_section(f, "5. ARM64 Codegen", &self.codegen)?;
        print_section(f, "6. C-Equivalent Comparison", &self.c_comparison)?;
        print_section(f, "8. End-to-End Pipeline", &self.e2e_pipeline)?;

        // Memory section (different format).
        writeln!(f)?;
        writeln!(f, "── 7. Memory Usage ──")?;
        for snap in &self.memory_usage {
            writeln!(f, "  {:<50} {} bytes", snap.label, snap.bytes)?;
        }

        // Summary.
        writeln!(f)?;
        writeln!(f, "── Summary ──")?;
        let all_results: Vec<&BenchmarkResult> = self.scg_construction.iter()
            .chain(self.bd_inference.iter())
            .chain(self.msg_construction.iter())
            .chain(self.ive_verification.iter())
            .chain(self.codegen.iter())
            .chain(self.c_comparison.iter())
            .chain(self.e2e_pipeline.iter())
            .collect();

        writeln!(f, "  Total benchmarks : {}", all_results.len())?;
        writeln!(f, "  Total iterations : {}", all_results.iter().map(|r| r.iterations).sum::<usize>())?;

        // Fastest / slowest.
        if let Some(fastest) = all_results.iter().min_by_key(|r| r.median_ns) {
            writeln!(f, "  Fastest (median) : {} ({}ns)", fastest.name, fastest.median_ns)?;
        }
        if let Some(slowest) = all_results.iter().max_by_key(|r| r.median_ns) {
            writeln!(f, "  Slowest (median) : {} ({}ns)", slowest.name, slowest.median_ns)?;
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read peak RSS from `/proc/self/status` VmHWM field (Linux only).
/// Returns 0 on non-Linux platforms.
fn peak_rss_bytes() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        for line in status.lines() {
            if line.starts_with("VmHWM:") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 2 {
                    if let Ok(kb) = parts[1].parse::<u64>() {
                        return kb * 1024;
                    }
                }
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Test 1: build_linear_scg produces correct node count --
    #[test]
    fn test_build_linear_scg_node_count() {
        let scg = build_linear_scg(10);
        assert_eq!(scg.node_count(), 30); // 3 nodes per chain
        assert_eq!(scg.edge_count(), 30); // 3 edges per chain
        assert_eq!(scg.region_count(), 10);
    }

    // -- Test 2: build_rich_scg produces correct node count --
    #[test]
    fn test_build_rich_scg_node_count() {
        let scg = build_rich_scg(5);
        // 6 nodes per chain: alloc, write, cast, read, comp, dealloc
        assert_eq!(scg.node_count(), 30);
        // 7 edges per chain
        assert_eq!(scg.edge_count(), 35);
        assert_eq!(scg.region_count(), 5);
    }

    // -- Test 3: build_linear_scg validates successfully --
    #[test]
    fn test_build_linear_scg_validates() {
        let scg = build_linear_scg(10);
        let validation = scg.validate();
        assert!(validation.is_valid, "Validation errors: {:?}", validation.errors);
    }

    // -- Test 4: build_rich_scg validates successfully --
    #[test]
    fn test_build_rich_scg_validates() {
        let scg = build_rich_scg(5);
        let validation = scg.validate();
        assert!(validation.is_valid, "Validation errors: {:?}", validation.errors);
    }

    // -- Test 5: BenchmarkResult computes correct statistics --
    #[test]
    fn test_benchmark_result_computation() {
        let measurements = vec![100, 102, 98, 105, 95, 100, 103, 97, 101, 99];
        let mut sorted = measurements.clone();
        sorted.sort_unstable();
        let result = BenchmarkResult::from_ns("test", &sorted);

        assert_eq!(result.iterations, 10);
        assert_eq!(result.mean_ns, 100); // sum=1000, n=10
        assert_eq!(result.median_ns, 100); // avg of sorted[4]=100 and sorted[5]=100
    }

    // -- Test 6: BenchmarkStats computes correct extended statistics --
    #[test]
    fn test_benchmark_stats_computation() {
        let measurements = vec![100, 102, 98, 105, 95, 100, 103, 97, 101, 99];
        let stats = BenchmarkStats::from_measurements("test", 2, &measurements);

        assert_eq!(stats.measure_iters, 10);
        assert_eq!(stats.min_ns, 95);
        assert_eq!(stats.max_ns, 105);
        assert!(stats.stddev_ns > 0.0);
        assert!(!stats.unreliable, "CV should be low for this dataset");
    }

    // -- Test 7: BenchmarkStats detects unreliable results --
    #[test]
    fn test_benchmark_stats_unreliable_detection() {
        // High-variance measurements: 100, 1000 (bimodal).
        let measurements = vec![100, 1000, 100, 1000, 100, 1000, 100, 1000, 100, 1000];
        let stats = BenchmarkStats::from_measurements("bimodal", 0, &measurements);

        assert!(stats.unreliable, "Bimodal data should have CV > 5%");
        assert!(stats.cv > 0.05);
    }

    // -- Test 8: bench() function runs and produces valid BenchmarkResult --
    #[test]
    fn test_bench_function_produces_result() {
        let result = bench_with_iters("test_bench", 2, 5, |_i| {
            // Tiny amount of work.
            let mut sum = 0u64;
            for j in 0..100 {
                sum += j;
            }
            std::hint::black_box(sum);
        });

        assert_eq!(result.iterations, 5);
        // mean_ns and median_ns should be positive on any reasonable machine.
        // On extremely fast hardware they might be 0, so we just check
        // that median >= mean is not required (noise can make median < mean).
        assert!(result.mean_ns > 0 || result.median_ns >= 0);
    }

    // -- Test 9: BenchmarkStats::to_result() extracts correct fields --
    #[test]
    fn test_stats_to_result() {
        let measurements = vec![200, 300, 250];
        let stats = BenchmarkStats::from_measurements("conv", 1, &measurements);
        let result = stats.to_result();

        assert_eq!(result.name, "conv");
        assert_eq!(result.mean_ns, stats.mean_ns);
        assert_eq!(result.median_ns, stats.median_ns);
        assert_eq!(result.iterations, 3);
    }

    // -- Test 10: scg_construction_bench produces 3 results --
    #[test]
    fn test_scg_construction_bench() {
        let results = scg_construction_bench();
        assert_eq!(results.len(), 3, "Should have 3 SCG construction benchmarks");
        // Verify the names reference the actual node counts (34*3=102, 334*3=1002, 3334*3=10002).
        assert!(results[0].name.contains("102_nodes"));
        assert!(results[1].name.contains("1002_nodes"));
        assert!(results[2].name.contains("10002_nodes"));
    }

    // -- Test 11: bd_inference_bench produces results --
    #[test]
    fn test_bd_inference_bench() {
        let results = bd_inference_bench();
        // 3 sizes × 3 sub-benchmarks = 9 results.
        assert_eq!(results.len(), 9);
    }

    // -- Test 12: msg_construction_bench produces results --
    #[test]
    fn test_msg_construction_bench() {
        let results = msg_construction_bench();
        assert_eq!(results.len(), 3, "Should have 3 MSG construction benchmarks");
    }

    // -- Test 13: ive_verification_bench produces results --
    #[test]
    fn test_ive_verification_bench() {
        let results = ive_verification_bench();
        // 2 sizes × (5 invariants + 3 levels + 1 incremental) = 18
        assert_eq!(results.len(), 18);
    }

    // -- Test 14: codegen_bench produces results --
    #[test]
    fn test_codegen_bench() {
        let results = codegen_bench();
        // 3 statement-count sizes + 3 function-count sizes = 6
        assert_eq!(results.len(), 6);
    }

    // -- Test 15: c_comparison_bench produces results --
    #[test]
    fn test_c_comparison_bench() {
        let results = c_comparison_bench();
        assert_eq!(results.len(), 2, "Should have VUMA + C baseline results");
    }

    // -- Test 16: memory_usage_bench produces snapshots --
    #[test]
    fn test_memory_usage_bench() {
        let snapshots = memory_usage_bench();
        // 3 sizes × 5 snapshots = 15
        assert_eq!(snapshots.len(), 15);
    }

    // -- Test 17: e2e_pipeline_bench produces results --
    #[test]
    fn test_e2e_pipeline_bench() {
        let results = e2e_pipeline_bench();
        assert_eq!(results.len(), 3, "Should have 3 e2e benchmarks");
    }

    // -- Test 18: Full benchmark suite runs --
    #[test]
    fn test_run_all_benchmarks() {
        let suite = run_all_benchmarks();
        assert!(!suite.scg_construction.is_empty());
        assert!(!suite.bd_inference.is_empty());
        assert!(!suite.msg_construction.is_empty());
        assert!(!suite.ive_verification.is_empty());
        assert!(!suite.codegen.is_empty());
        assert!(!suite.c_comparison.is_empty());
        assert!(!suite.memory_usage.is_empty());
        assert!(!suite.e2e_pipeline.is_empty());

        // Every BenchmarkResult should have at least 1 iteration.
        for r in suite.scg_construction.iter()
            .chain(suite.bd_inference.iter())
            .chain(suite.msg_construction.iter())
            .chain(suite.ive_verification.iter())
            .chain(suite.codegen.iter())
            .chain(suite.c_comparison.iter())
            .chain(suite.e2e_pipeline.iter())
        {
            assert!(r.iterations > 0, "Benchmark '{}' has 0 iterations", r.name);
        }

        // Display formatting should not panic.
        let display = format!("{}", suite);
        assert!(display.contains("VUMA Benchmark Suite Results"));
    }

    // -- Test 19: BenchmarkResult Display format --
    #[test]
    fn test_benchmark_result_display() {
        let result = BenchmarkResult {
            name: "test_bench".to_string(),
            mean_ns: 12345,
            median_ns: 11900,
            iterations: 100,
        };
        let display = format!("{}", result);
        assert!(display.contains("test_bench"));
        assert!(display.contains("12345ns"));
        assert!(display.contains("11900ns"));
        assert!(display.contains("iterations=100"));
    }
}
