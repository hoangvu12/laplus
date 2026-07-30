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

/// [`iso_from_epoch`] from one millisecond count rather than two numbers.
///
/// For the callers that do arithmetic on an instant before rendering it —
/// [`crate::threads::Adoption`] draws a window either side of now — where
/// splitting the seconds off at the call site would be the same division written
/// out in a place that has no other reason to know about it.
pub fn iso_from_epoch_millis(millis: u64) -> String {
    iso_from_epoch(millis / 1_000, (millis % 1_000) as u32)
}

/// A timestamp a *client* sent, back on this server's clock — or `None` for one
/// that does not name an instant at all.
///
/// The inverse of [`iso_from_epoch_millis`], and the only reader of this wire
/// that has to go that way. Every other timestamp here was rendered by this
/// module or by the registry's `strftime`, so comparing two of them is a string
/// comparison and needs no calendar — [`crate::threads::Adoption`] is that
/// argument written out. A wake time is the exception: `thread.snooze` carries a
/// moment the *developer* chose, and this server has to decide whether it is in
/// the future before it will store it.
///
/// **Strict about the shape, because the shape is the contract.** The client
/// builds one with `Date.toISOString()` (`Sidebar.snooze.ts`), which is
/// `YYYY-MM-DDTHH:MM:SS.mmmZ` — the same twenty-four characters
/// [`iso_from_epoch`] renders, digit for digit. Anything else is a string this
/// server cannot place on a clock, and storing one would put a value in a field
/// the contract types as an `IsoDateTime` that the client's own `Date.parse`
/// reads as `NaN`.
///
/// **The calendar is checked by rendering it back.** A shape check alone would
/// accept `2026-13-45T99:99:99.999Z`; instead the parsed instant is rendered by
/// [`iso_from_epoch_millis`] and compared against what arrived, so the only
/// strings that survive are the ones this module would have produced itself.
/// That makes the two functions inverses by construction rather than by two
/// lists of rules that have to be kept in step.
///
/// `None` for an instant before 1970 as well, which is not a special case: this
/// wire's timestamps are unsigned, and the one caller reads `None` as "not a
/// moment in the future" — which such a time is not.
pub fn epoch_millis_from_iso(rendered: &str) -> Option<u64> {
    let bytes = rendered.as_bytes();
    if bytes.len() != 24 {
        return None;
    }
    for (index, separator) in [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ] {
        if bytes[index] != separator {
            return None;
        }
    }
    // `get` rather than an index, because twenty-four *bytes* is not
    // twenty-four characters: a multi-byte character would put a range on a
    // boundary that is not a character boundary, and indexing there panics.
    let field = |range: std::ops::Range<usize>| -> Option<i64> {
        let text = rendered.get(range)?;
        text.bytes()
            .all(|character| character.is_ascii_digit())
            .then_some(())?;
        text.parse().ok()
    };

    let days = days_from_civil(field(0..4)?, field(5..7)?, field(8..10)?);
    let millis = days * 86_400_000
        + field(11..13)? * 3_600_000
        + field(14..16)? * 60_000
        + field(17..19)? * 1_000
        + field(20..23)?;

    let millis = u64::try_from(millis).ok()?;
    (iso_from_epoch_millis(millis) == rendered).then_some(millis)
}

/// A civil year, month and day to days since the Unix epoch.
///
/// Howard Hinnant's `days_from_civil`, the other half of the pair
/// [`civil_from_days`] is the first of. Total for every set of numbers the
/// digits above can spell, including the ones no calendar has — a thirteenth
/// month simply lands in the following year, and [`epoch_millis_from_iso`]'s
/// render-back is what turns that into a refusal.
///
/// Signed throughout, where [`civil_from_days`] can use unsigned halves: the
/// numbers here are whatever four digits spelled, so a zeroth day or a zeroth
/// month reaches the arithmetic and both would borrow past zero.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = match year >= 0 {
        true => year,
        false => year - 399,
    } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = match month > 2 {
        true => month - 3,
        false => month + 9,
    };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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

    /// The millisecond form renders the same instant the two-number form does,
    /// which is the whole of what it promises.
    #[test]
    fn a_millisecond_count_renders_as_the_same_instant() {
        assert_eq!(iso_from_epoch_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_from_epoch_millis(1_007), "1970-01-01T00:00:01.007Z");
        assert_eq!(
            iso_from_epoch_millis(1_700_000_000_909),
            iso_from_epoch(1_700_000_000, 909)
        );
    }

    /// Reading a rendering gives back the instant that produced it, for every
    /// instant the renderer's own test names.
    ///
    /// The pair is asserted as a round trip rather than against a second table
    /// of expected numbers, because that is the promise: the only strings
    /// [`epoch_millis_from_iso`] accepts are the ones this module renders.
    #[test]
    fn a_rendering_reads_back_as_the_instant_it_was_rendered_from() {
        for millis in [
            0,
            1_007,
            1_700_000_000_909,
            1_709_164_800_000,
            4_102_444_800_000,
            now_epoch_millis(),
        ] {
            let rendered = iso_from_epoch_millis(millis);
            assert_eq!(
                epoch_millis_from_iso(&rendered),
                Some(millis),
                "{rendered} did not read back"
            );
        }
    }

    /// A string that does not name an instant is `None` rather than a number,
    /// which is the whole of what the one caller needs: `thread.snooze` reads
    /// `None` as "not a moment in the future" and refuses it.
    ///
    /// The interesting half is the last four. A shape check alone would take
    /// them — twenty-four characters, separators in the right places, digits
    /// throughout — and the render-back is what turns a calendar nobody has
    /// into a refusal. The thirteenth month is the one that matters: it sorts
    /// *after* every real timestamp, so a server comparing strings without this
    /// would read it as comfortably in the future and store it.
    #[test]
    fn a_string_that_is_not_one_of_this_clocks_renderings_names_no_instant() {
        for hopeless in [
            "",
            "not-a-date",
            // Every other ISO 8601 shape a hand might reach for. Refused rather
            // than accommodated: this wire has one rendering, and a second
            // spelling of one moment is a field two readers disagree about.
            "2026-07-31T09:00:00Z",
            "2026-07-31T09:00:00.000+00:00",
            "2026-07-31 09:00:00.000Z",
            "2026-07-31T09:00:00.000z",
            // The right length and the wrong content.
            "2026-07-31T09:00:00.0000",
            "20260731T090000.000Z1234",
            "+026-07-31T09:00:00.000Z",
            // The right shape and no such moment.
            "2026-13-45T09:00:00.000Z",
            "2026-02-30T09:00:00.000Z",
            "2026-07-31T25:00:00.000Z",
            "2026-07-00T09:00:00.000Z",
            // Before this wire's clock starts.
            "1969-12-31T23:59:59.999Z",
        ] {
            assert_eq!(epoch_millis_from_iso(hopeless), None, "{hopeless}");
        }
    }

    /// A leap day is a real instant and a leap day in a year that has none is
    /// not — the one calendar rule a snooze can plausibly be given by hand.
    #[test]
    fn the_calendar_is_the_renderers_own() {
        assert!(epoch_millis_from_iso("2024-02-29T00:00:00.000Z").is_some());
        assert_eq!(epoch_millis_from_iso("2026-02-29T00:00:00.000Z"), None);
        assert!(epoch_millis_from_iso("2000-02-29T00:00:00.000Z").is_some());
        assert_eq!(epoch_millis_from_iso("2100-02-29T00:00:00.000Z"), None);
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
