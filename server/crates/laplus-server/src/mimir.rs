//! Owned Mimir runtime plugin bootstrap, authenticated loopback transport and driver.
//! The bridge is installed/enabled by the operator, never by a provider probe.

use crate::{
    mimir_protocol as protocol,
    session::{Decided, Driver, Driving, Opened, Reaped, Reply, Start},
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(65);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

fn random_hex() -> Result<String, String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| "Cannot obtain secure randomness for the Mimir bridge")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Ownership of only the private directory and file this launch created.
struct Bootstrap {
    dir: PathBuf,
    file: PathBuf,
}
impl Drop for Bootstrap {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        let _ = std::fs::remove_dir(&self.dir);
    }
}
impl Bootstrap {
    async fn create(workspace: &Path, token: &str) -> Result<Self, String> {
        #[cfg(windows)]
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or("Mimir bootstrap requires the current user's LOCALAPPDATA")?;
        #[cfg(not(windows))]
        let base = std::env::temp_dir();
        let dir = base.join(format!("laplus-mimir-{}", random_hex()?));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(&dir)
            .map_err(|_| "Cannot create private Mimir launch directory")?;
        let owned = Self {
            file: dir.join("launch.json"),
            dir,
        };
        #[cfg(windows)]
        owned.restrict_windows_acl().await?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&owned.file)
            .map_err(|_| "Cannot create private Mimir launch file")?;
        let bytes = serde_json::to_vec(&json!({"version":1,"token":token,"workspace":workspace}))
            .map_err(|_| "Cannot encode Mimir bootstrap")?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "Cannot write private Mimir launch file")?;
        Ok(owned)
    }

    /// Apply and verify an owner-only inheritable DACL *before* writing secrets.
    /// Fail closed when Windows security tooling or identity lookup is unavailable.
    #[cfg(windows)]
    async fn restrict_windows_acl(&self) -> Result<(), String> {
        let script = r#"$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $path=$env:LAPLUS_MIMIR_LAUNCH_DIR; $acl=Get-Acl -LiteralPath $path; $acl.SetOwner($sid); $acl.SetAccessRuleProtection($true,$false); foreach($existing in @($acl.Access)){[void]$acl.RemoveAccessRuleSpecific($existing)}; $rule=New-Object System.Security.AccessControl.FileSystemAccessRule($sid,'FullControl','ContainerInherit,ObjectInherit','None','Allow'); [void]$acl.AddAccessRule($rule); [System.IO.DirectoryInfo]::new($path).SetAccessControl($acl); $check=Get-Acl -LiteralPath $path; $rules=@($check.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])); if(!$check.AreAccessRulesProtected -or $check.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value){throw 'owner/DACL mismatch'}; if($rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid.Value -or $rules[0].AccessControlType -ne 'Allow' -or $rules[0].InheritanceFlags -ne 'ContainerInherit, ObjectInherit' -or $rules[0].PropagationFlags -ne 'None'){throw 'unexpected access rule'}"#;
        let mut command = Command::new("powershell.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .env("LAPLUS_MIMIR_LAUNCH_DIR", &self.dir)
            // Windows PowerShell must not load modules from an inherited pwsh 7 path.
            .env_remove("PSModulePath")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        crate::process::without_a_console(command.as_std_mut());
        let mut child = command
            .spawn()
            .map_err(|_| "Cannot enforce Windows owner-only ACL for Mimir bootstrap")?;
        let status = tokio::time::timeout(CONTROL_TIMEOUT, child.wait()).await;
        if !matches!(status,Ok(Ok(s)) if s.success()) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err("Cannot verify Windows owner-only ACL for Mimir bootstrap".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
struct Client {
    endpoint: reqwest::Url,
    token: String,
    // Ephemeral host-tool credentials; never settings, snapshots or Debug.
    redactions: Vec<String>,
    http: reqwest::Client,
}
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MimirClient")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}
impl Client {
    fn new(endpoint: &str, token: String) -> Result<Self, String> {
        let endpoint =
            reqwest::Url::parse(endpoint).map_err(|_| "Mimir readiness endpoint is invalid")?;
        if endpoint.scheme() != "http"
            || endpoint.host_str() != Some("127.0.0.1")
            || endpoint.port().is_none()
            || endpoint.path() != "/"
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            return Err("Mimir bridge must advertise an authenticated HTTP IPv4-loopback endpoint without credentials or a path".into());
        }
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONTROL_TIMEOUT)
            .build()
            .map_err(|_| "Cannot build Mimir HTTP client")?;
        Ok(Self {
            endpoint,
            token,
            redactions: Vec::new(),
            http,
        })
    }
    fn redact(&self, value: &str) -> String {
        self.redactions
            .iter()
            .fold(value.replace(&self.token, "[redacted]"), |value, secret| {
                value.replace(secret, "[redacted]")
            })
    }
    async fn api(&self, action: &str, mut fields: Value) -> Result<Value, String> {
        fields["version"] = json!(1);
        fields["action"] = json!(action);
        let body = serde_json::to_vec(&fields).map_err(|_| "Cannot encode Mimir request")?;
        if body.len() > protocol::MAX_FRAME {
            return Err("Mimir request exceeds the bridge's 4 MiB limit".into());
        }
        let response = self
            .http
            .post(self.endpoint.join("api").expect("fixed path"))
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json")
            .body(body)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|_| {
                format!(
                    "Mimir {action} transport failed or timed out; the operation was not retried"
                )
            })?;
        let status = response.status();
        let mut bytes = Vec::new();
        let mut chunks = response.bytes_stream();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(|_| "Mimir response was interrupted")?;
            if bytes.len() + chunk.len() > protocol::MAX_FRAME {
                return Err("Mimir response exceeds 4 MiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Mimir returned malformed JSON")?;
        protocol::version(&response)?;
        if let Some(error) = response.get("error") {
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Bridge operation failed");
            return Err(format!(
                "Mimir {action} failed ({}): {}",
                self.redact(code),
                self.redact(message)
            ));
        }
        if !status.is_success() {
            return Err(format!("Mimir {action} failed (HTTP {status})"));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| "Mimir response omitted result".into())
    }
    async fn snapshot(&self, id: &str) -> Result<Value, String> {
        self.api("snapshot", json!({"id":id})).await
    }
}

struct Bridge {
    child: Child,
    _bootstrap: Bootstrap,
    output: JoinHandle<()>,
    client: Client,
    supports_mcp: bool,
}
impl Bridge {
    async fn start(binary: &Path, command: &str, workspace: &Path) -> Result<Self, String> {
        let token = random_hex()?;
        let bootstrap = Bootstrap::create(workspace, &token).await?;
        let mut process = Command::new(binary);
        process
            .args(["run", "--output", "jsonl"])
            .arg(format!("{command} {}", bootstrap.file.display()))
            .current_dir(workspace)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        process.process_group(0);
        crate::process::without_a_console(process.as_std_mut());
        let mut child = process
            .spawn()
            .map_err(|e| format!("Mimir could not start: {e}"))?;
        crate::process::bound_to_this_server_async(&child);
        let stdout = child.stdout.take().expect("piped stdout");
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let output = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut ready = Some(ready_tx);
            loop {
                let mut line = Vec::new();
                // A malformed/noisy CLI cannot allocate an unbounded readiness line.
                let n = (&mut reader)
                    .take(64 * 1024 + 1)
                    .read_until(b'\n', &mut line)
                    .await;
                if !matches!(n,Ok(n) if n>0) || line.len() > 64 * 1024 {
                    break;
                }
                if let Some(sender) = ready.take() {
                    match readiness(&line) {
                        Ok(Some(value)) => {
                            let _ = sender.send(Ok(value));
                        }
                        Ok(None) => ready = Some(sender),
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            break;
                        }
                    }
                }
            }
            if let Some(sender) = ready {
                let _=sender.send(Err("Mimir exited without bridge readiness. Explicitly install and enable org.mimir.bridge with a compatible SDK version.".into()));
            }
        });
        let started=match tokio::time::timeout(STARTUP_TIMEOUT,ready_rx).await {
            Ok(Ok(Ok(value)))=>protocol::required(&value,"endpoint").and_then(|endpoint|Client::new(endpoint,token)).map(|client| {
                let actions=value.pointer("/capabilities/actions").and_then(Value::as_array).expect("validated readiness");
                (client, ["attach_mcp","detach_mcp"].iter().all(|action| actions.iter().any(|value|value.as_str()==Some(action))))
            }),
            Ok(Ok(Err(error)))=>Err(error),
            _=>Err("Mimir bridge did not become ready; check the installed/enabled plugin and SDK compatibility.".into()),
        };
        match started {
            Ok((client, supports_mcp)) => Ok(Self {
                child,
                _bootstrap: bootstrap,
                output,
                client,
                supports_mcp,
            }),
            Err(error) => {
                terminate(&mut child).await;
                output.abort();
                let _ = output.await;
                Err(error)
            }
        }
    }
    async fn stop(&mut self) {
        terminate(&mut self.child).await;
        self.output.abort();
        let _ = (&mut self.output).await;
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.output.abort();
        let _ = self.child.start_kill();
    }
}

fn readiness(line: &[u8]) -> Result<Option<Value>, String> {
    let Ok(outer) = serde_json::from_slice::<Value>(line) else {
        return Ok(None);
    };
    if outer.get("outcome").and_then(Value::as_str) == Some("display") {
        let value: Value = serde_json::from_str(
            outer
                .get("message")
                .and_then(Value::as_str)
                .ok_or("Mimir readiness omitted its message")?,
        )
        .map_err(|_| "Mimir bridge readiness is not JSON; check the qualified bridge command")?;
        protocol::version(&value)?;
        let actions = value
            .pointer("/capabilities/actions")
            .and_then(Value::as_array)
            .ok_or("Mimir bridge omitted capabilities")?;
        for action in [
            "catalog",
            "create",
            "open",
            "snapshot",
            "configure",
            "prompt",
            "answer",
            "cancel",
            "release",
            "decide_plan",
            "steer",
        ] {
            if !actions.iter().any(|v| v.as_str() == Some(action)) {
                return Err(format!(
                    "Mimir bridge does not support required action {action}"
                ));
            }
        }
        return Ok(Some(value));
    }
    Ok(None)
}

async fn terminate(child: &mut Child) {
    if let Some(pid) = child.id() {
        #[cfg(unix)]
        {
            // An owned process group, not arbitrary user Mimir processes.
            let mut command = Command::new("kill");
            command
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let _ = tokio::time::timeout(CONTROL_TIMEOUT, command.status()).await;
        }
        #[cfg(windows)]
        {
            let mut command = Command::new("taskkill.exe");
            command
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            crate::process::without_a_console(command.as_std_mut());
            let _ = tokio::time::timeout(CONTROL_TIMEOUT, command.status()).await;
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

enum Received {
    Event(Value),
    Snapshot(Value),
    Gap,
    Resync(String),
    Failed(String),
}
struct Events {
    incoming: mpsc::Receiver<Received>,
    task: JoinHandle<()>,
}
impl Drop for Events {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Events {
    fn start(client: Client, id: String, cursor: Option<String>) -> Self {
        let (tx, incoming) = mpsc::channel(64);
        let task = tokio::spawn(async move {
            if let Err(error) = read_events(&client, &id, cursor, &tx).await {
                let _ = tx.send(Received::Failed(error)).await;
            }
        });
        Self { incoming, task }
    }
}
async fn read_events(
    client: &Client,
    id: &str,
    mut cursor: Option<String>,
    tx: &mpsc::Sender<Received>,
) -> Result<(), String> {
    let mut failures = 0;
    loop {
        let mut url = client.endpoint.join("events").expect("fixed path");
        url.query_pairs_mut().append_pair("id", id);
        let mut request = client
            .http
            .get(url)
            .bearer_auth(&client.token)
            .header("Accept", "text/event-stream");
        if let Some(cursor) = &cursor {
            request = request.header("Last-Event-ID", cursor);
        }
        let response = tokio::time::timeout(CONTROL_TIMEOUT, request.send()).await;
        let response = match response {
            Ok(Ok(response)) if response.status().is_success() => response,
            _ => {
                failures += 1;
                if failures >= 3 {
                    return Err("Mimir event connection failed after bounded reconnects".into());
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
                continue;
            }
        };
        let mut chunks = response.bytes_stream();
        let mut decoder = protocol::SseDecoder::default();
        let mut ready = false;
        loop {
            let chunk = tokio::time::timeout(Duration::from_secs(330), chunks.next()).await;
            let bytes = match chunk {
                Ok(Some(Ok(bytes))) => bytes,
                _ => break,
            };
            for frame in decoder.push(&bytes)? {
                match frame.kind.as_str() {
                    "ready" => {
                        if frame.data.get("session_id").and_then(Value::as_str) != Some(id) {
                            return Err("Mimir SSE session identity mismatch".into());
                        }
                        ready = true;
                        failures = 0;
                    }
                    "closed" => return Err("Mimir bridge released its attachment".into()),
                    "gap" => {
                        // A bridge cursor is not a fence for the SDK event pump.
                        // Never settle from an unfenced snapshot on this source:
                        // that could admit B before A's delayed tail arrives.
                        let _ = tx.send(Received::Gap).await;
                        return Ok(());
                    }
                    "session" if ready => {
                        let event = frame
                            .data
                            .get("event")
                            .ok_or("Mimir SSE omitted its event")?
                            .clone();
                        if event.as_str() == Some("resync") {
                            // SDK run cancellation uses a normal session event. Keep
                            // its cursor so a causally expected save-and-stop boundary
                            // can resume after it instead of replaying it forever.
                            let received = frame
                                .id
                                .clone()
                                .map(Received::Resync)
                                .unwrap_or(Received::Gap);
                            let _ = tx.send(received).await;
                            return Ok(());
                        }
                        let refresh = event.get("completed").is_some()
                            || event.pointer("/observation/event/plan_submitted").is_some()
                            || event.pointer("/observation/event/plan_updated").is_some();
                        tx.send(Received::Event(event))
                            .await
                            .map_err(|_| "Mimir event consumer stopped")?;
                        if refresh {
                            let snapshot = client.snapshot(id).await?;
                            tx.send(Received::Snapshot(snapshot))
                                .await
                                .map_err(|_| "Mimir event consumer stopped")?;
                        }
                    }
                    "session" => {
                        return Err("Mimir emitted a session event before readiness".into())
                    }
                    _ => {}
                }
                if frame.id.is_some() {
                    cursor = frame.id;
                }
            }
        }
        // A partial trailing event was never acknowledged: replay it by the
        // last *complete* cursor, rather than pretending it was processed.
        if !ready {
            failures += 1;
            if failures >= 3 {
                return Err("Mimir SSE repeatedly closed before readiness".into());
            }
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// The bridge attachment and the revocable Laplus grant share one lifetime.
/// No Debug implementation: neither the HTTP declaration nor its secret is a
/// diagnostic or a persisted provider setting.
struct McpAttachment {
    id: String,
    _grant: crate::mcp::Session,
}

async fn attach_mcp(
    bridge: &mut Bridge,
    session_id: &str,
    thread_id: &str,
    platform: &dyn crate::mcp::Platform,
) -> Result<Option<McpAttachment>, String> {
    if !bridge.supports_mcp {
        return Ok(None);
    }
    let grant = platform
        .open_session(thread_id)
        .map_err(|error| error.to_string())?;
    bridge
        .client
        .redactions
        .push(grant.authorization().to_string());
    if let Some(token) = grant
        .authorization()
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
    {
        bridge.client.redactions.push(token.to_string());
    }
    let result = bridge.client.api("attach_mcp", json!({"id":session_id,"servers":[{
        "name":"laplus","transport":{"http":{"url":grant.endpoint(),"headers":[["Authorization",grant.authorization()]]}}
    }]})).await.map_err(|error|format!("Mimir host MCP connection failed: {error}"))?;
    let id = protocol::required(&result, "attachment_id")?.to_string();
    Ok(Some(McpAttachment { id, _grant: grant }))
}

pub(crate) struct Mimir {
    bridge: Bridge,
    events: Events,
    id: String,
    projection: protocol::Projection,
    failed: bool,
    history_gap: bool,
    // Each successful save-and-stop owns one asynchronous SDK cancellation boundary.
    pending_save_resyncs: u8,
    mcp: Option<McpAttachment>,
}
impl Driver for Mimir {
    const COALESCES_QUEUED_PROMPTS: bool = true;

    async fn open(start: &Start) -> Result<Opened<Self>, String> {
        let crate::session::DriverStart::Mimir(mimir_start) = &start.driver else {
            return Err("Mimir received another driver's settings".into());
        };
        if start.runtime_mode != "full-access" {
            return Err("Mimir supports only full-access runtime policy".into());
        }
        let resume = match &start.resume_cursor {
            Some(cursor)
                if cursor.provider == start.provider
                    && cursor.value.get("version") == Some(&json!(1)) =>
            {
                Some(protocol::required(&cursor.value, "sessionId")?.to_string())
            }
            Some(_) => {
                return Err(
                    "Mimir continuation belongs to an incompatible provider/protocol".into(),
                )
            }
            None => None,
        };
        let settings = &mimir_start.settings;
        let workspace = std::fs::canonicalize(&start.workspace_root)
            .map_err(|_| "Mimir workspace does not exist")?;
        let (binary, _) = crate::provider::resolve_named(
            &settings.binary_path,
            "mimir",
            &crate::process::Search::from_environment(),
        )
        .startable_for("Mimir CLI")?;
        let mut bridge = Bridge::start(&binary, &settings.bridge_command, &workspace).await?;
        let snapshot=match bridge.client.api(if resume.is_some(){"open"}else{"create"}, if let Some(id)=&resume{json!({"id":id})}else{json!({"cwd":workspace,"configuration":{"provider":null,"model":null,"reasoning":null,"mode":"build"}})}).await {
            Ok(snapshot)=>snapshot,Err(error)=>{bridge.stop().await;return Err(error);}
        };
        let id = match snapshot.pointer("/info/id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => {
                bridge.stop().await;
                return Err("Mimir snapshot omitted session ID".into());
            }
        };
        let mcp =
            match attach_mcp(&mut bridge, &id, &start.thread_id, mimir_start.mcp.as_ref()).await {
                Ok(attachment) => attachment,
                Err(error) => {
                    let _ = tokio::time::timeout(
                        CONTROL_TIMEOUT,
                        bridge.client.api("release", json!({"id":id})),
                    )
                    .await;
                    bridge.stop().await;
                    return Err(error);
                }
            };
        let mut projection = protocol::Projection::default();
        let mut decided = projection.snapshot(&snapshot, None, &crate::clock::now_iso());
        if snapshot["active_request"].is_null() {
            // A saved-plan decision can reopen without sending a prompt. Publish
            // the idle attach state rather than leaving the UI starting forever.
            decided
                .changes
                .push(crate::threads::Change::Session(crate::threads::Session {
                    status: crate::settling::SessionStatus::Ready,
                    runtime_mode: start.runtime_mode.clone(),
                    active_turn_id: None,
                    last_error: None,
                    updated_at: crate::clock::now_iso(),
                }));
        }
        let (kind, notice) = if mcp.is_some() {
            (
                "provider.mcp-attached",
                "Laplus host tools connected for this Mimir conversation.",
            )
        } else {
            ("provider.mcp-unavailable", "Laplus host tools are unavailable: this Mimir bridge does not advertise session MCP attachment support. The conversation remains usable without host tools.")
        };
        decided.changes.push(crate::threads::Change::Activity(
            crate::threads::Activity::info(kind, notice, json!({"detail":notice}), None),
        ));
        decided.provider_resume_cursor = Some(crate::provider::ResumeCursor {
            provider: start.provider.clone(),
            value: json!({"version":1,"sessionId":id}),
        });
        let events = Events::start(bridge.client.clone(), id.clone(), None);
        Ok(Opened {
            driver: Self {
                bridge,
                events,
                id,
                projection,
                failed: false,
                history_gap: false,
                pending_save_resyncs: 0,
                mcp,
            },
            decided,
        })
    }
    fn take_unfinished_message_ids(&mut self) -> Vec<String> {
        self.projection.take_unfinished_message_ids()
    }

    async fn next(&mut self, driving: &mut Driving) -> Option<Decided> {
        if self.failed {
            return None;
        }
        // The task owns all reconnect/snapshot I/O. Receiving and folding below
        // has no await after dequeue and is cancel-safe in session::drive.
        let event = self.events.incoming.recv().await?;
        Some(match event {
            Received::Event(event) => {
                self.projection
                    .event(&event, driving, &crate::clock::now_iso())
            }
            Received::Snapshot(snapshot) => {
                self.projection
                    .snapshot(&snapshot, Some(driving), &crate::clock::now_iso())
            }
            Received::Gap => {
                self.history_gap = true;
                Decided {
                    changes: vec![crate::threads::Change::Activity(
                        crate::threads::Activity::failed(
                            "provider.history-gap",
                            protocol::HISTORY_WARNING,
                        ),
                    )],
                    retires: true,
                    ..Default::default()
                }
            }
            Received::Resync(cursor) if self.pending_save_resyncs > 0 => {
                self.pending_save_resyncs = self
                    .pending_save_resyncs
                    .checked_sub(1)
                    .expect("positive pending save count");
                // Published bridge 0.2.1 delivers each Plan run cancellation
                // asynchronously after successful save-and-stop. Newer work may
                // already have been folded by then; resuming after this event's
                // exact cursor neither replays that work nor skips later events.
                // Explicit gaps, cursorless resync and excess resyncs remain fatal.
                self.events =
                    Events::start(self.bridge.client.clone(), self.id.clone(), Some(cursor));
                Decided::default()
            }
            Received::Resync(_) => {
                self.history_gap = true;
                Decided {
                    changes: vec![crate::threads::Change::Activity(
                        crate::threads::Activity::failed(
                            "provider.history-gap",
                            protocol::HISTORY_WARNING,
                        ),
                    )],
                    retires: true,
                    ..Default::default()
                }
            }
            Received::Failed(error) => {
                self.failed = true;
                Decided {
                    changes: vec![crate::threads::Change::Activity(
                        crate::threads::Activity::failed("provider.failed", &error),
                    )],
                    ..Default::default()
                }
            }
        })
    }
    async fn send(&mut self, prompt: &crate::threads::Prompt) -> io::Result<()> {
        let configuration = protocol::configuration(
            prompt.wanted.model.as_deref(),
            &prompt.wanted.model_options,
            &prompt.wanted.interaction_mode,
            &prompt.wanted.runtime_mode,
        )
        .map_err(io::Error::other)?;
        self.bridge
            .client
            .api(
                "configure",
                json!({"id":self.id,"configuration":configuration}),
            )
            .await
            .map_err(io::Error::other)?;
        let mut text = Vec::new();
        let mut images = Vec::new();
        for (message, attachments) in prompt.messages() {
            text.push(message);
            for attachment in attachments {
                if !attachment.mime.starts_with("image/") {
                    return Err(io::Error::other(
                        "Mimir bridge accepts image attachments only",
                    ));
                }
                let data = tokio::fs::read(&attachment.path).await?;
                images.push(json!({"media_type":attachment.mime,"data":data}));
            }
        }
        // There is one queue: session::drive holds next turns locally. Never
        // also submit follow_up, or a single click would create two requests.
        let accepted=self.bridge.client.api("prompt",json!({"id":self.id,"input":{"text":text.join("\n\n"),"images":images},"delivery":"start"})).await.map_err(io::Error::other)?;
        self.projection.accepted_request = Some(
            protocol::required(&accepted, "accepted")
                .map_err(io::Error::other)?
                .into(),
        );
        Ok(())
    }
    async fn interrupt(&mut self, _: &str) -> io::Result<()> {
        tokio::time::timeout(
            CONTROL_TIMEOUT,
            self.bridge.client.api("cancel", json!({"id":self.id})),
        )
        .await
        .map_err(|_| io::Error::other("Mimir cancellation timed out"))?
        .map(|_| ())
        .map_err(io::Error::other)
    }
    async fn answer(
        &mut self,
        asked: &crate::approval::ApprovalRequest,
        reply: Reply<'_>,
    ) -> io::Result<()> {
        let Reply::Answers(answers) = reply else {
            return Err(io::Error::other("Mimir supports native question answers, not remote tool approval or question dismissal"));
        };
        let answers = protocol::answers(
            asked
                .provider_request_id
                .as_ref()
                .ok_or_else(|| io::Error::other("Mimir question correlation is absent"))?,
            answers,
        )
        .map_err(io::Error::other)?;
        self.bridge
            .client
            .api(
                "answer",
                json!({"id":self.id,"request_id":asked.request_id,"answers":answers}),
            )
            .await
            .map(|_| ())
            .map_err(io::Error::other)
    }
    async fn measure(&mut self, _: &str) -> io::Result<()> {
        Err(io::Error::other(
            "Mimir context arrives through observations; on-demand measurement is unsupported",
        ))
    }
    async fn retune(&mut self, _: &str, asked: &crate::session::Pushed) -> io::Result<()> {
        // send configures the complete turn selection atomically, including
        // reasoning and Build/Plan. Reject unsupported runtime policy here.
        if let crate::session::Pushed::Mode { asked, .. } = asked {
            if asked != "full-access" {
                return Err(io::Error::other(
                    "Mimir supports only full-access runtime policy",
                ));
            }
        }
        Ok(())
    }
    async fn steer(&mut self, text: &str) -> io::Result<()> {
        self.bridge
            .client
            .api("steer", json!({"id":self.id,"text":text}))
            .await
            .map(|_| ())
            .map_err(io::Error::other)
    }
    async fn decide_plan(&mut self, id: &str, implement: bool) -> io::Result<Decided> {
        if !implement && self.pending_save_resyncs == u8::MAX {
            return Err(io::Error::other(
                "Too many Save-and-stop cancellations are still pending",
            ));
        }
        let result=self.bridge.client.api("decide_plan",json!({"id":self.id,"plan_id":id,"decision":if implement{"implement"}else{"save_and_stop"}})).await.map_err(io::Error::other)?;
        if !implement {
            // Each successful bridge mutation is one correlation boundary.
            self.pending_save_resyncs = self
                .pending_save_resyncs
                .checked_add(1)
                .expect("pending save count was bounded before mutation");
        }
        if implement {
            self.projection.accepted_request = Some(
                protocol::required(&result, "accepted")
                    .map_err(io::Error::other)?
                    .into(),
            );
        }
        // Acceptance is the mutation boundary. A second HTTP read must not
        // turn accepted implementation work into an apparent failure (and
        // leave its request without a Laplus turn). Completion observations
        // refresh the full artifact independently.
        let mut decided = Decided::default();
        if let Some(plan) = self
            .projection
            .plan
            .as_mut()
            .filter(|plan| plan["id"] == id)
        {
            plan["status"] = json!(if implement {
                "implementing"
            } else {
                "saved_stopped"
            });
            if let Some(plan) = protocol::proposed_plan(plan, None, &crate::clock::now_iso()) {
                decided
                    .changes
                    .push(crate::threads::Change::ProposedPlan(plan));
            }
        }
        decided
            .changes
            .push(crate::threads::Change::InteractionModeSet {
                interaction_mode: "default".into(),
            });
        Ok(decided)
    }
    fn close_input(&mut self) {
        // HTTP has no stdin EOF. Closing Laplus's prompt channel must still
        // wake next(), otherwise server shutdown waits forever on idle SSE.
        self.failed = true;
        self.events.task.abort();
        self.events.incoming.close();
    }
    async fn stop(mut self, driving: &mut Driving, asked_to_stop: bool) -> Reaped {
        if let Some(attachment) = self.mcp.take() {
            let _ = tokio::time::timeout(
                CONTROL_TIMEOUT,
                self.bridge.client.api(
                    "detach_mcp",
                    json!({"id":self.id,"attachment_id":attachment.id}),
                ),
            )
            .await;
            // Revoke even if remote cleanup failed. Releasing/reaping the
            // owned bridge below is the final bound on its SDK attachment.
            drop(attachment);
        }
        let _ = tokio::time::timeout(
            CONTROL_TIMEOUT,
            self.bridge.client.api("release", json!({"id":self.id})),
        )
        .await;
        self.events.task.abort();
        self.bridge.stop().await;
        Reaped {
            refused: None,
            // Normal idle eviction releases the bridge, not an unfinished turn.
            death: (!asked_to_stop && (driving.turn.is_some() || self.history_gap)).then(|| {
                if self.history_gap {
                    protocol::HISTORY_WARNING.into()
                } else {
                    "Mimir bridge ended; saved conversation data was retained.".into()
                }
            }),
        }
    }
    fn continuation_id(&self) -> Option<String> {
        Some(self.id.clone())
    }
}

/// Probe on the existing blocking provider-refresh worker, then reap the bridge.
pub(crate) fn describe(
    instance: &crate::provider::MimirInstance,
    search: &crate::process::Search,
    cwd: &Path,
) -> crate::config::Provider {
    use crate::config::{AuthStatus, Provider, ProviderAuth, ProviderState};
    let mut provider = Provider {
        instance_id: instance.identity.instance_id.clone(),
        driver: "mimir".into(),
        display_name: instance.display_name.clone(),
        enabled: instance.settings.enabled,
        installed: false,
        version: None,
        status: ProviderState::Error,
        message: None,
        auth: ProviderAuth {
            status: AuthStatus::Unknown,
            r#type: None,
            label: None,
            email: None,
        },
        checked_at: crate::clock::now_iso(),
        catalogue_state: None,
        models: vec![],
        // Native SDK prompt commands, not terminal-only configuration menus.
        slash_commands: vec![
            json!({"name":"goal","description":"Inspect the session goal (no arguments), or manage it","input":{"hint":"[duration] <objective> | pause | clear | edit <objective> | resume [duration]"}}),
            json!({"name":"compress","description":"Compress the current conversation"}),
            json!({"name":"init","description":"Create or update workspace instructions"}),
        ],
        skills: vec![],
        version_advisory: None,
        update_state: None,
    };
    let (binary, _) =
        match crate::provider::resolve_named(&instance.settings.binary_path, "mimir", search)
            .startable_for("Mimir CLI")
        {
            Ok(binary) => binary,
            Err(error) => {
                provider.message = Some(error);
                return provider;
            }
        };
    provider.installed = true;
    if !instance.settings.enabled {
        return provider;
    }
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Cannot start Mimir probe runtime".to_string())
        .and_then(|runtime| {
            runtime.block_on(async {
                let cwd = std::fs::canonicalize(cwd)
                    .map_err(|_| "Mimir probe workspace is unavailable")?;
                let mut bridge =
                    Bridge::start(&binary, &instance.settings.bridge_command, &cwd).await?;
                let result = bridge
                    .client
                    .api("catalog", json!({}))
                    .await
                    .and_then(|catalog| protocol::models(&catalog));
                bridge.stop().await;
                result
            })
        });
    match result {
        Ok(models) => {
            provider.models = models;
            provider.status = ProviderState::Ready;
            provider.catalogue_state = Some(crate::config::ProviderCatalogueState::Verified);
            provider.message=Some("Mimir bridge v1. Live integration; transient replay may be incomplete. Provider login and plugin administration remain in Mimir.".into());
        }
        Err(error) => provider.message = Some(error),
    }
    provider
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mimir_endpoint_refuses_remote_redirect_credentials() {
        for url in [
            "http://localhost:80",
            "https://127.0.0.1:32",
            "http://127.0.0.1:32/path",
            "http://user:pw@127.0.0.1:32",
            "http://127.0.0.1:32/?token=x",
            "http://192.168.0.1:32",
        ] {
            assert!(Client::new(url, "secret".into()).is_err(), "{url}");
        }
        assert!(Client::new("http://127.0.0.1:32", "secret".into()).is_ok());
    }
    #[tokio::test]
    async fn mimir_bootstrap_is_private_and_removed() {
        let project = tempfile::tempdir().unwrap();
        let workspace = project.path().canonicalize().unwrap();
        let token = random_hex().unwrap();
        let launch = Bootstrap::create(&workspace, &token).await.unwrap();
        let path = launch.file.clone();
        let value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["token"], token);
        assert_eq!(token.len(), 64);
        assert_eq!(value["workspace"], json!(workspace));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&launch.dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }

        #[cfg(windows)]
        {
            let script = r#"$ErrorActionPreference='Stop'; $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User; $dir=Get-Acl -LiteralPath $env:LAPLUS_MIMIR_LAUNCH_DIR; $file=Get-Acl -LiteralPath $env:LAPLUS_MIMIR_LAUNCH_FILE; $dirRules=@($dir.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])); $fileRules=@($file.GetAccessRules($true,$true,[System.Security.Principal.SecurityIdentifier])); if(!$dir.AreAccessRulesProtected -or $dir.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value -or $dirRules.Count -ne 1 -or $dirRules[0].IdentityReference.Value -ne $sid.Value){throw 'directory ACL mismatch'}; if($fileRules.Count -ne 1 -or $fileRules[0].IdentityReference.Value -ne $sid.Value -or !$fileRules[0].IsInherited -or $fileRules[0].AccessControlType -ne 'Allow'){throw 'launch file ACL mismatch'}"#;
            let mut command = Command::new("powershell.exe");
            command
                .args(["-NoProfile", "-NonInteractive", "-Command", script])
                .env("LAPLUS_MIMIR_LAUNCH_DIR", &launch.dir)
                .env("LAPLUS_MIMIR_LAUNCH_FILE", &path)
                .env_remove("PSModulePath")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            crate::process::without_a_console(command.as_std_mut());
            let status = tokio::time::timeout(CONTROL_TIMEOUT, command.status()).await;
            assert!(
                matches!(status, Ok(Ok(status)) if status.success()),
                "Mimir bootstrap ACL is private and inherited by launch.json"
            );
        }
        drop(launch);
        assert!(!path.exists());
    }
    #[test]
    fn mimir_readiness_requires_version_and_native_controls() {
        assert_eq!(readiness(b"{\"type\":\"notice\"}").unwrap(), None);
        let mut ready = json!({"version":1,"endpoint":"http://127.0.0.1:9000","capabilities":{"actions":["catalog","create","open","snapshot","configure","prompt","answer","cancel","release","decide_plan","steer"]}});
        let wire = |ready: &Value| {
            serde_json::to_vec(&json!({"outcome":"display","message":ready.to_string()})).unwrap()
        };
        assert!(readiness(&wire(&ready)).unwrap().is_some());
        ready["version"] = json!(2);
        assert!(readiness(&wire(&ready)).is_err());
        ready["version"] = json!(1);
        ready["capabilities"]["actions"]
            .as_array_mut()
            .unwrap()
            .pop();
        assert!(readiness(&wire(&ready)).unwrap_err().contains("steer"));
    }
    #[tokio::test]
    async fn mimir_api_authenticates_redacts_errors_and_never_retries_mutations() {
        use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        async fn api(
            State(count): State<Arc<AtomicUsize>>,
            headers: HeaderMap,
            Json(value): Json<Value>,
        ) -> Json<Value> {
            count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(headers["authorization"], "Bearer private-test-token");
            assert!(headers.get("origin").is_none());
            assert_eq!(value["version"], 1);
            assert_eq!(value["action"], "prompt");
            Json(
                json!({"version":1,"error":{"code":"sdk_error","message":"refused private-test-token"}}),
            )
        }
        let count = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = Client::new(
            &format!("http://{}", listener.local_addr().unwrap()),
            "private-test-token".into(),
        )
        .unwrap();
        let app = Router::new()
            .route("/api", post(api))
            .with_state(count.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let error = client
            .api("prompt", json!({"id":"sdk-session"}))
            .await
            .unwrap_err();
        assert!(error.contains("[redacted]"));
        assert!(!error.contains("private-test-token"));
        assert!(!format!("{client:?}").contains("private-test-token"));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        task.abort();
        let _ = task.await;
    }
}
