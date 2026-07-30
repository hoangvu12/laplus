//! Minting, listing and revoking pairing codes from a terminal.
//!
//! `laplus-server auth pairing create` is the answer to a question
//! `docs/running-headless.md` had a bad answer to: a server running as a
//! background service prints its startup credential into a log file, so pairing
//! a second device meant reading a log, or restarting the server and reading it
//! faster. Neither is a thing to ask of somebody who just wants their phone to
//! connect.
//!
//! **This works because a pairing code has no in-memory half.**
//! [`crate::store::Database::issue_pairing_link`] says so directly — "the row is
//! the code's whole existence" — so a second process opening the same SQLite
//! file can mint one that the running server will honour, without the two ever
//! speaking. That is the whole mechanism, and it is worth stating because it is
//! the only reason this is twenty lines of database call rather than an
//! authenticated HTTP round trip against a server whose address we would first
//! have to discover.
//!
//! Upstream's is `t3 auth pairing create|list|revoke`
//! (`apps/server/src/cli/auth.ts`), and this follows it deliberately: the same
//! three verbs, the same `--ttl`, `--label`, `--base-url` and `--json`, and the
//! same rule that `list` never reveals a secret.
//!
//! **What is added is [`crate::qr`].** Upstream prints one from `t3 serve` but
//! not from `auth pairing create`; the code minted here is the one most likely
//! to be carried to a phone, so it is the one that most wants a square to point
//! a camera at.

use crate::pairing::{self, PairingLink};

/// Which of the three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Create,
    List,
    Revoke,
}

impl Verb {
    pub fn parse(word: &str) -> Result<Verb, String> {
        match word {
            "create" | "new" => Ok(Verb::Create),
            "list" | "ls" => Ok(Verb::List),
            "revoke" | "delete" => Ok(Verb::Revoke),
            other => Err(format!(
                "unrecognised pairing command {other} — create, list or revoke"
            )),
        }
    }
}

/// Turn what an operator typed into the modifier SQLite's `strftime` takes.
///
/// `5m`, `2h`, `30d`, `90s`, and the spelled-out `15 minutes` upstream's help
/// text advertises. The unit is always plural in the output because SQLite
/// accepts both and one spelling is easier to read back in a test.
///
/// **A bare number is refused**, rather than assumed to be seconds or minutes.
/// The whole value of this flag is that the operator decides how long a
/// credential lives, and a guess about the unit is a guess about the size of the
/// window somebody else has to get in.
pub fn ttl_from(given: &str) -> Result<String, String> {
    let given = given.trim().to_lowercase();
    if given.is_empty() {
        return Err("a lifetime needs a value, for example 5m, 2h or 30d".to_string());
    }
    let split = given
        .find(|character: char| !character.is_ascii_digit())
        .ok_or_else(|| format!("{given} has no unit — try {given}m, {given}h or {given}d"))?;
    let (count, unit) = given.split_at(split);
    let count: u32 = count
        .parse()
        .map_err(|_| format!("{given} does not start with a number of anything"))?;
    if count == 0 {
        return Err("a lifetime of zero is a credential that has already expired".to_string());
    }
    let unit = match unit.trim() {
        "s" | "sec" | "secs" | "second" | "seconds" => "seconds",
        "m" | "min" | "mins" | "minute" | "minutes" => "minutes",
        "h" | "hr" | "hrs" | "hour" | "hours" => "hours",
        "d" | "day" | "days" => "days",
        other => {
            return Err(format!(
                "{other} is not a unit this understands — s, m, h or d"
            ))
        }
    };
    Ok(format!("+{count} {unit}"))
}

/// The base URL a phone would reach this server at, if one can be worked out.
///
/// `explicit` is `--base-url` and always wins: it is the only thing that can
/// know about a tunnel, a tailnet name or a reverse proxy, none of which are
/// visible from this machine's routing table. Otherwise the LAN address, which
/// is right for the phone-on-the-same-network case this feature exists for.
///
/// `None` is a machine with no route off itself and no `--base-url`, and the
/// caller prints the bare code instead. Not loopback: a pairing URL naming
/// `127.0.0.1` is one that works nowhere except the machine that cannot use it.
pub fn base_url(explicit: Option<&str>, port: u16) -> Option<String> {
    match explicit {
        Some(given) => {
            let trimmed = given.trim().trim_end_matches('/');
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        None => crate::endpoints::lan_address().map(|address| format!("http://{address}:{port}")),
    }
}

/// The URL that pairs a device, with the credential in the fragment.
///
/// The same shape `Server::pairing_url_for` builds, and it must stay the same
/// shape: the page reads `#token=` and nothing else. A fragment because a
/// fragment is never sent to the server — the browser keeps it and hands it to
/// the page — so the credential reaches the UI without travelling over HTTP.
pub fn pairing_url(base: &str, credential: &str) -> String {
    format!("{}/#token={credential}", base.trim_end_matches('/'))
}

/// What `create` prints when nobody asked for JSON.
///
/// The code is on its own line and labelled, because the fallback when the QR
/// does not scan is a human typing twelve characters, and they should not have
/// to pick them out of a URL to do it.
pub fn created_text(link: &PairingLink, url: Option<&str>) -> String {
    let mut said = vec![
        "laplus: a pairing code, good until it is used once.".to_string(),
        format!("  code:    {}", link.credential),
        format!("  expires: {}", link.expires_at),
    ];
    if let Some(label) = &link.label {
        said.push(format!("  label:   {label}"));
    }
    match url {
        Some(url) => {
            said.push(format!("  url:     {url}"));
            if let Some(drawn) = crate::qr::drawn(url) {
                said.push(String::new());
                said.push(drawn);
            }
        }
        None => {
            said.push(String::new());
            said.push(
                "laplus: no address to build a URL from — this machine has no route off itself."
                    .to_string(),
            );
            said.push(
                "laplus: pass --base-url, or type the code above into the pairing screen."
                    .to_string(),
            );
        }
    }
    said.join("\n")
}

/// What `create` prints for a script.
pub fn created_json(link: &PairingLink, url: Option<&str>) -> String {
    serde_json::json!({
        "id": link.id,
        "credential": link.credential,
        "expiresAt": link.expires_at,
        "createdAt": link.created_at,
        "label": link.label,
        "scopes": link.scopes,
        "url": url,
    })
    .to_string()
}

/// What `list` prints. **Never the credential.**
///
/// Upstream's `list` is documented as showing active tokens "without revealing
/// their secrets", and the reason holds here: the thing an operator is doing
/// with this output is deciding what to revoke, which needs an id and a label
/// and nothing else. A list that printed credentials would put every live code
/// into a scrollback every time somebody checked.
pub fn list_text(links: &[PairingLink]) -> String {
    if links.is_empty() {
        return "laplus: no pairing codes are outstanding.".to_string();
    }
    let mut said = vec![format!(
        "laplus: {} pairing code{} outstanding.",
        links.len(),
        if links.len() == 1 { "" } else { "s" }
    )];
    for link in links {
        said.push(format!(
            "  {}  expires {}{}",
            link.id,
            link.expires_at,
            match &link.label {
                Some(label) => format!("  ({label})"),
                None => String::new(),
            }
        ));
    }
    said
        .push("laplus: revoke one with `laplus-server auth pairing revoke <id>`.".to_string());
    said.join("\n")
}

/// [`list_text`] for a script, and with the same secret left out.
pub fn list_json(links: &[PairingLink]) -> String {
    let listed: Vec<serde_json::Value> = links
        .iter()
        .map(|link| {
            serde_json::json!({
                "id": link.id,
                "expiresAt": link.expires_at,
                "createdAt": link.created_at,
                "label": link.label,
                "scopes": link.scopes,
            })
        })
        .collect();
    serde_json::Value::Array(listed).to_string()
}

/// Mint one, against the database the running server is using.
///
/// Single-use and short-lived, like the code Settings mints and unlike the boot
/// grant in the service log — which is reusable for twenty-four hours because a
/// page reload must not lock the operator out of their own window. A code minted
/// here is carried to one device on purpose, so the second use of one is
/// somebody who should not have it.
pub fn create(
    database: &crate::store::Database,
    ttl: &str,
    label: Option<&str>,
) -> Result<PairingLink, String> {
    let id = pairing::record_id().map_err(|error| error.to_string())?;
    let credential = pairing::pairing_code().map_err(|error| error.to_string())?;
    let scopes: Vec<String> = pairing::ENVIRONMENT_SCOPES
        .iter()
        .map(|scope| scope.to_string())
        .collect();
    database
        .issue_pairing_link(crate::store::NewPairingLink {
            id: &id,
            credential: &credential,
            method: pairing::ONE_TIME_TOKEN_METHOD,
            scopes: &scopes,
            subject: pairing::PAIRING_SUBJECT,
            label,
            ttl: pairing::Ttl(ttl),
            reusable: false,
        })
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_units_an_operator_would_type() {
        assert_eq!(ttl_from("5m"), Ok("+5 minutes".to_string()));
        assert_eq!(ttl_from("2h"), Ok("+2 hours".to_string()));
        assert_eq!(ttl_from("30d"), Ok("+30 days".to_string()));
        assert_eq!(ttl_from("90s"), Ok("+90 seconds".to_string()));
        assert_eq!(ttl_from("15 minutes"), Ok("+15 minutes".to_string()));
        assert_eq!(ttl_from("1 HOUR"), Ok("+1 hours".to_string()));
    }

    /// The unit is the whole point of the flag; guessing it would be guessing
    /// how long a stranger has to get in.
    #[test]
    fn a_number_with_no_unit_is_refused_rather_than_assumed() {
        let failure = ttl_from("30").unwrap_err();
        assert!(failure.contains("30m"), "{failure}");
    }

    #[test]
    fn nonsense_lifetimes_are_refused() {
        assert!(ttl_from("").is_err());
        assert!(ttl_from("0m").is_err());
        assert!(ttl_from("5 fortnights").is_err());
        assert!(ttl_from("soon").is_err());
    }

    /// A tunnel or a tailnet name is not on this machine's routing table, so
    /// the operator's answer has to beat the discovered one.
    #[test]
    fn an_explicit_base_url_wins_and_loses_its_trailing_slash() {
        assert_eq!(
            base_url(Some("https://box.tailnet.ts.net/"), 4773),
            Some("https://box.tailnet.ts.net".to_string())
        );
        assert_eq!(base_url(Some("   "), 4773), None);
    }

    #[test]
    fn the_pairing_url_puts_the_credential_in_the_fragment() {
        assert_eq!(
            pairing_url("http://10.0.0.4:4773", "K7M2P9X4QW3Z"),
            "http://10.0.0.4:4773/#token=K7M2P9X4QW3Z"
        );
        // Whether the base ended in a slash is not the caller's problem.
        assert_eq!(
            pairing_url("http://10.0.0.4:4773/", "K7M2P9X4QW3Z"),
            "http://10.0.0.4:4773/#token=K7M2P9X4QW3Z"
        );
    }

    fn a_link() -> PairingLink {
        PairingLink {
            id: "pl_123".to_string(),
            credential: "K7M2P9X4QW3Z".to_string(),
            scopes: vec!["access:read".to_string()],
            subject: pairing::PAIRING_SUBJECT.to_string(),
            label: Some("phone".to_string()),
            created_at: "2026-07-30 12:00:00".to_string(),
            expires_at: "2026-07-30 12:05:00".to_string(),
        }
    }

    #[test]
    fn a_created_code_is_printed_with_a_square_to_scan() {
        let url = pairing_url("http://10.0.0.4:4773", "K7M2P9X4QW3Z");
        let said = created_text(&a_link(), Some(&url));
        assert!(said.contains("K7M2P9X4QW3Z"));
        assert!(said.contains("http://10.0.0.4:4773/#token=K7M2P9X4QW3Z"));
        assert!(said.contains('█'), "there is no QR code in:\n{said}");
    }

    /// Typing the code is the fallback when a camera will not read the square,
    /// so it stays legible on its own line rather than only inside a URL.
    #[test]
    fn the_code_is_printed_apart_from_the_url() {
        let said = created_text(&a_link(), Some("http://10.0.0.4:4773/#token=K7M2P9X4QW3Z"));
        assert!(said.contains("code:    K7M2P9X4QW3Z"));
    }

    #[test]
    fn with_no_address_it_says_what_to_do_instead_of_naming_loopback() {
        let said = created_text(&a_link(), None);
        assert!(!said.contains("127.0.0.1"));
        assert!(said.contains("--base-url"));
        assert!(said.contains("K7M2P9X4QW3Z"));
    }

    // The one rule `list` must not break.
    #[test]
    fn listing_never_prints_a_credential() {
        let links = vec![a_link()];
        assert!(!list_text(&links).contains("K7M2P9X4QW3Z"));
        assert!(!list_json(&links).contains("K7M2P9X4QW3Z"));
        assert!(list_text(&links).contains("pl_123"));
    }

    #[test]
    fn an_empty_list_says_so_rather_than_printing_a_header() {
        assert_eq!(
            list_text(&[]),
            "laplus: no pairing codes are outstanding."
        );
        assert_eq!(list_json(&[]), "[]");
    }

    #[test]
    fn the_json_carries_the_credential_because_a_script_asked_for_it() {
        let url = "http://10.0.0.4:4773/#token=K7M2P9X4QW3Z";
        let said = created_json(&a_link(), Some(url));
        let parsed: serde_json::Value = serde_json::from_str(&said).expect("json");
        assert_eq!(parsed["credential"], "K7M2P9X4QW3Z");
        assert_eq!(parsed["url"], url);
        assert_eq!(parsed["label"], "phone");
    }

    #[test]
    fn the_verbs_have_the_spellings_an_operator_reaches_for() {
        assert_eq!(Verb::parse("create"), Ok(Verb::Create));
        assert_eq!(Verb::parse("list"), Ok(Verb::List));
        assert_eq!(Verb::parse("revoke"), Ok(Verb::Revoke));
        assert!(Verb::parse("mint").is_err());
    }

    /// The whole premise: a second process writes a row the running server will
    /// honour. If this stops working, the feature is gone.
    #[test]
    fn a_code_minted_here_lands_in_the_database() {
        let scratch = tempfile::tempdir().unwrap();
        let database = crate::store::Database::open(&scratch.path().join("state.sqlite")).unwrap();
        let link = create(&database, "+5 minutes", Some("phone")).unwrap();

        assert_eq!(link.credential.len(), 12);
        assert_eq!(link.label.as_deref(), Some("phone"));
        let listed = database.active_pairing_links().unwrap();
        assert!(listed.iter().any(|other| other.id == link.id));
        assert!(database.revoke_pairing_link(&link.id).unwrap());
        let listed = database.active_pairing_links().unwrap();
        assert!(!listed.iter().any(|other| other.id == link.id));
    }
}
