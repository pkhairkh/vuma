//! MSG-based liveness invariant checker.
//!
//! This module implements Invariant 1 (Liveness) from the VUMA specification:
//! "Every access targets allocated memory."
//!
//! The checker works directly on the Memory State Graph (MSG) and performs
//! four complementary analyses:
//!
//! 1. **Use-after-free detection** — each access must target a region that is
//!    live (Allocated / Stack / Mapped / Device) at the access program point.
//!
//! 2. **Bounds checking** — the byte range of each access must be fully
//!    contained within the target region's address range.
//!
//! 3. **Derivation-after-free detection** — a derivation whose source region
//!    has already been freed at the point where the derivation is used
//!    constitutes a liveness violation.
//!
//! 4. **Circular wait dependency detection** — a wait-for graph is built from
//!    synchronisation edges between accesses on different regions; cycles in
//!    this graph indicate potential deadlocks where no region can be freed
//!    first.  Tarjan's strongly-connected-components (SCC) algorithm is used
//!    to detect such cycles.

use crate::access::{Access, AccessId};
use crate::address::Address;
use crate::derivation::{DerivationId, DerivationSource};
use crate::msg::MSG;
use crate::program_point::ProgramPoint;
use crate::region::{RegionId, RegionStatus};
use std::fmt;

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/// The outcome of checking the liveness invariant on an MSG.
#[derive(Debug, Clone)]
pub struct InvariantResult {
    /// `true` if no violations were found.
    pub satisfied: bool,
    /// The set of violations detected, in no particular order.
    pub violations: Vec<LivenessViolation>,
}

impl InvariantResult {
    /// An empty, satisfied result.
    pub fn ok() -> Self {
        Self {
            satisfied: true,
            violations: Vec::new(),
        }
    }

    /// A result with a single violation.
    pub fn fail(violation: LivenessViolation) -> Self {
        Self {
            satisfied: false,
            violations: vec![violation],
        }
    }

    /// Merge another result into this one.
    pub fn merge(&mut self, other: InvariantResult) {
        if !other.satisfied {
            self.satisfied = false;
        }
        self.violations.extend(other.violations);
    }
}

impl fmt::Display for InvariantResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.satisfied {
            write!(f, "Liveness invariant: SATISFIED")?;
        } else {
            write!(f, "Liveness invariant: VIOLATED ({} violation(s))", self.violations.len())?;
        }
        Ok(())
    }
}

/// A single violation of the liveness invariant.
#[derive(Debug, Clone)]
pub enum LivenessViolation {
    /// An access targets a region that has already been freed (use-after-free).
    UseAfterFree {
        region_id: RegionId,
        access_id: AccessId,
        free_point: ProgramPoint,
        access_point: ProgramPoint,
    },

    /// A region is allocated but never freed and not marked as Leaked, Stack,
    /// Mapped, or Device — i.e. it appears to be a memory leak.
    RegionNeverFreed {
        region_id: RegionId,
        alloc_point: ProgramPoint,
    },

    /// A derivation is used (i.e. an access targets it) after the source
    /// region has been freed.
    DerivationUsedAfterFree {
        derivation_id: DerivationId,
        region_id: RegionId,
        access_id: AccessId,
    },

    /// An access's byte range is not fully contained within the target
    /// region's address range.
    AccessOutOfBounds {
        access_id: AccessId,
        region_id: RegionId,
        access_start: Address,
        access_end: Address,
        region_start: Address,
        region_end: Address,
    },

    /// A cycle in the wait-for graph — regions that mutually wait for each
    /// other, preventing any of them from being freed (potential deadlock).
    CircularWaitDependency {
        cycle: Vec<RegionId>,
    },
}

impl fmt::Display for LivenessViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LivenessViolation::UseAfterFree {
                region_id,
                access_id,
                free_point,
                access_point,
            } => write!(
                f,
                "Use-after-free: access {} at {} targets region {} freed at {}",
                access_id, access_point, region_id, free_point
            ),
            LivenessViolation::RegionNeverFreed {
                region_id,
                alloc_point,
            } => write!(
                f,
                "Region {} allocated at {} is never freed and not marked Leaked",
                region_id, alloc_point
            ),
            LivenessViolation::DerivationUsedAfterFree {
                derivation_id,
                region_id,
                access_id,
            } => write!(
                f,
                "Derivation {} from region {} used by access {} after region freed",
                derivation_id, region_id, access_id
            ),
            LivenessViolation::AccessOutOfBounds {
                access_id,
                region_id,
                access_start,
                access_end,
                region_start,
                region_end,
            } => write!(
                f,
                "Access {} out of bounds: [{}, {}) not within region {} [{}, {})",
                access_id, access_start, access_end, region_id, region_start, region_end
            ),
            LivenessViolation::CircularWaitDependency { cycle } => {
                write!(f, "Circular wait dependency:")?;
                for rid in cycle {
                    write!(f, " {}", rid)?;
                }
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Wait-for graph
// ---------------------------------------------------------------------------

/// A directed graph over [`RegionId`] nodes used to detect circular wait
/// dependencies.  An edge `R1 → R2` means that the liveness of `R1` depends
/// on `R2` remaining alive — specifically, there exists a synchronisation
/// edge that orders an access on `R1` before an access on `R2`.
#[derive(Debug, Clone, Default)]
struct WaitForGraph {
    /// Adjacency list: node → set of successors.
    edges: hashbrown::HashMap<RegionId, hashbrown::HashSet<RegionId>>,
}

impl WaitForGraph {
    fn new() -> Self {
        Self::default()
    }

    /// Add a node to the graph (no-op if it already exists).
    fn add_node(&mut self, id: RegionId) {
        self.edges.entry(id).or_default();
    }

    /// Add a directed edge `from → to`.
    fn add_edge(&mut self, from: RegionId, to: RegionId) {
        if from != to {
            self.edges.entry(from).or_default().insert(to);
            self.edges.entry(to).or_default();
        }
    }

    /// All nodes in the graph.
    fn nodes(&self) -> impl Iterator<Item = RegionId> + '_ {
        self.edges.keys().copied()
    }

    /// Successors of a node.
    fn successors(&self, id: RegionId) -> Vec<RegionId> {
        match self.edges.get(&id) {
            Some(s) => s.iter().copied().collect(),
            None => Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tarjan's strongly-connected-components algorithm
// ---------------------------------------------------------------------------

/// Run Tarjan's SCC algorithm on the given wait-for graph.
///
/// Returns a vector of SCCs.  Each SCC with more than one node represents
/// a cycle in the wait-for graph — a circular wait dependency.
fn tarjan_scc(graph: &WaitForGraph) -> Vec<Vec<RegionId>> {
    let mut index_counter: u64 = 0;
    let mut stack: Vec<RegionId> = Vec::new();
    let mut on_stack: hashbrown::HashSet<RegionId> = hashbrown::HashSet::new();
    let mut indices: hashbrown::HashMap<RegionId, u64> = hashbrown::HashMap::new();
    let mut lowlinks: hashbrown::HashMap<RegionId, u64> = hashbrown::HashMap::new();
    let mut sccs: Vec<Vec<RegionId>> = Vec::new();

    let all_nodes: Vec<RegionId> = graph.nodes().collect();

    for node in all_nodes {
        if !indices.contains_key(&node) {
            tarjan_dfs(
                node,
                graph,
                &mut index_counter,
                &mut stack,
                &mut on_stack,
                &mut indices,
                &mut lowlinks,
                &mut sccs,
            );
        }
    }

    sccs
}

fn tarjan_dfs(
    v: RegionId,
    graph: &WaitForGraph,
    index_counter: &mut u64,
    stack: &mut Vec<RegionId>,
    on_stack: &mut hashbrown::HashSet<RegionId>,
    indices: &mut hashbrown::HashMap<RegionId, u64>,
    lowlinks: &mut hashbrown::HashMap<RegionId, u64>,
    sccs: &mut Vec<Vec<RegionId>>,
) {
    let v_index = *index_counter;
    indices.insert(v, v_index);
    lowlinks.insert(v, v_index);
    *index_counter += 1;
    stack.push(v);
    on_stack.insert(v);

    for w in graph.successors(v) {
        if !indices.contains_key(&w) {
            // w has not been visited; recurse
            tarjan_dfs(w, graph, index_counter, stack, on_stack, indices, lowlinks, sccs);
            let v_low = *lowlinks.get(&v).unwrap();
            let w_low = *lowlinks.get(&w).unwrap();
            lowlinks.insert(v, v_low.min(w_low));
        } else if on_stack.contains(&w) {
            // w is on the current stack — back edge
            let v_low = *lowlinks.get(&v).unwrap();
            let w_index = *indices.get(&w).unwrap();
            lowlinks.insert(v, v_low.min(w_index));
        }
    }

    // If v is a root node, pop the SCC
    if lowlinks.get(&v).copied() == indices.get(&v).copied() {
        let mut scc = Vec::new();
        loop {
            let w = stack.pop().expect("stack should not be empty in Tarjan's");
            on_stack.remove(&w);
            scc.push(w);
            if w == v {
                break;
            }
        }
        sccs.push(scc);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the root [`RegionId`] for a derivation by walking the derivation
/// chain in the MSG.  Returns `None` if the chain is broken.
fn region_of_derivation(msg: &MSG, deriv_id: DerivationId) -> Option<RegionId> {
    let deriv = msg.derivation(deriv_id)?;
    deriv.base_region(|id| msg.derivation(id).cloned())
}

/// Resolve the root [`RegionId`] for an access by following the access's
/// target derivation chain.
fn region_of_access(msg: &MSG, access: &Access) -> Option<RegionId> {
    region_of_derivation(msg, access.target)
}

/// Check whether a region is live at the given program point.
///
/// A region is considered live at `pp` if:
/// - its status is `Allocated` and `pp` falls in `[alloc_point, free_point)`,
/// - its status is `Stack`, `Mapped`, or `Device` (always live for our
///   purposes — the caller should check lifetimes more precisely if needed).
/// - its status is `Freed`, it is *not* live.
/// - its status is `Leaked`, it is *not* live (the region has been
///   deliberately abandoned).
fn is_region_live_at(msg: &MSG, region_id: RegionId, pp: &ProgramPoint) -> bool {
    let region = match msg.region(region_id) {
        Some(r) => r,
        None => return false,
    };

    match region.status {
        RegionStatus::Stack | RegionStatus::Mapped | RegionStatus::Device => true,
        RegionStatus::Freed | RegionStatus::Leaked => false,
        RegionStatus::Allocated => {
            // Region must have been allocated before `pp`.
            if &region.alloc_point > pp {
                return false;
            }
            // If the region has been freed, `pp` must be before the free point.
            match &region.free_point {
                Some(fp) => pp < fp,
                None => true,
            }
        }
    }
}

/// Build the wait-for graph from the MSG.
///
/// For each synchronisation edge `access1 ─[ordering]→ access2` where
/// `access1` targets region `R1` and `access2` targets region `R2`,
/// we add an edge `R1 → R2` (R1 "waits for" R2, since the completion
/// of R1's access must precede R2's access).
fn build_wait_for_graph(msg: &MSG) -> WaitForGraph {
    let mut graph = WaitForGraph::new();

    // Ensure every region is a node.
    for region in msg.regions() {
        graph.add_node(region.id);
    }

    // Add edges from sync relationships.
    for edge in msg.sync_edges() {
        let access1 = match msg.access(edge.access1) {
            Some(a) => a,
            None => continue,
        };
        let access2 = match msg.access(edge.access2) {
            Some(a) => a,
            None => continue,
        };

        let r1 = match region_of_access(msg, access1) {
            Some(r) => r,
            None => continue,
        };
        let r2 = match region_of_access(msg, access2) {
            Some(r) => r,
            None => continue,
        };

        // R1 must be alive when R2 is accessed (temporal dependency).
        if r1 != r2 {
            graph.add_edge(r1, r2);
        }
    }

    graph
}

// ---------------------------------------------------------------------------
// Main checker
// ---------------------------------------------------------------------------

/// Check the liveness invariant on the given MSG.
///
/// This performs the four sub-analyses described in the module-level docs
/// and returns an [`InvariantResult`] describing whether the invariant holds
/// and what violations (if any) were found.
pub fn check_liveness(msg: &MSG) -> InvariantResult {
    let mut result = InvariantResult::ok();

    check_access_liveness(msg, &mut result);
    check_region_eventual_free(msg, &mut result);
    check_derivation_liveness(msg, &mut result);
    check_circular_wait(msg, &mut result);

    result
}

/// Sub-analysis 1: every access must target a live region.
fn check_access_liveness(msg: &MSG, result: &mut InvariantResult) {
    for access in msg.accesses() {
        let region_id = match region_of_access(msg, access) {
            Some(rid) => rid,
            None => continue,
        };

        if !is_region_live_at(msg, region_id, &access.program_point) {
            // Determine the free point for the violation message.
            let region = msg.region(region_id);
            let free_point = region
                .and_then(|r| r.free_point.clone())
                .unwrap_or_else(|| ProgramPoint::new("<unknown>", 0, 0));

            result.satisfied = false;
            result.violations.push(LivenessViolation::UseAfterFree {
                region_id,
                access_id: access.id,
                free_point,
                access_point: access.program_point.clone(),
            });
        }

        // Bounds check: the access byte range must be within the region.
        if let Some(region) = msg.region(region_id) {
            // Derive the start address from the derivation's provenance range.
            let deriv = msg.derivation(access.target);
            if let Some(d) = deriv {
                let access_start = d.proven_range.0;
                let access_end = access_start + access.size;
                let region_end = region.end();

                if access_start < region.base || access_end > region_end {
                    result.satisfied = false;
                    result.violations.push(LivenessViolation::AccessOutOfBounds {
                        access_id: access.id,
                        region_id,
                        access_start,
                        access_end,
                        region_start: region.base,
                        region_end,
                    });
                }
            }
        }
    }
}

/// Sub-analysis 2: every region should eventually be freed or explicitly
/// marked as Leaked / Stack / Mapped / Device.
fn check_region_eventual_free(msg: &MSG, result: &mut InvariantResult) {
    for region in msg.regions() {
        match region.status {
            RegionStatus::Allocated => {
                // Allocated regions must eventually be freed.
                if region.free_point.is_none() {
                    result.satisfied = false;
                    result.violations.push(LivenessViolation::RegionNeverFreed {
                        region_id: region.id,
                        alloc_point: region.alloc_point.clone(),
                    });
                }
            }
            RegionStatus::Freed
            | RegionStatus::Stack
            | RegionStatus::Mapped
            | RegionStatus::Device
            | RegionStatus::Leaked => {
                // These statuses are acceptable — the region has been dealt with.
            }
        }
    }
}

/// Sub-analysis 3: derivations must not be used after the source region is
/// freed.
///
/// We check that for every derivation whose source is a region, and for
/// every access targeting that derivation, the access occurs before the
/// source region is freed.
fn check_derivation_liveness(msg: &MSG, result: &mut InvariantResult) {
    for access in msg.accesses() {
        // Walk the derivation chain; check each step whose source is a region.
        let chain = msg.derivation_chain(access.target);
        for deriv in &chain {
            let source_region_id = match &deriv.source {
                DerivationSource::Region(rid) => *rid,
                DerivationSource::AnotherDerivation(_) => continue,
            };

            // Check whether the source region was freed before this access.
            if let Some(region) = msg.region(source_region_id) {
                if let Some(ref fp) = region.free_point {
                    if &access.program_point >= fp {
                        result.satisfied = false;
                        result.violations.push(LivenessViolation::DerivationUsedAfterFree {
                            derivation_id: deriv.id,
                            region_id: source_region_id,
                            access_id: access.id,
                        });
                    }
                }
            }
        }
    }
}

/// Sub-analysis 4: detect circular wait dependencies via Tarjan's SCC
/// algorithm on the wait-for graph.
fn check_circular_wait(msg: &MSG, result: &mut InvariantResult) {
    let graph = build_wait_for_graph(msg);
    let sccs = tarjan_scc(&graph);

    for scc in sccs {
        // An SCC with more than one node is a cycle.
        // A single-node SCC is only a cycle if it has a self-loop (which we
        // exclude by construction in add_edge — `from != to`), so single-node
        // SCCs are not violations.
        if scc.len() > 1 {
            result.satisfied = false;
            result.violations.push(LivenessViolation::CircularWaitDependency {
                cycle: scc,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::AccessKind;
    use crate::address::Address;
    use crate::derivation::{Derivation, DerivationKind, DerivationSource};
    use crate::msg::MSG;
    use crate::program_point::ProgramPoint;
    use crate::region::{Region, RegionId, RegionStatus};
    use crate::sync::{Ordering, SyncEdge, SyncEdgeId};
    use crate::access::AccessId;
    use crate::derivation::DerivationId;

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    /// Helper: create a heap region with the given id, status, and optional free_point.
    fn make_region(id: u64, status: RegionStatus, free_line: Option<u32>) -> Region {
        Region {
            id: RegionId(id),
            base: Address::from(0x1000_u64 + (id as u64) * 0x1000),
            size: 0x200,
            status,
            alloc_point: dummy_pp(1),
            free_point: free_line.map(|l| dummy_pp(l)),
            owner_context: None,
        }
    }

    /// Helper: create a direct derivation from a region.
    fn make_direct_derivation(id: u64, region_id: u64) -> Derivation {
        let base = Address::from(0x1000_u64 + (region_id as u64) * 0x1000);
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(region_id)),
            kind: DerivationKind::Direct,
            proven_range: (base, base + 0x200),
        }
    }

    /// Helper: create an offset derivation from another derivation.
    fn make_offset_derivation(id: u64, parent_id: u64, offset: i64) -> Derivation {
        let parent_base = Address::from(0x1000_u64);
        let lo = parent_base.offset(offset);
        let hi = lo + 0x40;
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::AnotherDerivation(DerivationId(parent_id)),
            kind: DerivationKind::Offset { by: offset },
            proven_range: (lo, hi),
        }
    }

    /// Helper: create a read access.
    fn make_read_access(id: u64, target: DerivationId, line: u32, size: u64) -> Access {
        Access::new(AccessId(id), target, AccessKind::Read, size, dummy_pp(line))
    }

    // ----- Test 1: Satisfied — simple alloc, use, free -----

    #[test]
    fn liveness_satisfied_simple() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, RegionStatus::Allocated, Some(30)));
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(make_read_access(100, DerivationId(10), 10, 4));

        let result = check_liveness(&msg);
        assert!(result.satisfied, "Expected satisfied, got violations: {:?}", result.violations);
    }

    // ----- Test 2: Use-after-free -----

    #[test]
    fn use_after_free_detected() {
        let mut msg = MSG::new();
        // Region freed at line 5, but accessed at line 10.
        msg.add_region(make_region(1, RegionStatus::Freed, Some(5)));
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(make_read_access(100, DerivationId(10), 10, 4));

        let result = check_liveness(&msg);
        assert!(!result.satisfied);
        let has_uaf = result.violations.iter().any(|v| matches!(v, LivenessViolation::UseAfterFree { .. }));
        assert!(has_uaf, "Expected UseAfterFree violation, got: {:?}", result.violations);
    }

    // ----- Test 3: Region never freed -----

    #[test]
    fn region_never_freed_detected() {
        let mut msg = MSG::new();
        // Allocated but no free_point.
        msg.add_region(make_region(1, RegionStatus::Allocated, None));

        let result = check_liveness(&msg);
        assert!(!result.satisfied);
        let has_never_freed = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::RegionNeverFreed { .. })
        });
        assert!(has_never_freed, "Expected RegionNeverFreed violation, got: {:?}", result.violations);
    }

    // ----- Test 4: Region marked Leaked is acceptable -----

    #[test]
    fn leaked_region_is_acceptable() {
        let mut msg = MSG::new();
        msg.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            status: RegionStatus::Leaked,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        });
        // Add a derivation and access before the "leak" (no free_point → still live at access).
        // Actually, Leaked status means not live; but a Leaked region without accesses is fine.
        // Just checking that RegionNeverFreed is NOT reported for Leaked regions.

        let result = check_liveness(&msg);
        let has_never_freed = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::RegionNeverFreed { .. })
        });
        assert!(!has_never_freed, "Leaked region should not trigger RegionNeverFreed");
    }

    // ----- Test 5: Circular wait dependency -----

    #[test]
    fn circular_wait_detected() {
        let mut msg = MSG::new();

        // Two regions, both live.
        msg.add_region(make_region(1, RegionStatus::Allocated, Some(100)));
        msg.add_region(make_region(2, RegionStatus::Allocated, Some(100)));

        // Derivations.
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_derivation(make_direct_derivation(20, 2));

        // Accesses: a1 on region 1, a2 on region 2, a3 on region 1, a4 on region 2.
        msg.add_access(make_read_access(101, DerivationId(10), 10, 4));
        msg.add_access(make_read_access(102, DerivationId(20), 20, 4));
        msg.add_access(make_read_access(103, DerivationId(10), 30, 4));
        msg.add_access(make_read_access(104, DerivationId(20), 40, 4));

        // Sync edges: R1 access before R2 access (a1 → a2), and R2 access before R1 access (a4 → a3).
        // This creates a cycle: R1 → R2 → R1.
        msg.add_sync_edge(SyncEdge::new(
            SyncEdgeId(1),
            AccessId(101),
            AccessId(102),
            Ordering::HappensBefore,
        ));
        msg.add_sync_edge(SyncEdge::new(
            SyncEdgeId(2),
            AccessId(104),
            AccessId(103),
            Ordering::HappensBefore,
        ));

        let result = check_liveness(&msg);
        let has_circular = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::CircularWaitDependency { .. })
        });
        assert!(has_circular, "Expected CircularWaitDependency, got: {:?}", result.violations);
    }

    // ----- Test 6: No circular wait (acyclic sync) -----

    #[test]
    fn no_circular_wait_when_acyclic() {
        let mut msg = MSG::new();

        msg.add_region(make_region(1, RegionStatus::Allocated, Some(100)));
        msg.add_region(make_region(2, RegionStatus::Allocated, Some(100)));

        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_derivation(make_direct_derivation(20, 2));

        msg.add_access(make_read_access(101, DerivationId(10), 10, 4));
        msg.add_access(make_read_access(102, DerivationId(20), 20, 4));

        // Only one sync edge: R1 → R2 (no cycle).
        msg.add_sync_edge(SyncEdge::new(
            SyncEdgeId(1),
            AccessId(101),
            AccessId(102),
            Ordering::HappensBefore,
        ));

        let result = check_liveness(&msg);
        let has_circular = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::CircularWaitDependency { .. })
        });
        assert!(!has_circular, "No circular wait expected, got: {:?}", result.violations);
    }

    // ----- Test 7: Access out of bounds -----

    #[test]
    fn access_out_of_bounds_detected() {
        let mut msg = MSG::new();

        // Region at 0x1000, size 0x10 (small region).
        msg.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x10,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: Some(dummy_pp(50)),
            owner_context: None,
        });

        // Derivation with proven_range starting at 0x1000.
        msg.add_derivation(Derivation {
            id: DerivationId(10),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x1010_u64)),
        });

        // Access 32 bytes starting at 0x1000 → exceeds region end 0x1010.
        msg.add_access(make_read_access(100, DerivationId(10), 10, 32));

        let result = check_liveness(&msg);
        assert!(!result.satisfied);
        let has_oob = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::AccessOutOfBounds { .. })
        });
        assert!(has_oob, "Expected AccessOutOfBounds violation, got: {:?}", result.violations);
    }

    // ----- Test 8: Derivation used after source freed -----

    #[test]
    fn derivation_used_after_free() {
        let mut msg = MSG::new();

        // Region freed at line 5.
        msg.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            status: RegionStatus::Freed,
            alloc_point: dummy_pp(1),
            free_point: Some(dummy_pp(5)),
            owner_context: None,
        });

        // Direct derivation from region 1.
        msg.add_derivation(make_direct_derivation(10, 1));

        // Access at line 10 (after free at line 5).
        msg.add_access(make_read_access(100, DerivationId(10), 10, 4));

        let result = check_liveness(&msg);
        assert!(!result.satisfied);

        // Should have both UseAfterFree and DerivationUsedAfterFree.
        let has_derivation_after_free = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::DerivationUsedAfterFree { .. })
        });
        assert!(has_derivation_after_free, "Expected DerivationUsedAfterFree, got: {:?}", result.violations);
    }

    // ----- Test 9: Tarjan SCC on a simple 3-node cycle -----

    #[test]
    fn tarjan_detects_three_node_cycle() {
        let mut graph = WaitForGraph::new();
        graph.add_edge(RegionId(1), RegionId(2));
        graph.add_edge(RegionId(2), RegionId(3));
        graph.add_edge(RegionId(3), RegionId(1));

        let sccs = tarjan_scc(&graph);
        let cyclic_sccs: Vec<_> = sccs.into_iter().filter(|scc| scc.len() > 1).collect();
        assert_eq!(cyclic_sccs.len(), 1, "Expected exactly one cyclic SCC");
        let cycle = &cyclic_sccs[0];
        assert_eq!(cycle.len(), 3);
    }

    // ----- Test 10: Tarjan SCC on a DAG (no cycles) -----

    #[test]
    fn tarjan_no_cycles_on_dag() {
        let mut graph = WaitForGraph::new();
        graph.add_edge(RegionId(1), RegionId(2));
        graph.add_edge(RegionId(2), RegionId(3));
        graph.add_edge(RegionId(1), RegionId(3));

        let sccs = tarjan_scc(&graph);
        let cyclic_sccs: Vec<_> = sccs.into_iter().filter(|scc| scc.len() > 1).collect();
        assert!(cyclic_sccs.is_empty(), "No cycles expected in DAG");
    }

    // ----- Test 11: Stack region is always live -----

    #[test]
    fn stack_region_always_live() {
        let mut msg = MSG::new();
        msg.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            status: RegionStatus::Stack,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        });
        msg.add_derivation(make_direct_derivation(10, 1));
        msg.add_access(make_read_access(100, DerivationId(10), 10, 4));

        // No RegionNeverFreed for Stack; no UseAfterFree.
        let result = check_liveness(&msg);
        assert!(
            !result.violations.iter().any(|v| matches!(v, LivenessViolation::RegionNeverFreed { .. })),
            "Stack region should not trigger RegionNeverFreed"
        );
        assert!(
            !result.violations.iter().any(|v| matches!(v, LivenessViolation::UseAfterFree { .. })),
            "Stack region access should not trigger UseAfterFree"
        );
    }

    // ----- Test 12: Display formatting -----

    #[test]
    fn violation_display_formatting() {
        let v = LivenessViolation::UseAfterFree {
            region_id: RegionId(1),
            access_id: AccessId(10),
            free_point: ProgramPoint::new("main.vu", 5, 1),
            access_point: ProgramPoint::new("main.vu", 10, 1),
        };
        let s = format!("{}", v);
        assert!(s.contains("Use-after-free"), "Display should contain 'Use-after-free': {}", s);
        assert!(s.contains("R1"), "Display should contain region id: {}", s);

        let v2 = LivenessViolation::CircularWaitDependency {
            cycle: vec![RegionId(1), RegionId(2), RegionId(3)],
        };
        let s2 = format!("{}", v2);
        assert!(s2.contains("Circular wait"), "Display should contain 'Circular wait': {}", s2);
    }

    // ----- Test 13: Mapped and Device regions are acceptable -----

    #[test]
    fn mapped_and_device_regions_acceptable() {
        let mut msg = MSG::new();

        msg.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            status: RegionStatus::Mapped,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        });
        msg.add_region(Region {
            id: RegionId(2),
            base: Address::from(0x3000_u64),
            size: 0x200,
            status: RegionStatus::Device,
            alloc_point: dummy_pp(2),
            free_point: None,
            owner_context: None,
        });

        let result = check_liveness(&msg);
        assert!(
            !result.violations.iter().any(|v| matches!(v, LivenessViolation::RegionNeverFreed { .. })),
            "Mapped/Device regions should not trigger RegionNeverFreed"
        );
    }

    // ----- Test 14: InvariantResult::merge -----

    #[test]
    fn invariant_result_merge() {
        let mut r1 = InvariantResult::ok();
        let r2 = InvariantResult::fail(LivenessViolation::RegionNeverFreed {
            region_id: RegionId(1),
            alloc_point: dummy_pp(1),
        });
        r1.merge(r2);
        assert!(!r1.satisfied);
        assert_eq!(r1.violations.len(), 1);

        let r3 = InvariantResult::ok();
        r1.merge(r3);
        assert!(!r1.satisfied); // still violated
        assert_eq!(r1.violations.len(), 1);
    }

    // ----- Test 15: Access within bounds -----

    #[test]
    fn access_within_bounds_ok() {
        let mut msg = MSG::new();

        msg.add_region(Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x200,
            status: RegionStatus::Allocated,
            alloc_point: dummy_pp(1),
            free_point: Some(dummy_pp(50)),
            owner_context: None,
        });

        msg.add_derivation(Derivation {
            id: DerivationId(10),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x1000_u64), Address::from(0x1200_u64)),
        });

        // 4-byte access at 0x1000, well within 0x1200.
        msg.add_access(make_read_access(100, DerivationId(10), 10, 4));

        let result = check_liveness(&msg);
        assert!(
            !result.violations.iter().any(|v| matches!(v, LivenessViolation::AccessOutOfBounds { .. })),
            "Access within bounds should not trigger AccessOutOfBounds"
        );
    }

    // ----- Test 16: Offset derivation chain with use-after-free -----

    #[test]
    fn chained_derivation_use_after_free() {
        let mut msg = MSG::new();

        // Region freed at line 5.
        msg.add_region(make_region(1, RegionStatus::Freed, Some(5)));

        // d1: direct from region 1
        msg.add_derivation(make_direct_derivation(1, 1));

        // d2: offset from d1
        msg.add_derivation(make_offset_derivation(2, 1, 0x40));

        // Access d2 at line 10 (after free).
        msg.add_access(make_read_access(100, DerivationId(2), 10, 4));

        let result = check_liveness(&msg);
        assert!(!result.satisfied);

        let has_derivation_violation = result.violations.iter().any(|v| {
            matches!(v, LivenessViolation::DerivationUsedAfterFree { .. })
        });
        assert!(has_derivation_violation, "Expected DerivationUsedAfterFree for chained derivation");
    }

    // ----- Test 17: Empty MSG satisfies liveness trivially -----

    #[test]
    fn empty_msg_satisfies_liveness() {
        let msg = MSG::new();
        let result = check_liveness(&msg);
        assert!(result.satisfied, "Empty MSG should satisfy liveness invariant");
    }
}
