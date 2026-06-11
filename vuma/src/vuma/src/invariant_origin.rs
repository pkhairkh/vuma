//! Origin invariant checker for the VUMA Memory State Graph.
//!
//! This module implements **Invariant 4: Origin** from the VUMA specification
//! (VUMA-SPEC-INV-001, Section 6):
//!
//! > Every address traces to a valid allocation; arithmetic derivations stay
//! > within bounds.
//!
//! The invariant has three parts:
//!
//! - **Part A — Trace terminates at allocation**: Every derivation chain must
//!   terminate at a [`Region`](crate::region::Region), not diverge or cycle.
//! - **Part B — Arithmetic derivations stay in bounds**: The provenance range
//!   of every derivation must be contained within its root region.
//! - **Part C — No fabrication**: Every address must be traceable to a valid
//!   allocation; integer literals or untracked external values used as
//!   addresses are forbidden.
//!
//! Additionally, this module detects:
//!
//! - **Orphan derivations**: derivations whose chain is broken (a parent
//!   derivation is missing from the MSG).
//! - **Dangling derivations**: derivations whose root region has been freed
//!   (status = `Freed` or `Leaked`).
//!
//! # Provenance tracking and taint analysis
//!
//! Every derivation is assigned a [`ProvenanceInfo`] that records its root
//! region, the full chain of derivation IDs, and whether the root region is
//! live. Derivations that violate the origin invariant are marked as *tainted*,
//! and any child derivation that depends on a tainted parent is also tainted,
//! propagating the taint through the derivation graph.

use crate::access::{Access, AccessId};
use crate::address::Address;
use crate::derivation::{DerivationExpr, DerivationId, DerivationKind, DerivationSource};
use crate::msg::MSG;
use crate::region::{RegionId, RegionStatus};
use hashbrown::{HashMap, HashSet};
use std::fmt;

// ---------------------------------------------------------------------------
// Violation types
// ---------------------------------------------------------------------------

/// A specific violation of the Origin invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OriginViolation {
    /// A derivation chain does not terminate at a Region because a parent
    /// derivation is missing from the MSG (broken chain / orphan).
    OrphanDerivation {
        /// The derivation whose chain is broken.
        derivation_id: DerivationId,
        /// The parent derivation (or region reference) that could not be
        /// resolved.
        missing_parent: DerivationId,
    },

    /// A derivation traces to a root Region that has been freed (status is
    /// `Freed` or `Leaked`), making the derivation *dangling*.
    DanglingDerivation {
        /// The derivation whose root region is no longer live.
        derivation_id: DerivationId,
        /// The root region that has been freed.
        root_region: RegionId,
        /// The status of the root region at check time.
        region_status: RegionStatus,
    },

    /// The provenance range of a derivation extends beyond the bounds of
    /// its root region (Part B violation).
    OutOfBounds {
        /// The derivation that goes out of bounds.
        derivation_id: DerivationId,
        /// The root region that the derivation escapes.
        root_region: RegionId,
        /// The lower bound of the provenance range.
        proven_lo: Address,
        /// The upper bound of the provenance range.
        proven_hi: Address,
        /// The lower bound of the root region.
        region_base: Address,
        /// The upper bound of the root region (exclusive).
        region_end: Address,
    },

    /// A cycle was detected in the derivation chain (Part A violation).
    CycleInChain {
        /// The derivation where the cycle was detected.
        derivation_id: DerivationId,
        /// The derivation that closes the cycle.
        cycle_entry: DerivationId,
    },

    /// An access targets a derivation that violates the origin invariant
    /// (i.e. the derivation is orphan, dangling, out-of-bounds, or cyclic).
    AccessToInvalidDerivation {
        /// The access that targets a bad derivation.
        access_id: AccessId,
        /// The derivation that is invalid.
        derivation_id: DerivationId,
        /// A human-readable description of why the derivation is invalid.
        reason: String,
    },

    /// A derivation has an inverted (empty) provenance range (Part B: the
    /// range `[lo, hi)` must satisfy `lo < hi`).
    InvertedProvenanceRange {
        /// The derivation with the inverted range.
        derivation_id: DerivationId,
        /// The provenance range lower bound (which is >= hi).
        proven_lo: Address,
        /// The provenance range upper bound (which is <= lo).
        proven_hi: Address,
    },
}

impl fmt::Display for OriginViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OrphanDerivation {
                derivation_id,
                missing_parent,
            } => write!(
                f,
                "Origin violation: orphan derivation {} — parent {} not found in MSG",
                derivation_id, missing_parent
            ),
            Self::DanglingDerivation {
                derivation_id,
                root_region,
                region_status,
            } => write!(
                f,
                "Origin violation: dangling derivation {} — root region {} has status {:?}",
                derivation_id, root_region, region_status
            ),
            Self::OutOfBounds {
                derivation_id,
                root_region,
                proven_lo,
                proven_hi,
                region_base,
                region_end,
            } => write!(
                f,
                "Origin violation: derivation {} out of bounds — proven [{}, {}) vs region {} [{}, {})",
                derivation_id, proven_lo, proven_hi, root_region, region_base, region_end
            ),
            Self::CycleInChain {
                derivation_id,
                cycle_entry,
            } => write!(
                f,
                "Origin violation: cycle in derivation chain starting at {} — re-enters at {}",
                derivation_id, cycle_entry
            ),
            Self::AccessToInvalidDerivation {
                access_id,
                derivation_id,
                reason,
            } => write!(
                f,
                "Origin violation: access {} targets invalid derivation {} — {}",
                access_id, derivation_id, reason
            ),
            Self::InvertedProvenanceRange {
                derivation_id,
                proven_lo,
                proven_hi,
            } => write!(
                f,
                "Origin violation: derivation {} has inverted provenance range [{}, {})",
                derivation_id, proven_lo, proven_hi
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Provenance information
// ---------------------------------------------------------------------------

/// Provenance metadata computed for a single derivation during the origin
/// invariant check.
#[derive(Debug, Clone)]
pub struct ProvenanceInfo {
    /// The root [`RegionId`] that this derivation traces to, or `None` if the
    /// chain is broken (orphan).
    pub root_region: Option<RegionId>,
    /// The full derivation chain from root to this derivation
    /// `[root_derivation, ..., parent, self]`.
    pub chain: Vec<DerivationId>,
    /// Whether the root region is currently live (not Freed or Leaked).
    pub is_live: bool,
    /// Whether this derivation (or an ancestor) violates the origin invariant.
    pub is_tainted: bool,
    /// The cumulative offset from the root region's base address to the start
    /// of this derivation's provenance range, in bytes.
    pub cumulative_offset: i64,
}

impl fmt::Display for ProvenanceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let chain_str = self
            .chain
            .iter()
            .map(|id| format!("{}", id))
            .collect::<Vec<_>>()
            .join(" \u{2192} "); // →
        write!(
            f,
            "Provenance {{ root={:?}, chain=[{}], live={}, tainted={}, offset={} }}",
            self.root_region, chain_str, self.is_live, self.is_tainted, self.cumulative_offset
        )
    }
}

// ---------------------------------------------------------------------------
// Invariant result
// ---------------------------------------------------------------------------

/// The result of checking the Origin invariant on an MSG.
#[derive(Debug, Clone)]
pub struct InvariantResult {
    /// Whether the Origin invariant is satisfied (no violations).
    pub satisfied: bool,
    /// The list of violations found.
    pub violations: Vec<OriginViolation>,
    /// Provenance information for every derivation in the MSG.
    pub provenance_map: HashMap<DerivationId, ProvenanceInfo>,
    /// The set of derivation IDs that are tainted (they or their ancestors
    /// violate the origin invariant).
    pub taint_set: HashSet<DerivationId>,
}

impl fmt::Display for InvariantResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.satisfied {
            write!(
                f,
                "Origin invariant: SATISFIED ({} derivations checked, 0 violations)",
                self.provenance_map.len()
            )
        } else {
            write!(
                f,
                "Origin invariant: VIOLATED ({} derivations, {} violations, {} tainted)",
                self.provenance_map.len(),
                self.violations.len(),
                self.taint_set.len()
            )
        }
    }
}

// ---------------------------------------------------------------------------
// MSG enumeration helpers
// ---------------------------------------------------------------------------

/// Collect all derivation IDs present in the MSG by probing sequential IDs.
///
/// Since MSG does not currently expose a key iterator, we probe IDs from
/// 0 upward until we find a gap. This works for the auto-incrementing IDs
/// used by `MSG::add_derivation`.
fn collect_derivation_ids(msg: &MSG) -> Vec<DerivationId> {
    let mut ids = Vec::new();
    for raw in 0u64.. {
        let did = DerivationId(raw);
        if msg.derivation(did).is_some() {
            ids.push(did);
        } else if raw > 0 && ids.is_empty() {
            break;
        } else if raw > ids.last().map_or(0, |last| last.0) + 100 {
            // Stop after a gap of 100 — no more IDs expected.
            break;
        }
    }
    ids
}

/// Collect all access IDs present in the MSG by probing sequential IDs.
fn collect_access_ids(msg: &MSG) -> Vec<AccessId> {
    let mut ids = Vec::new();
    for raw in 0u64.. {
        let aid = AccessId(raw);
        if msg.access(aid).is_some() {
            ids.push(aid);
        } else if (raw > 0 && ids.is_empty()) || raw > ids.last().map_or(0, |last| last.0) + 100 {
            break;
        }
    }
    ids
}

// ---------------------------------------------------------------------------
// Origin checker
// ---------------------------------------------------------------------------

/// Check the Origin invariant (Invariant 4) on the given MSG.
///
/// This function examines every derivation in the MSG and verifies:
///
/// 1. Every derivation chain terminates at a live Region (Part A).
/// 2. Every derivation's provenance range is within its root region (Part B).
/// 3. No derivation has an inverted provenance range.
/// 4. Every access targets a derivation that satisfies the origin invariant.
///
/// It also performs taint analysis: derivations that violate the invariant
/// (or depend on a violating ancestor) are marked as tainted.
pub fn check_origin(msg: &MSG) -> InvariantResult {
    let mut violations: Vec<OriginViolation> = Vec::new();
    let mut provenance_map: HashMap<DerivationId, ProvenanceInfo> = HashMap::new();
    let mut taint_set: HashSet<DerivationId> = HashSet::new();

    // Phase 1: Compute provenance for every derivation.
    let all_deriv_ids = collect_derivation_ids(msg);
    for deriv_id in all_deriv_ids {
        let (info, deriv_violations) = compute_provenance(msg, deriv_id);
        if info.is_tainted {
            taint_set.insert(deriv_id);
        }
        for v in deriv_violations {
            violations.push(v);
        }
        provenance_map.insert(deriv_id, info);
    }

    // Phase 2: Propagate taint — any derivation whose chain includes a
    // tainted ancestor is also tainted.
    propagate_taint(&provenance_map, &mut taint_set);

    // Update is_tainted flags in provenance_map based on the final taint_set.
    for info in provenance_map.values_mut() {
        if let Some(&self_id) = info.chain.last() {
            info.is_tainted = taint_set.contains(&self_id);
        }
    }

    // Phase 3: Check that every access targets a derivation that satisfies
    // the origin invariant.
    let all_access_ids = collect_access_ids(msg);
    for access_id in all_access_ids {
        let access = match msg.access(access_id) {
            Some(a) => a,
            None => continue,
        };
        if let Some(v) = check_access_origin(access, &provenance_map, &taint_set) {
            violations.push(v);
        }
    }

    let satisfied = violations.is_empty();
    InvariantResult {
        satisfied,
        violations,
        provenance_map,
        taint_set,
    }
}

/// Compute provenance information for a single derivation, detecting
/// cycles, orphans, dangling references, out-of-bounds, and inverted ranges.
fn compute_provenance(msg: &MSG, deriv_id: DerivationId) -> (ProvenanceInfo, Vec<OriginViolation>) {
    let mut violations = Vec::new();
    let mut chain: Vec<DerivationId> = Vec::new();
    let mut visited: HashSet<DerivationId> = HashSet::new();

    // Walk the derivation chain backwards to find the root Region.
    let mut current_id = deriv_id;
    let mut root_region: Option<RegionId> = None;
    let mut is_tainted = false;
    let mut cumulative_offset: i64 = 0;

    loop {
        // Cycle detection.
        if visited.contains(&current_id) {
            violations.push(OriginViolation::CycleInChain {
                derivation_id: deriv_id,
                cycle_entry: current_id,
            });
            is_tainted = true;
            break;
        }
        visited.insert(current_id);
        chain.push(current_id);

        let deriv = match msg.derivation(current_id) {
            Some(d) => d,
            None => {
                // Should not happen since we iterate over existing IDs, but
                // handle defensively.
                break;
            }
        };

        // Accumulate offset from this derivation step.
        cumulative_offset += match deriv.kind {
            DerivationKind::Direct => 0,
            DerivationKind::Offset { by } => by,
            DerivationKind::Cast { .. } => 0,
            DerivationKind::Arithmetic { ref expr } => eval_expr_const(expr),
        };

        match deriv.source {
            DerivationSource::Region(rid) => {
                root_region = Some(rid);
                break;
            }
            DerivationSource::AnotherDerivation(parent_id) => {
                if msg.derivation(parent_id).is_some() {
                    current_id = parent_id;
                } else {
                    // Orphan: parent not found.
                    violations.push(OriginViolation::OrphanDerivation {
                        derivation_id: deriv_id,
                        missing_parent: parent_id,
                    });
                    is_tainted = true;
                    break;
                }
            }
        }
    }

    // Reverse the chain so it goes [root, ..., self].
    chain.reverse();

    // Check for inverted provenance range.
    let deriv = msg.derivation(deriv_id);
    if let Some(d) = deriv {
        if d.proven_range.0 >= d.proven_range.1 {
            violations.push(OriginViolation::InvertedProvenanceRange {
                derivation_id: deriv_id,
                proven_lo: d.proven_range.0,
                proven_hi: d.proven_range.1,
            });
            is_tainted = true;
        }
    }

    // Check root region liveness and bounds.
    let is_live = if let Some(rid) = root_region {
        match msg.region(rid) {
            Some(region) => {
                // Check dangling: is the root region live?
                if !region.is_live() {
                    violations.push(OriginViolation::DanglingDerivation {
                        derivation_id: deriv_id,
                        root_region: rid,
                        region_status: region.status.clone(),
                    });
                    is_tainted = true;
                }

                // Check bounds: provenance range must be within region range.
                if let Some(d) = deriv {
                    let region_base = region.base;
                    let region_end = region.end();
                    let (proven_lo, proven_hi) = d.proven_range;
                    if proven_lo < region_base || proven_hi > region_end {
                        violations.push(OriginViolation::OutOfBounds {
                            derivation_id: deriv_id,
                            root_region: rid,
                            proven_lo,
                            proven_hi,
                            region_base,
                            region_end,
                        });
                        is_tainted = true;
                    }
                }

                region.is_live()
            }
            None => {
                // The root Region is referenced but not present in the MSG.
                // This is an orphan situation — the region was never added.
                violations.push(OriginViolation::OrphanDerivation {
                    derivation_id: deriv_id,
                    missing_parent: DerivationId(rid.0),
                });
                is_tainted = true;
                false
            }
        }
    } else {
        // No root region found — orphan.
        if !is_tainted {
            violations.push(OriginViolation::OrphanDerivation {
                derivation_id: deriv_id,
                missing_parent: DerivationId(0),
            });
            is_tainted = true;
        }
        false
    };

    let info = ProvenanceInfo {
        root_region,
        chain,
        is_live,
        is_tainted,
        cumulative_offset,
    };

    (info, violations)
}

/// Propagate taint through the derivation graph: if a derivation is tainted,
/// all its children (derivations that source from it) are also tainted.
fn propagate_taint(
    provenance_map: &HashMap<DerivationId, ProvenanceInfo>,
    taint_set: &mut HashSet<DerivationId>,
) {
    // Build a reverse adjacency list: parent → children.
    let mut children: HashMap<DerivationId, Vec<DerivationId>> = HashMap::new();
    for (&deriv_id, info) in provenance_map.iter() {
        if info.chain.len() >= 2 {
            // The parent is the second-to-last element in the chain
            // (chain is [root, ..., parent, self]).
            let parent_idx = info.chain.len() - 2;
            let parent_id = info.chain[parent_idx];
            children.entry(parent_id).or_default().push(deriv_id);
        }
    }

    // BFS from all currently tainted derivations.
    let mut queue: Vec<DerivationId> = taint_set.iter().copied().collect();
    while let Some(current) = queue.pop() {
        if let Some(kids) = children.get(&current) {
            for &kid in kids {
                if !taint_set.contains(&kid) {
                    taint_set.insert(kid);
                    queue.push(kid);
                }
            }
        }
    }
}

/// Check that an access targets a derivation that satisfies the origin
/// invariant. Returns a violation if the target derivation is tainted.
fn check_access_origin(
    access: &Access,
    provenance_map: &HashMap<DerivationId, ProvenanceInfo>,
    taint_set: &HashSet<DerivationId>,
) -> Option<OriginViolation> {
    let target_id = access.target;

    if taint_set.contains(&target_id) {
        let info = provenance_map.get(&target_id);
        let reason = match info {
            Some(pi) if !pi.is_live && pi.root_region.is_some() => {
                format!(
                    "derivation is dangling (root region {:?} is not live)",
                    pi.root_region
                )
            }
            Some(pi) if pi.root_region.is_none() => {
                "derivation is orphan (no root region)".to_string()
            }
            _ => "derivation is tainted".to_string(),
        };
        Some(OriginViolation::AccessToInvalidDerivation {
            access_id: access.id,
            derivation_id: target_id,
            reason,
        })
    } else {
        None
    }
}

/// Evaluate a [`DerivationExpr`] to a constant offset, if possible.
///
/// For variable offsets ([`DerivationExpr::Scaled`]), we return 0 since the
/// actual value is unknown at static analysis time. This is conservative —
/// the out-of-bounds check on the provenance range will still catch escapes.
fn eval_expr_const(expr: &DerivationExpr) -> i64 {
    match expr {
        DerivationExpr::Constant(c) => *c,
        DerivationExpr::Scaled { .. } => 0,
        DerivationExpr::Add(a, b) => eval_expr_const(a) + eval_expr_const(b),
        DerivationExpr::Sub(a, b) => eval_expr_const(a) - eval_expr_const(b),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{Access, AccessId, AccessKind};
    use crate::address::Address;
    use crate::derivation::{Derivation, DerivationKind, DerivationSource};
    use crate::program_point::ProgramPoint;
    use crate::region::{Region, RegionStatus};

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    fn make_region(id: u64, base: u64, size: u64, status: RegionStatus) -> Region {
        Region {
            id: RegionId(id),
            base: Address::from(base),
            size,
            status,
            alloc_point: dummy_pp(1),
            free_point: None,
            owner_context: None,
        }
    }

    fn make_direct_derivation(id: u64, region_id: u64, lo: u64, hi: u64) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(region_id)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(lo), Address::from(hi)),
        }
    }

    fn make_offset_derivation(
        id: u64,
        parent_id: u64,
        offset: i64,
        lo: u64,
        hi: u64,
    ) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::AnotherDerivation(DerivationId(parent_id)),
            kind: DerivationKind::Offset { by: offset },
            proven_range: (Address::from(lo), Address::from(hi)),
        }
    }

    fn make_access(id: u64, target: u64, kind: AccessKind, size: u64) -> Access {
        Access::new(
            AccessId(id),
            DerivationId(target),
            kind,
            size,
            dummy_pp(id as u32),
        )
    }

    // ----- Test 1: Satisfied origin invariant -----
    #[test]
    fn origin_satisfied_simple() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100, RegionStatus::Allocated));
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));
        msg.add_access(make_access(1, 1, AccessKind::Read, 4));

        let result = check_origin(&msg);
        assert!(
            result.satisfied,
            "Expected satisfied, got violations: {:?}",
            result.violations
        );
        assert!(result.violations.is_empty());
        assert!(result.taint_set.is_empty());
    }

    // ----- Test 2: Orphan derivation (missing parent) -----
    #[test]
    fn origin_orphan_derivation() {
        let mut msg = MSG::new();
        // D1 sources from a non-existent derivation D99.
        msg.add_derivation(make_offset_derivation(1, 99, 0x10, 0x1010, 0x1100));

        let result = check_origin(&msg);
        assert!(
            !result.satisfied,
            "Expected violation for orphan derivation"
        );
        assert!(
            result
                .violations
                .iter()
                .any(|v| matches!(v, OriginViolation::OrphanDerivation { .. })),
            "Expected OrphanDerivation violation, got: {:?}",
            result.violations
        );
        assert!(result.taint_set.contains(&DerivationId(1)));
    }

    // ----- Test 3: Dangling derivation (freed root region) -----
    #[test]
    fn origin_dangling_derivation() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100, RegionStatus::Freed));
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));
        msg.add_access(make_access(1, 1, AccessKind::Read, 4));

        let result = check_origin(&msg);
        assert!(!result.satisfied);
        assert!(
            result
                .violations
                .iter()
                .any(|v| matches!(v, OriginViolation::DanglingDerivation { .. })),
            "Expected DanglingDerivation violation, got: {:?}",
            result.violations
        );
        // Also check that the access to the dangling derivation is flagged.
        assert!(
            result
                .violations
                .iter()
                .any(|v| matches!(v, OriginViolation::AccessToInvalidDerivation { .. })),
            "Expected AccessToInvalidDerivation violation"
        );
    }

    // ----- Test 4: Out-of-bounds derivation -----
    #[test]
    fn origin_out_of_bounds() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100, RegionStatus::Allocated));
        // Provenance range extends beyond region end (0x1100).
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1200));

        let result = check_origin(&msg);
        assert!(!result.satisfied);
        assert!(
            result
                .violations
                .iter()
                .any(|v| matches!(v, OriginViolation::OutOfBounds { .. })),
            "Expected OutOfBounds violation, got: {:?}",
            result.violations
        );
    }

    // ----- Test 5: Chained derivation with valid origin -----
    #[test]
    fn origin_chained_valid() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x400, RegionStatus::Allocated));
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1400));
        msg.add_derivation(make_offset_derivation(2, 1, 0x40, 0x1040, 0x1400));
        msg.add_derivation(make_offset_derivation(3, 2, 0x40, 0x1080, 0x1400));
        msg.add_access(make_access(1, 3, AccessKind::Write, 8));

        let result = check_origin(&msg);
        assert!(
            result.satisfied,
            "Expected satisfied, got: {:?}",
            result.violations
        );

        // Verify provenance chain for derivation 3.
        let info = result.provenance_map.get(&DerivationId(3)).unwrap();
        assert_eq!(info.root_region, Some(RegionId(1)));
        assert!(info.is_live);
        assert!(!info.is_tainted);
        assert_eq!(info.chain.len(), 3); // [D1, D2, D3]
        assert_eq!(info.chain[0], DerivationId(1));
        assert_eq!(info.chain[1], DerivationId(2));
        assert_eq!(info.chain[2], DerivationId(3));
    }

    // ----- Test 6: Taint propagation -----
    #[test]
    fn origin_taint_propagation() {
        let mut msg = MSG::new();
        // Region is freed -> D1 is dangling.
        msg.add_region(make_region(1, 0x1000, 0x100, RegionStatus::Freed));
        msg.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));
        // D2 sources from D1, so it should be tainted too.
        msg.add_derivation(make_offset_derivation(2, 1, 0x10, 0x1010, 0x1100));

        let result = check_origin(&msg);
        assert!(!result.satisfied);

        // D1 should be tainted (dangling).
        assert!(result.taint_set.contains(&DerivationId(1)));
        // D2 should be tainted via propagation.
        assert!(result.taint_set.contains(&DerivationId(2)));
    }

    // ----- Test 7: Inverted provenance range -----
    #[test]
    fn origin_inverted_provenance() {
        let mut msg = MSG::new();
        msg.add_region(make_region(1, 0x1000, 0x100, RegionStatus::Allocated));
        let bad_deriv = Derivation {
            id: DerivationId(1),
            source: DerivationSource::Region(RegionId(1)),
            kind: DerivationKind::Direct,
            proven_range: (Address::from(0x2000u64), Address::from(0x1000u64)),
        };
        msg.add_derivation(bad_deriv);

        let result = check_origin(&msg);
        assert!(!result.satisfied);
        assert!(
            result
                .violations
                .iter()
                .any(|v| matches!(v, OriginViolation::InvertedProvenanceRange { .. })),
            "Expected InvertedProvenanceRange violation, got: {:?}",
            result.violations
        );
    }

    // ----- Test 8: Empty MSG satisfies origin -----
    #[test]
    fn origin_empty_msg() {
        let msg = MSG::new();
        let result = check_origin(&msg);
        assert!(
            result.satisfied,
            "Empty MSG should satisfy origin invariant"
        );
        assert!(result.violations.is_empty());
        assert!(result.taint_set.is_empty());
    }

    // ----- Test 9: InvariantResult display -----
    #[test]
    fn invariant_result_display() {
        let msg = MSG::new();
        let result = check_origin(&msg);
        let display = format!("{}", result);
        assert!(display.contains("SATISFIED"));

        let mut msg2 = MSG::new();
        msg2.add_region(make_region(1, 0x1000, 0x100, RegionStatus::Freed));
        msg2.add_derivation(make_direct_derivation(1, 1, 0x1000, 0x1100));
        let result2 = check_origin(&msg2);
        let display2 = format!("{}", result2);
        assert!(display2.contains("VIOLATED"));
    }

    // ----- Test 10: OriginViolation display formatting -----
    #[test]
    fn violation_display() {
        let v = OriginViolation::OrphanDerivation {
            derivation_id: DerivationId(5),
            missing_parent: DerivationId(10),
        };
        let display = format!("{}", v);
        assert!(display.contains("D5"));
        assert!(display.contains("D10"));
        assert!(display.contains("orphan"));

        let v2 = OriginViolation::DanglingDerivation {
            derivation_id: DerivationId(3),
            root_region: RegionId(7),
            region_status: RegionStatus::Freed,
        };
        let display2 = format!("{}", v2);
        assert!(display2.contains("dangling"));
        assert!(display2.contains("R7"));
    }

    // ----- Test 11: ProvenanceInfo display -----
    #[test]
    fn provenance_info_display() {
        let info = ProvenanceInfo {
            root_region: Some(RegionId(42)),
            chain: vec![DerivationId(1), DerivationId(2), DerivationId(3)],
            is_live: true,
            is_tainted: false,
            cumulative_offset: 64,
        };
        let display = format!("{}", info);
        assert!(
            display.contains("42"),
            "Expected region 42 in display: {}",
            display
        );
        assert!(display.contains("D1"));
        assert!(display.contains("D3"));
        assert!(display.contains("live=true"));
        assert!(display.contains("tainted=false"));
    }

    // ----- Test 12: Orphan derivation via missing region -----
    #[test]
    fn origin_orphan_missing_region() {
        let mut msg = MSG::new();
        // D1 sources from R99 which was never added to the MSG.
        msg.add_derivation(make_direct_derivation(1, 99, 0x1000, 0x1100));

        let result = check_origin(&msg);
        assert!(!result.satisfied);
        assert!(
            result
                .violations
                .iter()
                .any(|v| matches!(v, OriginViolation::OrphanDerivation { .. })),
            "Expected OrphanDerivation for missing region, got: {:?}",
            result.violations
        );
        assert!(result.taint_set.contains(&DerivationId(1)));
    }
}
