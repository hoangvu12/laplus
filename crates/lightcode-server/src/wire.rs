//! The socket wire format the UI speaks — the message vocabulary that rides
//! inside WebSocket text frames.
//!
//! Pure, like [`crate::protocol`]: types and `serde`, no I/O. Everything here
//! is written against `fixtures/socket-wire/`, captured from the reference
//! TypeScript server in ticket 01 and described in `docs/socket-wire-format.md`.
//! Where a capture and the upstream type definitions disagree, the capture
//! wins — the definitions come from `effect/unstable/rpc`, an explicitly
//! unstable module.
//!
//! Two facts from that document shape this module:
//!
//! - **The framing is the WebSocket framing.** One JSON object per unfragmented
//!   text frame; no length prefix, no delimiter, no envelope above the frame.
//!   So there is no codec here, only the messages.
//! - **Correlation is by `requestId` alone.** Never by ordering — the reference
//!   server genuinely answers concurrent calls out of order.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A message from the client.
///
/// `_tag`-discriminated, like everything on this wire. An unrecognised tag
/// folds into [`ClientMessage::Unrecognized`] rather than failing the parse,
/// for the same reason the CLI protocol's enums have catch-all arms: a client
/// that learns a new message must not be able to kill the connection.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "_tag")]
pub enum ClientMessage {
    /// Starts a call. Used identically for unary methods and for streaming
    /// subscriptions — nothing in the envelope distinguishes them.
    Request {
        /// Client-assigned, a decimal string, unique within the connection.
        /// Unary calls and subscriptions share one id space.
        id: String,
        /// The method name, e.g. `server.getConfig`.
        tag: String,
        #[serde(default)]
        payload: Value,
    },
    /// Streaming back-pressure: the client has consumed a `Chunk` and the
    /// server may send the next one. Real back-pressure, not an advisory —
    /// see `docs/socket-wire-format.md`.
    Ack {
        #[serde(rename = "requestId")]
        request_id: String,
    },
    /// Cancels an in-flight call. This is how a subscription unsubscribes.
    Interrupt {
        #[serde(rename = "requestId")]
        request_id: String,
    },
    /// Keepalive. The UI sends one every ~5 s.
    Ping,
    /// A `_tag` this build does not know. Counted as drift, never fatal.
    #[serde(other)]
    Unrecognized,
}

impl ClientMessage {
    /// Parse one frame's payload. `Err` is a malformed frame — not JSON, or
    /// JSON without a usable `_tag` — which the connection counts rather than
    /// dies on.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

/// A message to the client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "_tag")]
pub enum ServerMessage {
    /// The terminal response for a request id. Every `Request` gets exactly
    /// one, except the unknown-method case — see [`ServerMessage::Defect`].
    Exit {
        #[serde(rename = "requestId")]
        request_id: String,
        exit: Exit,
    },
    /// One batch of stream values. `values` is non-empty and **does batch**;
    /// a conforming client iterates it rather than assuming one value.
    ///
    /// Nothing emits this yet — ticket 04 brings the first subscription — but
    /// it is part of the vocabulary and is pinned by the fixtures, so it is
    /// spelled out here with the rest.
    Chunk {
        #[serde(rename = "requestId")]
        request_id: String,
        values: Vec<Value>,
    },
    /// A connection-level failure not attributable to a declared error type.
    ///
    /// Note the absence of a `requestId`: this is what the reference server
    /// sends for an unknown method tag, and it deliberately leaves the caller's
    /// request without an `Exit`. See `docs/socket-wire-format.md`, open
    /// question 4.
    Defect { defect: Value },
    /// Reply to `Ping`.
    Pong,
}

/// The outcome carried by an [`ServerMessage::Exit`].
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "_tag")]
pub enum Exit {
    Success { value: Value },
    /// Note `cause` is an *array*. Effect models a failure as a cause tree and
    /// encodes it flat; the entries are [`Cause`].
    Failure { cause: Vec<Cause> },
}

/// One entry in a failure's cause array.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "_tag")]
pub enum Cause {
    /// A declared, typed error. The value is itself `_tag`-discriminated.
    Fail { error: Value },
    /// The call was cancelled. A client-initiated unsubscribe terminates this
    /// way, and a client must read it as a normal end rather than an error.
    Interrupt {
        #[serde(rename = "fiberId")]
        fiber_id: u64,
    },
    /// An undeclared defect inside a call. Never observed in a capture; the
    /// reference server sends a bare [`ServerMessage::Defect`] instead.
    Die { defect: Value },
}

impl ServerMessage {
    /// A successful unary response.
    pub fn success(request_id: impl Into<String>, value: Value) -> Self {
        ServerMessage::Exit {
            request_id: request_id.into(),
            exit: Exit::Success { value },
        }
    }

    /// A typed-error response: `Exit`/`Failure` with a single `Fail` cause.
    pub fn failure(request_id: impl Into<String>, error: Value) -> Self {
        ServerMessage::Exit {
            request_id: request_id.into(),
            exit: Exit::Failure {
                cause: vec![Cause::Fail { error }],
            },
        }
    }

    /// Serialize to the text of one WebSocket frame.
    pub fn to_frame(&self) -> String {
        // The variants are plain data with no maps keyed by anything but
        // strings, so this cannot fail.
        serde_json::to_string(self).expect("server message serializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Verbatim from `fixtures/socket-wire/01-browser-session.ndjson` — the
    /// browser's first frame, trace fields and all.
    #[test]
    fn a_browser_request_parses_and_its_tracing_fields_are_ignored() {
        let frame = r#"{"_tag":"Request","id":"0","tag":"server.getConfig","payload":{},"traceId":"1091713e6fd4a7ca567589e5537d499a","spanId":"9f2023d48d079987","sampled":true,"headers":[]}"#;

        match ClientMessage::parse(frame).expect("parses") {
            ClientMessage::Request { id, tag, payload } => {
                assert_eq!(id, "0");
                assert_eq!(tag, "server.getConfig");
                assert_eq!(payload, json!({}));
            }
            other => panic!("expected Request, got {other:?}"),
        }
    }

    /// The scripted client omits `traceId`/`spanId`/`sampled` entirely and the
    /// reference server is content, so we must be too.
    #[test]
    fn a_request_without_tracing_fields_parses() {
        let frame = r#"{"_tag":"Request","id":"1","tag":"no.such.method","payload":{},"headers":[]}"#;
        assert!(matches!(
            ClientMessage::parse(frame).expect("parses"),
            ClientMessage::Request { .. }
        ));
    }

    #[test]
    fn ack_and_interrupt_carry_the_request_id() {
        let ack = ClientMessage::parse(r#"{"_tag":"Ack","requestId":"1"}"#).expect("parses");
        assert!(matches!(ack, ClientMessage::Ack { request_id } if request_id == "1"));

        let interrupt =
            ClientMessage::parse(r#"{"_tag":"Interrupt","requestId":"0"}"#).expect("parses");
        assert!(matches!(interrupt, ClientMessage::Interrupt { request_id } if request_id == "0"));
    }

    /// `Eof`, `ClientEnd` and `ClientProtocolError` are in the upstream
    /// vocabulary but were never captured. If one ever arrives it degrades to
    /// a counter, the same way an unrecognised CLI event does.
    #[test]
    fn an_unrecognised_tag_degrades_rather_than_failing_the_parse() {
        for frame in [
            r#"{"_tag":"Eof","requestId":"0"}"#,
            r#"{"_tag":"ClientEnd"}"#,
            r#"{"_tag":"SomethingInventedNextYear","whatever":1}"#,
        ] {
            assert!(matches!(
                ClientMessage::parse(frame).expect("parses"),
                ClientMessage::Unrecognized
            ));
        }
    }

    #[test]
    fn a_malformed_frame_is_a_parse_error_rather_than_a_panic() {
        assert!(ClientMessage::parse("not json at all").is_err());
        assert!(ClientMessage::parse(r#"{"no":"tag"}"#).is_err());
    }

    /// Byte-for-byte against the captured frames. Field order is part of what
    /// is compared, which is stricter than the protocol requires — but it is
    /// free, and it means a reordering shows up here rather than in a diff
    /// someone is reading by eye.
    #[test]
    fn server_messages_serialize_to_the_captured_frames() {
        assert_eq!(ServerMessage::Pong.to_frame(), r#"{"_tag":"Pong"}"#);

        assert_eq!(
            ServerMessage::success("0", json!({"ok": true})).to_frame(),
            r#"{"_tag":"Exit","requestId":"0","exit":{"_tag":"Success","value":{"ok":true}}}"#
        );

        assert_eq!(
            ServerMessage::Defect {
                defect: json!("Unknown request tag: no.such.method"),
            }
            .to_frame(),
            r#"{"_tag":"Defect","defect":"Unknown request tag: no.such.method"}"#
        );

        assert_eq!(
            ServerMessage::failure("0", json!({"_tag": "ProjectReadFileError"})).to_frame(),
            r#"{"_tag":"Exit","requestId":"0","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"_tag":"ProjectReadFileError"}}]}}"#
        );

        assert_eq!(
            ServerMessage::Exit {
                request_id: "0".into(),
                exit: Exit::Failure {
                    cause: vec![Cause::Interrupt { fiber_id: 2494 }],
                },
            }
            .to_frame(),
            r#"{"_tag":"Exit","requestId":"0","exit":{"_tag":"Failure","cause":[{"_tag":"Interrupt","fiberId":2494}]}}"#
        );

        assert_eq!(
            ServerMessage::Chunk {
                request_id: "1".into(),
                values: vec![json!({"type": "snapshot"})],
            }
            .to_frame(),
            r#"{"_tag":"Chunk","requestId":"1","values":[{"type":"snapshot"}]}"#
        );
    }
}
