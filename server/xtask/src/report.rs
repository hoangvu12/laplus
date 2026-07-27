//! What the build says about itself.

use crate::loc::{self, Breakdown};
use crate::size::{self, Verdict};

/// Everything a release build learned about what it produced.
pub struct Measurements {
    /// The NSIS installer — what a developer downloads.
    pub installer: u64,
    /// What lands on their disk once it has run.
    pub installed: Footprint,
    /// The application itself, before the installer compressed it.
    pub binary: u64,
    /// The Rust that is the server.
    pub server: Breakdown,
}

/// A directory, weighed.
pub struct Footprint {
    pub bytes: u64,
    pub files: usize,
    pub source: Source,
}

/// How the footprint was arrived at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The installer was run and the directory it made was weighed. The true
    /// figure, and the reason it is not the default: it writes to the machine
    /// running the build.
    Installed,
    /// The files the bundle ships were weighed where they were built. Close,
    /// and honest about not being the same thing — it cannot see the
    /// uninstaller NSIS generates.
    Payload,
}

pub fn render(measured: &Measurements) -> String {
    let mut out = String::new();
    out.push_str("# Artifact size\n\n");
    out.push_str(
        "Written by `cargo xtask release`. The project exists for these numbers, so\n\
         they are produced by the build that makes the thing rather than checked by\n\
         hand afterwards.\n\n",
    );

    out.push_str("## What a developer downloads and what it costs them\n\n");
    out.push_str("| | size | against 20–30 MB |\n|---|---|---|\n");
    push_row(&mut out, "Installer (NSIS)", measured.installer);
    push_row(&mut out, "Installed on disk", measured.installed.bytes);
    push_row(&mut out, "Application binary", measured.binary);
    out.push_str(&format!(
        "\nThe installed figure covers {} files, and is {}. Upstream's Windows installer \
         is **{:.0} MB**, so this one is **{:.1}× smaller** and what it installs is \
         **{:.1}× smaller**.\n\n",
        measured.installed.files,
        match measured.installed.source {
            Source::Installed =>
                "what the installer left on disk, measured by running it and weighing the \
                 directory it made — which holds this artifact and nothing else, since \
                 ticket 30 moved the install out of the one a developer's own state is in",
            Source::Payload =>
                "what the bundle ships, weighed where it was built without installing it, \
                 so it does not count the uninstaller NSIS writes",
        },
        size::megabytes(size::BASELINE),
        size::against_baseline(measured.installer),
        size::against_baseline(measured.installed.bytes),
    ));

    if let Verdict::Over(over) = size::verdict(measured.installer) {
        out.push_str(&format!(
            "**MISSED.** The installer is {:.2} MB over the 30 MB ceiling. The spec is \
             explicit about what that means: the project's rationale is weakened and the \
             Electron-pruning fallback deserves reconsidering.\n\n\
             Where to look first: the web bundle is about four fifths of the artifact, and \
             embedding does not compress it — `t3code/apps/web/dist` adds its own size to \
             the binary byte for byte, so anything trimmed there comes off one for one. \
             The Rust side, Tauri's own overhead included, is under 5 MB and is not the \
             risk. Ticket 24's comments name the two largest single contributors.\n\n",
            size::megabytes(over),
        ));
    }

    out.push_str("## How much Rust the server is\n\n");
    if measured.server.balanced {
        out.push_str(&format!(
            "| | lines |\n|---|---|\n\
             | total | {} |\n\
             | comments | {} |\n\
             | `#[cfg(test)]` unit tests | {} |\n\
             | blank | {} |\n\
             | **production code** | **{}** |\n\n",
            thousands(measured.server.total),
            thousands(measured.server.comment),
            thousands(measured.server.test),
            thousands(measured.server.blank),
            thousands(measured.server.production()),
        ));
        out.push_str(&format!(
            "The spec's signal to stop and re-evaluate is roughly {} lines of Rust, and \
             the figure it is about is **production code: {}**{}. The total above it is \
             mostly prose and unit tests — a third of this server by line is its own \
             tests — and reporting *that* against the signal would write a false alarm \
             into every build.\n",
            thousands(loc::SIGNAL),
            thousands(measured.server.production()),
            // Stated as a margin rather than as a judgement, because "comfortably
            // inside" stops being true somewhere before 20,000 and nothing here
            // would notice.
            match measured.server.production().checked_sub(loc::SIGNAL) {
                Some(over) => format!(", which is {} lines past it — MISSED", thousands(over)),
                None => format!(
                    ", {} lines inside it",
                    thousands(loc::SIGNAL - measured.server.production())
                ),
            },
        ));
    } else {
        out.push_str(
            "The server's sources **could not be classified**: the scan ended inside a \
             comment, a literal or a `#[cfg(test)]` region, which cannot happen in a \
             well-formed file. No line count is reported, because a wrong one is worse \
             than none.\n",
        );
    }

    out
}

fn push_row(out: &mut String, label: &str, bytes: u64) {
    out.push_str(&format!(
        "| {label} | **{:.2} MB** | {} |\n",
        size::megabytes(bytes),
        match size::verdict(bytes) {
            Verdict::Under(under) => format!("{:.2} MB under the range", size::megabytes(under)),
            Verdict::Inside { headroom } =>
                format!("inside, {:.2} MB of headroom", size::megabytes(headroom)),
            Verdict::Over(over) => format!("**{:.2} MB over**", size::megabytes(over)),
        }
    ));
}

/// `32134` reads as a number; `32,134` reads as a size.
fn thousands(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::new();
    for (seen, digit) in digits.chars().enumerate() {
        if seen > 0 && (digits.len() - seen).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::size::MB;

    fn measurements(installer: u64) -> Measurements {
        Measurements {
            installer,
            installed: Footprint {
                bytes: 24 * MB,
                files: 3,
                source: Source::Installed,
            },
            binary: 24 * MB,
            server: Breakdown {
                total: 32_134,
                comment: 8_379,
                test: 10_052,
                blank: 1_607,
                balanced: true,
            },
        }
    }

    /// The ticket's fourth criterion, which is about the report and not about
    /// the build: a figure with neither the target nor the baseline beside it
    /// does not tell anyone whether the project worked.
    #[test]
    fn both_figures_are_recorded_against_the_target_and_the_baseline() {
        let written = render(&measurements(5 * MB));

        assert!(written.contains("5.00 MB"), "the installer: {written}");
        assert!(written.contains("24.00 MB"), "the footprint: {written}");
        assert!(written.contains("20–30 MB"), "the target: {written}");
        assert!(written.contains("318"), "upstream's baseline: {written}");
    }

    /// "That is a finding, not a footnote." A build that quietly notes it went
    /// over is the failure this wording is guarding against.
    #[test]
    fn a_miss_is_stated_as_a_miss_and_a_pass_makes_no_such_claim() {
        let missed = render(&measurements(42 * MB));
        assert!(missed.contains("MISSED"), "{missed}");
        assert!(
            missed.contains("12.00 MB over"),
            "the shortfall, by how much: {missed}"
        );

        let met = render(&measurements(5 * MB));
        assert!(!met.contains("MISSED"), "{met}");
    }

    /// Ticket 24's last comment, settled: reporting the naive total writes a
    /// false alarm into every build, so the figure set against the 20K signal
    /// is production code and the rest of the split is shown behind it.
    #[test]
    fn the_line_count_set_against_the_twenty_thousand_signal_is_production_code() {
        let written = render(&measurements(5 * MB));

        assert!(written.contains("12,096"), "production code: {written}");
        assert!(written.contains("32,134"), "and the total behind it: {written}");
        assert!(written.contains("20,000"), "the signal: {written}");
        assert!(
            !written.contains("MISSED"),
            "12,096 is inside the signal: {written}"
        );
    }

    /// Running the installer to see what it leaves behind touches the machine
    /// doing the build, so it is opt-in — and a footprint that was inferred
    /// from the payload instead has to say so rather than pass for a
    /// measurement.
    #[test]
    fn the_footprint_says_whether_it_was_measured_or_inferred() {
        let mut measured = measurements(5 * MB);
        measured.installed.source = Source::Installed;
        let real = render(&measured);
        assert!(real.contains("running it and weighing the directory it made"), "{real}");

        measured.installed.source = Source::Payload;
        let inferred = render(&measured);
        assert!(inferred.contains("without installing"), "{inferred}");
    }

    /// A scan that lost its place must not have its numbers quoted as though
    /// they were measurements.
    #[test]
    fn an_unbalanced_scan_is_reported_instead_of_its_numbers() {
        let mut measured = measurements(5 * MB);
        measured.server.balanced = false;

        let written = render(&measured);
        assert!(written.contains("could not be classified"), "{written}");
    }
}
