//! The socket endpoint: one `GET /ws` on loopback, and the connection loop
//! behind it.
//!
//! There is no REST surface. Everything the UI does goes through this one
//! endpoint, and — per ticket 01's captures — there is no transport-level
//! handshake either: the socket opens and the client's first frame is already
//! a `Request`.
//!
//! Requests are answered in arrival order. The protocol does not require that
//! (correlation is by `requestId`, and the reference server genuinely answers
//! out of order), it is simply what a single implemented method needs. The
//! first method that has to wait on something is the one that should make this
//! concurrent.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{RawQuery, State, WebSocketUpgrade};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::watch;

use crate::auth::{self, Credential, UpgradeRequest};
use crate::config::ServerConfig;
use crate::{http, rpc};
use crate::wire::{ClientMessage, ServerMessage};

/// Everything a connection can read. One instance, shared by every socket.
#[derive(Debug)]
pub struct ServerState {
    config: ServerConfig,
    /// Flipped once when the server is asked to stop. Open sockets watch it,
    /// because `axum`'s graceful shutdown waits for connections to end and a
    /// long-lived socket never would on its own.
    shutdown: watch::Receiver<bool>,
    live_connections: AtomicUsize,
    unrecognized_messages: AtomicUsize,
    unparseable_frames: AtomicUsize,
}

impl ServerState {
    fn new(config: ServerConfig, shutdown: watch::Receiver<bool>) -> Self {
        ServerState {
            config,
            shutdown,
            live_connections: AtomicUsize::new(0),
            unrecognized_messages: AtomicUsize::new(0),
            unparseable_frames: AtomicUsize::new(0),
        }
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Sockets currently open. A gauge rather than a counter: it is how
    /// "disconnection does not leak server state" is observed from outside,
    /// and it is the number to look at when the app feels stuck.
    pub fn live_connections(&self) -> usize {
        self.live_connections.load(Ordering::Relaxed)
    }

    /// Frames whose `_tag` this build does not know. The socket's half of the
    /// same drift accounting [`crate::protocol`] keeps for the CLI: a client
    /// that learns a new message type must not be able to kill a connection,
    /// and the fact that it happened must not vanish.
    pub fn unrecognized_messages(&self) -> usize {
        self.unrecognized_messages.load(Ordering::Relaxed)
    }

    /// Frames that were not parseable at all.
    pub fn unparseable_frames(&self) -> usize {
        self.unparseable_frames.load(Ordering::Relaxed)
    }
}

/// A running server.
///
/// Dropping this does not stop it; call [`Server::shutdown`]. That is
/// deliberate — the Tauri shell in ticket 23 holds one for the life of the
/// process, and the test harness wants an explicit stop it can await.
pub struct Server {
    local_addr: SocketAddr,
    state: Arc<ServerState>,
    shutdown: watch::Sender<bool>,
    serving: tokio::task::JoinHandle<()>,
}

impl Server {
    /// Bind to loopback on `port` and start serving. Port 0 asks the OS for a
    /// free one, which is what the tests use.
    ///
    /// Loopback is not a default here, it is the security model: v1 has no
    /// identity store, so reachability *is* the boundary.
    pub async fn bind(port: u16) -> std::io::Result<Server> {
        Server::bind_with(port, ServerConfig::detect()).await
    }

    pub async fn bind_with(port: u16, config: ServerConfig) -> std::io::Result<Server> {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let state = Arc::new(ServerState::new(config, shutdown.subscribe()));

        let app = Router::new()
            .route("/ws", get(upgrade))
            // The two answers the UI needs before it will open the socket at
            // all. See `crate::http`.
            .route("/.well-known/t3/environment", get(environment_descriptor))
            .route("/api/auth/session", get(auth_session))
            .with_state(Arc::clone(&state));

        let listener =
            tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
        let local_addr = listener.local_addr()?;

        let serving = tokio::spawn(async move {
            let served = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    // `changed()` only errors once every sender is gone, which
                    // would mean the `Server` was dropped — also a reason to
                    // stop accepting.
                    let _ = shutdown_rx.changed().await;
                })
                .await;
            if let Err(error) = served {
                eprintln!("lightcode: socket endpoint stopped: {error}");
            }
        });

        Ok(Server {
            local_addr,
            state,
            shutdown,
            serving,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The URL the UI connects to.
    pub fn ws_url(&self) -> String {
        format!("ws://{}/ws", self.local_addr)
    }

    pub fn state(&self) -> &Arc<ServerState> {
        &self.state
    }

    /// Stop accepting, close open sockets, and wait for the listener to go.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(true);
        let _ = self.serving.await;
    }

    /// Serve until the process is interrupted. This is what the binary calls.
    pub async fn serve_until_interrupted(self) {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("lightcode: cannot listen for interrupt: {error}");
        }
        self.shutdown().await;
    }
}

async fn environment_descriptor(State(state): State<Arc<ServerState>>) -> Response {
    Json(http::environment_descriptor(state.config()).clone()).into_response()
}

async fn auth_session(State(state): State<Arc<ServerState>>) -> Response {
    Json(http::auth_session_state(state.config()).to_value()).into_response()
}

async fn upgrade(
    State(state): State<Arc<ServerState>>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let request = UpgradeRequest {
        query: query.as_deref(),
        origin: header_str(&headers, header::ORIGIN),
        authorization: header_str(&headers, header::AUTHORIZATION),
        cookie: header_str(&headers, header::COOKIE),
    };

    match auth::authorize(request) {
        Ok(credential) => ws.on_upgrade(move |socket| connection(socket, state, credential)),
        Err(rejection) => {
            eprintln!("lightcode: {} (traceId {})", rejection.detail, rejection.trace_id);
            (
                StatusCode::UNAUTHORIZED,
                // The reference server sets this on the same refusal. Keeping
                // it means a browser reads the body rather than a CORS error.
                [(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")],
                Json(rejection.body()),
            )
                .into_response()
        }
    }
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

async fn connection(socket: WebSocket, state: Arc<ServerState>, credential: Credential) {
    let _live = LiveConnection::open(&state);
    let _ = credential; // recorded at the upgrade; nothing verifies it in v1.

    let mut shutdown = state.shutdown.clone();
    let (mut outgoing, mut incoming) = socket.split();

    loop {
        let frame = tokio::select! {
            _ = shutdown.changed() => {
                let _ = outgoing.send(Message::Close(None)).await;
                break;
            }
            frame = incoming.next() => frame,
        };

        let frame = match frame {
            Some(Ok(frame)) => frame,
            // A reset connection is how a closing browser tab often looks.
            // Nothing to report and nothing to clean up but this task.
            Some(Err(_)) | None => break,
        };

        let reply = match frame {
            Message::Text(text) => handle_frame(&state, text.as_str()),
            Message::Close(_) => break,
            // Every captured frame was text; the vocabulary has no binary
            // member. Ignore rather than close, so a stray frame is not fatal.
            Message::Binary(_) => None,
            // WebSocket-level control frames. Distinct from the JSON `Ping`
            // the UI sends every ~5 s, and answered by the library.
            Message::Ping(_) | Message::Pong(_) => None,
        };

        if let Some(reply) = reply {
            if outgoing.send(Message::Text(reply.into())).await.is_err() {
                break;
            }
        }
    }
}

/// Handle one text frame. `None` means "nothing to say", which is the right
/// answer for `Ack` and `Interrupt` while no method streams.
fn handle_frame(state: &ServerState, text: &str) -> Option<String> {
    let message = match ClientMessage::parse(text) {
        Ok(message) => message,
        Err(error) => {
            state.unparseable_frames.fetch_add(1, Ordering::Relaxed);
            eprintln!("lightcode: unparseable socket frame: {error}");
            return None;
        }
    };

    match message {
        ClientMessage::Request { id, tag, payload } => {
            Some(match rpc::dispatch(state.config(), &tag, &payload) {
                Ok(value) => ServerMessage::success(id, value).to_frame(),
                // An `Exit`/`Failure` under the caller's own `requestId`,
                // *not* the bare `Defect` the reference server sends. A
                // `Defect` carries no id and the client fails every pending
                // request and open subscription on the socket when it sees
                // one — see `DispatchError::to_error` for the evidence and
                // the reasoning.
                Err(error) => ServerMessage::failure(id, error.to_error()).to_frame(),
            })
        }
        ClientMessage::Ping => Some(ServerMessage::Pong.to_frame()),
        // Back-pressure and cancellation for streams. Nothing streams yet, so
        // an `Ack` has nothing to release and an `Interrupt` has nothing to
        // cancel. Both are still normal traffic and must not be errors.
        ClientMessage::Ack { .. } | ClientMessage::Interrupt { .. } => None,
        ClientMessage::Unrecognized => {
            state.unrecognized_messages.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// Keeps the live-connection gauge honest whichever way the loop exits —
/// clean close, reset, or panic.
struct LiveConnection<'a> {
    state: &'a ServerState,
}

impl<'a> LiveConnection<'a> {
    fn open(state: &'a ServerState) -> Self {
        state.live_connections.fetch_add(1, Ordering::Relaxed);
        LiveConnection { state }
    }
}

impl Drop for LiveConnection<'_> {
    fn drop(&mut self) {
        self.state.live_connections.fetch_sub(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> ServerState {
        ServerState::new(ServerConfig::detect(), watch::channel(false).1)
    }

    #[test]
    fn a_request_for_an_implemented_method_is_answered_with_a_success_exit() {
        let state = state();
        let reply = handle_frame(
            &state,
            r#"{"_tag":"Request","id":"7","tag":"server.getConfig","payload":{},"headers":[]}"#,
        )
        .expect("a reply");

        let value: serde_json::Value = serde_json::from_str(&reply).expect("valid json");
        assert_eq!(value["_tag"], "Exit");
        assert_eq!(value["requestId"], "7");
        assert_eq!(value["exit"]["_tag"], "Success");
        assert_eq!(value["exit"]["value"], state.config().to_value());
    }

    #[test]
    fn ping_is_answered_with_pong() {
        assert_eq!(
            handle_frame(&state(), r#"{"_tag":"Ping"}"#).as_deref(),
            Some(r#"{"_tag":"Pong"}"#)
        );
    }

    #[test]
    fn ack_and_interrupt_are_accepted_silently() {
        let state = state();
        assert!(handle_frame(&state, r#"{"_tag":"Ack","requestId":"1"}"#).is_none());
        assert!(handle_frame(&state, r#"{"_tag":"Interrupt","requestId":"1"}"#).is_none());
        assert_eq!(state.unrecognized_messages(), 0);
        assert_eq!(state.unparseable_frames(), 0);
    }

    #[test]
    fn an_unrecognised_frame_is_counted_rather_than_answered() {
        let state = state();
        assert!(handle_frame(&state, r#"{"_tag":"Eof","requestId":"0"}"#).is_none());
        assert_eq!(state.unrecognized_messages(), 1);
        assert_eq!(state.unparseable_frames(), 0);
    }

    #[test]
    fn a_malformed_frame_is_counted_rather_than_answered() {
        let state = state();
        assert!(handle_frame(&state, "{not json").is_none());
        assert_eq!(state.unparseable_frames(), 1);
        assert_eq!(state.unrecognized_messages(), 0);
    }

    /// The failure has to arrive under the caller's `requestId`, so it fails
    /// one call rather than the whole session.
    #[test]
    fn an_unimplemented_method_fails_only_its_own_request() {
        let reply = handle_frame(
            &state(),
            r#"{"_tag":"Request","id":"1","tag":"no.such.method","payload":{},"headers":[]}"#,
        )
        .expect("a reply");

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&reply).expect("valid json"),
            json!({
                "_tag": "Exit",
                "requestId": "1",
                "exit": {
                    "_tag": "Failure",
                    "cause": [{
                        "_tag": "Fail",
                        "error": {
                            "_tag": "ServerMethodNotImplementedError",
                            "method": "no.such.method",
                            "message": "Method not implemented by this server: no.such.method",
                        },
                    }],
                },
            })
        );
    }
}
