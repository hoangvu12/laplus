//! What a server with no window says about itself when it starts.
//!
//! The desktop shell has a Settings panel: it draws the exposure switch, lists
//! [`crate::endpoints`]'s addresses, and opens its own window on the right one.
//! `laplus-server` has a terminal and one chance to say the same things, to
//! somebody who may be reading them over ssh from a different machine than the
//! one they intend to use the server from.
//!
//! So this is a decision table rather than a `println!`, and it lives here
//! rather than in `laplus-server/src/main.rs` because a binary's `main` is the
//! one part of this crate no test runs, and the answers below are the whole
//! product of ticket 03. `Server::reachable_from` was split out of
//! `Server::reachable_addr` for a neighbouring reason — so its arithmetic could
//! be pinned without binding a listener — and between them they leave `main`
//! holding nothing but wiring.
//!
//! ## The rules
//!
//! **The address a phone can reach comes first.** A headless server bound to
//! `0.0.0.0` printed `http://127.0.0.1:4773/#token=…`, which carries the right
//! credential to the wrong host — the one URL that is useless on the device it
//! was printed for. The loopback line stays underneath it, because a developer
//! running this on their own machine still wants it and both are true.
//!
//! **Not finding a LAN address is two different problems.** Bound to loopback,
//! there is nothing to look for and the exposure line has already said so.
//! Bound wide with no route off this machine — a laptop with the Wi-Fi off, a
//! container on an internal network — is a thing to fix, and gets a sentence.
//! Telling those apart in the terminal is cheap; guessing at it from a phone
//! that will not connect is not.
//!
//! **The exposure mode is stated on every start, with where it came from.**
//! `--network` deliberately does not write `remote-access.json`
//! (`docs/adr/0023`), so a flag and a file *can* disagree, and on a box with no
//! Settings panel this line is the only place that shows.

use std::fmt::Write as _;

use crate::launch::Network;
use crate::remote_access::Exposure;

/// A line to print, and which stream it belongs on.
///
/// Two variants rather than a `Vec<String>` the caller sends to stdout, because
/// some of these are degradations and an operator who redirects the ordinary
/// output should still see them. `laplus: ` is prefixed by the caller, since
/// every line these binaries print already carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// Ordinary output: a mode, an address.
    Said(String),
    /// Something is not as it was asked for. Belongs on stderr.
    Warned(String),
}

impl Line {
    /// The text, whichever kind it is.
    pub fn text(&self) -> &str {
        match self {
            Line::Said(text) | Line::Warned(text) => text,
        }
    }
}

/// One place this server answers, as the two URLs for it.
///
/// Both are named because which one to print is a decision and not a fallback:
/// with a credential the operator gets a URL that opens the application, and
/// without one they get a URL that opens the pairing screen, which is a
/// different sentence rather than a shorter one.
///
/// `paired` is `None` for every [`Reachable`] in an announcement or for none of
/// them — there is one boot credential per server and it either minted or did
/// not. They are held per-address anyway because that is the shape
/// [`crate::Server`] answers in, and both come from it
/// ([`crate::Server::url_for`], [`crate::Server::pairing_url_for`]) rather than
/// being spelled out again here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reachable {
    /// The URL with the boot credential in its fragment.
    pub paired: Option<String>,
    /// The same URL without one. Lands on the pairing screen, which takes a
    /// code from anywhere — so this is recoverable rather than a dead end.
    pub plain: String,
}

/// Everything the announcement is built from.
///
/// Taken as data rather than read from a `Server`, so the combinations below
/// can be checked without binding a listener for each — including the two that
/// depend on the machine having, or not having, a route off itself.
#[derive(Debug, Clone)]
pub struct Announcement {
    /// What the server actually bound, after the flag has had its say. Read
    /// back from the configuration the listener was opened with, so this cannot
    /// describe a posture the socket does not have.
    pub exposure: Exposure,
    /// The override that decided it, or `None` when nothing on the way in did.
    pub network: Option<Network>,
    /// [`crate::remote_access::RemoteAccess::is_stored`] — whether, with no
    /// override, a `remote-access.json` decided the mode or the default did.
    /// Naming a file that is not there sends an operator looking for one.
    pub stored: bool,
    /// Loopback. Always present: a server that is listening is reachable from
    /// the machine it is listening on.
    pub local: Reachable,
    /// The address other machines send to, from
    /// [`crate::endpoints::advertised_host`]. `None` when exposure is loopback,
    /// and also when the routing table has no answer.
    pub lan: Option<Reachable>,
    /// Whether [`Announcement::lan`] came from `--advertise-host` rather than
    /// from the routing table.
    ///
    /// It changes one sentence and only when the server stayed on loopback,
    /// which is the single combination where the announcement would otherwise
    /// contradict itself — see
    /// `an_advertised_host_says_what_it_depends_on_when_the_server_stayed_on_loopback`.
    /// The server cannot tell a tunnel from a forgotten `--network`, so it says
    /// what the address depends on instead of guessing.
    pub advertised_by_operator: bool,
    /// The boot credential itself, printed on its own beside a LAN address so
    /// it can be typed into a pairing screen rather than into a URL. `None`
    /// when none was minted, which is also when every `paired` above is `None`.
    pub credential: Option<String>,
}

/// The lines to print, in the order to print them.
pub fn announce(announcement: &Announcement) -> Vec<Line> {
    let mut lines = vec![Line::Said(exposure_line(announcement))];

    match &announcement.lan {
        // The whole point of the ticket: a URL somebody can type into a phone,
        // above the one that only works where it was printed.
        Some(lan) => {
            lines.push(open(lan));
            // The connection string and the code, separately. Upstream's
            // `t3 serve` prints all three (`startupAccess.ts`), and the reason
            // holds harder here: the URL above is being typed by hand into a
            // phone, and `http://192.168.1.42:4773/` followed by twelve
            // characters into the box on the pairing screen is a great deal
            // less to get right than the same thing with `/#token=` in the
            // middle of it. A QR code would beat both and is a dependency;
            // ticket 03 left it as a follow-up.
            if let Some(code) = &announcement.credential {
                lines.push(Line::Said(format!(
                    "or open {} and pair with {code}",
                    lan.plain
                )));
            }
            // A host the operator named while the server sits on loopback is
            // either a tunnel — right, and the reason the flag is honoured
            // whatever the exposure — or a `--network` that was forgotten, in
            // which case the URL above cannot work. Nothing here can tell those
            // apart, so this names the condition rather than the mistake.
            if announcement.advertised_by_operator && !announcement.exposure.is_network_accessible()
            {
                lines.push(Line::Said(
                    "that address reaches this server only through something that \
                     forwards to it — a tunnel does; another machine on your \
                     network needs --network"
                        .to_string(),
                ));
            }
            if let Some(local) = &announcement.local.paired {
                lines.push(Line::Said(format!("on this machine, {local}")));
            }
        }
        None => {
            // Bound to loopback there was nothing to look for, and the line
            // above has already said so — printing a second sentence about it
            // would greet every `cargo run` with a complaint about a mode the
            // developer chose. Bound wide, this is a thing to go and fix.
            if announcement.exposure.is_network_accessible() {
                lines.push(Line::Warned(
                    "no network address was found: this machine has no route off \
                     itself, so nothing but this machine can reach the port it \
                     just opened"
                        .to_string(),
                ));
            }
            lines.push(open(&announcement.local));
        }
    }

    lines
}

/// `open <url>`, or the sentence for a server that has no credential to put in
/// one.
fn open(reachable: &Reachable) -> Line {
    match &reachable.paired {
        Some(url) => Line::Said(format!("open {url}")),
        None => Line::Warned(format!(
            "no boot credential was minted, so a browser opened at {} will ask \
             to be paired",
            reachable.plain
        )),
    }
}

/// `network access is on, from --network` and its siblings.
///
/// The source is named rather than described because the fix differs by source:
/// a flag is in whatever started this process, `LAPLUS_NETWORK` is in the unit
/// file or the shell that exported it, and the file is in the preferences
/// directory. An operator who reads "on" and did not want it needs to know
/// which of the three to go and change.
///
/// **The fourth case is no source at all**, and it is the common one: a fresh
/// box has no `remote-access.json`. Naming the file there would send somebody
/// looking for one that is not on the disk, so that case says "by default"
/// instead — which is also the only one of the four where there is nothing to
/// go and change.
fn exposure_line(announcement: &Announcement) -> String {
    let mut line = String::from("network access is ");
    line.push_str(match announcement.exposure {
        Exposure::NetworkAccessible => "on",
        Exposure::LocalOnly => "off",
    });
    match announcement.network {
        Some(network) => {
            let _ = write!(line, ", from {}", network.source.named());
        }
        None if announcement.stored => line.push_str(", from remote-access.json"),
        None => line.push_str(" by default"),
    }
    line.push_str(match announcement.exposure {
        Exposure::NetworkAccessible => " — this server is on your network",
        Exposure::LocalOnly => " — this server answers this machine only",
    });
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::NetworkSource;

    fn loopback() -> Announcement {
        Announcement {
            exposure: Exposure::LocalOnly,
            network: None,
            stored: true,
            local: Reachable {
                paired: Some("http://127.0.0.1:4773/#token=ABCD2345WXYZ".to_string()),
                plain: "http://127.0.0.1:4773/".to_string(),
            },
            lan: None,
            advertised_by_operator: false,
            credential: Some("ABCD2345WXYZ".to_string()),
        }
    }

    fn lan() -> Reachable {
        Reachable {
            paired: Some("http://192.168.1.42:4773/#token=ABCD2345WXYZ".to_string()),
            plain: "http://192.168.1.42:4773/".to_string(),
        }
    }

    fn from(source: NetworkSource, exposure: Exposure) -> Option<Network> {
        Some(Network { exposure, source })
    }

    fn said(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(Line::text).collect()
    }

    /// An operator-supplied host on a server that never left loopback is the one
    /// combination where the lines above contradict each other: "this server
    /// answers this machine only", and then a URL on some other host.
    ///
    /// **Both are true and the combination is legitimate** — a tunnel forwards a
    /// public hostname to `127.0.0.1`, so a loopback-bound server is exactly
    /// what `cloudflared` wants. It is also what somebody who typed
    /// `--advertise-host` and forgot `--network` has, and for them the URL cannot
    /// work. One sentence tells those apart without guessing which happened,
    /// because the server cannot know.
    #[test]
    fn an_advertised_host_says_what_it_depends_on_when_the_server_stayed_on_loopback() {
        let lines = announce(&Announcement {
            lan: Some(lan()),
            advertised_by_operator: true,
            ..loopback()
        });

        assert!(
            said(&lines)
                .iter()
                .any(|line| line.contains("only through something that forwards")),
            "{:?}",
            said(&lines)
        );

        // And it is not said when the server really is on the network, where the
        // address stands on its own and the sentence would be noise on every
        // start.
        let wide = announce(&Announcement {
            exposure: Exposure::NetworkAccessible,
            network: from(NetworkSource::Flag, Exposure::NetworkAccessible),
            lan: Some(lan()),
            advertised_by_operator: true,
            ..loopback()
        });
        assert!(
            !said(&wide)
                .iter()
                .any(|line| line.contains("only through something that forwards")),
            "{:?}",
            said(&wide)
        );
    }

    /// The acceptance criterion, and the reason the ticket exists: bound wide
    /// on a machine with a route off itself, the *first* URL names the LAN
    /// address and carries the credential in its fragment.
    #[test]
    fn the_address_a_phone_can_reach_is_printed_first() {
        let lines = announce(&Announcement {
            exposure: Exposure::NetworkAccessible,
            network: from(NetworkSource::Flag, Exposure::NetworkAccessible),
            lan: Some(lan()),
            ..loopback()
        });

        assert_eq!(
            said(&lines),
            vec![
                "network access is on, from --network — this server is on your network",
                "open http://192.168.1.42:4773/#token=ABCD2345WXYZ",
                "or open http://192.168.1.42:4773/ and pair with ABCD2345WXYZ",
                "on this machine, http://127.0.0.1:4773/#token=ABCD2345WXYZ",
            ]
        );
        assert!(
            lines.iter().all(|line| matches!(line, Line::Said(_))),
            "nothing went wrong here: {lines:?}"
        );
    }

    /// Bound to loopback, the output is what it was before ticket 03 — one URL
    /// — plus the one line ticket 04 adds. In particular there is no "no
    /// network address" complaint: there was nothing to look for, and a
    /// developer running `cargo run` would read it every single time.
    #[test]
    fn a_loopback_server_says_so_once_and_prints_what_it_always_did() {
        let lines = announce(&loopback());

        assert_eq!(
            said(&lines),
            vec![
                "network access is off, from remote-access.json — this server \
                 answers this machine only",
                "open http://127.0.0.1:4773/#token=ABCD2345WXYZ",
            ]
        );
    }

    /// The other half of "no LAN address", and the one that is a problem: the
    /// operator asked for the network and did not get it. Naming the cause is
    /// the difference between a five-second fix and an evening spent on a
    /// phone's Wi-Fi settings.
    #[test]
    fn a_wide_bind_with_no_route_off_the_machine_says_which_problem_it_is() {
        let lines = announce(&Announcement {
            exposure: Exposure::NetworkAccessible,
            network: from(NetworkSource::Environment, Exposure::NetworkAccessible),
            ..loopback()
        });

        assert_eq!(
            said(&lines),
            vec![
                "network access is on, from LAPLUS_NETWORK — this server is on your network",
                "no network address was found: this machine has no route off \
                 itself, so nothing but this machine can reach the port it just \
                 opened",
                "open http://127.0.0.1:4773/#token=ABCD2345WXYZ",
            ]
        );
        assert!(
            matches!(lines[1], Line::Warned(_)),
            "the operator asked for something they did not get: {lines:?}"
        );
    }

    /// On a box with no window this line is the only place a flag and a file
    /// disagreeing shows at all, so it names which one won.
    ///
    /// The last row is the one a fresh box gets, and the reason it does not say
    /// `remote-access.json`: there is no such file to go and look at.
    #[test]
    fn every_start_says_the_mode_and_where_it_came_from() {
        for (exposure, source, stored, expected) in [
            (
                Exposure::NetworkAccessible,
                None,
                true,
                "network access is on, from remote-access.json",
            ),
            (
                Exposure::LocalOnly,
                Some(NetworkSource::Flag),
                false,
                "network access is off, from --network",
            ),
            (
                Exposure::NetworkAccessible,
                Some(NetworkSource::Environment),
                false,
                "network access is on, from LAPLUS_NETWORK",
            ),
            (
                Exposure::LocalOnly,
                None,
                false,
                "network access is off by default",
            ),
        ] {
            let lines = announce(&Announcement {
                exposure,
                network: source.and_then(|source| from(source, exposure)),
                stored,
                ..loopback()
            });
            assert!(
                lines[0].text().starts_with(expected),
                "{:?} should start with {expected}",
                lines[0]
            );
        }
    }

    /// A credential that could not be minted is survivable — the pairing screen
    /// takes a code from anywhere — but it is not something to print as though
    /// it were an ordinary address, and the URL in it has to be one somebody can
    /// actually type.
    #[test]
    fn a_server_with_no_boot_credential_says_the_page_will_ask_to_be_paired() {
        let unpaired = Announcement {
            local: Reachable {
                paired: None,
                ..loopback().local
            },
            credential: None,
            ..loopback()
        };

        let lines = announce(&unpaired);
        assert!(matches!(lines[1], Line::Warned(_)), "{lines:?}");
        assert!(
            lines[1].text().contains("http://127.0.0.1:4773/"),
            "{lines:?}"
        );

        let networked = announce(&Announcement {
            exposure: Exposure::NetworkAccessible,
            lan: Some(Reachable {
                paired: None,
                ..lan()
            }),
            ..unpaired
        });
        assert!(matches!(networked[1], Line::Warned(_)), "{networked:?}");
        assert!(
            networked[1].text().contains("http://192.168.1.42:4773/"),
            "the address somebody would actually type, port and all: {networked:?}"
        );
    }

    /// Every line is prefixed with `laplus: ` by the caller, so none of them may
    /// arrive carrying their own — and none may be empty, which would print a
    /// bare prefix.
    #[test]
    fn no_line_carries_a_prefix_or_is_blank() {
        for exposure in [Exposure::LocalOnly, Exposure::NetworkAccessible] {
            for reachable in [None, Some(lan())] {
                let lines = announce(&Announcement {
                    exposure,
                    lan: reachable,
                    ..loopback()
                });
                for line in &lines {
                    assert!(!line.text().is_empty(), "{lines:?}");
                    assert!(!line.text().starts_with("laplus:"), "{lines:?}");
                }
            }
        }
    }
}
