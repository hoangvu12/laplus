//! The two plain HTTP answers the UI needs before it will open a socket.
//!
//! Everything the UI *does* goes over `/ws`. But it will not get that far
//! without these, and both are part of the local handshake rather than a
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
//! **Unlike everything in [`crate::wire`] and [`crate::config`], these two are
//! written from the contract only.** Ticket 01's proxy recorded `/ws`
//! connections and nothing else, so there is no capture to conform to here —
//! only `EnvironmentMetadataHttpApi` and `EnvironmentAuthHttpApi` in
//! `t3code/packages/contracts/src/environmentHttp.ts`. That is a weaker
//! footing than the rest of this ticket stands on, and worth knowing when one
//! of them turns out to be wrong.
//!
//! Neither requires a credential, matching upstream: the descriptor group has
//! no auth middleware, and the session endpoint's whole job is to report
//! whether a credential was present.

use serde::Serialize;
use serde_json::Value;

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
