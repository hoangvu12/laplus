//! Method dispatch: a request tag in, an answer out.
//!
//! The vocabulary is roughly sixty methods. Two are implemented — the
//! configuration the UI fetches before it can do anything else, and the
//! subscription that keeps it current — and every other tag lands in the
//! unknown-method path, which is itself part of the contract and is pinned by
//! a capture.
//!
//! Nothing in a `Request` says whether the answer will be one value or a
//! stream of them; that is knowledge the method name carries and the client
//! already has. [`Answer`] is where the two part company.

use serde_json::Value;

use crate::config_store::ConfigStore;
use crate::subscriptions::EventSource;

/// The tag the UI sends first, and the tag it re-sends as a liveness probe
/// when the server does not advertise `connectionProbe`.
pub const SERVER_GET_CONFIG: &str = "server.getConfig";

/// The configuration subscription — the simplest of the eight the UI opens,
/// and the one ticket 04 proves the streaming mechanism on.
pub const SUBSCRIBE_SERVER_CONFIG: &str = "subscribeServerConfig";

/// What a method answers with.
#[derive(Debug)]
pub enum Answer {
    /// One value, one `Exit`. The whole of a unary call.
    Value(Value),
    /// A stream of values, chunked until the client cancels it.
    Stream(EventSource),
}

/// Why a call produced no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchError {
    /// No method is wired to this tag.
    UnknownMethod(String),
}

impl DispatchError {
    /// The typed error to put in an `Exit`/`Failure`'s `Fail` cause.
    ///
    /// **This is a deliberate divergence from the reference server, and it is
    /// the one place in this ticket where a capture is not followed.**
    ///
    /// The reference server answers an unknown tag with a bare `Defect`
    /// (`fixtures/socket-wire/03-typed-error.ndjson`), which carries no
    /// `requestId`. In the client that is not a scoped failure:
    /// `RpcClient.ts` handles it as `clearEntries(Exit.die(message.defect))`,
    /// which fails *every* in-flight request and *every* open subscription on
    /// the socket, and the connection supervisor then reconnects on a
    /// 1/2/4/8/16-second backoff. An `Exit` whose error fails to decode, by
    /// contrast, is caught per request — `decodeExit(...).matchCauseEffect`
    /// writes the failure back under the same `requestId` and nothing else is
    /// touched. Both readings are from `effect@4.0.0-beta.78` in the vendored
    /// checkout; this answers open question 4 in `docs/socket-wire-format.md`.
    ///
    /// The reference server can afford `Defect` because it implements every
    /// tag its client sends, so a `Defect` only ever answers a tag no real
    /// client uses. lightcode implements one method of roughly sixty, so
    /// during the build-out `Defect` would be the *normal* answer to the UI's
    /// own boot sequence — and each one would tear down the session. The
    /// ticket asks for an error "the client understands, rather than dropping
    /// the connection", and this is what that means in practice.
    ///
    /// The error is `_tag`-discriminated like every other error on this wire.
    /// It will not decode against the method's declared error union, which
    /// costs exactly one request.
    pub fn to_error(&self) -> Value {
        match self {
            DispatchError::UnknownMethod(tag) => serde_json::json!({
                "_tag": "ServerMethodNotImplementedError",
                "method": tag,
                "message": format!("Method not implemented by this server: {tag}"),
            }),
        }
    }
}

/// Answer one call.
///
/// Takes the configuration store rather than the whole server state because
/// that is all any implemented method reads today. Later tickets widen it.
pub fn dispatch(
    config: &ConfigStore,
    tag: &str,
    _payload: &Value,
) -> Result<Answer, DispatchError> {
    match tag {
        SERVER_GET_CONFIG => Ok(Answer::Value(config.current().to_value())),
        // The payload is an empty struct in the contract, so there is nothing
        // to read out of it and nothing that can be wrong with it.
        SUBSCRIBE_SERVER_CONFIG => Ok(Answer::Stream(config.subscribe())),
        unknown => Err(DispatchError::UnknownMethod(unknown.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;
    use serde_json::json;

    fn store() -> ConfigStore {
        ConfigStore::new(ServerConfig::detect())
    }

    fn value(answer: Answer) -> Value {
        match answer {
            Answer::Value(value) => value,
            other => panic!("expected a unary answer, got {other:?}"),
        }
    }

    #[test]
    fn get_config_returns_the_config() {
        let config = store();
        let answer = dispatch(&config, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        assert_eq!(value(answer), config.current().to_value());
    }

    /// The client re-sends `server.getConfig` as its liveness probe, so a
    /// second call has to answer identically rather than consume anything.
    #[test]
    fn get_config_is_repeatable() {
        let config = store();
        let first = dispatch(&config, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        let second = dispatch(&config, SERVER_GET_CONFIG, &json!({})).expect("dispatches");
        assert_eq!(value(first), value(second));
    }

    /// A subscription is dispatched by the same path as a unary call and only
    /// parts company at the answer.
    #[test]
    fn the_configuration_subscription_answers_with_a_stream() {
        let config = store();
        let answer = dispatch(&config, SUBSCRIBE_SERVER_CONFIG, &json!({})).expect("dispatches");
        assert!(matches!(answer, Answer::Stream(_)));
    }

    /// The tag has to survive into the error, because it is the only thing
    /// that tells a developer which of the sixty methods is missing.
    #[test]
    fn an_unknown_tag_becomes_a_typed_error_naming_the_method() {
        let config = store();
        let error = dispatch(&config, "orchestration.subscribeShell", &json!({}))
            .expect_err("not implemented");
        assert_eq!(
            error,
            DispatchError::UnknownMethod("orchestration.subscribeShell".to_string())
        );

        let payload = error.to_error();
        assert_eq!(payload["_tag"], "ServerMethodNotImplementedError");
        assert_eq!(payload["method"], "orchestration.subscribeShell");
        assert!(payload["message"]
            .as_str()
            .expect("a message")
            .contains("orchestration.subscribeShell"));
    }
}
