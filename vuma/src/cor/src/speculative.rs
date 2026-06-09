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

use crate::config::Config;
use crate::types::{CompiledRegion, EdgeId, NodeId, RegionId};
use serde::{Deserialize, Serialize};

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
                actual_edge.map_or(true, |e| e == *expected_edge)
            }
            Assumption::HotPath(_) => {
                // HotPath validity is determined by profile data updates;
                // for now we consider it still valid.
                true
            }
            Assumption::NoContention(region) => {
                !contended_regions.contains(region)
            }
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
// Speculative optimizer
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

#[cfg(test)]
mod tests {
    use super::*;

    fn stub_region(id: RegionId) -> CompiledRegion {
        CompiledRegion {
            region_id: id,
            code: vec![0x90; 16], // NOP sled
        }
    }

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
}
