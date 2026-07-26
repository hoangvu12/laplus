//! The permissive local handshake at the socket upgrade.
//!
//! Accounts are out of scope for v1, but the handshake *shape* is not
//! optional: the UI presents a credential when it connects and the server has
//! to take it. So this module answers one question — may this upgrade proceed?
//! — and answers it without an identity store.
//!
//! The policy, from the spec:
//!
//! - **Bind to loopback.** Done where the listener is created, not here.
//! - **Reject non-local origins.** The one refusal this module makes. It
//!   matters because binding to loopback does *not* stop a page on another
//!   origin from asking the user's own browser to connect for it.
//! - **Verify the credential against nothing.** Every shape is accepted, and
//!   so is no credential at all. What is recorded is *which* shape arrived,
//!   because that is the thing later work might need and the thing a capture
//!   can be checked against.
//!
//! The shapes come from ticket 01's captures and from the reference server's
//! `authenticateWebSocketUpgrade`, in its precedence order: the `wsTicket`
//! query parameter first, then the `Authorization` header, then the
//! `t3_session` cookie.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::Value;

/// The cookie the browser UI presents. Named in the server config's auth
/// descriptor, so the two must agree.
pub const SESSION_COOKIE_NAME: &str = "t3_session";

/// The query parameter non-browser clients present. The browser cannot use it
/// — the WebSocket API cannot set request headers, which is the whole reason
/// the ticket shape exists.
pub const WEBSOCKET_TICKET_QUERY_PARAM: &str = "wsTicket";

/// Which credential shape the client presented. Recorded, never verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Credential {
    /// `GET /ws?wsTicket=…`
    WebSocketTicket,
    /// `Authorization: Bearer …`
    BearerToken,
    /// `Authorization: DPoP …`
    DpopToken,
    /// `Cookie: t3_session=…`
    SessionCookie,
    /// None of the above. Accepted anyway: v1 has no identity store to refuse
    /// against, and refusing here would mean building a pairing flow the spec
    /// puts out of scope.
    Absent,
}

/// A refused upgrade, ready to be rendered as the 401 the client understands.
#[derive(Debug, Clone)]
pub struct Rejection {
    /// Why, for the server's own logs. Not sent to the client.
    pub detail: String,
    /// Correlation id, echoed to the client so a refusal in the UI can be
    /// matched to a line in the log.
    pub trace_id: String,
}

/// The 401 body, verbatim in shape from
/// `fixtures/socket-wire/06-upgrade-rejected.ndjson`.
#[derive(Debug, Clone, Serialize)]
pub struct AuthInvalidBody {
    #[serde(rename = "_tag")]
    pub tag: &'static str,
    pub code: &'static str,
    pub reason: &'static str,
    #[serde(rename = "traceId")]
    pub trace_id: String,
}

impl Rejection {
    fn new(detail: impl Into<String>) -> Self {
        Rejection {
            detail: detail.into(),
            trace_id: trace_id(),
        }
    }

    /// The JSON body to return with `401 Unauthorized`.
    ///
    /// `reason` is `invalid_credential` even though what was actually wrong
    /// was the origin. The contract's `EnvironmentAuthInvalidReason` is a
    /// closed union of `missing_credential | invalid_credential`, and
    /// upstream has no origin check to have added a third member for. Reusing
    /// the closed union keeps the body decodable by the unmodified client;
    /// inventing a reason would not. The real cause is in
    /// [`Rejection::detail`], which stays server-side.
    pub fn body(&self) -> AuthInvalidBody {
        AuthInvalidBody {
            tag: "EnvironmentAuthInvalidError",
            code: "auth_invalid",
            reason: "invalid_credential",
            trace_id: self.trace_id.clone(),
        }
    }

    pub fn body_value(&self) -> Value {
        serde_json::to_value(self.body()).expect("auth rejection body serializes")
    }
}

/// What the upgrade request presented, in the shapes this module reads.
///
/// Taking this rather than a header map keeps the decision pure and keeps the
/// web framework out of the policy.
#[derive(Debug, Default, Clone, Copy)]
pub struct UpgradeRequest<'a> {
    /// The raw query string, without the leading `?`.
    pub query: Option<&'a str>,
    pub origin: Option<&'a str>,
    pub authorization: Option<&'a str>,
    pub cookie: Option<&'a str>,
}

/// Decide whether an upgrade may proceed.
pub fn authorize(request: UpgradeRequest<'_>) -> Result<Credential, Rejection> {
    if let Some(origin) = request.origin {
        if !is_local_origin(origin) {
            return Err(Rejection::new(format!(
                "refused socket upgrade from non-local origin {origin}"
            )));
        }
    }

    Ok(credential(request))
}

fn credential(request: UpgradeRequest<'_>) -> Credential {
    if let Some(query) = request.query {
        if query_param(query, WEBSOCKET_TICKET_QUERY_PARAM).is_some() {
            return Credential::WebSocketTicket;
        }
    }

    if let Some(authorization) = request.authorization {
        if let Some(token) = authorization.strip_prefix("Bearer ") {
            if !token.trim().is_empty() {
                return Credential::BearerToken;
            }
        }
        if let Some(token) = authorization.strip_prefix("DPoP ") {
            if !token.trim().is_empty() {
                return Credential::DpopToken;
            }
        }
    }

    if let Some(cookie) = request.cookie {
        if cookie_value(cookie, SESSION_COOKIE_NAME).is_some() {
            return Credential::SessionCookie;
        }
    }

    Credential::Absent
}

/// Is this `Origin` header value a page served from this machine?
///
/// A browser sends the literal `null` for opaque origins (`file://`,
/// sandboxed iframes), which is not local and is refused. An absent header is
/// handled by the caller — non-browser clients do not send one.
fn is_local_origin(origin: &str) -> bool {
    let after_scheme = match origin.split_once("://") {
        Some((scheme, rest)) if !scheme.is_empty() => rest,
        _ => return false,
    };
    // Origins carry no path, but be liberal about what we accept.
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = match authority.strip_prefix('[') {
        // IPv6 literal: the port, if any, is after the closing bracket.
        Some(rest) => rest.split(']').next().unwrap_or_default(),
        None => authority.split(':').next().unwrap_or_default(),
    };

    is_loopback_host(host)
}

fn is_loopback_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host == "::1" || host == "0:0:0:0:0:0:0:1" {
        return true;
    }
    // The whole 127.0.0.0/8 block, not just 127.0.0.1.
    match host.parse::<std::net::Ipv4Addr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => false,
    }
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, value)| *key == name && !value.trim().is_empty())
        .map(|(_, value)| value)
}

fn cookie_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header
        .split(';')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.trim(), value.trim()))
        .find(|(key, value)| *key == name && !value.is_empty())
        .map(|(_, value)| value)
}

/// A 32-hex-character correlation id, matching the shape the reference server
/// puts in `traceId`.
///
/// This is a diagnostic handle, not a secret: it exists so a refusal the user
/// sees can be found in the log. `RandomState` is seeded from the OS per
/// instance, which is ample for that and avoids a dependency for it.
fn trace_id() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);

    let mut id = String::with_capacity(32);
    for half in 0..2u128 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(now ^ half);
        id.push_str(&format!("{:016x}", hasher.finish()));
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>() -> UpgradeRequest<'a> {
        UpgradeRequest::default()
    }

    /// The browser UI's shape, from `01-browser-session.ndjson`.
    #[test]
    fn the_browsers_cookie_and_origin_are_accepted() {
        let accepted = authorize(UpgradeRequest {
            origin: Some("http://127.0.0.1:3999"),
            cookie: Some("t3_session=eyJ2Ijox.c2ln"),
            ..request()
        });
        assert_eq!(accepted.unwrap(), Credential::SessionCookie);
    }

    /// The non-browser shape, from `02-request-response.ndjson`. Note it sends
    /// no `Origin` at all.
    #[test]
    fn a_websocket_ticket_with_no_origin_is_accepted() {
        let accepted = authorize(UpgradeRequest {
            query: Some("wsTicket=eyJ2Ijox.c2ln"),
            ..request()
        });
        assert_eq!(accepted.unwrap(), Credential::WebSocketTicket);
    }

    #[test]
    fn authorization_headers_are_accepted_in_both_schemes() {
        assert_eq!(
            authorize(UpgradeRequest {
                authorization: Some("Bearer eyJ2Ijox.c2ln"),
                ..request()
            })
            .unwrap(),
            Credential::BearerToken
        );
        assert_eq!(
            authorize(UpgradeRequest {
                authorization: Some("DPoP eyJ2Ijox.c2ln"),
                ..request()
            })
            .unwrap(),
            Credential::DpopToken
        );
    }

    /// The ticket parameter wins over the other two, matching the reference
    /// server's precedence.
    #[test]
    fn the_ticket_parameter_takes_precedence() {
        let accepted = authorize(UpgradeRequest {
            query: Some("wsTicket=ticket"),
            authorization: Some("Bearer token"),
            cookie: Some("t3_session=cookie"),
            ..request()
        });
        assert_eq!(accepted.unwrap(), Credential::WebSocketTicket);
    }

    /// Permissive means permissive: no credential is still an upgrade. v1 has
    /// nothing to check one against, and refusing would mean shipping a
    /// pairing flow the spec puts out of scope.
    #[test]
    fn an_absent_credential_is_accepted_and_recorded_as_absent() {
        assert_eq!(authorize(request()).unwrap(), Credential::Absent);
    }

    /// Empty values are the same as no value — this is the shape check, and an
    /// empty string is not the shape.
    #[test]
    fn empty_credentials_are_not_mistaken_for_present_ones() {
        assert_eq!(
            authorize(UpgradeRequest {
                query: Some("wsTicket=&other=1"),
                cookie: Some("t3_session= ; unrelated=x"),
                authorization: Some("Bearer  "),
                ..request()
            })
            .unwrap(),
            Credential::Absent
        );
    }

    #[test]
    fn the_session_cookie_is_found_among_others() {
        assert_eq!(
            authorize(UpgradeRequest {
                cookie: Some("theme=dark; t3_session=eyJ2Ijox.c2ln; other=1"),
                ..request()
            })
            .unwrap(),
            Credential::SessionCookie
        );
    }

    /// The check matches on **host** and ignores the scheme, which cuts the
    /// opposite way from what you would guess, and ticket 23 walks straight
    /// into it: `tauri://localhost` is accepted because its host is
    /// `localhost`, while `http://tauri.localhost` — the origin Tauri v2
    /// actually uses on Windows — is refused, because `tauri.localhost` is
    /// neither `localhost` nor a `127.0.0.0/8` address. Pinned as a test
    /// rather than left as a note, so ticket 23 finds out by reading a failure
    /// rather than by guessing.
    #[test]
    fn the_origin_check_matches_on_host_and_ignores_the_scheme() {
        assert!(
            authorize(UpgradeRequest {
                origin: Some("tauri://localhost"),
                ..request()
            })
            .is_ok(),
            "a custom scheme on a loopback host is accepted"
        );
        assert!(
            authorize(UpgradeRequest {
                origin: Some("http://tauri.localhost"),
                ..request()
            })
            .is_err(),
            "tauri.localhost is a different host, and is refused until ticket 23 widens this"
        );
    }

    #[test]
    fn loopback_origins_are_accepted() {
        for origin in [
            "http://127.0.0.1:3999",
            "http://127.0.0.1",
            "http://127.4.5.6:1",
            "http://localhost:5173",
            "http://LocalHost",
            "https://localhost:8443",
            "http://[::1]:1420",
        ] {
            assert!(
                authorize(UpgradeRequest {
                    origin: Some(origin),
                    ..request()
                })
                .is_ok(),
                "{origin} should be accepted"
            );
        }
    }

    /// Binding to loopback does not stop a page elsewhere from asking the
    /// user's browser to connect on its behalf, which is what this refuses.
    #[test]
    fn non_local_origins_are_refused_even_with_a_valid_looking_credential() {
        for origin in [
            "https://evil.example",
            "http://127.0.0.1.evil.example",
            "http://localhost.evil.example",
            "http://192.168.1.10:3999",
            "null",
            "",
        ] {
            let refused = authorize(UpgradeRequest {
                origin: Some(origin),
                cookie: Some("t3_session=eyJ2Ijox.c2ln"),
                ..request()
            });
            assert!(refused.is_err(), "{origin} should be refused");
        }
    }

    /// The body the client already knows how to read — the shape captured in
    /// `06-upgrade-rejected.ndjson`.
    #[test]
    fn a_refusal_renders_the_captured_error_body() {
        let refused = authorize(UpgradeRequest {
            origin: Some("https://evil.example"),
            ..request()
        })
        .expect_err("refused");

        let body = refused.body_value();
        assert_eq!(body["_tag"], "EnvironmentAuthInvalidError");
        assert_eq!(body["code"], "auth_invalid");
        assert_eq!(body["reason"], "invalid_credential");
        assert_eq!(
            body["traceId"].as_str().expect("traceId is a string").len(),
            32
        );
        assert!(refused.detail.contains("evil.example"));
    }

    #[test]
    fn trace_ids_are_hex_and_do_not_repeat() {
        let first = trace_id();
        let second = trace_id();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
