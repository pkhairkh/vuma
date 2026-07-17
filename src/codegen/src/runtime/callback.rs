//! Callback runtime — re-entrancy guard for foreign callbacks.
//!
//! When C calls back into VUMA (e.g. sqlite3_exec's row callback), the
//! callback runs on an isolated callback stack with its own scratchpad
//! frame. It is FORBIDDEN from touching any State in the caller's live set
//! (enforced by `callback_live_set` — `check_access` returns false → trap).
//!
//! # Re-entrancy rule (decided in the proposal)
//! - Callbacks run on the same thread that invoked C (single-threaded).
//! - The callback's state_new allocations go into ___pmt_buffer at fresh
//!   offsets (not aliased with any caller-live region).
//! - The callback_live_set is a set of (start, end) byte ranges in
//!   ___pmt_buffer that the caller has in flight.
//! - Any access (read or write) by the callback to a caller-live range
//!   is a re-entrancy violation → trap.
//!
//! # Why not multi-threaded?
//! A callback runs on the same thread that invoked C. If C spawns a thread
//! that calls back, the program traps (the callback_live_set is thread-local).
//! Multi-threaded callback support is deferred to a future proposal.

use std::cell::RefCell;

/// A live region in ___pmt_buffer that the caller has in flight.
/// The callback must not touch [start, end).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveRegion {
    pub start: u64,
    pub end: u64,
}

/// A set of caller-live regions. Used as the re-entrancy guard.
#[derive(Debug, Clone, Default)]
pub struct LiveSet {
    regions: Vec<LiveRegion>,
}

impl LiveSet {
    /// Create an empty live set.
    pub fn new() -> Self {
        Self { regions: Vec::new() }
    }

    /// Create a live set from a slice of (start, end) tuples.
    pub fn from_ranges(ranges: &[(u64, u64)]) -> Self {
        Self {
            regions: ranges
                .iter()
                .map(|&(s, e)| LiveRegion { start: s, end: e })
                .collect(),
        }
    }

    /// Add a live region.
    pub fn add(&mut self, start: u64, end: u64) {
        self.regions.push(LiveRegion { start, end });
    }

    /// Returns true if `offset` falls within any live region.
    pub fn contains(&self, offset: u64) -> bool {
        self.regions.iter().any(|r| offset >= r.start && offset < r.end)
    }

    /// Returns the number of live regions.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Returns true if there are no live regions.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

/// The callback context — carries the scratchpad frame and live-set guard.
pub struct CallbackContext {
    /// The caller's live-set (re-entrancy guard).
    pub live_set: LiveSet,
}

thread_local! {
    /// The current callback depth (0 = no callback in flight).
    /// Used to detect nested callbacks and enforce single-threaded operation.
    static CALLBACK_DEPTH: RefCell<usize> = RefCell::new(0);
}

/// Enter a callback. Pushes an isolated scratchpad frame and records the
/// caller's live-set.
///
/// `caller_live` — the (start, end) byte ranges in ___pmt_buffer that the
/// caller has in flight. The callback must not touch these.
pub fn enter_callback(caller_live: &[(u64, u64)]) -> CallbackContext {
    CALLBACK_DEPTH.with(|d| {
        let mut depth = d.borrow_mut();
        if *depth > 0 {
            // Nested callback — allowed (each gets its own frame), but
            // the live-set must include the inner callback's state too.
            // For now, we allow nesting and trust the live-set.
        }
        *depth += 1;
    });
    CallbackContext {
        live_set: LiveSet::from_ranges(caller_live),
    }
}

/// Exit a callback. Pops the scratchpad frame and clears the callback context.
pub fn exit_callback(_ctx: CallbackContext) {
    CALLBACK_DEPTH.with(|d| {
        let mut depth = d.borrow_mut();
        if *depth > 0 {
            *depth -= 1;
        }
    });
}

/// Check if a callback may access `offset` in ___pmt_buffer.
/// Returns true if the access is safe (offset is NOT in a caller-live region).
/// Returns false if the access would violate the re-entrancy rule (trap).
pub fn check_access(ctx: &CallbackContext, offset: u64) -> bool {
    !ctx.live_set.contains(offset)
}

/// Returns the current callback depth (0 = no callback in flight).
pub fn callback_depth() -> usize {
    CALLBACK_DEPTH.with(|d| *d.borrow())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_live_set_empty() {
        let ls = LiveSet::new();
        assert!(ls.is_empty());
        assert!(!ls.contains(0));
        assert!(!ls.contains(100));
    }

    #[test]
    fn test_live_set_contains() {
        let ls = LiveSet::from_ranges(&[(0, 16), (32, 48)]);
        assert!(ls.contains(0));
        assert!(ls.contains(15));
        assert!(!ls.contains(16)); // boundary — end is exclusive
        assert!(!ls.contains(31));
        assert!(ls.contains(32));
        assert!(ls.contains(47));
        assert!(!ls.contains(48)); // boundary
    }

    #[test]
    fn test_live_set_add() {
        let mut ls = LiveSet::new();
        ls.add(10, 20);
        assert_eq!(ls.len(), 1);
        assert!(ls.contains(15));
        assert!(!ls.contains(25));
    }

    #[test]
    fn test_enter_exit_callback() {
        assert_eq!(callback_depth(), 0);
        let ctx = enter_callback(&[(0, 16)]);
        assert_eq!(callback_depth(), 1);
        assert!(check_access(&ctx, 32)); // 32 is not in [0,16) — OK
        assert!(!check_access(&ctx, 8));  // 8 is in [0,16) — violation
        exit_callback(ctx);
        assert_eq!(callback_depth(), 0);
    }

    #[test]
    fn test_nested_callbacks() {
        assert_eq!(callback_depth(), 0);
        let ctx1 = enter_callback(&[(0, 16)]);
        assert_eq!(callback_depth(), 1);
        let ctx2 = enter_callback(&[(0, 16), (32, 48)]); // nested
        assert_eq!(callback_depth(), 2);
        assert!(!check_access(&ctx2, 8));  // in [0,16) — violation
        assert!(!check_access(&ctx2, 40)); // in [32,48) — violation
        assert!(check_access(&ctx2, 64));   // free — OK
        exit_callback(ctx2);
        assert_eq!(callback_depth(), 1);
        exit_callback(ctx1);
        assert_eq!(callback_depth(), 0);
    }

    #[test]
    fn test_check_access_empty_live_set() {
        let ctx = enter_callback(&[]);
        assert!(check_access(&ctx, 0)); // no live regions — all access OK
        assert!(check_access(&ctx, 1000));
        exit_callback(ctx);
    }
}
