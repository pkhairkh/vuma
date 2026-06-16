//! Compilation telemetry for VUMA.
//!
//! This module provides privacy-safe telemetry that tracks compilation metrics
//! such as stage timings, memory usage, and error counts. **No source code is
//! ever included in telemetry output** — only aggregate metrics.
//!
//! # Architecture
//!
//! - [`TelemetryCollector`] accumulates metrics during compilation.
//! - [`TelemetryReport`] is the final, JSON-serializable report.
//! - The `--telemetry` CLI flag outputs the report to stdout.
//!
//! # Privacy
//!
//! The telemetry system is designed to be privacy-safe:
//! - No source code, file paths, or user-identifiable data is collected.
//! - Only aggregate metrics: timing, counts, sizes.
//! - The report is opt-in (requires `--telemetry` flag).
//!
//! # Example
//!
//! ```rust,ignore
//! use vuma::telemetry::TelemetryCollector;
//!
//! let mut collector = TelemetryCollector::new();
//! collector.stage_start("parse");
//! // ... do parsing ...
//! collector.stage_end("parse");
//! collector.increment_error_count();
//!
//! let report = collector.finalize();
//! println!("{}", serde_json::to_string_pretty(&report).unwrap());
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

// ═══════════════════════════════════════════════════════════════════════════
// TelemetryReport
// ═══════════════════════════════════════════════════════════════════════════

/// A privacy-safe compilation telemetry report.
///
/// Contains only aggregate metrics — no source code, file paths, or
/// user-identifiable data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryReport {
    /// VUMA version string.
    pub version: String,
    /// Total wall-clock compilation time in milliseconds.
    pub total_time_ms: u64,
    /// Per-stage timing in milliseconds.
    pub stage_timings: HashMap<String, StageMetrics>,
    /// Peak memory usage in bytes (best-effort estimate).
    pub peak_memory_bytes: u64,
    /// Total number of errors encountered.
    pub error_count: usize,
    /// Total number of warnings encountered.
    pub warning_count: usize,
    /// Number of SCG nodes in the final graph.
    pub scg_node_count: usize,
    /// Number of IR functions generated.
    pub ir_function_count: usize,
    /// Number of IR instructions generated.
    pub ir_instruction_count: usize,
    /// Size of the emitted binary in bytes.
    pub binary_size_bytes: usize,
    /// Optimization level used.
    pub opt_level: String,
    /// Verification level used.
    pub verification_level: String,
    /// Whether debug info was included.
    pub debug_info: bool,
    /// Compilation target.
    pub target: String,
    /// Timestamp of the report (ISO 8601).
    pub timestamp: String,
}

/// Metrics for a single compilation stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageMetrics {
    /// Wall-clock time for this stage in milliseconds.
    pub time_ms: u64,
    /// Number of errors produced by this stage.
    pub error_count: usize,
    /// Number of warnings produced by this stage.
    pub warning_count: usize,
    /// Memory delta (approximate) in bytes during this stage.
    pub memory_delta_bytes: i64,
}

// ═══════════════════════════════════════════════════════════════════════════
// TelemetryCollector
// ═══════════════════════════════════════════════════════════════════════════

/// Collects telemetry metrics during compilation.
///
/// Usage:
/// 1. Create a new collector with [`TelemetryCollector::new`].
/// 2. Call [`TelemetryCollector::stage_start`] before each pipeline stage.
/// 3. Call [`TelemetryCollector::stage_end`] after each stage completes.
/// 4. Call [`TelemetryCollector::increment_error_count`] / [`TelemetryCollector::increment_warning_count`]
///    as errors/warnings are encountered.
/// 5. Call [`TelemetryCollector::finalize`] to produce the report.
pub struct TelemetryCollector {
    /// Per-stage in-flight timers.
    active_stages: HashMap<String, Instant>,
    /// Per-stage accumulated metrics.
    stage_metrics: HashMap<String, StageMetrics>,
    /// Total compilation start time.
    start_time: Instant,
    /// Global error count.
    error_count: usize,
    /// Global warning count.
    warning_count: usize,
    /// Peak memory (approximate).
    peak_memory_bytes: u64,
    /// Memory at start.
    start_memory_bytes: u64,
    /// SCG node count (set by caller).
    scg_node_count: usize,
    /// IR function count.
    ir_function_count: usize,
    /// IR instruction count.
    ir_instruction_count: usize,
    /// Binary size.
    binary_size_bytes: usize,
    /// Optimization level.
    opt_level: String,
    /// Verification level.
    verification_level: String,
    /// Debug info flag.
    debug_info: bool,
    /// Compilation target.
    target: String,
}

impl TelemetryCollector {
    /// Create a new telemetry collector, starting the global timer.
    pub fn new() -> Self {
        let start_memory_bytes = approximate_memory_usage();
        Self {
            active_stages: HashMap::new(),
            stage_metrics: HashMap::new(),
            start_time: Instant::now(),
            error_count: 0,
            warning_count: 0,
            peak_memory_bytes: start_memory_bytes,
            start_memory_bytes,
            scg_node_count: 0,
            ir_function_count: 0,
            ir_instruction_count: 0,
            binary_size_bytes: 0,
            opt_level: "O2".to_string(),
            verification_level: "normal".to_string(),
            debug_info: false,
            target: "linux".to_string(),
        }
    }

    /// Mark the start of a pipeline stage.
    pub fn stage_start(&mut self, stage: &str) {
        self.active_stages.insert(stage.to_string(), Instant::now());
    }

    /// Mark the end of a pipeline stage, recording its timing.
    pub fn stage_end(&mut self, stage: &str) {
        if let Some(start) = self.active_stages.remove(stage) {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let current_memory = approximate_memory_usage();
            if current_memory > self.peak_memory_bytes {
                self.peak_memory_bytes = current_memory;
            }
            let memory_delta = current_memory as i64 - self.start_memory_bytes as i64;

            self.stage_metrics.insert(
                stage.to_string(),
                StageMetrics {
                    time_ms: elapsed_ms,
                    error_count: 0,
                    warning_count: 0,
                    memory_delta_bytes: memory_delta,
                },
            );
        }
    }

    /// Increment the error count for a specific stage.
    pub fn increment_stage_error(&mut self, stage: &str) {
        self.error_count += 1;
        if let Some(metrics) = self.stage_metrics.get_mut(stage) {
            metrics.error_count += 1;
        }
    }

    /// Increment the warning count for a specific stage.
    pub fn increment_stage_warning(&mut self, stage: &str) {
        self.warning_count += 1;
        if let Some(metrics) = self.stage_metrics.get_mut(stage) {
            metrics.warning_count += 1;
        }
    }

    /// Increment the global error count.
    pub fn increment_error_count(&mut self) {
        self.error_count += 1;
    }

    /// Increment the global warning count.
    pub fn increment_warning_count(&mut self) {
        self.warning_count += 1;
    }

    /// Set the SCG node count.
    pub fn set_scg_node_count(&mut self, count: usize) {
        self.scg_node_count = count;
    }

    /// Set the IR function count.
    pub fn set_ir_function_count(&mut self, count: usize) {
        self.ir_function_count = count;
    }

    /// Set the IR instruction count.
    pub fn set_ir_instruction_count(&mut self, count: usize) {
        self.ir_instruction_count = count;
    }

    /// Set the binary size.
    pub fn set_binary_size(&mut self, size: usize) {
        self.binary_size_bytes = size;
    }

    /// Set the optimization level.
    pub fn set_opt_level(&mut self, level: &str) {
        self.opt_level = level.to_string();
    }

    /// Set the verification level.
    pub fn set_verification_level(&mut self, level: &str) {
        self.verification_level = level.to_string();
    }

    /// Set the debug info flag.
    pub fn set_debug_info(&mut self, enabled: bool) {
        self.debug_info = enabled;
    }

    /// Set the compilation target.
    pub fn set_target(&mut self, target: &str) {
        self.target = target.to_string();
    }

    /// Produce the final telemetry report.
    pub fn finalize(self) -> TelemetryReport {
        let total_time_ms = self.start_time.elapsed().as_millis() as u64;
        let timestamp = chrono::Utc::now().to_rfc3339();

        TelemetryReport {
            version: env!("CARGO_PKG_VERSION").to_string(),
            total_time_ms,
            stage_timings: self.stage_metrics,
            peak_memory_bytes: self.peak_memory_bytes,
            error_count: self.error_count,
            warning_count: self.warning_count,
            scg_node_count: self.scg_node_count,
            ir_function_count: self.ir_function_count,
            ir_instruction_count: self.ir_instruction_count,
            binary_size_bytes: self.binary_size_bytes,
            opt_level: self.opt_level,
            verification_level: self.verification_level,
            debug_info: self.debug_info,
            target: self.target,
            timestamp,
        }
    }
}

impl Default for TelemetryCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Memory estimation
// ═══════════════════════════════════════════════════════════════════════════

/// Best-effort approximation of current process memory usage in bytes.
///
/// On Linux, reads `/proc/self/status` for `VmRSS`.
/// On other platforms, returns 0 (not supported).
fn approximate_memory_usage() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    // Format: "VmRSS:    12345 kB"
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<u64>() {
                            return kb * 1024;
                        }
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

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_collector_basic() {
        let mut collector = TelemetryCollector::new();
        collector.set_opt_level("O2");
        collector.set_verification_level("normal");
        collector.set_target("linux");

        collector.stage_start("parse");
        std::thread::sleep(std::time::Duration::from_millis(1));
        collector.stage_end("parse");

        collector.stage_start("codegen");
        std::thread::sleep(std::time::Duration::from_millis(1));
        collector.stage_end("codegen");

        collector.increment_error_count();
        collector.increment_warning_count();
        collector.set_scg_node_count(42);
        collector.set_ir_function_count(3);
        collector.set_ir_instruction_count(100);
        collector.set_binary_size(2048);

        let report = collector.finalize();
        assert_eq!(report.error_count, 1);
        assert_eq!(report.warning_count, 1);
        assert_eq!(report.scg_node_count, 42);
        assert_eq!(report.ir_function_count, 3);
        assert_eq!(report.ir_instruction_count, 100);
        assert_eq!(report.binary_size_bytes, 2048);
        assert_eq!(report.opt_level, "O2");
        assert!(report.stage_timings.contains_key("parse"));
        assert!(report.stage_timings.contains_key("codegen"));
        assert!(report.stage_timings["parse"].time_ms >= 1);
    }

    #[test]
    fn test_telemetry_report_json_serialization() {
        let mut collector = TelemetryCollector::new();
        collector.stage_start("test-stage");
        collector.stage_end("test-stage");

        let report = collector.finalize();
        let json = serde_json::to_string(&report).unwrap();
        assert!(json.contains("version"));
        assert!(json.contains("total_time_ms"));
        assert!(json.contains("error_count"));
        assert!(json.contains("stage_timings"));
        assert!(json.contains("timestamp"));

        // Verify no source code leaks into telemetry
        assert!(!json.contains("source"));
        assert!(!json.contains("file_path"));
    }

    #[test]
    fn test_telemetry_stage_errors() {
        let mut collector = TelemetryCollector::new();
        collector.stage_start("parse");
        collector.increment_stage_error("parse");
        collector.increment_stage_error("parse");
        collector.increment_stage_warning("parse");
        collector.stage_end("parse");

        let report = collector.finalize();
        assert_eq!(report.error_count, 2);
        assert_eq!(report.warning_count, 1);
        assert_eq!(report.stage_timings["parse"].error_count, 2);
        assert_eq!(report.stage_timings["parse"].warning_count, 1);
    }

    #[test]
    fn test_telemetry_report_pretty_json() {
        let collector = TelemetryCollector::new();
        let report = collector.finalize();
        let pretty = serde_json::to_string_pretty(&report).unwrap();
        assert!(pretty.contains("\"version\""));
        assert!(pretty.contains("\"total_time_ms\""));
    }
}
