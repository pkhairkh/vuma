//! Cleanup invariant checker (Invariant 5).
//!
//! Verifies that every memory region is eventually freed or explicitly leaked,
//! that no region is freed more than once, and that no access targets a freed
//! region (use-after-free). This module implements the formal specification
//! from VUMA-SPEC-INV-001 §7.
//!
//! # Formal Statement
//!
//! **Part A — Every region is freed or explicitly leaked:**
//!
//! > ∀ r ∈ R : r.free_point ≠ null ∨ r.status = Leaked ∨ r.status ∈ {Stack, Mapped, Device}
//!
//! **Part B — No double-free:**
//!
//! > ∀ r ∈ R : count_free(r) ≤ 1
//!
//! **Part C — Freed regions are not accessed (temporal safety):**
//!
//! > ∀ r ∈ R, ∀ a ∈ A :
//! >   region_of(a.target) = r ∧ r.free_point = pp_f
//! >   ⇒ a.program_point <_pp pp_f
//!
//! # Architecture
//!
//! The checker operates in two modes:
//!
//! 1. **MSG-only mode** — inspects the [`MSG`] directly for basic violations
//!    (leaks, use-after-free). Double-free is only partially detectable since
//!    the MSG stores a single `free_point` per region.
//!
//! 2. **Tracked mode** — uses a [`FreeTracker`] to record every free event
//!    observed by the front-end, enabling full double-free detection.

use crate::access::{Access, AccessId};
use crate::derivation::{DerivationId, DerivationSource};
use crate::msg::MSG;
use crate::program_point::ProgramPoint;
use crate::region::{RegionId, RegionStatus};
use hashbrown::HashMap;
use std::fmt;

// ---------------------------------------------------------------------------
// Violation types
// ---------------------------------------------------------------------------

/// A specific violation of the Cleanup invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupViolation {
    /// A region was allocated but never freed and is not marked as [`Leaked`][RegionStatus::Leaked].
    ///
    /// Corresponds to VUMA-SPEC-INV-001 §7 Part A.
    Leak {
        region_id: RegionId,
        alloc_point: ProgramPoint,
    },

    /// A region was freed more than once.
    ///
    /// Corresponds to VUMA-SPEC-INV-001 §7 Part B.
    DoubleFree {
        region_id: RegionId,
        first_free: ProgramPoint,
        second_free: ProgramPoint,
    },

    /// An access targeted a region after it had been freed.
    ///
    /// Corresponds to VUMA-SPEC-INV-001 §7 Part C (temporal safety).
    UseAfterFree {
        access_id: AccessId,
        region_id: RegionId,
        access_point: ProgramPoint,
        free_point: ProgramPoint,
    },

    /// A region is still in [`Allocated`][RegionStatus::Allocated] status at
    /// program end (a more specific form of leak).
    NotFreedAtEnd {
        region_id: RegionId,
        alloc_point: ProgramPoint,
    },

    /// A region that requires cleanup has an invalid lifecycle transition.
    /// For example, a region with `Freed` status but no `free_point`.
    InvalidTransition {
        region_id: RegionId,
        from_status: RegionStatus,
        to_status: RegionStatus,
        point: ProgramPoint,
    },
}

impl fmt::Display for CleanupViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CleanupViolation::Leak {
                region_id,
                alloc_point,
            } => write!(
                f,
                "leak: region {} allocated at {} was never freed and is not marked Leaked",
                region_id, alloc_point
            ),
            CleanupViolation::DoubleFree {
                region_id,
                first_free,
                second_free,
            } => write!(
                f,
                "double-free: region {} freed at {} and again at {}",
                region_id, first_free, second_free
            ),
            CleanupViolation::UseAfterFree {
                access_id,
                region_id,
                access_point,
                free_point,
            } => write!(
                f,
                "use-after-free: access {} at {} targets region {} freed at {}",
                access_id, access_point, region_id, free_point
            ),
            CleanupViolation::NotFreedAtEnd {
                region_id,
                alloc_point,
            } => write!(
                f,
                "not-freed-at-end: region {} allocated at {} is still in Allocated status at program end",
                region_id, alloc_point
            ),
            CleanupViolation::InvalidTransition {
                region_id,
                from_status,
                to_status,
                point,
            } => write!(
                f,
                "invalid-transition: region {} transitioned from {} to {} at {}",
                region_id, from_status, to_status, point
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

/// The result of checking the Cleanup invariant against an MSG.
#[derive(Debug, Clone)]
pub struct InvariantResult {
    /// Whether the Cleanup invariant is satisfied (no violations found).
    pub satisfied: bool,
    /// All violations discovered during the check.
    pub violations: Vec<CleanupViolation>,
}

impl InvariantResult {
    /// Create an empty, satisfied result.
    pub fn ok() -> Self {
        Self {
            satisfied: true,
            violations: Vec::new(),
        }
    }

    /// Create a result from a list of violations.
    pub fn from_violations(violations: Vec<CleanupViolation>) -> Self {
        let satisfied = violations.is_empty();
        Self {
            satisfied,
            violations,
        }
    }

    /// Merge another result into this one.
    pub fn merge(&mut self, other: InvariantResult) {
        self.violations.extend(other.violations);
        self.satisfied = self.satisfied && other.satisfied;
    }

    /// Add a single violation.
    pub fn add(&mut self, v: CleanupViolation) {
        self.satisfied = false;
        self.violations.push(v);
    }
}

impl fmt::Display for InvariantResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.satisfied {
            write!(f, "Cleanup invariant: SATISFIED")
        } else {
            write!(
                f,
                "Cleanup invariant: VIOLATED ({} violation(s))",
                self.violations.len()
            )?;
            for v in &self.violations {
                write!(f, "\n  - {}", v)?;
            }
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Free tracker — records every free event for double-free detection
// ---------------------------------------------------------------------------

/// Tracks free events per region, enabling double-free detection even when
/// the MSG only stores a single `free_point`.
///
/// The front-end should call [`FreeTracker::record_free`] for every free
/// operation it observes. After the full program trace has been fed in,
/// pass the tracker to [`check_cleanup_with_tracker`] for a comprehensive
/// check.
#[derive(Debug, Clone, Default)]
pub struct FreeTracker {
    /// Maps each region to the ordered list of program points where a free
    /// was attempted.
    free_events: HashMap<RegionId, Vec<ProgramPoint>>,
}

impl FreeTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a free operation targeting `region_id` at `point`.
    pub fn record_free(&mut self, region_id: RegionId, point: ProgramPoint) {
        self.free_events.entry(region_id).or_default().push(point);
    }

    /// Returns the number of free events recorded for `region_id`.
    pub fn free_count(&self, region_id: RegionId) -> usize {
        self.free_events.get(&region_id).map_or(0, |v| v.len())
    }

    /// Returns the free events for `region_id`, if any.
    pub fn free_events(&self, region_id: RegionId) -> Option<&[ProgramPoint]> {
        self.free_events.get(&region_id).map(|v| v.as_slice())
    }

    /// Returns all region IDs that have at least one recorded free event.
    pub fn freed_region_ids(&self) -> impl Iterator<Item = RegionId> + '_ {
        self.free_events.keys().copied()
    }

    /// Detect double-free: return pairs of program points where the same
    /// region was freed more than once.
    fn detect_double_frees(&self) -> Vec<CleanupViolation> {
        let mut violations = Vec::new();
        for (&region_id, events) in &self.free_events {
            if events.len() > 1 {
                // Record each pair of consecutive frees as a separate
                // violation so the user can see the exact sequence.
                for window in events.windows(2) {
                    violations.push(CleanupViolation::DoubleFree {
                        region_id,
                        first_free: window[0].clone(),
                        second_free: window[1].clone(),
                    });
                }
            }
        }
        violations
    }
}

// ---------------------------------------------------------------------------
// Resource lifetime tracking
// ---------------------------------------------------------------------------

/// Tracks the lifetime of a region from allocation to deallocation.
///
/// A lifetime is *complete* when the region has been freed. A lifetime is
/// *leaked* when the region is never freed and not marked as Leaked.
#[derive(Debug, Clone)]
pub struct ResourceLifetime {
    /// The region this lifetime describes.
    pub region_id: RegionId,
    /// When the region was allocated.
    pub alloc_point: ProgramPoint,
    /// When the region was freed, if applicable.
    pub free_point: Option<ProgramPoint>,
    /// Current status of the region.
    pub status: RegionStatus,
    /// Number of accesses that occurred while the region was live.
    pub live_access_count: u64,
    /// Number of accesses that occurred after the region was freed.
    pub post_free_access_count: u64,
}

impl ResourceLifetime {
    /// Returns `true` if this lifetime is complete (region was freed).
    pub fn is_complete(&self) -> bool {
        matches!(self.status, RegionStatus::Freed)
    }

    /// Returns `true` if this lifetime represents a leak (never freed, not
    /// marked Leaked, not a stack/mapped/device region).
    pub fn is_leaked(&self) -> bool {
        matches!(self.status, RegionStatus::Allocated)
    }

    /// Returns `true` if there were accesses after the region was freed.
    pub fn has_use_after_free(&self) -> bool {
        self.post_free_access_count > 0
    }

    /// Returns the duration of the lifetime in terms of program-point
    /// ordering, if both allocation and free points are available.
    ///
    /// Returns `None` if the region has not been freed or if the points
    /// are not comparable (different files).
    pub fn span(&self) -> Option<std::cmp::Ordering> {
        self.free_point.as_ref().map(|fp| fp.cmp(&self.alloc_point))
    }
}

impl fmt::Display for ResourceLifetime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Lifetime({} alloc={} status={}",
            self.region_id, self.alloc_point, self.status
        )?;
        if let Some(ref fp) = self.free_point {
            write!(f, " free={}", fp)?;
        }
        write!(
            f,
            " live_accesses={} post_free_accesses={})",
            self.live_access_count, self.post_free_access_count
        )
    }
}

// ---------------------------------------------------------------------------
// Core checker functions
// ---------------------------------------------------------------------------

/// Check the Cleanup invariant against an MSG (basic mode).
///
/// This inspects every region and access in the graph and reports:
/// - **Leaks**: regions in `Allocated` status that were never freed.
/// - **Use-after-free**: accesses targeting a freed region where the
///   access program point is ≥ the region's free point.
/// - **Not-freed-at-end**: regions still in `Allocated` status at program
///   end.
/// - **Invalid transitions**: structural inconsistencies such as a `Freed`
///   region without a `free_point`.
///
/// For full double-free detection, use [`check_cleanup_with_tracker`].
pub fn check_cleanup(msg: &MSG) -> InvariantResult {
    let mut result = InvariantResult::ok();

    // Build a map from RegionId → free_point for quick lookup.
    let mut region_free_points: HashMap<RegionId, ProgramPoint> = HashMap::new();
    for region in msg.regions() {
        if let Some(ref fp) = region.free_point {
            region_free_points.insert(region.id, fp.clone());
        }
    }

    // Check each region for leaks and lifecycle violations.
    for region in msg.regions() {
        match region.status {
            RegionStatus::Allocated => {
                // Region is still allocated — it should have been freed or
                // explicitly marked Leaked.
                result.add(CleanupViolation::Leak {
                    region_id: region.id,
                    alloc_point: region.alloc_point.clone(),
                });
                result.add(CleanupViolation::NotFreedAtEnd {
                    region_id: region.id,
                    alloc_point: region.alloc_point.clone(),
                });
            }
            RegionStatus::Freed => {
                // Good — region was properly freed.
                // Verify that the free_point is set.
                if region.free_point.is_none() {
                    // This is a structural inconsistency: a Freed region
                    // without a free_point. We treat it as an invalid
                    // transition.
                    result.add(CleanupViolation::InvalidTransition {
                        region_id: region.id,
                        from_status: RegionStatus::Allocated,
                        to_status: RegionStatus::Freed,
                        point: region.alloc_point.clone(),
                    });
                }
            }
            RegionStatus::Leaked => {
                // Explicitly leaked — acceptable.
            }
            RegionStatus::Stack | RegionStatus::Mapped | RegionStatus::Device => {
                // These are implicitly managed; no cleanup violation.
            }
        }
    }

    // Check each access for use-after-free.
    for access in msg.accesses() {
        // Resolve the region targeted by this access.
        if let Some(region_id) = resolve_access_region(msg, access) {
            if let Some(free_point) = region_free_points.get(&region_id) {
                // The region was freed. Check if the access occurs after
                // the free point.
                if access.program_point >= *free_point {
                    result.add(CleanupViolation::UseAfterFree {
                        access_id: access.id,
                        region_id,
                        access_point: access.program_point.clone(),
                        free_point: free_point.clone(),
                    });
                }
            }
        }
    }

    result
}

/// Check the Cleanup invariant with full double-free detection.
///
/// In addition to the checks performed by [`check_cleanup`], this function
/// uses the [`FreeTracker`] to detect cases where the same region was freed
/// more than once.
pub fn check_cleanup_with_tracker(msg: &MSG, tracker: &FreeTracker) -> InvariantResult {
    let mut result = check_cleanup(msg);

    // Merge double-free violations from the tracker.
    let double_frees = tracker.detect_double_frees();
    for df in double_frees {
        result.add(df);
    }

    result
}

/// Compute resource lifetimes for all regions in the MSG.
///
/// This produces a [`ResourceLifetime`] for each region, including counts
/// of live and post-free accesses. Useful for generating reports and
/// understanding cleanup behaviour.
pub fn compute_lifetimes(msg: &MSG) -> HashMap<RegionId, ResourceLifetime> {
    let mut lifetimes: HashMap<RegionId, ResourceLifetime> = HashMap::new();

    // Initialize lifetimes from regions.
    for region in msg.regions() {
        lifetimes.insert(
            region.id,
            ResourceLifetime {
                region_id: region.id,
                alloc_point: region.alloc_point.clone(),
                free_point: region.free_point.clone(),
                status: region.status.clone(),
                live_access_count: 0,
                post_free_access_count: 0,
            },
        );
    }

    // Count accesses per region.
    for access in msg.accesses() {
        if let Some(region_id) = resolve_access_region(msg, access) {
            if let Some(lifetime) = lifetimes.get_mut(&region_id) {
                if let Some(ref fp) = lifetime.free_point {
                    if access.program_point >= *fp {
                        lifetime.post_free_access_count += 1;
                    } else {
                        lifetime.live_access_count += 1;
                    }
                } else {
                    // Region was never freed — all accesses are live.
                    lifetime.live_access_count += 1;
                }
            }
        }
    }

    lifetimes
}

// ---------------------------------------------------------------------------
// Input-based checker (for when MSG iteration is not available)
// ---------------------------------------------------------------------------

/// Simplified region info for cleanup checking.
#[derive(Debug, Clone)]
pub struct RegionInfo {
    /// Current status of the region.
    pub status: RegionStatus,
    /// Where the region was allocated.
    pub alloc_point: ProgramPoint,
    /// Where the region was freed, if applicable.
    pub free_point: Option<ProgramPoint>,
}

/// Simplified access info for cleanup checking.
#[derive(Debug, Clone)]
pub struct AccessInfo {
    /// The region this access targets (resolved by the caller).
    pub target_region: RegionId,
    /// Where this access occurs.
    pub program_point: ProgramPoint,
}

/// Input data for the cleanup invariant checker when MSG iteration
/// is not available through the public API.
///
/// The caller is responsible for extracting this data from the MSG.
#[derive(Debug, Clone, Default)]
pub struct CleanupInput {
    /// Regions to check, with their IDs.
    pub regions: Vec<(RegionId, RegionInfo)>,
    /// Accesses to check, with their IDs.
    pub accesses: Vec<(AccessId, AccessInfo)>,
    /// Free events from the tracker.
    pub free_tracker: FreeTracker,
}

/// Check the Cleanup invariant using extracted [`CleanupInput`].
///
/// This is the alternative entry point when MSG iteration is not directly
/// available. The caller extracts the relevant data from the MSG and
/// passes it here.
pub fn check_cleanup_input(input: &CleanupInput) -> InvariantResult {
    let mut result = InvariantResult::ok();

    // Build a map for quick free_point lookup.
    let mut free_points: HashMap<RegionId, ProgramPoint> = HashMap::new();
    for (id, info) in &input.regions {
        if let Some(ref fp) = info.free_point {
            free_points.insert(*id, fp.clone());
        }
    }

    // Part A: Check each region for leaks and lifecycle violations.
    for (id, info) in &input.regions {
        match info.status {
            RegionStatus::Allocated => {
                result.add(CleanupViolation::Leak {
                    region_id: *id,
                    alloc_point: info.alloc_point.clone(),
                });
                result.add(CleanupViolation::NotFreedAtEnd {
                    region_id: *id,
                    alloc_point: info.alloc_point.clone(),
                });
            }
            RegionStatus::Freed => {
                if info.free_point.is_none() {
                    result.add(CleanupViolation::InvalidTransition {
                        region_id: *id,
                        from_status: RegionStatus::Allocated,
                        to_status: RegionStatus::Freed,
                        point: info.alloc_point.clone(),
                    });
                }
            }
            RegionStatus::Leaked
            | RegionStatus::Stack
            | RegionStatus::Mapped
            | RegionStatus::Device => {
                // Acceptable — no cleanup violation.
            }
        }
    }

    // Part B: Double-free detection from tracker.
    let double_frees = input.free_tracker.detect_double_frees();
    for df in double_frees {
        result.add(df);
    }

    // Part C: Use-after-free detection.
    for (access_id, info) in &input.accesses {
        if let Some(free_point) = free_points.get(&info.target_region) {
            if info.program_point >= *free_point {
                result.add(CleanupViolation::UseAfterFree {
                    access_id: *access_id,
                    region_id: info.target_region,
                    access_point: info.program_point.clone(),
                    free_point: free_point.clone(),
                });
            }
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Resolve the [`RegionId`] targeted by an access by tracing its
/// derivation chain back to the root region.
fn resolve_access_region(msg: &MSG, access: &Access) -> Option<RegionId> {
    resolve_derivation_region(msg, access.target)
}

/// Walk the derivation chain from `did` to its root [`RegionId`].
fn resolve_derivation_region(msg: &MSG, did: DerivationId) -> Option<RegionId> {
    let mut current_id = did;
    loop {
        let derivation = msg.derivation(current_id)?;
        match &derivation.source {
            DerivationSource::Region(rid) => return Some(*rid),
            DerivationSource::AnotherDerivation(parent_id) => {
                current_id = *parent_id;
            }
        }
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
    use crate::derivation::{Derivation, DerivationId, DerivationKind, DerivationSource};
    use crate::msg::MSG;
    use crate::region::{Region, RegionId, RegionStatus};

    fn dummy_pp(line: u32) -> ProgramPoint {
        ProgramPoint::new("test.vu", line, 1)
    }

    fn make_region(
        id: u64,
        status: RegionStatus,
        alloc_line: u32,
        free_line: Option<u32>,
    ) -> Region {
        Region {
            id: RegionId(id),
            base: Address::from(0x1000_u64 * id),
            size: 0x100,
            status,
            alloc_point: dummy_pp(alloc_line),
            free_point: free_line.map(dummy_pp),
            owner_context: None,
        }
    }

    fn make_derivation(id: u64, region_id: u64) -> Derivation {
        Derivation {
            id: DerivationId(id),
            source: DerivationSource::Region(RegionId(region_id)),
            kind: DerivationKind::Direct,
            proven_range: (
                Address::from(0x1000_u64 * region_id),
                Address::from(0x1000_u64 * region_id + 0x100),
            ),
        }
    }

    fn make_access(id: u64, derivation_id: u64, kind: AccessKind, line: u32) -> Access {
        Access::new(
            AccessId(id),
            DerivationId(derivation_id),
            kind,
            4,
            dummy_pp(line),
        )
    }

    // -----------------------------------------------------------------------
    // Test 1: Satisfied — all regions properly freed
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_satisfied_all_freed() {
        let mut msg = MSG::new();

        // Region R1: allocated at line 1, freed at line 10.
        let r1 = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Freed,
            alloc_point: dummy_pp(1),
            free_point: Some(dummy_pp(10)),
            owner_context: None,
        };
        msg.add_region(r1);

        // Region R2: allocated at line 2, freed at line 20.
        let r2 = Region {
            id: RegionId(2),
            base: Address::from(0x2000_u64),
            size: 0x100,
            status: RegionStatus::Freed,
            alloc_point: dummy_pp(2),
            free_point: Some(dummy_pp(20)),
            owner_context: None,
        };
        msg.add_region(r2);

        // Access before free.
        msg.add_derivation(make_derivation(1, 1));
        msg.add_access(make_access(1, 1, AccessKind::Read, 5));

        let result = check_cleanup(&msg);
        assert!(
            result.satisfied,
            "Expected no violations, got: {:?}",
            result.violations
        );
        assert!(result.violations.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 2: Leak — region never freed
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_leak_detected() {
        let mut msg = MSG::new();

        // Region still in Allocated status — never freed.
        msg.add_region(make_region(1, RegionStatus::Allocated, 1, None));

        let result = check_cleanup(&msg);
        assert!(!result.satisfied);

        let has_leak = result
            .violations
            .iter()
            .any(|v| matches!(v, CleanupViolation::Leak { .. }));
        let has_not_freed = result
            .violations
            .iter()
            .any(|v| matches!(v, CleanupViolation::NotFreedAtEnd { .. }));
        assert!(has_leak, "Expected a Leak violation");
        assert!(has_not_freed, "Expected a NotFreedAtEnd violation");
    }

    // -----------------------------------------------------------------------
    // Test 3: Use-after-free
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_use_after_free() {
        let mut msg = MSG::new();

        // Region freed at line 10.
        msg.add_region(make_region(1, RegionStatus::Freed, 1, Some(10)));

        // Derivation targeting region 1.
        msg.add_derivation(make_derivation(1, 1));

        // Access at line 15 — after the free at line 10.
        msg.add_access(make_access(1, 1, AccessKind::Read, 15));

        let result = check_cleanup(&msg);
        assert!(!result.satisfied);

        let has_uaf = result
            .violations
            .iter()
            .any(|v| matches!(v, CleanupViolation::UseAfterFree { .. }));
        assert!(has_uaf, "Expected a UseAfterFree violation");

        // Verify the details of the violation.
        if let Some((access_id, region_id, access_point, free_point)) =
            result.violations.iter().find_map(|v| match v {
                CleanupViolation::UseAfterFree {
                    access_id,
                    region_id,
                    access_point,
                    free_point,
                } => Some((
                    *access_id,
                    *region_id,
                    access_point.clone(),
                    free_point.clone(),
                )),
                _ => None,
            })
        {
            assert_eq!(access_id, AccessId(1));
            assert_eq!(region_id, RegionId(1));
            assert_eq!(access_point, dummy_pp(15));
            assert_eq!(free_point, dummy_pp(10));
        }
    }

    // -----------------------------------------------------------------------
    // Test 4: Double-free via FreeTracker
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_double_free() {
        let mut tracker = FreeTracker::new();

        // Region 1 freed twice.
        tracker.record_free(RegionId(1), dummy_pp(10));
        tracker.record_free(RegionId(1), dummy_pp(20));

        let violations = tracker.detect_double_frees();
        assert_eq!(violations.len(), 1);

        assert!(
            matches!(
                &violations[0],
                CleanupViolation::DoubleFree {
                    region_id: RegionId(1),
                    first_free,
                    second_free,
                } if *first_free == dummy_pp(10) && *second_free == dummy_pp(20)
            ),
            "Expected DoubleFree violation for R1 at lines 10 and 20"
        );
    }

    // -----------------------------------------------------------------------
    // Test 5: Explicitly leaked region is acceptable
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_explicitly_leaked_is_ok() {
        let mut msg = MSG::new();

        // Region marked as Leaked — this is intentional.
        msg.add_region(make_region(1, RegionStatus::Leaked, 1, None));

        let result = check_cleanup(&msg);
        assert!(result.satisfied, "Leaked regions should not be violations");
    }

    // -----------------------------------------------------------------------
    // Test 6: Stack/mapped/device regions are acceptable without free
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_stack_mapped_device_ok() {
        let mut msg = MSG::new();

        msg.add_region(make_region(1, RegionStatus::Stack, 1, None));
        msg.add_region(make_region(2, RegionStatus::Mapped, 2, None));
        msg.add_region(make_region(3, RegionStatus::Device, 3, None));

        let result = check_cleanup(&msg);
        assert!(
            result.satisfied,
            "Stack/Mapped/Device regions should not be violations"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Access before free is fine (no use-after-free)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_access_before_free_ok() {
        let mut msg = MSG::new();

        msg.add_region(make_region(1, RegionStatus::Freed, 1, Some(20)));
        msg.add_derivation(make_derivation(1, 1));
        msg.add_access(make_access(1, 1, AccessKind::Write, 10));

        let result = check_cleanup(&msg);
        assert!(
            result.satisfied,
            "Access before free should not be a violation"
        );
    }

    // -----------------------------------------------------------------------
    // Test 8: FreeTracker — no double-free for single free
    // -----------------------------------------------------------------------

    #[test]
    fn test_tracker_no_double_free() {
        let mut tracker = FreeTracker::new();
        tracker.record_free(RegionId(1), dummy_pp(10));

        assert_eq!(tracker.free_count(RegionId(1)), 1);
        assert!(tracker.detect_double_frees().is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 9: FreeTracker — triple free produces two violations
    // -----------------------------------------------------------------------

    #[test]
    fn test_tracker_triple_free() {
        let mut tracker = FreeTracker::new();
        tracker.record_free(RegionId(1), dummy_pp(10));
        tracker.record_free(RegionId(1), dummy_pp(20));
        tracker.record_free(RegionId(1), dummy_pp(30));

        let violations = tracker.detect_double_frees();
        // Three frees → two consecutive pairs: (10,20) and (20,30).
        assert_eq!(violations.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Test 10: InvariantResult merge
    // -----------------------------------------------------------------------

    #[test]
    fn test_invariant_result_merge() {
        let mut r1 = InvariantResult::ok();
        let r2 = InvariantResult::from_violations(vec![CleanupViolation::Leak {
            region_id: RegionId(1),
            alloc_point: dummy_pp(1),
        }]);

        assert!(r1.satisfied);
        assert!(!r2.satisfied);

        r1.merge(r2);
        assert!(!r1.satisfied);
        assert_eq!(r1.violations.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Test 11: ResourceLifetime
    // -----------------------------------------------------------------------

    #[test]
    fn test_resource_lifetime() {
        let lt = ResourceLifetime {
            region_id: RegionId(1),
            alloc_point: dummy_pp(1),
            free_point: Some(dummy_pp(10)),
            status: RegionStatus::Freed,
            live_access_count: 5,
            post_free_access_count: 0,
        };

        assert!(lt.is_complete());
        assert!(!lt.is_leaked());
        assert!(!lt.has_use_after_free());
        assert_eq!(lt.span(), Some(std::cmp::Ordering::Greater));
    }

    // -----------------------------------------------------------------------
    // Test 12: check_cleanup_input — leak and use-after-free
    // -----------------------------------------------------------------------

    #[test]
    fn test_check_cleanup_input_leak_and_uaf() {
        let mut input = CleanupInput::default();

        // Region 1: leaked (never freed, still Allocated).
        input.regions.push((
            RegionId(1),
            RegionInfo {
                status: RegionStatus::Allocated,
                alloc_point: dummy_pp(1),
                free_point: None,
            },
        ));

        // Region 2: properly freed at line 20.
        input.regions.push((
            RegionId(2),
            RegionInfo {
                status: RegionStatus::Freed,
                alloc_point: dummy_pp(2),
                free_point: Some(dummy_pp(20)),
            },
        ));

        // Access to region 2 after it was freed.
        input.accesses.push((
            AccessId(1),
            AccessInfo {
                target_region: RegionId(2),
                program_point: dummy_pp(30),
            },
        ));

        let result = check_cleanup_input(&input);
        assert!(!result.satisfied);

        let leak_count = result
            .violations
            .iter()
            .filter(|v| matches!(v, CleanupViolation::Leak { .. }))
            .count();
        let uaf_count = result
            .violations
            .iter()
            .filter(|v| matches!(v, CleanupViolation::UseAfterFree { .. }))
            .count();
        assert_eq!(leak_count, 1, "Expected one Leak violation for R1");
        assert_eq!(uaf_count, 1, "Expected one UseAfterFree violation for R2");
    }

    // -----------------------------------------------------------------------
    // Test 13: Freed region without free_point is invalid transition
    // -----------------------------------------------------------------------

    #[test]
    fn test_freed_without_free_point_is_invalid() {
        let mut msg = MSG::new();

        // Region in Freed status but with no free_point — structural error.
        let r = Region {
            id: RegionId(1),
            base: Address::from(0x1000_u64),
            size: 0x100,
            status: RegionStatus::Freed,
            alloc_point: dummy_pp(1),
            free_point: None, // Inconsistent!
            owner_context: None,
        };
        msg.add_region(r);

        let result = check_cleanup(&msg);
        assert!(!result.satisfied);

        let has_invalid = result
            .violations
            .iter()
            .any(|v| matches!(v, CleanupViolation::InvalidTransition { .. }));
        assert!(
            has_invalid,
            "Expected InvalidTransition for Freed region without free_point"
        );
    }

    // -----------------------------------------------------------------------
    // Test 14: Compute lifetimes
    // -----------------------------------------------------------------------

    #[test]
    fn test_compute_lifetimes() {
        let mut msg = MSG::new();

        msg.add_region(make_region(1, RegionStatus::Freed, 1, Some(20)));
        msg.add_derivation(make_derivation(1, 1));

        // Access before free (line 10 < line 20).
        msg.add_access(make_access(1, 1, AccessKind::Read, 10));
        // Access after free (line 30 >= line 20).
        msg.add_access(make_access(2, 1, AccessKind::Read, 30));

        let lifetimes = compute_lifetimes(&msg);
        let lt = &lifetimes[&RegionId(1)];
        assert!(lt.is_complete());
        assert!(lt.has_use_after_free());
        assert_eq!(lt.live_access_count, 1);
        assert_eq!(lt.post_free_access_count, 1);
    }

    // -----------------------------------------------------------------------
    // Test 15: Display formatting
    // -----------------------------------------------------------------------

    #[test]
    fn test_violation_display() {
        let v = CleanupViolation::Leak {
            region_id: RegionId(42),
            alloc_point: dummy_pp(10),
        };
        assert_eq!(
            format!("{}", v),
            "leak: region R42 allocated at test.vu:10:1 was never freed and is not marked Leaked"
        );

        let v = CleanupViolation::DoubleFree {
            region_id: RegionId(5),
            first_free: dummy_pp(20),
            second_free: dummy_pp(30),
        };
        assert_eq!(
            format!("{}", v),
            "double-free: region R5 freed at test.vu:20:1 and again at test.vu:30:1"
        );

        let v = CleanupViolation::UseAfterFree {
            access_id: AccessId(3),
            region_id: RegionId(1),
            access_point: dummy_pp(40),
            free_point: dummy_pp(20),
        };
        assert_eq!(
            format!("{}", v),
            "use-after-free: access A3 at test.vu:40:1 targets region R1 freed at test.vu:20:1"
        );
    }

    // -----------------------------------------------------------------------
    // Test 16: Empty MSG satisfies cleanup
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_msg_satisfies() {
        let msg = MSG::new();
        let result = check_cleanup(&msg);
        assert!(result.satisfied);
        assert!(result.violations.is_empty());
    }

    // -----------------------------------------------------------------------
    // Test 17: check_cleanup_with_tracker combines checks
    // -----------------------------------------------------------------------

    #[test]
    fn test_cleanup_with_tracker_combined() {
        let mut msg = MSG::new();
        let mut tracker = FreeTracker::new();

        // Region 1: properly freed in MSG.
        msg.add_region(make_region(1, RegionStatus::Freed, 1, Some(20)));

        // But the tracker shows it was freed twice!
        tracker.record_free(RegionId(1), dummy_pp(20));
        tracker.record_free(RegionId(1), dummy_pp(30));

        let result = check_cleanup_with_tracker(&msg, &tracker);
        assert!(!result.satisfied);

        let has_double_free = result
            .violations
            .iter()
            .any(|v| matches!(v, CleanupViolation::DoubleFree { .. }));
        assert!(has_double_free, "Expected DoubleFree from tracker");
    }
}
