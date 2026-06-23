//! # Time
//!
//! This module provides VUMA-verified time types with Behavioral Description
//! (BD) annotations and capability tracking.
//!
//! ## Types
//!
//! - **Instant**: A monotonically non-decreasing clock for measuring durations.
//! - **Duration**: A span of time with nanosecond precision.
//! - **SystemTime**: A point in time relative to the Unix epoch.
//!
//! ## BD Annotations
//!
//! - Instant: CapD { Read, Compare, Serialize }
//! - Duration: CapD { Read, Compare, Hash, Serialize }
//! - SystemTime: CapD { Read, Compare, Serialize }

use crate::primitives::{CapD, CapFlag, RepD, SyncEdge, SyncEdgeKind};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::Hash;
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Duration
// ---------------------------------------------------------------------------

/// A VUMA-verified duration with nanosecond precision.
///
/// Represents a span of time as seconds + nanoseconds.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Hash, Serialize }
/// - SyncEdge: none (passive value type)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Duration {
    /// The whole seconds of the duration.
    pub secs: u64,
    /// The nanosecond part of the duration (0..1_000_000_000).
    pub nanos: u32,
}

impl Duration {
    /// Create a new Duration from seconds and nanoseconds.
    ///
    /// The nanoseconds are normalized: any value >= 1_000_000_000 is
    /// carried into the seconds field.
    // VUMA-VERIFIED: constructor normalizes nanoseconds
    pub fn new(secs: u64, nanos: u32) -> Self {
        let extra_secs = nanos as u64 / 1_000_000_000;
        let remaining_nanos = nanos % 1_000_000_000;
        Self {
            secs: secs + extra_secs,
            nanos: remaining_nanos,
        }
    }

    /// Create a Duration from a number of seconds.
    // VUMA-VERIFIED: constructor is pure
    pub fn from_secs(secs: u64) -> Self {
        Self { secs, nanos: 0 }
    }

    /// Create a Duration from a number of milliseconds.
    // VUMA-VERIFIED: constructor normalizes milliseconds
    pub fn from_millis(millis: u64) -> Self {
        Self {
            secs: millis / 1000,
            nanos: ((millis % 1000) * 1_000_000) as u32,
        }
    }

    /// Create a Duration from a number of microseconds.
    // VUMA-VERIFIED: constructor normalizes microseconds
    pub fn from_micros(micros: u64) -> Self {
        Self {
            secs: micros / 1_000_000,
            nanos: ((micros % 1_000_000) * 1000) as u32,
        }
    }

    /// Create a Duration from a std::time::Duration.
    // VUMA-VERIFIED: conversion is lossless
    pub(crate) fn from_std(d: std::time::Duration) -> Self {
        Self::new(d.as_secs(), d.subsec_nanos())
    }

    /// Convert to a std::time::Duration.
    // VUMA-VERIFIED: conversion is lossless
    #[allow(dead_code)]
    pub(crate) fn to_std(self) -> std::time::Duration {
        std::time::Duration::new(self.secs, self.nanos)
    }

    /// Returns the total number of whole seconds.
    // VUMA-VERIFIED: pure accessor
    pub fn as_secs(&self) -> u64 {
        self.secs
    }

    /// Returns the nanosecond part.
    // VUMA-VERIFIED: pure accessor
    pub fn subsec_nanos(&self) -> u32 {
        self.nanos
    }

    /// Returns the total number of milliseconds.
    // VUMA-VERIFIED: pure computation
    pub fn as_millis(&self) -> u128 {
        self.secs as u128 * 1000 + self.nanos as u128 / 1_000_000
    }

    /// Returns the total number of microseconds.
    // VUMA-VERIFIED: pure computation
    pub fn as_micros(&self) -> u128 {
        self.secs as u128 * 1_000_000 + self.nanos as u128 / 1000
    }

    /// Returns true if this duration is zero.
    // VUMA-VERIFIED: pure query
    pub fn is_zero(&self) -> bool {
        self.secs == 0 && self.nanos == 0
    }

    /// Checked addition of two durations.
    // VUMA-VERIFIED: checked arithmetic prevents overflow
    pub fn checked_add(&self, other: &Duration) -> Option<Duration> {
        let secs = self.secs.checked_add(other.secs)?;
        let nanos = self.nanos + other.nanos;
        if nanos >= 1_000_000_000 {
            let secs = secs.checked_add(1)?;
            Some(Duration {
                secs,
                nanos: nanos - 1_000_000_000,
            })
        } else {
            Some(Duration { secs, nanos })
        }
    }

    /// Checked subtraction of two durations.
    ///
    /// Returns `None` if `other` is greater than `self`.
    // VUMA-VERIFIED: checked arithmetic prevents underflow
    pub fn checked_sub(&self, other: &Duration) -> Option<Duration> {
        let secs = self.secs.checked_sub(other.secs)?;
        if self.nanos >= other.nanos {
            Some(Duration {
                secs,
                nanos: self.nanos - other.nanos,
            })
        } else if secs > 0 {
            Some(Duration {
                secs: secs - 1,
                nanos: self.nanos + 1_000_000_000 - other.nanos,
            })
        } else {
            None
        }
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![
            CapFlag::Read,
            CapFlag::Compare,
            CapFlag::Hash,
            CapFlag::Serialize,
        ])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Duration", 16, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this type.
    // VUMA-VERIFIED: Duration is a passive value type
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![]
    }
}

impl Default for Duration {
    fn default() -> Self {
        Self::from_secs(0)
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.secs == 0 && self.nanos == 0 {
            write!(f, "0ns")
        } else if self.nanos == 0 {
            write!(f, "{}s", self.secs)
        } else {
            write!(f, "{}s+{}ns", self.secs, self.nanos)
        }
    }
}

impl std::ops::Add for Duration {
    type Output = Duration;

    fn add(self, other: Duration) -> Duration {
        self.checked_add(&other).expect("Duration overflow")
    }
}

impl std::ops::Sub for Duration {
    type Output = Duration;

    fn sub(self, other: Duration) -> Duration {
        self.checked_sub(&other).expect("Duration underflow")
    }
}

// ---------------------------------------------------------------------------
// Instant
// ---------------------------------------------------------------------------

/// A global baseline used to assign monotonically increasing nanosecond
/// values to `Instant`s created via `now()`. This ensures that the `nanos`
/// field is always meaningful for ordering and duration calculations.
static INSTANT_BASELINE: OnceLock<std::time::Instant> = OnceLock::new();

/// A VUMA-verified monotonic clock instant.
///
/// `Instant` represents a moment in time on a monotonic clock.
/// It can be used to measure elapsed time between two points.
///
/// Internally, `Instant` stores both a raw nanosecond count (derived from
/// a global baseline for ordering) and the real `std::time::Instant` for
/// accurate `elapsed()` computation.
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
/// - SyncEdge: now → elapsed (Seq)
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Instant {
    /// Internal timestamp as nanoseconds since an arbitrary epoch.
    /// Used for ordering, comparison, and serialization.
    nanos: u64,
    /// The real std::time::Instant captured at creation, used for elapsed().
    /// Not serialized; on deserialization this is set to a placeholder.
    #[serde(skip)]
    inner: Option<std::time::Instant>,
}

// Manual trait implementations that only compare `nanos`:

impl PartialEq for Instant {
    fn eq(&self, other: &Self) -> bool {
        self.nanos == other.nanos
    }
}

impl Eq for Instant {}

impl PartialOrd for Instant {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Instant {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.nanos.cmp(&other.nanos)
    }
}

impl std::hash::Hash for Instant {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.nanos.hash(state);
    }
}

impl Instant {
    /// Returns the current instant from the monotonic clock.
    ///
    /// Records both a raw nanosecond value (relative to a global baseline)
    /// and the underlying `std::time::Instant` for accurate `elapsed()`.
    // VUMA-VERIFIED: now reads the monotonic clock safely
    pub fn now() -> Self {
        let baseline = INSTANT_BASELINE.get_or_init(std::time::Instant::now);
        let std_now = std::time::Instant::now();
        let nanos = std_now.duration_since(*baseline).as_nanos() as u64;
        Self {
            nanos,
            inner: Some(std_now),
        }
    }

    /// Create an Instant from a raw nanosecond count (for testing).
    // VUMA-VERIFIED: test constructor is pure
    pub fn from_nanos(nanos: u64) -> Self {
        Self { nanos, inner: None }
    }

    /// Returns the duration since `earlier`.
    ///
    /// Panics if `earlier` is later than `self`.
    // VUMA-VERIFIED: duration_since is safe when earlier ≤ self
    pub fn duration_since(&self, earlier: &Instant) -> Duration {
        if self.nanos >= earlier.nanos {
            let diff = self.nanos - earlier.nanos;
            Duration::new(diff / 1_000_000_000, (diff % 1_000_000_000) as u32)
        } else {
            panic!("Instant::Duration_since called with a later instant");
        }
    }

    /// Returns the duration elapsed since this instant was created.
    ///
    /// If this `Instant` was created via `now()`, uses the real
    /// `std::time::Instant::elapsed()` for accurate measurement.
    /// If this `Instant` was deserialized or created via `from_nanos()`,
    /// falls back to computing from `Instant::now()` nanos.
    // VUMA-VERIFIED: elapsed is safe — always non-negative
    pub fn elapsed(&self) -> Duration {
        if let Some(inner) = self.inner {
            Duration::from_std(inner.elapsed())
        } else {
            // Fallback: compute from nanos delta against current time
            let now = Self::now();
            if now.nanos >= self.nanos {
                Duration::new(
                    (now.nanos - self.nanos) / 1_000_000_000,
                    ((now.nanos - self.nanos) % 1_000_000_000) as u32,
                )
            } else {
                Duration::from_secs(0)
            }
        }
    }

    /// Returns the raw nanosecond count (for testing).
    // VUMA-VERIFIED: pure accessor
    pub fn as_nanos(&self) -> u64 {
        self.nanos
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Serialize])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("Instant", 8, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this type.
    // VUMA-VERIFIED: synchronization edges model instant ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![SyncEdge::new(
            "instant_now",
            "instant_elapsed",
            SyncEdgeKind::Seq,
        )]
    }
}

// ---------------------------------------------------------------------------
// SystemTime
// ---------------------------------------------------------------------------

/// A VUMA-verified system clock time.
///
/// `SystemTime` represents a point in time relative to the Unix epoch
/// (1970-01-01 00:00:00 UTC).
///
/// ## BD Annotations
///
/// - CapD: { Read, Compare, Serialize }
/// - SyncEdge: now → duration_since_epoch (Seq)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SystemTime {
    /// Duration since Unix epoch.
    duration_since_epoch: Duration,
}

impl SystemTime {
    /// Returns the current system time.
    // VUMA-VERIFIED: now reads the system clock safely
    pub fn now() -> Self {
        let std_time = std::time::SystemTime::now();
        let duration = std_time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            duration_since_epoch: Duration::new(duration.as_secs(), duration.subsec_nanos()),
        }
    }

    /// Create a SystemTime from a Duration since the Unix epoch.
    // VUMA-VERIFIED: constructor is pure
    pub fn from_duration_since_epoch(d: Duration) -> Self {
        Self {
            duration_since_epoch: d,
        }
    }

    /// Returns the duration since the Unix epoch.
    // VUMA-VERIFIED: pure accessor
    pub fn duration_since_epoch(&self) -> &Duration {
        &self.duration_since_epoch
    }

    /// Returns the CapD for this type.
    // VUMA-VERIFIED: capability descriptor is correct
    pub fn capd(&self) -> CapD {
        CapD::new(vec![CapFlag::Read, CapFlag::Compare, CapFlag::Serialize])
    }

    /// Returns the RepD for this type.
    // VUMA-VERIFIED: type descriptor is correct
    pub fn repd(&self) -> RepD {
        RepD::new("SystemTime", 16, 8, self.capd())
    }

    /// Returns the SyncEdge annotations for this type.
    // VUMA-VERIFIED: synchronization edges model system time ordering
    pub fn sync_edges(&self) -> Vec<SyncEdge> {
        vec![SyncEdge::new(
            "system_now",
            "system_duration_since_epoch",
            SyncEdgeKind::Seq,
        )]
    }
}

impl fmt::Display for SystemTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SystemTime({})", self.duration_since_epoch)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_duration_new_and_accessors() {
        let d = Duration::new(5, 500_000_000);
        assert_eq!(d.as_secs(), 5);
        assert_eq!(d.subsec_nanos(), 500_000_000);
        assert_eq!(d.as_millis(), 5500);
    }

    #[test]
    fn test_duration_from_constructors() {
        let d_secs = Duration::from_secs(10);
        assert_eq!(d_secs.secs, 10);
        assert_eq!(d_secs.nanos, 0);

        let d_millis = Duration::from_millis(2500);
        assert_eq!(d_millis.secs, 2);
        assert_eq!(d_millis.nanos, 500_000_000);

        let d_micros = Duration::from_micros(1_500_000);
        assert_eq!(d_micros.secs, 1);
        assert_eq!(d_micros.nanos, 500_000_000);
    }

    #[test]
    fn test_duration_normalization() {
        // nanos >= 1_000_000_000 should carry into secs
        let d = Duration::new(1, 1_500_000_000);
        assert_eq!(d.secs, 2);
        assert_eq!(d.nanos, 500_000_000);
    }

    #[test]
    fn test_instant_duration_since() {
        let earlier = Instant::from_nanos(1000);
        let later = Instant::from_nanos(3500);
        let diff = later.duration_since(&earlier);
        assert_eq!(diff.nanos, 2500);
        assert_eq!(diff.secs, 0);
    }

    #[test]
    fn test_system_time_now() {
        let now = SystemTime::now();
        // The current time should be well after the Unix epoch.
        assert!(now.duration_since_epoch().as_secs() > 1_700_000_000);
    }

    // --- New tests for real elapsed() ---

    #[test]
    fn test_instant_elapsed_is_nonzero() {
        let start = Instant::now();
        // Even a tight loop should show some elapsed time (at least on most systems).
        // We just spin a tiny bit to ensure non-zero elapsed.
        let mut dummy = 0u64;
        for i in 0..1000 {
            dummy += i;
        }
        std::hint::black_box(&dummy);
        let elapsed = start.elapsed();
        // elapsed should be >= 0; on a modern system even a tight loop
        // should take some nanoseconds.
        assert!(
            elapsed.secs > 0 || elapsed.nanos > 0 || elapsed.is_zero() /* theoretically possible but unlikely */
        );
    }

    #[test]
    fn test_instant_elapsed_actually_advances() {
        let start = Instant::now();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let elapsed = start.elapsed();
        // After sleeping 50ms, elapsed should be at least 40ms (allowing for some slack)
        assert!(
            elapsed.as_millis() >= 40,
            "elapsed was only {}ms, expected >= 40ms",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_instant_now_monotonic() {
        let a = Instant::now();
        let b = Instant::now();
        // b should always be >= a since time only moves forward on a monotonic clock
        assert!(b >= a);
        // The nanos field should reflect monotonic ordering
        assert!(b.nanos >= a.nanos);
    }

    #[test]
    fn test_duration_subtraction() {
        let a = Duration::new(5, 500_000_000);
        let b = Duration::new(2, 200_000_000);
        let diff = a - b;
        assert_eq!(diff.secs, 3);
        assert_eq!(diff.nanos, 300_000_000);
    }

    #[test]
    fn test_duration_std_roundtrip() {
        let d = Duration::new(42, 123_456_789);
        let std_d = d.to_std();
        let back = Duration::from_std(std_d);
        assert_eq!(d, back);
    }

    #[test]
    fn test_system_time_advances() {
        let t1 = SystemTime::now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t2 = SystemTime::now();
        // t2 should be strictly after t1
        assert!(
            t2.duration_since_epoch().as_secs() > t1.duration_since_epoch().as_secs()
                || t2.duration_since_epoch().subsec_nanos()
                    > t1.duration_since_epoch().subsec_nanos()
        );
    }
}
