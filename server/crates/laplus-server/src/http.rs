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
#[derive(Debug, Clone, Serialize)]
pub struct AuthSessionState<'a> {
    /// Always true. v1 has no identity store, so there is no state in which a
    /// local client is *un*authenticated — and answering `false` would send
    /// the UI to a pairing screen backed by a pairing flow that does not
    /// exist. This is the same permissive posture [`crate::auth`] takes at the
    /// socket upgrade, and it is bounded the same way: by binding to loopback
    /// and refusing non-local origins.
    pub authenticated: bool,
    pub auth: &'a AuthDescriptor,
    // `scopes`, `sessionMethod` and `expiresAt` are optional in the contract
    // and omitted here. Nothing is scoped because nothing is denied; no method
    // established the session; and it does not expire. The UI reads `scopes`
    // only to display them, with a null fallback.
}

pub fn auth_session_state(config: &ServerConfig) -> AuthSessionState<'_> {
    AuthSessionState {
        authenticated: true,
        auth: &config.auth,
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

    #[test]
    fn the_session_state_reports_the_same_auth_descriptor_as_the_config() {
        let config = ServerConfig::detect();
        let state = auth_session_state(&config).to_value();
        assert_eq!(state["authenticated"], serde_json::json!(true));
        assert_eq!(state["auth"], config.to_value()["auth"]);
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

    /// Optional fields are omitted, not sent as null: the contract types them
    /// as `optionalKey`, where absent decodes cleanly and null does not.
    #[test]
    fn unset_optional_fields_are_absent_rather_than_null() {
        let state = auth_session_state(&ServerConfig::detect()).to_value();
        // `serde_json::Value` orders keys itself, so compare the set.
        let fields: Vec<&str> = state
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(fields, ["auth", "authenticated"]);
    }
}
