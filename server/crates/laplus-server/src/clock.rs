//! The wall clock, rendered the one way this wire understands it.
//!
//! The contract types a great many fields as `IsoDateTime`, and a client parses
//! every one of them with the same `new Date`. So there is one renderer, and it
//! is here rather than beside the first module that needed it.
//!
//! Hand-rolled rather than pulled in with a date crate, and not out of thrift:
//! the only thing this server wants from a calendar is this one string.
//! [`crate::store`] is the deliberate exception — the registry's timestamps come
//! from SQLite's own `strftime`, because a row's clock has to be the database's.
//! The two renderings match digit for digit, and
//! `crate::provider::tests::the_clock_renders_the_way_the_registrys_does` is what
//! holds them to it: one payload carries both clocks and a client parses both the
//! same way.
//!
//! Lifted out of [`crate::provider`] by ticket 10, which needs a timestamp on
//! every message, activity and session change rather than one per probe.

use std::time::{SystemTime, UNIX_EPOCH};

/// Now, rendered as the contract's `IsoDateTime`.
pub fn now_iso() -> String {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        // Before 1970 the machine's clock is wrong in a way this cannot fix, and
        // a panic in a status probe would be a poor way to say so.
        .unwrap_or_default();
    iso_from_epoch(since_epoch.as_secs(), since_epoch.subsec_millis())
}

/// Now, in milliseconds since the epoch.
///
/// For the contract's handful of bare-number `revision` fields, which are read
/// as "newer than the last one" and never rendered. A wall clock rather than a
/// counter because nothing here persists a counter across a restart, and a
/// revision that restarted at zero would read as older than what a client
/// already held.
pub fn now_epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// The civil date and time `seconds` after the Unix epoch, as an ISO string.
///
/// Split out from [`now_iso`] because a clock that cannot be given a time is a
/// clock that cannot be tested.
pub fn iso_from_epoch(seconds: u64, milliseconds: u32) -> String {
    let days = seconds / 86_400;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{milliseconds:03}Z",
        rest / 3_600,
        (rest % 3_600) / 60,
        rest % 60,
    )
}

/// Days since the Unix epoch to a civil year, month and day.
///
/// Howard Hinnant's `civil_from_days`, which is the standard way to do this
/// without a table of leap years: it shifts the epoch to March 1st of year 0 so
/// the leap day lands at the end of the cycle, then divides by the lengths of the
/// 400-, 100- and 4-year eras. Correct for every date this server can produce,
/// and the proof is arithmetic rather than a list of cases.
fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let shifted = days + 719_468;
    let era = match shifted >= 0 {
        true => shifted,
        false => shifted - 146_096,
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = match shifted_month < 10 {
        true => shifted_month + 3,
        false => shifted_month - 9,
    };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Five instants chosen for the arithmetic rather than for coverage: the
    /// epoch, a sub-second, an ordinary date, a leap day, and a century boundary
    /// that is not a leap year.
    #[test]
    fn the_clock_renders_a_known_instant_correctly() {
        assert_eq!(iso_from_epoch(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_epoch(1, 7), "1970-01-01T00:00:01.007Z");
        assert_eq!(
            iso_from_epoch(1_700_000_000, 909),
            "2023-11-14T22:13:20.909Z"
        );
        assert_eq!(iso_from_epoch(1_709_164_800, 0), "2024-02-29T00:00:00.000Z");
        assert_eq!(iso_from_epoch(4_102_444_800, 0), "2100-01-01T00:00:00.000Z");
    }

    /// Two timestamps taken in order never go backwards. The registry sorts on
    /// these and the client folds events by them, so a clock that could invert
    /// would reorder a transcript.
    #[test]
    fn the_clock_does_not_run_backwards() {
        let first = now_iso();
        let second = now_iso();
        assert!(first <= second, "{first} then {second}");
        assert_eq!(first.len(), 24, "{first} is not the captured shape");
    }
}
