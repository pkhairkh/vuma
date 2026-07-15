//! Hand-written UTC timestamp formatting.
//!
//! Replaces the `chrono` crate for the small subset of functionality VUMA
//! actually used: `Local::now().format("%Y-%m-%dT%H:%M:%S%.3f")` and
//! `Utc::now().to_rfc3339()`.
//!
//! The conversion from epoch-seconds to a civil (year, month, day) date uses
//! Howard Hinnant's `civil_from_days` algorithm — see
//! <https://howardhinnant.github.io/date_algorithms.html>. It is a pure
//! integer algorithm with no platform-specific code paths.
//!
//! All output is UTC. The legacy local-time log format is preserved in shape
//! (no timezone suffix) but is now reported in UTC, because there is no
//! portable `std`-only way to fetch the local timezone offset. This is an
//! acceptable trade-off for VUMA's logging/telemetry use cases.
//!
//! # Examples
//!
//! ```
//! use vuma::time::{now_utc_rfc3339, now_utc_iso8601_millis};
//!
//! let rfc = now_utc_rfc3339();          // "2024-08-13T12:34:56Z"
//! let iso = now_utc_iso8601_millis();   // "2024-08-13T12:34:56.789"
//! assert!(rfc.ends_with('Z'));
//! assert!(iso.as_bytes()[19] == b'.');
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

// ═══════════════════════════════════════════════════════════════════════════
// civil_from_days — Howard Hinnant's date algorithm
// ═══════════════════════════════════════════════════════════════════════════

/// Convert a count of days since the Unix epoch (1970-01-01) into a
/// `(year, month, day)` triple using the `civil_from_days` algorithm
/// from <https://howardhinnant.github.io/date_algorithms.html>.
///
/// `days` may be negative (for dates before 1970-01-01). The algorithm
/// is valid for any year in the range `[-32768, 32767]` and well beyond.
const fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch from 1970-01-01 to 0000-03-01 (start of a 400-year
    // era cycle). This makes leap-day the last day of the year, simplifying
    // the month-length arithmetic.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146_096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (y + if m <= 2 { 1 } else { 0 }, m, d)
}

// ═══════════════════════════════════════════════════════════════════════════
// Epoch helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Wall-clock seconds + sub-second nanos since the Unix epoch.
///
/// Returns `(0, 0)` if the system clock is set before `UNIX_EPOCH` (which
/// would be a system misconfiguration, not a normal condition).
fn epoch_parts() -> (i64, u32) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_secs() as i64, now.subsec_nanos())
}

/// Split a count of epoch-seconds into `(days_since_epoch, seconds_of_day)`,
/// where `seconds_of_day` is in `[0, 86_400)`. Uses `div_euclid`/`rem_euclid`
/// so negative epoch-seconds (pre-1970 dates) are handled correctly.
const fn split_days_and_sod(secs: i64) -> (i64, u32) {
    (secs.div_euclid(86_400), secs.rem_euclid(86_400) as u32)
}

// ═══════════════════════════════════════════════════════════════════════════
// Public formatters
// ═══════════════════════════════════════════════════════════════════════════

/// Format the current UTC time as an RFC 3339 string:
/// `YYYY-MM-DDTHH:MM:SSZ`.
///
/// This is the drop-in replacement for `chrono::Utc::now().to_rfc3339()`.
/// The trailing `Z` indicates UTC. Milliseconds are intentionally omitted
/// to keep the format stable; if sub-second precision is needed, use
/// [`now_utc_iso8601_millis`].
pub fn now_utc_rfc3339() -> String {
    let (secs, _nanos) = epoch_parts();
    format_secs_rfc3339(secs)
}

/// Format a specific count of Unix epoch seconds as
/// `YYYY-MM-DDTHH:MM:SSZ`. Useful for deterministic tests.
pub fn format_secs_rfc3339(secs: i64) -> String {
    let (days, sod) = split_days_and_sod(secs);
    let (y, m, d) = civil_from_days(days);
    let hh = sod / 3600;
    let mm = (sod % 3600) / 60;
    let ss = sod % 60;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, m, d, hh, mm, ss)
}

/// Format the current UTC time as `YYYY-MM-DDTHH:MM:SS.mmm` (no timezone
/// suffix, with millisecond precision).
///
/// This is the drop-in replacement for
/// `chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.3f")` used by the VUMA
/// logger. The shape is preserved; only the timezone has changed from
/// local to UTC.
pub fn now_utc_iso8601_millis() -> String {
    let (secs, nanos) = epoch_parts();
    let (days, sod) = split_days_and_sod(secs);
    let (y, m, d) = civil_from_days(days);
    let hh = sod / 3600;
    let mm = (sod % 3600) / 60;
    let ss = sod % 60;
    let ms = nanos / 1_000_000;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
        y, m, d, hh, mm, ss, ms
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Unix epoch — 1970-01-01.
    #[test]
    fn civil_from_days_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    /// Y2K — 2000-01-01 is 10_957 days after 1970-01-01.
    #[test]
    fn civil_from_days_y2k() {
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    }

    /// Leap day — 2020-02-29 is 18_321 days after 1970-01-01.
    #[test]
    fn civil_from_days_leap_day() {
        assert_eq!(civil_from_days(18_321), (2020, 2, 29));
    }

    /// Non-leap century — 1900-02-28 (1900 was NOT a leap year).
    #[test]
    fn civil_from_days_century_non_leap() {
        // 1900-01-01 is -25_567 days; +58 days = 1900-02-28 (not 02-29).
        assert_eq!(civil_from_days(-25_567 + 58), (1900, 2, 28));
    }

    /// Pre-epoch date — 1969-12-31 is day -1.
    #[test]
    fn civil_from_days_pre_epoch() {
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    /// RFC 3339 string for the Unix epoch is exactly `1970-01-01T00:00:00Z`.
    #[test]
    fn rfc3339_at_epoch() {
        assert_eq!(format_secs_rfc3339(0), "1970-01-01T00:00:00Z");
    }

    /// RFC 3339 for a known timestamp — 2020-02-29T12:34:56Z.
    /// 18_321 days * 86_400 + (12*3600 + 34*60 + 56) = 1_582_979_696.
    #[test]
    fn rfc3339_known_timestamp() {
        assert_eq!(
            format_secs_rfc3339(1_582_979_696),
            "2020-02-29T12:34:56Z"
        );
    }

    /// Negative epoch seconds (pre-1970) still format correctly.
    #[test]
    fn rfc3339_pre_epoch() {
        // -1 second = 1969-12-31T23:59:59Z.
        assert_eq!(format_secs_rfc3339(-1), "1969-12-31T23:59:59Z");
    }

    /// Live `now_utc_rfc3339()` has the expected shape.
    #[test]
    fn now_utc_rfc3339_shape() {
        let s = now_utc_rfc3339();
        assert_eq!(s.len(), 20, "expected 20-byte RFC 3339 string, got {:?}", s);
        assert!(s.ends_with('Z'));
        assert_eq!(s.as_bytes()[4], b'-');
        assert_eq!(s.as_bytes()[7], b'-');
        assert_eq!(s.as_bytes()[10], b'T');
        assert_eq!(s.as_bytes()[13], b':');
        assert_eq!(s.as_bytes()[16], b':');
    }

    /// Live `now_utc_iso8601_millis()` has the expected shape.
    #[test]
    fn now_utc_iso8601_millis_shape() {
        let s = now_utc_iso8601_millis();
        assert_eq!(s.len(), 23, "expected 23-byte ISO 8601 string, got {:?}", s);
        assert_eq!(s.as_bytes()[19], b'.');
        // The last 3 chars are millisecond digits.
        for &b in &s.as_bytes()[20..23] {
            assert!(b.is_ascii_digit(), "expected digit, got {}", b as char);
        }
    }
}
