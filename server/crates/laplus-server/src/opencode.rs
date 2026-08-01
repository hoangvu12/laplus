//! HTTP ownership and event-stream lifetime for OpenCode.

use std::{fmt, net::TcpListener as StdTcpListener, path::Path, process::Stdio, time::Duration};

use futures_util::StreamExt;
use reqwest::{Method, StatusCode, Url};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::{
    process::{Child, Command},
    sync::{mpsc, watch},
    task::JoinHandle,
};

use crate::opencode_protocol::{
    Health, OpenCodeEvent, Session, SseDecodeError, SseDecoder, StructuredError,
};

#[derive(Clone)]
pub struct OpenCodeClient {
    base_url: Url,
    directory: String,
    password: Option<String>,
    http: reqwest::Client,
}

impl fmt::Debug for OpenCodeClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenCodeClient")
            .field("base_url", &self.base_url)
            .field("directory", &self.directory)
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum OpenCodeError {
    InvalidBaseUrl(String),
    Transport(reqwest::Error),
    Authentication {
        status: StatusCode,
        error: Option<StructuredError>,
    },
    MissingSession {
        status: StatusCode,
        error: StructuredError,
    },
    Server {
        status: StatusCode,
        error: Option<StructuredError>,
        body: String,
    },
    MalformedJson {
        source: serde_json::Error,
        body: String,
    },
    MalformedSse(SseDecodeError),
    StreamClosed,
}

impl fmt::Display for OpenCodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBaseUrl(value) => write!(formatter, "invalid OpenCode base URL: {value}"),
            Self::Transport(error) => write!(formatter, "OpenCode transport failed: {error}"),
            Self::Authentication { status, .. } => {
                write!(formatter, "OpenCode authentication failed ({status})")
            }
            Self::MissingSession { .. } => formatter.write_str("OpenCode session does not exist"),
            Self::Server { status, .. } => write!(formatter, "OpenCode request failed ({status})"),
            Self::MalformedJson { source, .. } => {
                write!(formatter, "OpenCode returned malformed JSON: {source}")
            }
            Self::MalformedSse(error) => {
                write!(formatter, "OpenCode returned malformed SSE: {error}")
            }
            Self::StreamClosed => formatter.write_str("OpenCode event stream closed"),
        }
    }
}

impl std::error::Error for OpenCodeError {}

impl OpenCodeClient {
    pub fn new(
        base_url: &str,
        directory: impl Into<String>,
        password: Option<String>,
    ) -> Result<Self, OpenCodeError> {
        let mut base_url =
            Url::parse(base_url).map_err(|_| OpenCodeError::InvalidBaseUrl(base_url.into()))?;
        if !matches!(base_url.scheme(), "http" | "https") || base_url.host().is_none() {
            return Err(OpenCodeError::InvalidBaseUrl(base_url.to_string()));
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        Ok(Self {
            base_url,
            directory: directory.into(),
            password,
            http: reqwest::Client::new(),
        })
    }

    pub async fn health(&self) -> Result<Health, OpenCodeError> {
        self.request_json(Method::GET, "global/health", Option::<&()>::None)
            .await
    }
    pub async fn providers(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "provider", Option::<&()>::None)
            .await
    }
    pub async fn agents(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "agent", Option::<&()>::None)
            .await
    }
    pub async fn create_session(&self, body: &Value) -> Result<Session, OpenCodeError> {
        self.request_json(Method::POST, "session", Some(body)).await
    }
    pub async fn session(&self, id: &str) -> Result<Session, OpenCodeError> {
        self.request_json(Method::GET, &format!("session/{id}"), Option::<&()>::None)
            .await
    }
    pub async fn prompt(&self, id: &str, body: &Value) -> Result<(), OpenCodeError> {
        let response = self
            .request(
                Method::POST,
                &format!("session/{id}/prompt_async"),
                Some(body),
            )
            .send()
            .await
            .map_err(OpenCodeError::Transport)?;
        classify_response(response).await.map(|_| ())
    }
    pub async fn abort(&self, id: &str) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::POST,
            &format!("session/{id}/abort"),
            Option::<&()>::None,
        )
        .await
    }
    pub async fn revert(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(Method::POST, &format!("session/{id}/revert"), Some(body))
            .await
    }
    pub async fn reply_permission(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(Method::POST, &format!("permission/{id}/reply"), Some(body))
            .await
    }
    pub async fn reply_question(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(Method::POST, &format!("question/{id}/reply"), Some(body))
            .await
    }
    pub async fn reject_question(&self, id: &str) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::POST,
            &format!("question/{id}/reject"),
            Option::<&()>::None,
        )
        .await
    }

    pub async fn subscribe(&self) -> Result<EventStream, OpenCodeError> {
        let response = self
            .request(Method::GET, "event", Option::<&()>::None)
            .send()
            .await
            .map_err(OpenCodeError::Transport)?;
        let response = classify_response(response).await?;
        let (events_tx, events_rx) = mpsc::channel(32);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let mut chunks = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            loop {
                tokio::select! {
                    changed = cancel_rx.changed() => if changed.is_ok() && *cancel_rx.borrow() { break },
                    chunk = chunks.next() => match chunk {
                        Some(Ok(bytes)) => for decoded in decoder.push(&bytes) {
                            if events_tx.send(decoded.map_err(OpenCodeError::MalformedSse)).await.is_err() { return; }
                        },
                        Some(Err(error)) => { let _ = events_tx.send(Err(OpenCodeError::Transport(error))).await; return; }
                        None => { if let Some(error) = decoder.finish() { let _ = events_tx.send(Err(OpenCodeError::MalformedSse(error))).await; } return; }
                    }
                }
            }
        });
        Ok(EventStream {
            events: events_rx,
            cancel: cancel_tx,
            task: Some(task),
            unknown_count: 0,
        })
    }

    fn request<B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> reqwest::RequestBuilder {
        let mut url = self.base_url.join(path).expect("validated base URL");
        url.query_pairs_mut()
            .append_pair("directory", &self.directory);
        let mut request = self.http.request(method, url);
        if let Some(password) = &self.password {
            request = request.basic_auth("opencode", Some(password));
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        request
    }

    async fn request_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, OpenCodeError> {
        let response = self
            .request(method, path, body)
            .send()
            .await
            .map_err(OpenCodeError::Transport)?;
        let bytes = classify_response(response)
            .await?
            .bytes()
            .await
            .map_err(OpenCodeError::Transport)?;
        serde_json::from_slice(&bytes).map_err(|source| OpenCodeError::MalformedJson {
            source,
            body: String::from_utf8_lossy(&bytes).into_owned(),
        })
    }
}

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const READINESS_POLL: Duration = Duration::from_millis(50);
const EXIT_GRACE: Duration = Duration::from_secs(2);
const EXIT_POLL: Duration = Duration::from_millis(20);

/// One loopback OpenCode server owned by a conversation.
struct OwnedServer {
    child: Child,
    process_group_id: u32,
}

impl OwnedServer {
    async fn start(binary: &Path, directory: &str) -> Result<(Self, OpenCodeClient), String> {
        let listener = StdTcpListener::bind(("127.0.0.1", 0))
            .map_err(|error| format!("OpenCode could not reserve a loopback port: {error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        drop(listener);
        let mut command = Command::new(binary);
        command
            .arg("serve")
            .arg("--hostname=127.0.0.1")
            .arg(format!("--port={port}"))
            .current_dir(directory)
            .env("OPENCODE_CONFIG_CONTENT", "{}")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().map_err(|error| {
            format!(
                "The OpenCode binary {} could not be started in {directory}: {error}",
                binary.display()
            )
        })?;
        let process_group_id = child.id().ok_or_else(|| "OpenCode started without a process id.".to_string())?;
        let mut owned = Self { child, process_group_id };
        let client = OpenCodeClient::new(&format!("http://127.0.0.1:{port}"), directory, None)
            .map_err(|error| error.to_string())?;
        let ready = tokio::time::timeout(STARTUP_TIMEOUT, async {
            loop {
                if let Some(status) = owned.child.try_wait().map_err(|error| error.to_string())? {
                    return Err(format!("OpenCode exited before becoming ready ({status})."));
                }
                match client.health().await {
                    Ok(health) if health.healthy => return Ok(()),
                    _ => tokio::time::sleep(READINESS_POLL).await,
                }
            }
        })
        .await;
        match ready {
            Ok(Ok(())) => Ok((owned, client)),
            Ok(Err(error)) => {
                owned.stop().await;
                Err(error)
            }
            Err(_) => {
                owned.stop().await;
                Err(format!(
                    "OpenCode did not become ready within {} seconds.",
                    STARTUP_TIMEOUT.as_secs()
                ))
            }
        }
    }

    async fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            terminate_owned_group(self.process_group_id, true).await;
            return;
        }
        let pid = self.process_group_id;

        terminate_owned_group(pid, false).await;
        let deadline = tokio::time::Instant::now() + EXIT_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => {
                    // The group leader exiting does not prove that everything it
                    // launched honored the graceful signal. The durable group id
                    // remains valid long enough to remove any surviving child.
                    terminate_owned_group(pid, true).await;
                    return;
                }
                Err(_) => break,
                Ok(None) if tokio::time::Instant::now() >= deadline => break,
                Ok(None) => tokio::time::sleep(EXIT_POLL).await,
            }
        }

        terminate_owned_group(pid, true).await;
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

async fn terminate_owned_group(pid: u32, force: bool) {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill.exe");
        command.args(["/PID", &pid.to_string(), "/T"]);
        if force { command.arg("/F"); }
        command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        crate::process::without_a_console(command.as_std_mut());
        let _ = command.status().await;
    }
    #[cfg(unix)]
    {
        let signal = if force { "-KILL" } else { "-TERM" };
        let _ = Command::new("kill")
            .args([signal, "--", &format!("-{pid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;
    }
}

pub(crate) struct OpenCode {
    client: OpenCodeClient,
    events: EventStream,
    session_id: String,
    owned: OwnedServer,
    model: Option<String>,
    settled: bool,
}

fn cursor(start: &crate::session::Start, session_id: &str) -> crate::provider::ResumeCursor {
    crate::provider::ResumeCursor {
        provider: start.provider.clone(),
        value: serde_json::json!({"version": 1, "sessionId": session_id}),
    }
}

fn event_session(properties: &Value) -> Option<&str> {
    properties
        .get("sessionID")
        .and_then(Value::as_str)
        .or_else(|| {
            properties
                .get("part")
                .and_then(|part| part.get("sessionID"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            properties
                .get("info")
                .and_then(|info| info.get("sessionID"))
                .and_then(Value::as_str)
        })
}

impl crate::session::Driver for OpenCode {
    async fn open(start: &crate::session::Start) -> Result<crate::session::Opened<Self>, String> {
        if start.resume_cursor.is_some() {
            return Err("OpenCode continuation is not available until ticket 15.".to_string());
        }
        let settings = start.driver.opencode()?;
        let (binary, _) = crate::provider::resolve_named(
            &settings.binary_path,
            "opencode",
            &crate::process::Search::from_environment(),
        )
        .startable_for("OpenCode CLI")?;
        let (mut owned, client) = OwnedServer::start(&binary, &start.workspace_root).await?;
        let opened = async {
            let events = client
                .subscribe()
                .await
                .map_err(|error| error.to_string())?;
            let session = client
                .create_session(&serde_json::json!({"title": "Laplus conversation"}))
                .await
                .map_err(|error| error.to_string())?;
            Ok::<_, String>((events, session))
        }
        .await;
        let (events, session) = match opened {
            Ok(value) => value,
            Err(error) => {
                owned.stop().await;
                return Err(error);
            }
        };
        Ok(crate::session::Opened {
            driver: Self {
                client,
                events,
                session_id: session.id.clone(),
                owned,
                model: start.model.clone(),
                settled: false,
            },
            decided: crate::session::Decided {
                provider_resume_cursor: Some(cursor(start, &session.id)),
                ..Default::default()
            },
        })
    }

    async fn next(
        &mut self,
        driving: &mut crate::session::Driving,
    ) -> Option<crate::session::Decided> {
        let event = match self.events.next().await {
            Ok(event) => event,
            Err(_) => return None,
        };
        let envelope = event.envelope();
        if event_session(&envelope.properties).is_some_and(|id| id != self.session_id) {
            return Some(Default::default());
        }
        let mut decided = crate::session::Decided::default();
        match envelope.kind.as_str() {
            "message.part.delta" => {
                if envelope.properties.get("field").and_then(Value::as_str) == Some("text") {
                    if let (Some(active), Some(text)) = (
                        driving.turn.as_mut(),
                        envelope.properties.get("delta").and_then(Value::as_str),
                    ) {
                        let message_id = active
                            .assistant_message_id
                            .get_or_insert_with(crate::threads::fresh_message_id)
                            .clone();
                        decided
                            .changes
                            .push(crate::threads::Change::AssistantDelta {
                                message_id,
                                turn_id: active.turn_id.clone(),
                                text: text.to_string(),
                            });
                    }
                }
            }
            "message.part.updated" => {
                let part = envelope
                    .properties
                    .get("part")
                    .unwrap_or(&envelope.properties);
                if part.get("type").and_then(Value::as_str) == Some("text") {
                    if let (Some(active), Some(text)) = (
                        driving.turn.as_mut(),
                        part.get("text").and_then(Value::as_str),
                    ) {
                        let message_id = active
                            .assistant_message_id
                            .take()
                            .unwrap_or_else(crate::threads::fresh_message_id);
                        decided
                            .changes
                            .push(crate::threads::Change::AssistantMessage {
                                message_id,
                                turn_id: active.turn_id.clone(),
                                text: text.to_string(),
                            });
                    }
                }
            }
            "session.idle" => return Some(opencode_settle(driving, &mut self.settled)),
            "session.status"
                if envelope
                    .properties
                    .pointer("/status/type")
                    .and_then(Value::as_str)
                    == Some("idle") =>
            {
                return Some(opencode_settle(driving, &mut self.settled));
            }
            _ => {}
        }
        Some(decided)
    }

    async fn send(&mut self, text: &str) -> std::io::Result<()> {
        self.settled = false;
        let mut body = serde_json::json!({"parts": [{"type":"text", "text":text}]});
        if let Some(model) = self.model.as_deref() {
            let (provider, model) = model
                .split_once('/')
                .ok_or_else(|| std::io::Error::other("OpenCode model must be provider/model"))?;
            body["model"] = serde_json::json!({"providerID":provider, "modelID":model});
        }
        self.client
            .prompt(&self.session_id, &body)
            .await
            .map_err(std::io::Error::other)
    }

    async fn interrupt(&mut self, _request_id: &str) -> std::io::Result<()> {
        Ok(())
    }
    async fn answer(
        &mut self,
        _asked: &crate::approval::ApprovalRequest,
        _reply: crate::session::Reply<'_>,
    ) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "OpenCode approvals require ticket 13",
        ))
    }
    async fn measure(&mut self, _request_id: &str) -> std::io::Result<()> {
        Ok(())
    }
    async fn retune(
        &mut self,
        _request_id: &str,
        asked: &crate::session::Pushed,
    ) -> std::io::Result<()> {
        if let crate::session::Pushed::Model { asked, .. } = asked {
            self.model = Some(asked.clone());
        }
        Ok(())
    }
    fn close_input(&mut self) {
        self.events.close();
    }
    async fn stop(
        mut self,
        driving: &mut crate::session::Driving,
        asked_to_stop: bool,
    ) -> crate::session::Reaped {
        self.events.cancel().await;
        self.owned.stop().await;
        crate::session::Reaped {
            refused: None,
            death: (driving.turn.is_some() && !asked_to_stop)
                .then(|| "OpenCode stopped before the turn finished.".to_string()),
        }
    }
}

fn opencode_settle(
    driving: &mut crate::session::Driving,
    settled: &mut bool,
) -> crate::session::Decided {
    if *settled {
        return Default::default();
    }
    let Some(finished) = driving.turn.take() else {
        return Default::default();
    };
    *settled = true;
    driving.finished = Some(crate::session::Finished {
        turn_id: finished.turn_id.clone(),
        status: "ready",
    });
    crate::session::Decided {
        changes: vec![crate::threads::Change::Activity(
            crate::threads::Activity::info(
                "turn.completed",
                "Turn completed.",
                serde_json::json!({"durationMs":Value::Null,"totalCostUsd":Value::Null,"isError":false,"interrupted":false}),
                Some(finished.turn_id.clone()),
            ),
        )],
        settles: Some(crate::session::Settles {
            turn_id: Some(finished.turn_id),
            status: crate::settling::SessionStatus::Ready,
            last_error: None,
        }),
        ..Default::default()
    }
}

async fn classify_response(
    response: reqwest::Response,
) -> Result<reqwest::Response, OpenCodeError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let bytes = response.bytes().await.map_err(OpenCodeError::Transport)?;
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let error = serde_json::from_slice::<StructuredError>(&bytes).ok();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(OpenCodeError::Authentication { status, error });
    }
    if status == StatusCode::NOT_FOUND && error.as_ref().is_some_and(is_missing_session) {
        return Err(OpenCodeError::MissingSession {
            status,
            error: error.expect("checked"),
        });
    }
    Err(OpenCodeError::Server {
        status,
        error,
        body,
    })
}

fn is_missing_session(error: &StructuredError) -> bool {
    error.name.as_deref() == Some("NotFoundError")
        && error
            .data
            .as_ref()
            .and_then(|value| value.get("message"))
            .and_then(Value::as_str)
            .or(error.message.as_deref())
            .is_some_and(|message| message.to_ascii_lowercase().contains("session"))
}

pub struct EventStream {
    events: mpsc::Receiver<Result<OpenCodeEvent, OpenCodeError>>,
    cancel: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    unknown_count: u64,
}

impl EventStream {
    pub async fn next(&mut self) -> Result<OpenCodeEvent, OpenCodeError> {
        match self.events.recv().await {
            Some(Ok(event)) => {
                if event.is_unknown() {
                    self.unknown_count += 1;
                }
                Ok(event)
            }
            Some(Err(error)) => Err(error),
            None => Err(OpenCodeError::StreamClosed),
        }
    }
    pub fn unknown_count(&self) -> u64 {
        self.unknown_count
    }
    fn close(&mut self) {
        let _ = self.cancel.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
    pub async fn cancel(mut self) {
        let _ = self.cancel.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        let _ = self.cancel.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
