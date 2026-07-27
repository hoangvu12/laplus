//! Which port a lightcode process serves on.
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

/// The port lightcode listens on unless told otherwise.
///
/// Not upstream's 3773, so a reference server and lightcode can run side by side
/// while the port is still being compared against captures.
pub const DEFAULT_PORT: u16 = 4773;

/// The port this process was asked for: `--port <n>` or `--port=<n>` on the
/// command line, else `LIGHTCODE_PORT`, else [`DEFAULT_PORT`].
///
/// Port 0 asks the operating system for a free one. The tests use it, and so may
/// a developer running two builds at once — with the cost above understood.
pub fn requested_port() -> Result<u16, String> {
    port_from(
        std::env::args().skip(1),
        std::env::var("LIGHTCODE_PORT").ok(),
    )
}

fn port_from(
    mut arguments: impl Iterator<Item = String>,
    environment: Option<String>,
) -> Result<u16, String> {
    if let Some(argument) = arguments.next() {
        let value = match argument.strip_prefix("--port=") {
            Some(value) => value.to_string(),
            None if argument == "--port" => arguments
                .next()
                .ok_or_else(|| "--port needs a value".to_string())?,
            None => return Err(format!("unrecognised argument {argument}")),
        };
        if let Some(extra) = arguments.next() {
            return Err(format!("unrecognised argument {extra}"));
        }
        return value
            .parse()
            .map_err(|_| format!("{value} is not a port number"));
    }

    match environment {
        Some(value) => value
            .parse()
            .map_err(|_| format!("LIGHTCODE_PORT={value} is not a port number")),
        None => Ok(DEFAULT_PORT),
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

    #[test]
    fn nothing_asked_for_is_the_default() {
        assert_eq!(port_from(arguments(&[]), None), Ok(DEFAULT_PORT));
    }

    #[test]
    fn the_environment_is_read_when_the_command_line_is_silent() {
        assert_eq!(port_from(arguments(&[]), Some("5000".to_string())), Ok(5000));
    }

    /// An argument beats the environment, which is the order every other tool
    /// uses and the one a developer overriding a single run expects.
    #[test]
    fn an_argument_wins_over_the_environment_in_both_spellings() {
        assert_eq!(
            port_from(arguments(&["--port", "5001"]), Some("5000".to_string())),
            Ok(5001)
        );
        assert_eq!(
            port_from(arguments(&["--port=5002"]), Some("5000".to_string())),
            Ok(5002)
        );
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
            (vec![], Some("nonsense".to_string())),
        ] {
            let refused = port_from(arguments(&given), environment);
            assert!(refused.is_err(), "{given:?} should be refused");
            assert!(
                !refused.unwrap_err().is_empty(),
                "{given:?} should say what was wrong"
            );
        }
    }

    #[test]
    fn zero_is_a_port_like_any_other_here() {
        assert_eq!(port_from(arguments(&["--port", "0"]), None), Ok(0));
    }
}
