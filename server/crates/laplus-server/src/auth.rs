//! Who may talk to this server: the origin half.
//!
//! This module answers one question — is this request *from* somewhere this
//! server will listen to, and what did it present? — and answers it without
//! touching a database or a web framework. The other half of the answer, does
//! the presented credential verify, is a database read and lives in
//! [`crate::server`] against [`crate::store`]. Neither half is sufficient
//! alone.
//!
//! The policy:
//!
//! - **Bind to loopback.** Done where the listener is created, not here.
//! - **Refuse an origin that is neither this machine nor named by the user.**
//!   The one refusal this module makes. It matters because binding to loopback
//!   does *not* stop a page on another origin from asking the user's own
//!   browser to connect on its behalf. The named hosts are
//!   [`crate::remote_access`], which is how a phone on a tunnel gets in.
//! - **Report what was presented, and verify none of it.** [`authorize`]
//!   returns a [`Presented`], which is an obligation rather than a permission.
//!
//! The credential shapes come from ticket 01's captures and from the reference
//! server's `authenticateWebSocketUpgrade`, in its precedence order: the
//! `wsTicket` query parameter first, then the `Authorization` header, then the
//! `t3_session` cookie.
//!
//! ## This module used to accept an absent credential
//!
//! Until ticket 73 it did, deliberately — `docs/adr/0015`. v1 had no identity
//! store to check one against and no pairing flow to build one on, and the
//! reasoning held because **loopback was the boundary**: only a program already
//! running as the user could reach the port at all, and such a program is
//! already the user.
//!
//! A tunnel dissolves that reasoning rather than stretching it. `cloudflared`
//! runs on this machine and dials `127.0.0.1`, so a request from the far side of
//! the world arrives with the same peer address as the window's and whatever
//! headers it cares to send. There is no signal at this layer that tells them
//! apart. So the exemption had to go, and with it the last thing that made
//! laplus's handshake unlike the reference server's — which has always refused
//! a request carrying nothing (`EnvironmentAuth.ts:599-601`).
//!
//! The desktop window keeps working because it stopped being exempt and started
//! carrying a credential, the way upstream's always has. See
//! [`crate::Server::window_url`]. `docs/adr/0019` records all of this.
//!
//! **[`authorize`] has many callers**: the socket upgrade, the two
//! orchestration snapshot routes, and ticket 73's own. They must all accept
//! exactly what an upgrade accepts — a credential good enough to open the socket
//! that was not good enough to read a snapshot would send the client back to the
//! socket fallback, which is the round trip ticket 31 exists to remove.

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
    /// Which of `EnvironmentAuthInvalidReason`'s two members this is.
    reason: &'static str,
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
            reason: "invalid_credential",
        }
    }

    /// A credential was presented and did not verify.
    ///
    /// Ticket 73's routes, which are the first thing in this server that
    /// verifies a credential against anything at all. [`authorize`] does not
    /// build one of these: what it refuses is an origin, and it reports that
    /// through the same `invalid_credential` for the reason
    /// [`Rejection::body`] gives.
    pub fn invalid_credential(detail: impl Into<String>) -> Rejection {
        Rejection::new(detail)
    }

    /// No credential was presented where one was required.
    ///
    /// The union's other member, and worth telling apart from the one above:
    /// a phone whose thirty days ran out presents a token that no longer
    /// verifies, while a phone whose storage was cleared presents nothing. The
    /// first should re-pair and the second should re-attach, and the client is
    /// the only thing that can tell the user which — which is why the
    /// distinction is on the wire and not only in the log.
    pub fn missing_credential(detail: impl Into<String>) -> Rejection {
        Rejection {
            reason: "missing_credential",
            ..Rejection::new(detail)
        }
    }

    /// The JSON body to return with `401 Unauthorized`.
    ///
    /// For a refusal from [`authorize`], `reason` is `invalid_credential` even
    /// though what was actually wrong was the origin. The contract's
    /// `EnvironmentAuthInvalidReason` is a closed union of
    /// `missing_credential | invalid_credential`, and upstream has no origin
    /// check to have added a third member for. Reusing the closed union keeps
    /// the body decodable by the unmodified client; inventing a reason would
    /// not. The real cause is in [`Rejection::detail`], which stays
    /// server-side.
    pub fn body(&self) -> AuthInvalidBody {
        AuthInvalidBody {
            tag: "EnvironmentAuthInvalidError",
            code: "auth_invalid",
            reason: self.reason,
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

/// A credential that got past the origin check, and the token to verify it by.
///
/// **This module does not verify it.** Verification is a database read and
/// [`crate::store`] is the only file that speaks SQL, so what `authorize`
/// returns is the *obligation*: here is what arrived, here is the string, now
/// go and check it. [`crate::server`] is where the two meet, and it is one
/// function there — nothing else in the tree is allowed to decide it has
/// checked enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presented<'a> {
    pub shape: Credential,
    /// The credential itself. Empty exactly when `shape` is
    /// [`Credential::Absent`].
    pub token: &'a str,
}

/// Decide whether a request may proceed as far as having its credential checked.
///
/// **Every caller must then check it.** A `Presented` is not permission; see
/// its own note. The one thing settled here is the origin, because that is the
/// question a database cannot answer.
///
/// ## The rule
///
/// A page may reach this server if it was served from this machine, or from a
/// host the user named in [`crate::remote_access`]. Everything else is refused
/// before any credential is looked at.
///
/// **A request with no `Origin` at all passes this check**, and that is not an
/// oversight — it is why the credential check that follows is not optional. A
/// browser always sends `Origin` on a WebSocket upgrade and on any request that
/// is not a `GET`, so an absent one means a client that is not a browser, which
/// no origin rule can say anything useful about. `curl` sends whatever it likes.
/// The origin check exists to stop *a page somewhere else* from making the
/// user's own browser talk to this server; it has never been able to stop a
/// program, and before ticket 73 it did not have to, because loopback was the
/// boundary and a program on this machine is already the user.
///
/// A tunnel dissolves that. `cloudflared` runs on this machine and dials
/// `127.0.0.1`, so a request that came from the far side of the world arrives
/// looking exactly like one from the window — same peer address, and any header
/// it likes. **Nothing at this layer can tell them apart**, which is why
/// `docs/adr/0019` supersedes `0015` and why an absent credential is now
/// refused rather than accepted.
pub fn authorize<'a>(
    request: UpgradeRequest<'a>,
    allowed: &crate::remote_access::RemoteAccess,
) -> Result<Presented<'a>, Rejection> {
    if let Some(origin) = request.origin {
        if !is_admissible_origin(origin, allowed) {
            return Err(Rejection::new(format!(
                "refused a request from origin {origin}, which is neither this machine \
                 nor named in remote-access.json"
            )));
        }
    }

    Ok(presented(request))
}

fn presented(request: UpgradeRequest<'_>) -> Presented<'_> {
    if let Some(query) = request.query {
        if let Some(ticket) = query_param(query, WEBSOCKET_TICKET_QUERY_PARAM) {
            return Presented {
                shape: Credential::WebSocketTicket,
                token: ticket,
            };
        }
    }

    if let Some(authorization) = request.authorization {
        if let Some(token) = authorization.strip_prefix("Bearer ") {
            if !token.trim().is_empty() {
                return Presented {
                    shape: Credential::BearerToken,
                    token: token.trim(),
                };
            }
        }
        if let Some(token) = authorization.strip_prefix("DPoP ") {
            if !token.trim().is_empty() {
                return Presented {
                    shape: Credential::DpopToken,
                    token: token.trim(),
                };
            }
        }
    }

    if let Some(cookie) = request.cookie {
        if let Some(session) = cookie_value(cookie, SESSION_COOKIE_NAME) {
            return Presented {
                shape: Credential::SessionCookie,
                token: session,
            };
        }
    }

    Presented {
        shape: Credential::Absent,
        token: "",
    }
}

/// Is this `Origin` one this server will hear from at all?
///
/// This machine, or a host the user wrote down. Both halves matter: without the
/// first the window cannot connect, and without the second a tunnel cannot.
fn is_admissible_origin(origin: &str, allowed: &crate::remote_access::RemoteAccess) -> bool {
    let Some(host) = origin_host(origin) else {
        // A browser sends the literal `null` for opaque origins (`file://`,
        // sandboxed iframes), which parses to no host and is refused.
        return false;
    };
    is_loopback_host(host) || allowed.allows(host)
}

/// The host an `Origin` header names, or `None` if it names none.
///
/// Matches on **host** and ignores the scheme, which cuts the opposite way from
/// what you would guess: `tauri://localhost` is local because its host is
/// `localhost`, while `http://tauri.localhost` is not, because
/// `tauri.localhost` is a different host. [`crate::remote_access`] matches the
/// same way and says why for the allowlist half.
fn origin_host(origin: &str) -> Option<&str> {
    let after_scheme = match origin.split_once("://") {
        Some((scheme, rest)) if !scheme.is_empty() => rest,
        _ => return None,
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

    (!host.is_empty()).then_some(host)
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
///
/// Shared with [`crate::http`], because every error in the contract's
/// `EnvironmentHttpCommonError` union carries one of these and they all have to
/// look alike.
pub(crate) fn trace_id() -> String {
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
    use crate::remote_access::RemoteAccess;

    fn request<'a>() -> UpgradeRequest<'a> {
        UpgradeRequest::default()
    }

    /// The ordinary machine: loopback, and nothing else named.
    fn loopback_only() -> RemoteAccess {
        RemoteAccess::none()
    }

    /// A machine whose owner has put a tunnel in front of laplus.
    fn tunnelled() -> RemoteAccess {
        RemoteAccess::from_hosts(["phone.example"])
    }

    fn shape(request: UpgradeRequest<'_>, allowed: &RemoteAccess) -> Credential {
        authorize(request, allowed).expect("admitted").shape
    }

    /// The browser UI's shape, from `01-browser-session.ndjson`.
    #[test]
    fn the_browsers_cookie_and_origin_are_admitted() {
        let admitted = authorize(
            UpgradeRequest {
                origin: Some("http://127.0.0.1:3999"),
                cookie: Some("t3_session=eyJ2Ijox.c2ln"),
                ..request()
            },
            &loopback_only(),
        )
        .expect("admitted");

        assert_eq!(admitted.shape, Credential::SessionCookie);
        // The token travels with the shape, because the caller has to verify it
        // and reaching back into the header to find it again would be a second
        // reading that could disagree with this one.
        assert_eq!(admitted.token, "eyJ2Ijox.c2ln");
    }

    /// The non-browser shape, from `02-request-response.ndjson`. Note it sends
    /// no `Origin` at all.
    #[test]
    fn a_websocket_ticket_with_no_origin_is_admitted() {
        let admitted = authorize(
            UpgradeRequest {
                query: Some("wsTicket=eyJ2Ijox.c2ln"),
                ..request()
            },
            &loopback_only(),
        )
        .expect("admitted");

        assert_eq!(admitted.shape, Credential::WebSocketTicket);
        assert_eq!(admitted.token, "eyJ2Ijox.c2ln");
    }

    #[test]
    fn authorization_headers_are_admitted_in_both_schemes() {
        assert_eq!(
            shape(
                UpgradeRequest {
                    authorization: Some("Bearer eyJ2Ijox.c2ln"),
                    ..request()
                },
                &loopback_only()
            ),
            Credential::BearerToken
        );
        assert_eq!(
            shape(
                UpgradeRequest {
                    authorization: Some("DPoP eyJ2Ijox.c2ln"),
                    ..request()
                },
                &loopback_only()
            ),
            Credential::DpopToken
        );
    }

    /// The ticket parameter wins over the other two, matching the reference
    /// server's precedence.
    #[test]
    fn the_ticket_parameter_takes_precedence() {
        let admitted = authorize(
            UpgradeRequest {
                query: Some("wsTicket=ticket"),
                authorization: Some("Bearer token"),
                cookie: Some("t3_session=cookie"),
                ..request()
            },
            &loopback_only(),
        )
        .expect("admitted");

        assert_eq!(admitted.shape, Credential::WebSocketTicket);
        assert_eq!(admitted.token, "ticket");
    }

    /// **This module no longer decides whether an absent credential is
    /// acceptable, and that is the point of ticket 73.**
    ///
    /// It reports `Absent` and admits the request as far as the credential
    /// check, which then refuses it — `crate::server::authorized`. The split
    /// exists because the two questions have different answers for
    /// `/oauth/token`, which is how a client holding nothing comes to hold
    /// something.
    #[test]
    fn an_absent_credential_is_reported_rather_than_judged() {
        let admitted =
            authorize(request(), &loopback_only()).expect("the origin is not the problem");
        assert_eq!(admitted.shape, Credential::Absent);
        assert_eq!(admitted.token, "");
    }

    /// Empty values are the same as no value — this is the shape check, and an
    /// empty string is not the shape.
    #[test]
    fn empty_credentials_are_not_mistaken_for_present_ones() {
        assert_eq!(
            shape(
                UpgradeRequest {
                    query: Some("wsTicket=&other=1"),
                    cookie: Some("t3_session= ; unrelated=x"),
                    authorization: Some("Bearer  "),
                    ..request()
                },
                &loopback_only()
            ),
            Credential::Absent
        );
    }

    #[test]
    fn the_session_cookie_is_found_among_others() {
        let admitted = authorize(
            UpgradeRequest {
                cookie: Some("theme=dark; t3_session=eyJ2Ijox.c2ln; other=1"),
                ..request()
            },
            &loopback_only(),
        )
        .expect("admitted");
        assert_eq!(admitted.shape, Credential::SessionCookie);
        assert_eq!(admitted.token, "eyJ2Ijox.c2ln");
    }

    /// The check matches on **host** and ignores the scheme, which cuts the
    /// opposite way from what you would guess: `tauri://localhost` is admitted
    /// because its host is `localhost`, while `http://tauri.localhost` — the
    /// origin Tauri v2 actually uses on Windows — is refused, because
    /// `tauri.localhost` is neither `localhost` nor a `127.0.0.0/8` address.
    ///
    /// This was pinned so that ticket 23 would find out by reading a failure
    /// rather than by guessing, and it did its job: ticket 23 serves the UI
    /// from this server instead (see [`crate::ui`]), so the window's origin is
    /// the loopback address it was already pointed at and this stays as it is.
    #[test]
    fn the_origin_check_matches_on_host_and_ignores_the_scheme() {
        assert!(
            authorize(
                UpgradeRequest {
                    origin: Some("tauri://localhost"),
                    ..request()
                },
                &loopback_only()
            )
            .is_ok(),
            "a custom scheme on a loopback host is admitted"
        );
        assert!(
            authorize(
                UpgradeRequest {
                    origin: Some("http://tauri.localhost"),
                    ..request()
                },
                &loopback_only()
            )
            .is_err(),
            "tauri.localhost is a different host, and is refused"
        );
    }

    #[test]
    fn loopback_origins_are_admitted() {
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
                authorize(
                    UpgradeRequest {
                        origin: Some(origin),
                        ..request()
                    },
                    &loopback_only()
                )
                .is_ok(),
                "{origin} should be admitted"
            );
        }
    }

    /// Binding to loopback does not stop a page elsewhere from asking the
    /// user's browser to connect on its behalf, which is what this refuses.
    #[test]
    fn unnamed_origins_are_refused_even_with_a_valid_looking_credential() {
        for origin in [
            "https://evil.example",
            "http://127.0.0.1.evil.example",
            "http://localhost.evil.example",
            "http://192.168.1.10:3999",
            "null",
            "",
        ] {
            let refused = authorize(
                UpgradeRequest {
                    origin: Some(origin),
                    cookie: Some("t3_session=eyJ2Ijox.c2ln"),
                    ..request()
                },
                &loopback_only(),
            );
            assert!(refused.is_err(), "{origin} should be refused");
        }
    }

    /// The tunnel case: the host the user wrote down is admitted, and nothing
    /// else becomes admitted alongside it.
    #[test]
    fn a_named_host_is_admitted_and_its_neighbours_are_not() {
        assert!(
            authorize(
                UpgradeRequest {
                    origin: Some("https://phone.example"),
                    cookie: Some("t3_session=eyJ2Ijox.c2ln"),
                    ..request()
                },
                &tunnelled()
            )
            .is_ok(),
            "the host named in remote-access.json is admitted"
        );

        for origin in [
            "https://evil.example",
            "https://evil.phone.example",
            "https://phone.example.evil",
        ] {
            assert!(
                authorize(
                    UpgradeRequest {
                        origin: Some(origin),
                        cookie: Some("t3_session=eyJ2Ijox.c2ln"),
                        ..request()
                    },
                    &tunnelled()
                )
                .is_err(),
                "{origin} should be refused"
            );
        }
    }

    /// **An acceptance criterion**: an origin absent from the allowlist is
    /// refused even holding a credential that would otherwise verify. The
    /// origin is settled before anything looks at what was presented, so there
    /// is no credential good enough to get past it.
    #[test]
    fn an_unnamed_origin_is_refused_before_its_credential_is_even_reported() {
        let refused = authorize(
            UpgradeRequest {
                origin: Some("https://evil.example"),
                query: Some("wsTicket=a-ticket-that-would-verify"),
                ..request()
            },
            &tunnelled(),
        );
        assert!(refused.is_err());
    }

    /// The body the client already knows how to read — the shape captured in
    /// `06-upgrade-rejected.ndjson`.
    #[test]
    fn a_refusal_renders_the_captured_error_body() {
        let refused = authorize(
            UpgradeRequest {
                origin: Some("https://evil.example"),
                ..request()
            },
            &loopback_only(),
        )
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

    /// The union's other member, which only the routes build. A phone whose
    /// storage was cleared and a phone whose session expired are different
    /// things to tell the user.
    #[test]
    fn a_missing_credential_is_reported_as_its_own_reason() {
        let refusal = Rejection::missing_credential("nothing was presented");
        assert_eq!(refusal.body_value()["reason"], "missing_credential");
        assert_eq!(
            Rejection::invalid_credential("something was").body_value()["reason"],
            "invalid_credential"
        );
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
