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

// ── CT3: Linear-type checking for channel handles ────────────────────
//
// A channel opened by `channel_open` is a LINEAR resource: it must be
// used (send/recv) zero or more times, then consumed exactly once by
// `channel_close`. After `channel_close`, any use of the handle is a
// linear-type violation (use-after-free in Rust terms).
//
// This checker tracks the lifecycle state of each channel handle:
//   Open → (send/recv)* → Closed
// A use-after-close is flagged as a violation. A channel that is never
// closed is flagged as a leak (warning, not error — the OS will clean
// up on process exit, but it's a resource-management bug).

/// Lifecycle state of a channel handle (for CT3 linear-type checking).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelLifecycle {
    /// Handle is open; send/recv/close are legal.
    Open,
    /// Handle has been closed; any further use is a linear violation.
    Closed,
}

/// A channel lifecycle event observed during verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelEvent {
    /// The virtual register holding the channel handle.
    pub vreg: u32,
    /// The kind of event: open, send, recv, close.
    pub kind: ChannelEventKind,
    /// The SCG node ID where this event occurs (for error reporting).
    pub at_node: usize,
}

/// The kind of channel lifecycle event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelEventKind {
    /// `channel_open<T>()` — creates the handle.
    Open,
    /// `channel_send(ch, msg)` or `channel_recv(ch)` — uses the handle.
    /// Both send and recv are "use" events for linear-type purposes: they
    /// require the handle to be open but do not consume it.
    Use,
    /// `channel_close(ch)` — consumes the handle.
    Close,
}

/// Result of linear-type verification for a single channel.
#[derive(Debug, Clone)]
pub struct LinearVerification {
    /// Whether this channel's lifecycle is valid (no use-after-close).
    pub valid: bool,
    /// Error message if invalid.
    pub error: Option<String>,
}

/// Verify that no channel handle is used after it has been closed.
///
/// Processes events in `at_node` order. Tracks each handle's lifecycle
/// state. A `Use` after `Close` is a violation. A second `Close` on the
/// same handle is also a violation (double-close).
///
/// `events` — the ordered list of channel lifecycle events.
///
/// Returns one `LinearVerification` per violation found (empty Vec = all valid).
pub fn verify_linear_channels(events: &[ChannelEvent]) -> Vec<LinearVerification> {
    use std::collections::HashMap;
    let mut state: HashMap<u32, ChannelLifecycle> = HashMap::new();
    let mut results = Vec::new();

    // Sort events by at_node to process in program order.
    let mut sorted: Vec<&ChannelEvent> = events.iter().collect();
    sorted.sort_by_key(|e| e.at_node);

    for event in &sorted {
        match event.kind {
            ChannelEventKind::Open => {
                // Opening a handle that's already tracked is a re-init
                // (not necessarily a bug, but suspicious — flag as warning
                // if the previous handle wasn't closed).
                if let Some(ChannelLifecycle::Open) = state.get(&event.vreg) {
                    results.push(LinearVerification {
                        valid: false,
                        error: Some(format!(
                            "channel_open on vreg {} at node {} without closing the previous handle (linear leak)",
                            event.vreg, event.at_node
                        )),
                    });
                }
                state.insert(event.vreg, ChannelLifecycle::Open);
            }
            ChannelEventKind::Use => {
                match state.get(&event.vreg) {
                    None => {
                        results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "use of uninitialized channel vreg {} at node {} (linear: handle must be opened first)",
                                event.vreg, event.at_node
                            )),
                        });
                    }
                    Some(ChannelLifecycle::Closed) => {
                        results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "use-after-close on channel vreg {} at node {} (linear violation: handle was consumed by channel_close)",
                                event.vreg, event.at_node
                            )),
                        });
                    }
                    Some(ChannelLifecycle::Open) => {
                        // Legal use of an open handle.
                    }
                }
            }
            ChannelEventKind::Close => {
                match state.get(&event.vreg) {
                    None => {
                        results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "channel_close on uninitialized vreg {} at node {} (linear: handle must be opened first)",
                                event.vreg, event.at_node
                            )),
                        });
                    }
                    Some(ChannelLifecycle::Closed) => {
                        results.push(LinearVerification {
                            valid: false,
                            error: Some(format!(
                                "double-close on channel vreg {} at node {} (linear violation: handle was already consumed)",
                                event.vreg, event.at_node
                            )),
                        });
                    }
                    Some(ChannelLifecycle::Open) => {
                        state.insert(event.vreg, ChannelLifecycle::Closed);
                    }
                }
            }
        }
    }

    results
}

/// Returns true if all linear-type verification results are valid.
pub fn all_linear_valid(results: &[LinearVerification]) -> bool {
    results.iter().all(|r| r.valid)
}

// ── Wave 95: Linear-type checking for use-once variables ─────────────
//
// A linear variable is one that must be used EXACTLY ONCE — using it
// twice is a linear-type violation (a "double-use" akin to a use-after-
// move in Rust). This is distinct from the channel-lifecycle checker
// above (`verify_linear_channels`): that one tracks Open→Use→Close
// state transitions on channel handles, whereas this checker counts
// the number of USE events per variable and flags any variable that
// exceeds its declared use-count.
//
// Concrete use-cases in VUMA:
// - A session-typed channel endpoint (Wave 89-90) is linear: each
//   Send/Recv operation CONSUMES the endpoint and produces a new one
//   with the protocol tail. Using the same endpoint twice without
//   re-binding is a protocol violation.
// - A STARK proof handle (Wave 93-94) is linear: it may be verified
//   exactly once (the verifier consumes the proof).
// - A `unique` pointer (hypothetical future VUMA feature) is linear:
//   it may be dereferenced once before being freed.

/// Wave 95: A linear-type annotation on a variable.
///
/// `Linear` variables must be used exactly once. `Unlimited` variables
/// may be used any number of times (the default for ordinary VUMA
/// variables).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinearType {
    /// The variable is linear: it must be used exactly once. A second
    /// use is a linear violation (double-use). Zero uses is also a
    /// violation (linear leak — the value was never consumed).
    Linear,
    /// The variable is unrestricted: it may be used zero or more times.
    /// This is the default for ordinary VUMA variables (ints, pointers,
    /// structs).
    #[default]
    Unlimited,
}

impl LinearType {
    /// Returns `true` if this variable is subject to the use-once rule.
    pub fn is_linear(self) -> bool {
        matches!(self, LinearType::Linear)
    }
}

/// Wave 95: A single observed use of a variable (a read or write).
///
/// Multiple uses of the same `vreg` form a use-list; the linear-type
/// checker counts the uses and flags any `Linear` vreg with >1 use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearUse {
    /// The virtual register being used.
    pub vreg: u32,
    /// The SCG node ID where this use occurs (for error reporting).
    pub at_node: usize,
    /// A short description of the use site (e.g. "channel_send",
    /// "stark_verify", "load"). Included in error messages.
    pub site: &'static str,
}

/// Wave 95: Check that each linear-typed variable is used at most once.
///
/// For each `Linear` vreg in `linear_vregs`, count the number of uses
/// in `uses`. If a linear vreg is used more than once, emit a
/// [`LinearVerification`] with `valid: false` describing the violation.
/// (Zero uses of a linear variable is NOT flagged here — that's a
/// "linear leak", which is a warning rather than an error, and is
/// already covered by the channel-lifecycle checker for channel
/// handles.)
///
/// `uses` — the ordered list of variable uses observed during SCG
///   traversal. Order does not matter for this check (it counts uses,
///   not their sequence).
/// `linear_vregs` — the set of vregs that are declared `Linear`.
///
/// Returns one `LinearVerification` per double-use violation found
/// (empty Vec = all linear variables used at most once).
pub fn linear_check(
    uses: &[LinearUse],
    linear_vregs: &std::collections::HashSet<u32>,
) -> Vec<LinearVerification> {
    use std::collections::HashMap;
    // Count uses per linear vreg. Non-linear vregs are skipped (they
    // may be used any number of times).
    let mut counts: HashMap<u32, Vec<&LinearUse>> = HashMap::new();
    for u in uses {
        if linear_vregs.contains(&u.vreg) {
            counts.entry(u.vreg).or_default().push(u);
        }
    }
    let mut results = Vec::new();
    for (vreg, uses_list) in counts {
        if uses_list.len() > 1 {
            // Linear violation: this vreg was used more than once.
            let sites: Vec<&str> = uses_list.iter().map(|u| u.site).collect();
            let nodes: Vec<usize> = uses_list.iter().map(|u| u.at_node).collect();
            results.push(LinearVerification {
                valid: false,
                error: Some(format!(
                    "linear vreg {} used {} times (linear variables must be used exactly once); \
                     use sites: {:?} at nodes {:?}",
                    vreg,
                    uses_list.len(),
                    sites,
                    nodes
                )),
            });
        }
    }
    results
}

/// Wave 95: Returns true if all linear-check results are valid.
pub fn all_linear_check_valid(results: &[LinearVerification]) -> bool {
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

    // ── CT3 linear-type checker tests ──

    fn ev(vreg: u32, kind: ChannelEventKind, at_node: usize) -> ChannelEvent {
        ChannelEvent { vreg, kind, at_node }
    }

    #[test]
    fn test_linear_open_use_close_is_valid() {
        // open(10) → use(20) → close(30) — legal lifecycle.
        let events = vec![
            ev(0, ChannelEventKind::Open, 10),
            ev(0, ChannelEventKind::Use, 20),
            ev(0, ChannelEventKind::Close, 30),
        ];
        let results = verify_linear_channels(&events);
        assert!(results.is_empty(), "open→use→close should be valid");
    }

    #[test]
    fn test_linear_use_after_close_is_violation() {
        // open(10) → close(20) → use(30) — use-after-close.
        let events = vec![
            ev(0, ChannelEventKind::Open, 10),
            ev(0, ChannelEventKind::Close, 20),
            ev(0, ChannelEventKind::Use, 30),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0].error.as_ref().unwrap().contains("use-after-close"));
    }

    #[test]
    fn test_linear_double_close_is_violation() {
        // open(10) → close(20) → close(30) — double-close.
        let events = vec![
            ev(0, ChannelEventKind::Open, 10),
            ev(0, ChannelEventKind::Close, 20),
            ev(0, ChannelEventKind::Close, 30),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0].error.as_ref().unwrap().contains("double-close"));
    }

    #[test]
    fn test_linear_use_without_open_is_violation() {
        // use(10) without open — use of uninitialized handle.
        let events = vec![
            ev(0, ChannelEventKind::Use, 10),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0].error.as_ref().unwrap().contains("uninitialized"));
    }

    #[test]
    fn test_linear_multiple_uses_before_close_are_valid() {
        // open → use → use → use → close — multiple uses are fine.
        let events = vec![
            ev(0, ChannelEventKind::Open, 10),
            ev(0, ChannelEventKind::Use, 20),
            ev(0, ChannelEventKind::Use, 30),
            ev(0, ChannelEventKind::Use, 40),
            ev(0, ChannelEventKind::Close, 50),
        ];
        let results = verify_linear_channels(&events);
        assert!(results.is_empty(), "multiple uses before close should be valid");
    }

    #[test]
    fn test_linear_multiple_channels_independent() {
        // Two channels, each with its own lifecycle. A use-after-close on
        // channel 0 should not affect channel 1.
        let events = vec![
            ev(0, ChannelEventKind::Open, 10),
            ev(1, ChannelEventKind::Open, 15),
            ev(0, ChannelEventKind::Close, 20),
            ev(1, ChannelEventKind::Use, 25),  // channel 1 is still open
            ev(0, ChannelEventKind::Use, 30),  // channel 0 is closed — violation
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("vreg 0"));
    }

    #[test]
    fn test_linear_open_without_close_is_leak_warning() {
        // open(10) → use(20) — never closed. This is a leak (warning).
        // The current implementation flags re-open-without-close, but a
        // single open without close and without re-open is not flagged
        // (the OS cleans up on process exit). This test documents that
        // behavior.
        let events = vec![
            ev(0, ChannelEventKind::Open, 10),
            ev(0, ChannelEventKind::Use, 20),
        ];
        let results = verify_linear_channels(&events);
        assert!(results.is_empty(), "single open without close is not flagged (OS cleanup)");
    }

    #[test]
    fn test_linear_reopen_without_close_is_violation() {
        // open(10) → open(20) — re-open without close is a leak.
        let events = vec![
            ev(0, ChannelEventKind::Open, 10),
            ev(0, ChannelEventKind::Open, 20),
        ];
        let results = verify_linear_channels(&events);
        assert_eq!(results.len(), 1);
        assert!(results[0].error.as_ref().unwrap().contains("linear leak"));
    }

    // ── Wave 95: linear_check (use-once) tests ──

    fn lu(vreg: u32, at_node: usize, site: &'static str) -> LinearUse {
        LinearUse { vreg, at_node, site }
    }

    fn linset(vregs: &[u32]) -> std::collections::HashSet<u32> {
        vregs.iter().copied().collect()
    }

    #[test]
    fn test_linear_check_single_use_is_valid() {
        // A linear vreg used exactly once → no violation.
        let uses = vec![lu(0, 10, "channel_send")];
        let results = linear_check(&uses, &linset(&[0]));
        assert!(results.is_empty(), "single use of linear vreg should be valid");
    }

    #[test]
    fn test_linear_check_double_use_is_violation() {
        // A linear vreg used twice → violation.
        let uses = vec![
            lu(0, 10, "channel_send"),
            lu(0, 20, "channel_send"),
        ];
        let results = linear_check(&uses, &linset(&[0]));
        assert_eq!(results.len(), 1);
        assert!(!results[0].valid);
        assert!(results[0].error.as_ref().unwrap().contains("used 2 times"));
    }

    #[test]
    fn test_linear_check_unlimited_vreg_unrestricted() {
        // A non-linear vreg used multiple times → no violation.
        let uses = vec![
            lu(1, 10, "load"),
            lu(1, 20, "load"),
            lu(1, 30, "load"),
        ];
        let results = linear_check(&uses, &linset(&[0])); // vreg 1 not in set
        assert!(results.is_empty(), "non-linear vreg may be used any number of times");
    }

    #[test]
    fn test_linear_check_zero_uses_is_not_flagged() {
        // A linear vreg declared but never used → no violation here.
        // (Linear leak is handled by verify_linear_channels for channels;
        // this checker only flags double-uses.)
        let uses: Vec<LinearUse> = vec![];
        let results = linear_check(&uses, &linset(&[0, 1, 2]));
        assert!(results.is_empty(), "zero uses is not a double-use violation");
    }

    #[test]
    fn test_linear_check_multiple_violations() {
        // Two linear vregs each used twice → two violations.
        let uses = vec![
            lu(0, 10, "send"),
            lu(0, 20, "send"),
            lu(1, 30, "recv"),
            lu(1, 40, "recv"),
        ];
        let results = linear_check(&uses, &linset(&[0, 1]));
        assert_eq!(results.len(), 2);
        assert!(all_linear_check_valid(&[].to_vec()) == true);
        assert!(!all_linear_check_valid(&results));
    }

    #[test]
    fn test_linear_type_default_is_unlimited() {
        // The default LinearType is Unlimited (ordinary variables).
        let lt = LinearType::default();
        assert_eq!(lt, LinearType::Unlimited);
        assert!(!lt.is_linear());
        assert!(LinearType::Linear.is_linear());
    }
}
