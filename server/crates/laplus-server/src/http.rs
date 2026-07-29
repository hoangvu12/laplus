//! The plain HTTP answers the UI wants beside the socket.
//!
//! Everything the UI *does* goes over `/ws`. Two of these it needs before it
//! will get that far, and both are part of the local handshake rather than a
//! feature surface:
//!
//! - `GET /.well-known/t3/environment` — "what am I talking to?" The UI fetches
//!   this before it registers a connection at all. No descriptor means no
//!   connection catalogue entry, means no supervisor, means the socket is never
//!   opened. The failure is swallowed and retried every 3 s, so a server
//!   missing this looks like a UI that simply never connects.
//! - `GET /api/auth/session` — "am I signed in?" The root route awaits this
//!   before rendering, so a server missing it leaves the window blank.
//!
//! **Unlike everything in [`crate::wire`] and [`crate::config`], those two are
//! written from the contract only.** Ticket 01's proxy recorded `/ws`
//! connections and nothing else, so there is no capture to conform to here —
//! only `EnvironmentMetadataHttpApi` and `EnvironmentAuthHttpApi` in
//! `packages/contracts/src/environmentHttp.ts`. That is a weaker footing than
//! the rest of that ticket stands on, and worth knowing when one of them turns
//! out to be wrong.
//!
//! Neither requires a credential, matching upstream: the descriptor group has
//! no auth middleware, and the session endpoint's whole job is to report
//! whether a credential was present.
//!
//! ## The two snapshots (ticket 31)
//!
//! - `GET /api/orchestration/shell` — the project list and the thread list.
//! - `GET /api/orchestration/threads/{threadId}` — one conversation.
//!
//! Neither is a capability. The socket already carries both payloads and the
//! client falls back to it when the fetch fails, so what these buy is that the
//! fetch is no longer a guaranteed miss: the client's own comment says the
//! response "is gzip-compressible by the transport and keeps the (potentially
//! multi-KB) snapshot off the socket". Until they existed, every snapshot cost
//! a failed round trip and put a 404 and a warning in the console that
//! `tools/ui-driver` reads, which is this repository's only instrument for the
//! UI half.
//!
//! These two stand on firmer ground than the two above.
//! `EnvironmentOrchestrationHttpApi` pins them as a versioned in-tree schema —
//! paths, params, headers, auth middleware, success types and the error union —
//! and it is the same contract the client's typed API client is built from, so
//! no capture would tell us more than it does. The payloads themselves are
//! not written from the contract at all: they are
//! [`crate::orchestration::Shell::shell_snapshot`] and
//! [`crate::threads::Threads::detail_snapshot`], the objects the socket has been
//! sending since tickets 05 and 10.
//!
//! What lives here is only the *refusals*, because the bodies are the whole of
//! what the routes add. The credential check is [`crate::auth::authorize`]'s,
//! unchanged and shared with the socket upgrade.

use serde::Serialize;
use serde_json::{json, Value};

use crate::config::{AuthDescriptor, EnvironmentDescriptor, ServerConfig};

/// `GET /.well-known/t3/environment`.
///
/// The same descriptor `server.getConfig` carries, so a client cannot see two
/// different answers to "which machine is this?" depending on which it asked.
pub fn environment_descriptor(config: &ServerConfig) -> &EnvironmentDescriptor {
    &config.environment
}

/// `GET /api/auth/session`.
///
/// ## This used to answer `authenticated: true` to everyone
///
/// It did, and said so: v1 had no identity store, so there was no state in
/// which a client was *un*authenticated and answering `false` would have sent
/// the UI to a pairing screen backed by nothing. Ticket 73 built the identity
/// store, and left this route behind — its scope item 7 is exactly "stop
/// hardcoding `authenticated: true`", and only the `bootstrapMethods` half of
/// it landed.
///
/// Leaving it cost more than a wrong field. **`scopes` is what the Settings
/// panel gates its entire local-environment section on** — `canManageLocalBackend`
/// in `ConnectionsSettings.tsx` is `scopes?.includes("access:write")`, and a
/// response that omits `scopes` reads as a client that may manage nothing. So
/// the window, holding a boot grant minted with every administrative scope,
/// was shown a panel saying it lacked them, and the button that mints a pairing
/// code was in the half that panel does not render. The server could pair a
/// phone; there was no way to ask it to.
///
/// A `false` here is not a refusal — this route is the probe a client makes
/// *before* it holds anything, and 200 is the honest answer to "am I signed
/// in?" when the answer is no. The client already loops on it:
/// `waitForAuthenticatedSessionAfterBootstrap` in
/// `apps/web/src/environments/primary/auth.ts` polls until it flips.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionState<'a> {
    pub authenticated: bool,
    pub auth: &'a AuthDescriptor,
    /// What the verified session may do — the grant's, verbatim.
    ///
    /// Absent rather than empty when nothing verified: the contract types it
    /// `optionalKey`, and an empty array is a client that authenticated and may
    /// do nothing, which is a different sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// Which of `ServerAuthSessionMethod`'s three established this session.
    ///
    /// Absent for a `wsTicket`, which is not one of the three: a ticket is how
    /// a socket is opened, not how a session is held. Nothing asks this route
    /// with one, but [`crate::server::authorized`] accepts one everywhere, so
    /// the shape has to have an answer that is not a guess.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_method: Option<&'static str>,
    // `expiresAt` is the third optional field and stays omitted. Reporting it
    // means carrying the session's expiry out of the store alongside the grant,
    // which nothing reads: the client re-probes rather than counting down.
}

/// Nothing verified. The shape a client sees before it has paired.
pub fn unauthenticated_session(config: &ServerConfig) -> AuthSessionState<'_> {
    AuthSessionState {
        authenticated: false,
        auth: &config.auth,
        scopes: None,
        session_method: None,
    }
}

/// A credential verified, and this is what it is good for.
pub fn authenticated_session<'a>(
    config: &'a ServerConfig,
    scopes: Vec<String>,
    session_method: Option<&'static str>,
) -> AuthSessionState<'a> {
    AuthSessionState {
        authenticated: true,
        auth: &config.auth,
        scopes: Some(scopes),
        session_method,
    }
}

impl AuthSessionState<'_> {
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("auth session state serializes")
    }
}

/// A snapshot request this server will not answer, as the contract's
/// `EnvironmentHttpCommonError` union types the refusal.
///
/// Every member of that union is the same four fields — a `_tag`, a machine
/// `code`, a `reason` from a closed set, and a correlation id — and each pins
/// its own status in `httpApiStatus`. Carrying the status here rather than
/// leaving it to the caller is what keeps the two together: a `not_found` body
/// returned with a 500 decodes on the client as neither.
///
/// The status is a `u16` and not `axum`'s `StatusCode` for the same reason
/// nothing else in this module or in [`crate::auth`] mentions the web
/// framework: the policy is testable without one, and the handler is the only
/// place that needs to know how a response is written.
///
/// The union's 401 is **not** built here. `EnvironmentAuthInvalidError` is
/// [`crate::auth::Rejection::body`]'s, because that one is pinned by a capture
/// — `fixtures/socket-wire/06-upgrade-rejected.ndjson` — rather than by the
/// schema, and the routes refuse a credential in exactly the same words the
/// socket upgrade does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub status: u16,
    tag: &'static str,
    code: &'static str,
    reason: &'static str,
    trace_id: String,
}

impl Refusal {
    fn new(status: u16, tag: &'static str, code: &'static str, reason: &'static str) -> Refusal {
        Refusal {
            status,
            tag,
            code,
            reason,
            trace_id: crate::auth::trace_id(),
        }
    }

    /// The JSON body to return with [`Refusal::status`].
    pub fn to_value(&self) -> Value {
        json!({
            "_tag": self.tag,
            "code": self.code,
            "reason": self.reason,
            "traceId": self.trace_id,
        })
    }
}

/// `GET /api/orchestration/threads/{threadId}` for a thread this server does
/// not hold.
///
/// **A typed 404 and not a bare one**, and the difference is the point of
/// answering at all. The client catches `EnvironmentResourceNotFoundError` by
/// tag and logs it at debug — "deferring to the socket subscription" — where
/// anything else it cannot decode becomes "Could not load the thread snapshot
/// over HTTP", at warning, in the console. A "New thread" pane asks about a
/// draft roughly four times a second, so the difference between those two is
/// whether that console is usable.
pub fn thread_not_found() -> Refusal {
    Refusal::new(
        404,
        "EnvironmentResourceNotFoundError",
        "not_found",
        "thread_not_found",
    )
}

/// The registry could not be read, so there is no shell snapshot to send.
///
/// `orchestration_snapshot_failed` is one of `EnvironmentInternalErrorReason`'s
/// members and names this exact case. The thread route has no counterpart
/// because it answers from memory and has nothing that can fail: a conversation
/// is either open or it is [`thread_not_found`].
pub fn shell_snapshot_unavailable() -> Refusal {
    Refusal::new(
        500,
        "EnvironmentInternalError",
        "internal_error",
        "orchestration_snapshot_failed",
    )
}

// --- ticket 73: the pairing routes ------------------------------------------

/// A `scope` parameter that is not an RFC 6749 scope list.
///
/// 400 rather than 401: the credential may well be good, the request is not.
/// `invalid_scope` is the contract's own member for it.
pub fn invalid_scope() -> Refusal {
    Refusal::new(
        400,
        "EnvironmentRequestInvalidError",
        "invalid_request",
        "invalid_scope",
    )
}

/// A `scope` parameter asking for something the pairing code did not grant.
///
/// A different mistake from [`invalid_scope`] and the contract gives it its own
/// member: the scope is spelled correctly and this server understands it, and
/// the code being spent simply was not minted with it. The client can tell the
/// user to mint a code that grants it; for `invalid_scope` there is nothing to
/// say but "this client and this server disagree".
pub fn scope_not_granted() -> Refusal {
    Refusal::new(
        400,
        "EnvironmentRequestInvalidError",
        "invalid_request",
        "scope_not_granted",
    )
}

/// A request this server cannot read as the payload the contract declares for
/// the route.
///
/// Two cases wear it. A token-exchange body naming a grant this server does not
/// implement — the contract types `grant_type`, `subject_token_type` and
/// `requested_token_type` as literals, so anything else is a client that has
/// been changed without this server. And a JSON body that is not the object the
/// route's payload schema describes, which is the same thing arriving a
/// different way.
///
/// `invalid_command` is the nearest member of `EnvironmentRequestInvalidReason`.
/// There is no `unsupported_grant_type` and no `malformed_payload`; the closed
/// union is what the client decodes, so the nearest member is the answer and
/// the exact cause goes to the log.
pub fn unsupported_request() -> Refusal {
    Refusal::new(
        400,
        "EnvironmentRequestInvalidError",
        "invalid_request",
        "invalid_command",
    )
}

/// The database would not answer while minting, listing, revoking or spending.
///
/// Five reasons rather than one because `EnvironmentInternalErrorReason` names
/// all five, and the client logs the reason: a user who reports "it said
/// pairing_links_load_failed" has told us which statement broke.
pub fn pairing_credential_unavailable() -> Refusal {
    internal("pairing_credential_issuance_failed")
}

pub fn pairing_links_unavailable() -> Refusal {
    internal("pairing_links_load_failed")
}

pub fn pairing_link_revoke_failed() -> Refusal {
    internal("pairing_link_revoke_failed")
}

pub fn access_token_unavailable() -> Refusal {
    internal("access_token_issuance_failed")
}

pub fn websocket_ticket_unavailable() -> Refusal {
    internal("websocket_ticket_issuance_failed")
}

/// The database would not answer while checking a credential.
///
/// **A 500 and not the 401 beside it**, which is the distinction worth keeping:
/// a credential that failed to verify and a credential that could not be
/// verified are different events, and only the first is the user's to fix.
/// Answering 401 for a disk error would send somebody to re-pair a phone that
/// was never the problem.
pub fn credential_verification_failed() -> Refusal {
    internal("internal_error")
}

fn internal(reason: &'static str) -> Refusal {
    Refusal::new(500, "EnvironmentInternalError", "internal_error", reason)
}

/// `POST /oauth/token` — the contract's `AuthAccessTokenResult`.
///
/// `token_type` is `Bearer` and not `DPoP`: this server does not implement
/// proof-of-possession, which ticket 73 puts out of scope, and saying `DPoP`
/// would be advertising a check that is not made.
pub fn access_token(session: &crate::pairing::Session) -> Value {
    json!({
        "access_token": session.token,
        "issued_token_type": "urn:ietf:params:oauth:token-type:access_token",
        "token_type": "Bearer",
        "expires_in": session.expires_in,
        "scope": crate::pairing::encode_scopes(&session.scopes),
    })
}

/// `POST /api/auth/browser-session` — the contract's
/// `AuthBrowserSessionResult`.
///
/// `authenticated` is `Schema.Literal(true)`, so there is no shape of this body
/// that reports a failure: a credential that did not verify is the 401, not a
/// `false` here.
pub fn browser_session(session: &crate::pairing::Session) -> Value {
    json!({
        "authenticated": true,
        "scopes": session.scopes,
        "sessionMethod": crate::pairing::BROWSER_SESSION_METHOD,
        "expiresAt": session.expires_at,
    })
}

/// The `Set-Cookie` value that carries a browser session.
///
/// The reference server's attributes (`auth/http.ts:232-237`), with one
/// deliberate difference: `Max-Age` rather than `Expires`. Both express the same
/// thing, `Max-Age` wins wherever a browser sees both, and it is a count of
/// seconds — which is what [`crate::pairing::Session::expires_in`] already is,
/// computed by the database in the same statement as the expiry. Rendering an
/// `Expires` would mean formatting an HTTP-date, which means a second way of
/// saying when this session ends and a chance for the two to disagree.
///
/// - **`HttpOnly`**, so a cross-site scripting bug in the UI cannot read the
///   session out of `document.cookie`. Nothing in this application has any
///   reason to read it from JavaScript: the browser attaches it by itself.
/// - **`SameSite=Lax`**, which is what stops a page on another origin from
///   causing the browser to send this cookie along with a request it made up.
///   The origin check in [`crate::auth`] is the other half of that and neither
///   replaces the other — `Lax` is the browser refusing, the origin check is
///   this server refusing.
/// - **No `Secure`**, because the window reaches this server over plain HTTP on
///   loopback and a `Secure` cookie would never be stored at all. A browser
///   accepts a non-`Secure` cookie over HTTPS, so the tunnel case still works;
///   the reverse would not.
pub fn session_cookie(token: &str, max_age_seconds: i64) -> String {
    format!(
        "{name}={token}; Max-Age={max_age_seconds}; Path=/; HttpOnly; SameSite=Lax",
        name = crate::auth::SESSION_COOKIE_NAME
    )
}

/// `POST /api/auth/browser-session`'s body — the contract's
/// `AuthBrowserSessionRequest`. One required field.
pub fn read_browser_session_request(body: &str) -> Result<String, PayloadProblem> {
    let value: Value = serde_json::from_str(body).map_err(|_| PayloadProblem::Malformed)?;
    let credential = value
        .as_object()
        .and_then(|object| object.get("credential"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|credential| !credential.is_empty())
        .ok_or(PayloadProblem::Malformed)?;
    Ok(credential.to_string())
}

pub fn browser_session_unavailable() -> Refusal {
    internal("browser_session_issuance_failed")
}

/// `POST /api/auth/websocket-ticket` — the contract's
/// `AuthWebSocketTicketResult`.
pub fn websocket_ticket(ticket: &crate::pairing::WebSocketTicket) -> Value {
    json!({
        "ticket": ticket.ticket,
        "expiresAt": ticket.expires_at,
    })
}

/// `POST /api/auth/pairing-token` — the contract's
/// `AuthPairingCredentialResult`.
///
/// `label` is omitted rather than null when there is none, for the reason
/// [`AuthSessionState`]'s optional fields are: the contract types it
/// `optionalKey`, where absent decodes and null does not.
pub fn pairing_credential(link: &crate::pairing::PairingLink) -> Value {
    let mut body = json!({
        "id": link.id,
        "credential": link.credential,
        "expiresAt": link.expires_at,
    });
    if let Some(label) = &link.label {
        body["label"] = json!(label);
    }
    body
}

/// `GET /api/auth/pairing-links` — an array of the contract's
/// `AuthPairingLink`.
pub fn pairing_links(links: &[crate::pairing::PairingLink]) -> Value {
    Value::Array(
        links
            .iter()
            .map(|link| {
                let mut entry = json!({
                    "id": link.id,
                    "credential": link.credential,
                    "scopes": link.scopes,
                    "subject": link.subject,
                    "createdAt": link.created_at,
                    "expiresAt": link.expires_at,
                });
                if let Some(label) = &link.label {
                    entry["label"] = json!(label);
                }
                entry
            })
            .collect(),
    )
}

/// `POST /api/auth/pairing-links/revoke` — the contract's
/// `AuthPairingLinkRevokeResult`.
pub fn pairing_link_revoked(revoked: bool) -> Value {
    json!({ "revoked": revoked })
}

/// Why a JSON request body is not the payload the contract declares.
///
/// Two cases and not one because the routes answer them differently: a
/// malformed body is [`unsupported_request`] where the contract allows a 400
/// and something else where it does not, while a bad scope list is always
/// [`invalid_scope`]. See the handlers in [`crate::server`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadProblem {
    /// Not JSON, not an object, or a field of the wrong type.
    Malformed,
    /// A `scopes` array that is empty, repeats itself, or names something
    /// outside [`crate::pairing::ENVIRONMENT_SCOPES`].
    InvalidScope,
}

/// `POST /api/auth/pairing-token`'s body — the contract's
/// `AuthCreatePairingCredentialInput`, with its two optional fields resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingCredentialRequest {
    pub label: Option<String>,
    /// Never empty: an absent `scopes` becomes
    /// [`crate::pairing::default_scopes`] here rather than at the database, so
    /// that the row and the response describe the same grant.
    pub scopes: Vec<String>,
}

/// Read that body.
///
/// Hand-written against `serde_json::Value` rather than derived, for the reason
/// [`form_fields`] is hand-written: a `#[derive(Deserialize)]` behind `axum`'s
/// `Json` extractor answers a body it cannot decode with the framework's own
/// 422, and every refusal this server makes has to be in the contract's shape.
///
/// **An empty body is an empty object.** Both of this payload's fields are
/// optional, so `{}` and nothing are the same request, and a client that sends
/// no body for a payload with nothing required is not wrong.
///
/// A `label` that is blank once trimmed is treated as absent rather than
/// refused. The contract types it `TrimmedNonEmptyString`, so a blank one is a
/// client that did not trim — and the useful answer to that is the pairing code
/// they asked for, not a 400 naming a field they left empty.
pub fn read_pairing_credential_request(body: &str) -> Result<PairingCredentialRequest, PayloadProblem> {
    let value: Value = if body.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(body).map_err(|_| PayloadProblem::Malformed)?
    };
    let object = value.as_object().ok_or(PayloadProblem::Malformed)?;

    let label = match object.get("label") {
        None | Some(Value::Null) => None,
        Some(Value::String(label)) => Some(label.trim()).filter(|label| !label.is_empty()).map(str::to_string),
        Some(_) => return Err(PayloadProblem::Malformed),
    };

    let scopes = match object.get("scopes") {
        None | Some(Value::Null) => crate::pairing::default_scopes(),
        Some(Value::Array(entries)) => {
            let mut scopes: Vec<String> = Vec::with_capacity(entries.len());
            for entry in entries {
                let scope = entry.as_str().ok_or(PayloadProblem::Malformed)?;
                // Empty, repeated and unknown are all `invalid_scope`, matching
                // the reference server's own check at this endpoint
                // (`auth/http.ts:337-345`). A repeat is refused rather than
                // deduplicated because the client builds this array from a
                // checkbox list: one arriving twice means the list is wrong,
                // and quietly fixing it hides that.
                if !crate::pairing::is_environment_scope(scope) || scopes.iter().any(|seen| seen == scope) {
                    return Err(PayloadProblem::InvalidScope);
                }
                scopes.push(scope.to_string());
            }
            if scopes.is_empty() {
                return Err(PayloadProblem::InvalidScope);
            }
            scopes
        }
        Some(_) => return Err(PayloadProblem::Malformed),
    };

    Ok(PairingCredentialRequest { label, scopes })
}

/// `POST /api/auth/pairing-links/revoke`'s body — the contract's
/// `AuthRevokePairingLinkInput`. One required field, so unlike the payload
/// above there is no empty-body case: nothing names nothing to revoke.
pub fn read_revoke_pairing_link_request(body: &str) -> Result<String, PayloadProblem> {
    let value: Value = serde_json::from_str(body).map_err(|_| PayloadProblem::Malformed)?;
    let id = value
        .as_object()
        .and_then(|object| object.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or(PayloadProblem::Malformed)?;
    Ok(id.to_string())
}

/// Read an `application/x-www-form-urlencoded` body.
///
/// Hand-written for the same reason [`crate::auth`] reads its own cookies:
/// `axum`'s `Form` rejects a body it cannot decode with its own 422, and this
/// server answers every refusal in the contract's shape or the client cannot
/// read it. One route posts a form — `/oauth/token`, because RFC 6749 says so —
/// so this is thirty lines rather than a dependency.
pub fn form_fields(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) => (percent_decode(key), percent_decode(value)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

/// `+` is a space and `%XX` is a byte, which is the whole of the encoding.
///
/// A malformed escape is kept verbatim rather than dropped: this decodes
/// credentials, and silently altering one turns "your code was mistyped" into
/// "your code was refused for no stated reason".
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let high = (bytes[index + 1] as char).to_digit(16);
                let low = (bytes[index + 2] as char).to_digit(16);
                match (high, low) {
                    (Some(high), Some(low)) => {
                        out.push((high * 16 + low) as u8);
                        index += 3;
                    }
                    _ => {
                        out.push(bytes[index]);
                        index += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// The one field of a form this server reads more than once.
pub fn form_field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The descriptor is reachable two ways and has to read the same both
    /// times — the UI compares the id it discovered over HTTP against the one
    /// the socket reports, and a mismatch reads as connecting to the wrong
    /// machine.
    #[test]
    fn the_descriptor_matches_the_one_in_the_server_config() {
        let config = ServerConfig::detect();
        let over_http = serde_json::to_value(environment_descriptor(&config)).expect("serializes");
        assert_eq!(over_http, config.to_value()["environment"]);
    }

    /// Both shapes carry the descriptor, and it is the config's either way: a
    /// client that has not paired reads `bootstrapMethods` off this response to
    /// know *how* to pair, so the unauthenticated answer is the one that can
    /// least afford to describe a different server.
    #[test]
    fn the_session_state_reports_the_same_auth_descriptor_as_the_config() {
        let config = ServerConfig::detect();

        let signed_out = unauthenticated_session(&config).to_value();
        assert_eq!(signed_out["authenticated"], serde_json::json!(false));
        assert_eq!(signed_out["auth"], config.to_value()["auth"]);

        let signed_in = authenticated_session(
            &config,
            vec!["access:write".to_string()],
            Some(crate::pairing::BROWSER_SESSION_METHOD),
        )
        .to_value();
        assert_eq!(signed_in["authenticated"], serde_json::json!(true));
        assert_eq!(signed_in["auth"], config.to_value()["auth"]);
    }

    /// The one field the Settings panel gates on, spelled the way the contract
    /// spells it.
    ///
    /// `canManageLocalBackend` reads `scopes` and nothing else, so a rename to
    /// snake_case here would not fail a decode — `scopes` is `optionalKey` — it
    /// would silently put the panel back in the state this pair of functions
    /// exists to get it out of.
    #[test]
    fn a_verified_session_reports_its_scopes_and_how_it_was_established() {
        let config = ServerConfig::detect();
        let state = authenticated_session(
            &config,
            vec!["orchestration:read".to_string(), "access:write".to_string()],
            Some(crate::pairing::BEARER_SESSION_METHOD),
        )
        .to_value();

        assert_eq!(
            state["scopes"],
            serde_json::json!(["orchestration:read", "access:write"])
        );
        assert_eq!(state["sessionMethod"], serde_json::json!("bearer-access-token"));
    }

    /// The client decodes a refusal by its `_tag` and branches on it, so the
    /// four fields and the status they travel with are the whole of what these
    /// are. A `reason` outside the contract's closed set, or a status that does
    /// not match the body, fails the decode — and a failed decode is the
    /// generic warning these exist to avoid.
    #[test]
    fn each_refusal_carries_the_status_and_the_reason_the_contract_pins_for_it() {
        for (refusal, status, tag, code, reason) in [
            (
                thread_not_found(),
                404,
                "EnvironmentResourceNotFoundError",
                "not_found",
                "thread_not_found",
            ),
            (
                shell_snapshot_unavailable(),
                500,
                "EnvironmentInternalError",
                "internal_error",
                "orchestration_snapshot_failed",
            ),
        ] {
            assert_eq!(refusal.status, status);
            let body = refusal.to_value();
            assert_eq!(body["_tag"], serde_json::json!(tag));
            assert_eq!(body["code"], serde_json::json!(code));
            assert_eq!(body["reason"], serde_json::json!(reason));
            let trace_id = body["traceId"].as_str().expect("a traceId");
            assert_eq!(trace_id.len(), 32);
            assert!(trace_id.chars().all(|digit| digit.is_ascii_hexdigit()));
        }
    }

    /// The handle that makes a refusal in the window findable in the log. Two
    /// refusals sharing one would be two events that cannot be told apart.
    #[test]
    fn two_refusals_do_not_share_a_trace_id() {
        assert_ne!(
            thread_not_found().to_value()["traceId"],
            thread_not_found().to_value()["traceId"]
        );
    }

    // --- ticket 73 ----------------------------------------------------------

    /// The encoding `/oauth/token` arrives in. Hand-written, so the cases that
    /// would otherwise be a library's problem are this module's.
    #[test]
    fn a_form_body_decodes_its_escapes_and_its_plus_signs() {
        let fields = form_fields(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange\
             &subject_token=ABCD2345WXYZ&client_label=My+Phone&empty=&bare",
        );
        assert_eq!(
            form_field(&fields, "grant_type"),
            Some("urn:ietf:params:oauth:grant-type:token-exchange")
        );
        assert_eq!(form_field(&fields, "subject_token"), Some("ABCD2345WXYZ"));
        assert_eq!(form_field(&fields, "client_label"), Some("My Phone"));
        assert_eq!(form_field(&fields, "empty"), Some(""));
        assert_eq!(form_field(&fields, "bare"), Some(""));
        assert_eq!(form_field(&fields, "absent"), None);
    }

    /// A malformed escape is kept verbatim rather than dropped. This decodes
    /// credentials: silently altering one turns "your code was mistyped" into
    /// "your code was refused and nobody will say why".
    #[test]
    fn a_malformed_escape_survives_rather_than_changing_the_credential() {
        let fields = form_fields("subject_token=AB%ZZ&trailing=CD%2");
        assert_eq!(form_field(&fields, "subject_token"), Some("AB%ZZ"));
        assert_eq!(form_field(&fields, "trailing"), Some("CD%2"));
    }

    #[test]
    fn an_empty_form_body_has_no_fields() {
        assert!(form_fields("").is_empty());
    }

    /// Both fields optional means an absent body and `{}` are one request.
    #[test]
    fn an_absent_pairing_payload_is_the_standard_grant_with_no_label() {
        for body in ["", "   ", "{}"] {
            let request = read_pairing_credential_request(body).expect("reads");
            assert_eq!(request.label, None, "{body:?}");
            assert_eq!(request.scopes, crate::pairing::default_scopes(), "{body:?}");
        }
    }

    /// A label that is blank once trimmed is absent rather than refused. The
    /// useful answer to a client that did not trim is the code they asked for.
    #[test]
    fn a_blank_label_is_treated_as_no_label() {
        for body in [r#"{"label":"  "}"#, r#"{"label":""}"#, r#"{"label":null}"#] {
            assert_eq!(
                read_pairing_credential_request(body).expect("reads").label,
                None,
                "{body}"
            );
        }
        assert_eq!(
            read_pairing_credential_request(r#"{"label":"  Phone  "}"#)
                .expect("reads")
                .label,
            Some("Phone".to_string())
        );
    }

    #[test]
    fn a_pairing_payload_that_is_not_the_shape_is_malformed() {
        for body in [
            "not json",
            "[]",
            "7",
            r#"{"label":7}"#,
            r#"{"scopes":"relay:read"}"#,
            r#"{"scopes":[7]}"#,
        ] {
            assert_eq!(
                read_pairing_credential_request(body),
                Err(PayloadProblem::Malformed),
                "{body}"
            );
        }
    }

    /// Empty, repeated and unknown are one refusal, matching the reference
    /// server's check at this endpoint.
    #[test]
    fn a_scope_list_that_is_empty_repeated_or_unknown_is_an_invalid_scope() {
        for body in [
            r#"{"scopes":[]}"#,
            r#"{"scopes":["relay:read","relay:read"]}"#,
            r#"{"scopes":["orchestration:destroy"]}"#,
            r#"{"scopes":[""]}"#,
        ] {
            assert_eq!(
                read_pairing_credential_request(body),
                Err(PayloadProblem::InvalidScope),
                "{body}"
            );
        }
    }

    /// Order is the client's, because the client built it from a checkbox list
    /// and Settings displays it back.
    #[test]
    fn a_scope_list_keeps_the_order_it_arrived_in() {
        let request =
            read_pairing_credential_request(r#"{"scopes":["relay:read","orchestration:read"]}"#)
                .expect("reads");
        assert_eq!(
            request.scopes,
            vec!["relay:read".to_string(), "orchestration:read".to_string()]
        );
    }

    #[test]
    fn a_revoke_payload_needs_an_id_that_is_a_non_empty_string() {
        assert_eq!(
            read_revoke_pairing_link_request(r#"{"id":"  abc  "}"#),
            Ok("abc".to_string())
        );
        for body in ["", "{}", r#"{"id":""}"#, r#"{"id":"  "}"#, r#"{"id":7}"#, "[]"] {
            assert_eq!(
                read_revoke_pairing_link_request(body),
                Err(PayloadProblem::Malformed),
                "{body}"
            );
        }
    }

    /// The five ticket-73 refusals, each with the status its body claims. Same
    /// property as the test above and the same reason: a `scope_not_granted`
    /// body returned with a 500 decodes on the client as neither.
    #[test]
    fn the_pairing_refusals_carry_the_status_and_reason_the_contract_pins() {
        for (refusal, status, tag, reason) in [
            (invalid_scope(), 400, "EnvironmentRequestInvalidError", "invalid_scope"),
            (
                scope_not_granted(),
                400,
                "EnvironmentRequestInvalidError",
                "scope_not_granted",
            ),
            (
                unsupported_request(),
                400,
                "EnvironmentRequestInvalidError",
                "invalid_command",
            ),
            (
                pairing_credential_unavailable(),
                500,
                "EnvironmentInternalError",
                "pairing_credential_issuance_failed",
            ),
            (
                pairing_links_unavailable(),
                500,
                "EnvironmentInternalError",
                "pairing_links_load_failed",
            ),
            (
                pairing_link_revoke_failed(),
                500,
                "EnvironmentInternalError",
                "pairing_link_revoke_failed",
            ),
            (
                access_token_unavailable(),
                500,
                "EnvironmentInternalError",
                "access_token_issuance_failed",
            ),
            (
                websocket_ticket_unavailable(),
                500,
                "EnvironmentInternalError",
                "websocket_ticket_issuance_failed",
            ),
        ] {
            assert_eq!(refusal.status, status, "{reason}");
            let body = refusal.to_value();
            assert_eq!(body["_tag"], serde_json::json!(tag), "{reason}");
            assert_eq!(body["reason"], serde_json::json!(reason));
        }
    }

    /// Optional fields are omitted, not sent as null: the contract types them
    /// as `optionalKey`, where absent decodes cleanly and null does not.
    #[test]
    fn unset_optional_fields_are_absent_rather_than_null() {
        let config = ServerConfig::detect();
        // `serde_json::Value` orders keys itself, so compare the set.
        let fields = |state: serde_json::Value| -> Vec<String> {
            state
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect()
        };

        assert_eq!(
            fields(unauthenticated_session(&config).to_value()),
            ["auth", "authenticated"]
        );

        // A `wsTicket` names no `ServerAuthSessionMethod`, so that field drops
        // out while `scopes` stays — the two are independently optional and a
        // single `Option` over both would have made this shape unreachable.
        assert_eq!(
            fields(authenticated_session(&config, vec!["access:read".to_string()], None).to_value()),
            ["auth", "authenticated", "scopes"]
        );
    }
}
