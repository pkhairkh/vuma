//! Profile-guided optimization data collection and analysis.
//!
//! The profiling subsystem continuously collects runtime observations — branch
//! directions, call frequencies, edge traversal counts, hardware performance
//! counter values — and exposes methods to identify hot paths, cold spots, and
//! generate optimization recommendations.
//!
//! # Design notes
//!
//! * **Always-on profiling**: COR continuously collects profile data as the
//!   program executes. There is no separate "profiled run"; the data
//!   converges incrementally.
//! * **Low overhead**: Counters are simple `u64` bumps; no heap allocation
//!   occurs on the fast path.
//! * **Region-class granularity**: Allocation statistics are partitioned by
//!   *region class* (a coarse grouping of compiled regions) so the optimizer
//!   can make decisions without examining every individual region.
//! * **Pi 5 PMU support**: Hardware performance counters (cycles, instructions,
//!   cache misses, branch misses) are collected per sample when running on
//!   Raspberry Pi 5, enabling micro-architectural optimisation.
//!
//! # Core types
//!
//! * [`ProfileCollector`] — thread-safe collector that accumulates profiling
//!   samples and produces [`ProfileData`].
//! * [`ProfileData`] — execution counts per SCG node, edge traversal
//!   frequencies, and hot-path information.
//! * [`ProfileSample`] — a single profiling sample: timestamp, node id,
//!   execution time in nanoseconds, and optional PMU counters.
//! * [`HotPath`] — a sequence of nodes that collectively account for >80 %
//!   of total execution time.
//! * [`ProfileReport`] — a full analysis of profiling data including hot
//!   spots, cold spots, and optimisation recommendations.

use crate::types::{EdgeId, NodeId, SCG};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

// ---------------------------------------------------------------------------
// Pi 5 Hardware Performance Counters (PMU)
// ---------------------------------------------------------------------------

/// Raspberry Pi 5 hardware performance counter snapshot.
///
/// The Pi 5's Cortex-A76 cores expose PMU counters via the `cntvct_el0`
/// virtual counter and the ARMv8.2-A PMU event system. This struct captures
/// the four most useful events for optimisation guidance:
///
/// * **Cycle count** — total CPU cycles elapsed (via `PMCCNTR_EL0`).
/// * **Instruction count** — instructions architecturally executed.
/// * **Cache misses** — L1 data cache refills from main memory.
/// * **Branch misses** — branches mispredicted at the branch-prediction unit.
///
/// On targets where PMU counters are unavailable (e.g. user-space without
/// `perf_event_open` access), all fields default to zero and are simply
/// ignored by the analysis pipeline.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Pi5PmuCounters {
    /// Total CPU cycles elapsed during the sampled region.
    pub cycle_count: u64,
    /// Instructions architecturally executed.
    pub instruction_count: u64,
    /// L1 data-cache misses (refills from DRAM).
    pub cache_misses: u64,
    /// Branch mispredictions.
    pub branch_misses: u64,
}

impl Pi5PmuCounters {
    /// Creates a zeroed PMU counter snapshot.
    pub fn new() -> Self {
        Self::default()
    }

    /// Computes instructions-per-cycle (IPC). Returns 0.0 if cycle_count is 0.
    pub fn ipc(&self) -> f64 {
        if self.cycle_count == 0 {
            0.0
        } else {
            self.instruction_count as f64 / self.cycle_count as f64
        }
    }

    /// Computes cache miss rate (misses / instructions). Returns 0.0 if
    /// instruction_count is 0.
    pub fn cache_miss_rate(&self) -> f64 {
        if self.instruction_count == 0 {
            0.0
        } else {
            self.cache_misses as f64 / self.instruction_count as f64
        }
    }

    /// Computes branch misprediction rate. Returns 0.0 if instruction_count is 0.
    pub fn branch_miss_rate(&self) -> f64 {
        if self.instruction_count == 0 {
            0.0
        } else {
            self.branch_misses as f64 / self.instruction_count as f64
        }
    }
}

// ---------------------------------------------------------------------------
// AllocStats
// ---------------------------------------------------------------------------

/// Statistics about memory allocation behaviour within the runtime.
///
/// `AllocStats` tracks aggregate allocation counts as well as per-region-class
/// breakdowns, enabling the optimizer to identify allocation-heavy regions and
/// suggest strategies such as arena allocation or object pooling.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AllocStats {
    /// Total number of heap allocations observed.
    pub total_allocations: u64,
    /// Total number of heap frees observed.
    pub total_frees: u64,
    /// Peak observed live heap usage (in bytes).
    pub peak_usage: u64,
    /// Allocation statistics partitioned by region class.
    ///
    /// The key is a human-readable region class label (e.g. `"hot_loop"`,
    /// `"cold_path"`); the value is the number of allocations attributed to
    /// that class.
    pub by_region_class: HashMap<String, u64>,
}

impl AllocStats {
    /// Creates an empty `AllocStats`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a single allocation of `size` bytes in the given region class.
    pub fn record_alloc(&mut self, size: u64, region_class: &str) {
        self.total_allocations += 1;
        *self
            .by_region_class
            .entry(region_class.to_owned())
            .or_insert(0) += 1;
        // A simple peak-tracking heuristic: each alloc adds to live set.
        // In a real implementation we'd track the current live set precisely.
        let live = self.total_allocations.saturating_sub(self.total_frees);
        let estimated_usage = live * size; // rough estimate
        if estimated_usage > self.peak_usage {
            self.peak_usage = estimated_usage;
        }
    }

    /// Records a single free in the given region class.
    pub fn record_free(&mut self, region_class: &str) {
        self.total_frees += 1;
        // We don't decrement by_region_class since that tracks cumulative
        // allocations per class, not live objects.
        let _ = region_class; // acknowledged
    }

    /// Returns the current number of live allocations (approximate).
    pub fn live_allocations(&self) -> u64 {
        self.total_allocations.saturating_sub(self.total_frees)
    }
}

// ---------------------------------------------------------------------------
// ProfileSample
// ---------------------------------------------------------------------------

/// A single profiling sample captured during execution.
///
/// Each sample records the SCG node that was executing, how long it took,
/// when it was captured, and (optionally) Pi 5 hardware PMU counter values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSample {
    /// Timestamp of the sample, measured as nanoseconds since profiling started.
    pub timestamp_ns: u64,
    /// The SCG node that was executing when the sample was taken.
    pub node_id: NodeId,
    /// Wall-clock execution time of the node in nanoseconds.
    pub execution_time_ns: u64,
    /// Optional Pi 5 hardware performance counter snapshot.
    pub pmu: Option<Pi5PmuCounters>,
}

impl ProfileSample {
    /// Creates a new profile sample with the given timestamp, node, and
    /// execution time (no PMU counters).
    pub fn new(timestamp_ns: u64, node_id: NodeId, execution_time_ns: u64) -> Self {
        Self {
            timestamp_ns,
            node_id,
            execution_time_ns,
            pmu: None,
        }
    }

    /// Creates a profile sample with PMU counters attached.
    pub fn with_pmu(
        timestamp_ns: u64,
        node_id: NodeId,
        execution_time_ns: u64,
        pmu: Pi5PmuCounters,
    ) -> Self {
        Self {
            timestamp_ns,
            node_id,
            execution_time_ns,
            pmu: Some(pmu),
        }
    }
}

// ---------------------------------------------------------------------------
// HotPath
// ---------------------------------------------------------------------------

/// A hot path: a sequence of SCG nodes that collectively account for more
/// than 80 % of total observed execution time.
///
/// The nodes are ordered by descending execution time (hottest first). The
/// `total_time_ns` field records the sum of execution times across all nodes
/// in the path, while `cumulative_fraction` records what fraction of the
/// *global* execution time this path represents.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotPath {
    /// Nodes on the hot path, sorted hottest first.
    pub nodes: Vec<NodeId>,
    /// Total execution time (ns) of all nodes on this path.
    pub total_time_ns: u64,
    /// Fraction of global execution time accounted for (0.0 – 1.0).
    pub cumulative_fraction: f64,
}

impl Default for HotPath {
    fn default() -> Self {
        Self::new()
    }
}

impl HotPath {
    /// Creates an empty hot path.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            total_time_ns: 0,
            cumulative_fraction: 0.0,
        }
    }

    /// Returns `true` if the hot path accounts for >80 % of execution time.
    pub fn is_dominant(&self) -> bool {
        self.cumulative_fraction > 0.80
    }
}

// ---------------------------------------------------------------------------
// ProfileData
// ---------------------------------------------------------------------------

/// Collected profile data used to guide runtime optimizations.
///
/// The struct is updated continuously as the program executes. The optimizer
/// periodically reads it to decide which regions to recompile at higher
/// optimization levels or speculatively specialize.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileData {
    /// Per-node execution counts (how many times each SCG node was entered).
    pub call_counts: HashMap<NodeId, u64>,

    /// Per-edge traversal frequencies (how many times each SCG edge was taken).
    pub edge_frequencies: HashMap<EdgeId, u64>,

    /// Per-node cumulative execution time in nanoseconds.
    pub node_time_ns: HashMap<NodeId, u64>,

    /// Aggregated Pi 5 PMU counters per node.
    pub node_pmu: HashMap<NodeId, Pi5PmuCounters>,

    /// Allocation statistics.
    pub allocation_stats: AllocStats,

    /// Hot paths cache (recomputed on demand).
    hot_paths_cache: Vec<HotPath>,
}

impl ProfileData {
    /// Creates empty profile data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an access (execution) of the given node with optional execution
    /// time.
    pub fn record_access(&mut self, node_id: NodeId) {
        let count = self.call_counts.entry(node_id).or_insert(0);
        *count += 1;
    }

    /// Records an execution of the given node with a known execution time.
    pub fn record_access_timed(&mut self, node_id: NodeId, time_ns: u64) {
        let count = self.call_counts.entry(node_id).or_insert(0);
        *count += 1;
        let time = self.node_time_ns.entry(node_id).or_insert(0);
        *time += time_ns;
    }

    /// Records a call (invocation) of the given node, equivalent to
    /// `record_access` but with a distinct semantic to support future
    /// call-graph construction.
    pub fn record_call(&mut self, node_id: NodeId) {
        let count = self.call_counts.entry(node_id).or_insert(0);
        *count += 1;
    }

    /// Records a traversal of the given SCG edge.
    pub fn record_edge(&mut self, edge_id: EdgeId) {
        let freq = self.edge_frequencies.entry(edge_id).or_insert(0);
        *freq += 1;
    }

    /// Records PMU counter values for a node (accumulates into existing values).
    pub fn record_pmu(&mut self, node_id: NodeId, pmu: &Pi5PmuCounters) {
        let entry = self.node_pmu.entry(node_id).or_default();
        entry.cycle_count += pmu.cycle_count;
        entry.instruction_count += pmu.instruction_count;
        entry.cache_misses += pmu.cache_misses;
        entry.branch_misses += pmu.branch_misses;
    }

    /// Ingests a batch of [`ProfileSample`] values, updating all internal
    /// counters.
    pub fn ingest_samples(&mut self, samples: &[ProfileSample]) {
        for sample in samples {
            self.record_access_timed(sample.node_id, sample.execution_time_ns);
            if let Some(ref pmu) = sample.pmu {
                self.record_pmu(sample.node_id, pmu);
            }
        }
    }

    /// Computes the total execution time across all nodes (ns).
    pub fn total_execution_time_ns(&self) -> u64 {
        self.node_time_ns.values().sum()
    }

    /// Returns hot paths whose nodes collectively account for >80 % of total
    /// execution time.
    ///
    /// The algorithm sorts all nodes by descending cumulative execution time
    /// and greedily adds nodes to the hot path until the 80 % threshold is
    /// crossed.
    pub fn compute_hot_paths(&mut self) -> Vec<HotPath> {
        let total = self.total_execution_time_ns();
        if total == 0 {
            self.hot_paths_cache = Vec::new();
            return Vec::new();
        }

        // Sort nodes by cumulative time, descending.
        let mut nodes: Vec<(NodeId, u64)> = self
            .node_time_ns
            .iter()
            .map(|(&n, &t)| (n, t))
            .collect();
        nodes.sort_by_key(|b| std::cmp::Reverse(b.1));

        let mut accumulated: u64 = 0;
        let mut path_nodes: Vec<NodeId> = Vec::new();

        for (node_id, time_ns) in &nodes {
            path_nodes.push(*node_id);
            accumulated += time_ns;
            let fraction = accumulated as f64 / total as f64;
            if fraction > 0.80 {
                let hot_path = HotPath {
                    nodes: path_nodes.clone(),
                    total_time_ns: accumulated,
                    cumulative_fraction: fraction,
                };
                self.hot_paths_cache = vec![hot_path];
                return self.hot_paths_cache.clone();
            }
        }

        // If we get here, even all nodes together don't exceed 80 % (edge
        // case with very few samples). Return whatever we have.
        let fraction = accumulated as f64 / total as f64;
        let hot_path = HotPath {
            nodes: path_nodes,
            total_time_ns: accumulated,
            cumulative_fraction: fraction,
        };
        self.hot_paths_cache = vec![hot_path];
        self.hot_paths_cache.clone()
    }

    /// Returns the top `k` hottest nodes by access count, sorted descending.
    pub fn get_hot_paths(&mut self, k: usize) -> Vec<(NodeId, u64)> {
        let mut pairs: Vec<(NodeId, u64)> =
            self.call_counts.iter().map(|(&n, &c)| (n, c)).collect();
        pairs.sort_by_key(|b| std::cmp::Reverse(b.1));
        pairs.truncate(k);
        pairs
    }

    /// Returns nodes with zero or negligible execution (cold spots).
    ///
    /// A node is considered "cold" if it has a call count of zero or its
    /// cumulative execution time is less than `cold_threshold_ns`.
    pub fn cold_spots(&self, all_nodes: &[NodeId], cold_threshold_ns: u64) -> Vec<NodeId> {
        all_nodes
            .iter()
            .copied()
            .filter(|&n| {
                let count = self.call_counts.get(&n).copied().unwrap_or(0);
                let time = self.node_time_ns.get(&n).copied().unwrap_or(0);
                count == 0 || time < cold_threshold_ns
            })
            .collect()
    }

    /// Returns a list of optimization suggestions based on the collected
    /// profile data.
    pub fn suggest_optimizations(&self) -> Vec<OptimizationSuggestion> {
        let mut suggestions = Vec::new();

        // Suggest inlining for the top 3 hottest nodes.
        let mut nodes: Vec<(NodeId, u64)> = self
            .call_counts
            .iter()
            .map(|(&n, &c)| (n, c))
            .collect();
        nodes.sort_by_key(|b| std::cmp::Reverse(b.1));
        for (node_id, count) in nodes.iter().take(3) {
            if *count > 100 {
                suggestions.push(OptimizationSuggestion {
                    kind: SuggestionKind::Inline,
                    target_node: *node_id,
                    reason: format!("Node called {} times — candidate for inlining", count),
                });
            }
        }

        // Suggest arena allocation for high-allocation region classes.
        for (class, allocs) in &self.allocation_stats.by_region_class {
            if *allocs > 50 {
                suggestions.push(OptimizationSuggestion {
                    kind: SuggestionKind::ArenaAlloc,
                    target_node: 0, // no specific node; applies to class
                    reason: format!(
                        "Region class '{}' has {} allocations — consider arena allocation",
                        class, allocs
                    ),
                });
            }
        }

        // Suggest cache optimisation for nodes with high cache miss rates.
        for (&node_id, pmu) in &self.node_pmu {
            if pmu.cache_miss_rate() > 0.05 {
                suggestions.push(OptimizationSuggestion {
                    kind: SuggestionKind::CacheOptimize,
                    target_node: node_id,
                    reason: format!(
                        "Node {} has {:.1}% cache miss rate — consider data layout optimisation",
                        node_id,
                        pmu.cache_miss_rate() * 100.0
                    ),
                });
            }
        }

        // Suggest branch prediction layout for high branch miss rates.
        for (&node_id, pmu) in &self.node_pmu {
            if pmu.branch_miss_rate() > 0.03 {
                suggestions.push(OptimizationSuggestion {
                    kind: SuggestionKind::BranchLayout,
                    target_node: node_id,
                    reason: format!(
                        "Node {} has {:.1}% branch misprediction rate — consider branch layout hints",
                        node_id,
                        pmu.branch_miss_rate() * 100.0
                    ),
                });
            }
        }

        suggestions
    }
}

// ---------------------------------------------------------------------------
// ProfileCollector
// ---------------------------------------------------------------------------

/// Thread-safe runtime profiling data collector.
///
/// `ProfileCollector` is the central object through which the COR records
/// profiling observations. It wraps a [`ProfileData`] instance in a
/// [`Mutex`] so it can be shared across threads, and provides convenience
/// methods for recording samples and edges.
///
/// # Usage
///
/// ```no_run
/// use vuma_cor::profile::ProfileCollector;
/// use vuma_cor::types::NodeId;
///
/// let collector = ProfileCollector::new();
/// collector.record_access(42);
/// collector.record_edge(7);
/// let data = collector.snapshot();
/// ```
pub struct ProfileCollector {
    /// Inner profile data, protected by a mutex.
    data: Mutex<ProfileData>,
    /// Nanosecond origin used for computing relative timestamps.
    epoch: Instant,
    /// Atomic counter for total number of samples collected (fast-path read).
    sample_count: AtomicU64,
}

impl ProfileCollector {
    /// Creates a new, empty profile collector.
    pub fn new() -> Self {
        Self {
            data: Mutex::new(ProfileData::new()),
            epoch: Instant::now(),
            sample_count: AtomicU64::new(0),
        }
    }

    /// Returns the number of samples collected so far.
    pub fn sample_count(&self) -> u64 {
        self.sample_count.load(Ordering::Relaxed)
    }

    /// Records an execution of the given SCG node.
    pub fn record_access(&self, node_id: NodeId) {
        let mut data = self.data.lock().unwrap();
        data.record_access(node_id);
        self.sample_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records an execution of the given node with timing information.
    pub fn record_access_timed(&self, node_id: NodeId, time_ns: u64) {
        let mut data = self.data.lock().unwrap();
        data.record_access_timed(node_id, time_ns);
        self.sample_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records a traversal of the given SCG edge.
    pub fn record_edge(&self, edge_id: EdgeId) {
        let mut data = self.data.lock().unwrap();
        data.record_edge(edge_id);
    }

    /// Records a full [`ProfileSample`].
    pub fn record_sample(&self, sample: &ProfileSample) {
        let mut data = self.data.lock().unwrap();
        data.record_access_timed(sample.node_id, sample.execution_time_ns);
        if let Some(ref pmu) = sample.pmu {
            data.record_pmu(sample.node_id, pmu);
        }
        self.sample_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Records PMU counter values for a node.
    pub fn record_pmu(&self, node_id: NodeId, pmu: &Pi5PmuCounters) {
        let mut data = self.data.lock().unwrap();
        data.record_pmu(node_id, pmu);
    }

    /// Creates a [`ProfileSample`] using the collector's internal clock as
    /// the timestamp.
    pub fn make_sample(&self, node_id: NodeId, execution_time_ns: u64) -> ProfileSample {
        let timestamp_ns = self.epoch.elapsed().as_nanos() as u64;
        ProfileSample::new(timestamp_ns, node_id, execution_time_ns)
    }

    /// Creates a [`ProfileSample`] with PMU counters using the collector's
    /// internal clock.
    pub fn make_sample_with_pmu(
        &self,
        node_id: NodeId,
        execution_time_ns: u64,
        pmu: Pi5PmuCounters,
    ) -> ProfileSample {
        let timestamp_ns = self.epoch.elapsed().as_nanos() as u64;
        ProfileSample::with_pmu(timestamp_ns, node_id, execution_time_ns, pmu)
    }

    /// Takes a snapshot of the current profile data.
    ///
    /// This clones the internal `ProfileData`, so the caller gets a
    /// consistent view while the collector continues to accumulate.
    pub fn snapshot(&self) -> ProfileData {
        self.data.lock().unwrap().clone()
    }

    /// Resets all collected profile data.
    pub fn reset(&self) {
        let mut data = self.data.lock().unwrap();
        *data = ProfileData::new();
        self.sample_count.store(0, Ordering::Relaxed);
        let _ = self.epoch; // epoch stays the same — timestamps remain monotonic
    }
}

impl Default for ProfileCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// ProfileReport
// ---------------------------------------------------------------------------

/// A comprehensive analysis report produced from profiling data.
///
/// `ProfileReport` is the output of [`collect_profile`] and contains hot
/// spots, cold spots, hot paths, and optimisation recommendations derived
/// from the raw profile samples and SCG structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileReport {
    /// Total number of samples analysed.
    pub total_samples: usize,
    /// Total execution time across all sampled nodes (ns).
    pub total_execution_time_ns: u64,
    /// Nodes ranked by execution time (hottest first), with their cumulative
    /// time and call count.
    pub hot_spots: Vec<NodeHotSpot>,
    /// Nodes with zero or negligible execution.
    pub cold_spots: Vec<NodeId>,
    /// Hot paths (node sequences accounting for >80 % of execution time).
    pub hot_paths: Vec<HotPath>,
    /// Aggregate Pi 5 PMU counters across all samples.
    pub aggregate_pmu: Pi5PmuCounters,
    /// Per-node PMU counter summaries.
    pub node_pmu: HashMap<NodeId, Pi5PmuCounters>,
    /// Optimisation suggestions derived from the analysis.
    pub recommendations: Vec<OptimizationSuggestion>,
}

/// A hot spot: a single node with its execution statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHotSpot {
    /// The SCG node identifier.
    pub node_id: NodeId,
    /// Number of times this node was executed.
    pub call_count: u64,
    /// Cumulative execution time (ns) for this node.
    pub total_time_ns: u64,
    /// Fraction of global execution time (0.0 – 1.0).
    pub time_fraction: f64,
}

// ---------------------------------------------------------------------------
// collect_profile — main analysis entry point
// ---------------------------------------------------------------------------

/// Analyses a batch of profile samples against the SCG and produces a
/// [`ProfileReport`].
///
/// This is the primary entry point for profile-guided analysis. It:
///
/// 1. Ingests all samples into a [`ProfileData`] instance.
/// 2. Computes hot spots (nodes ranked by execution time).
/// 3. Identifies cold spots (nodes with zero or negligible execution).
/// 4. Computes hot paths (node sequences accounting for >80 % of time).
/// 5. Aggregates PMU counter data.
/// 6. Generates optimisation recommendations.
pub fn collect_profile(scg: &SCG, samples: &[ProfileSample]) -> ProfileReport {
    let mut profile_data = ProfileData::new();
    profile_data.ingest_samples(samples);

    let total_time = profile_data.total_execution_time_ns();

    // --- Hot spots ---
    let mut hot_spots: Vec<NodeHotSpot> = profile_data
        .node_time_ns
        .iter()
        .map(|(&node_id, &time_ns)| {
            let call_count = profile_data.call_counts.get(&node_id).copied().unwrap_or(0);
            let time_fraction = if total_time > 0 {
                time_ns as f64 / total_time as f64
            } else {
                0.0
            };
            NodeHotSpot {
                node_id,
                call_count,
                total_time_ns: time_ns,
                time_fraction,
            }
        })
        .collect();
    hot_spots.sort_by_key(|b| std::cmp::Reverse(b.total_time_ns));

    // --- Cold spots ---
    // Build a list of all node IDs from 0..scg.node_count (in the full
    // implementation the SCG would provide an iterator).
    let all_nodes: Vec<NodeId> = (0..scg.node_count as u64).collect();
    let cold_threshold_ns = if total_time > 0 { total_time / 1000 } else { 1 };
    let cold_spots = profile_data.cold_spots(&all_nodes, cold_threshold_ns);

    // --- Hot paths ---
    let hot_paths = profile_data.compute_hot_paths();

    // --- Aggregate PMU ---
    let mut aggregate_pmu = Pi5PmuCounters::new();
    for pmu in profile_data.node_pmu.values() {
        aggregate_pmu.cycle_count += pmu.cycle_count;
        aggregate_pmu.instruction_count += pmu.instruction_count;
        aggregate_pmu.cache_misses += pmu.cache_misses;
        aggregate_pmu.branch_misses += pmu.branch_misses;
    }

    // --- Recommendations ---
    let recommendations = profile_data.suggest_optimizations();

    ProfileReport {
        total_samples: samples.len(),
        total_execution_time_ns: total_time,
        hot_spots,
        cold_spots,
        hot_paths,
        aggregate_pmu,
        node_pmu: profile_data.node_pmu.clone(),
        recommendations,
    }
}

// ---------------------------------------------------------------------------
// Optimization suggestion types
// ---------------------------------------------------------------------------

/// The kind of optimization being suggested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SuggestionKind {
    /// Inline the node's body at its call sites.
    Inline,
    /// Replace per-call heap allocations with an arena.
    ArenaAlloc,
    /// Speculatively specialize generic code.
    Specialize,
    /// Optimise data layout for better cache utilisation.
    CacheOptimize,
    /// Reorder branches to favour the common path.
    BranchLayout,
}

/// A single optimization suggestion produced by profile analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationSuggestion {
    /// The kind of optimization.
    pub kind: SuggestionKind,
    /// The node that would be affected (0 if the suggestion is region-wide).
    pub target_node: NodeId,
    /// Human-readable explanation.
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NodeKind, SCGEdge, SCGNode};

    #[test]
    fn record_access_increments_count() {
        let mut profile = ProfileData::new();
        profile.record_access(42);
        profile.record_access(42);
        profile.record_access(7);
        assert_eq!(profile.call_counts[&42], 2);
        assert_eq!(profile.call_counts[&7], 1);
    }

    #[test]
    fn get_hot_paths_returns_top_k() {
        let mut profile = ProfileData::new();
        for _ in 0..200 {
            profile.record_access(1);
        }
        for _ in 0..100 {
            profile.record_access(2);
        }
        for _ in 0..50 {
            profile.record_access(3);
        }
        let hot = profile.get_hot_paths(2);
        assert_eq!(hot.len(), 2);
        assert_eq!(hot[0], (1, 200));
        assert_eq!(hot[1], (2, 100));
    }

    #[test]
    fn alloc_stats_peak_tracking() {
        let mut stats = AllocStats::new();
        stats.record_alloc(64, "hot_loop");
        stats.record_alloc(64, "hot_loop");
        assert_eq!(stats.total_allocations, 2);
        assert!(stats.peak_usage > 0);
    }

    #[test]
    fn suggest_optimizations_inline() {
        let mut profile = ProfileData::new();
        for _ in 0..200 {
            profile.record_call(99);
        }
        let suggestions = profile.suggest_optimizations();
        assert!(
            suggestions
                .iter()
                .any(|s| s.kind == SuggestionKind::Inline && s.target_node == 99)
        );
    }

    #[test]
    fn profile_sample_creation_and_pmu() {
        let sample = ProfileSample::new(1000, 42, 500);
        assert_eq!(sample.timestamp_ns, 1000);
        assert_eq!(sample.node_id, 42);
        assert_eq!(sample.execution_time_ns, 500);
        assert!(sample.pmu.is_none());

        let pmu = Pi5PmuCounters {
            cycle_count: 10000,
            instruction_count: 8000,
            cache_misses: 50,
            branch_misses: 10,
        };
        let sample_with_pmu = ProfileSample::with_pmu(2000, 43, 600, pmu.clone());
        assert!(sample_with_pmu.pmu.is_some());
        let p = sample_with_pmu.pmu.unwrap();
        assert_eq!(p.cycle_count, 10000);
        assert_eq!(p.instruction_count, 8000);
    }

    #[test]
    fn hot_path_dominance_threshold() {
        let mut profile = ProfileData::new();
        // Node 1 takes 85% of time, node 2 takes 15%
        profile.record_access_timed(1, 8500);
        profile.record_access_timed(2, 1500);
        let hot_paths = profile.compute_hot_paths();
        assert_eq!(hot_paths.len(), 1);
        assert!(hot_paths[0].is_dominant());
        assert!(hot_paths[0].nodes.contains(&1));
        // Node 1 alone exceeds 80%, so the hot path should contain just node 1
        assert_eq!(hot_paths[0].nodes.len(), 1);
        assert_eq!(hot_paths[0].nodes[0], 1);
        assert!((hot_paths[0].cumulative_fraction - 0.85).abs() < 0.01);
    }

    #[test]
    fn profile_collector_thread_safe() {
        use std::sync::Arc;
        use std::thread;

        let collector = Arc::new(ProfileCollector::new());
        let mut handles = Vec::new();

        for i in 0..4 {
            let c = Arc::clone(&collector);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    c.record_access(i * 100 + j);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let data = collector.snapshot();
        // 4 threads × 50 accesses = 200 total
        let total: u64 = data.call_counts.values().sum();
        assert_eq!(total, 200);
        assert_eq!(collector.sample_count(), 200);
    }

    #[test]
    fn collect_profile_produces_report() {
        let mut scg = SCG::new();
        for i in 0..5u64 {
            scg.insert_node(SCGNode::new(i, NodeKind::Compute));
        }
        for i in 0..4u64 {
            scg.insert_edge(SCGEdge::new(i, i, i + 1));
        }

        let samples = vec![
            ProfileSample::new(100, 0, 5000),
            ProfileSample::new(200, 0, 5000),
            ProfileSample::new(300, 1, 3000),
            ProfileSample::new(400, 2, 2000),
        ];

        let report = collect_profile(&scg, &samples);

        assert_eq!(report.total_samples, 4);
        assert_eq!(report.total_execution_time_ns, 15000);
        assert!(!report.hot_spots.is_empty());

        // Node 0 should be the hottest (10000 ns)
        assert_eq!(report.hot_spots[0].node_id, 0);
        assert_eq!(report.hot_spots[0].total_time_ns, 10000);

        // Cold spots should include nodes 3 and 4 (not in samples)
        assert!(report.cold_spots.contains(&3));
        assert!(report.cold_spots.contains(&4));
    }

    #[test]
    fn pi5_pmu_counters_ipc_and_rates() {
        let pmu = Pi5PmuCounters {
            cycle_count: 1000,
            instruction_count: 500,
            cache_misses: 25,
            branch_misses: 10,
        };

        assert!((pmu.ipc() - 0.5).abs() < 0.001);
        assert!((pmu.cache_miss_rate() - 0.05).abs() < 0.001);
        assert!((pmu.branch_miss_rate() - 0.02).abs() < 0.001);

        // Zero-cycle edge case
        let zero_pmu = Pi5PmuCounters::new();
        assert_eq!(zero_pmu.ipc(), 0.0);
        assert_eq!(zero_pmu.cache_miss_rate(), 0.0);
        assert_eq!(zero_pmu.branch_miss_rate(), 0.0);
    }

    #[test]
    fn edge_frequencies_recorded() {
        let mut profile = ProfileData::new();
        profile.record_edge(10);
        profile.record_edge(10);
        profile.record_edge(20);
        assert_eq!(profile.edge_frequencies[&10], 2);
        assert_eq!(profile.edge_frequencies[&20], 1);
    }

    #[test]
    fn ingest_samples_accumulates_pmu() {
        let mut profile = ProfileData::new();
        let pmu1 = Pi5PmuCounters {
            cycle_count: 100,
            instruction_count: 80,
            cache_misses: 5,
            branch_misses: 2,
        };
        let pmu2 = Pi5PmuCounters {
            cycle_count: 200,
            instruction_count: 160,
            cache_misses: 10,
            branch_misses: 3,
        };
        let samples = vec![
            ProfileSample::with_pmu(100, 1, 500, pmu1),
            ProfileSample::with_pmu(200, 1, 600, pmu2),
        ];
        profile.ingest_samples(&samples);

        assert_eq!(profile.call_counts[&1], 2);
        assert_eq!(profile.node_time_ns[&1], 1100);
        let node_pmu = &profile.node_pmu[&1];
        assert_eq!(node_pmu.cycle_count, 300);
        assert_eq!(node_pmu.instruction_count, 240);
        assert_eq!(node_pmu.cache_misses, 15);
        assert_eq!(node_pmu.branch_misses, 5);
    }
}
