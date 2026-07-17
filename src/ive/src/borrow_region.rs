//! Borrow-Region Verifier (scaffold — Wave 4 implements the real logic).
//!
//! Tracks `#[borrow]` regions per extern call site. A borrow-region is
//! "in flight" from the extern call node until the call's return. Rule:
//! any StateWrite to a borrowed region DURING the call window is a
//! violation (C might be mid-read). On call return, the borrow is
//! auto-released and the region is marked "preserved" (not invalidated).
//!
//! This file is a scaffold: the structs and function signatures are defined
//! but verify_borrow_regions always returns Vec::new() (always valid).
//! Wave 4 replaces the stub with the real implementation.

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
}

/// Result of borrow-region verification.
#[derive(Debug, Clone)]
pub struct BorrowVerification {
    /// Whether this borrow-region check passed (true) or failed (false).
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

/// Verify that no StateWrite hits a borrowed region during its borrow window.
///
/// STUB: always returns Vec::new() (all valid). Wave 4 implements the real
/// check: for each BorrowWrite, if it hits a BorrowRegion whose call_site
/// is "in flight" (the call hasn't returned yet), that's a violation.
///
/// `regions` — the set of #[borrow] regions with their call sites.
/// `writes` — the set of StateWrite operations observed in the function.
/// `call_order` — the ordered list of SCG node IDs (to determine which
///   calls are "in flight" at each write point).
pub fn verify_borrow_regions(
    regions: &[BorrowRegion],
    writes: &[BorrowWrite],
    call_order: &[usize],
) -> Vec<BorrowVerification> {
    // STUB: no violations. Wave 4 replaces this.
    let _ = (regions, writes, call_order);
    Vec::new()
}

/// Returns true if all verification results are valid.
pub fn all_valid(results: &[BorrowVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stub_returns_empty() {
        let regions = vec![BorrowRegion {
            vreg: 0,
            byte_range: (0, 16),
            call_site: 10,
        }];
        let writes = vec![BorrowWrite {
            vreg: 0,
            offset: 0,
            size: 4,
        }];
        let results = verify_borrow_regions(&regions, &writes, &[10, 11]);
        assert!(results.is_empty()); // stub always returns empty
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
}
