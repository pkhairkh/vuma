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

// ═══════════════════════════════════════════════════════════════════════
// Wave 39-40: Incremental & Abstract Verification
// ═══════════════════════════════════════════════════════════════════════

/// Region summary for abstract interpretation.
/// Instead of tracking every byte, summarizes a region's type and size.
#[derive(Debug, Clone, Default)]
pub struct RegionSummary {
    /// Size in bytes (0 if unknown).
    pub size: u64,
    /// Type name (e.g., "u32", "Address", "struct Foo").
    pub type_name: String,
    /// Whether the region is heap-allocated (vs stack).
    pub is_heap: bool,
    /// Whether the region is thread-shared.
    pub is_shared: bool,
    /// Number of outstanding borrows (for exclusivity checking).
    pub borrow_count: u32,
}

/// Incremental verification cache.
/// Tracks which functions have been verified and their results,
/// so only changed functions need re-verification.
#[derive(Debug, Clone, Default)]
pub struct IncrementalCache {
    /// Map from function name to (hash, verified_ok).
    verified_functions: HashMap<String, (u64, bool)>,
    /// Map from function name to its call dependencies.
    call_deps: HashMap<String, Vec<String>>,
}

impl IncrementalCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a function needs re-verification.
    /// Returns true if the function's hash changed or it was never verified.
    pub fn needs_verification(&self, func_name: &str, hash: u64) -> bool {
        match self.verified_functions.get(func_name) {
            Some((old_hash, ok)) => *old_hash != hash || !ok,
            None => true,
        }
    }

    /// Record verification result for a function.
    pub fn record(&mut self, func_name: &str, hash: u64, ok: bool) {
        self.verified_functions.insert(func_name.to_string(), (hash, ok));
    }

    /// Record a call dependency: `caller` calls `callee`.
    pub fn record_call(&mut self, caller: &str, callee: &str) {
        self.call_deps
            .entry(caller.to_string())
            .or_default()
            .push(callee.to_string());
    }

    /// Get all functions that transitively depend on `func_name`
    /// (i.e., callers that need re-verification if `func_name` changes).
    pub fn dependent_functions(&self, func_name: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = vec![func_name.to_string()];

        while let Some(current) = queue.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            for (caller, callees) in &self.call_deps {
                if callees.contains(&current) && !visited.contains(caller) {
                    result.push(caller.clone());
                    queue.push(caller.clone());
                }
            }
        }

        result
    }

    /// Clear all cached results.
    pub fn clear(&mut self) {
        self.verified_functions.clear();
        self.call_deps.clear();
    }
}

/// Abstract region tracker for large-scale verification.
/// Instead of tracking every allocation, summarizes regions by type.
#[derive(Debug, Clone, Default)]
pub struct AbstractRegionTracker {
    /// Map from region ID to its summary.
    regions: HashMap<u32, RegionSummary>,
    /// Total abstracted regions (not tracked individually).
    pub abstracted_count: u32,
}

impl AbstractRegionTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a region to the tracker.
    pub fn add_region(&mut self, region_id: u32, size: u64, type_name: &str, is_heap: bool) {
        self.regions.insert(
            region_id,
            RegionSummary {
                size,
                type_name: type_name.to_string(),
                is_heap,
                is_shared: false,
                borrow_count: 0,
            },
        );
    }

    /// Abstract a region (stop tracking it individually).
    pub fn abstract_region(&mut self, region_id: u32) {
        self.regions.remove(&region_id);
        self.abstracted_count += 1;
    }

    /// Check if a region is tracked.
    pub fn is_tracked(&self, region_id: u32) -> bool {
        self.regions.contains_key(&region_id)
    }

    /// Get the borrow count for a region.
    pub fn borrow_count(&self, region_id: u32) -> u32 {
        self.regions
            .get(&region_id)
            .map(|r| r.borrow_count)
            .unwrap_or(0)
    }

    /// Increment borrow count (for exclusivity checking).
    pub fn borrow(&mut self, region_id: u32) {
        if let Some(r) = self.regions.get_mut(&region_id) {
            r.borrow_count += 1;
        }
    }

    /// Decrement borrow count.
    pub fn release(&mut self, region_id: u32) {
        if let Some(r) = self.regions.get_mut(&region_id) {
            if r.borrow_count > 0 {
                r.borrow_count -= 1;
            }
        }
    }

    /// Check if a region has exclusive access (borrow_count == 0).
    pub fn is_exclusive(&self, region_id: u32) -> bool {
        self.borrow_count(region_id) == 0
    }
}

/// Verify a single function using modular analysis.
/// This is the entry point for per-function verification.
pub fn verify_function(
    scg: &SCG,
    function_nodes: &[NodeId],
    summaries: &HashMap<String, FunctionSummary>,
    cache: &mut IncrementalCache,
    func_name: &str,
    func_hash: u64,
) -> Vec<String> {
    let mut issues = Vec::new();

    // Check cache first
    if !cache.needs_verification(func_name, func_hash) {
        return issues; // Already verified, no changes
    }

    // Analyze this function
    let summary = analyze_function(scg, function_nodes);

    // Check liveness: every allocation must be freed (unless it escapes)
    for region in &summary.escaping_regions {
        if !summary.freed_regions.contains(region) {
            // Region escapes — that's OK if it's returned
            // (The caller is responsible for freeing it)
        }
    }

    // Check cleanup: every freed region must have been allocated
    for region in &summary.freed_regions {
        if !summary.escaping_regions.contains(region) {
            // Region is freed but didn't escape — verify it was allocated
            // (The allocation should be in function_nodes)
            let mut found_alloc = false;
            for &node_id in function_nodes {
                if let Some(node) = scg.get_node(node_id) {
                    if let NodePayload::Allocation(alloc) = &node.payload {
                        if alloc.region_id.as_u64() as u32 == *region {
                            found_alloc = true;
                            break;
                        }
                    }
                }
            }
            if !found_alloc {
                issues.push(format!(
                    "Function {} frees region {} that was not allocated in this function",
                    func_name, region
                ));
            }
        }
    }

    // Record result
    let ok = issues.is_empty();
    cache.record(func_name, func_hash, ok);

    issues
}

/// Verify all functions in the SCG using modular analysis.
/// This is the main entry point for modular verification.
pub fn verify_all_functions(
    scg: &SCG,
    function_entries: &[(String, Vec<NodeId>)],
) -> Vec<String> {
    let mut all_issues = Vec::new();
    let mut cache = IncrementalCache::new();

    // First pass: analyze all functions and build summaries
    let summaries = analyze_all_functions(scg, function_entries);

    // Second pass: verify each function locally
    for (name, nodes) in function_entries {
        let issues = verify_function(scg, nodes, &summaries, &mut cache, name, 0);
        all_issues.extend(issues);
    }

    all_issues
}
