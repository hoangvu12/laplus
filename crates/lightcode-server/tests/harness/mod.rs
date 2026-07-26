//! The socket test harness: start a server, connect a client, drive methods.
//!
//! This is the project's **primary test seam**. The spec puts the bulk of
//! testing here rather than giving each subsystem its own seam, because this
//! is the genuine contract with the UI — filesystem, projects, provider,
//! orchestration, terminal and git are all meant to be exercised through it.
//! Ticket 03 builds it for one method; every later ticket adds calls, not
//! plumbing.
//!
//! Three things it deliberately does:
//!
//! - **Speaks a different WebSocket implementation from the server.** The
//!   server is `axum`/`tungstenite`-on-the-inside; the client here is
//!   `tokio-tungstenite` driven directly. A passing test means two stacks
//!   agree on the framing.
//! - **Correlates by `requestId`, never by arrival order.** The reference
//!   server answers concurrent calls out of order and a conforming client must
//!   cope, so the harness copes — otherwise it would quietly bake in an
//!   assumption the real UI does not make.
//! - **Times every read out.** A protocol bug usually presents as "nothing
//!   arrives". Without a timeout that is a hung suite instead of a failure.

#![allow(dead_code)]

pub mod captures;
pub mod shape;
pub mod workspace;

use std::path::Path;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use lightcode_server::config::ServerConfig;
use lightcode_server::config_store::ConfigChange;
use lightcode_server::store::Database;
use lightcode_server::Server;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

/// How long any single read may take before the test fails instead of hanging.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

/// A server bound to a free loopback port, with the sockets it hands out.
pub struct TestServer {
    server: Server,
}

impl TestServer {
    /// A server whose registry lives only as long as it does. What every test
    /// that is not about persistence wants: nothing is shared between tests,
    /// and the developer's own project list is never touched.
    pub async fn start() -> TestServer {
        TestServer::start_with(ServerConfig::detect()).await
    }

    pub async fn start_with(config: ServerConfig) -> TestServer {
        TestServer::start_on(config, Database::in_memory().expect("an in-memory database")).await
    }

    /// A server whose registry is a file. Start a second one on the same path
    /// and that is a restart — which is how the "survives a restart" test is
    /// driven without a second process.
    pub async fn start_at(database: &Path) -> TestServer {
        TestServer::start_on(
            ServerConfig::detect(),
            Database::open(database).expect("the database opens"),
        )
        .await
    }

    async fn start_on(config: ServerConfig, database: Database) -> TestServer {
        let server = Server::bind_with(0, config, database)
            .await
            .expect("server binds to a free loopback port");
        TestServer { server }
    }

    pub fn ws_url(&self) -> String {
        self.server.ws_url()
    }

    pub fn addr(&self) -> std::net::SocketAddr {
        self.server.local_addr()
    }

    /// Sockets currently open, as the server counts them. Used to check that
    /// disconnecting leaves nothing behind.
    pub fn live_connections(&self) -> usize {
        self.server.state().live_connections()
    }

    /// Wait for the gauge to reach `expected`, or fail saying what it was.
    ///
    /// A closed socket is torn down by the connection's own task, so the
    /// count drops a moment after the client stops caring. Polling is the
    /// honest way to observe that without pretending it is synchronous.
    pub async fn await_live_connections(&self, expected: usize) {
        self.await_gauge("live connections", expected, || self.live_connections())
            .await;
    }

    /// The waiting the three gauges share.
    ///
    /// Each of them is a number some other task moves — a connection's own
    /// teardown, a pump ending, a deferred listing finishing — so none of them
    /// settles synchronously with the call that caused it. `name` is only for
    /// the failure message, and the failure message is the point: without it a
    /// gauge that never settles is a hung suite instead of a sentence saying
    /// which one and what it stuck at.
    async fn await_gauge(&self, name: &str, expected: usize, read: impl Fn() -> usize) {
        let deadline = std::time::Instant::now() + READ_TIMEOUT;
        while read() != expected {
            assert!(
                std::time::Instant::now() < deadline,
                "{name} stayed at {} instead of settling to {expected}",
                read()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Subscriptions the server is currently pumping, across every
    /// connection. The subscription half of [`TestServer::live_connections`],
    /// and how "resources are released on unsubscribe" is observed from
    /// outside.
    pub fn live_subscriptions(&self) -> usize {
        self.server.state().live_subscriptions()
    }

    /// Wait for the subscription gauge to reach `expected`, or fail saying
    /// what it was. Same reasoning as [`TestServer::await_live_connections`]:
    /// a pump is torn down by its own task, a moment after whatever ended it.
    pub async fn await_live_subscriptions(&self, expected: usize) {
        self.await_gauge("live subscriptions", expected, || {
            self.live_subscriptions()
        })
        .await;
    }

    /// Workspaces the server is watching. The filesystem half of the same
    /// accounting: a watch outlives the call that started it, so this is where
    /// one that was never released shows up.
    pub fn watched_workspaces(&self) -> usize {
        self.server.state().watched_workspaces()
    }

    /// Wait for the watch gauge to reach `expected`, or fail saying what it
    /// was. `projects.listEntries` is answered off the read loop, so the watch
    /// it starts is in place a moment after the client has its answer.
    pub async fn await_watched_workspaces(&self, expected: usize) {
        self.await_gauge("watched workspaces", expected, || {
            self.watched_workspaces()
        })
        .await;
    }

    pub fn unrecognized_messages(&self) -> usize {
        self.server.state().unrecognized_messages()
    }

    pub fn unparseable_frames(&self) -> usize {
        self.server.state().unparseable_frames()
    }

    /// The config the server will answer `server.getConfig` with.
    pub fn config(&self) -> Value {
        self.server.state().config().current().to_value()
    }

    /// Make a server-side configuration change, as a later ticket's provider
    /// registry or settings writer will. This is the *cause* a subscriber
    /// observes; nothing about the assertion reaches into the server.
    pub fn change_config(&self, change: ConfigChange) {
        self.server.state().config().apply(change);
    }

    /// A settings change that alters one visible flag. The cheapest real
    /// change to make, and the one most tests want.
    pub fn toggle_a_setting(&self) -> bool {
        let mut settings = self.server.state().config().current().settings.clone();
        settings.enable_assistant_streaming = !settings.enable_assistant_streaming;
        let now = settings.enable_assistant_streaming;
        self.change_config(ConfigChange::Settings(Box::new(settings)));
        now
    }

    /// Connect the way a non-browser client does: a `wsTicket`, no `Origin`.
    pub async fn connect(&self) -> SocketClient {
        self.connect_as(ClientIdentity::ticket())
            .await
            .expect("upgrade is accepted")
    }

    pub async fn connect_as(&self, identity: ClientIdentity) -> Result<SocketClient, Refusal> {
        let mut url = self.ws_url();
        if let Some(ticket) = &identity.ticket {
            url.push_str("?wsTicket=");
            url.push_str(ticket);
        }

        let mut request = url
            .into_client_request()
            .expect("the server's own url is a valid websocket request");
        for (name, value) in [
            ("origin", identity.origin.as_deref()),
            ("cookie", identity.cookie.as_deref()),
            ("authorization", identity.authorization.as_deref()),
        ] {
            if let Some(value) = value {
                request.headers_mut().insert(
                    name,
                    value.parse().expect("header value is valid ascii"),
                );
            }
        }

        match connect_async(request).await {
            Ok((socket, _response)) => Ok(SocketClient {
                socket,
                next_id: 0,
                buffered: Vec::new(),
            }),
            Err(WsError::Http(response)) => {
                let body = response
                    .body()
                    .as_ref()
                    .and_then(|bytes| serde_json::from_slice(bytes).ok())
                    .unwrap_or(Value::Null);
                Err(Refusal {
                    status: response.status().as_u16(),
                    body,
                })
            }
            Err(error) => panic!("connecting failed for a non-http reason: {error}"),
        }
    }

    /// A plain `GET`, for the two endpoints the UI hits before it opens a
    /// socket. Raw HTTP rather than a client library, for the same reason
    /// [`TestServer::raw_upgrade`] is: no dependency, and nothing between the
    /// assertion and the bytes.
    pub async fn get(&self, path: &str) -> HttpResponse {
        let raw = self
            .raw_request(&format!(
                "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
                self.addr()
            ))
            .await;

        let (head, body) = raw
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("no header/body boundary in: {raw}"));

        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("no status code in: {head}"));

        HttpResponse {
            status,
            head: head.to_string(),
            body: serde_json::from_str(body).unwrap_or(Value::Null),
        }
    }

    /// Perform the upgrade by hand and return the raw response head.
    ///
    /// Some of what ticket 01 pinned is in the handshake itself rather than in
    /// any frame — that `permessage-deflate` is declined, that no subprotocol
    /// is negotiated. A WebSocket library normalises those away, so the only
    /// way to assert them is to speak HTTP directly.
    pub async fn raw_upgrade(&self, request_head: &str) -> String {
        // A 101 leaves the connection open, so stop at the blank line rather
        // than waiting for an end that never comes.
        self.raw_exchange(request_head, StopAt::EndOfHead).await
    }

    /// Send a request and read until the server closes. Only safe with
    /// `Connection: close`.
    pub async fn raw_request(&self, request_head: &str) -> String {
        self.raw_exchange(request_head, StopAt::EndOfStream).await
    }

    async fn raw_exchange(&self, request_head: &str, stop: StopAt) -> String {
        let mut stream = TcpStream::connect(self.addr())
            .await
            .expect("connects to the listener");
        stream
            .write_all(request_head.as_bytes())
            .await
            .expect("writes the request");

        let mut response = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut chunk))
                .await
                .expect("the server answers within the timeout")
                .expect("reads from the socket");
            if read == 0 {
                break;
            }
            response.extend_from_slice(&chunk[..read]);
            if stop == StopAt::EndOfHead
                && response.windows(4).any(|window| window == b"\r\n\r\n")
            {
                break;
            }
        }

        String::from_utf8_lossy(&response).into_owned()
    }

    pub async fn stop(self) {
        self.server.shutdown().await;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopAt {
    EndOfHead,
    EndOfStream,
}

/// A plain HTTP response, with its body parsed as JSON when it is JSON.
#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub head: String,
    pub body: Value,
}

/// What a client presents at the upgrade.
#[derive(Debug, Default, Clone)]
pub struct ClientIdentity {
    pub ticket: Option<String>,
    pub origin: Option<String>,
    pub cookie: Option<String>,
    pub authorization: Option<String>,
}

impl ClientIdentity {
    /// Nothing at all — the permissive path.
    pub fn anonymous() -> Self {
        ClientIdentity::default()
    }

    /// The shape a non-browser client sends, as captured in
    /// `fixtures/socket-wire/02-request-response.ndjson`.
    pub fn ticket() -> Self {
        ClientIdentity {
            ticket: Some("eyJ2IjoxLCJraW5kIjoid2Vic29ja2V0In0.c2lnbmF0dXJl".to_string()),
            ..ClientIdentity::default()
        }
    }

    /// The shape the browser UI sends: a session cookie and a loopback origin.
    pub fn browser() -> Self {
        ClientIdentity {
            cookie: Some("t3_session=eyJ2IjoxLCJraW5kIjoic2Vzc2lvbiJ9.c2lnbmF0dXJl".to_string()),
            origin: Some("http://127.0.0.1".to_string()),
            ..ClientIdentity::default()
        }
    }

    pub fn with_origin(mut self, origin: &str) -> Self {
        self.origin = Some(origin.to_string());
        self
    }
}

/// A refused upgrade: the socket never opened.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub status: u16,
    pub body: Value,
}

/// One open socket, and the request-id space that belongs to it.
#[derive(Debug)]
pub struct SocketClient {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    /// Frames that arrived while waiting for a different `requestId`. Kept so
    /// out-of-order answers are not lost, which is the whole point of
    /// correlating rather than assuming FIFO.
    buffered: Vec<Value>,
}

/// What a call came back as. A `Defect` is a distinct outcome rather than an
/// error case because it carries no `requestId` and therefore is not, strictly,
/// an answer to anything.
#[derive(Debug, Clone)]
pub enum Outcome {
    Success(Value),
    Failure(Vec<Value>),
    Defect(Value),
}

impl Outcome {
    /// The success value, or a panic naming what came back instead.
    pub fn expect_success(self) -> Value {
        match self {
            Outcome::Success(value) => value,
            other => panic!("expected a success exit, got {other:?}"),
        }
    }

    /// The typed error from a refused call, checked to be one the method
    /// declares.
    ///
    /// Two assertions, and each is a distinct way of failing the client: an
    /// error that is not `Fail` is not a refusal at all, and one whose `_tag`
    /// is not in the method's declared union fails to decode and costs the
    /// call.
    ///
    /// **Not asserted here: a `message`.** Whether the sentence is on the wire
    /// depends on the error's own schema. `ProjectReadFileError` declares
    /// `message` as a field and carries it — see the captured payload in
    /// `fixtures/socket-wire/03-typed-error.ndjson` — while every
    /// `ExternalLauncher*` class defines `message` as a getter over its
    /// structured fields, so the client computes it and the reference server
    /// sends nothing. A blanket assertion here would force lightcode to send a
    /// field the reference server does not. Tests that care assert on it
    /// themselves.
    pub fn expect_declared(self, tag: &str) -> Value {
        match self {
            Outcome::Failure(cause) => {
                assert_eq!(cause.len(), 1, "one cause entry: {cause:?}");
                assert_eq!(cause[0]["_tag"], "Fail");
                assert_eq!(
                    cause[0]["error"]["_tag"], tag,
                    "the error must be one the method declares, or the client cannot decode it"
                );
                cause[0]["error"].clone()
            }
            other => panic!("expected a typed failure, got {other:?}"),
        }
    }
}

impl SocketClient {
    /// Send a `Request` and wait for its answer, correlating by `requestId`.
    pub async fn call(&mut self, tag: &str, payload: Value) -> Outcome {
        let id = self.send_request(tag, payload).await;
        self.await_outcome(&id).await
    }

    /// Send a `Request` and return the answering frame untouched, envelope and
    /// all. Conformance tests compare envelopes, not just payloads.
    pub async fn call_raw(&mut self, tag: &str, payload: Value) -> Value {
        self.send_request(tag, payload).await;
        self.recv().await
    }

    /// Send a `Request` without waiting, returning its id. For driving
    /// concurrent calls.
    pub async fn send_request(&mut self, tag: &str, payload: Value) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.send(json!({
            "_tag": "Request",
            "id": id,
            "tag": tag,
            "payload": payload,
            "headers": [],
        }))
        .await;
        id
    }

    /// Wait for the answer to an already-sent request.
    pub async fn await_outcome(&mut self, request_id: &str) -> Outcome {
        let frame = self.take_frame(|frame| answers(frame, request_id)).await;
        outcome(frame)
    }

    /// The next frame matching `wanted`, reading until one arrives.
    ///
    /// Frames that do not match are buffered rather than dropped, which is the
    /// whole reason this exists: the reference server answers concurrent calls
    /// out of order and interleaves a subscription's chunks with everything
    /// else the connection is doing, so a client that consumed frames in
    /// arrival order would lose the ones it was not waiting for.
    async fn take_frame(&mut self, wanted: impl Fn(&Value) -> bool) -> Value {
        loop {
            if let Some(index) = self.buffered.iter().position(&wanted) {
                return self.buffered.remove(index);
            }

            let frame = self.recv().await;
            if wanted(&frame) {
                return frame;
            }
            self.buffered.push(frame);
        }
    }

    /// Open a subscription and return its request id.
    ///
    /// There is no `subscribe` verb on this wire — a subscription is an
    /// ordinary `Request` — so this is [`SocketClient::send_request`] under a
    /// name that says what the caller means.
    pub async fn subscribe(&mut self, tag: &str, payload: Value) -> String {
        self.send_request(tag, payload).await
    }

    /// Acknowledge a chunk, releasing the server to send the next one.
    ///
    /// Not optional: the server holds at most one un-acknowledged chunk per
    /// request, so a test that forgets this simply stops receiving.
    pub async fn ack(&mut self, request_id: &str) {
        self.send(json!({"_tag": "Ack", "requestId": request_id})).await;
    }

    /// Cancel a call. For a subscription this is the unsubscribe.
    pub async fn interrupt(&mut self, request_id: &str) {
        self.send(json!({"_tag": "Interrupt", "requestId": request_id}))
            .await;
    }

    /// The next frame that concerns `request_id`, whole — a `Chunk` or the
    /// terminal `Exit`.
    pub async fn next_frame_for(&mut self, request_id: &str) -> Value {
        self.take_frame(|frame| concerns(frame, request_id)).await
    }

    /// The `values` of the next `Chunk` for `request_id`.
    ///
    /// Deliberately does **not** acknowledge. `Ack` is load-bearing on this
    /// wire — the server stops after one un-acknowledged chunk — so a test
    /// that wants to keep receiving has to say so, the same way the UI does.
    pub async fn next_chunk(&mut self, request_id: &str) -> Vec<Value> {
        let frame = self.next_frame_for(request_id).await;
        assert_eq!(
            frame["_tag"],
            json!("Chunk"),
            "expected a chunk for {request_id}, the stream ended instead: {frame}"
        );
        frame["values"]
            .as_array()
            .unwrap_or_else(|| panic!("a chunk's values are an array: {frame}"))
            .clone()
    }

    /// The single value of the next chunk, acknowledged. `values` genuinely
    /// batches, so a test that expects one value says so.
    pub async fn next_event(&mut self, request_id: &str) -> Value {
        let values = self.next_chunk(request_id).await;
        self.ack(request_id).await;
        assert_eq!(values.len(), 1, "expected a single value, got {values:#?}");
        values.into_iter().next().expect("a value")
    }

    /// `Ping` and the `Pong` that answers it.
    pub async fn ping(&mut self) -> Value {
        self.send(json!({"_tag": "Ping"})).await;
        self.recv().await
    }

    /// Send a frame exactly as given. For malformed and unrecognised frames,
    /// which the typed helpers cannot express.
    pub async fn send(&mut self, frame: Value) {
        self.send_text(&frame.to_string()).await;
    }

    pub async fn send_text(&mut self, text: &str) {
        self.socket
            .send(Message::Text(text.into()))
            .await
            .expect("sends a text frame");
    }

    /// The next frame, whatever it is.
    pub async fn recv(&mut self) -> Value {
        loop {
            let frame = tokio::time::timeout(READ_TIMEOUT, self.socket.next())
                .await
                .expect("a frame arrives within the timeout")
                .expect("the socket is still open")
                .expect("the frame is readable");

            match frame {
                Message::Text(text) => {
                    return serde_json::from_str(text.as_str())
                        .unwrap_or_else(|error| panic!("frame is not json: {error}: {text}"))
                }
                Message::Binary(bytes) => panic!("unexpected binary frame of {} bytes", bytes.len()),
                Message::Close(frame) => panic!("server closed the socket: {frame:?}"),
                // Control frames the library handles; keep reading.
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            }
        }
    }

    /// Assert nothing more arrives for a while. Used to check that a frame the
    /// server should ignore really is ignored.
    pub async fn expect_silence(&mut self, how_long: Duration) {
        if let Ok(Some(frame)) = tokio::time::timeout(how_long, self.socket.next()).await {
            panic!("expected silence, got {frame:?}");
        }
    }

    pub async fn close(mut self) {
        let _ = self.socket.close(None).await;
    }

    /// Vanish without a close handshake — a dropped TCP connection, which is
    /// what a killed browser tab or a lost network looks like from the
    /// server's side.
    pub fn abandon(self) {
        drop(self);
    }
}

/// Does this frame answer `request_id`? A `Defect` answers everything, because
/// it carries no id and no `Exit` will follow.
fn answers(frame: &Value, request_id: &str) -> bool {
    match frame["_tag"].as_str() {
        Some("Exit") => frame["requestId"] == json!(request_id),
        Some("Defect") => true,
        _ => false,
    }
}

/// Is this frame about `request_id` at all — a `Chunk` or an `Exit`?
fn concerns(frame: &Value, request_id: &str) -> bool {
    matches!(frame["_tag"].as_str(), Some("Chunk") | Some("Exit"))
        && frame["requestId"] == json!(request_id)
        || answers(frame, request_id)
}

fn outcome(frame: Value) -> Outcome {
    match frame["_tag"].as_str() {
        Some("Defect") => Outcome::Defect(frame["defect"].clone()),
        Some("Exit") => match frame["exit"]["_tag"].as_str() {
            Some("Success") => Outcome::Success(frame["exit"]["value"].clone()),
            Some("Failure") => Outcome::Failure(
                frame["exit"]["cause"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            ),
            other => panic!("unknown exit tag {other:?} in {frame}"),
        },
        other => panic!("frame {other:?} is not an answer: {frame}"),
    }
}
