//! # VUMA Performance Benchmark Suite
//!
//! Structured benchmarks measuring compilation-pipeline performance and
//! codegen quality across all 8 VUMA backends.
//!
//! # Benchmark Categories
//!
//! | # | Benchmark                 | What it measures                                    |
//! |---|---------------------------|-----------------------------------------------------|
//! | 1 | SHA256d                   | Compile time, binary size, estimated instruction count per backend |
//! | 2 | Compilation Speed         | Parse→SCG→IR→codegen time for programs of varying size |
//! | 3 | Backend Comparison        | Same program, binary sizes across all 8 backends    |
//! | 4 | Codegen Quality           | Count redundant loads/stores in stack-slot output   |
//!
//! # Methodology
//!
//! - **Warmup**: 3 iterations (results discarded).
//! - **Measurement**: 10 iterations (all recorded, median reported).
//! - **Reporting**: [`BenchmarkResult`] with mean_ns, median_ns, iterations.
//!
//! # Integration
//!
//! The benchmark suite can be invoked via `vuma --bench` or used
//! programmatically from the `vuma-tests` crate.

pub mod sha256d;
pub mod compilation_speed;
pub mod backend_comparison;
pub mod codegen_quality;

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════════
// Core types
// ═══════════════════════════════════════════════════════════════════════════

/// The result of a single benchmark measurement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkResult {
    /// Benchmark name (e.g., "sha256d/aarch64").
    pub name: String,
    /// Mean time in nanoseconds.
    pub mean_ns: u64,
    /// Median time in nanoseconds.
    pub median_ns: u64,
    /// Number of iterations measured.
    pub iterations: usize,
    /// Optional extra data (e.g., binary size, instruction count).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
}

impl BenchmarkResult {
    /// Create a new benchmark result.
    pub fn new(name: impl Into<String>, mean_ns: u64, median_ns: u64, iterations: usize) -> Self {
        Self {
            name: name.into(),
            mean_ns,
            median_ns,
            iterations,
            extra: None,
        }
    }

    /// Create with extra JSON data.
    pub fn with_extra(mut self, key: &str, value: impl Serialize) -> Self {
        let extra = self.extra.get_or_insert_with(|| serde_json::Value::Object(Default::default()));
        if let serde_json::Value::Object(map) = extra {
            map.insert(key.to_string(), serde_json::to_value(&value).unwrap_or_default());
        }
        self
    }

    /// Return the mean time as a Duration.
    pub fn mean_duration(&self) -> Duration {
        Duration::from_nanos(self.mean_ns)
    }

    /// Return the median time as a Duration.
    pub fn median_duration(&self) -> Duration {
        Duration::from_nanos(self.median_ns)
    }
}

impl fmt::Display for BenchmarkResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: mean={:.2}ms median={:.2}ms ({} iterations)",
            self.name,
            self.mean_ns as f64 / 1_000_000.0,
            self.median_ns as f64 / 1_000_000.0,
            self.iterations
        )
    }
}

/// A complete benchmark suite report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkSuiteReport {
    /// All benchmark results.
    pub results: Vec<BenchmarkResult>,
    /// Timestamp of the benchmark run.
    pub timestamp: String,
    /// Total wall-clock time for the entire suite.
    pub total_time_ms: u64,
}

impl BenchmarkSuiteReport {
    /// Create a new empty report.
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            total_time_ms: 0,
        }
    }

    /// Add a benchmark result.
    pub fn add(&mut self, result: BenchmarkResult) {
        self.results.push(result);
    }

    /// Find a result by name prefix.
    pub fn find(&self, prefix: &str) -> Vec<&BenchmarkResult> {
        self.results.iter().filter(|r| r.name.starts_with(prefix)).collect()
    }
}

impl fmt::Display for BenchmarkSuiteReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "VUMA Benchmark Suite Report ({})", self.timestamp)?;
        writeln!(f, "Total time: {:.2}s", self.total_time_ms as f64 / 1000.0)?;
        writeln!(f, "{}", "─".repeat(60))?;
        for result in &self.results {
            writeln!(f, "  {}", result)?;
        }
        Ok(())
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Utility functions
// ═══════════════════════════════════════════════════════════════════════════

/// Run a closure `iterations` times and return (mean_ns, median_ns).
pub fn measure<F: Fn()>(f: F, iterations: usize) -> (u64, u64) {
    let mut times: Vec<u64> = Vec::with_capacity(iterations);

    // Warmup
    for _ in 0..3 {
        f();
    }

    // Measure
    for _ in 0..iterations {
        let start = std::time::Instant::now();
        f();
        let elapsed = start.elapsed().as_nanos() as u64;
        times.push(elapsed);
    }

    times.sort_unstable();
    let mean = times.iter().sum::<u64>() / times.len() as u64;
    let median = times[times.len() / 2];
    (mean, median)
}

/// Run the full benchmark suite and return a report.
pub fn run_full_suite() -> BenchmarkSuiteReport {
    let mut report = BenchmarkSuiteReport::new();
    let suite_start = std::time::Instant::now();

    // SHA256d benchmarks
    report.results.extend(sha256d::run_benchmarks());

    // Compilation speed benchmarks
    report.results.extend(compilation_speed::run_benchmarks());

    // Backend comparison
    report.results.extend(backend_comparison::run_benchmarks());

    // Codegen quality
    report.results.extend(codegen_quality::run_benchmarks());

    report.total_time_ms = suite_start.elapsed().as_millis() as u64;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_benchmark_result_display() {
        let result = BenchmarkResult::new("test/bench", 1_000_000, 900_000, 10);
        let s = format!("{}", result);
        assert!(s.contains("test/bench"));
        assert!(s.contains("1.00ms"));
    }

    #[test]
    fn test_benchmark_result_with_extra() {
        let result = BenchmarkResult::new("test/bench", 1_000_000, 900_000, 10)
            .with_extra("binary_size", 4096u64);
        assert!(result.extra.is_some());
    }

    #[test]
    fn test_suite_report() {
        let mut report = BenchmarkSuiteReport::new();
        report.add(BenchmarkResult::new("bench1", 100, 90, 5));
        report.add(BenchmarkResult::new("bench2", 200, 180, 5));
        assert_eq!(report.results.len(), 2);

        let found = report.find("bench1");
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn test_measure_utility() {
        let (mean, median) = measure(|| { let _ = 1 + 1; }, 10);
        assert!(mean > 0);
        assert!(median > 0);
    }
}
