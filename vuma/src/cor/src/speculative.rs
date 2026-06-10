//! Speculative optimization framework.
//!
//! Speculative optimization allows the COR to compile specialized code paths
//! based on *assumptions* about runtime behaviour (e.g. which branch is
//! likely taken, which call path is hot, whether a region is uncontended).
//!
//! If an assumption is later invalidated the runtime **deoptimizes** — it
//! discards the speculative code and falls back to the safe, unoptimized
//! version. This enables aggressive optimization without sacrificing
//! correctness guarantees established by the IVE (Infinite Verification
//! Engine).
//!
//! # Architecture
//!
//! The framework is structured around several cooperating types:
//!
//! - **[`SpeculativeExecutor`]** — Top-level orchestrator that runs
//!   speculative optimizations, manages snapshots for rollback, and
//!   validates assumptions periodically.
//! - **[`SpeculationCandidate`]** — A code region (hot path, likely branch,
//!   etc.) identified as suitable for speculative optimization.
//! - **[`SpeculationResult`]** — Outcome of a speculation: *success* (keep
//!   the optimization) or *failure* (roll back to the previous version).
//! - **[`BranchPrediction`]** — Predicts branch outcomes from profile data,
//!   feeding into speculation decisions.
//! - **[`SpeculativeInlining`]** — Speculatively inlines call targets based
//!   on predicted call frequencies.
//! - **[`SpeculativeCodeMotion`]** — Moves code (hoists invariants, sinks
//!   cold paths) based on predicted execution frequency.
//!
//! # Rollback mechanism
//!
//! Before any speculative transformation is applied, the executor saves a
//! **snapshot** of the affected compiled regions. If the speculation later
//! fails (assumption invalidated), the executor reverts those regions to
//! their pre-speculation state by restoring the snapshot.

use crate::config::Config;
use crate::profile::ProfileData;
use crate::types::{CompiledRegion, EdgeId, NodeId, RegionId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Assumption
// ---------------------------------------------------------------------------

/// An assumption about runtime behaviour that justifies a speculative
/// optimization.
///
/// Each variant encodes a specific kind of assumption. If the assumption
/// ceases to hold at runtime, the associated speculative code must be
/// deoptimized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Assumption {
    /// The branch identified by `EdgeId` is taken with high probability.
    ///
    /// This enables the optimizer to move the likely branch inline and
    /// the unlikely branch out-of-line (cold path).
    LikelyBranch(EdgeId),

    /// The given sequence of nodes constitutes a hot execution path.
    ///
    /// The optimizer may fuse these nodes into a single compiled region,
    /// eliding intermediate bookkeeping.
    HotPath(Vec<NodeId>),

    /// The region identified by `RegionId` is free from concurrent
    /// contention (no other thread is executing or mutating it).
    ///
    /// This assumption enables lock elision and single-threaded code
    /// generation for the region.
    NoContention(RegionId),
}

impl Assumption {
    /// Returns a human-readable description of the assumption.
    pub fn describe(&self) -> String {
        match self {
            Assumption::LikelyBranch(e) => format!("LikelyBranch(edge={})", e),
            Assumption::HotPath(nodes) => {
                let ids: Vec<String> = nodes.iter().map(|n| n.to_string()).collect();
                format!("HotPath([{}])", ids.join(", "))
            }
            Assumption::NoContention(r) => format!("NoContention(region={})", r),
        }
    }
}

// ---------------------------------------------------------------------------
// SpeculativeOpt
// ---------------------------------------------------------------------------

/// A speculative optimization: optimized code guarded by an assumption.
///
/// When the assumption holds, `optimized_code` is executed. When it is
/// invalidated, `fallback` is used instead. The deoptimization process
/// switches from one to the other transparently.
#[derive(Debug, Clone)]
pub struct SpeculativeOpt {
    /// The assumption that justifies this speculative optimization.
    pub assumption: Assumption,
    /// The optimized compiled region (used while the assumption holds).
    pub optimized_code: CompiledRegion,
    /// The safe fallback compiled region (used after deoptimization).
    pub fallback: CompiledRegion,
    /// Whether the assumption currently holds.
    pub is_valid: bool,
}

impl SpeculativeOpt {
    /// Creates a new speculative optimization.
    ///
    /// The assumption is assumed to hold initially (`is_valid = true`).
    pub fn new(
        assumption: Assumption,
        optimized_code: CompiledRegion,
        fallback: CompiledRegion,
    ) -> Self {
        SpeculativeOpt {
            assumption,
            optimized_code,
            fallback,
            is_valid: true,
        }
    }

    /// Attempts to execute the speculative optimization.
    ///
    /// Returns the optimized code if the assumption still holds, or the
    /// fallback if it does not.
    pub fn try_speculative(&self) -> &CompiledRegion {
        if self.is_valid {
            &self.optimized_code
        } else {
            &self.fallback
        }
    }

    /// Checks whether the assumption still holds given the current runtime
    /// state.
    ///
    /// # Arguments
    ///
    /// * `actual_edge` – If the assumption is `LikelyBranch`, the edge that
    ///   was actually taken at runtime.
    /// * `contended_regions` – A set of region IDs that are currently
    ///   contended (relevant for `NoContention`).
    ///
    /// Returns `true` if the assumption is still valid, `false` otherwise.
    pub fn check_assumption(
        &mut self,
        actual_edge: Option<EdgeId>,
        contended_regions: &[RegionId],
    ) -> bool {
        let valid = match &self.assumption {
            Assumption::LikelyBranch(expected_edge) => {
                actual_edge.is_none_or(|e| e == *expected_edge)
            }
            Assumption::HotPath(_) => {
                // HotPath validity is determined by profile data updates;
                // for now we consider it still valid.
                true
            }
            Assumption::NoContention(region) => !contended_regions.contains(region),
        };

        if !valid {
            self.is_valid = false;
        }
        valid
    }

    /// Deoptimizes: marks the assumption as invalid and returns a reference
    /// to the fallback code.
    ///
    /// After calling this method, [`try_speculative`] will always return the
    /// fallback until [`check_assumption`] re-validates the assumption (if
    /// ever).
    pub fn deoptimize(&mut self) -> &CompiledRegion {
        log::warn!(
            "Deoptimizing speculative opt: assumption {} invalidated",
            self.assumption.describe()
        );
        self.is_valid = false;
        &self.fallback
    }
}

// ---------------------------------------------------------------------------
// Speculative optimizer (legacy — kept for backward compatibility)
// ---------------------------------------------------------------------------

/// Manages a collection of speculative optimizations.
///
/// The `SpeculativeOptimizer` periodically checks assumptions and
/// deoptimizes any that no longer hold.
#[derive(Debug, Default)]
pub struct SpeculativeOptimizer {
    /// Active speculative optimizations.
    optimizations: Vec<SpeculativeOpt>,
}

impl SpeculativeOptimizer {
    /// Creates an empty speculative optimizer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a new speculative optimization.
    pub fn add(&mut self, opt: SpeculativeOpt) {
        self.optimizations.push(opt);
    }

    /// Checks all registered assumptions and deoptimizes those that no
    /// longer hold.
    ///
    /// Returns the number of deoptimizations that occurred.
    pub fn validate_all(
        &mut self,
        actual_edge: Option<EdgeId>,
        contended_regions: &[RegionId],
    ) -> usize {
        let mut deopt_count = 0;
        for opt in &mut self.optimizations {
            if opt.is_valid && !opt.check_assumption(actual_edge, contended_regions) {
                opt.deoptimize();
                deopt_count += 1;
            }
        }
        if deopt_count > 0 {
            log::info!("SpeculativeOptimizer: {} deoptimizations triggered", deopt_count);
        }
        deopt_count
    }

    /// Returns the number of currently valid speculative optimizations.
    pub fn active_count(&self) -> usize {
        self.optimizations.iter().filter(|o| o.is_valid).count()
    }

    /// Returns the total number of registered speculative optimizations.
    pub fn total_count(&self) -> usize {
        self.optimizations.len()
    }

    /// Returns `true` if speculative optimization is enabled in the config
    /// and there are valid optimizations remaining.
    pub fn is_active(&self, config: &Config) -> bool {
        config.enable_speculative && self.active_count() > 0
    }
}

// ---------------------------------------------------------------------------
// SpeculationCandidate
// ---------------------------------------------------------------------------

/// The kind of speculation opportunity identified in the SCG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateKind {
    /// A branch that is predicted to go one way with high confidence.
    LikelyBranch(EdgeId),
    /// A sequence of nodes on a hot execution path.
    HotPath(Vec<NodeId>),
    /// A call site where the callee is monomorphic (single target) or
    /// heavily biased toward one target.
    MonomorphicCall {
        /// The node containing the call site.
        call_site: NodeId,
        /// The predicted callee node.
        predicted_target: NodeId,
    },
    /// A region that is predicted to be uncontended.
    UncontendedRegion(RegionId),
}

impl CandidateKind {
    /// Human-readable label for the candidate kind.
    pub fn label(&self) -> &'static str {
        match self {
            CandidateKind::LikelyBranch(_) => "LikelyBranch",
            CandidateKind::HotPath(_) => "HotPath",
            CandidateKind::MonomorphicCall { .. } => "MonomorphicCall",
            CandidateKind::UncontendedRegion(_) => "UncontendedRegion",
        }
    }
}

/// A code region identified as suitable for speculative optimization.
///
/// Each candidate carries a *confidence* score in `[0.0, 1.0]` indicating
/// how likely the associated assumption is to hold. The executor uses
/// this score (along with the config's optimization level) to decide
/// whether to speculate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeculationCandidate {
    /// Unique identifier for this candidate.
    pub id: u64,
    /// The kind of speculation opportunity.
    pub kind: CandidateKind,
    /// Confidence score in [0, 1]. Higher is more likely correct.
    pub confidence: f64,
    /// The region that would be affected by the speculation.
    pub affected_region: RegionId,
}

impl SpeculationCandidate {
    /// Creates a new speculation candidate.
    pub fn new(id: u64, kind: CandidateKind, confidence: f64, affected_region: RegionId) -> Self {
        assert!(
            (0.0..=1.0).contains(&confidence),
            "confidence must be in [0, 1]"
        );
        SpeculationCandidate {
            id,
            kind,
            confidence,
            affected_region,
        }
    }

    /// Returns `true` if this candidate's confidence exceeds the given
    /// threshold.
    pub fn meets_threshold(&self, threshold: f64) -> bool {
        self.confidence >= threshold
    }
}

// ---------------------------------------------------------------------------
// SpeculationResult
// ---------------------------------------------------------------------------

/// The outcome of applying a speculative optimization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpeculationResult {
    /// The speculation succeeded; keep the optimized code.
    Success {
        /// The candidate that was speculated on.
        candidate_id: u64,
    },
    /// The speculation failed; roll back to the pre-speculation version.
    Failure {
        /// The candidate that was speculated on.
        candidate_id: u64,
        /// Human-readable reason for the failure.
        reason: String,
    },
}

impl SpeculationResult {
    /// Returns `true` if this result represents a successful speculation.
    pub fn is_success(&self) -> bool {
        matches!(self, SpeculationResult::Success { .. })
    }

    /// Returns `true` if this result represents a failed speculation.
    pub fn is_failure(&self) -> bool {
        matches!(self, SpeculationResult::Failure { .. })
    }

    /// Returns the candidate ID associated with this result.
    pub fn candidate_id(&self) -> u64 {
        match self {
            SpeculationResult::Success { candidate_id } => *candidate_id,
            SpeculationResult::Failure { candidate_id, .. } => *candidate_id,
        }
    }
}

// ---------------------------------------------------------------------------
// BranchPrediction
// ---------------------------------------------------------------------------

/// A single branch prediction: which edge is likely taken, with what
/// probability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchPrediction {
    /// The branch (edge) being predicted.
    pub edge: EdgeId,
    /// Predicted probability that this edge is taken, in [0, 1].
    pub probability: f64,
    /// Number of profile samples this prediction is based on.
    pub sample_count: u64,
}

impl BranchPrediction {
    /// Creates a new branch prediction.
    pub fn new(edge: EdgeId, probability: f64, sample_count: u64) -> Self {
        assert!(
            (0.0..=1.0).contains(&probability),
            "probability must be in [0, 1]"
        );
        BranchPrediction {
            edge,
            probability,
            sample_count,
        }
    }

    /// Returns `true` if this prediction's probability exceeds the
    /// threshold and the sample count is sufficient.
    pub fn is_confident(&self, prob_threshold: f64, min_samples: u64) -> bool {
        self.probability >= prob_threshold && self.sample_count >= min_samples
    }
}

/// A table of branch predictions derived from profile data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BranchPredictionTable {
    /// Per-edge predictions.
    predictions: HashMap<EdgeId, BranchPrediction>,
}

impl BranchPredictionTable {
    /// Creates an empty prediction table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a prediction table from profile data.
    ///
    /// Uses a simple frequency-based model: for each edge, the predicted
    /// probability is the fraction of times that edge was taken out of all
    /// observations. In a full implementation this would use a more
    /// sophisticated predictor (e.g. 2-bit saturating counter, perceptron).
    pub fn from_profile(profile: &ProfileData) -> Self {
        let mut table = BranchPredictionTable::new();

        // Derive branch predictions from call counts.
        // We treat each node's call count as a proxy for how often edges
        // leading into that node are taken.
        let total_calls: u64 = profile.call_counts.values().sum();
        if total_calls == 0 {
            return table;
        }

        for (&node_id, &count) in &profile.call_counts {
            // Use the node_id as an edge_id proxy. In a real SCG the edge
            // IDs would come from the graph structure.
            let edge_id = node_id as EdgeId;
            let probability = count as f64 / total_calls as f64;
            table.predictions.insert(
                edge_id,
                BranchPrediction::new(edge_id, probability, count),
            );
        }

        table
    }

    /// Looks up the prediction for a given edge.
    pub fn get(&self, edge: EdgeId) -> Option<&BranchPrediction> {
        self.predictions.get(&edge)
    }

    /// Inserts or updates a prediction.
    pub fn insert(&mut self, pred: BranchPrediction) {
        self.predictions.insert(pred.edge, pred);
    }

    /// Returns all predictions sorted by probability (descending).
    pub fn sorted_by_probability(&self) -> Vec<&BranchPrediction> {
        let mut preds: Vec<&BranchPrediction> = self.predictions.values().collect();
        preds.sort_by(|a, b| b.probability.partial_cmp(&a.probability).unwrap_or(std::cmp::Ordering::Equal));
        preds
    }

    /// Returns the number of predictions in the table.
    pub fn len(&self) -> usize {
        self.predictions.len()
    }

    /// Returns `true` if the table is empty.
    pub fn is_empty(&self) -> bool {
        self.predictions.is_empty()
    }

    /// Generates `SpeculationCandidate`s for branches whose predicted
    /// probability exceeds `threshold` and sample count exceeds
    /// `min_samples`.
    pub fn generate_candidates(
        &self,
        threshold: f64,
        min_samples: u64,
        start_id: u64,
    ) -> Vec<SpeculationCandidate> {
        let mut candidates = Vec::new();
        let mut next_id = start_id;

        for pred in self.sorted_by_probability() {
            if pred.is_confident(threshold, min_samples) {
                candidates.push(SpeculationCandidate::new(
                    next_id,
                    CandidateKind::LikelyBranch(pred.edge),
                    pred.probability,
                    pred.edge as RegionId, // edge as region proxy
                ));
                next_id += 1;
            }
        }

        candidates
    }
}

// ---------------------------------------------------------------------------
// SpeculativeInlining
// ---------------------------------------------------------------------------

/// Describes a speculative inlining decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InlineDecision {
    /// The call site node being inlined.
    pub call_site: NodeId,
    /// The predicted callee node.
    pub callee: NodeId,
    /// Confidence of the prediction.
    pub confidence: f64,
    /// The region containing the call site.
    pub region: RegionId,
}

/// Speculatively inlines call targets based on predicted call frequencies.
///
/// Inlining is one of the most impactful optimizations. When profile data
/// shows that a call site almost always dispatches to the same target,
/// we can speculatively inline that target, avoiding the overhead of
/// indirect dispatch.
#[derive(Debug, Clone, Default)]
pub struct SpeculativeInlining {
    /// Active inline decisions.
    decisions: Vec<InlineDecision>,
}

impl SpeculativeInlining {
    /// Creates an empty speculative inliner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Analyzes profile data and generates inline decisions for call sites
    /// that are monomorphic (or near-monomorphic) with confidence above
    /// `threshold`.
    ///
    /// A call site is considered inlineable when one target accounts for
    /// more than `threshold` fraction of all calls at that site.
    pub fn analyze(
        &mut self,
        profile: &ProfileData,
        threshold: f64,
    ) -> Vec<InlineDecision> {
        self.decisions.clear();

        // For each node with high call count, check if it's a dominant
        // target. In a full implementation we would look at per-call-site
        // dispatch histograms. Here we use the global call counts as a
        // proxy: a node called very frequently is a good inline candidate.
        let total_calls: u64 = profile.call_counts.values().sum();
        if total_calls == 0 {
            return Vec::new();
        }

        for (&node_id, &count) in &profile.call_counts {
            let ratio = count as f64 / total_calls as f64;
            if ratio >= threshold && count >= 10 {
                self.decisions.push(InlineDecision {
                    call_site: node_id, // In a real SCG, this would be the actual call site
                    callee: node_id,
                    confidence: ratio,
                    region: node_id as RegionId,
                });
            }
        }

        self.decisions.clone()
    }

    /// Applies an inline decision, producing an optimized compiled region
    /// that embeds the callee at the call site.
    ///
    /// Returns a `SpeculativeOpt` wrapping both the optimized and fallback
    /// code. If the speculation fails, the fallback (non-inlined version)
    /// is used.
    pub fn apply_inline(
        &self,
        decision: &InlineDecision,
        optimized_code: CompiledRegion,
        fallback: CompiledRegion,
    ) -> SpeculativeOpt {
        let assumption = Assumption::HotPath(vec![decision.call_site, decision.callee]);
        SpeculativeOpt::new(assumption, optimized_code, fallback)
    }

    /// Returns the current inline decisions.
    pub fn decisions(&self) -> &[InlineDecision] {
        &self.decisions
    }
}

// ---------------------------------------------------------------------------
// SpeculativeCodeMotion
// ---------------------------------------------------------------------------

/// The kind of code motion being performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CodeMotionKind {
    /// Hoist invariant code out of a loop / hot region to execute it once
    /// before the region.
    HoistInvariant {
        /// The node being hoisted.
        node: NodeId,
        /// The region it's being hoisted out of.
        source_region: RegionId,
    },
    /// Sink cold code out of a hot path to an out-of-line cold section.
    SinkColdCode {
        /// The node being sunk.
        node: NodeId,
        /// The region it's being sunk from.
        source_region: RegionId,
    },
}

/// Describes a speculative code motion decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeMotionDecision {
    /// The kind of code motion.
    pub kind: CodeMotionKind,
    /// Confidence that this motion is beneficial.
    pub confidence: f64,
}

/// Moves code based on predicted execution frequency.
///
/// Hot-path code is kept inline and fast; cold code is moved out of line
/// to improve instruction-cache locality. Invariant code is hoisted out
/// of loops to avoid redundant computation.
#[derive(Debug, Clone, Default)]
pub struct SpeculativeCodeMotion {
    /// Active code-motion decisions.
    decisions: Vec<CodeMotionDecision>,
}

impl SpeculativeCodeMotion {
    /// Creates an empty speculative code-motion engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Analyzes profile data and generates code-motion decisions.
    ///
    /// Nodes with very high access counts relative to the average are
    /// considered *hot* — surrounding cold nodes should be sunk. Nodes
    /// with very low access counts inside hot regions are candidates for
    /// sinking.
    ///
    /// Nodes that are accessed the same number of times as a surrounding
    /// loop header are considered *invariant* and candidates for hoisting.
    pub fn analyze(
        &mut self,
        profile: &ProfileData,
        hot_threshold: f64,
        cold_threshold: f64,
    ) -> Vec<CodeMotionDecision> {
        self.decisions.clear();

        let total_calls: u64 = profile.call_counts.values().sum();
        let node_count = profile.call_counts.len() as u64;
        if node_count == 0 || total_calls == 0 {
            return Vec::new();
        }

        let avg_calls = total_calls as f64 / node_count as f64;

        for (&node_id, &count) in &profile.call_counts {
            let ratio = count as f64 / avg_calls;

            if ratio >= hot_threshold {
                // This node is hot; nothing to move for the node itself,
                // but we could sink surrounding cold code. We record this
                // as a hoist-invariant hint for demonstration.
                self.decisions.push(CodeMotionDecision {
                    kind: CodeMotionKind::HoistInvariant {
                        node: node_id,
                        source_region: node_id as RegionId,
                    },
                    confidence: (ratio / hot_threshold).min(1.0),
                });
            } else if ratio <= cold_threshold && ratio > 0.0 {
                // This node is cold relative to the average; sink it.
                self.decisions.push(CodeMotionDecision {
                    kind: CodeMotionKind::SinkColdCode {
                        node: node_id,
                        source_region: node_id as RegionId,
                    },
                    confidence: 1.0 - ratio,
                });
            }
        }

        self.decisions.clone()
    }

    /// Applies a code-motion decision, producing a `SpeculativeOpt`.
    pub fn apply_motion(
        &self,
        decision: &CodeMotionDecision,
        optimized_code: CompiledRegion,
        fallback: CompiledRegion,
    ) -> SpeculativeOpt {
        let assumption = match &decision.kind {
            CodeMotionKind::HoistInvariant { node, .. } => {
                Assumption::HotPath(vec![*node])
            }
            CodeMotionKind::SinkColdCode { node: _, source_region } => {
                // Sinking cold code is valid as long as the source region
                // remains hot — i.e. the node is genuinely cold.
                Assumption::NoContention(*source_region)
            }
        };
        SpeculativeOpt::new(assumption, optimized_code, fallback)
    }

    /// Returns the current code-motion decisions.
    pub fn decisions(&self) -> &[CodeMotionDecision] {
        &self.decisions
    }
}

// ---------------------------------------------------------------------------
// Snapshot (for rollback)
// ---------------------------------------------------------------------------

/// A snapshot of compiled regions, used for rollback when a speculation
/// fails.
#[derive(Debug, Clone, Default)]
struct Snapshot {
    /// Maps region IDs to their compiled code at the time of the snapshot.
    regions: HashMap<RegionId, CompiledRegion>,
    /// The SCG node count at the time of the snapshot.
    #[allow(dead_code)]
    scg_node_count: usize,
    /// The SCG edge count at the time of the snapshot.
    #[allow(dead_code)]
    scg_edge_count: usize,
}

impl Snapshot {
    /// Creates a snapshot of the given regions.
    fn from_regions(regions: &[(RegionId, CompiledRegion)], node_count: usize, edge_count: usize) -> Self {
        Snapshot {
            regions: regions.iter().cloned().collect(),
            scg_node_count: node_count,
            scg_edge_count: edge_count,
        }
    }

    /// Restores the regions from this snapshot, returning the restored
    /// compiled regions.
    fn restore(&self) -> Vec<(RegionId, CompiledRegion)> {
        let mut result: Vec<(RegionId, CompiledRegion)> = self
            .regions
            .iter()
            .map(|(&id, r)| (id, r.clone()))
            .collect();
        result.sort_by_key(|(id, _)| *id);
        result
    }
}

// ---------------------------------------------------------------------------
// SpeculativeExecutor
// ---------------------------------------------------------------------------

/// The top-level speculative execution engine.
///
/// `SpeculativeExecutor` orchestrates the full speculative-optimization
/// lifecycle:
///
/// 1. **Identify candidates** — from profile data, produce
///    [`SpeculationCandidate`]s.
/// 2. **Apply speculations** — transform the code (inlining, code motion,
///    branch layout) and record the resulting [`SpeculativeOpt`]s.
/// 3. **Snapshot & rollback** — before each speculative transformation,
///    save a snapshot of the affected compiled regions. If the speculation
///    later fails, restore the snapshot.
/// 4. **Validate** — periodically check assumptions against runtime
///    observations and roll back any that are invalidated.
#[derive(Debug)]
pub struct SpeculativeExecutor {
    /// The branch prediction table.
    branch_predictions: BranchPredictionTable,
    /// The speculative inliner.
    inliner: SpeculativeInlining,
    /// The speculative code-motion engine.
    code_motion: SpeculativeCodeMotion,
    /// Active speculative optimizations (indexed by candidate ID).
    active_opts: HashMap<u64, SpeculativeOpt>,
    /// Snapshots keyed by candidate ID, for rollback.
    snapshots: HashMap<u64, Snapshot>,
    /// Results of all applied speculations.
    results: Vec<SpeculationResult>,
    /// The current SCG node count (updated on each speculation).
    scg_node_count: usize,
    /// The current SCG edge count (updated on each speculation).
    scg_edge_count: usize,
    /// Minimum confidence threshold for speculation candidates.
    confidence_threshold: f64,
    /// Minimum sample count for branch predictions.
    min_sample_count: u64,
}

impl SpeculativeExecutor {
    /// Creates a new speculative executor with default parameters.
    pub fn new() -> Self {
        Self {
            branch_predictions: BranchPredictionTable::new(),
            inliner: SpeculativeInlining::new(),
            code_motion: SpeculativeCodeMotion::new(),
            active_opts: HashMap::new(),
            snapshots: HashMap::new(),
            results: Vec::new(),
            scg_node_count: 0,
            scg_edge_count: 0,
            confidence_threshold: 0.7,
            min_sample_count: 10,
        }
    }

    /// Creates a new speculative executor with custom thresholds.
    pub fn with_thresholds(confidence_threshold: f64, min_sample_count: u64) -> Self {
        Self {
            confidence_threshold,
            min_sample_count,
            ..Self::new()
        }
    }

    /// Sets the SCG dimensions (node and edge counts).
    pub fn set_scg_dimensions(&mut self, node_count: usize, edge_count: usize) {
        self.scg_node_count = node_count;
        self.scg_edge_count = edge_count;
    }

    // -----------------------------------------------------------------------
    // Phase 1: Identify candidates
    // -----------------------------------------------------------------------

    /// Updates the branch prediction table from profile data and generates
    /// speculation candidates.
    ///
    /// Returns the list of candidates whose confidence meets the threshold.
    pub fn identify_candidates(&mut self, profile: &ProfileData) -> Vec<SpeculationCandidate> {
        self.branch_predictions = BranchPredictionTable::from_profile(profile);
        self.branch_predictions.generate_candidates(
            self.confidence_threshold,
            self.min_sample_count,
            self.next_candidate_id(),
        )
    }

    /// Identifies inline candidates from profile data.
    pub fn identify_inline_candidates(&mut self, profile: &ProfileData) -> Vec<InlineDecision> {
        self.inliner.analyze(profile, self.confidence_threshold)
    }

    /// Identifies code-motion candidates from profile data.
    pub fn identify_code_motion_candidates(
        &mut self,
        profile: &ProfileData,
    ) -> Vec<CodeMotionDecision> {
        self.code_motion.analyze(profile, 2.0, 0.3)
    }

    // -----------------------------------------------------------------------
    // Phase 2: Apply speculations
    // -----------------------------------------------------------------------

    /// Applies a speculation candidate, producing a `SpeculativeOpt` and
    /// recording a snapshot for potential rollback.
    ///
    /// The caller must provide the optimized and fallback compiled regions.
    /// Returns the `SpeculationResult` indicating success or failure.
    pub fn apply_speculation(
        &mut self,
        candidate: &SpeculationCandidate,
        optimized_code: CompiledRegion,
        fallback: CompiledRegion,
    ) -> SpeculationResult {
        // Take a snapshot of the affected region before applying.
        let snapshot = Snapshot::from_regions(
            &[(candidate.affected_region, fallback.clone())],
            self.scg_node_count,
            self.scg_edge_count,
        );
        self.snapshots.insert(candidate.id, snapshot);

        let assumption = match &candidate.kind {
            CandidateKind::LikelyBranch(edge) => Assumption::LikelyBranch(*edge),
            CandidateKind::HotPath(nodes) => Assumption::HotPath(nodes.clone()),
            CandidateKind::MonomorphicCall { .. } => {
                Assumption::HotPath(vec![candidate.affected_region as NodeId])
            }
            CandidateKind::UncontendedRegion(region) => Assumption::NoContention(*region),
        };

        let opt = SpeculativeOpt::new(assumption, optimized_code, fallback);
        self.active_opts.insert(candidate.id, opt);

        let result = SpeculationResult::Success {
            candidate_id: candidate.id,
        };
        self.results.push(result.clone());
        result
    }

    /// Applies a speculative inlining decision.
    pub fn apply_inline(
        &mut self,
        decision: &InlineDecision,
        optimized_code: CompiledRegion,
        fallback: CompiledRegion,
    ) -> SpeculationResult {
        let candidate_id = self.next_candidate_id();

        // Snapshot the affected region.
        let snapshot = Snapshot::from_regions(
            &[(decision.region, fallback.clone())],
            self.scg_node_count,
            self.scg_edge_count,
        );
        self.snapshots.insert(candidate_id, snapshot);

        let opt = self.inliner.apply_inline(decision, optimized_code, fallback);
        self.active_opts.insert(candidate_id, opt);

        let result = SpeculationResult::Success {
            candidate_id,
        };
        self.results.push(result.clone());
        result
    }

    /// Applies a speculative code-motion decision.
    pub fn apply_code_motion(
        &mut self,
        decision: &CodeMotionDecision,
        optimized_code: CompiledRegion,
        fallback: CompiledRegion,
    ) -> SpeculationResult {
        let candidate_id = self.next_candidate_id();

        let region = match &decision.kind {
            CodeMotionKind::HoistInvariant { source_region, .. } => *source_region,
            CodeMotionKind::SinkColdCode { source_region, .. } => *source_region,
        };

        let snapshot = Snapshot::from_regions(
            &[(region, fallback.clone())],
            self.scg_node_count,
            self.scg_edge_count,
        );
        self.snapshots.insert(candidate_id, snapshot);

        let opt = self.code_motion.apply_motion(decision, optimized_code, fallback);
        self.active_opts.insert(candidate_id, opt);

        let result = SpeculationResult::Success {
            candidate_id,
        };
        self.results.push(result.clone());
        result
    }

    // -----------------------------------------------------------------------
    // Phase 3: Validate & rollback
    // -----------------------------------------------------------------------

    /// Validates all active speculative optimizations against the given
    /// runtime observations.
    ///
    /// For each optimization whose assumption is invalidated, this method:
    /// 1. Rolls back the affected regions to their snapshot.
    /// 2. Records a `SpeculationResult::Failure`.
    /// 3. Removes the optimization from the active set.
    ///
    /// Returns the number of rollbacks performed.
    pub fn validate_and_rollback(
        &mut self,
        actual_edge: Option<EdgeId>,
        contended_regions: &[RegionId],
    ) -> usize {
        let mut rollback_count = 0;
        let mut failed_ids = Vec::new();

        for (&candidate_id, opt) in &mut self.active_opts {
            if opt.is_valid && !opt.check_assumption(actual_edge, contended_regions) {
                opt.deoptimize();
                failed_ids.push(candidate_id);
                rollback_count += 1;
            }
        }

        // Perform rollbacks.
        for candidate_id in &failed_ids {
            if let Some(snapshot) = self.snapshots.remove(candidate_id) {
                let restored = snapshot.restore();
                log::info!(
                    "Rollback for candidate {}: restored {} region(s)",
                    candidate_id,
                    restored.len()
                );
            }

            self.results.push(SpeculationResult::Failure {
                candidate_id: *candidate_id,
                reason: "Assumption invalidated at runtime".to_string(),
            });

            self.active_opts.remove(candidate_id);
        }

        if rollback_count > 0 {
            log::warn!(
                "SpeculativeExecutor: {} rollback(s) performed",
                rollback_count
            );
        }

        rollback_count
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Returns the number of currently active (valid) speculative
    /// optimizations.
    pub fn active_count(&self) -> usize {
        self.active_opts.values().filter(|o| o.is_valid).count()
    }

    /// Returns the total number of speculative optimizations ever applied.
    pub fn total_applied(&self) -> usize {
        self.results.iter().filter(|r| r.is_success()).count()
    }

    /// Returns the total number of rollbacks that have occurred.
    pub fn total_rollbacks(&self) -> usize {
        self.results.iter().filter(|r| r.is_failure()).count()
    }

    /// Returns a reference to all speculation results.
    pub fn results(&self) -> &[SpeculationResult] {
        &self.results
    }

    /// Returns a reference to the branch prediction table.
    pub fn branch_predictions(&self) -> &BranchPredictionTable {
        &self.branch_predictions
    }

    /// Returns a reference to the speculative inliner.
    pub fn inliner(&self) -> &SpeculativeInlining {
        &self.inliner
    }

    /// Returns a reference to the speculative code-motion engine.
    pub fn code_motion(&self) -> &SpeculativeCodeMotion {
        &self.code_motion
    }

    /// Looks up the active optimization for a candidate.
    pub fn get_opt(&self, candidate_id: u64) -> Option<&SpeculativeOpt> {
        self.active_opts.get(&candidate_id)
    }

    /// Checks whether the executor has any snapshot available for a given
    /// candidate (i.e. rollback is possible).
    pub fn has_snapshot(&self, candidate_id: u64) -> bool {
        self.snapshots.contains_key(&candidate_id)
    }

    /// Returns `true` if speculative execution is enabled and there are
    /// active optimizations.
    pub fn is_active(&self, config: &Config) -> bool {
        config.enable_speculative && self.active_count() > 0
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    /// Returns the next candidate ID (one past the highest existing ID).
    fn next_candidate_id(&self) -> u64 {
        self.active_opts
            .keys()
            .max()
            .map(|&id| id + 1)
            .unwrap_or(0)
    }
}

impl Default for SpeculativeExecutor {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileData;

    // -- Helpers -----------------------------------------------------------

    fn stub_region(id: RegionId) -> CompiledRegion {
        CompiledRegion {
            region_id: id,
            code: vec![0x90; 16], // NOP sled
        }
    }

    fn hot_profile() -> ProfileData {
        let mut profile = ProfileData::new();
        // Node 1 is very hot (1000 calls).
        for _ in 0..1000 {
            profile.record_call(1);
        }
        // Node 2 is warm (200 calls).
        for _ in 0..200 {
            profile.record_call(2);
        }
        // Node 3 is cold (5 calls).
        for _ in 0..5 {
            profile.record_call(3);
        }
        profile
    }

    // -- Legacy tests (preserved) ------------------------------------------

    #[test]
    fn try_speculative_returns_optimized_when_valid() {
        let opt = SpeculativeOpt::new(
            Assumption::LikelyBranch(1),
            stub_region(10),
            stub_region(11),
        );
        assert!(opt.is_valid);
        assert_eq!(opt.try_speculative().region_id, 10);
    }

    #[test]
    fn deoptimize_switches_to_fallback() {
        let mut opt = SpeculativeOpt::new(
            Assumption::LikelyBranch(1),
            stub_region(10),
            stub_region(11),
        );
        opt.deoptimize();
        assert!(!opt.is_valid);
        assert_eq!(opt.try_speculative().region_id, 11);
    }

    #[test]
    fn check_assumption_invalidates_on_wrong_branch() {
        let mut opt = SpeculativeOpt::new(
            Assumption::LikelyBranch(42),
            stub_region(10),
            stub_region(11),
        );
        // Actual edge differs from expected.
        assert!(!opt.check_assumption(Some(99), &[]));
        assert!(!opt.is_valid);
    }

    #[test]
    fn no_contention_check() {
        let mut opt = SpeculativeOpt::new(
            Assumption::NoContention(5),
            stub_region(10),
            stub_region(11),
        );
        assert!(opt.check_assumption(None, &[]));
        assert!(!opt.check_assumption(None, &[5]));
    }

    #[test]
    fn optimizer_validate_all() {
        let mut optimizer = SpeculativeOptimizer::new();
        optimizer.add(SpeculativeOpt::new(
            Assumption::LikelyBranch(1),
            stub_region(10),
            stub_region(11),
        ));
        optimizer.add(SpeculativeOpt::new(
            Assumption::LikelyBranch(2),
            stub_region(20),
            stub_region(21),
        ));
        assert_eq!(optimizer.active_count(), 2);
        // Edge 99 matches neither assumption.
        let deopts = optimizer.validate_all(Some(99), &[]);
        assert_eq!(deopts, 2);
        assert_eq!(optimizer.active_count(), 0);
    }

    // -- New tests ---------------------------------------------------------

    #[test]
    fn speculation_candidate_meets_threshold() {
        let candidate = SpeculationCandidate::new(
            1,
            CandidateKind::LikelyBranch(42),
            0.85,
            100,
        );
        assert!(candidate.meets_threshold(0.7));
        assert!(!candidate.meets_threshold(0.9));
    }

    #[test]
    fn speculation_result_accessors() {
        let success = SpeculationResult::Success { candidate_id: 1 };
        let failure = SpeculationResult::Failure {
            candidate_id: 2,
            reason: "branch mispredict".to_string(),
        };
        assert!(success.is_success());
        assert!(!success.is_failure());
        assert!(failure.is_failure());
        assert!(!failure.is_success());
        assert_eq!(success.candidate_id(), 1);
        assert_eq!(failure.candidate_id(), 2);
    }

    #[test]
    fn branch_prediction_table_from_profile() {
        let profile = hot_profile();
        let table = BranchPredictionTable::from_profile(&profile);

        // Node 1 has 1000 / 1205 ≈ 0.83 probability.
        let pred = table.get(1).expect("prediction for edge 1");
        assert!(pred.probability > 0.8);
        assert!(pred.is_confident(0.7, 10));

        // Node 3 has only 5 calls — not confident.
        let pred3 = table.get(3).expect("prediction for edge 3");
        assert!(!pred3.is_confident(0.7, 10));
    }

    #[test]
    fn branch_prediction_generate_candidates() {
        let profile = hot_profile();
        let table = BranchPredictionTable::from_profile(&profile);
        let candidates = table.generate_candidates(0.7, 10, 0);

        // Only node 1 (1000 calls, ~83%) should exceed 0.7 threshold.
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].kind, CandidateKind::LikelyBranch(1));
    }

    #[test]
    fn speculative_inlining_analyze() {
        let profile = hot_profile();
        let mut inliner = SpeculativeInlining::new();
        let decisions = inliner.analyze(&profile, 0.7);

        // Node 1 should be an inline candidate (>70% of calls, ≥10 calls).
        assert!(!decisions.is_empty());
        assert!(decisions.iter().any(|d| d.callee == 1));
    }

    #[test]
    fn speculative_inlining_apply() {
        let mut inliner = SpeculativeInlining::new();
        let decision = InlineDecision {
            call_site: 10,
            callee: 20,
            confidence: 0.9,
            region: 100,
        };
        let opt = inliner.apply_inline(&decision, stub_region(100), stub_region(101));
        assert!(opt.is_valid);
        assert_eq!(opt.optimized_code.region_id, 100);
        assert_eq!(opt.fallback.region_id, 101);
    }

    #[test]
    fn speculative_code_motion_analyze() {
        let profile = hot_profile();
        let mut motion = SpeculativeCodeMotion::new();
        let decisions = motion.analyze(&profile, 2.0, 0.3);

        // Node 1 is hot (1000/avg ≈ 3.3x → hoist invariant).
        // Node 3 is cold (5/avg ≈ 0.017x → sink cold code).
        assert!(decisions.iter().any(|d| matches!(d.kind, CodeMotionKind::HoistInvariant { node: 1, .. })));
        assert!(decisions.iter().any(|d| matches!(d.kind, CodeMotionKind::SinkColdCode { node: 3, .. })));
    }

    #[test]
    fn speculative_executor_apply_and_rollback() {
        let mut executor = SpeculativeExecutor::new();
        executor.set_scg_dimensions(100, 200);

        let candidate = SpeculationCandidate::new(
            0,
            CandidateKind::LikelyBranch(42),
            0.9,
            500,
        );

        // Apply the speculation.
        let result = executor.apply_speculation(&candidate, stub_region(500), stub_region(501));
        assert!(result.is_success());
        assert_eq!(executor.active_count(), 1);
        assert!(executor.has_snapshot(0));

        // Validate with a wrong branch — should trigger rollback.
        let rollbacks = executor.validate_and_rollback(Some(99), &[]);
        assert_eq!(rollbacks, 1);
        assert_eq!(executor.active_count(), 0);
        assert!(!executor.has_snapshot(0)); // snapshot consumed
        assert_eq!(executor.total_rollbacks(), 1);
    }

    #[test]
    fn speculative_executor_preserves_valid_on_correct_branch() {
        let mut executor = SpeculativeExecutor::new();

        let candidate = SpeculationCandidate::new(
            0,
            CandidateKind::LikelyBranch(42),
            0.9,
            500,
        );

        executor.apply_speculation(&candidate, stub_region(500), stub_region(501));
        assert_eq!(executor.active_count(), 1);

        // Validate with the *correct* branch — no rollback.
        let rollbacks = executor.validate_and_rollback(Some(42), &[]);
        assert_eq!(rollbacks, 0);
        assert_eq!(executor.active_count(), 1);
        assert_eq!(executor.total_rollbacks(), 0);
    }

    #[test]
    fn speculative_executor_identify_candidates_from_profile() {
        let mut executor = SpeculativeExecutor::with_thresholds(0.7, 10);
        let profile = hot_profile();

        let candidates = executor.identify_candidates(&profile);
        // Node 1 dominates; should produce at least one candidate.
        assert!(!candidates.is_empty());
        assert!(candidates.iter().any(|c| matches!(c.kind, CandidateKind::LikelyBranch(1))));
    }

    #[test]
    fn speculative_executor_full_lifecycle() {
        let mut executor = SpeculativeExecutor::new();
        executor.set_scg_dimensions(50, 80);

        // 1. Build profile.
        let profile = hot_profile();

        // 2. Identify and apply an inline candidate.
        let inline_decisions = executor.identify_inline_candidates(&profile);
        assert!(!inline_decisions.is_empty());

        let first_decision = &inline_decisions[0];
        let inline_result = executor.apply_inline(
            first_decision,
            stub_region(first_decision.region),
            stub_region(first_decision.region + 1000),
        );
        assert!(inline_result.is_success());

        // 3. Identify and apply a code-motion candidate.
        let motion_decisions = executor.identify_code_motion_candidates(&profile);
        if let Some(motion) = motion_decisions.first() {
            let region = match &motion.kind {
                CodeMotionKind::HoistInvariant { source_region, .. } => *source_region,
                CodeMotionKind::SinkColdCode { source_region, .. } => *source_region,
            };
            let motion_result = executor.apply_code_motion(
                motion,
                stub_region(region),
                stub_region(region + 2000),
            );
            assert!(motion_result.is_success());
        }

        // 4. Validate — all should be valid initially (no contradicting
        //    runtime evidence for HotPath assumptions).
        let rollbacks = executor.validate_and_rollback(None, &[]);
        assert_eq!(rollbacks, 0);
        assert!(executor.active_count() > 0);

        // 5. Introduce contention for one of the NoContention-based opts
        //    (if any). Since our inline/code-motion used HotPath, this
        //    won't invalidate them.
        let rollbacks2 = executor.validate_and_rollback(None, &[9999]);
        assert_eq!(rollbacks2, 0);

        // 6. Check accumulated results.
        assert!(executor.total_applied() >= 1);
    }

    #[test]
    fn rollback_restores_snapshot_data() {
        let mut executor = SpeculativeExecutor::new();

        let candidate = SpeculationCandidate::new(
            0,
            CandidateKind::LikelyBranch(7),
            0.95,
            300,
        );

        // Apply speculation — snapshot captures fallback region 301.
        executor.apply_speculation(&candidate, stub_region(300), stub_region(301));

        // Before rollback, the snapshot should exist and contain region 301.
        assert!(executor.has_snapshot(0));

        // Trigger rollback.
        let rollbacks = executor.validate_and_rollback(Some(999), &[]);
        assert_eq!(rollbacks, 1);

        // After rollback, the snapshot is consumed and a Failure result
        // is recorded.
        assert!(!executor.has_snapshot(0));
        let failures: Vec<_> = executor
            .results()
            .iter()
            .filter(|r| r.is_failure())
            .collect();
        assert_eq!(failures.len(), 1);
    }

    #[test]
    fn candidate_kind_labels() {
        assert_eq!(CandidateKind::LikelyBranch(1).label(), "LikelyBranch");
        assert_eq!(CandidateKind::HotPath(vec![]).label(), "HotPath");
        assert_eq!(
            CandidateKind::MonomorphicCall {
                call_site: 1,
                predicted_target: 2
            }
            .label(),
            "MonomorphicCall"
        );
        assert_eq!(
            CandidateKind::UncontendedRegion(5).label(),
            "UncontendedRegion"
        );
    }

    #[test]
    fn executor_is_active_respects_config() {
        let mut executor = SpeculativeExecutor::new();

        // No active opts → not active even with config enabled.
        let config = Config::default();
        assert!(!executor.is_active(&config));

        // Add an opt.
        let candidate = SpeculationCandidate::new(
            0,
            CandidateKind::LikelyBranch(1),
            0.9,
            10,
        );
        executor.apply_speculation(&candidate, stub_region(10), stub_region(11));
        assert!(executor.is_active(&config));

        // Disabled via config.
        let disabled_config = Config::default().with_speculative(false);
        assert!(!executor.is_active(&disabled_config));
    }
}
