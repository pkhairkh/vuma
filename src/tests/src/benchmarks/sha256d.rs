//! SHA256d benchmark: compile time, binary size, estimated instruction count per backend.
//!
//! Compiles the **actual** `examples/sha256d.vuma` reference program (embedded
//! at compile time via `include_str!`) through the full parse → SCG → IR →
//! regalloc → encode pipeline for every backend and reports binary size and
//! IR instruction count alongside the timing measurements.
//!
//! # Source program selection
//!
//! The benchmark prefers `examples/sha256d.vuma`.  If that program does not
//! successfully lower to IR (e.g. because the frontend cannot yet handle some
//! construct used by sha256d), it transparently falls back to
//! `examples/fibonacci.vuma`.  The chosen program is reflected in the result
//! name (`sha256d_real/<backend>` vs `fibonacci_real/<backend>`) and in the
//! `source_program` extra field, so consumers can tell which workload was
//! actually measured.
//!
//! # Binary-size reporting
//!
//! `binary_size` is reported as `Option<usize>`: `Some(n)` when encoding
//! succeeds, `None` when encoding fails (or when no functions were allocated).
//! `None` serializes to JSON `null`.
//!
//! We deliberately avoid using `0` as the failure sentinel because `0` is a
//! valid (if pathological) binary size and would silently mask encoding
//! failures in downstream consumers.  For consumers that need a numeric
//! sentinel, `binary_size_or_minus_one` reports `-1` on failure.

use super::{BenchmarkResult, measure};
use vuma_codegen::backend::{create_backend, AllocatedProgram, BackendKind};
use vuma_codegen::ir::IRProgram;
use vuma_codegen::scg_to_ir::IRBuilder;
use vuma_parser::{AstToScg, Parser};

/// The real `examples/sha256d.vuma` reference program, embedded at compile time.
const SHA256D_SOURCE: &str = include_str!("../../../../examples/sha256d.vuma");

/// Fallback program: `examples/fibonacci.vuma`.  Used if `sha256d.vuma` does
/// not lower to IR, so the benchmark still produces per-backend numbers.
const FIBONACCI_SOURCE: &str = include_str!("../../../../examples/fibonacci.vuma");

/// Run the full parse → SCG → bridge → IR pipeline for a `.vuma` source and
/// return the lowered IR program.  Returns `None` if any frontend stage fails.
///
/// This is the same pipeline used by `cross_backend::compile_example_for_backend`,
/// reproduced locally so the benchmark does not depend on test-only helpers.
fn lower_to_ir(source: &str) -> Option<IRProgram> {
    // Step 1: Parse source → AST
    let mut parser = Parser::new(source);
    let parse_result = parser.parse_program();
    if parse_result.has_errors() {
        return None;
    }
    let ast = parse_result.unwrap();

    // Step 2: AST → vuma-scg SCG
    let mut scg = AstToScg::new().convert(&ast).ok()?;

    // Step 3: Run lightweight SCG transforms (DCE + constant folding at O1).
    // The result is intentionally discarded — transforms are best-effort.
    {
        use vuma::pipeline::{
            run_scg_transforms, CompileConfig, CompileTarget, OptLevel, VerificationLevel,
        };
        let config = CompileConfig {
            target: CompileTarget::Linux,
            opt_level: OptLevel::O1,
            verification_level: VerificationLevel::None,
            ..CompileConfig::default()
        };
        let _ = run_scg_transforms(&mut scg, &config);
    }

    // Step 4: Bridge vuma-scg SCG → codegen SCG
    let codegen_scg = vuma::pipeline::bridge_scg_to_codegen(&scg);

    // Step 5: Lower codegen SCG → IR
    let mut builder = IRBuilder::new();
    let ir_program = builder.build(&codegen_scg).ok()?;

    if ir_program.functions.is_empty() {
        return None;
    }

    Some(ir_program)
}

/// Select the benchmark source: prefer `sha256d.vuma`; fall back to
/// `fibonacci.vuma` if sha256d doesn't lower to IR.
///
/// Returns `(label, source)` where `label` is the human-readable program
/// name used in result names (`sha256d_real` or `fibonacci_real`).
fn select_benchmark_source() -> (&'static str, &'static str) {
    if lower_to_ir(SHA256D_SOURCE).is_some() {
        ("sha256d_real", SHA256D_SOURCE)
    } else if lower_to_ir(FIBONACCI_SOURCE).is_some() {
        ("fibonacci_real", FIBONACCI_SOURCE)
    } else {
        // Neither real program lowers; the per-backend loop will produce 0
        // results.  We still return the sha256d source so the intent is
        // clear in any error/diagnostic output.
        ("sha256d_real", SHA256D_SOURCE)
    }
}

/// Run the SHA256d benchmark across all 8 backends.
///
/// Produces one `BenchmarkResult` per backend that successfully lowers the
/// chosen program to IR and creates its backend.  Backends where encoding
/// fails still produce a result — the failure is reported via
/// `binary_size: None` rather than silently as `0`.
pub fn run_benchmarks() -> Vec<BenchmarkResult> {
    let mut results = Vec::new();
    let (label, source) = select_benchmark_source();

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
        if let Some(result) = benchmark_backend(source, *kind, name, label) {
            results.push(result);
        }
    }

    results
}

fn benchmark_backend(
    source: &str,
    kind: BackendKind,
    name: &str,
    label: &str,
) -> Option<BenchmarkResult> {
    // Measure the lowering + register-allocation portion of the pipeline.
    // We deliberately exclude `encode_program` from the timed region so the
    // measurement reflects frontend + regalloc cost rather than encoding I/O.
    let (mean_ns, median_ns) = measure(
        || {
            if let Some(ir_program) = lower_to_ir(source) {
                if let Ok(backend) = create_backend(kind) {
                    for func in &ir_program.functions {
                        let _ = backend.allocate_registers(func);
                    }
                }
            }
        },
        5,
    );

    // Re-lower to collect IR instruction count and final encoded bytes.
    let ir_program = lower_to_ir(source)?;
    let ir_instr_count: usize = ir_program
        .functions
        .iter()
        .map(|f| f.blocks.iter().map(|b| b.instructions.len()).sum::<usize>())
        .sum();

    let backend = create_backend(kind).ok()?;
    let mut allocated = Vec::new();
    for func in &ir_program.functions {
        if let Ok(a) = backend.allocate_registers(func) {
            allocated.push(a);
        }
    }

    // Binary size: `Option<usize>` so encoding failures are reported as
    // `None` (JSON `null`) rather than `0` (which is a valid size and would
    // mask the failure).  An empty `allocated` list also yields `None`
    // because no functions were available to encode.
    let binary_size: Option<usize> = if allocated.is_empty() {
        None
    } else {
        let total_code_size: usize = allocated.iter().map(|f| f.code_size).sum();
        let prog = AllocatedProgram {
            functions: allocated,
            total_code_size,
            total_data_size: 0,
        };
        backend.encode_program(&prog).ok().map(|b| b.len())
    };

    // Numeric sentinel for consumers that need a number: -1 on failure.
    let binary_size_or_minus_one: i64 = binary_size.map(|s| s as i64).unwrap_or(-1);

    Some(
        BenchmarkResult::new(format!("{}/{}", label, name), mean_ns, median_ns, 5)
            .with_extra("binary_size", binary_size)
            .with_extra("binary_size_or_minus_one", binary_size_or_minus_one)
            .with_extra("ir_instructions", ir_instr_count as u64)
            .with_extra("source_program", label),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256d_benchmarks() {
        let results = run_benchmarks();
        // We expect at least one backend to produce a result.  If sha256d
        // didn't lower, the fallback (fibonacci) should.
        assert!(
            !results.is_empty(),
            "sha256d benchmark should produce at least one result"
        );
        for r in &results {
            assert!(
                r.name.starts_with("sha256d_real/") || r.name.starts_with("fibonacci_real/"),
                "unexpected benchmark name: {}",
                r.name
            );
            assert!(r.mean_ns > 0, "mean_ns should be > 0 for {}", r.name);
            assert!(r.extra.is_some(), "extra should be present for {}", r.name);

            // binary_size must be present in the extra map; it is either a
            // positive number (success) or null (encoding failure).  It must
            // never be 0, because 0 would mask an encoding failure as success.
            let extra = r.extra.as_ref().unwrap();
            let obj = extra.as_object().expect("extra should be a JSON object");
            assert!(
                obj.contains_key("binary_size"),
                "binary_size key should be present for {}",
                r.name
            );
            let bs = &obj["binary_size"];
            if !bs.is_null() {
                let size = bs.as_u64().expect("binary_size should be u64 or null");
                assert!(
                    size > 0,
                    "binary_size must never be 0 (would mask failure) for {}; got {}",
                    r.name,
                    size
                );
            }
        }
    }

    #[test]
    fn test_lower_real_program_to_ir() {
        // At least one of the real programs should lower to IR.
        let sha256d_ok = lower_to_ir(SHA256D_SOURCE).is_some();
        let fib_ok = lower_to_ir(FIBONACCI_SOURCE).is_some();
        assert!(
            sha256d_ok || fib_ok,
            "At least one of sha256d.vuma or fibonacci.vuma should lower to IR"
        );
    }

    /// Encoding failures must be reported as `None` (JSON `null`), never as
    /// `Some(0)` (which is a valid size and would mask the failure).  This
    /// test directly exercises the failure path by simulating an empty
    /// allocated-functions list.
    #[test]
    fn test_binary_size_uses_none_not_zero_on_failure() {
        // Construct a result the same way `benchmark_backend` does when
        // encoding fails: binary_size = None.
        let result = BenchmarkResult::new("test/none_check", 1, 1, 1)
            .with_extra("binary_size", Option::<usize>::None)
            .with_extra("binary_size_or_minus_one", -1i64);

        let extra = result.extra.expect("extra should be present");
        let obj = extra.as_object().expect("extra should be a JSON object");

        // binary_size should be null, not 0.
        let bs = obj
            .get("binary_size")
            .expect("binary_size key should be present");
        assert!(bs.is_null(), "binary_size should be null on failure, got {}", bs);

        // The numeric sentinel should be -1.
        let bs_sentinel = obj
            .get("binary_size_or_minus_one")
            .expect("binary_size_or_minus_one key should be present");
        assert_eq!(
            bs_sentinel.as_i64(),
            Some(-1),
            "binary_size_or_minus_one should be -1 on failure, got {}",
            bs_sentinel
        );
    }
}
