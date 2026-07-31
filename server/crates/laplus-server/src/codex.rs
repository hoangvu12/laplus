//! The `codex app-server` transport for provider probes and conversations.
//!
//! Responses on this wire omit `jsonrpc` and may arrive in any order, while
//! requests sent by app-server use an independent id space. Classification is
//! therefore by shape first and client responses are correlated only through
//! the ids this client has in `pending`.
//!
//! Codex labels agent messages `commentary` before tool use and `final_answer`
//! afterwards. Both are published as transcript messages: they are agent-authored
//! prose addressed to the developer, while the work log is reserved for what the
//! agent did. Keeping the item ids separate preserves Codex's phase boundary
//! without inventing an activity kind that the other driver does not produce.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::process::{Child as AsyncChild, ChildStdin as AsyncChildStdin, Command as AsyncCommand};
use tokio::sync::mpsc as async_mpsc;

use crate::approval::ApprovalRequest;
use crate::codex_protocol::{
    self as protocol, Access, Capabilities, CommandExecution, ConversationFold,
    ConversationState, Incoming, Request,
};
use crate::config::{CodexSettings, ProviderAuth, ProviderModel};
use crate::config_store::ProviderProcessLifetime;
use crate::session::{
    Decided, Driver, Driving, Finished, Pushed, Reaped, Reply, Settles, Start,
};
use crate::settling::SessionStatus;
use crate::threads::{Activity, Change};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);
const OUTPUT_QUEUE: usize = 256;
const EXIT_GRACE: Duration = Duration::from_secs(2);
const EXIT_POLL: Duration = Duration::from_millis(20);
pub struct Snapshot {
    pub version: Option<String>,
    pub auth: ProviderAuth,
    pub models: Vec<ProviderModel>,
    pub skills: Vec<Value>,
}

pub(crate) fn probe(
    binary: &Path,
    settings: &CodexSettings,
    roots: &[PathBuf],
    lifetime: &ProviderProcessLifetime,
) -> Result<Snapshot, String> {
    let _active_process = lifetime.begin()?;
    let cwd = roots
        .first()
        .cloned()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut client = Client::start(binary, settings, &cwd, lifetime.clone())?;

    let version = protocol::decode_initialize(client.request(Request::Initialize)?)?;
    client.write(&protocol::initialized())?;

    let account_id = client.send_request(Request::Account)?;
    let models_id = client.send_request(Request::Models { cursor: None })?;
    let cwds: Vec<String> = match roots.is_empty() {
        true => vec![cwd.display().to_string()],
        false => roots
            .iter()
            .map(|root| root.display().to_string())
            .collect(),
    };
    let skills_id = client.send_request(Request::Skills { cwds })?;

    let auth = protocol::decode_account(client.wait(account_id)?)?;
    let skills = protocol::decode_skills(client.wait(skills_id)?)?;
    let mut page = protocol::decode_models(client.wait(models_id)?)?;
    let mut models = Vec::new();
    loop {
        models.extend(page.models);
        let cursor = page.next_cursor;
        let Some(cursor) = cursor else { break };
        page = protocol::decode_models(
            client.request(Request::Models {
                cursor: Some(cursor),
            })?,
        )?;
    }
    protocol::append_custom_models(&mut models, &settings.custom_models);

    Ok(Snapshot {
        version: Some(version),
        auth,
        models,
        skills,
    })
}

/// One long-lived `codex app-server` behind one conversation.
pub(crate) struct Codex {
    app_server: AppServer,
    folding: ConversationState,
    thread_id: String,
    active_turn_id: Option<String>,
    capabilities: Capabilities,
}

impl Driver for Codex {
    async fn open(start: &Start) -> Result<Codex, String> {
        let settings = start.driver.codex()?;
        let capabilities = Capabilities::current();
        let (binary, _) = crate::provider::resolve_codex(
            &settings.binary_path,
            &crate::process::Search::from_environment(),
        )
        .startable_codex()?;
        let mut app_server = AppServer::start(
            &binary,
            settings,
            Path::new(&start.workspace_root),
        )
        .await
        .map_err(|error| {
            format!(
                "The Codex binary {} could not be started in {}: {error}",
                binary.display(),
                start.workspace_root
            )
        })?;

        let opened = async {
            let initialized = app_server.request(Request::Initialize).await?;
            protocol::decode_initialize(initialized)?;
            app_server.write(&protocol::initialized()).await?;
            let access = Access::for_runtime_mode(&start.runtime_mode)?;
            let thread = app_server
                .request(Request::ThreadStart {
                    cwd: start.workspace_root.clone(),
                    model: start.model.clone(),
                    access,
                })
                .await?;
            protocol::decode_thread_start(thread)
        }
        .await;
        let thread_id = match opened {
            Ok(thread_id) => thread_id,
            Err(error) => {
                app_server.stop().await;
                return Err(error);
            }
        };

        Ok(Codex {
            app_server,
            folding: ConversationState::new(),
            thread_id,
            active_turn_id: None,
            capabilities,
        })
    }

    async fn next(&mut self, driving: &mut Driving) -> Option<Decided> {
        let line = self.app_server.next_line().await?;
        let observed = self.app_server.observe(&line);
        let folded = match observed {
            Observed::Notification => self.folding.fold_line(&line),
            Observed::Response {
                method,
                result: Ok(result),
                ..
            } if method == "turn/start" => {
                self.folding
                    .fold_message(serde_json::json!({"result": result}))
            }
            Observed::Response {
                method,
                result: Err(error),
                ..
            } if method == "turn/start" => {
                self.active_turn_id = None;
                return Some(settle(driving, Ending::Failed(error), None));
            }
            Observed::Response {
                method,
                result: Ok(_),
                turn_id: Some(interrupted_turn),
            } if method == "turn/interrupt"
                && self.active_turn_id.as_deref() == Some(interrupted_turn.as_str()) =>
            {
                self.active_turn_id = None;
                return Some(settle_interrupted(driving));
            }
            Observed::Response {
                method,
                result: Err(error),
                turn_id: Some(interrupted_turn),
            } if method == "turn/interrupt"
                && self.active_turn_id.as_deref() == Some(interrupted_turn.as_str()) =>
            {
                let Some(active) = driving.turn.as_mut() else {
                    return Some(Decided::default());
                };
                if !active.was_stopped() {
                    return Some(Decided::default());
                }
                active.carries_on();
                return Some(Decided {
                    changes: vec![Change::Activity(Activity::failed(
                        "turn.interrupt-failed",
                        &format!("Codex would not stop the turn: {error}"),
                    ))],
                    ..Decided::default()
                });
            }
            Observed::Malformed => self.folding.fold_line(&line),
            Observed::Request => self.folding.fold_line(&line),
            Observed::Response { .. } => ConversationFold::Nothing,
        };

        let idle = matches!(
            &folded,
            ConversationFold::ThreadStatus { status } if status == "idle"
        );
                let stale_completion = matches!(
                    &folded,
                    ConversationFold::TurnCompleted(completion)
                        if self.active_turn_id.as_deref() != Some(completion.turn_id.as_str())
                );
                if stale_completion {
                    return Some(Decided::default());
                }
                match &folded {
                    ConversationFold::TurnStarted { turn_id } => {
                        self.active_turn_id = Some(turn_id.clone());
                    }
                    ConversationFold::TurnCompleted(_) => {
                        self.active_turn_id = None;
                    }
                    _ => {}
                }
                let decided = decide(folded, driving);
                // Empty capabilities promise the completion that carries the
                // outcome, so idle cannot win that race. Experimental API omits
                // completion; the handshake policy flips this fallback with it.
                if idle
                    && self.capabilities.idle_is_terminal()
                    && driving.turn.is_some()
                    && decided.settles.is_none()
                {
                    self.active_turn_id = None;
                    return Some(settle(driving, Ending::Completed, None));
                }
                Some(decided)
    }

    async fn send(&mut self, text: &str) -> std::io::Result<()> {
        self.app_server
            .send_request(Request::TurnStart {
                thread_id: self.thread_id.clone(),
                text: text.to_string(),
            })
            .await
            .map(|_| ())
            .map_err(std::io::Error::other)
    }

    async fn interrupt(&mut self, _request_id: &str) -> std::io::Result<()> {
        let turn_id = self
            .active_turn_id
            .clone()
            .ok_or_else(|| std::io::Error::other("Codex has not started the turn yet"))?;
        self.app_server
            .send_request(Request::TurnInterrupt {
                thread_id: self.thread_id.clone(),
                turn_id,
            })
            .await
            .map(|_| ())
            .map_err(std::io::Error::other)
    }

    async fn answer(&mut self, asked: &ApprovalRequest, reply: Reply<'_>) -> std::io::Result<()> {
        let Reply::Decided(decision) = reply else {
            return Err(std::io::Error::other("Codex did not ask an approval question"));
        };
        let response_id = asked
            .provider_request_id
            .as_ref()
            .ok_or_else(|| std::io::Error::other("Codex approval lost its JSON-RPC request id"))?;
        if !asked
            .available_decisions
            .as_ref()
            .is_some_and(|offered| offered.contains(&decision))
        {
            return Err(std::io::Error::other(format!(
                "Codex did not offer the '{}' approval decision",
                decision.as_str()
            )));
        }
        self.app_server
            .write(&protocol::approval_response(response_id, decision.as_str()))
            .await
            .map_err(std::io::Error::other)
    }

    async fn measure(&mut self, _request_id: &str) -> std::io::Result<()> {
        Ok(())
    }

    async fn retune(&mut self, _request_id: &str, _asked: &Pushed) -> std::io::Result<()> {
        Err(std::io::Error::other(
            "Codex model and access changes have not been implemented",
        ))
    }

    fn close_input(&mut self) {
        self.app_server.close_input();
    }

    async fn stop(self, driving: &mut Driving, asked_to_stop: bool) -> Reaped {
        let complaint = self.app_server.stop().await;
        let death = (driving.turn.is_some() && !asked_to_stop).then(|| match complaint {
            Some(complaint) => format!(
                "Codex stopped before the turn finished. The agent said: {complaint}"
            ),
            None => "Codex stopped before the turn finished.".to_string(),
        });
        Reaped {
            refused: None,
            death,
        }
    }
}

fn decide(folded: ConversationFold, driving: &mut Driving) -> Decided {
    let mut decided = Decided::default();
    match folded {
        ConversationFold::Nothing
        | ConversationFold::ThreadStarted { .. }
        | ConversationFold::TurnStarted { .. } => {}
        ConversationFold::ThreadStatus { .. } => {}
        ConversationFold::ReasoningStarted { .. } => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided
                .changes
                .push(Change::Activity(crate::worklog::thinking_started(turn_id)));
        }
        ConversationFold::ReasoningDelta { .. } => {}
        ConversationFold::ReasoningCompleted { text, .. } => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            if let Some(thinking) = crate::worklog::thinking(&text, turn_id) {
                decided.changes.push(Change::Activity(thinking));
            }
        }
        // Commentary and final answers deliberately share the transcript path;
        // the module-level policy explains why the phase is not an activity kind.
        ConversationFold::AssistantDelta { text, .. } => {
            let Some(active) = driving.turn.as_mut() else {
                return decided;
            };
            let message_id = active
                .assistant_message_id
                .get_or_insert_with(crate::threads::fresh_message_id)
                .clone();
            decided.changes.push(Change::AssistantDelta {
                message_id,
                turn_id: active.turn_id.clone(),
                text,
            });
        }
        ConversationFold::AssistantCompleted { text, .. } => {
            let Some(active) = driving.turn.as_mut() else {
                return decided;
            };
            let message_id = active
                .assistant_message_id
                .take()
                .unwrap_or_else(crate::threads::fresh_message_id);
            decided.changes.push(Change::AssistantMessage {
                message_id,
                turn_id: active.turn_id.clone(),
                text,
            });
        }
        ConversationFold::CommandStarted(command) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided
                .changes
                .push(Change::Activity(command_call(&command).invoked(turn_id)));
        }
        ConversationFold::CommandCompleted(command) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            decided.changes.push(Change::Activity(
                command_call(&command).command_returned(
                    crate::worklog::CommandReturned {
                        output: command.aggregated_output.as_deref().unwrap_or_default(),
                        status: &command.status,
                        exit_code: command.exit_code,
                        duration_ms: command.duration_ms,
                    },
                    turn_id,
                ),
            ));
        }
        ConversationFold::ApprovalRequested(request) => {
            let turn_id = driving.turn.as_ref().map(|turn| turn.turn_id.clone());
            // Command executions have their own item lifecycle. The other two
            // approval methods do not yet have a rendered lifecycle, so publish
            // their call here before publishing the panel that asks about it.
            if request.request_kind != "command" {
                decided
                    .changes
                    .push(Change::Activity(request.call().invoked(turn_id.clone())));
            }
            let asked = request.permission();
            decided
                .changes
                .push(Change::Activity(crate::worklog::requested(&asked, turn_id)));
            driving.outstanding.insert(asked.request_id.clone(), asked);
        }
        ConversationFold::TurnCompleted(completion) => {
            let ending = match (
                driving.turn.as_ref().is_some_and(|turn| turn.was_stopped()),
                completion.error,
            ) {
                (true, _) => Ending::Stopped,
                (false, Some(error)) => Ending::Failed(error),
                (false, None) => Ending::Completed,
            };
            return settle(driving, ending, completion.duration_ms);
        }
    }
    decided
}

fn command_call(command: &CommandExecution) -> crate::worklog::Call {
    crate::worklog::Call {
        id: command.id.clone(),
        name: "Command".to_string(),
        input: serde_json::json!({
            "command": command.command,
            "cwd": command.cwd,
            "processId": command.process_id,
        }),
    }
}

enum Ending {
    Completed,
    Failed(String),
    Stopped,
}

impl Ending {
    fn failed(&self) -> bool {
        matches!(self, Ending::Failed(_))
    }

    fn stopped(&self) -> bool {
        matches!(self, Ending::Stopped)
    }

    fn summary(&self, duration_ms: Option<u64>) -> String {
        match self {
            Ending::Failed(error) => format!("Turn failed. Codex said: {error}"),
            Ending::Stopped => "Turn stopped by the developer.".to_string(),
            Ending::Completed => match duration_ms {
                Some(duration) => {
                    format!("Turn completed in {:.1}s.", duration as f64 / 1_000.0)
                }
                None => "Turn completed.".to_string(),
            },
        }
    }

    fn session_status(&self) -> SessionStatus {
        match self {
            Ending::Completed => SessionStatus::Ready,
            Ending::Failed(_) => SessionStatus::Error,
            Ending::Stopped => SessionStatus::Interrupted,
        }
    }

    fn checkpoint_status(&self) -> Option<&'static str> {
        match self {
            Ending::Completed => Some("ready"),
            Ending::Failed(_) => Some("error"),
            Ending::Stopped => None,
        }
    }
}

fn settle(driving: &mut Driving, ending: Ending, duration_ms: Option<u64>) -> Decided {
    let Some(finished) = driving.turn.take() else {
        return Decided::default();
    };
    let turn_id = finished.turn_id;
    let failed = ending.failed();
    if let Some(status) = ending.checkpoint_status() {
        driving.finished = Some(Finished {
            turn_id: turn_id.clone(),
            status,
        });
    }

    let summary = ending.summary(duration_ms);
    let mut activity = Activity::info(
        "turn.completed",
        &summary,
        serde_json::json!({
            "durationMs": duration_ms,
            "totalCostUsd": Value::Null,
            "isError": failed,
            "interrupted": ending.stopped(),
        }),
        Some(turn_id.clone()),
    );
    if failed {
        activity.tone = "error";
    }

    Decided {
        changes: vec![Change::Activity(activity)],
        settles: Some(Settles {
            turn_id: Some(turn_id),
            status: ending.session_status(),
            last_error: failed.then_some(summary),
        }),
        ..Decided::default()
    }
}

fn settle_interrupted(driving: &mut Driving) -> Decided {
    let Some(active) = driving.turn.as_mut() else {
        return Decided::default();
    };
    if !active.was_stopped() {
        return Decided::default();
    }
    let closing = active
        .assistant_message_id
        .take()
        .map(|message_id| Change::AssistantMessage {
            message_id,
            turn_id: active.turn_id.clone(),
            // Codex sends no authoritative message after an interrupt. Empty
            // text closes the stream while preserving its accumulated deltas.
            text: String::new(),
        });
    let mut decided = settle(driving, Ending::Stopped, None);
    if let Some(closing) = closing {
        decided.changes.insert(0, closing);
    }
    decided
}

struct AppServer {
    child: AsyncChild,
    stdin: Option<AsyncChildStdin>,
    output: async_mpsc::Receiver<String>,
    pending: HashMap<u64, Pending>,
    next_id: u64,
    complaint: Arc<Mutex<Option<String>>>,
    stderr: Option<tokio::task::JoinHandle<()>>,
}

impl AppServer {
    async fn start(
        binary: &Path,
        settings: &CodexSettings,
        cwd: &Path,
    ) -> std::io::Result<AppServer> {
        let launch_args = shell_words::split(&settings.launch_args).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
        })?;
        let mut command = AsyncCommand::new(binary);
        command
            .arg("app-server")
            .args(launch_args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if !settings.home_path.trim().is_empty() {
            command.env(
                "CODEX_HOME",
                crate::projects::expand_home(settings.home_path.trim()),
            );
        }
        crate::process::without_a_console(command.as_std_mut());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or_else(missing_async_pipe)?;
        let stdout = child.stdout.take().ok_or_else(missing_async_pipe)?;
        let child_stderr = child.stderr.take().ok_or_else(missing_async_pipe)?;

        let (lines, output) = async_mpsc::channel(OUTPUT_QUEUE);
        tokio::spawn(async move {
            let mut reader = AsyncBufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if lines.send(line).await.is_err() {
                    return;
                }
            }
        });

        let complaint = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&complaint);
        let stderr = tokio::spawn(async move {
            let mut reader = AsyncBufReader::new(child_stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                eprintln!("laplus: codex: {line}");
                if !line.trim().is_empty() {
                    *latest
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(line.trim().to_string());
                }
            }
        });

        Ok(AppServer {
            child,
            stdin: Some(stdin),
            output,
            pending: HashMap::new(),
            next_id: 1,
            complaint,
            stderr: Some(stderr),
        })
    }

    async fn request(&mut self, request: Request) -> Result<Value, String> {
        let id = self.send_request(request).await?;
        loop {
            let line = tokio::time::timeout(RESPONSE_TIMEOUT, self.output.recv())
                .await
                .map_err(|_| "Codex stopped answering a request".to_string())?
                .ok_or_else(|| "Codex stopped before answering a request".to_string())?;
            match protocol::decode_incoming(&line)? {
                Incoming::Request { id, method, .. } => {
                    self.write(&protocol::unsupported_request(&id, &method)).await?;
                }
                Incoming::Notification => {}
                Incoming::Response { id: response_id, result } => {
                    if self.pending.remove(&response_id).is_none() {
                        continue;
                    }
                    if response_id == id {
                        return result;
                    }
                }
            }
        }
    }

    async fn send_request(&mut self, request: Request) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        let pending = Pending {
            method: request.method().to_string(),
            turn_id: match &request {
                Request::TurnInterrupt { turn_id, .. } => Some(turn_id.clone()),
                _ => None,
            },
        };
        self.write(&request.message(id)).await?;
        self.pending.insert(id, pending);
        Ok(id)
    }

    async fn write(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "Codex stdin is closed".to_string())?;
        let mut line = message.to_string();
        line.push('\n');
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("Codex request could not be written: {error}"))?;
        stdin
            .flush()
            .await
            .map_err(|error| format!("Codex request could not be written: {error}"))
    }

    async fn next_line(&mut self) -> Option<String> {
        self.output.recv().await
    }

    fn observe(&mut self, line: &str) -> Observed {
        match protocol::decode_incoming(line) {
            Ok(Incoming::Notification) => Observed::Notification,
            Ok(Incoming::Request { .. }) => Observed::Request,
            Ok(Incoming::Response { id, result }) => match self.pending.remove(&id) {
                Some(pending) => Observed::Response {
                    method: pending.method,
                    turn_id: pending.turn_id,
                    result,
                },
                None => Observed::Response {
                    method: String::new(),
                    turn_id: None,
                    result,
                },
            },
            Err(_) => Observed::Malformed,
        }
    }

    fn close_input(&mut self) {
        drop(self.stdin.take());
    }

    async fn stop(mut self) -> Option<String> {
        drop(self.stdin.take());
        let deadline = tokio::time::Instant::now() + EXIT_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return self.last_words().await,
                Err(_) => break,
                Ok(None) if tokio::time::Instant::now() >= deadline => break,
                Ok(None) => tokio::time::sleep(EXIT_POLL).await,
            }
        }

        #[cfg(windows)]
        if let Some(pid) = self.child.id() {
            let mut command = AsyncCommand::new("taskkill.exe");
            command
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            crate::process::without_a_console(command.as_std_mut());
            let _ = command.status().await;
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        self.last_words().await
    }

    async fn last_words(&mut self) -> Option<String> {
        if let Some(stderr) = self.stderr.take() {
            let _ = tokio::time::timeout(EXIT_GRACE, stderr).await;
        }
        self.complaint
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

enum Observed {
    Notification,
    Request,
    Response {
        method: String,
        turn_id: Option<String>,
        result: Result<Value, String>,
    },
    Malformed,
}

struct Pending {
    method: String,
    turn_id: Option<String>,
}

fn missing_async_pipe() -> std::io::Error {
    std::io::Error::other("Codex was started without one of its stdio pipes")
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    output: mpsc::Receiver<String>,
    pending: HashMap<u64, String>,
    responses: HashMap<u64, Result<Value, String>>,
    next_id: u64,
    stderr: Arc<Mutex<Option<String>>>,
    lifetime: ProviderProcessLifetime,
}

impl Client {
    fn start(
        binary: &Path,
        settings: &CodexSettings,
        cwd: &Path,
        lifetime: ProviderProcessLifetime,
    ) -> Result<Client, String> {
        let launch_args = shell_words::split(&settings.launch_args)
            .map_err(|error| format!("Codex launch arguments could not be read: {error}"))?;
        let mut command = Command::new(binary);
        command
            .arg("app-server")
            .args(launch_args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if !settings.home_path.trim().is_empty() {
            command.env(
                "CODEX_HOME",
                crate::projects::expand_home(settings.home_path.trim()),
            );
        }
        crate::process::without_a_console(&mut command);
        let mut child = command
            .spawn()
            .map_err(|error| format!("{} could not be started: {error}", binary.display()))?;
        let pipes = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(stdin), Some(stdout), Some(child_stderr)) = pipes else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Codex was started without one of its stdio pipes".to_string());
        };

        let (lines, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(None));
        let latest = Arc::clone(&stderr);
        std::thread::spawn(move || {
            for line in BufReader::new(child_stderr).lines().map_while(Result::ok) {
                if !line.trim().is_empty() {
                    // Severity words are Codex's logging vocabulary, not process
                    // state. Only a failed request makes stderr diagnostic.
                    eprintln!("laplus: codex: {line}");
                    *latest
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
                        Some(line.trim().to_string());
                }
            }
        });

        Ok(Client {
            child,
            stdin: Some(stdin),
            output,
            pending: HashMap::new(),
            responses: HashMap::new(),
            next_id: 1,
            stderr,
            lifetime,
        })
    }

    fn request(&mut self, request: Request) -> Result<Value, String> {
        let id = self.send_request(request)?;
        self.wait(id)
    }

    fn send_request(&mut self, request: Request) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&request.message(id))?;
        self.pending.insert(id, request.method().to_string());
        Ok(id)
    }

    fn write(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| "Codex stdin is closed".to_string())?;
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("Codex request could not be written: {error}"))
    }

    fn wait(&mut self, wanted: u64) -> Result<Value, String> {
        let deadline = Instant::now() + RESPONSE_TIMEOUT;
        loop {
            if self.lifetime.is_cancelled() {
                return Err("Codex provider probe was cancelled during server shutdown".to_string());
            }
            if let Some(response) = self.responses.remove(&wanted) {
                return response;
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .unwrap_or_default();
            let wait = remaining.min(CANCELLATION_POLL);
            let line = match self.output.recv_timeout(wait) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) if wait < remaining => continue,
                Err(error) => return Err(self.wait_error(wanted, error)),
            };
            match protocol::decode_incoming(&line)? {
                // A method plus an id is app-server asking us something. Its id
                // is independent from, and never looked up in, `pending`.
                Incoming::Request { id, method, .. } => {
                    self.write(&protocol::unsupported_request(&id, &method))?;
                }
                Incoming::Notification => {}
                Incoming::Response { id, result } => {
                    if self.pending.remove(&id).is_some() {
                        self.responses.insert(id, result);
                    }
                }
            }
        }
    }

    fn wait_error(&self, wanted: u64, error: mpsc::RecvTimeoutError) -> String {
        let request = self
            .pending
            .get(&wanted)
            .map(String::as_str)
            .unwrap_or("unknown request");
        let last = self
            .stderr
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match last {
            Some(last) => format!(
                "Codex stopped answering {request} ({error}); stderr ended with: {last}"
            ),
            None => format!("Codex stopped answering {request} ({error})"),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        drop(self.stdin.take());
        crate::process::terminate_tree_and_wait(&mut self.child);
    }
}
