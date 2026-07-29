//! Which port a laplus process serves on.
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

use std::collections::BTreeMap;
use std::path::PathBuf;

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
    let flags = flags_from(std::env::args().skip(1), &["port"])?;
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
}

/// What the plain server was asked for, from the command line and then the
/// environment.
pub fn requested() -> Result<Requested, String> {
    let flags = flags_from(std::env::args().skip(1), &["port", "ui"])?;
    Ok(Requested {
        port: port_in(&flags, std::env::var("LAPLUS_PORT").ok())?,
        ui: ui_in(&flags, std::env::var("LAPLUS_UI").ok()),
    })
}

/// The flags this process was given, by name and without the `--`.
///
/// `--name value` and `--name=value` are the same thing, which is what every
/// other tool does. An unknown flag is a refusal rather than something to skip:
/// a typo that started a server with the default it was told not to use is the
/// silent failure [`DEFAULT_PORT`]'s note is about.
fn flags_from(
    mut arguments: impl Iterator<Item = String>,
    allowed: &[&str],
) -> Result<BTreeMap<String, String>, String> {
    let mut given = BTreeMap::new();
    while let Some(argument) = arguments.next() {
        let (name, value) = match argument.split_once('=') {
            Some((name, value)) => (name.to_string(), value.to_string()),
            None => {
                let value = arguments
                    .next()
                    .ok_or_else(|| format!("{argument} needs a value"))?;
                (argument, value)
            }
        };
        let flag = name
            .strip_prefix("--")
            .filter(|flag| allowed.contains(flag))
            .ok_or_else(|| format!("unrecognised argument {name}"))?;
        if given.insert(flag.to_string(), value).is_some() {
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
    fn port_from(given: &[&str], environment: Option<String>) -> Result<u16, String> {
        port_in(&flags_from(arguments(given), &["port", "ui"])?, environment)
    }

    fn ui_from(given: &[&str], environment: Option<String>) -> Result<Option<PathBuf>, String> {
        Ok(ui_in(
            &flags_from(arguments(given), &["port", "ui"])?,
            environment,
        ))
    }

    #[test]
    fn nothing_asked_for_is_the_default() {
        assert_eq!(port_from(&[], None), Ok(DEFAULT_PORT));
        assert_eq!(ui_from(&[], None), Ok(None));
    }

    #[test]
    fn the_environment_is_read_when_the_command_line_is_silent() {
        assert_eq!(port_from(&[], Some("5000".to_string())), Ok(5000));
        assert_eq!(
            ui_from(&[], Some("dist".to_string())),
            Ok(Some(PathBuf::from("dist")))
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
    }

    /// Ticket 01. The two flags are independent, and either order works.
    #[test]
    fn the_port_and_the_bundle_are_asked_for_together() {
        let flags = flags_from(arguments(&["--ui", "dist", "--port", "5003"]), &["port", "ui"])
            .expect("both are recognised");
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

    /// The shell embeds its bundle, so being handed a directory to serve means
    /// the developer expected something that will not happen. Refused rather
    /// than ignored, which is the whole reason there are two entry points.
    #[test]
    fn the_shells_flag_set_does_not_include_the_bundle() {
        assert!(flags_from(arguments(&["--ui", "dist"]), &["port"]).is_err());
        assert!(flags_from(arguments(&["--port", "5000"]), &["port"]).is_ok());
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
}
