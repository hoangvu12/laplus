//! Where this server listens.
//!
//! Loopback by default, and that default is the product's posture rather than a
//! placeholder: laplus answers the window running beside it. This file exists
//! for the one case that is not that — somebody who wants a phone to reach it,
//! which is what ticket 73 is for.
//!
//! ## What used to be here
//!
//! An `allowedOrigins` list, and an origin check in [`crate::auth`] that refused
//! anything not on it. Upstream has no such list: its only origin handling is
//! CORS behind `devOrigin`, for development
//! (`pingdotgg/t3code:apps/server/src/http.ts:57-67`). The list is gone and this
//! follows upstream, so what a request is allowed to do is settled by its
//! credential and nothing else.
//!
//! A file left over from before that change still has the field in it. It is
//! read by nothing and is not an error to find — see [`RemoteAccess::load`].
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
//! **The safe default is loopback.** A file that will not parse leaves this
//! server on this machine, so the failure mode of a typo is a phone that cannot
//! connect rather than a port open to the network.

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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteAccess {
    exposure: Exposure,
}

/// Is a bound address one that something other than this machine can reach?
///
/// A port of upstream's `isRemoteReachableHost` (`auth/utils.ts:53-67`), and
/// the whole of what settles [`crate::config::ServerAuthDescriptor::policy`]
/// there and here. Upstream takes a host string because it has one to take;
/// this takes the address actually bound, which is the same question with no
/// parsing in between.
pub fn is_remote_reachable_host(host: std::net::Ipv4Addr) -> bool {
    if host.is_unspecified() {
        return true;
    }
    !host.is_loopback()
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

    /// The same, with the mode changed. What the switch writes.
    pub fn with_exposure(&self, exposure: Exposure) -> RemoteAccess {
        RemoteAccess { exposure }
    }

    /// Write it back, for the switch in Settings that changes it.
    ///
    /// Written to a temporary and renamed, so a crash between `write` and
    /// `close` cannot leave a half-written file that [`RemoteAccess::load`]
    /// would then refuse — and refusing this file means falling back to
    /// loopback, which for a user who is mid-way through turning the switch
    /// *on* would look exactly like the feature not working.
    pub fn save(&self, directory: &Path) -> std::io::Result<()> {
        let body = serde_json::to_string_pretty(&serde_json::json!({
            "mode": self.exposure.mode(),
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
                 This server will listen on this machine only.",
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

        // `allowedOrigins` is read by nothing now and is not an error to find:
        // it is what every file written before this change has in it, and a
        // machine that upgrades should keep working rather than be told off
        // for the contents of a file it did not write by hand.
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

        RemoteAccess { exposure }
    }

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

    /// The ordinary machine: no file, and this server answers itself only.
    #[test]
    fn a_machine_with_no_file_stays_on_this_machine() {
        let directory = temporary();
        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::LocalOnly);
        assert_eq!(access.bind_address(), std::net::Ipv4Addr::LOCALHOST);
    }

    /// Every file written before the switch existed has only `allowedOrigins`,
    /// and those machines were bound to loopback. Reading one must not now move
    /// them — nor complain, because the user did not write that file by hand.
    #[test]
    fn a_file_from_before_the_switch_stays_on_this_machine() {
        let directory = temporary();
        written(directory.path(), r#"{ "allowedOrigins": ["phone.example"] }"#);

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::LocalOnly);
    }

    /// The field this file used to carry is now read by nothing, and a machine
    /// that upgrades with one still in place keeps the mode it had.
    #[test]
    fn a_leftover_host_list_no_longer_decides_anything() {
        let directory = temporary();
        written(
            directory.path(),
            r#"{ "mode": "network-accessible", "allowedOrigins": "not-an-array" }"#,
        );

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::NetworkAccessible);
    }

    #[test]
    fn turning_the_switch_on_binds_every_interface() {
        let directory = temporary();
        written(directory.path(), r#"{ "mode": "network-accessible" }"#);

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::NetworkAccessible);
        assert_eq!(access.bind_address(), std::net::Ipv4Addr::UNSPECIFIED);
    }

    /// A mode this server does not know is the one case that must fall *back*
    /// rather than through: an unreadable switch position admitting the network
    /// would be a typo opening the machine up.
    #[test]
    fn a_mode_this_server_does_not_know_stays_on_this_machine() {
        let directory = temporary();
        written(directory.path(), r#"{ "mode": "everyone-welcome" }"#);

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::LocalOnly);
    }

    #[test]
    fn a_file_that_will_not_parse_stays_on_this_machine() {
        let directory = temporary();
        written(directory.path(), "{ not json");

        let access = RemoteAccess::load(directory.path());
        assert_eq!(access.exposure(), Exposure::LocalOnly);
    }

    /// What the switch in Settings writes, read back through the same loader.
    #[test]
    fn what_settings_writes_is_what_the_next_start_reads() {
        let directory = temporary();

        RemoteAccess::none()
            .with_exposure(Exposure::NetworkAccessible)
            .save(directory.path())
            .expect("writes");

        let reloaded = RemoteAccess::load(directory.path());
        assert_eq!(reloaded.exposure(), Exposure::NetworkAccessible);

        reloaded
            .with_exposure(Exposure::LocalOnly)
            .save(directory.path())
            .expect("writes");
        assert_eq!(
            RemoteAccess::load(directory.path()).exposure(),
            Exposure::LocalOnly
        );
    }

    /// The predicate the auth policy is settled from, against the two addresses
    /// [`RemoteAccess::bind_address`] can actually answer.
    #[test]
    fn only_the_unspecified_address_is_reachable_from_elsewhere() {
        assert!(is_remote_reachable_host(std::net::Ipv4Addr::UNSPECIFIED));
        assert!(!is_remote_reachable_host(std::net::Ipv4Addr::LOCALHOST));
        // The whole 127.0.0.0/8 block, matching upstream's `host.startsWith("127.")`.
        assert!(!is_remote_reachable_host(std::net::Ipv4Addr::new(
            127, 0, 0, 2
        )));
        assert!(is_remote_reachable_host(std::net::Ipv4Addr::new(
            192, 168, 1, 4
        )));
    }

    /// The bug this replaced: the policy used to be settled from the tunnel
    /// hostname list, which the user edits while the server runs, so it went
    /// stale the moment they added one. It is settled from the bound address
    /// now, and a bound address cannot move under a running server.
    #[test]
    fn the_switch_decides_what_the_policy_reports() {
        let local = RemoteAccess::none();
        assert!(!is_remote_reachable_host(local.bind_address()));

        let networked = local.with_exposure(Exposure::NetworkAccessible);
        assert!(is_remote_reachable_host(networked.bind_address()));
    }
}
