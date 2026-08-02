//! Who may talk to this server: the reading half.
//!
//! This module answers one question — what did this request present? — and
//! answers it without touching a database or a web framework. The other half of
//! the answer, does the presented credential verify, is a database read and
//! lives in [`crate::server`] against [`crate::store`]. Neither half is
//! sufficient alone.
//!
//! The policy:
//!
//! - **Bind to loopback**, unless the user turned that off. Done where the
//!   listener is created, not here — [`crate::remote_access`].
//! - **Report what was presented, and verify none of it.** [`authorize`]
//!   returns a [`Presented`], which is an obligation rather than a permission.
//!
//! There is no third rule, and there used to be: this module refused an origin
//! that was neither this machine nor named by the user, and that check has been
//! removed. `Origin` is read off the upgrade and consulted by nothing.
//! [`authorize`] carries the reasoning and what it gives up; this list said the
//! opposite for a while after the code stopped doing it, which is the sort of
//! comment that is worse than none.
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
use sha2::{Digest, Sha256};

/// A conversation-scoped MCP bearer. It is deliberately neither serializable
/// nor printable; the provider adapter must explicitly expose it for one HTTP
/// header.
pub struct McpGrant(String);

impl McpGrant {
    pub(crate) fn expose(&self) -> &str { &self.0 }
    #[doc(hidden)]
    pub fn for_adapter(authorization: String) -> Self { Self(authorization) }
}

impl std::fmt::Debug for McpGrant {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[redacted]")
    }
}

#[derive(Clone, Copy)]
pub(crate) struct McpVerifier([u8; 32]);

impl McpVerifier {
    pub(crate) fn verifies(&self, authorization: &str) -> bool {
        let Some(secret) = authorization.strip_prefix("Bearer ") else { return false };
        Self::for_secret(secret).0 == self.0
    }

    fn for_secret(secret: &str) -> Self { Self(Sha256::digest(secret.as_bytes()).into()) }
}

pub(crate) fn mint_mcp_grant() -> Result<(McpGrant, McpVerifier), getrandom::Error> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes)?;
    let secret = bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    Ok((McpGrant(format!("Bearer {secret}")), McpVerifier::for_secret(&secret)))
}

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
    /// build one of these — it refuses nothing at all now that the origin rule
    /// is gone, and returns a [`Presented`] for its caller to check.
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
    /// `reason` carries only what the contract's `EnvironmentAuthInvalidReason`
    /// allows — a closed union of `missing_credential | invalid_credential` —
    /// so a refusal whose real cause is neither is reported as
    /// `invalid_credential` rather than given an invented third member, which
    /// would cost the client its ability to decode the body at all. The real
    /// cause is in [`Rejection::detail`], which stays server-side.
    ///
    /// This paragraph used to be about the origin check, which was the one
    /// refusal that had no honest member here. That check is gone and the
    /// argument outlived it: every refusal this server now makes is genuinely
    /// about a credential.
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
    /// **Read and consulted by nothing**, since the origin rule was removed —
    /// see [`authorize`]. Kept because it is what a refusal is logged with, and
    /// because ticket 02 of the headless-Linux effort needs the header when it
    /// answers a second origin with CORS.
    pub origin: Option<&'a str>,
    pub authorization: Option<&'a str>,
    pub cookie: Option<&'a str>,
}

/// What arrived, and the token to verify it by.
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

/// Report which credential a request carries.
///
/// **Every caller must then check it.** A `Presented` is not permission; see
/// its own note. Nothing is settled here — this reads a header and says what it
/// found.
///
/// ## Why there is no origin rule
///
/// There used to be one: a page could reach this server if it came from this
/// machine or from a host named in `remote-access.json`. Upstream has no such
/// rule — `apps/server`'s only origin handling is CORS, behind `devOrigin`, for
/// development (`pingdotgg/t3code:apps/server/src/http.ts:57-67`) — and this now
/// follows it.
///
/// What that gives up is worth stating plainly rather than leaving implied. The
/// check could never stop a program: `curl` sends whatever headers it likes, and
/// a browser omits `Origin` on a plain `GET`. What it did stop was *a page
/// somewhere else* driving the user's own browser into this server with the
/// session cookie attached. Without it, that is the credential's problem alone —
/// which is the posture upstream ships and `docs/adr/0019` already relies on for
/// everything that is not a browser.
pub fn authorize(request: UpgradeRequest<'_>) -> Presented<'_> {
    presented(request)
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

    fn request<'a>() -> UpgradeRequest<'a> {
        UpgradeRequest::default()
    }

    fn shape(request: UpgradeRequest<'_>) -> Credential {
        authorize(request).shape
    }

    /// The browser UI's shape, from `01-browser-session.ndjson`.
    #[test]
    fn the_browsers_cookie_and_origin_are_admitted() {
        let admitted = authorize(UpgradeRequest {
                origin: Some("http://127.0.0.1:3999"),
                cookie: Some("t3_session=eyJ2Ijox.c2ln"),
                ..request()
            });

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
        let admitted = authorize(UpgradeRequest {
                query: Some("wsTicket=eyJ2Ijox.c2ln"),
                ..request()
            });

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
            ),
            Credential::BearerToken
        );
        assert_eq!(
            shape(
                UpgradeRequest {
                    authorization: Some("DPoP eyJ2Ijox.c2ln"),
                    ..request()
                },
            ),
            Credential::DpopToken
        );
    }

    /// The ticket parameter wins over the other two, matching the reference
    /// server's precedence.
    #[test]
    fn the_ticket_parameter_takes_precedence() {
        let admitted = authorize(UpgradeRequest {
                query: Some("wsTicket=ticket"),
                authorization: Some("Bearer token"),
                cookie: Some("t3_session=cookie"),
                ..request()
            });

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
            authorize(request());
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
            ),
            Credential::Absent
        );
    }

    #[test]
    fn the_session_cookie_is_found_among_others() {
        let admitted = authorize(UpgradeRequest {
                cookie: Some("theme=dark; t3_session=eyJ2Ijox.c2ln; other=1"),
                ..request()
            });
        assert_eq!(admitted.shape, Credential::SessionCookie);
        assert_eq!(admitted.token, "eyJ2Ijox.c2ln");
    }

    /// The body the client already knows how to read — the shape captured in
    /// `06-upgrade-rejected.ndjson`.
    #[test]
    fn a_refusal_renders_the_captured_error_body() {
        let refused = Rejection::new("refused a request from origin https://evil.example");

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
