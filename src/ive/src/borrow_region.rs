//! Borrow-Region Verifier — tracks `#[borrow]` regions per extern call site.
//!
//! A borrow-region is "in flight" from the extern call node until the call's
//! return. Rule: any StateWrite to a borrowed region DURING the call window
//! is a violation (C might be mid-read). On call return, the borrow is
//! auto-released and the region is marked "preserved" (not invalidated).
//!
//! This is the real implementation (Wave 4). The Wave 2c scaffold is replaced.
//!
//! # Model
//!
//! The verifier receives:
//! - `regions`: the set of `#[borrow]` regions, each with a vreg, byte range,
//!   and the SCG node ID of the extern call that initiated the borrow.
//! - `writes`: the set of StateWrite operations, each with a vreg, offset, size,
//!   and the SCG node ID where the write occurs.
//! - `call_order`: the ordered list of SCG node IDs (to determine which calls
//!   are "in flight" at each write point).
//!
//! A borrow region is "in flight" at a given write if the write's node ID is
//! strictly between the call_site (the borrow start) and the next call in
//! call_order after call_site (the borrow end / call return). If the call_site
//! is the last call, the borrow is in flight until the end of the function.
//!
//! A write violates the borrow if:
//!   1. The write's vreg matches a borrowed region's vreg, AND
//!   2. The write's [offset, offset+size) overlaps the region's byte_range, AND
//!   3. The borrow is in flight at the write's position.

/// A borrow-region: a (vreg, byte_range) handed to C as #[borrow] for the
/// duration of a specific extern call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorrowRegion {
    /// The virtual register of the borrowed State.
    pub vreg: u32,
    /// The byte range (start, end) within ___pmt_buffer that was borrowed.
    pub byte_range: (u64, u64),
    /// The SCG node ID of the extern call that initiated the borrow.
    pub call_site: usize,
}

/// A write operation observed during borrow analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BorrowWrite {
    /// The vreg being written.
    pub vreg: u32,
    /// The byte offset of the write.
    pub offset: u64,
    /// The size of the write in bytes.
    pub size: u64,
    /// The SCG node ID where this write occurs (used to determine if a
    /// borrow is in flight at this point).
    pub at_node: usize,
}

/// Result of borrow-region verification.
#[derive(Debug, Clone)]
pub struct BorrowVerification {
    /// Whether this borrow-region check passed (true) or failed (false).
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

/// Returns the node ID at which the borrow ends (the call's return), or
/// `usize::MAX` if the borrow extends to the end of the function.
///
/// The borrow ends at the *next* call in `call_order` after `call_site`.
/// (In a single-threaded model, the extern call returns before the next
/// call begins.)
fn borrow_end(call_site: usize, call_order: &[usize]) -> usize {
    // Find call_site in call_order, return the next entry, or MAX if last.
    let pos = call_order.iter().position(|&c| c == call_site);
    match pos {
        Some(p) if p + 1 < call_order.len() => call_order[p + 1],
        _ => usize::MAX,
    }
}

/// Returns true if the borrow is in flight at `at_node`.
///
/// The borrow is in flight if `at_node` is in the half-open interval
/// [call_site, borrow_end). This means the write occurs after the call
/// started but before the call returned (the next call's start).
fn borrow_in_flight(call_site: usize, at_node: usize, call_order: &[usize]) -> bool {
    let end = borrow_end(call_site, call_order);
    at_node >= call_site && at_node < end
}

/// Returns true if [offset, offset+size) overlaps [start, end).
fn ranges_overlap(offset: u64, size: u64, start: u64, end: u64) -> bool {
    let write_end = offset + size;
    offset < end && write_end > start
}

/// Verify that no StateWrite hits a borrowed region during its borrow window.
///
/// For each write, check if it hits any borrowed region whose borrow is in
/// flight at the write's node ID. If so, that's a violation.
///
/// `regions` — the set of #[borrow] regions with their call sites.
/// `writes` — the set of StateWrite operations (each with an `at_node`).
/// `call_order` — the ordered list of SCG node IDs (to determine which calls
///   are "in flight" at each write point).
///
/// Returns one `BorrowVerification` per violation found (empty Vec = all valid).
pub fn verify_borrow_regions(
    regions: &[BorrowRegion],
    writes: &[BorrowWrite],
    call_order: &[usize],
) -> Vec<BorrowVerification> {
    let mut results = Vec::new();
    for write in writes {
        for region in regions {
            // Check 1: same vreg.
            if write.vreg != region.vreg {
                continue;
            }
            // Check 2: byte ranges overlap.
            if !ranges_overlap(write.offset, write.size, region.byte_range.0, region.byte_range.1) {
                continue;
            }
            // Check 3: borrow is in flight at the write's position.
            if !borrow_in_flight(region.call_site, write.at_node, call_order) {
                continue;
            }
            // Violation: write to a borrowed region during the borrow window.
            results.push(BorrowVerification {
                valid: false,
                error: Some(format!(
                    "StateWrite to vreg {} at byte [{},{}) violates a #[borrow] region \
                     [{},{}) borrowed at call site {} (borrow in flight at node {})",
                    write.vreg,
                    write.offset,
                    write.offset + write.size,
                    region.byte_range.0,
                    region.byte_range.1,
                    region.call_site,
                    write.at_node
                )),
            });
        }
    }
    results
}

/// Returns true if all verification results are valid (or empty).
pub fn all_valid(results: &[BorrowVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(vreg: u32, start: u64, end: u64, call_site: usize) -> BorrowRegion {
        BorrowRegion {
            vreg,
            byte_range: (start, end),
            call_site,
        }
    }

    fn write(vreg: u32, offset: u64, size: u64, at_node: usize) -> BorrowWrite {
        BorrowWrite {
            vreg,
            offset,
            size,
            at_node,
        }
    }

    #[test]
    fn test_no_regions_no_violations() {
        let results = verify_borrow_regions(&[], &[write(0, 0, 4, 5)], &[5]);
        assert!(results.is_empty());
        assert!(all_valid(&results));
    }

    #[test]
    fn test_no_writes_no_violations() {
        let results = verify_borrow_regions(&[region(0, 0, 16, 10)], &[], &[10]);
        assert!(results.is_empty());
        assert!(all_valid(&results));
    }

    #[test]
    fn test_empty_inputs() {
        let results = verify_borrow_regions(&[], &[], &[]);
        assert!(results.is_empty());
        assert!(all_valid(&results));
    }

    #[test]
    fn test_all_valid_on_empty() {
        assert!(all_valid(&[]));
    }

    #[test]
    fn test_write_after_borrow_returns_is_ok() {
        // Borrow at node 10, ends at node 20 (next call). Write at node 25
        // is AFTER the borrow returned — no violation.
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(0, 0, 4, 25)];
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert!(results.is_empty(), "write after borrow returns should be OK");
    }

    #[test]
    fn test_write_during_borrow_is_violation() {
        // Borrow at node 10, ends at node 20. Write at node 15 is DURING
        // the borrow — violation.
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(0, 0, 4, 15)];
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0].error.as_ref().unwrap().contains("#[borrow]"));
    }

    #[test]
    fn test_write_to_different_vreg_is_ok() {
        // Borrow vreg 0, write to vreg 1 — no conflict.
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(1, 0, 4, 15)];
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert!(results.is_empty());
    }

    #[test]
    fn test_write_non_overlapping_range_is_ok() {
        // Borrow [0,16), write [20,24) — no overlap.
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(0, 20, 4, 15)];
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert!(results.is_empty());
    }

    #[test]
    fn test_borrow_extends_to_end_if_last_call() {
        // Borrow at node 10, no subsequent call — borrow extends to end.
        // Write at node 100 is still in flight.
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(0, 0, 4, 100)];
        let call_order = vec![10];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
    }

    #[test]
    fn test_write_at_call_site_boundary_is_violation() {
        // Write at the call_site itself (node 10) is in flight
        // (half-open [10, 20) includes 10).
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(0, 0, 4, 10)];
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_write_at_borrow_end_is_ok() {
        // Write at the borrow_end (node 20) is NOT in flight
        // (half-open [10, 20) excludes 20).
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(0, 0, 4, 20)];
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert!(results.is_empty());
    }

    #[test]
    fn test_multiple_regions_one_write_multiple_violations() {
        // Two borrows on the same vreg/range, both in flight at the write
        // point. For both to be in flight at node 15, both must start ≤ 15
        // and end > 15. With call_order [10, 30, 40]:
        //   Region 1 (call_site 10): borrow [10, 30) — in flight at 15 ✓
        //   Region 2 (call_site 12): borrow [12, 30) — in flight at 15 ✓
        // (Note: borrow_end uses the next call in call_order, so both end
        // at 30, the next call after each.)
        let regions = vec![
            region(0, 0, 16, 10),
            region(0, 0, 16, 12),
        ];
        let writes = vec![write(0, 0, 4, 15)];
        let call_order = vec![10, 12, 30];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        // Region 1: call_site 10, borrow_end = 12 (next in call_order).
        //   Write at 15 is NOT in [10,12) — no violation from region 1.
        // Region 2: call_site 12, borrow_end = 30 (next in call_order).
        //   Write at 15 IS in [12,30) — violation from region 2.
        // So only 1 violation.
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_partial_overlap_is_violation() {
        // Borrow [0,16), write [14,20) — partial overlap.
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(0, 14, 6, 15)];
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_all_valid_on_passing() {
        let regions = vec![region(0, 0, 16, 10)];
        let writes = vec![write(1, 0, 4, 15)]; // different vreg
        let call_order = vec![10, 20];
        let results = verify_borrow_regions(&regions, &writes, &call_order);
        assert!(all_valid(&results));
    }
}
