//! The number this project exists for, and what it means.

/// The bottom of the target range, in bytes.
pub const TARGET_LOW: u64 = 20 * MB;

/// The top of it, and the only figure that can actually be *missed*: the spec's
/// first user story asks for an installer "under ~30 MB".
pub const TARGET_HIGH: u64 = 30 * MB;

/// Upstream's Windows installer, the thing this is measured against.
///
/// Read as mebibytes for consistency with everything else here, and the figure
/// itself is inherited from the spec rather than measured by this project — if
/// upstream's 318 was decimal megabytes, every multiple below is flattering by
/// about 5% (63× would be 60×). Not worth chasing at these margins, worth
/// knowing before anyone quotes the multiple as precise.
pub const BASELINE: u64 = 318 * MB;

/// A mebibyte. Every published figure in this project — 318, 24.16, 21.25 — is
/// in these, so this is what "MB" means here and changing it would silently
/// restate every comparison.
pub const MB: u64 = 1024 * 1024;

/// How a measured artifact sits against the target range.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Below the range. Not a miss — the range's floor is an expectation, not a
    /// requirement, and coming in under it is the project working.
    Under(u64),
    /// Where the spec expected this to land.
    Inside {
        /// What is left before 30 MB.
        headroom: u64,
    },
    /// The finding ticket 24 is told not to bury: above 30 MB weakens the
    /// project's rationale.
    Over(u64),
}

pub fn verdict(bytes: u64) -> Verdict {
    if bytes > TARGET_HIGH {
        Verdict::Over(bytes - TARGET_HIGH)
    } else if bytes < TARGET_LOW {
        Verdict::Under(TARGET_LOW - bytes)
    } else {
        Verdict::Inside {
            headroom: TARGET_HIGH - bytes,
        }
    }
}

/// Bytes as the megabytes every other figure in this project is quoted in.
pub fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / MB as f64
}

/// How many times smaller than upstream, as the project's headline claim.
pub fn against_baseline(bytes: u64) -> f64 {
    BASELINE as f64 / bytes as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boundaries, because "materially above the target range" is the
    /// sentence that decides whether the Electron-pruning fallback comes back.
    #[test]
    fn only_passing_thirty_megabytes_is_a_miss() {
        assert_eq!(verdict(24 * MB), Verdict::Inside { headroom: 6 * MB });
        assert_eq!(verdict(TARGET_LOW), Verdict::Inside { headroom: 10 * MB });
        assert_eq!(verdict(TARGET_HIGH), Verdict::Inside { headroom: 0 });

        assert_eq!(verdict(TARGET_HIGH + MB), Verdict::Over(MB));
        assert_eq!(verdict(5 * MB), Verdict::Under(15 * MB));
    }

    /// The figures in the ticket and the spec are mebibytes: 24.16 MB is
    /// 25,337,856 bytes, measured on the shell ticket 23 built.
    #[test]
    fn megabytes_are_the_ones_every_other_figure_is_quoted_in() {
        assert_eq!(format!("{:.2}", megabytes(25_337_856)), "24.16");
        assert_eq!(format!("{:.2}", megabytes(BASELINE)), "318.00");
    }

    /// "About 13× smaller than upstream" is the project's claim, and this is
    /// where that multiple comes from.
    #[test]
    fn the_baseline_comparison_is_a_multiple_of_upstreams_installer() {
        assert_eq!(format!("{:.1}", against_baseline(25_337_856)), "13.2");
        assert_eq!(format!("{:.1}", against_baseline(BASELINE)), "1.0");
    }
}
