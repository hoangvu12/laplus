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

/// Where this server listens.
///
/// Upstream's `DesktopServerExposureMode`, and the same two values, because the
/// switch in Settings is upstream's switch and reports what it set. The strings
/// are upstream's too — they travel to the page as they are written here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Exposure {
    /// `127.0.0.1`. The default, and what every machine gets until somebody
    /// turns the switch on.
    #[default]
    LocalOnly,
    /// `0.0.0.0` — every interface, so a phone on the same network can reach
    /// this machine without a tunnel in front of it.
    NetworkAccessible,
}

impl Exposure {
    pub fn mode(self) -> &'static str {
        match self {
            Exposure::LocalOnly => "local-only",
            Exposure::NetworkAccessible => "network-accessible",
        }
    }

    fn from_mode(mode: &str) -> Option<Exposure> {
        match mode {
            "local-only" => Some(Exposure::LocalOnly),
            "network-accessible" => Some(Exposure::NetworkAccessible),
            _ => None,
        }
    }

    pub fn is_network_accessible(self) -> bool {
        matches!(self, Exposure::NetworkAccessible)
    }
}

/// Who may reach this server, and from where.
///
/// Two questions that used to be one. Until the switch existed this file held
/// only `allowedOrigins`, because a tunnel was the only way in and naming its
/// hostname was the only decision — see this module's header, which is still
/// the reasoning for that half. `mode` is the other half and the reason both
/// live here rather than in `settings.json`: `ServerSettings` is the contract's
/// and closed, and a field of laplus's invention in it fails the client's
/// decode of the whole payload.
///
/// **They do not replace each other.** Binding to every interface answers the
/// LAN and says nothing about a `trycloudflare.com` hostname, which resolves to
/// Cloudflare and arrives here as an `Origin` this machine has never heard of.
/// Upstream keeps both for the same reason.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteAccess {
    hosts: Vec<String>,
    exposure: Exposure,
}

impl RemoteAccess {
    /// Loopback only. What every test that is not about this gets, and what a
    /// machine with no such file gets.
    pub fn none() -> RemoteAccess {
        RemoteAccess::default()
    }

    /// Where this server should listen.
    pub fn exposure(&self) -> Exposure {
        self.exposure
    }

    /// The address to bind, which is the whole of what [`Exposure`] decides.
    ///
    /// `0.0.0.0` is a real change of posture and not a wider default: until the
    /// switch is turned on this answers `127.0.0.1`, and `docs/adr/0022` is why
    /// the switch is allowed to exist at all now that a credential is verified.
    pub fn bind_address(&self) -> std::net::Ipv4Addr {
        match self.exposure {
            Exposure::LocalOnly => std::net::Ipv4Addr::LOCALHOST,
            Exposure::NetworkAccessible => std::net::Ipv4Addr::UNSPECIFIED,
        }
    }

    /// The same list, with the mode changed. What the switch writes.
    pub fn with_exposure(&self, exposure: Exposure) -> RemoteAccess {
        RemoteAccess {
            hosts: self.hosts.clone(),
            exposure,
        }
    }

    /// The same mode, with the tunnel hostnames changed. What the list writes.
    pub fn with_hosts<I, S>(&self, entries: I) -> RemoteAccess
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        RemoteAccess {
            exposure: self.exposure,
            ..RemoteAccess::from_hosts(entries)
        }
    }

    /// Write it back, for the two controls in Settings that change it.
    ///
    /// Whole-file rather than a patch: there are two fields and the caller has
    /// just read both. Written to a temporary and renamed, so a crash between
    /// `write` and `close` cannot leave a half-written file that
    /// [`RemoteAccess::load`] would then refuse — and refusing this file means
    /// admitting nobody, which for a user who is mid-way through turning
    /// pairing *on* would look exactly like the feature not working.
    pub fn save(&self, directory: &Path) -> std::io::Result<()> {
        let body = serde_json::to_string_pretty(&serde_json::json!({
            "mode": self.exposure.mode(),
            "allowedOrigins": self.hosts,
        }))
        .expect("the remote access file serializes");

        std::fs::create_dir_all(directory)?;
        let destination = directory.join(FILE);
        let temporary = directory.join(format!("{FILE}.writing"));
        std::fs::write(&temporary, body)?;
        std::fs::rename(&temporary, &destination)
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

        // Read before the hosts, and kept whatever happens to them. The switch
        // and the tunnel list are independent controls over one file, so a
        // machine bound to every interface must not fall back to loopback
        // because somebody mistyped a hostname underneath it — that would be
        // one control silently undoing the other.
        let exposure = match stored.get("mode") {
            // Absent is the ordinary case, not a problem: every file written
            // before the switch existed has only `allowedOrigins`, and
            // loopback is what those machines were already doing.
            None => Exposure::default(),
            Some(mode) => match mode.as_str().and_then(Exposure::from_mode) {
                Some(exposure) => exposure,
                None => {
                    complain(&format!(
                        "`mode` is {mode}, which is neither `local-only` nor \
                         `network-accessible`; staying on this machine."
                    ));
                    Exposure::default()
                }
            },
        };

        // From here a bad `allowedOrigins` costs the hosts and not the mode.
        let loopback_only = |()| RemoteAccess {
            hosts: Vec::new(),
            exposure,
        };

        let Some(entries) = stored.get("allowedOrigins") else {
            // Not a complaint any more: a file that only turns the switch on is
            // a complete file now, and the common one.
            return loopback_only(());
        };
        let Some(entries) = entries.as_array() else {
            complain("`allowedOrigins` is not an array.");
            return loopback_only(());
        };

        let mut hosts: Vec<String> = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(entry) = entry.as_str() else {
                complain("`allowedOrigins` holds something that is not a string.");
                return loopback_only(());
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

        // Only worth saying when the file exists to name hosts and names none.
        // A file that is here to turn the switch on has nothing missing.
        if hosts.is_empty() && !exposure.is_network_accessible() {
            complain("it admits no hosts.");
        }
        RemoteAccess { hosts, exposure }
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
        if self.hosts.contains(&host) {
            return true;
        }

        // A phone on the same network loads the page from this machine's own
        // LAN address and then opens a socket carrying it as the `Origin`. That
        // origin is not loopback and nobody typed it into the tunnel list, so
        // without this the switch would bind the port, serve the page, and
        // refuse the socket — which is precisely the failure ticket 73 was
        // reported for, moved one address along.
        //
        // Narrow on purpose: it admits *this machine's own address*, not the
        // subnet. A page served from another machine on the network is still a
        // page this server has never heard of, and the credential check behind
        // this still has to pass either way.
        self.exposure.is_network_accessible()
            && crate::endpoints::lan_address()
                .is_some_and(|address| address.to_string() == host)
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
            exposure: Exposure::default(),
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
        assert_eq!(access.exposure(), Exposure::LocalOnly);
        assert_eq!(access.bind_address(), std::net::Ipv4Addr::LOCALHOST);
    }

    /// Every file written before the switch existed has only `allowedOrigins`,
    /// and those machines were bound to loopback. Reading one must not now move
    /// them.
    #[test]
    fn a_file_from_before_the_switch_stays_on_this_machine() {
        let directory = temporary();
        written(directory.path(), r#"{ "allowedOrigins": ["phone.example"] }"#);

        let access = RemoteAccess::load(directory.path());
        assert!(access.allows("phone.example"));
        assert_eq!(access.exposure(), Exposure::LocalOnly);
    }

    #[test]
    fn turning_the_switch_on_binds_every_interface() {
        let directory = temporary();
        written(directory.path(), r#"{ "mode": "network-accessible" }"#);

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::NetworkAccessible);
        assert_eq!(access.bind_address(), std::net::Ipv4Addr::UNSPECIFIED);
        // No `allowedOrigins` at all, and that is a complete file now.
        assert!(access.is_empty());
    }

    /// The two controls are independent, and this is the failure that would
    /// make them not be: a mistyped hostname must not quietly pull the machine
    /// back to loopback while the switch still reads as on.
    #[test]
    fn a_bad_host_list_does_not_take_the_bind_address_with_it() {
        let directory = temporary();
        written(
            directory.path(),
            r#"{ "mode": "network-accessible", "allowedOrigins": "not-an-array" }"#,
        );

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::NetworkAccessible);
        assert!(access.is_empty());
    }

    /// A mode this server does not know is the one case that must fall *back*
    /// rather than through: an unreadable switch position admitting the network
    /// would be a typo opening the machine up.
    #[test]
    fn a_mode_this_server_does_not_know_stays_on_this_machine() {
        let directory = temporary();
        written(
            directory.path(),
            r#"{ "mode": "everyone-welcome", "allowedOrigins": ["phone.example"] }"#,
        );

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::LocalOnly);
        assert!(access.allows("phone.example"), "the hosts still stand");
    }

    /// What the two controls in Settings write, read back through the same
    /// loader. Both halves at once, because they share a file and the whole
    /// risk of writing it whole is that one control clears the other.
    #[test]
    fn what_settings_writes_is_what_the_next_start_reads() {
        let directory = temporary();

        RemoteAccess::from_hosts(["phone.example"])
            .with_exposure(Exposure::NetworkAccessible)
            .save(directory.path())
            .expect("writes");

        let reloaded = RemoteAccess::load(directory.path());
        assert_eq!(reloaded.exposure(), Exposure::NetworkAccessible);
        assert!(reloaded.allows("phone.example"));

        // Turning the switch back off keeps the tunnel hostname, which is the
        // point of `with_exposure` taking the list along with it.
        reloaded
            .with_exposure(Exposure::LocalOnly)
            .save(directory.path())
            .expect("writes");
        let again = RemoteAccess::load(directory.path());
        assert_eq!(again.exposure(), Exposure::LocalOnly);
        assert!(again.allows("phone.example"));
    }

    /// The switch admits this machine's own LAN address and nothing else on the
    /// network.
    ///
    /// Skipped on a machine with no route off itself, because there is then no
    /// address to be admitted and nothing to assert — the suite has to pass on
    /// a laptop with the Wi-Fi off.
    #[test]
    fn the_switch_admits_this_machines_own_address_and_not_its_neighbours() {
        let Some(own) = crate::endpoints::lan_address() else {
            return;
        };
        let networked = RemoteAccess::none().with_exposure(Exposure::NetworkAccessible);

        assert!(networked.allows(&own.to_string()));

        // A different address on the same network is still a stranger.
        let octets = own.octets();
        let neighbour = std::net::Ipv4Addr::new(
            octets[0],
            octets[1],
            octets[2],
            octets[3].wrapping_add(1).max(1),
        );
        if neighbour != own {
            assert!(!networked.allows(&neighbour.to_string()));
        }

        // And it is the switch that admits it, not the address being local.
        assert!(!RemoteAccess::none().allows(&own.to_string()));
    }

    /// And the other way: editing the list must not move the switch.
    #[test]
    fn changing_the_tunnel_list_leaves_the_switch_where_it_was() {
        let directory = temporary();
        RemoteAccess::none()
            .with_exposure(Exposure::NetworkAccessible)
            .with_hosts(["tunnel.example", "https://phone.example:8443"])
            .save(directory.path())
            .expect("writes");

        let reloaded = RemoteAccess::load(directory.path());
        assert_eq!(reloaded.exposure(), Exposure::NetworkAccessible);
        assert!(reloaded.allows("tunnel.example"));
        assert!(reloaded.allows("phone.example"));
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
