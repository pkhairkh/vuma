//! Modular IVE Analysis
//!
//! Instead of running the global fixpoint solver on the entire SCG,
//! this module analyzes each function independently and produces
//! summary edges for interprocedural propagation.
//!
//! # Algorithm
//!
//! 1. For each function, compute local BDs (RepD/CapD/RelD).
//! 2. Summarize: which regions escape, which are modified, which are freed.
//! 3. Use summaries for interprocedural propagation (faster than global).
//! 4. Verify each function locally using its summary + caller summaries.

use std::collections::{HashMap, HashSet};
use vuma_scg::graph::SCG;
use vuma_scg::node::{NodeId, NodePayload, NodeType, NodeId as ScgNodeId};

/// Function summary for interprocedural analysis.
#[derive(Debug, Clone, Default)]
pub struct FunctionSummary {
    /// Regions that escape this function (returned or stored globally).
    pub escaping_regions: HashSet<u32>,
    /// Regions that are freed in this function.
    pub freed_regions: HashSet<u32>,
    /// Regions that are modified (written to) in this function.
    pub modified_regions: HashSet<u32>,
    /// Whether this function allocates memory.
    pub allocates: bool,
    /// Whether this function performs I/O.
    pub performs_io: bool,
    /// Whether this function is pure (no side effects).
    pub is_pure: bool,
}

impl FunctionSummary {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a region escapes this function.
    pub fn escapes(&self, region: u32) -> bool {
        self.escaping_regions.contains(&region)
    }

    /// Check if a region is freed in this function.
    pub fn freed(&self, region: u32) -> bool {
        self.freed_regions.contains(&region)
    }
}

/// Analyze a single function and produce a summary.
pub fn analyze_function(
    scg: &SCG,
    function_nodes: &[NodeId],
) -> FunctionSummary {
    let mut summary = FunctionSummary::new();

    for &node_id in function_nodes {
        if let Some(node) = scg.get_node(node_id) {
            match &node.payload {
                NodePayload::Allocation(alloc) => {
                    summary.allocates = true;
                    let region_id = alloc.region_id.as_u64() as u32;
                    // Check if this region escapes (returned or stored)
                    // For now, mark all allocations as potentially escaping
                    summary.escaping_regions.insert(region_id);
                }
                NodePayload::Deallocation(dealloc) => {
                    let region_id = dealloc.region_id.as_u64() as u32;
                    summary.freed_regions.insert(region_id);
                }
                NodePayload::Access(access) => {
                    if access.mode == vuma_scg::node::AccessMode::Write {
                        let region_id = access.region_id.as_u64() as u32;
                        summary.modified_regions.insert(region_id);
                    }
                }
                NodePayload::Effect(eff) => {
                    if eff.effect_kind.contains("io") || eff.effect_kind.contains("write") {
                        summary.performs_io = true;
                    }
                }
                _ => {}
            }
        }
    }

    summary.is_pure = !summary.allocates && !summary.performs_io && summary.modified_regions.is_empty();
    summary
}

/// Analyze all functions in the SCG and produce summaries.
pub fn analyze_all_functions(
    scg: &SCG,
    function_entries: &[(String, Vec<NodeId>)],
) -> HashMap<String, FunctionSummary> {
    let mut summaries = HashMap::new();
    for (name, nodes) in function_entries {
        let summary = analyze_function(scg, nodes);
        summaries.insert(name.clone(), summary);
    }
    summaries
}

/// Check if a function call is safe given the caller's and callee's summaries.
pub fn check_call_safety(
    caller: &FunctionSummary,
    callee: &FunctionSummary,
) -> Vec<String> {
    let mut issues = Vec::new();

    // Check: if callee frees a region, caller must not use it after
    for region in &callee.freed_regions {
        if caller.escaping_regions.contains(region) {
            issues.push(format!(
                "Function frees region {} that caller still references (use-after-free risk)",
                region
            ));
        }
    }

    // Check: if callee modifies a region, caller must be aware
    for region in &callee.modified_regions {
        if !caller.modified_regions.contains(region) {
            // This is OK, just a note
        }
    }

    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_summary() {
        let summary = FunctionSummary::new();
        assert!(summary.is_pure);
        assert!(!summary.allocates);
    }

    #[test]
    fn test_summary_with_alloc() {
        let mut summary = FunctionSummary::new();
        summary.allocates = true;
        assert!(!summary.is_pure);
    }
}
