//! What a laplus process was asked for on the way in: a port, a bundle, and
//! whether to leave loopback.
//!
//! Shared by the two binaries — the plain server and the desktop shell — because
//! they have to agree: a developer who starts the shell and then points a browser
//! at it is looking at the same address either way.
//!
//! **The default is fixed, and that is the decision.** Asking the operating
//! system for a free port would be the more obliging thing for a desktop app to
//! do, and it is wrong here: since ticket 23 the window is pointed at
//! `http://127.0.0.1:<port>/`, so the port is part of the page's **origin**, and
//! the UI keeps the developer's layout, drafts, sidebar state and last-open
//! thread in `localStorage` — which browsers scope per origin. An ephemeral port
//! would hand the app a different origin on every launch and quietly lose all of
//! it, every time, with nothing in any log to say why. A port already in use is
//! a loud failure at startup; the alternative is a silent one the developer
//! would blame on the app forgetting things.
//!
//! ## `--network` decides this run and nothing after it
//!
//! The switch in Settings owns `remote-access.json`. `--network` does **not**
//! write to it: it overrides the exposure for the process it was given to, and
//! the next start reads the file again as though the flag had never been passed.
//!
//! One `laplus-server --network` run that rewrote that file would silently
//! change what the *desktop application* does on its next launch, from a
//! terminal on a box the user may not even be sitting at. Overriding one process
//! is the smaller claim, and it is the one a service unit wants — the unit file
//! is the record of what that service does, and it should not have to be
//! reconciled against a file it wrote the first time it started.
//!
//! `docs/adr/0023` is the long version, including why the flag can turn exposure
//! *off* as well as on.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::remote_access::Exposure;

/// The port laplus listens on unless told otherwise.
///
/// Not upstream's 3773, so a reference server and laplus can run side by side
/// while the port is still being compared against captures.
pub const DEFAULT_PORT: u16 = 4773;

/// The port this process was asked for: `--port <n>` or `--port=<n>` on the
/// command line, else `LAPLUS_PORT`, else [`DEFAULT_PORT`].
///
/// Port 0 asks the operating system for a free one. The tests use it, and so may
/// a developer running two builds at once — with the cost above understood.
///
/// **The shell's entry point, and it takes no other flag.** `laplus-server` uses
/// [`requested`] instead: it accepts `--ui`, which the shell has no use for
/// because its bundle is compiled in. Two entry points rather than one shared
/// parser that ignores what it does not know, because a shell that silently
/// disregarded `--ui` would be a shell the developer thinks is serving a
/// directory.
pub fn requested_port() -> Result<u16, String> {
    let flags = flags_from(std::env::args().skip(1), &["port"], &[])?;
    port_in(&flags, std::env::var("LAPLUS_PORT").ok())
}

/// Everything `laplus-server` was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requested {
    pub port: u16,
    /// The UI bundle to serve, if this server was pointed at one. `None` is a
    /// server that answers calls and no pages — what the plain binary was
    /// before ticket 01 of the headless-Linux effort, and still is by default.
    pub ui: Option<PathBuf>,
    /// The exposure this run was told to use, and which of the two places said
    /// so. `None` is neither of them saying anything, which leaves
    /// `remote-access.json` in charge — see this module's header.
    pub network: Option<Network>,
    /// The host to print in the startup URLs, when the operator knows one the
    /// server cannot discover. `None` leaves the question to
    /// [`crate::endpoints::advertised_host`], which is right on a LAN and
    /// cannot be right everywhere — see this module's header.
    pub advertise_host: Option<String>,
}

/// An exposure the command line or the environment insisted on, and which.
///
/// The two travel together because the startup line has to name the source:
/// a flag and a file disagreeing is otherwise invisible on a box with no
/// window, and "the switch is on but the server is not listening for you" is
/// the single most confusing state this feature has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    pub exposure: Exposure,
    pub source: NetworkSource,
}

/// Which of the two overrides was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkSource {
    Flag,
    Environment,
}

impl NetworkSource {
    /// What to call it in a sentence, spelled the way the operator typed it.
    pub fn named(self) -> &'static str {
        match self {
            NetworkSource::Flag => "--network",
            NetworkSource::Environment => "LAPLUS_NETWORK",
        }
    }
}

/// What the plain server was asked for, from the command line and then the
/// environment.
pub fn requested() -> Result<Requested, String> {
    requested_from(std::env::args().skip(1))
}

/// [`requested`], from arguments somebody else has already peeled a verb off.
fn requested_from(arguments: impl Iterator<Item = String>) -> Result<Requested, String> {
    let flags = flags_from(arguments, &["port", "ui", "advertise-host"], &["network"])?;
    Ok(Requested {
        port: port_in(&flags, std::env::var("LAPLUS_PORT").ok())?,
        ui: ui_in(&flags, std::env::var("LAPLUS_UI").ok()),
        network: network_in(&flags, std::env::var("LAPLUS_NETWORK").ok())?,
        advertise_host: advertise_host_in(&flags, std::env::var("LAPLUS_ADVERTISE_HOST").ok())?,
    })
}

/// What this process was asked to *do*, which is nearly always "be a server".
///
/// **One verb, and it is not a general command line.** `service` is here because
/// installing a background service needs the same flags a run does — the unit it
/// writes has to start the server the way the operator would have — and putting
/// it anywhere else means a second parser that has to agree with this one about
/// what `--network` means. Everything without a leading verb is a run, so the
/// spelling that has always worked still does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invoked {
    Serve(Requested),
    /// `service <verb> [flags…]`. The flags are what the unit will carry.
    Service {
        verb: crate::service::Verb,
        requested: Requested,
        /// The flags exactly as typed, to write into `ExecStart`. Rebuilding
        /// them from [`Requested`] would bake this run's *defaults* into the
        /// unit — a port nobody chose and an exposure that came from
        /// `remote-access.json` rather than from the command line.
        arguments: Vec<String>,
    },
}

/// Read the command line as a verb and its flags.
pub fn invoked() -> Result<Invoked, String> {
    invoked_from(std::env::args().skip(1).collect())
}

fn invoked_from(arguments: Vec<String>) -> Result<Invoked, String> {
    let Some(first) = arguments.first() else {
        return Ok(Invoked::Serve(requested_from(arguments.into_iter())?));
    };
    if first != "service" {
        return Ok(Invoked::Serve(requested_from(arguments.into_iter())?));
    }
    let verb = arguments
        .get(1)
        .ok_or_else(|| "service needs a command — install, status or uninstall".to_string())?;
    let verb = crate::service::Verb::parse(verb)?;
    let rest: Vec<String> = arguments.into_iter().skip(2).collect();
    Ok(Invoked::Service {
        verb,
        requested: requested_from(rest.clone().into_iter())?,
        arguments: rest,
    })
}

/// The flags this process was given, by name and without the `--`.
///
/// `--name value` and `--name=value` are the same thing, which is what every
/// other tool does. An unknown flag is a refusal rather than something to skip:
/// a typo that started a server with the default it was told not to use is the
/// silent failure [`DEFAULT_PORT`]'s note is about.
///
/// `switches` are the flags that mean something on their own — `--network` is
/// the only one, and it records `true` when it appears bare. They still accept
/// `--network=false`, because a run that has to override an environment
/// variable *downwards* has no other spelling; what they do not accept is
/// `--network false`, which would make `--network --port 4773` eat its
/// neighbour and start a server on the default port.
fn flags_from(
    mut arguments: impl Iterator<Item = String>,
    valued: &[&str],
    switches: &[&str],
) -> Result<BTreeMap<String, String>, String> {
    let mut given = BTreeMap::new();
    while let Some(argument) = arguments.next() {
        let (name, attached) = match argument.split_once('=') {
            Some((name, value)) => (name.to_string(), Some(value.to_string())),
            None => (argument, None),
        };
        let flag = name
            .strip_prefix("--")
            .filter(|flag| valued.contains(flag) || switches.contains(flag))
            .ok_or_else(|| format!("unrecognised argument {name}"))?
            .to_string();
        let value = match attached {
            Some(value) => value,
            None if switches.contains(&flag.as_str()) => "true".to_string(),
            None => arguments
                .next()
                .ok_or_else(|| format!("{name} needs a value"))?,
        };
        if given.insert(flag.clone(), value).is_some() {
            return Err(format!("--{flag} was given more than once"));
        }
    }
    Ok(given)
}

fn port_in(
    flags: &BTreeMap<String, String>,
    environment: Option<String>,
) -> Result<u16, String> {
    match flags.get("port") {
        Some(value) => value
            .parse()
            .map_err(|_| format!("{value} is not a port number")),
        None => match environment {
            Some(value) => value
                .parse()
                .map_err(|_| format!("LAPLUS_PORT={value} is not a port number")),
            None => Ok(DEFAULT_PORT),
        },
    }
}

/// The bundle directory, argument before environment, absent if neither said.
///
/// **No validation here**, unlike the port: whether the path names a bundle is
/// a question for the filesystem, and [`crate::ui::Assets::from_directory`]
/// answers it with an error that says which path it tried. Checking twice would
/// mean two different sentences for the same mistake.
fn ui_in(flags: &BTreeMap<String, String>, environment: Option<String>) -> Option<PathBuf> {
    flags
        .get("ui")
        .cloned()
        .or(environment)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Whether this run was told to leave loopback, argument before environment.
///
/// **Validated, unlike the bundle path**, for the reason [`port_in`] is: a
/// misread value here does not fail later somewhere legible, it silently picks
/// one of the two answers. `LAPLUS_NETWORK=flase` falling back to loopback is a
/// phone that cannot connect and a log that says nothing about why; the same
/// typo meaning "on" would be worse.
fn network_in(
    flags: &BTreeMap<String, String>,
    environment: Option<String>,
) -> Result<Option<Network>, String> {
    let (raw, source) = match flags.get("network") {
        Some(value) => (value.clone(), NetworkSource::Flag),
        // A blank one is nothing asked for rather than a value to refuse, which
        // is [`ui_in`]'s rule and the same reason: an exported `LAPLUS_NETWORK=`
        // is a common way to mean "not set".
        None => match environment.filter(|value| !value.trim().is_empty()) {
            Some(value) => (value, NetworkSource::Environment),
            None => return Ok(None),
        },
    };

    let exposure = exposure_from(&raw).ok_or_else(|| match source {
        NetworkSource::Flag => format!("{raw} is not on or off"),
        NetworkSource::Environment => format!("LAPLUS_NETWORK={raw} is not on or off"),
    })?;
    Ok(Some(Network { exposure, source }))
}

/// The host the startup URLs should name, argument before environment.
///
/// **For the box whose own address is not the one anybody reaches it at.**
/// [`crate::endpoints::advertised_host`] asks the routing table, which is the
/// right question on a LAN and unanswerable on a cloud instance: there the only
/// address on the machine is the private one — `10.0.0.x` inside a VCN — and the
/// public address is NAT'd somewhere the server cannot see. So the printed URL
/// named a host nothing outside the subnet could reach, and the working one had
/// to be assembled by hand by somebody who already knew that.
///
/// **It changes what is printed and nothing else.** The listener, the bind
/// address and the exposure are all untouched: this is the operator telling the
/// server a fact about the network it is on, which is why it is not validated
/// against anything but its own shape. A host that does not resolve prints a URL
/// that does not work, exactly as the routing table's own wrong answer already
/// did — and unlike that one, this is a value somebody chose and can see.
///
/// It is honoured whether or not the server left loopback, because a tunnel is
/// the other case this is for: `cloudflared` forwards a public hostname to
/// `127.0.0.1`, so the server is loopback-bound and the hostname works anyway.
///
/// **A host, not a URL.** [`crate::Server::url_for`] supplies the scheme and
/// takes the port from the listener, so anything carrying either would be
/// interpolated into nonsense. Refused rather than repaired: stripping a scheme
/// raises the question of what a port that disagrees with the listener should
/// mean, and there is no answer to that which is not a surprise.
fn advertise_host_in(
    flags: &BTreeMap<String, String>,
    environment: Option<String>,
) -> Result<Option<String>, String> {
    let Some(host) = flags
        .get("advertise-host")
        .cloned()
        .or(environment)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };

    if host.contains("://") || host.contains('/') || host.contains(':') {
        return Err(format!(
            "--advertise-host takes a host and not a URL: {host} — the scheme is \
             http and the port comes from the listener"
        ));
    }
    Ok(Some(host))
}

/// The spellings of yes and no this accepts.
///
/// Wider than one word each because this value arrives from three kinds of
/// author — a person typing a flag, a `systemd` unit's `Environment=`, and a
/// `docker run -e` — and each of those has its own habit. It is not open-ended:
/// anything else is a refusal, so a value nobody meant cannot land on a default.
fn exposure_from(value: &str) -> Option<Exposure> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Some(Exposure::NetworkAccessible),
        "false" | "0" | "off" | "no" => Some(Exposure::LocalOnly),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(arguments: &[&str]) -> impl Iterator<Item = String> {
        arguments
            .iter()
            .map(|argument| argument.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// The server's flag set, which is the wider of the two.
    fn server_flags(given: &[&str]) -> Result<BTreeMap<String, String>, String> {
        flags_from(
            arguments(given),
            &["port", "ui", "advertise-host"],
            &["network"],
        )
    }

    fn port_from(given: &[&str], environment: Option<String>) -> Result<u16, String> {
        port_in(&server_flags(given)?, environment)
    }

    fn ui_from(given: &[&str], environment: Option<String>) -> Result<Option<PathBuf>, String> {
        Ok(ui_in(&server_flags(given)?, environment))
    }

    fn network_from(
        given: &[&str],
        environment: Option<String>,
    ) -> Result<Option<Network>, String> {
        network_in(&server_flags(given)?, environment)
    }

    fn on(source: NetworkSource) -> Result<Option<Network>, String> {
        Ok(Some(Network {
            exposure: Exposure::NetworkAccessible,
            source,
        }))
    }

    fn off(source: NetworkSource) -> Result<Option<Network>, String> {
        Ok(Some(Network {
            exposure: Exposure::LocalOnly,
            source,
        }))
    }

    fn advertise_host_from(
        given: &[&str],
        environment: Option<String>,
    ) -> Result<Option<String>, String> {
        advertise_host_in(&server_flags(given)?, environment)
    }

    #[test]
    fn nothing_asked_for_is_the_default() {
        assert_eq!(port_from(&[], None), Ok(DEFAULT_PORT));
        assert_eq!(ui_from(&[], None), Ok(None));
        // Not `Some(LocalOnly)`: nothing was said, so the file decides.
        assert_eq!(network_from(&[], None), Ok(None));
        // Nothing insisted on, so the routing table is left to answer.
        assert_eq!(advertise_host_from(&[], None), Ok(None));
    }

    /// A host to print, for the box whose own address is not the one anybody
    /// reaches it at. A cloud instance is the case: the routing table names the
    /// private VCN address, because that genuinely is the only address on the
    /// machine, and the public one is NAT'd somewhere the server cannot see.
    ///
    /// Trimmed and emptied-to-`None` like [`ui_in`], for its reason: an exported
    /// `LAPLUS_ADVERTISE_HOST=` is a common way to mean "not set".
    #[test]
    fn a_host_to_advertise_is_taken_from_the_argument_then_the_environment() {
        assert_eq!(
            advertise_host_from(&["--advertise-host", "129.150.37.24"], None),
            Ok(Some("129.150.37.24".to_string()))
        );
        assert_eq!(
            advertise_host_from(&["--advertise-host=laplus.example.com"], None),
            Ok(Some("laplus.example.com".to_string()))
        );
        assert_eq!(
            advertise_host_from(&[], Some("box.example.com".to_string())),
            Ok(Some("box.example.com".to_string()))
        );
        assert_eq!(
            advertise_host_from(&["--advertise-host", "chosen"], Some("ignored".to_string())),
            Ok(Some("chosen".to_string()))
        );
        assert_eq!(
            advertise_host_from(&["--advertise-host", "  spaced  "], None),
            Ok(Some("spaced".to_string()))
        );
        assert_eq!(advertise_host_from(&[], Some("   ".to_string())), Ok(None));
    }

    /// **A host and not a URL**, which is the one mistake this flag invites: the
    /// port comes from the listener and the scheme is `http` because that is all
    /// this server speaks, so a value carrying either would be interpolated into
    /// nonsense — `http://http://host:4773/`.
    ///
    /// Refused rather than repaired. Stripping a scheme would leave the question
    /// of what to do with a path or a port that disagrees with the listener, and
    /// an operator who is told the rule once types it correctly forever.
    #[test]
    fn a_host_to_advertise_is_not_a_url() {
        assert!(advertise_host_from(&["--advertise-host", "http://host"], None).is_err());
        assert!(advertise_host_from(&["--advertise-host", "host:4773"], None).is_err());
        assert!(advertise_host_from(&["--advertise-host", "host/path"], None).is_err());
    }

    #[test]
    fn the_environment_is_read_when_the_command_line_is_silent() {
        assert_eq!(port_from(&[], Some("5000".to_string())), Ok(5000));
        assert_eq!(
            ui_from(&[], Some("dist".to_string())),
            Ok(Some(PathBuf::from("dist")))
        );
        assert_eq!(
            network_from(&[], Some("1".to_string())),
            on(NetworkSource::Environment)
        );
    }

    /// An argument beats the environment, which is the order every other tool
    /// uses and the one a developer overriding a single run expects.
    #[test]
    fn an_argument_wins_over_the_environment_in_both_spellings() {
        assert_eq!(
            port_from(&["--port", "5001"], Some("5000".to_string())),
            Ok(5001)
        );
        assert_eq!(
            port_from(&["--port=5002"], Some("5000".to_string())),
            Ok(5002)
        );
        assert_eq!(
            ui_from(&["--ui", "chosen"], Some("ignored".to_string())),
            Ok(Some(PathBuf::from("chosen")))
        );
        assert_eq!(
            ui_from(&["--ui=chosen"], Some("ignored".to_string())),
            Ok(Some(PathBuf::from("chosen")))
        );
        assert_eq!(
            network_from(&["--network"], Some("off".to_string())),
            on(NetworkSource::Flag)
        );
        // The direction that has no other spelling: one run pulled back onto
        // loopback despite an environment that says otherwise.
        assert_eq!(
            network_from(&["--network=false"], Some("on".to_string())),
            off(NetworkSource::Flag)
        );
    }

    /// Ticket 04. `--network` on its own is the whole of what an operator
    /// should have to type, and it must not swallow the flag after it.
    #[test]
    fn the_network_switch_stands_alone() {
        assert_eq!(network_from(&["--network"], None), on(NetworkSource::Flag));

        let flags = server_flags(&["--network", "--port", "5004"]).expect("both are recognised");
        assert_eq!(port_in(&flags, None), Ok(5004));
        assert_eq!(
            network_in(&flags, None),
            on(NetworkSource::Flag),
            "--network took the next argument as its value"
        );
    }

    /// Three kinds of author write this value — a person, a systemd unit, a
    /// `docker run -e` — and each has its own habit.
    #[test]
    fn on_and_off_have_more_than_one_spelling_each() {
        for said in ["true", "1", "on", "yes", "ON", " yes "] {
            assert_eq!(
                network_from(&[], Some(said.to_string())),
                on(NetworkSource::Environment),
                "{said} should mean on"
            );
        }
        for said in ["false", "0", "off", "no", "Off"] {
            assert_eq!(
                network_from(&[], Some(said.to_string())),
                off(NetworkSource::Environment),
                "{said} should mean off"
            );
        }
    }

    /// A value nobody meant must not land on either answer. Falling back to
    /// loopback is a phone that cannot connect with nothing in the log to say
    /// why; falling forward is a typo opening the machine up.
    #[test]
    fn a_network_value_this_does_not_understand_is_refused() {
        for (given, environment) in [
            (vec!["--network=flase"], None),
            (vec!["--network="], None),
            (vec!["--network=local-only"], None),
            (vec!["--network", "--network"], None),
            (vec![], Some("enabled".to_string())),
        ] {
            let refused = network_from(&given, environment.clone());
            assert!(refused.is_err(), "{given:?} {environment:?} should be refused");
            assert!(
                !refused.unwrap_err().is_empty(),
                "{given:?} should say what was wrong"
            );
        }
    }

    /// An exported `LAPLUS_NETWORK=` is a common way to mean "not set", and
    /// leaving the file in charge is what that should do — not refuse, and not
    /// force loopback over a file that says otherwise.
    #[test]
    fn a_blank_environment_value_leaves_the_file_in_charge() {
        assert_eq!(network_from(&[], Some(String::new())), Ok(None));
        assert_eq!(network_from(&[], Some("  ".to_string())), Ok(None));
    }

    /// Ticket 01. The two flags are independent, and either order works.
    #[test]
    fn the_port_and_the_bundle_are_asked_for_together() {
        let flags = server_flags(&["--ui", "dist", "--port", "5003"]).expect("both are recognised");
        assert_eq!(port_in(&flags, None), Ok(5003));
        assert_eq!(ui_in(&flags, None), Some(PathBuf::from("dist")));
    }

    /// A typo has to be a refusal with a sentence, not a silent fall back to
    /// the default — which would start a server the developer then cannot find.
    #[test]
    fn a_malformed_request_is_refused_rather_than_ignored() {
        for (given, environment) in [
            (vec!["--port"], None),
            (vec!["--port", "http"], None),
            (vec!["--port", "5000", "--wat"], None),
            (vec!["--prot", "5000"], None),
            (vec!["--ui"], None),
            (vec!["--port", "1", "--port", "2"], None),
            (vec![], Some("nonsense".to_string())),
        ] {
            let refused = port_from(&given, environment);
            assert!(refused.is_err(), "{given:?} should be refused");
            assert!(
                !refused.unwrap_err().is_empty(),
                "{given:?} should say what was wrong"
            );
        }
    }

    /// The shell embeds its bundle and has a Settings panel for the switch, so
    /// being handed either means the developer expected something that will not
    /// happen. Refused rather than ignored, which is the whole reason there are
    /// two entry points.
    ///
    /// `--network` in particular: the shell restarts itself when the switch
    /// moves (`docs/adr/0022`), so a flag that overrode the file for one run
    /// would be undone by the first use of the panel it sits behind.
    #[test]
    fn the_shells_flag_set_is_the_port_and_nothing_else() {
        assert!(flags_from(arguments(&["--ui", "dist"]), &["port"], &[]).is_err());
        assert!(flags_from(arguments(&["--network"]), &["port"], &[]).is_err());
        assert!(flags_from(arguments(&["--port", "5000"]), &["port"], &[]).is_ok());
    }

    /// An empty value is nothing asked for rather than a path named "". The
    /// environment is where this arrives from — an exported `LAPLUS_UI=` is a
    /// common way to mean "not set".
    #[test]
    fn a_blank_bundle_path_is_no_bundle_at_all() {
        assert_eq!(ui_from(&[], Some("   ".to_string())), Ok(None));
        assert_eq!(ui_from(&["--ui="], None), Ok(None));
    }

    #[test]
    fn zero_is_a_port_like_any_other_here() {
        assert_eq!(port_from(&["--port", "0"], None), Ok(0));
    }

    fn invoked_with(arguments: &[&str]) -> Result<Invoked, String> {
        invoked_from(arguments.iter().map(|argument| argument.to_string()).collect())
    }

    /// The spelling that has always worked, still working. Every run of this
    /// binary before `service` existed had no verb at all.
    #[test]
    fn no_verb_is_a_server_as_it_always_was() {
        assert!(matches!(invoked_with(&[]), Ok(Invoked::Serve(_))));
        assert!(matches!(
            invoked_with(&["--port", "5000"]),
            Ok(Invoked::Serve(_))
        ));
    }

    /// The flags travel twice over: parsed, so this run can refuse a bad one
    /// before touching systemd, and verbatim, so the unit starts the server the
    /// operator described rather than the one this parser defaulted to.
    #[test]
    fn the_service_verb_keeps_the_flags_both_ways() {
        let invoked = invoked_with(&["service", "install", "--network", "--port", "5000"]);
        let Ok(Invoked::Service {
            verb,
            requested,
            arguments,
        }) = invoked
        else {
            panic!("expected a service invocation, got {invoked:?}");
        };
        assert_eq!(verb, crate::service::Verb::Install);
        assert_eq!(requested.port, 5000);
        assert_eq!(arguments, vec!["--network", "--port", "5000"]);
    }

    #[test]
    fn a_service_verb_with_no_command_says_which_ones_there_are() {
        let failure = invoked_with(&["service"]).unwrap_err();
        assert!(failure.contains("install"));
        assert!(failure.contains("uninstall"));
    }

    /// A bad flag is refused before anything is written, which is the whole
    /// reason the flags are parsed here as well as copied.
    #[test]
    fn an_unknown_flag_after_the_verb_is_still_refused() {
        assert!(invoked_with(&["service", "install", "--porrt", "5000"]).is_err());
    }
}
