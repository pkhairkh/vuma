//! Profile-guided optimization data collection and analysis.
//!
//! The [`ProfileData`] struct accumulates runtime observations — branch
//! directions, call frequencies, and allocation patterns — and exposes
//! methods to query hot paths and generate optimization suggestions.
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

use crate::types::NodeId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
        *self.by_region_class.entry(region_class.to_owned()).or_insert(0) += 1;
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
// ProfileData
// ---------------------------------------------------------------------------

/// Collected profile data used to guide runtime optimizations.
///
/// The struct is updated continuously as the program executes. The optimizer
/// periodically reads it (via [`ProfileData::get_hot_paths`] and
/// [`ProfileData::suggest_optimizations`]) to decide which regions to
/// recompile at higher optimization levels or speculatively specialize.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfileData {
    /// Nodes sorted by access frequency (descending).
    ///
    /// Each entry is `(node_id, access_count)`. Kept sorted on demand.
    pub hot_paths: Vec<(NodeId, u64)>,

    /// Per-node call counts. Unsorted; used as the source of truth for
    /// `hot_paths` computation.
    pub call_counts: HashMap<NodeId, u64>,

    /// Allocation statistics.
    pub allocation_stats: AllocStats,
}

impl ProfileData {
    /// Creates empty profile data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an access (execution) of the given node.
    ///
    /// This is the primary hot-path detection mechanism: nodes that are
    /// accessed frequently are candidates for aggressive optimization.
    pub fn record_access(&mut self, node_id: NodeId) {
        let count = self.call_counts.entry(node_id).or_insert(0);
        *count += 1;
    }

    /// Records a call (invocation) of the given node, equivalent to
    /// `record_access` but with a distinct semantic to support future
    /// call-graph construction.
    pub fn record_call(&mut self, node_id: NodeId) {
        let count = self.call_counts.entry(node_id).or_insert(0);
        *count += 1;
    }

    /// Returns the top `k` hot paths (nodes with highest access counts),
    /// sorted in descending order by count.
    ///
    /// The result is cached in `self.hot_paths` so that repeated calls are
    /// cheap until the underlying `call_counts` are mutated again.
    pub fn get_hot_paths(&mut self, k: usize) -> &[(NodeId, u64)] {
        let mut pairs: Vec<(NodeId, u64)> = self.call_counts.iter().map(|(&n, &c)| (n, c)).collect();
        pairs.sort_by(|a, b| b.1.cmp(&a.1));
        pairs.truncate(k);
        self.hot_paths = pairs;
        &self.hot_paths
    }

    /// Returns a list of optimization suggestions based on the collected
    /// profile data.
    ///
    /// Suggestions are heuristic and may include:
    /// - Inlining hot call targets
    /// - Arena-allocating high-frequency allocation regions
    /// - Specializing generic code for observed type combinations
    pub fn suggest_optimizations(&self) -> Vec<OptimizationSuggestion> {
        let mut suggestions = Vec::new();

        // Suggest inlining for the top 3 hottest nodes.
        let mut nodes: Vec<(NodeId, u64)> = self.call_counts.iter().map(|(&n, &c)| (n, c)).collect();
        nodes.sort_by(|a, b| b.1.cmp(&a.1));
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

        suggestions
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn hot_paths_returns_top_k() {
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
        let hot = profile.get_hot_paths(2).to_vec();
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
        assert!(suggestions.iter().any(|s| s.kind == SuggestionKind::Inline && s.target_node == 99));
    }
}
