//! HTTP ownership and event-stream lifetime for OpenCode.

use std::{
    collections::HashMap, fmt, net::TcpListener as StdTcpListener, path::Path, process::Stdio,
    time::Duration,
};

use futures_util::StreamExt;
use reqwest::{Method, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
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
            Self::Server { status, error, .. } => {
                write!(formatter, "OpenCode request failed ({status})")?;
                if let Some(detail) = structured_detail(error.as_ref()) {
                    write!(formatter, ": {detail}")?;
                }
                Ok(())
            }
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

fn structured_detail(error: Option<&StructuredError>) -> Option<String> {
    let error = error?;
    match (error.name.as_deref(), error.message.as_deref()) {
        (Some(name), Some(message)) => Some(format!("{name}: {message}")),
        (Some(name), None) => Some(name.to_string()),
        (None, Some(message)) => Some(message.to_string()),
        (None, None) => None,
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
    pub async fn config(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "config", Option::<&()>::None)
            .await
    }
    pub async fn session_statuses(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "session/status", Option::<&()>::None)
            .await
    }
    pub async fn agents(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "agent", Option::<&()>::None)
            .await
    }
    /// Every skill this server would load, in the shape `opencode debug skill`
    /// prints — the two are the same list read the same way, which is what lets
    /// a local instance and an external one publish the same catalogue.
    pub async fn skills(&self) -> Result<Value, OpenCodeError> {
        self.request_json(Method::GET, "skill", Option::<&()>::None)
            .await
    }
    pub async fn register_mcp(&self, session: &crate::mcp::Session) -> Result<(), String> {
        let body = serde_json::json!({
            "name":"laplus",
            "config":{
                "type":"remote",
                "url":session.endpoint(),
                "headers":{"Authorization":session.authorization()},
                "oauth":false
            }
        });
        let statuses: Value = self.request_json(Method::POST, "mcp", Some(&body)).await
            .map_err(|error| redact(error.to_string(), Some(session.authorization())))?;
        let status = statuses.pointer("/laplus/status").and_then(Value::as_str);
        if status == Some("connected") { return Ok(()); }
        let detail = statuses.pointer("/laplus/error").and_then(Value::as_str)
            .or_else(|| statuses.pointer("/laplus/message").and_then(Value::as_str));
        Err(match detail {
            Some(detail) => format!("OpenCode could not connect to Laplus MCP: {}", redact(detail.to_string(), Some(session.authorization()))),
            None => format!("OpenCode did not connect to Laplus MCP (status: {}).", status.unwrap_or("missing")),
        })
    }
    pub async fn create_session(&self, body: &Value) -> Result<Session, OpenCodeError> {
        self.request_json(Method::POST, "session", Some(body)).await
    }
    pub async fn update_session(&self, id: &str, body: &Value) -> Result<Session, OpenCodeError> {
        self.request_json(Method::PATCH, &format!("session/{id}"), Some(body))
            .await
    }
    pub async fn session(&self, id: &str) -> Result<Session, OpenCodeError> {
        self.request_json(Method::GET, &format!("session/{id}"), Option::<&()>::None)
            .await
    }
    pub async fn messages(&self, id: &str) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::GET,
            &format!("session/{id}/message"),
            Option::<&()>::None,
        )
        .await
    }
    pub async fn fork_session(&self, id: &str) -> Result<Session, OpenCodeError> {
        self.request_json(Method::POST, &format!("session/{id}/fork"), Some(&serde_json::json!({})))
            .await
    }
    pub async fn move_session(&self, id: &str, directory: &str) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::POST,
            "experimental/control-plane/move-session",
            Some(&serde_json::json!({
                "sessionID": id,
                "destination": {"directory": directory},
                "moveChanges": false
            })),
        )
        .await
    }
    pub async fn prompt(&self, id: &str, body: &Value) -> Result<(), OpenCodeError> {
        let response = self
            .request(
                Method::POST,
                &format!("session/{id}/prompt_async"),
                Some(body),
            )
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(OpenCodeError::Transport)?;
        classify_response(response, self.password.as_deref())
            .await
            .map(|_| ())
    }
    pub async fn prompt_sync(&self, id: &str, body: &Value) -> Result<Value, OpenCodeError> {
        self.request_json(Method::POST, &format!("session/{id}/message"), Some(body))
            .await
    }
    pub async fn delete_session(&self, id: &str) -> Result<Value, OpenCodeError> {
        self.request_json(Method::DELETE, &format!("session/{id}"), Option::<&()>::None)
            .await
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
    pub async fn reply_legacy_permission(
        &self,
        session_id: &str,
        id: &str,
        body: &Value,
    ) -> Result<Value, OpenCodeError> {
        self.request_json(
            Method::POST,
            &format!("session/{session_id}/permissions/{id}"),
            Some(body),
        )
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
        let response = classify_response(response, self.password.as_deref()).await?;
        let (events_tx, events_rx) = mpsc::channel(32);
        let (cancel_tx, mut cancel_rx) = watch::channel(false);
        let password = self.password.clone();
        let task = tokio::spawn(async move {
            let mut chunks = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            loop {
                tokio::select! {
                    changed = cancel_rx.changed() => if changed.is_ok() && *cancel_rx.borrow() { break },
                    chunk = chunks.next() => match chunk {
                        Some(Ok(bytes)) => for decoded in decoder.push(&bytes) {
                            if events_tx.send(decoded.map_err(|error| OpenCodeError::MalformedSse(redact_sse(error, password.as_deref())))).await.is_err() { return; }
                        },
                        Some(Err(error)) => { let _ = events_tx.send(Err(OpenCodeError::Transport(error))).await; return; }
                        None => { if let Some(error) = decoder.finish() { let _ = events_tx.send(Err(OpenCodeError::MalformedSse(redact_sse(error, password.as_deref())))).await; } return; }
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
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(OpenCodeError::Transport)?;
        let bytes = classify_response(response, self.password.as_deref())
            .await?
            .bytes()
            .await
            .map_err(OpenCodeError::Transport)?;
        serde_json::from_slice(&bytes).map_err(|source| OpenCodeError::MalformedJson {
            source,
            body: redact(
                String::from_utf8_lossy(&bytes).into_owned(),
                self.password.as_deref(),
            ),
        })
    }
}

/// Roll an OpenCode conversation back by completed assistant turns.
///
/// OpenCode names the retained boundary with an assistant message id rather
/// than accepting a count. Read the full history and translate the checkpoint
/// count exactly as the upstream adapter does: keep the assistant message just
/// before the removed suffix, or omit `messageID` when the whole history goes.
/// The durable cursor is only read here; a failed rollback therefore cannot
/// replace the continuation identity remembered by the thread.
pub async fn rollback(
    start: &crate::session::Start,
    removed_turns: u64,
) -> Result<(), String> {
    if removed_turns == 0 {
        return Ok(());
    }

    let resume_session_id = resume_session_id(start)?;
    let settings = start.driver.opencode()?;
    let (mut owned, client) = if settings.server_url.is_empty() {
        let (binary, _) = crate::provider::resolve_named(
            &settings.binary_path,
            "opencode",
            &crate::process::Search::from_environment(),
        )
        .startable_for("OpenCode CLI")?;
        let (owned, client) = OwnedServer::start(&binary, &start.workspace_root).await?;
        (Some(owned), client)
    } else {
        let password =
            (!settings.server_password.is_empty()).then(|| settings.server_password.clone());
        (
            None,
            OpenCodeClient::new(&settings.server_url, &start.workspace_root, password)
                .map_err(|error| error.to_string())?,
        )
    };

    let result = async {
        let session = recover_session(&client, start, resume_session_id.as_deref(), false).await?;
        let messages = client
            .messages(&session.id)
            .await
            .map_err(|error| error.to_string())?;
        let entries = messages
            .as_array()
            .ok_or_else(|| "OpenCode returned a malformed session message list".to_string())?;
        let assistant_ids = entries
            .iter()
            .filter_map(|entry| {
                let info = entry.get("info")?;
                (info.get("role").and_then(Value::as_str) == Some("assistant"))
                    .then(|| info.get("id").and_then(Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>();
        let removed = usize::try_from(removed_turns).unwrap_or(usize::MAX);
        let target = assistant_ids
            .len()
            .checked_sub(removed.saturating_add(1))
            .and_then(|index| assistant_ids.get(index).copied());
        let body = target
            .map(|message_id| serde_json::json!({ "messageID": message_id }))
            .unwrap_or_else(|| serde_json::json!({}));
        client
            .revert(&session.id, &body)
            .await
            .map_err(|error| error.to_string())?;
        Ok::<(), String>(())
    }
    .await;

    if let Some(owned) = owned.as_mut() {
        owned.stop().await;
    }
    result
}

/// The tool OpenCode spawns a subagent with. `explore` and `general` are its
/// stock ones (`GET /agent`, `"mode": "subagent"`), and a project may configure
/// more; the tool is the same either way, and which agent ran is `subagent_type`
/// in its input.
const TASK_TOOL: &str = "task";

/// What [`OpenCode::subagent_row`] decided to draw for a part, which is a
/// three-way question the [`Option`] it used to return could only answer two
/// ways. Not to be confused with [`SubagentRow`], which is what is *known* about
/// a subagent; this is what to put on screen for one part.
///
/// The two silences mean opposite things. "Not a subagent" invites the ordinary
/// [`tool_activity`] to draw the part; "nothing to draw yet" asks for no row at
/// all, and answering it with the tool row is what put a second row, keyed on the
/// bare call id, beside every subagent's own — see
/// `a_subagent_is_not_also_drawn_as_a_tool_called_task`.
enum Drawn {
    /// Some other tool. The ordinary tool row should handle it.
    NotASubagent,
    /// A subagent's part, but not one worth drawing yet. The naming rule in
    /// [`OpenCode::subagent_row`] says why waiting is right; this says that
    /// waiting means waiting rather than falling through.
    TooEarly,
    Row(crate::threads::Activity),
}

/// What is known about one subagent, accumulated across the events that mention
/// it.
///
/// Held because no single event carries the whole row: the `task` part knows
/// which agent was asked for and what for, the child session knows what it is
/// saying, and the ending knows the answer. Each arrives without the others.
#[derive(Default)]
struct SubagentRow {
    /// Which subagent — `explore`, `general`, or a project's own.
    kind: Option<String>,
    /// What it was asked for, as the parent described it.
    description: Option<String>,
    /// The last thing it said, from its own session.
    said: Option<String>,
    /// Its `task` call has reported, so nothing may reopen the row.
    finished: bool,
}

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const READINESS_POLL: Duration = Duration::from_millis(50);
const EXIT_GRACE: Duration = Duration::from_secs(2);
/// How long one OpenCode request may take before it is a failure rather than a
/// wait.
///
/// `reqwest::Client::new()` has **no** request timeout, and every call this
/// client makes used to inherit that. An OpenCode wedged on a stalled provider
/// socket — which is the ordinary failure of an OpenAI-compatible proxy, and
/// has no default `chunkTimeout` above it (opencode#37580) — answers its HTTP
/// port never rather than slowly, and two of these calls are awaited in places
/// where "never" is fatal to more than the call:
///
/// - [`OpenCode::send`] is awaited by the session loop *before* its `select!`,
///   so a hung prompt stops the loop reading its own signals. The developer's
///   Stop does nothing, no event is normalized, and the conversation shows
///   Working for as long as the process lives.
/// - [`OpenCode::stop`] aborts the remote session before reaping the child, so
///   a hung abort meant the owned server was never killed. This machine had 64
///   `opencode serve` processes and 5.35 GB of them, three days deep.
///
/// Applied per request rather than on the client, because
/// [`OpenCodeClient::subscribe`] is a stream that is *supposed* to stay open
/// and a client-wide timeout would end it here.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The bound on asking OpenCode to stop its own work while laplus is reaping
/// it. Shorter than [`REQUEST_TIMEOUT`], because nothing downstream of this
/// call needs its answer: what follows is killing the process either way, and
/// the only thing a longer wait buys is a longer leak.
const ABORT_TIMEOUT: Duration = Duration::from_secs(5);
// OpenCode's `/global/health` answers `healthy: true` before its provider
// catalogue finishes initialising. The catalogue wait below is what turns
// `/global/health` answering green into "every subsequent `client.providers()`
// call returns a populated inventory". Six seconds warm, longer cold, never
// longer than this on a healthy install.
const CATALOGUE_TIMEOUT: Duration = Duration::from_secs(30);
const CATALOGUE_POLL: Duration = Duration::from_millis(250);
const EXIT_POLL: Duration = Duration::from_millis(20);

/// One loopback OpenCode server owned by a conversation.
pub(crate) struct OwnedServer {
    child: Child,
    process_group_id: u32,
}

impl OwnedServer {
    pub(crate) async fn start(binary: &Path, directory: &str) -> Result<(Self, OpenCodeClient), String> {
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
        crate::process::without_a_console(command.as_std_mut());
        let child = command.spawn().map_err(|error| {
            format!(
                "The OpenCode binary {} could not be started in {directory}: {error}",
                binary.display()
            )
        })?;
        let process_group_id = child
            .id()
            .ok_or_else(|| "OpenCode started without a process id.".to_string())?;
        let mut owned = Self {
            child,
            process_group_id,
        };
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
            Ok(Ok(())) => {
                wait_for_catalogue(&client, &mut owned).await;
                Ok((owned, client))
            }
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

    pub(crate) async fn stop(&mut self) {
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
        if force {
            command.arg("/F");
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
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

/// Block until OpenCode's `/provider` inventory has at least one connected
/// provider and one populated provider entry, or the timeout elapses. The
/// health endpoint answers green before the catalogue finishes initialising,
/// and `client.providers()` in `OpenCode::open` silently caches whatever it
/// sees at that moment — empty `connected`, empty `all`, no `maxTokens` rows
/// for the rest of the conversation. This wait turns "health is green" into
/// "every later `client.providers()` call sees a populated inventory".
///
/// **Non-fatal**: if the catalogue does not populate in time, or opencode exits
/// during the wait, log a warning and proceed with an empty `context_windows`
/// map — the rest of the conversation still works, the context meter just
/// shows only the token count with no percentage. Failing the session open
/// here was a regression because laplus spawns its owned opencode with
/// `OPENCODE_CONFIG_CONTENT = "{}"`, which yields an empty catalogue on
/// builds whose bundled opencode doesn't repopulate it post-startup.
async fn wait_for_catalogue(
    client: &OpenCodeClient,
    owned: &mut OwnedServer,
) {
    let polled = tokio::time::timeout(CATALOGUE_TIMEOUT, async {
        loop {
            if let Ok(Some(status)) = owned.child.try_wait() {
                eprintln!(
                    "laplus: OpenCode exited before its catalogue populated ({status}); \
                     context meter will show token count without a window for this conversation."
                );
                return;
            }
            if let Ok(value) = client.providers().await {
                let connected = value
                    .get("connected")
                    .and_then(Value::as_array)
                    .map(|list| !list.is_empty())
                    .unwrap_or(false);
                let populated = crate::provider::opencode_catalogue_models(&value)
                    .is_ok_and(|models| !models.is_empty());
                if connected && populated {
                    return;
                }
            }
            tokio::time::sleep(CATALOGUE_POLL).await;
        }
    })
    .await;
    if let Err(_) = polled {
        eprintln!(
            "laplus: OpenCode catalogue did not populate within {} seconds; \
             context meter will show token count without a window for this conversation.",
            CATALOGUE_TIMEOUT.as_secs()
        );
    }
}

pub(crate) struct OpenCode {
    client: OpenCodeClient,
    events: EventStream,
    session_id: String,
    /// Present only when Laplus launched the endpoint. An operator-owned
    /// endpoint shares all session behavior, but its lifetime is never ours.
    owned: Option<OwnedServer>,
    mcp_session: Option<crate::mcp::Session>,
    model: Option<String>,
    context_windows: HashMap<String, u64>,
    compacts_automatically: Option<bool>,
    settled: bool,
    roles: HashMap<String, String>,
    pending_parts: HashMap<String, Value>,
    pending_deltas: HashMap<String, String>,
    emitted_parts: HashMap<String, String>,
    assistant_text: String,
    ignore_idle_until_busy: bool,
    pending_permissions: HashMap<String, crate::approval::ApprovalRequest>,
    pending_questions: HashMap<String, crate::approval::ApprovalRequest>,
    /// One row per subagent, by the id of the `task` call that owns it.
    subagent_rows: HashMap<String, SubagentRow>,
    /// Which `task` call a child session belongs to.
    ///
    /// OpenCode runs a subagent as a *session of its own* — `session.created`
    /// with a `parentID` — and every event it produces arrives on the same
    /// stream as its parent's. Without this they are simply discarded as
    /// belonging to another conversation, which is true and is also why a
    /// subagent used to be a row that said nothing from the moment it started
    /// until the moment it finished.
    subagent_sessions: HashMap<String, String>,
}

fn context_windows(providers: &Value) -> HashMap<String, u64> {
    crate::provider::opencode_catalogue_models(providers)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|model| model.context_window.map(|window| (model.slug, window)))
        .collect()
}

fn message_token_usage(
    info: &Value,
    context_windows: &HashMap<String, u64>,
    compacts_automatically: Option<bool>,
) -> Option<crate::protocol::TokenUsage> {
    let tokens = info.get("tokens")?;
    let input = tokens.get("input").and_then(Value::as_u64)?;
    let cache_read = tokens.pointer("/cache/read").and_then(Value::as_u64).unwrap_or(0);
    let cache_write = tokens.pointer("/cache/write").and_then(Value::as_u64).unwrap_or(0);
    let output = tokens.get("output").and_then(Value::as_u64)?;
    let input_tokens = input.saturating_add(cache_read).saturating_add(cache_write);
    let used_tokens = tokens
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| input_tokens.saturating_add(output));
    if used_tokens == 0 {
        return None;
    }
    // OpenCode puts `providerID`/`modelID` at the top level of an assistant
    // message's `info`, but nests them under `info.model` on a user message.
    // Token usage arrives on assistant messages, so the top-level form is the
    // one that matters — read it first, fall back to the nested form.
    let slug = info
        .get("providerID")
        .and_then(Value::as_str)
        .zip(info.get("modelID").and_then(Value::as_str))
        .or_else(|| {
            let model = info.get("model")?;
            Some((model.get("providerID")?.as_str()?, model.get("modelID")?.as_str()?))
        })
        .map(|(provider_id, model_id)| format!("{provider_id}/{model_id}"));
    Some(crate::protocol::TokenUsage {
        used_tokens,
        total_processed_tokens: None,
        max_tokens: slug.as_ref().and_then(|slug| context_windows.get(slug)).copied(),
        input_tokens: Some(input_tokens),
        output_tokens: Some(output),
        compacts_automatically,
    })
}

fn question_id(index: usize, header: &str) -> String {
    let slug = header.to_lowercase().chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect::<String>().split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-");
    if slug.is_empty() { format!("question-{index}") } else { format!("question-{index}-{slug}") }
}

fn question_request(properties: &Value) -> Option<crate::approval::ApprovalRequest> {
    let id = properties.get("id")?.as_str()?;
    let questions = properties.get("questions")?.as_array()?.iter().enumerate().map(|(index, question)| {
        let mut question = question.clone();
        let header = question.get("header").and_then(Value::as_str).unwrap_or("");
        question["id"] = Value::String(question_id(index, header));
        question["multiSelect"] = Value::Bool(question.get("multiple").and_then(Value::as_bool).unwrap_or(false));
        question
    }).collect::<Vec<_>>();
    Some(crate::approval::ApprovalRequest {
        request_id: id.to_string(),
        tool_name: crate::worklog::ASK_USER_QUESTION.to_string(),
        input: serde_json::json!({"questions": questions}),
        tool_use_id: None,
        description: Some("OpenCode needs your input".to_string()),
        suggestions: Vec::new(),
        available_decisions: None,
        provider_request_id: Some(serde_json::json!({"kind":"question.asked","id":id})),
    })
}

fn ordered_question_answers(request: &crate::approval::ApprovalRequest, answers: &Value) -> Value {
    Value::Array(request.input["questions"].as_array().into_iter().flatten().map(|question| {
        let id = question.get("id").and_then(Value::as_str).unwrap_or("");
        match answers.get(id) {
            Some(Value::Array(values)) => Value::Array(values.clone()),
            Some(Value::String(value)) => serde_json::json!([value]),
            _ => serde_json::json!([]),
        }
    }).collect())
}

fn keyed_question_answers(request: &crate::approval::ApprovalRequest, answers: &Value) -> Value {
    let ordered = answers.as_array();
    Value::Object(request.input["questions"].as_array().into_iter().flatten().enumerate().filter_map(|(index, question)| {
        let id = question.get("id")?.as_str()?.to_string();
        Some((id, ordered.and_then(|answers| answers.get(index)).cloned().unwrap_or_else(|| serde_json::json!([]))))
    }).collect())
}

/// The OpenCode permission rules for a shared runtime mode. Ticket 15 uses the
/// same value when adopting or forking a session; keeping it here prevents
/// resume from growing a second, subtly different access translation.
pub(crate) fn permission_rules(runtime_mode: &str) -> Value {
    if runtime_mode == "full-access" {
        return serde_json::json!([{"permission":"*","pattern":"*","action":"allow"}]);
    }
    serde_json::json!([
        {"permission":"*","pattern":"*","action":"ask"},
        {"permission":"bash","pattern":"*","action":"ask"},
        {"permission":"edit","pattern":"*","action":"ask"},
        {"permission":"webfetch","pattern":"*","action":"ask"},
        {"permission":"websearch","pattern":"*","action":"ask"},
        {"permission":"codesearch","pattern":"*","action":"ask"},
        {"permission":"external_directory","pattern":"*","action":"ask"},
        {"permission":"doom_loop","pattern":"*","action":"ask"},
        {"permission":"question","pattern":"*","action":"allow"}
    ])
}

fn tool_activity(
    part: &Value,
    raw: &Value,
    turn_id: Option<String>,
) -> Option<crate::threads::Activity> {
    if part.get("type").and_then(Value::as_str) != Some("tool") {
        return None;
    }
    let id = part.get("callID").and_then(Value::as_str)?;
    let tool = part.get("tool").and_then(Value::as_str).unwrap_or("Tool");
    let state = part.get("state")?;
    let status = state
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("pending");
    let (kind, shared_status) = match status {
        "pending" => ("tool.started", "inProgress"),
        "completed" => ("tool.completed", "completed"),
        "error" => ("tool.completed", "failed"),
        _ => ("tool.updated", "inProgress"),
    };
    let detail = match status {
        "completed" => state.get("output").cloned(),
        "error" => state.get("error").cloned(),
        _ => state
            .get("title")
            .cloned()
            .or_else(|| state.get("input").cloned()),
    };
    let mut payload = serde_json::json!({
        "itemType": crate::worklog::opencode_item_type(tool), "status": shared_status, "title": tool,
        "data": {"toolCallId": id, "toolName": tool, "tool": tool, "state": state, "raw": raw}
    });
    if let Some(detail) = detail {
        payload["detail"] = detail;
    }
    Some(crate::threads::Activity::tool(kind, tool, payload, turn_id))
}

fn permission_request(
    properties: &Value,
    legacy: bool,
) -> Option<crate::approval::ApprovalRequest> {
    let id = properties.get("id")?.as_str()?;
    let permission = properties
        .get(if legacy { "type" } else { "permission" })?
        .as_str()?;
    let patterns = properties
        .get("patterns")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|v| !v.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        });
    let description = patterns
        .filter(|v| !v.is_empty())
        .or_else(|| Some(permission.to_string()));
    Some(crate::approval::ApprovalRequest {
        request_id: id.to_string(),
        tool_name: permission.to_string(),
        input: properties
            .get("metadata")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        tool_use_id: properties
            .pointer("/tool/callID")
            .or_else(|| properties.get("callID"))
            .and_then(Value::as_str)
            .map(str::to_string),
        description,
        suggestions: Vec::new(),
        available_decisions: Some(vec![
            crate::worklog::Decision::Accept,
            crate::worklog::Decision::AcceptForSession,
            crate::worklog::Decision::Decline,
            crate::worklog::Decision::Cancel,
        ]),
        provider_request_id: Some(
            serde_json::json!({"kind":if legacy {"permission.updated"} else {"permission.asked"},"id":id}),
        ),
    })
}

fn cursor(start: &crate::session::Start, session_id: &str) -> crate::provider::ResumeCursor {
    crate::provider::ResumeCursor {
        provider: start.provider.clone(),
        value: serde_json::json!({"version": 1, "sessionId": session_id}),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V1Cursor {
    version: u64,
    #[serde(rename = "sessionId")]
    session_id: String,
}

fn resume_session_id(start: &crate::session::Start) -> Result<Option<String>, String> {
    let Some(cursor) = &start.resume_cursor else {
        return Ok(None);
    };
    let parsed: V1Cursor = serde_json::from_value(cursor.value.clone())
        .map_err(|_| "OpenCode provider resume cursor is malformed or incompatible.".to_string())?;
    if parsed.version != 1 || parsed.session_id.is_empty() {
        return Err("OpenCode provider resume cursor is malformed or incompatible.".to_string());
    }
    Ok(Some(parsed.session_id))
}

fn session_directory(session: &Session) -> Result<&str, String> {
    session
        .extra
        .get("directory")
        .and_then(Value::as_str)
        .filter(|directory| !directory.is_empty())
        .ok_or_else(|| "OpenCode session did not report its working directory.".to_string())
}

fn canonical_directory(path: &str) -> Result<std::path::PathBuf, String> {
    std::fs::canonicalize(path)
        .map_err(|error| format!("OpenCode session directory {path} could not be verified: {error}"))
}

fn session_is_in(session: &Session, directory: &str) -> Result<bool, String> {
    let requested = canonical_directory(directory)?;
    match std::fs::canonicalize(session_directory(session)?) {
        Ok(recovered) => Ok(recovered == requested),
        // A removed worktree is necessarily not the requested, existing one.
        // Recovery must still be allowed to fork its durable history.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "OpenCode session directory could not be verified: {error}"
        )),
    }
}

async fn create_session(
    client: &OpenCodeClient,
    start: &crate::session::Start,
) -> Result<Session, String> {
    client
        .create_session(&serde_json::json!({
            "permission": permission_rules(&start.runtime_mode)
        }))
        .await
        .map_err(|error| error.to_string())
}

async fn recover_session(
    client: &OpenCodeClient,
    start: &crate::session::Start,
    session_id: Option<&str>,
    missing_starts_fresh: bool,
) -> Result<Session, String> {
    let Some(session_id) = session_id else {
        return if missing_starts_fresh {
            create_session(client, start).await
        } else {
            Err("OpenCode rollback requires an adopted provider resume cursor.".to_string())
        };
    };

    let recovered = match client.session(session_id).await {
        Ok(session) => session,
        Err(OpenCodeError::MissingSession { .. }) if missing_starts_fresh => {
            return create_session(client, start).await;
        }
        Err(error) => return Err(error.to_string()),
    };

    let adopted = if session_is_in(&recovered, &start.workspace_root)? {
        recovered
    } else {
        let forked = client
            .fork_session(&recovered.id)
            .await
            .map_err(|error| error.to_string())?;
        if session_is_in(&forked, &start.workspace_root)? {
            forked
        } else {
            client
                .move_session(&forked.id, &start.workspace_root)
                .await
                .map_err(|error| error.to_string())?;
            let moved = client
                .session(&forked.id)
                .await
                .map_err(|error| error.to_string())?;
            if !session_is_in(&moved, &start.workspace_root)? {
                return Err("OpenCode session move did not reach the requested working directory."
                    .to_string());
            }
            moved
        }
    };

    client
        .update_session(
            &adopted.id,
            &serde_json::json!({"permission": permission_rules(&start.runtime_mode)}),
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(adopted)
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

impl OpenCode {
    fn assistant_role(&self, properties: &Value) -> Option<bool> {
        let message_id = properties.get("messageID").and_then(Value::as_str)?;
        self.roles.get(message_id).map(|role| role == "assistant")
    }

    fn emit_text(
        &mut self,
        part_id: &str,
        kind: &str,
        text: &str,
        driving: &mut crate::session::Driving,
        decided: &mut crate::session::Decided,
    ) {
        let emitted = self.emitted_parts.entry(part_id.to_string()).or_default();
        // Cumulative updates reconcile true deltas. A stale snapshot is ignored;
        // a divergent snapshot cannot safely retract text already on screen.
        let suffix = if text.starts_with(emitted.as_str()) {
            &text[emitted.len()..]
        } else if emitted.starts_with(text) {
            ""
        } else {
            ""
        };
        if suffix.is_empty() {
            return;
        }
        emitted.push_str(suffix);
        if kind == "reasoning" {
            if let Some(activity) = crate::worklog::thinking(
                suffix,
                driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
            ) {
                decided
                    .changes
                    .push(crate::threads::Change::Activity(activity));
            }
            return;
        }
        let Some(active) = driving.turn.as_mut() else {
            return;
        };
        self.assistant_text.push_str(suffix);
        let message_id = active
            .assistant_message_id
            .get_or_insert_with(crate::threads::fresh_message_id)
            .clone();
        decided
            .changes
            .push(crate::threads::Change::AssistantDelta {
                message_id,
                turn_id: active.turn_id.clone(),
                text: suffix.to_string(),
            });
    }

    /// The `task` tool is a subagent, so it gets a subagent's row.
    ///
    /// Returns [`Drawn::NotASubagent`] for every other tool, which is what
    /// leaves the ordinary [`tool_activity`] to handle them. A `task` part is
    /// never handed to it, whatever this decides about drawing one — that is what
    /// stops a subagent being drawn twice, once as itself and once as a tool
    /// called `task`.
    ///
    /// The row is [`crate::worklog::subagent`], the same builder the Claude driver
    /// uses, so a subagent looks like a subagent whichever agent is running. That
    /// is worth the small translation here: the two protocols disagree about
    /// almost everything else, and this is the one place where what the developer
    /// is being told is identical.
    ///
    /// **Keyed on the call rather than on the child session.** The session id
    /// arrives in `metadata` only once the subagent has actually started, so a row
    /// keyed on it would be a second row appearing a beat after the first. The
    /// call id is there from the `pending` part onwards and lasts exactly as long
    /// as the subagent does, because OpenCode's `task` tool does not return early.
    ///
    /// **The row and the child's own stream are decided together**, from the
    /// same part, because they are two views of one child and a driver that
    /// could publish one without the other would let the launcher and what it
    /// launches disagree. The row is the compact index — identity, assignment,
    /// state, latest activity — and carries the `childId` the stream is
    /// addressed by; [`crate::subagents`] holds the work itself. The child id is
    /// the `task` call's, for the same reason the row's collapse key is: it is
    /// there from the `pending` part onwards and lasts exactly as long as the
    /// subagent.
    fn subagent_row(
        &mut self,
        part: &Value,
        driving: &crate::session::Driving,
        decided: &mut crate::session::Decided,
    ) -> Drawn {
        if part.get("type").and_then(Value::as_str) != Some("tool")
            || part.get("tool").and_then(Value::as_str) != Some(TASK_TOOL)
        {
            return Drawn::NotASubagent;
        }
        // From here the part is a subagent's whatever else is true of it. A `task`
        // part too malformed to build a row from is still not a tool row.
        let Some(call) = part.get("callID").and_then(Value::as_str).map(str::to_string) else {
            return Drawn::TooEarly;
        };
        let Some(state) = part.get("state") else {
            return Drawn::TooEarly;
        };
        let status = state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let field = |object: Option<&Value>, name: &str| {
            object
                .and_then(|object| object.get(name))
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let input = state.get("input");

        // The join, recorded as soon as the subagent names itself: from here its
        // own events can find this row instead of being dropped.
        if let Some(child) = field(state.get("metadata"), "sessionId") {
            self.subagent_sessions.insert(child, call.clone());
        }

        let row = self.subagent_rows.entry(call.clone()).or_default();
        if let Some(kind) = field(input, "subagent_type") {
            row.kind = Some(kind);
        }
        if let Some(description) = field(input, "description") {
            row.description = Some(description);
        }
        row.finished |= matches!(status, "completed" | "error");
        let said = row.said.clone();
        let (kind, description) = (row.kind.clone(), row.description.clone());

        // The child's own stream, from the same part. Recorded from `pending`
        // onwards — before the row can be named — so that identity exists for
        // the whole of the child's life rather than from the moment it becomes
        // presentable.
        let update = crate::subagents::Update::for_child(call.clone())
            .named(kind.clone())
            .assigned(description.clone());
        decided.child_streams.push(match status {
            "completed" => update.concluded(crate::subagents::Outcome::completed(field(
                Some(state),
                "output",
            ))),
            "error" => update.concluded(crate::subagents::Outcome::failed(field(
                Some(state),
                "error",
            ))),
            "pending" => update.in_state(crate::subagents::State::Pending),
            _ => update.in_state(crate::subagents::State::Working),
        });

        // OpenCode announces the call before it knows what it is: the first
        // `task` part is `pending` with an empty state, and the input naming the
        // agent arrives with `running`. Publishing that first one would put a row
        // reading "Subagent task" on screen and rename it a beat later, which is
        // the defect the Claude driver has a whole record to avoid. So the row
        // waits until the subagent can be named — unless the call is already over,
        // because an unnamed subagent that failed still has to be reported.
        if kind.is_none() && !matches!(status, "completed" | "error") {
            return Drawn::TooEarly;
        }

        Drawn::Row(crate::worklog::subagent(
            &crate::protocol::SubagentTask {
                task_id: call.clone(),
                tool_use_id: Some(call.clone()),
                status: match status {
                    "completed" => "completed",
                    "error" => "failed",
                    _ => "running",
                }
                .to_string(),
                description,
                subagent_type: kind,
                summary: match status {
                    "completed" => field(Some(state), "output"),
                    "error" => field(Some(state), "error"),
                    _ => None,
                },
                said,
            },
            driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
            Some(&call),
        ))
    }

    /// Something happened in a subagent's own session. Put it in that subagent's
    /// work stream and on its row, or say nothing.
    ///
    /// **Two destinations, and they take different amounts.** The stream keeps
    /// the child's prose as an ordered entry per part, because that is the thing
    /// the developer opens the child to read. The row keeps only the latest of
    /// it, because a row is one line. The `task` part that owns both is what
    /// ends them, so nothing here needs to notice the child going idle.
    ///
    /// A part is keyed by OpenCode's own part id, which is what makes the
    /// cumulative text it resends an edit of one entry rather than a paragraph
    /// per token — see [`crate::subagents::NewEntry`].
    ///
    /// The child's tool calls, permissions and token accounting are still
    /// dropped here. They are ticket 02's, and inventing them from what this
    /// function already sees is not available: it would be prose relabelled.
    fn child_session_event(
        &mut self,
        envelope: &crate::opencode_protocol::EventEnvelope,
        driving: &crate::session::Driving,
        decided: &mut crate::session::Decided,
    ) {
        let properties = &envelope.properties;
        match envelope.kind.as_str() {
            // Recorded so the parts below can tell the subagent's own words from
            // the prompt it was handed, which arrives as a text part too.
            "message.updated" => {
                let info = properties.get("info").unwrap_or(properties);
                if let (Some(id), Some(role)) = (
                    info.get("id").and_then(Value::as_str),
                    info.get("role").and_then(Value::as_str),
                ) {
                    self.roles.insert(id.to_string(), role.to_string());
                }
            }
            "message.part.updated" => {
                let part = properties.get("part").unwrap_or(properties);
                if part.get("type").and_then(Value::as_str) != Some("text") {
                    return;
                }
                let is_the_subagent = part
                    .get("messageID")
                    .and_then(Value::as_str)
                    .and_then(|id| self.roles.get(id))
                    .is_some_and(|role| role == "assistant");
                if !is_the_subagent {
                    return;
                }
                let Some(said) = part.get("text").and_then(Value::as_str) else {
                    return;
                };
                if said.trim().is_empty() {
                    return;
                }
                let Some(call) = properties
                    .get("sessionID")
                    .and_then(Value::as_str)
                    .and_then(|session| self.subagent_sessions.get(session))
                    .cloned()
                else {
                    return;
                };
                let Some(row) = self.subagent_rows.get_mut(&call) else {
                    return;
                };
                // A subagent that has already reported does not speak again: the
                // `task` part has published its output, and reopening the row
                // would replace the answer with whatever was said on the way to
                // it. The stream is closed for the same reason and by the same
                // rule — see [`crate::subagents::Streams::record`].
                if row.finished {
                    return;
                }
                decided.child_streams.push(
                    crate::subagents::Update::for_child(call.clone())
                        .in_state(crate::subagents::State::Working)
                        .with(crate::subagents::NewEntry::said(
                            part.get("id").and_then(Value::as_str).map(str::to_string),
                            said,
                        )),
                );
                row.said = Some(said.to_string());
                let task = crate::protocol::SubagentTask {
                    task_id: call.clone(),
                    tool_use_id: Some(call.clone()),
                    status: "running".to_string(),
                    description: row.description.clone(),
                    subagent_type: row.kind.clone(),
                    summary: None,
                    said: row.said.clone(),
                };
                decided
                    .changes
                    .push(crate::threads::Change::Activity(crate::worklog::subagent(
                        &task,
                        driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
                        Some(&call),
                    )));
            }
            _ => {}
        }
    }

    fn normalize_part(
        &mut self,
        part: &Value,
        driving: &mut crate::session::Driving,
        decided: &mut crate::session::Decided,
    ) {
        let part_id = part
            .get("id")
            .or_else(|| part.get("partID"))
            .and_then(Value::as_str)
            .unwrap_or("__legacy_text");
        self.pending_parts.insert(part_id.to_string(), part.clone());
        if self.assistant_role(part) == Some(false) {
            return;
        }
        let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "text" | "reasoning") {
            return;
        }
        if let Some(delta) = self.pending_deltas.remove(part_id) {
            let previous = self.emitted_parts.get(part_id).cloned().unwrap_or_default();
            self.emit_text(part_id, kind, &(previous + &delta), driving, decided);
        }
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            self.emit_text(part_id, kind, text, driving, decided);
        }
    }

    fn normalize_delta(
        &mut self,
        properties: &Value,
        driving: &mut crate::session::Driving,
        decided: &mut crate::session::Decided,
    ) {
        let part_id = properties
            .get("partID")
            .or_else(|| properties.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("__legacy_text");
        let Some(delta) = properties.get("delta").and_then(Value::as_str) else {
            return;
        };
        let part = self.pending_parts.get(part_id).cloned();
        let Some(part) = part else {
            self.pending_deltas
                .entry(part_id.to_string())
                .or_default()
                .push_str(delta);
            return;
        };
        if self
            .assistant_role(properties)
            .or_else(|| self.assistant_role(&part))
            == Some(false)
        {
            self.pending_deltas
                .entry(part_id.to_string())
                .or_default()
                .push_str(delta);
            return;
        }
        let kind = part.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(kind, "text" | "reasoning") {
            return;
        }
        let cumulative = self.emitted_parts.get(part_id).cloned().unwrap_or_default() + delta;
        self.emit_text(part_id, kind, &cumulative, driving, decided);
    }

    fn settle(
        &mut self,
        driving: &mut crate::session::Driving,
        status: crate::settling::SessionStatus,
        error: Option<String>,
    ) -> crate::session::Decided {
        if self.settled {
            return Default::default();
        }
        let Some(finished) = driving.turn.take() else {
            return Default::default();
        };
        let interrupted = finished.was_stopped();
        self.settled = true;
        self.ignore_idle_until_busy = true;
        let mut changes = Vec::new();
        if !self.assistant_text.is_empty() {
            let message_id = finished
                .assistant_message_id
                .unwrap_or_else(crate::threads::fresh_message_id);
            changes.push(crate::threads::Change::AssistantMessage {
                message_id,
                turn_id: finished.turn_id.clone(),
                text: self.assistant_text.clone(),
            });
        }
        if let Some(message) = error.as_deref().filter(|_| !interrupted) {
            changes.push(crate::threads::Change::Activity(
                crate::threads::Activity::failed("session.failed", message),
            ));
        } else {
            changes.push(crate::threads::Change::Activity(crate::threads::Activity::info(
                "turn.completed", if interrupted { "Turn stopped by the developer." } else { "Turn completed." },
                serde_json::json!({"durationMs":Value::Null,"totalCostUsd":Value::Null,"isError":false,"interrupted":interrupted}),
                Some(finished.turn_id.clone()),
            )));
        }
        driving.finished = Some(crate::session::Finished {
            turn_id: finished.turn_id.clone(),
            status: if interrupted {
                "interrupted"
            } else if error.is_some() {
                "error"
            } else {
                "ready"
            },
        });
        // A steer shares this turn's normalization state. Clear it only once
        // the turn settles so the next independent turn starts cleanly.
        self.pending_parts.clear();
        self.pending_deltas.clear();
        self.emitted_parts.clear();
        self.assistant_text.clear();
        crate::session::Decided {
            changes,
            settles: Some(crate::session::Settles {
                turn_id: Some(finished.turn_id),
                status: if interrupted {
                    crate::settling::SessionStatus::Interrupted
                } else {
                    status
                },
                last_error: if interrupted { None } else { error },
            }),
            ..Default::default()
        }
    }
}

fn structured_event_error(error: &Value) -> String {
    let name = error.get("name").and_then(Value::as_str);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.pointer("/data/message").and_then(Value::as_str));
    match (name, message) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (Some(name), None) => name.to_string(),
        (None, Some(message)) => message.to_string(),
        _ => "OpenCode reported a session error.".to_string(),
    }
}

pub(crate) fn prompt_parts(text: &str, attachments: &[crate::threads::PromptAttachment]) -> Vec<Value> {
    let mut parts = Vec::new();
    if !text.trim().is_empty() { parts.push(serde_json::json!({"type":"text", "text":text})); }
    parts.extend(attachments.iter().filter_map(|attachment| {
        // External endpoints receive the same local file URL as owned servers.
        let url = reqwest::Url::from_file_path(&attachment.path).ok()?;
        Some(serde_json::json!({"type":"file","mime":attachment.mime,"filename":attachment.filename,"url":url.to_string()}))
    }));
    parts
}

impl crate::session::Driver for OpenCode {
    const COALESCES_QUEUED_PROMPTS: bool = true;
    const APPROVAL_RESOLVED_BY_EVENT: bool = true;
    const USER_INPUT_RESOLVED_BY_EVENT: bool = true;
    const INTERRUPT_RECONCILIATION_AFTER: Option<std::time::Duration> = Some(std::time::Duration::from_secs(2));

    async fn open(start: &crate::session::Start) -> Result<crate::session::Opened<Self>, String> {
        // Cursor validation precedes launching an owned process or making any
        // HTTP request, so incompatible durable state cannot mutate upstream.
        let resume_session_id = resume_session_id(start)?;
        let opencode_start = start.driver.opencode_start()?;
        let settings = &opencode_start.settings;
        let (mut owned, client) = if settings.server_url.is_empty() {
            let (binary, _) = crate::provider::resolve_named(
                &settings.binary_path,
                "opencode",
                &crate::process::Search::from_environment(),
            )
            .startable_for("OpenCode CLI")?;
            let (owned, client) = OwnedServer::start(&binary, &start.workspace_root).await?;
            (Some(owned), client)
        } else {
            let password =
                (!settings.server_password.is_empty()).then(|| settings.server_password.clone());
            let client = OpenCodeClient::new(&settings.server_url, &start.workspace_root, password)
                .map_err(|error| error.to_string())?;
            (None, client)
        };
        let mcp_session = if owned.is_some() {
            let session = match opencode_start.mcp.open_session(&start.thread_id) {
                Ok(session) => session,
                Err(error) => {
                    if let Some(owned) = owned.as_mut() { owned.stop().await; }
                    return Err(error.to_string());
                }
            };
            if let Err(error) = client.register_mcp(&session).await {
                drop(session);
                if let Some(owned) = owned.as_mut() { owned.stop().await; }
                return Err(format!("OpenCode MCP registration failed: {error}"));
            }
            Some(session)
        } else {
            None
        };
        let context_windows = client
            .providers()
            .await
            .ok()
            .map(|providers| context_windows(&providers))
            .unwrap_or_default();
        let compacts_automatically = client
            .config()
            .await
            .ok()
            .map(|config| config.pointer("/compaction/auto").and_then(Value::as_bool).unwrap_or(true));
        let opened = async {
            let session = recover_session(&client, start, resume_session_id.as_deref(), true).await?;
            let events = client.subscribe().await.map_err(|error| error.to_string())?;
            Ok::<_, String>((events, session))
        }
        .await;
        let (events, session) = match opened {
            Ok(value) => value,
            Err(error) => {
                if let Some(owned) = owned.as_mut() {
                    owned.stop().await;
                }
                return Err(error);
            }
        };
        Ok(crate::session::Opened {
            driver: Self {
                client,
                events,
                session_id: session.id.clone(),
                owned,
                mcp_session,
                model: start.model.clone(),
                context_windows,
                compacts_automatically,
                settled: false,
                roles: HashMap::new(),
                pending_parts: HashMap::new(),
                pending_deltas: HashMap::new(),
                emitted_parts: HashMap::new(),
                assistant_text: String::new(),
                ignore_idle_until_busy: false,
                pending_permissions: HashMap::new(),
                pending_questions: HashMap::new(),
                subagent_rows: HashMap::new(),
                subagent_sessions: HashMap::new(),
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
        let unknown = event.is_unknown();
        let envelope = event.envelope();
        // Another conversation's event — almost always. The exception is a
        // subagent, which OpenCode runs as a session of its own whose events
        // arrive here: those belong to a row in *this* conversation, so they are
        // offered to it before the rest are dropped.
        if event_session(&envelope.properties).is_some_and(|id| id != self.session_id) {
            let mut decided = crate::session::Decided::default();
            // Known children only. The `task` part reaches `running` carrying the
            // child's session id before the child produces anything — event 4 of
            // the capture, against event 5 — so nothing is missed by requiring
            // the introduction first, and every other conversation on the stream
            // stays out of this one's bookkeeping.
            if event_session(&envelope.properties)
                .is_some_and(|id| self.subagent_sessions.contains_key(id))
            {
                self.child_session_event(envelope, driving, &mut decided);
            }
            return Some(decided);
        }
        let mut decided = crate::session::Decided::default();
        match envelope.kind.as_str() {
            "message.updated" => {
                let info = envelope
                    .properties
                    .get("info")
                    .unwrap_or(&envelope.properties);
                if let (Some(id), Some(role)) = (
                    info.get("id").and_then(Value::as_str),
                    info.get("role").and_then(Value::as_str),
                ) {
                    self.roles.insert(id.to_string(), role.to_string());
                    if role == "assistant" {
                        let parts = self
                            .pending_parts
                            .values()
                            .filter(|part| {
                                part.get("messageID").and_then(Value::as_str) == Some(id)
                            })
                            .cloned()
                            .collect::<Vec<_>>();
                        for part in parts {
                            self.normalize_part(&part, driving, &mut decided);
                        }
                    }
                }
                if let Some(usage) = driving
                    .usage_to_report(message_token_usage(
                        info,
                        &self.context_windows,
                        self.compacts_automatically,
                    ))
                {
                    let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
                    decided.changes.push(crate::threads::Change::Activity(
                        crate::turn::context_window_row(&usage, turn_id),
                    ));
                }
            }
            "message.part.delta" => {
                if envelope.properties.get("field").and_then(Value::as_str) == Some("text") {
                    self.normalize_delta(&envelope.properties, driving, &mut decided);
                }
            }
            "message.part.updated" => {
                let part = envelope
                    .properties
                    .get("part")
                    .unwrap_or(&envelope.properties);
                // A subagent before a tool, because `task` is both and only one of
                // them is what the developer wants to read. `TooEarly` draws
                // nothing *and falls through to nothing* — the tool row is not a
                // consolation prize for a row that is not ready.
                match self.subagent_row(part, driving, &mut decided) {
                    Drawn::Row(activity) => decided
                        .changes
                        .push(crate::threads::Change::Activity(activity)),
                    Drawn::TooEarly => {}
                    Drawn::NotASubagent => {
                        if let Some(activity) = tool_activity(
                            part,
                            &serde_json::to_value(envelope).unwrap_or(Value::Null),
                            driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
                        ) {
                            decided
                                .changes
                                .push(crate::threads::Change::Activity(activity));
                        }
                    }
                }
                self.normalize_part(part, driving, &mut decided);
            }
            "permission.asked" | "permission.updated" => {
                let legacy = envelope.kind == "permission.updated";
                if let Some(request) = permission_request(&envelope.properties, legacy) {
                    if !self.pending_permissions.contains_key(&request.request_id) {
                        self.pending_permissions
                            .insert(request.request_id.clone(), request.clone());
                        driving
                            .outstanding
                            .insert(request.request_id.clone(), request.clone());
                        decided.changes.push(crate::threads::Change::Activity(
                            crate::worklog::requested(
                                &request,
                                driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
                            ),
                        ));
                    }
                }
            }
            "permission.replied" => {
                let id = envelope
                    .properties
                    .get("requestID")
                    .or_else(|| envelope.properties.get("permissionID"))
                    .and_then(Value::as_str);
                if let Some(id) = id {
                    let request = self.pending_permissions.remove(id)
                        .or_else(|| driving.outstanding.get(id).cloned());
                    driving.outstanding.remove(id);
                    let decision = match envelope.properties.get("reply")
                        .or_else(|| envelope.properties.get("response"))
                        .and_then(Value::as_str) {
                        Some("once") => Some(crate::worklog::Decision::Accept),
                        Some("always") => Some(crate::worklog::Decision::AcceptForSession),
                        Some("reject") => Some(crate::worklog::Decision::Decline),
                        _ => None,
                    };
                    if let (Some(request), Some(decision)) = (request, decision) {
                        decided.changes.push(crate::threads::Change::Activity(
                            crate::worklog::resolved(&request, decision, driving.turn.as_ref().map(|turn| turn.turn_id.clone()))
                        ));
                    }
                }
            }
            "question.asked" => {
                if let Some(request) = question_request(&envelope.properties) {
                    if !self.pending_questions.contains_key(&request.request_id)
                        && !self.pending_permissions.contains_key(&request.request_id) {
                        self.pending_questions.insert(request.request_id.clone(), request.clone());
                        driving.outstanding.insert(request.request_id.clone(), request.clone());
                        if let Some(questions) = crate::worklog::questions(&request) {
                            decided.changes.push(crate::threads::Change::Activity(
                                crate::worklog::user_input_requested(&request, questions, driving.turn.as_ref().map(|turn| turn.turn_id.clone()))
                            ));
                        }
                    }
                }
            }
            "question.replied" | "question.rejected" => {
                if let Some(id) = envelope.properties.get("requestID").and_then(Value::as_str) {
                    if let Some(request) = self.pending_questions.remove(id) {
                        driving.outstanding.remove(id);
                        let answers = if envelope.kind == "question.replied" {
                            keyed_question_answers(&request, envelope.properties.get("answers").unwrap_or(&Value::Null))
                        } else { serde_json::json!({}) };
                        decided.changes.push(crate::threads::Change::Activity(
                            crate::worklog::user_input_resolved(id, &answers, driving.turn.as_ref().map(|turn| turn.turn_id.clone()))
                        ));
                    }
                }
            }
            "session.updated" => {
                let info = envelope
                    .properties
                    .get("info")
                    .unwrap_or(&envelope.properties);
                if let Some(title) = info
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                {
                    decided.changes.push(crate::threads::Change::MetaUpdated(
                        crate::threads::MetaUpdate {
                            title: Some(title.to_string()),
                            title_regeneration: Some(None),
                            regenerate_title: false,
                            previous_title: None,
                            model_selection: None,
                            branch: None,
                            worktree_path: None,
                        },
                    ));
                }
            }
            "session.idle" => {
                if self.ignore_idle_until_busy { return Some(decided); }
                return Some(self.settle(driving, crate::settling::SessionStatus::Ready, None))
            }
            "session.status"
                if envelope
                    .properties
                    .pointer("/status/type")
                    .and_then(Value::as_str)
                    == Some("idle") =>
            {
                if self.ignore_idle_until_busy { return Some(decided); }
                return Some(self.settle(driving, crate::settling::SessionStatus::Ready, None));
            }
            "session.status" => match envelope
                .properties
                .pointer("/status/type")
                .and_then(Value::as_str)
            {
                // Dispatching the prompt already published the shared running
                // session. Re-publishing it here through the settlement seam
                // would clear activeTurnId, so busy is an idempotent confirmation.
                Some("busy") => self.ignore_idle_until_busy = false,
                Some("retry") => {
                    let status = &envelope.properties["status"];
                    let message = status
                        .pointer("/message")
                        .and_then(Value::as_str)
                        .or_else(|| {
                            status
                                .pointer("/error/data/message")
                                .and_then(Value::as_str)
                        })
                        .unwrap_or("OpenCode is retrying the request.");
                    decided.changes.push(crate::threads::Change::Activity(
                        crate::threads::Activity::info(
                            "runtime.warning",
                            message,
                            status.clone(),
                            driving.turn.as_ref().map(|turn| turn.turn_id.clone()),
                        ),
                    ));
                }
                _ => {}
            },
            "session.error" => {
                if self.ignore_idle_until_busy { return Some(decided); }
                let error = envelope
                    .properties
                    .get("error")
                    .unwrap_or(&envelope.properties);
                let message = structured_event_error(error);
                return Some(self.settle(
                    driving,
                    crate::settling::SessionStatus::Error,
                    Some(message),
                ));
            }
            _ if unknown => eprintln!("OpenCode event not understood: {}", envelope.kind),
            _ => {}
        }
        Some(decided)
    }

    async fn send(&mut self, prompt: &crate::threads::Prompt) -> std::io::Result<()> {
        self.settled = false;
        let parts = prompt
            .messages()
            .flat_map(|(text, attachments)| prompt_parts(text, attachments))
            .collect::<Vec<_>>();
        if parts.is_empty() { return Err(std::io::Error::other("OpenCode prompt has no resolvable text or attachments")); }
        let mut body = serde_json::json!({"parts": parts});
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

    /// Bounded at [`ABORT_TIMEOUT`] rather than [`REQUEST_TIMEOUT`], because the
    /// session loop awaits this *before* its `select!` and reads nothing while
    /// it runs. Every second spent here is a second the developer's Stop has
    /// visibly done nothing, and the interrupt reconciliation two seconds later
    /// is what establishes the real outcome anyway — so failing fast here loses
    /// no information and returns the loop to its signals.
    async fn interrupt(&mut self, _request_id: &str) -> std::io::Result<()> {
        match tokio::time::timeout(ABORT_TIMEOUT, self.client.abort(&self.session_id)).await {
            Ok(outcome) => outcome.map(|_| ()).map_err(std::io::Error::other),
            Err(_) => Err(std::io::Error::other(
                "OpenCode did not answer the request to stop the turn.",
            )),
        }
    }
    async fn reconcile_interrupt(&mut self, driving: &mut crate::session::Driving) -> Result<crate::session::Decided, String> {
        self.client.session(&self.session_id).await.map_err(|error| error.to_string())?;
        let messages = self.client.messages(&self.session_id).await.map_err(|error| error.to_string())?;
        let statuses = self.client.session_statuses().await.map_err(|error| error.to_string())?;
        if statuses.get(&self.session_id).and_then(|status| status.get("type")).and_then(Value::as_str).is_some_and(|status| status != "idle") {
            return Err("OpenCode still reports the interrupted session as busy".to_string());
        }
        if let Some(text) = messages.as_array()
            .and_then(|messages| messages.iter().rev().find(|message| message.pointer("/info/role").and_then(Value::as_str) == Some("assistant")))
            .and_then(|message| message.get("parts")).and_then(Value::as_array)
            .map(|parts| parts.iter().filter(|part| part.get("type").and_then(Value::as_str) == Some("text")).filter_map(|part| part.get("text").and_then(Value::as_str)).collect::<String>())
            .filter(|text| text.starts_with(&self.assistant_text))
        {
            self.assistant_text = text;
        }
        self.ignore_idle_until_busy = true;
        Ok(self.settle(driving, crate::settling::SessionStatus::Interrupted, None))
    }
    async fn answer(
        &mut self,
        asked: &crate::approval::ApprovalRequest,
        reply: crate::session::Reply<'_>,
    ) -> std::io::Result<()> {
        if let Some(pending) = self.pending_questions.get(&asked.request_id) {
            return match reply {
                crate::session::Reply::Answers(answers) => self.client.reply_question(
                    &asked.request_id,
                    &serde_json::json!({"answers": ordered_question_answers(pending, answers)}),
                ).await.map(|_| ()).map_err(std::io::Error::other),
                crate::session::Reply::Rejected => self.client.reject_question(&asked.request_id)
                    .await.map(|_| ()).map_err(std::io::Error::other),
                crate::session::Reply::Decided(_) => Err(std::io::Error::other(
                    "OpenCode questions require answers or rejection",
                )),
            };
        }
        let crate::session::Reply::Decided(decision) = reply else {
            return Err(std::io::Error::other(
                "OpenCode permission replies require a decision",
            ));
        };
        let Some(pending) = self.pending_permissions.get(&asked.request_id) else {
            return Err(std::io::Error::other(
                "unknown pending OpenCode permission request",
            ));
        };
        if pending.provider_request_id != asked.provider_request_id {
            return Err(std::io::Error::other(
                "OpenCode permission identity does not match",
            ));
        }
        let reply = match decision {
            crate::worklog::Decision::Accept => "once",
            crate::worklog::Decision::AcceptForSession => "always",
            crate::worklog::Decision::Decline | crate::worklog::Decision::Cancel => "reject",
        };
        let legacy = asked.provider_request_id.as_ref()
            .and_then(|value| value.get("kind"))
            .and_then(Value::as_str) == Some("permission.updated");
        let result = if legacy {
            self.client.reply_legacy_permission(&self.session_id, &asked.request_id, &serde_json::json!({"response":reply})).await
        } else {
            self.client.reply_permission(&asked.request_id, &serde_json::json!({"reply":reply})).await
        };
        result.map(|_| ()).map_err(std::io::Error::other)
    }
    async fn measure(&mut self, _request_id: &str) -> std::io::Result<()> {
        Ok(())
    }
    async fn retune(
        &mut self,
        _request_id: &str,
        asked: &crate::session::Pushed,
    ) -> std::io::Result<()> {
        match asked {
            crate::session::Pushed::Model { asked, .. } => self.model = Some(asked.clone()),
            crate::session::Pushed::Mode { asked, .. } => {
                self.client
                    .update_session(
                        &self.session_id,
                        &serde_json::json!({"permission":permission_rules(asked)}),
                    )
                    .await
                    .map_err(std::io::Error::other)?;
            }
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
        // Bounded twice over — [`REQUEST_TIMEOUT`] on the call and
        // [`ABORT_TIMEOUT`] around it — because everything below reaps the
        // child, and an OpenCode that has stopped answering its own port is
        // exactly the OpenCode that most needs reaping. Sequencing the kill
        // behind an unbounded request to the process being killed is how this
        // machine accumulated three days of orphaned servers.
        let abort_failure = if asked_to_stop && driving.turn.is_some() {
            match tokio::time::timeout(ABORT_TIMEOUT, self.client.abort(&self.session_id)).await {
                Ok(outcome) => outcome.err().map(|error| {
                    format!("OpenCode could not abort its active work while stopping: {error}")
                }),
                Err(_) => Some(
                    "OpenCode did not answer the request to abort its active work while stopping."
                        .to_string(),
                ),
            }
        } else {
            None
        };
        self.events.cancel().await;
        if let Some(owned) = self.owned.as_mut() {
            owned.stop().await;
        }
        drop(self.mcp_session.take());
        crate::session::Reaped {
            refused: None,
            death: abort_failure.or_else(|| {
                (driving.turn.is_some() && !asked_to_stop)
                    .then(|| "OpenCode stopped before the turn finished.".to_string())
            }),
        }
    }
}

async fn classify_response(
    response: reqwest::Response,
    password: Option<&str>,
) -> Result<reqwest::Response, OpenCodeError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let bytes = response.bytes().await.map_err(OpenCodeError::Transport)?;
    let body = redact(String::from_utf8_lossy(&bytes).into_owned(), password);
    let error = serde_json::from_slice::<StructuredError>(&bytes)
        .ok()
        .map(|error| redact_structured(error, password));
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

fn redact(value: String, password: Option<&str>) -> String {
    password
        .filter(|password| !password.is_empty())
        .map_or(value.clone(), |password| {
            value.replace(password, "[redacted]")
        })
}

fn redact_structured(mut error: StructuredError, password: Option<&str>) -> StructuredError {
    error.name = error.name.map(|value| redact(value, password));
    error.message = error.message.map(|value| redact(value, password));
    if let Some(data) = error.data.as_mut() {
        redact_value(data, password);
    }
    error
}

fn redact_value(value: &mut Value, password: Option<&str>) {
    match value {
        Value::String(text) => *text = redact(std::mem::take(text), password),
        Value::Array(values) => values
            .iter_mut()
            .for_each(|value| redact_value(value, password)),
        Value::Object(values) => values
            .values_mut()
            .for_each(|value| redact_value(value, password)),
        _ => {}
    }
}

fn redact_sse(error: SseDecodeError, password: Option<&str>) -> SseDecodeError {
    match error {
        SseDecodeError::MalformedField(value) => {
            SseDecodeError::MalformedField(redact(value, password))
        }
        SseDecodeError::MalformedJson(value) => {
            SseDecodeError::MalformedJson(redact(value, password))
        }
        other => other,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn windows() -> HashMap<String, u64> {
        [("opencode/deepseek-v4-flash-free".to_string(), 128_000)]
            .into_iter()
            .collect()
    }

    #[test]
    fn token_usage_reads_model_from_top_level_assistant_info() {
        let info = serde_json::json!({
            "id": "msg_1",
            "role": "assistant",
            "modelID": "deepseek-v4-flash-free",
            "providerID": "opencode",
            "tokens": {"input": 10, "output": 20, "total": 30, "cache": {"read": 0, "write": 0}}
        });
        let usage = message_token_usage(&info, &windows(), None).expect("a usage");
        assert_eq!(usage.max_tokens, Some(128_000));
        assert_eq!(usage.used_tokens, 30);
    }

    #[test]
    fn token_usage_reads_model_from_nested_user_info() {
        let info = serde_json::json!({
            "id": "msg_2",
            "role": "user",
            "model": {"providerID": "opencode", "modelID": "deepseek-v4-flash-free"},
            "tokens": {"input": 10, "output": 0, "total": 10, "cache": {"read": 0, "write": 0}}
        });
        let usage = message_token_usage(&info, &windows(), None).expect("a usage");
        assert_eq!(usage.max_tokens, Some(128_000));
    }

    #[test]
    fn context_window_flows_from_real_catalogue_to_real_message_shape() {
        // The `/provider` inventory as opencode reports it: `connected` names
        // the provider, `all` carries it, and each model exposes its window at
        // `/limit/context` (there is no `contextWindow` field). The assistant
        // message `info` carries `providerID`/`modelID` at the top level, not
        // nested under `model`. Both quirks are from a live owned server.
        let inventory = serde_json::json!({
            "connected": ["opencode"],
            "all": [{
                "id": "opencode",
                "name": "OpenCode Zen",
                "models": {
                    "deepseek-v4-flash-free": {
                        "id": "deepseek-v4-flash-free",
                        "providerID": "opencode",
                        "name": "DeepSeek V4 Flash Free",
                        "limit": {"context": 200_000, "output": 128_000}
                    }
                }
            }]
        });
        let windows = context_windows(&inventory);
        assert_eq!(windows.get("opencode/deepseek-v4-flash-free"), Some(&200_000));
        let info = serde_json::json!({
            "id": "msg_4",
            "role": "assistant",
            "modelID": "deepseek-v4-flash-free",
            "providerID": "opencode",
            "tokens": {"input": 1000, "output": 500, "total": 1500, "cache": {"read": 0, "write": 0}}
        });
        let usage = message_token_usage(&info, &windows, None).expect("a usage");
        assert_eq!(usage.max_tokens, Some(200_000));
        assert_eq!(usage.used_tokens, 1500);
    }

    /// A custom provider declared in `opencode.json` with no `limit` block.
    /// OpenCode fills the gap with `context: 0` rather than omitting the field,
    /// so a catalogue that knows nothing about the model still answers with one
    /// — this is every OpenAI-compatible proxy, captured verbatim from a live
    /// owned server (`iroha/MiniMax-M3`, `source: "config"`).
    ///
    /// Nought is not a window. Carried as `Some(0)` it is a maximum the meter
    /// would have to divide by; carried as absence it takes the same path as a
    /// catalogue that never loaded, which is the used-tokens-only fallback the
    /// meter already draws.
    #[test]
    fn a_model_declared_without_a_limit_has_no_window_rather_than_a_window_of_nought() {
        let inventory = serde_json::json!({
            "connected": ["iroha"],
            "all": [{
                "id": "iroha",
                "name": "Iroha",
                "source": "config",
                "models": {
                    "MiniMax-M3": {
                        "id": "MiniMax-M3",
                        "providerID": "iroha",
                        "name": "MiniMax-M3",
                        "limit": {"context": 0, "output": 0}
                    }
                }
            }]
        });
        let windows = context_windows(&inventory);
        assert_eq!(windows.get("iroha/MiniMax-M3"), None);

        let info = serde_json::json!({
            "id": "msg_5",
            "role": "assistant",
            "modelID": "MiniMax-M3",
            "providerID": "iroha",
            "tokens": {"input": 1000, "output": 500, "total": 1500, "cache": {"read": 0, "write": 0}}
        });
        let usage = message_token_usage(&info, &windows, None).expect("a usage");
        assert_eq!(usage.max_tokens, None);
        assert_eq!(usage.used_tokens, 1500, "the token count survives the missing window");
    }

    #[test]
    fn token_usage_yields_no_max_when_slug_is_unknown() {
        let info = serde_json::json!({
            "id": "msg_3",
            "role": "assistant",
            "modelID": "some-other-model",
            "providerID": "opencode",
            "tokens": {"input": 10, "output": 20, "total": 30, "cache": {"read": 0, "write": 0}}
        });
        let usage = message_token_usage(&info, &windows(), None).expect("a usage");
        assert_eq!(usage.max_tokens, None);
    }
}
