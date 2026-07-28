//! Which origins other than this machine may reach this server.
//!
//! Empty by default, and that default is the product's posture rather than a
//! placeholder: laplus binds to loopback and answers the window running beside
//! it. This file exists for the one case that is not that — a developer who has
//! put a tunnel in front of laplus so their phone can reach it, which is what
//! ticket 73 is for.
//!
//! ## Why a file, and why not an environment variable
//!
//! [`crate::config`]'s own note (`config.rs:350-358`) argues against an
//! environment variable for anything the suite touches: `LOCALAPPDATA` is
//! process-global mutable state, so a test that set one would be setting it for
//! every test running beside it. The seam it establishes instead is
//! [`crate::config::ServerConfig::detect_in`], which takes the directory as an
//! argument — and that is what this reads from.
//!
//! The other half of the reason is the user. laplus is opened by
//! double-clicking `laplus.exe`; there is no shell in which to have exported
//! anything. A file next to `settings.json` and `keybindings.json` is somewhere
//! they already are, and is something a Settings panel can later edit. An
//! environment variable would be quicker only for someone launching from a
//! terminal beside `cloudflared`, and two sources for one policy is two places
//! to look when a phone is refused.
//!
//! ## Why a bad file is logged rather than reported to the UI
//!
//! [`crate::config::ConfigIssue`] would be the obvious home for "your
//! remote-access file will not parse", and it is not available.
//! `ServerConfigIssue` in the contract is a **closed union of two
//! `keybindings.*` literals**, so a `kind` of this module's invention would not
//! be an oddly-named row in a list — it would fail the client's decode of the
//! entire `server.getConfig` payload and the application would not open. That
//! is the same wall [`crate::settings`] hit, and this follows the same answer it
//! did: complain to the log, keep the safe default.
//!
//! **The safe default is the empty list.** A file that will not parse admits
//! nothing, so the failure mode of a typo is a phone that cannot connect rather
//! than a machine that admits everybody.

use std::path::Path;

use serde_json::Value;

/// The file, in the preferences directory.
const FILE: &str = "remote-access.json";

/// The hosts this server will accept a request from besides loopback.
///
/// Hosts and not origins, despite the field being called `allowedOrigins` on
/// the wire and in the file: [`crate::auth`] matches on host and ignores the
/// scheme, and this has to agree with it or the two would disagree about what
/// an origin is. See [`RemoteAccess::allows`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteAccess {
    hosts: Vec<String>,
}

impl RemoteAccess {
    /// Loopback only. What every test that is not about this gets, and what a
    /// machine with no such file gets.
    pub fn none() -> RemoteAccess {
        RemoteAccess::default()
    }

    /// Read the file, or answer [`RemoteAccess::none`] and say why.
    pub fn load(directory: &Path) -> RemoteAccess {
        let path = directory.join(FILE);
        let complain = |detail: &str| {
            eprintln!(
                "laplus: the remote access file at {} was not used: {detail} \
                 No origin beyond this machine will be admitted.",
                path.display()
            );
        };

        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            // Nothing written, which is every machine that has not put a tunnel
            // in front of laplus — so this is the ordinary case and not a
            // problem to report.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return RemoteAccess::none()
            }
            Err(error) => {
                complain(&format!("it could not be read: {error}."));
                return RemoteAccess::none();
            }
        };

        let stored: Value = match serde_json::from_str(&raw) {
            Ok(stored) => stored,
            Err(error) => {
                complain(&format!("it is not valid JSON: {error}."));
                return RemoteAccess::none();
            }
        };

        let Some(entries) = stored.get("allowedOrigins") else {
            complain("it has no `allowedOrigins` array.");
            return RemoteAccess::none();
        };
        let Some(entries) = entries.as_array() else {
            complain("`allowedOrigins` is not an array.");
            return RemoteAccess::none();
        };

        let mut hosts: Vec<String> = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(entry) = entry.as_str() else {
                complain("`allowedOrigins` holds something that is not a string.");
                return RemoteAccess::none();
            };
            match host_of(entry) {
                // One bad entry does not discard the rest. Unlike the cases
                // above, the file's *shape* is understood here — this is one
                // line the user got wrong, and refusing the others would turn a
                // typo into "nothing works" with no way to tell which line did
                // it.
                None => complain(&format!("`{entry}` is not a host or an origin; skipping it.")),
                Some(host) if hosts.contains(&host) => {}
                Some(host) => hosts.push(host),
            }
        }

        if hosts.is_empty() {
            complain("it admits no hosts.");
        }
        RemoteAccess { hosts }
    }

    /// May a page served from this host reach this server?
    ///
    /// **Host, not origin**, so `https://phone.example` and
    /// `http://phone.example` are the same answer — matching how
    /// [`crate::auth`] has always treated loopback, where the scheme is ignored
    /// because `tauri://localhost` is as local as `http://localhost`. Being
    /// stricter here would mean a user who wrote a bare hostname was silently
    /// admitting nothing, and a user who wrote `https://…` was refused the day
    /// their tunnel served plain HTTP.
    ///
    /// What that costs is worth saying out loud: somebody who can make the
    /// browser resolve this hostname to a server they control — DNS control, or
    /// a machine on the same network answering for it — is admitted. That is a
    /// far larger foothold than this check, and the credential behind it still
    /// has to verify.
    pub fn allows(&self, host: &str) -> bool {
        let host = host.to_ascii_lowercase();
        self.hosts.contains(&host)
    }

    /// The hosts, for a log line at startup. Not on the wire: the contract has
    /// no field for it, and `tests/socket_conformance.rs` would report an
    /// addition.
    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }

    /// Build one directly. For the suite, and for a caller assembling a config
    /// without a directory to read from.
    pub fn from_hosts<I, S>(entries: I) -> RemoteAccess
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        RemoteAccess {
            hosts: entries
                .into_iter()
                .filter_map(|entry| host_of(entry.as_ref()))
                .collect(),
        }
    }
}

/// The host part of whatever the user wrote.
///
/// Deliberately forgiving about the shape, because the field is called
/// `allowedOrigins` and a person writing one will reasonably produce any of
/// `phone.example`, `https://phone.example`, or `https://phone.example:8443`.
/// All three name one host and all three are meant the same way.
///
/// `None` for something with no host in it at all, which is the only case worth
/// telling the user about.
fn host_of(entry: &str) -> Option<String> {
    let entry = entry.trim();
    if entry.is_empty() {
        return None;
    }

    let after_scheme = match entry.split_once("://") {
        Some((scheme, rest)) if !scheme.is_empty() => rest,
        Some(_) => return None,
        None => entry,
    };
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or_default();
    // Anything with credentials in it — `user@host` — keeps the host.
    let authority = authority.rsplit('@').next().unwrap_or_default();

    let host = match authority.strip_prefix('[') {
        // IPv6 literal: the port, if any, follows the closing bracket.
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };

    if host.is_empty() || host.contains(' ') {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(directory: &Path, contents: &str) {
        std::fs::write(directory.join(FILE), contents).expect("writes the file");
    }

    fn temporary() -> tempfile::TempDir {
        tempfile::tempdir().expect("a temporary directory")
    }

    /// The ordinary machine: no file, and nothing beyond loopback admitted.
    #[test]
    fn a_machine_with_no_file_admits_nothing_beyond_loopback() {
        let directory = temporary();
        let access = RemoteAccess::load(directory.path());
        assert!(access.is_empty());
        assert!(!access.allows("phone.example"));
    }

    #[test]
    fn a_named_host_is_admitted() {
        let directory = temporary();
        written(directory.path(), r#"{ "allowedOrigins": ["phone.example"] }"#);

        let access = RemoteAccess::load(directory.path());
        assert!(access.allows("phone.example"));
        assert!(!access.allows("other.example"));
    }

    /// Whatever shape the user wrote, it names one host.
    #[test]
    fn a_host_is_read_out_of_whatever_the_user_wrote() {
        for written_as in [
            "phone.example",
            "https://phone.example",
            "http://phone.example",
            "https://phone.example:8443",
            "https://phone.example/",
            "  https://PHONE.example  ",
        ] {
            assert_eq!(
                host_of(written_as),
                Some("phone.example".to_string()),
                "{written_as}"
            );
        }
        assert_eq!(host_of("http://[2001:db8::1]:8443"), Some("2001:db8::1".to_string()));
    }

    #[test]
    fn the_match_is_case_insensitive_because_hostnames_are() {
        let access = RemoteAccess::from_hosts(["Phone.Example"]);
        assert!(access.allows("phone.example"));
        assert!(access.allows("PHONE.EXAMPLE"));
    }

    /// A subdomain is a different host and is not admitted. Naming
    /// `phone.example` must not hand over `evil.phone.example`.
    #[test]
    fn a_named_host_does_not_admit_anything_underneath_it() {
        let access = RemoteAccess::from_hosts(["phone.example"]);
        assert!(!access.allows("evil.phone.example"));
        assert!(!access.allows("phone.example.evil"));
        assert!(!access.allows("phone-example"));
    }

    /// The failure mode of a typo has to be a phone that cannot connect, never
    /// a machine that admits everybody.
    #[test]
    fn a_file_that_will_not_parse_admits_nothing() {
        for contents in [
            "not json",
            "[]",
            r#"{ "allowedOrigins": "phone.example" }"#,
            r#"{ "allowed": ["phone.example"] }"#,
            r#"{ "allowedOrigins": [7] }"#,
        ] {
            let directory = temporary();
            written(directory.path(), contents);
            let access = RemoteAccess::load(directory.path());
            assert!(access.is_empty(), "{contents}");
            assert!(!access.allows("phone.example"), "{contents}");
        }
    }

    /// One unusable line does not discard the others. The user gets told which
    /// line, and the rest of the file still works.
    #[test]
    fn one_bad_entry_does_not_discard_the_rest() {
        let directory = temporary();
        written(
            directory.path(),
            r#"{ "allowedOrigins": ["phone.example", "", "https://", "tablet.example"] }"#,
        );

        let access = RemoteAccess::load(directory.path());
        assert!(access.allows("phone.example"));
        assert!(access.allows("tablet.example"));
        assert_eq!(access.hosts().len(), 2);
    }

    #[test]
    fn a_host_named_twice_is_held_once() {
        let directory = temporary();
        written(
            directory.path(),
            r#"{ "allowedOrigins": ["phone.example", "https://phone.example:8443"] }"#,
        );
        assert_eq!(RemoteAccess::load(directory.path()).hosts().len(), 1);
    }

    /// Loopback is [`crate::auth`]'s business and never this file's. Naming it
    /// here is harmless and changes nothing, which is worth pinning so that
    /// nobody moves the loopback rule in here later and quietly makes an empty
    /// file lock the window out.
    #[test]
    fn admitting_loopback_is_not_this_files_job() {
        let access = RemoteAccess::none();
        assert!(!access.allows("127.0.0.1"));
        assert!(!access.allows("localhost"));
    }
}
