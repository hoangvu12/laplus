//! A pairing URL as a square somebody's phone can look at.
//!
//! The problem this solves is small and completely real: pairing a phone with a
//! headless server means moving a twelve-character credential from an SSH window
//! to a device that cannot paste from it. Typing `open http://10.0.0.4:4773/#token=k7m2p9x4qw3z`
//! by hand, correctly, is the actual user experience of `docs/running-headless.md`
//! without this module.
//!
//! Upstream reached the same conclusion and prints one from `t3 serve`
//! (`formatHeadlessServeOutput`, over a hand-written `renderTerminalQrCode`).
//! This uses the `qrcode` crate's own unicode renderer, which draws the same
//! half-block characters for the same reason: a terminal cell is about twice as
//! tall as it is wide, so one character carrying two rows is what keeps the
//! square square and the whole code inside eighty columns.
//!
//! **The quiet zone is not decoration.** The four-module border is part of the
//! specification, and a reader that has to find the code's edge against whatever
//! text happens to be above it in a scrollback needs it. It is the difference
//! between a code that scans on the first try and one somebody blames their
//! camera for.

/// Draw `value` as a QR code, or say why not.
///
/// `None` rather than an error type because there is exactly one thing a caller
/// does with the failure: print the URL alone, which it was going to print
/// anyway. A pairing URL that will not encode is a URL long past anything a
/// phone camera would read, and the server has no business refusing to start
/// over it.
pub fn drawn(value: &str) -> Option<String> {
    let code = qrcode::QrCode::new(value.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pairing_url_becomes_a_square_of_blocks() {
        let drawn = drawn("http://192.168.1.10:4773/#token=k7m2p9x4qw3z").expect("a code");
        let lines: Vec<&str> = drawn.lines().collect();
        assert!(lines.len() > 10, "a QR code is more than {} rows", lines.len());
        // Every row the same width, or it is not a square and will not scan.
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|line| line.chars().count() == width));
        assert!(drawn.chars().any(|character| character == '█'));
    }

    /// Eighty columns is the terminal this is read in. A code wider than the
    /// window wraps, and a wrapped QR code is not a QR code.
    #[test]
    fn a_pairing_url_fits_in_a_terminal() {
        let drawn = drawn("http://192.168.100.200:4773/#token=k7m2p9x4qw3z").expect("a code");
        let widest = drawn.lines().map(|line| line.chars().count()).max().unwrap_or(0);
        assert!(widest <= 80, "{widest} columns is wider than a terminal");
    }

    /// The border is part of the format. Without it a reader cannot tell the
    /// code from the scrollback above it.
    #[test]
    fn there_is_a_quiet_zone_around_it() {
        let drawn = drawn("http://10.0.0.4:4773/#token=k7m2p9x4qw3z").expect("a code");
        let lines: Vec<&str> = drawn.lines().collect();
        assert!(lines[0].chars().all(|character| character == ' ' || character == '█'));
        assert!(
            lines[0].trim().is_empty() || lines[1].trim().is_empty(),
            "the top of the code is not padded"
        );
    }

    /// Nothing here refuses to start a server. A value too long to encode is
    /// answered with no code rather than an error a caller has to handle.
    #[test]
    fn something_too_long_to_encode_is_simply_no_code() {
        assert_eq!(drawn(&"x".repeat(10_000)), None);
    }
}
