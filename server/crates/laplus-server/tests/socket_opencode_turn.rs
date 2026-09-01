//! OpenCode's first complete turn through the same WebSocket boundary as the UI.
//!
//! The configured binary is a small platform script which re-enters this test
//! executable as the ignored HTTP/SSE peer below. That keeps the test hermetic
//! while exercising the real child-process, HTTP, session and socket paths.

mod harness;

use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use futures_util::stream;
use harness::{
    conversation::{
        activity, assistant_sends, create_project, create_thread, follow_up, follow_up_in, interrupt_turn, last_session,
        revert_checkpoint,
        respond_to_approval, respond_to_user_input, start_turn, start_turn_in,
    },
    subagents::{child_row, child_stream, folded_entries},
    workspace::Workspace,
    SocketClient, TestServer,
};
use laplus_server::config::ServerConfig;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

fn part_labels(parts: &[Value]) -> Vec<(&str, &str)> {
    parts.iter().map(|part| (
        part["type"].as_str().unwrap(),
        part["text"].as_str().or_else(|| part["filename"].as_str()).unwrap(),
    )).collect()
}

struct FakeOpenCode {
    directory: tempfile::TempDir,
    log: PathBuf,
}

#[derive(Clone, Copy)]
enum Startup {
    Healthy,
    DelayedCatalogue,
    Gated,
    ResistsStop,
    Runaway,
    Exit,
    NeverReady,
    McpFailure,
    /// A turn that spawns a subagent, which OpenCode runs as a session of its
    /// own — see [`a_subagent_gets_a_row_of_its_own_and_says_what_it_is_doing`].
    Subagent,
}

impl FakeOpenCode {
    fn new() -> Self {
        Self::scripted(Startup::Healthy)
    }

    fn exiting() -> Self {
        Self::scripted(Startup::Exit)
    }

    fn busy() -> Self {
        Self::scripted(Startup::Gated)
    }

    fn delayed_catalogue() -> Self {
        Self::scripted(Startup::DelayedCatalogue)
    }

    fn resisting_stop() -> Self {
        Self::scripted(Startup::ResistsStop)
    }

    fn runaway() -> Self {
        Self::scripted(Startup::Runaway)
    }

    fn spawning_a_subagent() -> Self {
        Self::scripted(Startup::Subagent)
    }

    fn never_ready() -> Self {
        Self::scripted(Startup::NeverReady)
    }

    fn mcp_failure() -> Self { Self::scripted(Startup::McpFailure) }

    fn scripted(startup: Startup) -> Self {
        let directory = tempfile::tempdir().expect("temporary OpenCode directory");
        let log = directory.path().join("requests.jsonl");
        let executable = std::env::current_exe().expect("the socket test executable");
        let path = directory.path().join(if cfg!(windows) {
            "opencode.cmd"
        } else {
            "opencode"
        });
        let serve = match (startup, cfg!(windows)) {
            (Startup::Exit, true) => "exit /b 23",
            (Startup::Exit, false) => "exit 23",
            (Startup::Healthy, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::DelayedCatalogue, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\nset OPENCODE_TEST_DELAY_CATALOGUE=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::ResistsStop, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::Runaway, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\nset OPENCODE_TEST_GATED=true\r\nset OPENCODE_TEST_RUNAWAY=true\r\nset OPENCODE_TEST_SUBAGENT=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::Gated, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\nset OPENCODE_TEST_GATED=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::NeverReady, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=false\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::McpFailure, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\nset OPENCODE_TEST_MCP_FAIL=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::Subagent, true) => "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\nset OPENCODE_TEST_SUBAGENT=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::Healthy, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::DelayedCatalogue, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true OPENCODE_TEST_DELAY_CATALOGUE=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::ResistsStop, false) => "trap '' TERM\nOPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::Runaway, false) => "trap '' TERM\nOPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true OPENCODE_TEST_GATED=true OPENCODE_TEST_RUNAWAY=true OPENCODE_TEST_SUBAGENT=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::Gated, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true OPENCODE_TEST_GATED=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::NeverReady, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=false exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::McpFailure, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true OPENCODE_TEST_MCP_FAIL=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::Subagent, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true OPENCODE_TEST_SUBAGENT=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
        };
        let serve = serve
            .replace("{log}", &log.display().to_string())
            .replace("{executable}", &executable.display().to_string());
        let script = if cfg!(windows) {
            format!(
                "@echo off\r\nif \"%1\"==\"--version\" (echo 1.18.10& exit /b 0)\r\nif \"%1\"==\"models\" (echo openai/gpt-5& echo {{\"id\":\"gpt-5\",\"name\":\"GPT 5\",\"variants\":{{}}}}& exit /b 0)\r\nif \"%1\"==\"agent\" (echo build ^(primary^)& exit /b 0)\r\n{serve}\r\n"
            )
        } else {
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  --version) echo 1.18.10; exit 0;;\n  models) printf '%s\\n' 'openai/gpt-5' '{{\"id\":\"gpt-5\",\"name\":\"GPT 5\",\"variants\":{{}}}}'; exit 0;;\n  agent) echo 'build (primary)'; exit 0;;\nesac\n{serve}\n"
            )
        };
        std::fs::write(&path, script).expect("write fake OpenCode executable");
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
                .expect("make fake OpenCode executable");
        }
        Self { directory, log }
    }

    fn configured(&self) -> String {
        self.directory
            .path()
            .join(if cfg!(windows) {
                "opencode.cmd"
            } else {
                "opencode"
            })
            .display()
            .to_string()
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests_through(4).await
    }

    async fn requests_through(&self, count: usize) -> Vec<Value> {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&self.log) {
                    if !contents.is_empty() && !contents.ends_with('\n') {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        continue;
                    }
                    let values = contents
                        .lines()
                        .map(|line| serde_json::from_str(line).expect("logged JSON request"))
                        .collect::<Vec<_>>();
                    if values.len() >= count {
                        return values;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("OpenCode records its session and prompt requests")
    }
}

fn opencode_config(opencode: &FakeOpenCode) -> ServerConfig {
    let mut config = ServerConfig::detect();
    config.settings.provider_instances.insert(
        "openLocal".into(),
        json!({"driver":"opencode","displayName":"OpenCode Local","config":{
            "binaryPath":opencode.configured(),"serverUrl":"","serverPassword":"",
            "customModels":["openai/gpt-5"]
        }}),
    );
    config
}

struct SocketTurn {
    _workspace: Workspace,
    opencode: FakeOpenCode,
    server: TestServer,
    client: SocketClient,
    subscription: String,
}

#[derive(Debug)]
struct FakeMcpPlatform {
    opens: AtomicUsize,
    origin: Mutex<Option<String>>,
    sessions: Arc<Mutex<std::collections::HashMap<String, String>>>,
}

impl laplus_server::mcp::Platform for FakeMcpPlatform {
    fn open_session(&self, thread_id: &str) -> Result<laplus_server::mcp::Session, laplus_server::mcp::OpenError> {
        let ordinal = self.opens.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("fake-{thread_id}-{ordinal}");
        let authorization = format!("Bearer fake-grant-{ordinal}");
        self.sessions.lock().unwrap().insert(id.clone(), authorization.clone());
        let endpoint = format!("{}/mcp/{id}", self.origin.lock().unwrap().as_ref().ok_or(laplus_server::mcp::OpenError)?);
        let sessions = Arc::clone(&self.sessions);
        Ok(laplus_server::mcp::Session::for_adapter(endpoint, authorization, move || { sessions.lock().unwrap().remove(&id); }))
    }
    fn set_origin(&self, origin: String) { *self.origin.lock().unwrap() = Some(origin); }
    fn authorizes(&self, id: &str, authorization: &str) -> bool { self.sessions.lock().unwrap().get(id).is_some_and(|grant| grant == authorization) }
    fn live_sessions(&self) -> usize { self.sessions.lock().unwrap().len() }
    fn dispatch<'a>(&'a self, _id: &'a str, message: Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + 'a>> {
        Box::pin(async move { laplus_server::mcp::dispatch(message) })
    }
}

async fn start_socket_turn(
    opencode: FakeOpenCode,
    project_id: &str,
    thread_id: &str,
) -> SocketTurn {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(opencode_config(&opencode)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project(project_id, workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread(project_id, thread_id);
    create["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation(thread_id).await;
    let mut command = start_turn(thread_id, &format!("message-{thread_id}"), "say hello");
    command["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    SocketTurn {
        _workspace: workspace,
        opencode,
        server,
        client,
        subscription,
    }
}

async fn wait_until_port_closes(port: u16, because: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{because}"));
}

#[tokio::test]
async fn owned_opencode_uses_the_injected_generic_mcp_platform() {
    let opencode = FakeOpenCode::new();
    let platform = Arc::new(FakeMcpPlatform {
        opens: AtomicUsize::new(0),
        origin: Mutex::new(None),
        sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
    });
    let server = TestServer::start_with_mcp(opencode_config(&opencode), platform.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("fake-mcp-project", workspace.path())).await.expect_success();
    let mut create = create_thread("fake-mcp-project", "fake-mcp-thread");
    create["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation("fake-mcp-thread").await;
    let mut turn = start_turn("fake-mcp-thread", "fake-mcp-message", "hello");
    turn["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", turn).await.expect_success();
    client.events_through_the_turn(&subscription).await;
    assert_eq!(platform.opens.load(Ordering::SeqCst), 1);
    assert_eq!(server.live_mcp_sessions(), 1);
    client.call("orchestration.dispatchCommand", json!({"type":"thread.session.stop","commandId":"fake:stop","threadId":"fake-mcp-thread","createdAt":"2026-08-01T00:00:00.000Z"})).await.expect_success();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while server.live_mcp_sessions() != 0 { tokio::task::yield_now().await; }
    }).await.expect("fake platform session is released");
    client.close().await;
    server.stop().await;
}

fn set_runtime_mode(thread_id: &str, mode: &str) -> Value {
    json!({"type":"thread.runtime-mode.set","commandId":"test:mode","threadId":thread_id,"runtimeMode":mode})
}

#[tokio::test]
async fn opencode_tools_and_permissions_cross_the_socket_and_reply_on_the_v2_route() {
    let peer = ExternalOpenCode::with_permissions().await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("permission-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("permission-project", "permission-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    create["runtimeMode"] = json!("approval-required");
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("permission-thread").await;
    let mut command = start_turn_in(
        "permission-thread",
        "permission-message",
        "use a tool",
        "approval-required",
    );
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    let asked = client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["requestId"] == "per-1"
        })
        .await;
    let requested = asked
        .iter()
        .find(|item| item["event"]["payload"]["activity"]["payload"]["requestId"] == "per-1")
        .unwrap();
    assert_eq!(
        requested["event"]["payload"]["activity"]["payload"]["requestKind"],
        "command"
    );
    assert_eq!(
        requested["event"]["payload"]["activity"]["payload"]["data"]["toolCallId"],
        "call-1"
    );
    let started = asked
        .iter()
        .find(|item| {
            item["event"]["payload"]["activity"]["kind"] == "tool.updated"
                && item["event"]["payload"]["activity"]["payload"]["data"]["toolName"]
                    == "mystery"
        })
        .unwrap();
    assert_eq!(
        started["event"]["payload"]["activity"]["payload"]["itemType"],
        "dynamic_tool_call"
    );
    assert_eq!(
        started["event"]["payload"]["activity"]["payload"]["data"]["state"]["input"]["secret"],
        42
    );
    assert_eq!(
        started["event"]["payload"]["activity"]["payload"]["data"]["raw"]["id"],
        "evt-tool-1"
    );
    let item_types = asked
        .iter()
        .filter(|item| matches!(item["event"]["payload"]["activity"]["kind"].as_str(), Some("tool.started" | "tool.updated")))
        .map(|item| {
            item["event"]["payload"]["activity"]["payload"]["itemType"]
                .as_str()
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        item_types,
        vec!["dynamic_tool_call"]
    );
    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("permission-thread", "per-1", "accept"),
        )
        .await
        .expect_success();
    let requests = peer.requests_through(3).await;
    let create = requests
        .iter()
        .find(|request| request["operation"] == "create")
        .unwrap();
    assert_eq!(
        create["body"]["permission"][0],
        json!({"permission":"*","pattern":"*","action":"ask"})
    );
    assert_eq!(
        create["body"]["permission"][8],
        json!({"permission":"question","pattern":"*","action":"allow"})
    );
    let reply = requests
        .iter()
        .find(|request| request["operation"] == "permission.reply")
        .unwrap();
    assert_eq!(reply["requestId"], "per-1");
    assert_eq!(reply["body"], json!({"reply":"once"}));
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn every_shared_permission_decision_has_an_opencode_v2_reply() {
    for (index, decision, expected) in [
        (1, "accept", "once"),
        (2, "acceptForSession", "always"),
        (3, "decline", "reject"),
        (4, "cancel", "reject"),
    ] {
        let peer = ExternalOpenCode::with_permissions().await;
        let workspace = Workspace::with(&["src/"]);
        let server = TestServer::start_with(peer.config(None)).await;
        let mut client = server.connect().await;
        let project = format!("decision-project-{index}");
        let thread = format!("decision-thread-{index}");
        client
            .call(
                "orchestration.dispatchCommand",
                create_project(&project, workspace.path()),
            )
            .await
            .expect_success();
        let mut create = create_thread(&project, &thread);
        create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
        client
            .call("orchestration.dispatchCommand", create)
            .await
            .expect_success();
        let subscription = client.watch_conversation(&thread).await;
        let mut command = start_turn(&thread, &format!("message-{index}"), "ask");
        command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
        client
            .call("orchestration.dispatchCommand", command)
            .await
            .expect_success();
        client
            .values_until(&subscription, |item| {
                item["event"]["payload"]["activity"]["kind"] == "approval.requested"
            })
            .await;
        client
            .call(
                "orchestration.dispatchCommand",
                respond_to_approval(&thread, "per-1", decision),
            )
            .await
            .expect_success();
        let requests = peer.requests_through(3).await;
        let reply = requests
            .iter()
            .find(|request| request["operation"] == "permission.reply")
            .unwrap();
        assert_eq!(
            reply["body"],
            json!({"reply":expected}),
            "decision {decision}"
        );
        client.close().await;
        server.stop().await;
        peer.task.abort();
    }
}

#[tokio::test]
async fn retuning_opencode_reapplies_the_same_permission_rules_with_patch() {
    let peer = ExternalOpenCode::start(None).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("retune-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("retune-project", "retune-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("retune-thread").await;
    let mut first = start_turn("retune-thread", "retune-message-1", "first");
    first["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", first)
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("retune-thread", "approval-required"),
        )
        .await
        .expect_success();
    let mut second = start_turn_in(
        "retune-thread",
        "retune-message-2",
        "second",
        "approval-required",
    );
    second["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", second)
        .await
        .expect_success();
    let requests = peer.requests_through(4).await;
    let update = requests
        .iter()
        .find(|request| request["operation"] == "update")
        .unwrap();
    assert_eq!(update["sessionId"], "ses_owned_1");
    assert_eq!(
        update["body"]["permission"][0],
        json!({"permission":"*","pattern":"*","action":"ask"})
    );
    assert_eq!(
        update["body"]["permission"][8],
        json!({"permission":"question","pattern":"*","action":"allow"})
    );
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[derive(Clone, Default)]
struct PeerState {
    subscriber: Arc<Mutex<Option<mpsc::Sender<Result<String, Infallible>>>>>,
    log: Arc<PathBuf>,
    mcp_failed: bool,
    healthy: bool,
    authorization: Option<String>,
    idle_release: Option<Arc<Notify>>,
    prompts: Arc<AtomicUsize>,
    catalogue_requests: Arc<AtomicUsize>,
    message_snapshots: Arc<AtomicUsize>,
    delayed_catalogue: bool,
    permissions: bool,
    questions: bool,
    resume: ResumeBehavior,
    gets: Arc<AtomicUsize>,
    creates: Arc<AtomicUsize>,
    rollback_probe: Option<PathBuf>,
    rollback_fails: bool,
    /// Script the turn that spawns a subagent rather than the plain one.
    subagent: bool,
    /// Script the same subagent doing the *whole* of what OpenCode exposes — a
    /// command, a read, a search, an edit, another tool, a retry warning — plus
    /// one event kind this build has never heard of.
    child_work: bool,
    /// Script a blocker raised by the subagent's own session: `"permission"`,
    /// `"question"`, or `"legacy"` — the session-scoped pre-`permission.asked`
    /// envelope, whose reply route needs the *child's* session id.
    child_blocker: Option<&'static str>,
    /// Refuse the reply route, so the developer's decision never reaches the
    /// child that is waiting for it.
    child_reply_fails: bool,
    fail_reconciliation: bool,
    fail_followup_prompt: bool,
    /// Script a turn whose narration is several text parts spoken around a tool
    /// call — commentary, tools, more commentary, an announced-but-empty part —
    /// which is the transcript shape one-message-per-text-part exists to keep
    /// apart. Held before its second part completes when an idle gate is set.
    text_parts: bool,
    /// Accept the abort and never answer it. An OpenCode wedged on a stalled
    /// provider socket does exactly this: the port is open, the request is
    /// read, and no response is ever written.
    unanswered_abort: bool,
    /// The first two reconciliation snapshots pause, the third contains new
    /// output, proving that equal point samples were not quiescence.
    output_changes_during_stop: bool,
    /// The provider's authoritative history holds more of the turn than the
    /// event stream ever delivered: the block that was streaming completed,
    /// two further blocks were spoken after the tool call, and an earlier
    /// block reads differently there than what the developer was shown. Every
    /// snapshot is identical, which is what lets the quiet window close.
    lost_suffix: bool,
}

#[derive(Clone, Copy, Default)]
enum ResumeBehavior {
    #[default]
    Normal,
    Missing,
    GetFailure,
    UpdateFailure,
    ForkTarget,
    ForkThenMove,
    ForkFailure,
    MoveFailure,
    VerificationFailure,
}

fn assert_authorization(headers: &HeaderMap, state: &PeerState) {
    assert_eq!(
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        state.authorization.as_deref(),
        "the external endpoint receives exactly the configured authentication"
    );
}

fn append(path: &Path, value: Value) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open peer request log");
    writeln!(file, "{value}").expect("write peer request log");
}

async fn health(State(state): State<PeerState>) -> Json<Value> {
    Json(json!({"healthy":state.healthy,"version":"1.18.10"}))
}

async fn add_mcp(State(state): State<PeerState>, Json(body): Json<Value>) -> Json<Value> {
    let endpoint = body.pointer("/config/url").and_then(Value::as_str).expect("MCP endpoint");
    let authorization = body.pointer("/config/headers/Authorization").and_then(Value::as_str).expect("MCP authorization");
    append(&state.log, json!({
        "operation":"mcp.add",
        "authorizationPresent":authorization.starts_with("Bearer "),
        "oauth":body.pointer("/config/oauth"),
        "url":endpoint
    }));
    let response = reqwest::Client::new().post(endpoint)
        .header(header::AUTHORIZATION.as_str(), authorization)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"opencode-test","version":"1.18.10"}
        }}))
        .send().await.expect("connect to Laplus MCP");
    assert_eq!(response.status(), StatusCode::OK);
    let initialized: Value = response.json().await.expect("MCP initialize response");
    assert_eq!(initialized["result"]["protocolVersion"], "2025-06-18");
    Json(if state.mcp_failed {
        json!({"laplus":{"status":"failed","error":"scripted connection refusal"}})
    } else {
        json!({"laplus":{"status":"connected"}})
    })
}

async fn events(State(state): State<PeerState>, headers: HeaderMap) -> Response {
    assert_authorization(&headers, &state);
    let (tx, rx) = mpsc::channel(32);
    *state.subscriber.lock().expect("subscriber lock") = Some(tx);
    let body = Body::from_stream(stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    }));
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(body)
        .unwrap()
}

async fn providers(State(state): State<PeerState>) -> Json<Value> {
    if state.delayed_catalogue
        && state.catalogue_requests.fetch_add(1, Ordering::SeqCst) < 2
    {
        return Json(json!({"providers": [], "connected": []}));
    }
    Json(json!({
        "providers": [{
            "id": "openai",
            "models": {
                "gpt-5": {"id": "gpt-5", "name": "GPT 5", "limit": {"context": 200_000}},
                "gpt-alt": {"id": "gpt-alt", "name": "GPT Alt", "limit": {"context": 100_000}}
            }
        }],
        "connected": ["openai"]
    }))
}

async fn opencode_config_snapshot() -> Json<Value> {
    Json(json!({"compaction": {"auto": false}}))
}

async fn create_session(
    State(state): State<PeerState>,
    headers: HeaderMap,
    Query(query): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    assert_authorization(&headers, &state);
    append(
        &state.log,
        json!({"operation":"create","directory":query.get("directory"),"body":body}),
    );
    let create = state.creates.fetch_add(1, Ordering::SeqCst);
    Json(json!({"id":if matches!(state.resume, ResumeBehavior::Missing) && create > 0 { "ses_fresh_2" } else { "ses_owned_1" }}))
}

async fn update_session(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    append(
        &state.log,
        json!({"operation":"update","sessionId":session_id,"body":body}),
    );
    if matches!(state.resume, ResumeBehavior::UpdateFailure) {
        Err(StatusCode::UNAUTHORIZED)
    } else {
        Ok(Json(json!({"id":session_id})))
    }
}

async fn get_session(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    append(
        &state.log,
        json!({"operation":"get","sessionId":session_id,"directory":query.get("directory")}),
    );
    let get = state.gets.fetch_add(1, Ordering::SeqCst);
    match state.resume {
        ResumeBehavior::Missing => Err((StatusCode::NOT_FOUND, Json(json!({"name":"NotFoundError","message":"session not found"})))),
        ResumeBehavior::GetFailure => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"name":"InternalError","message":"failed to load session"})))),
        ResumeBehavior::VerificationFailure if get > 0 => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"name":"InternalError","message":"verification failed"})))),
        ResumeBehavior::ForkThenMove if get > 0 => Ok(Json(json!({"id":session_id,"directory":query.get("directory")}))),
        ResumeBehavior::ForkTarget | ResumeBehavior::ForkThenMove | ResumeBehavior::ForkFailure | ResumeBehavior::MoveFailure | ResumeBehavior::VerificationFailure => Ok(Json(json!({"id":session_id,"directory":"/old/opencode/worktree"}))),
        _ => Ok(Json(json!({"id":session_id,"directory":query.get("directory")}))),
    }
}

async fn fork_session(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    append(&state.log, json!({"operation":"fork","sessionId":session_id,"directory":query.get("directory"),"body":body}));
    if matches!(state.resume, ResumeBehavior::ForkFailure) {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let directory = if matches!(state.resume, ResumeBehavior::ForkTarget) {
        query.get("directory").cloned().unwrap_or_default()
    } else {
        "/old/opencode/worktree".to_string()
    };
    Ok(Json(json!({"id":"ses_forked_1","directory":directory})))
}

async fn move_session(
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    append(&state.log, json!({"operation":"move","body":body}));
    if matches!(state.resume, ResumeBehavior::MoveFailure) {
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    } else {
        Ok(Json(json!(true)))
    }
}

async fn session_messages(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
) -> Result<Json<Value>, StatusCode> {
    append(&state.log, json!({"operation":"messages","sessionId":session_id}));
    if state.fail_reconciliation {
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let snapshot = state.message_snapshots.fetch_add(1, Ordering::SeqCst) + 1;
    // The whole turn as the provider kept it, which is more than the stream
    // delivered. Deliberately awkward in the same way the SSE script is: the
    // first block reads differently here than what was shown, the tool part
    // sits between the blocks it was spoken between, and the two trailing
    // blocks were never streamed at all. Every request answers identically,
    // because stability across snapshots is what the quiet window measures.
    if state.lost_suffix {
        return Ok(Json(json!([
            {"info":{"id":"message-0","role":"user"},"parts":[
                {"id":"prt-prompt","type":"text","text":"look around"}
            ]},
            {"info":{"id":"message-1","role":"assistant"},"parts":[
                {"id":"prt-a","type":"text","text":"Reading the forest first. "},
                {"id":"prt-tool","type":"tool","callID":"call-parts-1","tool":"bash",
                 "state":{"status":"completed","input":{"command":"ls -1"},"title":"ls -1","output":"src"}},
                {"id":"prt-b","type":"text","text":"The tree holds eleven files."},
                {"id":"prt-d","type":"text","text":"Then I looked again."},
                {"id":"prt-e","type":"text","text":"Nothing else to add."}
            ]}
        ])));
    }
    let text = if state.output_changes_during_stop && snapshot >= 3 {
        format!("output snapshot {snapshot}")
    } else {
        String::new()
    };
    Ok(Json(json!([
        {"info":{"id":"assistant-1","role":"assistant"},"parts":[]},
        {"info":{"id":"assistant-2","role":"assistant"},"parts":[
            {"id":"stop-proof-text","type":"text","text":text}
        ]}
    ])))
}

async fn session_statuses(State(state): State<PeerState>) -> Result<Json<Value>, StatusCode> {
    append(&state.log, json!({"operation":"status"}));
    if state.fail_reconciliation {
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    } else {
        Ok(Json(json!({})))
    }
}

async fn revert_session(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    if let Some(probe) = &state.rollback_probe {
        assert_eq!(
            std::fs::read_to_string(probe).expect("restored file exists before provider rollback"),
            "before\n",
            "the working tree must be restored before OpenCode history is touched"
        );
    }
    append(&state.log, json!({"operation":"revert","sessionId":session_id,"body":body}));
    if state.rollback_fails {
        Err(StatusCode::INTERNAL_SERVER_ERROR)
    } else {
        Ok(Json(json!(true)))
    }
}

async fn prompt(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> StatusCode {
    assert_authorization(&headers, &state);
    append(
        &state.log,
        json!({"operation":"prompt","sessionId":session_id,"body":body}),
    );
    let prompt_number = state.prompts.fetch_add(1, Ordering::SeqCst) + 1;
    if prompt_number > 1 {
        return if state.fail_followup_prompt {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::NO_CONTENT
        };
    }
    let sender = state
        .subscriber
        .lock()
        .expect("subscriber lock")
        .clone()
        .expect("event subscription precedes prompt");
    if session_id != "ses_owned_1" {
        for event in [
            format!("data: {{\"type\":\"message.part.updated\",\"properties\":{{\"part\":{{\"id\":\"text-fresh\",\"messageID\":\"message-fresh\",\"sessionID\":\"{session_id}\",\"type\":\"text\",\"text\":\"continued OpenCode session\"}}}}}}\n\n"),
            format!("data: {{\"type\":\"session.idle\",\"properties\":{{\"sessionID\":\"{session_id}\"}}}}\n\n"),
        ] {
            sender.send(Ok(event)).await.unwrap();
        }
        return StatusCode::NO_CONTENT;
    }
    if state.permissions {
        for event in [
            "data: {\"id\":\"evt-tool-1\",\"type\":\"message.part.updated\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"part\":{\"id\":\"part-tool-1\",\"sessionID\":\"ses_owned_1\",\"messageID\":\"message-1\",\"type\":\"tool\",\"callID\":\"call-1\",\"tool\":\"mystery\",\"state\":{\"status\":\"running\",\"input\":{\"secret\":42},\"time\":{\"start\":1}}}}}\n\n",
            "data: {\"id\":\"evt-per-1\",\"type\":\"permission.asked\",\"properties\":{\"id\":\"per-1\",\"sessionID\":\"ses_owned_1\",\"permission\":\"bash\",\"patterns\":[\"cargo test\"],\"metadata\":{\"command\":\"cargo test\"},\"always\":[],\"tool\":{\"messageID\":\"message-1\",\"callID\":\"call-1\"}}}\n\n",
        ] { sender.send(Ok(event.to_string())).await.unwrap(); }
        return StatusCode::NO_CONTENT;
    }
    if state.questions {
        sender.send(Ok("data: {\"type\":\"question.asked\",\"properties\":{\"id\":\"que-1\",\"sessionID\":\"ses_owned_1\",\"questions\":[{\"header\":\"Database Choice\",\"question\":\"Which database?\",\"options\":[{\"label\":\"SQLite\",\"description\":\"Local\"},{\"label\":\"Postgres\",\"description\":\"Shared\"}],\"multiple\":false},{\"header\":\"Features\",\"question\":\"Which features?\",\"options\":[{\"label\":\"Search\",\"description\":\"Search\"},{\"label\":\"Sync\",\"description\":\"Sync\"}],\"multiple\":true}]}}\n\n".to_string())).await.unwrap();
        return StatusCode::NO_CONTENT;
    }
    // One turn, narrated the way OpenCode actually narrates: a first text part,
    // a tool call with its statuses, reasoning, then a second text part — and
    // behind the gate on purpose, so an interrupt can land between the second
    // part's first delta and its completion. Held through reconciliation, the
    // gate also keeps every confirmation of quiet back, which is what forces
    // the bounded reconcile to close the partials; what the gate releases
    // afterwards is therefore *late* output, and none of it may revise what
    // settlement closed. The snapshots it sends are deliberately awkward — a
    // duplicate of the same cumulative text and then a stale regression — and
    // the title event last is the drain marker: a reader that has seen it knows
    // every earlier event was delivered first.
    if state.text_parts {
        for event in [
            "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"busy\"}}}\n\n",
            "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt-a\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"\"}}}\n\n",
            "data: {\"type\":\"message.updated\",\"properties\":{\"info\":{\"id\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"role\":\"assistant\"}}}\n\n",
            "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"messageID\":\"message-1\",\"partID\":\"prt-a\",\"field\":\"text\",\"delta\":\"Reading the tree first. \"}}\n\n",
            "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt-a\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"Reading the tree first. \"}}}\n\n",
            "data: {\"type\":\"message.part.updated\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"part\":{\"id\":\"prt-tool\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"tool\",\"callID\":\"call-parts-1\",\"tool\":\"bash\",\"state\":{\"status\":\"running\",\"input\":{\"command\":\"ls -1\"},\"time\":{\"start\":1}}}}}\n\n",
            "data: {\"type\":\"message.part.updated\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"part\":{\"id\":\"prt-tool\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"tool\",\"callID\":\"call-parts-1\",\"tool\":\"bash\",\"state\":{\"status\":\"completed\",\"input\":{\"command\":\"ls -1\"},\"title\":\"ls -1\",\"output\":\"src\",\"time\":{\"start\":1,\"end\":2}}}}}\n\n",
            "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prs-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"reasoning\",\"text\":\"weighing what changed\"}}}\n\n",
            "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt-b\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"\"}}}\n\n",
            "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"messageID\":\"message-1\",\"partID\":\"prt-b\",\"field\":\"text\",\"delta\":\"The tree holds \"}}\n\n",
        ] {
            sender.send(Ok(event.to_string())).await.expect("send scripted text-part SSE event");
        }
        let gated = state.idle_release.is_some();
        let finish = async move {
            if let Some(release) = &state.idle_release {
                release.notified().await;
            }
            for event in [
                "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt-b\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"The tree holds eleven files.\"}}}\n\n",
                "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt-b\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"The tree holds eleven files.\"}}}\n\n",
                "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt-b\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"The tree holds\"}}}\n\n",
                // Announced and never filled: a part that says nothing has to
                // produce no message at all.
                "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"prt-c\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"\"}}}\n\n",
                "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"idle\"}}}\n\n",
                "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n",
                "data: {\"type\":\"session.updated\",\"properties\":{\"info\":{\"id\":\"ses_owned_1\",\"title\":\"Late marker\"}}}\n\n",
            ] {
                sender.send(Ok(event.to_string())).await.expect("send scripted text-part SSE event");
            }
        };
        if gated {
            tokio::spawn(finish);
        } else {
            finish.await;
        }
        return StatusCode::NO_CONTENT;
    }
    for event in [
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"reason-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"reasoning\",\"text\":\"check the stream\"}}}\n\n",
        "data: {\"type\":\"message.updated\",\"properties\":{\"info\":{\"id\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"role\":\"assistant\",\"providerID\":\"openai\",\"modelID\":\"gpt-5\",\"tokens\":{\"input\":12000,\"output\":500,\"reasoning\":300,\"cache\":{\"read\":9000,\"write\":100}}}}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"text-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"\"}}}\n\n",
        "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"busy\"}}}\n\n",
        "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"messageID\":\"message-1\",\"partID\":\"text-1\",\"field\":\"text\",\"delta\":\"hello \"}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"text-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"hello from OpenCode\"}}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"text-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"hello\"}}}\n\n",
        "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"retry\",\"message\":\"Retrying upstream\"}}}\n\n",
        "data: {\"type\":\"session.updated\",\"properties\":{\"info\":{\"id\":\"ses_owned_1\",\"title\":\"Upstream title\"}}}\n\n",
        "data: {\"type\":\"future.event\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n",
    ] {
        sender.send(Ok(event.to_string())).await.expect("send scripted SSE event");
    }
    // A subagent, in the order a real one arrives: the `task` call is announced
    // before it knows which session it will run in, the child's own session then
    // produces its messages, and the call carries the answer. Shapes and ordering
    // are from a driven OpenCode 1.18.10 — the metadata reaches the parent (here,
    // the second `task` part) before the child says anything, which is what lets
    // the driver require an introduction before listening to a child.
    //
    // **Split across the idle gate on purpose.** A gated peer stops here, so a
    // test can open the child's work stream while it is genuinely mid-work and
    // then watch the rest arrive — which is the replay/live boundary the whole
    // subagent-ux feature turns on. An ungated peer runs both halves back to
    // back, so the tests written before the gate see exactly what they did.
    if state.subagent {
        for event in [
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_owned_1","part":{"id":"prt-task","messageID":"message-1","sessionID":"ses_owned_1","type":"tool","callID":"call_task_1","tool":"task","state":{"status":"pending"}}}}"#,
            r#"data: {"type":"session.created","properties":{"sessionID":"ses_child_1","info":{"id":"ses_child_1","parentID":"ses_owned_1","agent":"explore","title":"Count files (@explore subagent)"}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_owned_1","part":{"id":"prt-task","messageID":"message-1","sessionID":"ses_owned_1","type":"tool","callID":"call_task_1","tool":"task","state":{"status":"running","input":{"description":"Count the files","subagent_type":"explore","prompt":"how many files"},"metadata":{"parentSessionId":"ses_owned_1","sessionId":"ses_child_1"}}}}}"#,
            // The prompt it was handed. A text part like any other, and not the
            // subagent talking.
            r#"data: {"type":"message.updated","properties":{"sessionID":"ses_child_1","info":{"id":"child-message-1","sessionID":"ses_child_1","role":"user"}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-1","messageID":"child-message-1","sessionID":"ses_child_1","type":"text","text":"how many files"}}}"#,
            // The subagent itself. The part is sent twice, carrying the prose so
            // far, which is what OpenCode does and what makes an entry key
            // load-bearing rather than decorative.
            r#"data: {"type":"message.updated","properties":{"sessionID":"ses_child_1","info":{"id":"child-message-2","sessionID":"ses_child_1","role":"assistant"}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-2","messageID":"child-message-2","sessionID":"ses_child_1","type":"text","text":"looking"}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-2","messageID":"child-message-2","sessionID":"ses_child_1","type":"text","text":"looking through the directory"}}}"#,
        ] {
            sender
                .send(Ok(format!("{event}\n\n")))
                .await
                .expect("send scripted subagent SSE event");
        }
    }

    // Everything OpenCode exposes about a child's work, in the shapes a driven
    // 1.18.10 emits them: a `tool` part per call, announced `pending` with an
    // empty state and then carrying its input, its title and finally its output
    // or its error. Interleaved with the child's prose, because that is how it
    // arrives and because chronology is the thing the child tab preserves.
    //
    // The last two are deliberate awkwardness rather than coverage: a retry
    // warning on the child's own session, and an event kind no build of laplus
    // has ever seen. Neither may break the parent turn or the child's stream.
    if state.child_work {
        for event in [
            r#"data: {"type":"message.updated","properties":{"sessionID":"ses_child_1","info":{"id":"child-message-2","sessionID":"ses_child_1","role":"assistant"}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-say","messageID":"child-message-2","sessionID":"ses_child_1","type":"text","text":"counting them"}}}"#,
            // Announced before it knows what it is. Nothing to draw yet.
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-bash","messageID":"child-message-2","sessionID":"ses_child_1","type":"tool","callID":"child_call_bash","tool":"bash","state":{"status":"pending"}}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-bash","messageID":"child-message-2","sessionID":"ses_child_1","type":"tool","callID":"child_call_bash","tool":"bash","state":{"status":"running","input":{"command":"ls -1 src | wc -l","description":"Count the files"}}}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-read","messageID":"child-message-2","sessionID":"ses_child_1","type":"tool","callID":"child_call_read","tool":"read","state":{"status":"completed","input":{"filePath":"src/main.rs"},"title":"src/main.rs","output":"fn main() {}"}}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-grep","messageID":"child-message-2","sessionID":"ses_child_1","type":"tool","callID":"child_call_grep","tool":"grep","state":{"status":"completed","input":{"pattern":"fn main","path":"src"},"title":"grep fn main","output":"src/main.rs:1"}}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-edit","messageID":"child-message-2","sessionID":"ses_child_1","type":"tool","callID":"child_call_edit","tool":"edit","state":{"status":"completed","input":{"filePath":"src/counted.rs"},"title":"src/counted.rs","output":"1 addition"}}}}"#,
            r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-fetch","messageID":"child-message-2","sessionID":"ses_child_1","type":"tool","callID":"child_call_fetch","tool":"webfetch","state":{"status":"error","input":{"url":"https://example.invalid"},"error":"could not resolve host"}}}}"#,
            r#"data: {"type":"session.status","properties":{"sessionID":"ses_child_1","status":{"type":"retry","message":"Retrying the child's request"}}}"#,
            r#"data: {"type":"child.future.event","properties":{"sessionID":"ses_child_1","whatever":{"nested":true}}}"#,
        ] {
            sender
                .send(Ok(format!("{event}\n\n")))
                .await
                .expect("send scripted child work SSE event");
        }
    }

    // A blocker raised by the descendant rather than by the conversation: the
    // request carries the *child's* session id, which is the whole difference
    // and the whole of what has to be routed correctly.
    if let Some(blocker) = state.child_blocker {
        let event = if blocker == "legacy" {
            // The pre-`permission.asked` envelope: `type` rather than
            // `permission`, and answered on a *session-scoped* route.
            r#"data: {"type":"permission.updated","properties":{"id":"child-legacy-1","sessionID":"ses_child_1","type":"bash","patterns":["rm -rf build"],"metadata":{"command":"rm -rf build"},"tool":{"messageID":"child-message-2","callID":"child_call_rm"}}}"#
        } else if blocker == "question" {
            r#"data: {"type":"question.asked","properties":{"id":"child-que-1","sessionID":"ses_child_1","questions":[{"header":"Scope","question":"Count tests too?","options":[{"label":"Yes","description":"Include tests"},{"label":"No","description":"Source only"}],"multiple":false}]}}"#
        } else {
            r#"data: {"type":"permission.asked","properties":{"id":"child-per-1","sessionID":"ses_child_1","permission":"bash","patterns":["rm -rf build"],"metadata":{"command":"rm -rf build"},"always":[],"tool":{"messageID":"child-message-2","callID":"child_call_rm"}}}"#
        };
        sender
            .send(Ok(format!("{event}\n\n")))
            .await
            .expect("send scripted child blocker SSE event");
    }

    let gated = state.idle_release.is_some();
    let finish = async move {
        if let Some(release) = &state.idle_release {
            release.notified().await;
        }
        // The same call, finishing. Behind the gate on purpose: a subscription
        // opened while it was `running` has the entry already, so what arrives
        // here has to be an *in-place* update of it — which is the only way a
        // live reader learns a tool call succeeded.
        if state.child_work {
            sender
                .send(Ok(concat!(
                    r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-bash","messageID":"child-message-2","sessionID":"ses_child_1","type":"tool","callID":"child_call_bash","tool":"bash","state":{"status":"completed","input":{"command":"ls -1 src | wc -l"},"title":"ls -1 src | wc -l","output":"11"}}}}"#,
                    "\n\n"
                ).to_string()))
                .await
                .expect("send scripted child work SSE event");
        }
        if state.subagent {
            for event in [
                r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-3","messageID":"child-message-2","sessionID":"ses_child_1","type":"text","text":"eleven so far"}}}"#,
                r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_owned_1","part":{"id":"prt-task","messageID":"message-1","sessionID":"ses_owned_1","type":"tool","callID":"call_task_1","tool":"task","state":{"status":"completed","output":"eleven files","input":{"description":"Count the files","subagent_type":"explore"},"metadata":{"parentSessionId":"ses_owned_1","sessionId":"ses_child_1"}}}}}"#,
                // After the answer. Must not reopen the row or the stream.
                r#"data: {"type":"message.part.updated","properties":{"sessionID":"ses_child_1","part":{"id":"child-prt-4","messageID":"child-message-2","sessionID":"ses_child_1","type":"text","text":"anything else?"}}}"#,
            ] {
                sender
                    .send(Ok(format!("{event}\n\n")))
                    .await
                    .expect("send scripted subagent SSE event");
            }
        }
        for event in [
            "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"idle\"}}}\n\n",
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n",
            "data: {\"type\":\"session.error\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"error\":{\"name\":\"AbortError\",\"message\":\"request aborted\"}}}\n\n",
        ] {
            sender.send(Ok(event.to_string())).await.expect("send scripted SSE event");
        }
    };
    if gated {
        tokio::spawn(finish);
    } else {
        finish.await;
    }
    StatusCode::NO_CONTENT
}

async fn abort(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    headers: HeaderMap,
) -> Json<Value> {
    assert_authorization(&headers, &state);
    append(
        &state.log,
        json!({"operation":"abort","sessionId":session_id}),
    );
    // Logged first, so a test can prove laplus *asked* and then prove what it
    // did about never being answered.
    if state.unanswered_abort {
        std::future::pending::<()>().await;
    }
    Json(json!(true))
}

async fn reply_permission(
    AxumPath(request_id): AxumPath<String>,
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    append(
        &state.log,
        json!({"operation":"permission.reply","requestId":request_id,"body":body}),
    );
    let sender = state.subscriber.lock().unwrap().clone().unwrap();
    // A descendant's permission is replied to on the *child's* session, and the
    // conversation does not go idle because of it: the child carries on working
    // and its `task` call is still in flight.
    if state.child_reply_fails {
        // Read and refused: OpenCode took the decision and would not accept it,
        // so nothing will ever tell the child.
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if state.child_blocker == Some("permission") {
        sender.send(Ok(format!(
            "data: {{\"type\":\"permission.replied\",\"properties\":{{\"sessionID\":\"ses_child_1\",\"requestID\":\"{request_id}\",\"reply\":{}}}}}\n\n",
            body["reply"]
        ))).await.unwrap();
        return Ok(Json(json!(true)));
    }
    for event in [
        "data: {\"id\":\"evt-reply-1\",\"type\":\"permission.replied\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"requestID\":\"per-1\",\"reply\":\"once\"}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"part\":{\"type\":\"tool\",\"callID\":\"call-bash\",\"tool\":\"Bash\",\"state\":{\"status\":\"error\",\"input\":{},\"error\":\"denied\",\"time\":{\"start\":1,\"end\":2}}}}\n\n",
        "data: {\"id\":\"evt-tool-2\",\"type\":\"message.part.updated\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"part\":{\"id\":\"part-tool-1\",\"sessionID\":\"ses_owned_1\",\"messageID\":\"message-1\",\"type\":\"tool\",\"callID\":\"call-1\",\"tool\":\"mystery\",\"state\":{\"status\":\"completed\",\"input\":{\"secret\":42},\"output\":\"done\",\"title\":\"Mystery\",\"metadata\":{},\"time\":{\"start\":1,\"end\":2}}}}}\n\n",
        "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n",
    ] { sender.send(Ok(event.to_string())).await.unwrap(); }
    Ok(Json(json!(true)))
}

async fn reply_legacy_permission(
    AxumPath((session_id, request_id)): AxumPath<(String, String)>,
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    append(&state.log, json!({"operation":"permission.reply.legacy","sessionId":session_id,"requestId":request_id,"body":body}));
    let sender = state.subscriber.lock().unwrap().clone().unwrap();
    if state.child_blocker == Some("legacy") {
        sender.send(Ok(format!(
            "data: {{\"type\":\"permission.replied\",\"properties\":{{\"sessionID\":\"ses_child_1\",\"requestID\":\"{request_id}\",\"reply\":{}}}}}\n\n",
            body["response"]
        ))).await.unwrap();
        return Json(json!(true));
    }
    sender.send(Ok("data: {\"type\":\"permission.replied\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"permissionID\":\"legacy-1\",\"response\":\"once\"}}\n\n".to_string())).await.unwrap();
    Json(json!(true))
}

async fn reply_question(AxumPath(request_id): AxumPath<String>, State(state): State<PeerState>, Json(body): Json<Value>) -> Json<Value> {
    append(&state.log, json!({"operation":"question.reply","requestId":request_id,"body":body}));
    let sender = state.subscriber.lock().unwrap().clone().unwrap();
    // See `reply_permission`: a descendant's question resolves on the child's
    // session, and does not settle the conversation.
    if state.child_blocker == Some("question") {
        sender.send(Ok(format!("data: {{\"type\":\"question.replied\",\"properties\":{{\"sessionID\":\"ses_child_1\",\"requestID\":\"{request_id}\",\"answers\":{}}}}}\n\n", body["answers"]))).await.unwrap();
        return Json(json!(true));
    }
    sender.send(Ok(format!("data: {{\"type\":\"question.replied\",\"properties\":{{\"sessionID\":\"ses_owned_1\",\"requestID\":\"{request_id}\",\"answers\":{}}}}}\n\n", body["answers"]))).await.unwrap();
    sender.send(Ok("data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n".to_string())).await.unwrap();
    Json(json!(true))
}

async fn reject_question(AxumPath(request_id): AxumPath<String>, State(state): State<PeerState>) -> Json<Value> {
    append(&state.log, json!({"operation":"question.reject","requestId":request_id}));
    let sender = state.subscriber.lock().unwrap().clone().unwrap();
    sender.send(Ok(format!("data: {{\"type\":\"question.rejected\",\"properties\":{{\"sessionID\":\"ses_owned_1\",\"requestID\":\"{request_id}\"}}}}\n\n"))).await.unwrap();
    sender.send(Ok("data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n".to_string())).await.unwrap();
    Json(json!(true))
}

struct ExternalOpenCode {
    _directory: tempfile::TempDir,
    endpoint: String,
    log: PathBuf,
    task: tokio::task::JoinHandle<()>,
    prompts: Arc<AtomicUsize>,
}

impl ExternalOpenCode {
    async fn start(password: Option<&str>) -> Self {
        Self::start_with_idle_release(password, None).await
    }

    async fn with_permissions() -> Self {
        Self::start_configured(None, None, true).await
    }

    async fn with_questions() -> Self {
        Self::start_configured_with_rollback(
            None, None, false, ResumeBehavior::Normal, None, false, true, false, false,
        )
        .await
    }

    async fn for_resume(resume: ResumeBehavior) -> Self {
        Self::start_configured_with_resume(None, None, false, resume).await
    }

    async fn for_rollback(probe: PathBuf, fails: bool) -> Self {
        Self::start_configured_with_rollback(
            None,
            None,
            false,
            ResumeBehavior::Normal,
            Some(probe),
            fails,
            false,
            false,
            false,
        )
        .await
    }

    async fn start_with_idle_release(
        password: Option<&str>,
        idle_release: Option<Arc<Notify>>,
    ) -> Self {
        Self::start_configured(password, idle_release, false).await
    }

    async fn with_reconciliation_failure(idle_release: Arc<Notify>) -> Self {
        Self::start_configured_with_rollback(
            None,
            Some(idle_release),
            false,
            ResumeBehavior::Normal,
            None,
            false,
            false,
            true,
            false,
        )
        .await
    }

    async fn with_followup_delivery_failure(idle_release: Arc<Notify>) -> Self {
        Self::start_configured_with_rollback(
            None,
            Some(idle_release),
            false,
            ResumeBehavior::Normal,
            None,
            false,
            false,
            false,
            true,
        )
        .await
    }

    async fn start_configured(
        password: Option<&str>,
        idle_release: Option<Arc<Notify>>,
        permissions: bool,
    ) -> Self {
        Self::start_configured_with_resume(
            password,
            idle_release,
            permissions,
            ResumeBehavior::Normal,
        )
        .await
    }

    async fn start_configured_with_resume(
        password: Option<&str>,
        idle_release: Option<Arc<Notify>>,
        permissions: bool,
        resume: ResumeBehavior,
    ) -> Self {
        Self::start_configured_with_rollback(
            password,
            idle_release,
            permissions,
            resume,
            None,
            false,
            false,
            false,
            false,
        )
        .await
    }

    async fn start_configured_with_rollback(
        password: Option<&str>,
        idle_release: Option<Arc<Notify>>,
        permissions: bool,
        resume: ResumeBehavior,
        rollback_probe: Option<PathBuf>,
        rollback_fails: bool,
        questions: bool,
        fail_reconciliation: bool,
        fail_followup_prompt: bool,
    ) -> Self {
        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            authorization: password.map(|password| match password {
                "external-secret-that-must-stay-private" => {
                    "Basic b3BlbmNvZGU6ZXh0ZXJuYWwtc2VjcmV0LXRoYXQtbXVzdC1zdGF5LXByaXZhdGU="
                        .to_string()
                }
                _ => panic!("test password needs an explicit wire expectation"),
            }),
            idle_release,
            permissions,
            questions,
            resume,
            rollback_probe,
            rollback_fails,
            fail_reconciliation,
            fail_followup_prompt,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    /// A peer that delegates a subagent and stops half way through the child's
    /// work until the test releases it.
    ///
    /// In-process rather than the spawned [`FakeOpenCode`] because the gate has
    /// to be held by the test: what is being driven is a client opening a
    /// child's work stream while the child is still working.
    async fn spawning_a_subagent(release: Arc<Notify>) -> Self {
        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            idle_release: Some(release),
            subagent: true,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    /// The same subagent, doing everything OpenCode exposes about a child's
    /// work rather than only talking.
    async fn spawning_a_working_subagent(release: Arc<Notify>) -> Self {
        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            idle_release: Some(release),
            subagent: true,
            child_work: true,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    /// A subagent that stops for the developer: `"permission"`, `"question"`, or
    /// `"legacy"`.
    async fn spawning_a_blocked_subagent(release: Arc<Notify>, blocker: &'static str) -> Self {
        Self::blocked_subagent(release, blocker, false).await
    }

    /// The same, with a reply route that refuses the developer's decision.
    async fn refusing_the_decision(release: Arc<Notify>) -> Self {
        Self::blocked_subagent(release, "permission", true).await
    }

    async fn blocked_subagent(
        release: Arc<Notify>,
        blocker: &'static str,
        child_reply_fails: bool,
    ) -> Self {
        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            idle_release: Some(release),
            subagent: true,
            child_blocker: Some(blocker),
            child_reply_fails,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    /// A turn narrated as several text parts around a tool call, held before
    /// its second part completes until the gate is released.
    async fn narrating_in_text_parts(idle_release: Option<Arc<Notify>>) -> Self {
        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            idle_release,
            text_parts: true,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    /// The same interleaved narration, streamed only as far as the second
    /// block's first delta — and a provider history that holds the rest of the
    /// turn the stream never delivered.
    ///
    /// What a developer meets when the event stream dies mid-turn and the stop
    /// that follows has to recover the transcript from `session.messages`
    /// alone. The gate is never released, so nothing but reconciliation can
    /// account for anything beyond the delta the stream stopped on.
    async fn narrating_past_a_lost_suffix(idle_release: Arc<Notify>) -> Self {
        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            idle_release: Some(idle_release),
            text_parts: true,
            lost_suffix: true,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    async fn changing_output_during_stop(idle_release: Arc<Notify>) -> Self {
        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            idle_release: Some(idle_release),
            output_changes_during_stop: true,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    /// An external peer whose `abort` is accepted and never answered.
    async fn with_unanswered_abort(idle_release: Arc<Notify>) -> Self {        let directory = tempfile::tempdir().expect("external peer directory");
        let log = directory.path().join("requests.jsonl");
        let state = PeerState {
            log: Arc::new(log.clone()),
            healthy: true,
            idle_release: Some(idle_release),
            unanswered_abort: true,
            ..Default::default()
        };
        Self::serving(directory, log, state).await
    }

    /// The routes and the listener, once. Every constructor above differs only
    /// in the [`PeerState`] it hands over.
    async fn serving(directory: tempfile::TempDir, log: PathBuf, state: PeerState) -> Self {
        let prompts = state.prompts.clone();
        let app = Router::new()
            .route("/global/health", get(health))
            .route("/provider", get(providers))
            .route("/config", get(opencode_config_snapshot))
            .route("/event", get(events))
            .route("/session", post(create_session))
            .route("/session/{id}", get(get_session).patch(update_session))
            .route("/session/{id}/fork", post(fork_session))
            .route("/session/{id}/message", get(session_messages))
            .route("/session/status", get(session_statuses))
            .route("/session/{id}/revert", post(revert_session))
            .route("/experimental/control-plane/move-session", post(move_session))
            .route("/session/{id}/prompt_async", post(prompt))
            .route("/session/{id}/abort", post(abort))
            .route("/permission/{id}/reply", post(reply_permission))
            .route("/session/{id}/permissions/{permission_id}", post(reply_legacy_permission))
            .route("/question/{id}/reply", post(reply_question))
            .route("/question/{id}/reject", post(reject_question))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            _directory: directory,
            endpoint,
            log,
            task,
            prompts,
        }
    }

    fn config(&self, password: Option<&str>) -> ServerConfig {
        let mut config = ServerConfig::detect();
        config.settings.provider_instances.insert(
            "openExternal".into(),
            json!({"driver":"opencode","displayName":"OpenCode External","config":{
                "binaryPath":"this-binary-must-never-be-started","serverUrl":self.endpoint,
                "serverPassword":password.unwrap_or_default(),"customModels":["openai/gpt-5", "openai/gpt-alt"]
            }}),
        );
        config
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests_through(2).await
    }

    fn reset_prompts(&self) {
        // A restarted Laplus process is a fresh driver even though this
        // operator-owned scripted peer deliberately remains alive.
        // The peer's first-prompt script should therefore run again.
        self.prompts.store(0, Ordering::SeqCst);
    }

    async fn requests_through(&self, count: usize) -> Vec<Value> {
        self.requests_through_within(count, std::time::Duration::from_secs(5)).await
    }

    async fn requests_through_within(
        &self,
        count: usize,
        bound: std::time::Duration,
    ) -> Vec<Value> {
        tokio::time::timeout(bound, async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&self.log) {
                    let values = contents
                        .lines()
                        .map(|line| serde_json::from_str(line).unwrap())
                        .collect::<Vec<Value>>();
                    if values.len() >= count {
                        return values;
                    }
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("external peer receives requests")
    }
}

async fn start_question_turn(peer: &ExternalOpenCode, suffix: &str) -> (Workspace, TestServer, SocketClient, String) {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    let project = format!("question-project-{suffix}");
    let thread = format!("question-thread-{suffix}");
    client.call("orchestration.dispatchCommand", create_project(&project, workspace.path())).await.expect_success();
    let mut create = create_thread(&project, &thread);
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation(&thread).await;
    let mut command = start_turn(&thread, &format!("message-{suffix}"), "ask me");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", command).await.expect_success();
    (workspace, server, client, subscription)
}

#[tokio::test]
async fn opencode_questions_preserve_order_ids_and_resolve_only_after_the_reply_event() {
    let peer = ExternalOpenCode::with_questions().await;
    let (_workspace, server, mut client, subscription) = start_question_turn(&peer, "answer").await;
    let (asked, request_id) = client.events_until_user_input(&subscription).await;
    let questions = &activity(&asked, "user-input.requested")["payload"]["activity"]["payload"]["questions"];
    assert_eq!(questions[0]["id"], "question-0-database-choice");
    assert_eq!(questions[1]["id"], "question-1-features");
    assert_eq!(questions[1]["multiSelect"], true);
    assert_eq!(questions[0]["options"][0]["label"], "SQLite");
    client.call("orchestration.dispatchCommand", respond_to_user_input("question-thread-answer", &request_id, json!({"question-1-features":["Sync","Search"],"question-0-database-choice":["Postgres"]}))).await.expect_success();
    let settled = client.events_through_the_turn(&subscription).await;
    assert!(settled.iter().any(|item| item["event"]["payload"]["activity"]["kind"] == "user-input.resolved"));
    let requests = peer.requests_through(3).await;
    let reply = requests.iter().find(|request| request["operation"] == "question.reply").unwrap();
    assert_eq!(reply["body"]["answers"], json!([["Postgres"],["Sync","Search"]]));
    server.stop().await; peer.task.abort();
}

#[tokio::test]
async fn rejecting_an_opencode_question_uses_the_distinct_route_and_event() {
    let peer = ExternalOpenCode::with_questions().await;
    let (_workspace, server, mut client, subscription) = start_question_turn(&peer, "reject").await;
    let (_, request_id) = client.events_until_user_input(&subscription).await;
    client.call("orchestration.dispatchCommand", json!({"type":"thread.user-input.reject","commandId":"test:reject-question","threadId":"question-thread-reject","requestId":request_id,"createdAt":"2026-08-02T00:00:00.000Z"})).await.expect_success();
    let settled = client.events_through_the_turn(&subscription).await;
    assert!(settled.iter().any(|item| item["event"]["payload"]["activity"]["kind"] == "user-input.resolved"));
    let requests = peer.requests_through(3).await;
    assert!(requests.iter().any(|request| request["operation"] == "question.reject"));
    server.stop().await; peer.task.abort();
}

#[tokio::test]
async fn opencode_prompt_resolves_stored_attachments_and_omits_missing_references() {
    let peer = ExternalOpenCode::start(None).await;
    let workspace = Workspace::with(&["src/"]);
    let preferences = tempfile::tempdir().expect("persistent test preferences");
    let server = TestServer::start_persistent_with_config_in(preferences.path(), peer.config(None)).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("attachment-project", workspace.path())).await.expect_success();
    let mut create = create_thread("attachment-project", "attachment-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation("attachment-thread").await;
    let mut invalid = start_turn("attachment-thread", "invalid-attachment-message", "must not commit");
    invalid["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    invalid["message"]["attachments"] = json!([{"type":"image","name":"vector.svg","mimeType":"image/svg+xml","sizeBytes":2,"dataUrl":"data:image/svg+xml;base64,aGk="}]);
    client.call("orchestration.dispatchCommand", invalid).await.expect_declared("OrchestrationDispatchCommandError");
    let after_refusal = server.connect().await.into_thread_snapshot("attachment-thread").await;
    assert!(after_refusal["thread"]["messages"].as_array().unwrap().is_empty(), "refusal committed no user message");
    let mut command = start_turn("attachment-thread", "attachment-message", "inspect");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    command["message"]["attachments"] = json!([
        {"type":"image","name":"screen.png","mimeType":"image/png","sizeBytes":2,"dataUrl":"data:image/png;base64,aGk="},
        {"type":"image","id":"image-missing","name":"missing.png","mimeType":"image/png","sizeBytes":2}
    ]);
    client.call("orchestration.dispatchCommand", command).await.expect_success();
    let events = client.events_through_the_turn(&subscription).await;
    let sent = events.iter().find(|item| item["event"]["type"] == "thread.message-sent").expect("durable user-message event");
    assert_eq!(sent["event"]["payload"]["attachments"], json!([{"type":"image","id":"attachment-message-0","name":"screen.png","mimeType":"image/png","sizeBytes":2}]));
    assert!(!sent.to_string().contains("data:image/png"), "inline upload data is not durable");
    let requests = peer.requests_through(2).await;
    let prompt = requests.iter().find(|request| request["operation"] == "prompt").unwrap();
    assert_eq!(prompt["body"]["parts"][0], json!({"type":"text","text":"inspect"}));
    assert_eq!(prompt["body"]["parts"][1]["type"], "file");
    assert_eq!(prompt["body"]["parts"][1]["mime"], "image/png");
    assert_eq!(prompt["body"]["parts"][1]["filename"], "screen.png");
    assert!(prompt["body"]["parts"][1]["url"].as_str().unwrap().starts_with("file://"));
    assert_eq!(prompt["body"]["parts"].as_array().unwrap().len(), 2, "missing references are omitted independently");
    let snapshot = server.connect().await.into_thread_snapshot("attachment-thread").await;
    assert_eq!(snapshot["thread"]["messages"][0]["attachments"], sent["event"]["payload"]["attachments"]);
    let issued = client.call("assets.createUrl", json!({"resource":{"_tag":"attachment","attachmentId":"attachment-message-0"}})).await.expect_success();
    assert_eq!(server.get(issued["relativeUrl"].as_str().unwrap()).await.text, "hi");
    client.close().await;
    server.stop().await;

    let restarted = TestServer::start_persistent_with_config_in(preferences.path(), peer.config(None)).await;
    let client = restarted.connect().await;
    let snapshot = client.into_thread_snapshot("attachment-thread").await;
    assert_eq!(snapshot["thread"]["messages"][0]["attachments"], json!([{"type":"image","id":"attachment-message-0","name":"screen.png","mimeType":"image/png","sizeBytes":2}]));
    let mut client = restarted.connect().await;
    let issued = client.call("assets.createUrl", json!({"resource":{"_tag":"attachment","attachmentId":"attachment-message-0"}})).await.expect_success();
    assert_eq!(restarted.get(issued["relativeUrl"].as_str().unwrap()).await.text, "hi");
    restarted.stop().await; peer.task.abort();
}

#[tokio::test]
async fn interrupting_opencode_aborts_and_keeps_partial_output_despite_duplicate_idle() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(Arc::clone(&idle_release))).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("interrupt-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("interrupt-project", "interrupt-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("interrupt-thread").await;
    let mut command = start_turn("interrupt-thread", "interrupt-message", "begin");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    let before = client.events_until_streaming(&subscription).await;
    let turn_id = last_session(&before, "running OpenCode turn")["payload"]["session"]
        ["activeTurnId"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("interrupt-thread", Some(&turn_id)),
        )
        .await
        .expect_success();
    idle_release.notify_one();
    let after = client.events_through_the_turn(&subscription).await;
    let requests = peer.requests_through(3).await;
    assert!(requests
        .iter()
        .any(|request| request["operation"] == "abort"));
    assert_eq!(
        after
            .iter()
            .filter(|item| item["event"]["payload"]["activity"]["kind"] == "turn.completed")
            .count(),
        1
    );
    // The peer queued a duplicate idle and then a late abort error after the
    // first idle. Give the session loop a chance to consume both before reading
    // the durable result; neither may revise the developer-owned interruption.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("interrupt-thread")
        .await;
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "interrupted");
    assert_eq!(snapshot["thread"]["session"]["status"], "interrupted");
    assert_eq!(snapshot["thread"]["session"]["lastError"], Value::Null);
    assert!(snapshot["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "assistant"
            && message["text"].as_str().unwrap_or("").contains("hello")));
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn missing_opencode_idle_reconciles_and_late_idle_cannot_settle_queued_work() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(Arc::clone(&release))).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("reconcile-project", workspace.path())).await.expect_success();
    let mut create = create_thread("reconcile-project", "reconcile-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation("reconcile-thread").await;
    let mut first = start_turn("reconcile-thread", "message-a", "A");
    first["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", first).await.expect_success();
    let events = client.events_until_streaming(&subscription).await;
    let a = last_session(&events, "running A")["payload"]["session"]["activeTurnId"].as_str().unwrap().to_string();
    client.call("orchestration.dispatchCommand", interrupt_turn("reconcile-thread", Some(&a))).await.expect_success();
    client.call("orchestration.dispatchCommand", follow_up("reconcile-thread", "message-b", "B")).await.expect_success();
    client.call("orchestration.dispatchCommand", follow_up("reconcile-thread", "message-c", "C")).await.expect_success();
    let before = server.connect().await.into_thread_snapshot("reconcile-thread").await;
    let b = before["thread"]["messages"].as_array().unwrap().iter().find(|message| message["id"] == "message-b").unwrap()["turnId"].as_str().unwrap().to_string();
    let c = before["thread"]["messages"].as_array().unwrap().iter().find(|message| message["id"] == "message-c").unwrap()["turnId"].as_str().unwrap().to_string();
    assert_ne!(a, b);
    assert_eq!(b, c);
    let requests = peer
        .requests_through_within(7, std::time::Duration::from_secs(10))
        .await;
    assert!(requests.iter().any(|request| request["operation"] == "messages"));
    let queued_prompt = requests.iter().filter(|request| request["operation"] == "prompt").last().unwrap();
    let queued_text = queued_prompt["body"]["parts"].as_array().unwrap().iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n");
    assert!(queued_text.find('B') < queued_text.find('C'));
    release.notify_one();
    for _ in 0..10 { tokio::task::yield_now().await; }
    let after = server.connect().await.into_thread_snapshot("reconcile-thread").await;
    assert_eq!(after["thread"]["session"]["activeTurnId"], b);
    assert_eq!(after["thread"]["session"]["status"], "running");
    client.close().await; server.stop().await; peer.task.abort();
}

#[tokio::test]
async fn a_busy_opencode_prompt_is_durable_and_starts_after_the_active_turn_settles() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(Arc::clone(&idle_release))).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("steer-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("steer-project", "steer-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("steer-thread").await;
    let mut first = start_turn("steer-thread", "message-1", "begin");
    first["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", first)
        .await
        .expect_success();
    let before = client.events_until_streaming(&subscription).await;
    let active = last_session(&before, "busy OpenCode turn")["payload"]["session"]["activeTurnId"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("steer-thread", "message-2", "change course"),
        )
        .await
        .expect_success();
    tokio::task::yield_now().await;
    let requests = peer.requests().await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["operation"] == "prompt")
            .count(),
        1,
        "the queued message was sent to the busy OpenCode session"
    );
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("steer-thread")
        .await;
    let queued = snapshot["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == "message-2")
        .unwrap();
    assert_ne!(queued["turnId"], active);
    idle_release.notify_one();
    let later = client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.turn-start-requested"
        })
        .await;
    let new_turn = later
        .iter()
        .find(|item| item["event"]["type"] == "thread.turn-start-requested")
        .unwrap();
    assert_ne!(new_turn["event"]["payload"]["turnId"], active);
    let requests = peer.requests_through(3).await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["operation"] == "prompt")
            .count(),
        2
    );
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn busy_opencode_messages_stay_separate_but_start_one_queued_turn_in_order() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(Arc::clone(&idle_release))).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("queue-project", workspace.path())).await.expect_success();
    let mut create = create_thread("queue-project", "queue-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation("queue-thread").await;
    let mut first = start_turn("queue-thread", "message-1", "A");
    first["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", first).await.expect_success();
    client.events_until_streaming(&subscription).await;
    let mut second = follow_up_in("queue-thread", "message-2", "B", "approval-required");
    second["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-alt"});
    second["message"]["attachments"] = json!([{
        "type":"image", "name":"b.png", "mimeType":"image/png",
        "sizeBytes":2, "dataUrl":"data:image/png;base64,aGk="
    }]);
    client.call("orchestration.dispatchCommand", second).await.expect_success();
    let mut third = follow_up_in("queue-thread", "message-3", "C", "full-access");
    third["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    third["message"]["attachments"] = json!([{
        "type":"image", "name":"c.gif", "mimeType":"image/gif",
        "sizeBytes":3, "dataUrl":"data:image/gif;base64,dHdv"
    }]);
    client.call("orchestration.dispatchCommand", third).await.expect_success();
    let snapshot = server.connect().await.into_thread_snapshot("queue-thread").await;
    let queued = snapshot["thread"]["messages"].as_array().unwrap().iter()
        .filter(|message| message["id"] == "message-2" || message["id"] == "message-3")
        .collect::<Vec<_>>();
    assert_eq!(queued.len(), 2);
    assert_eq!(queued[0]["text"], "B");
    assert_eq!(queued[1]["text"], "C");
    assert_eq!(queued[0]["turnId"], queued[1]["turnId"]);
    assert_eq!(queued[0]["attachments"], json!([{"type":"image","id":"message-2-0","name":"b.png","mimeType":"image/png","sizeBytes":2}]));
    assert_eq!(queued[1]["attachments"], json!([{"type":"image","id":"message-3-0","name":"c.gif","mimeType":"image/gif","sizeBytes":3}]));
    let messages = snapshot["thread"]["messages"].as_array().unwrap();
    let reply = messages.iter().position(|message| {
        message["role"] == "assistant"
            && message["text"].as_str().unwrap_or("").contains("hello")
    }).expect("A's completed reply is durable before the queued messages");
    let first_queued = messages.iter().position(|message| message["id"] == "message-2").unwrap();
    assert!(reply < first_queued);
    idle_release.notify_one();
    let requests = peer.requests_through(4).await;
    let prompts = requests.iter().filter(|request| request["operation"] == "prompt").collect::<Vec<_>>();
    assert_eq!(prompts.len(), 2, "B and C opened more than one queued turn");
    assert_eq!(prompts[0]["sessionId"], prompts[1]["sessionId"], "the queued turn lost A's provider history");
    assert_eq!(prompts[1]["body"]["model"], json!({"providerID":"openai","modelID":"gpt-alt"}));
    let parts = prompts[1]["body"]["parts"].as_array().unwrap();
    assert_eq!(part_labels(parts), vec![("text", "B"), ("file", "b.png"), ("text", "C"), ("file", "c.gif")]);
    assert!(requests.iter().any(|request| request["operation"] == "update"), "B's captured approval-required mode was not applied");
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn stopping_busy_opencode_aborts_before_releasing_the_external_session() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(idle_release)).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("stop-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("stop-project", "stop-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("stop-thread").await;
    let mut command = start_turn("stop-thread", "stop-message", "begin");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    client.events_until_streaming(&subscription).await;
    client.call("orchestration.dispatchCommand", json!({"type":"thread.session.stop","commandId":"test:stop:busy-opencode","threadId":"stop-thread","createdAt":"2026-08-01T00:00:00.000Z"})).await.expect_success();
    let requests = peer.requests_through(3).await;
    assert!(requests
        .iter()
        .any(|request| request["operation"] == "abort"));
    assert!(
        tokio::net::TcpStream::connect(peer.endpoint.trim_start_matches("http://"))
            .await
            .is_ok(),
        "external endpoint remains operator-owned"
    );
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// An OpenCode that accepts the abort and never answers it.
///
/// This is the ordinary failure of an OpenAI-compatible proxy rather than an
/// exotic one: the provider stream stalls with no `chunkTimeout` above it
/// (opencode#37580), and the server that is blocked draining it stops answering
/// its own HTTP port. The socket is open and the request is read, so nothing
/// below the transport ever errors.
///
/// `reqwest::Client::new()` has no request timeout, and the session loop awaits
/// [`Driver::interrupt`] *before* its `select!` — so an unanswered abort used to
/// stop the loop reading its own signals. Stop did nothing, no event was
/// normalized, and the conversation showed Working for as long as the process
/// lived. The same unbounded await sat in front of the reap in `Driver::stop`,
/// which is how one machine accumulated 64 orphaned `opencode serve` processes
/// over three days.
///
/// What is asserted is that the loop keeps reading, not how long it took to get
/// there: the refusal is reported, and a session stop sent afterwards is still
/// heard. Neither is reachable if the interrupt never returns.
#[tokio::test]
async fn an_unanswered_abort_is_reported_and_leaves_the_session_loop_reading() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::with_unanswered_abort(Arc::clone(&idle_release)).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("deaf-abort-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("deaf-abort-project", "deaf-abort-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("deaf-abort-thread").await;
    let mut command = start_turn("deaf-abort-thread", "deaf-abort-message", "begin");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    let before = client.events_until_streaming(&subscription).await;
    let turn_id = last_session(&before, "running OpenCode turn")["payload"]["session"]
        ["activeTurnId"]
        .as_str()
        .unwrap()
        .to_string();

    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("deaf-abort-thread", Some(&turn_id)),
        )
        .await
        .expect_success();

    // The peer logged the abort, so laplus asked. This row is what it did about
    // never being answered — and it cannot be published by a loop still waiting.
    // Three: the session create, the prompt, and the abort. Waiting for the
    // count is what makes this the abort rather than whichever two had already
    // arrived.
    let requests = peer.requests_through(3).await;
    assert!(
        requests
            .iter()
            .any(|request| request["operation"] == "abort"),
        "laplus never asked OpenCode to stop: {requests:#?}"
    );
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "turn.interrupt-failed"
        })
        .await;

    // The claim that matters. A loop parked in an unbounded request reads no
    // further signal, so a stop arriving afterwards is the proof it recovered.
    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type":"thread.session.stop",
                "commandId":"test:stop:deaf-abort",
                "threadId":"deaf-abort-thread",
                "createdAt":"2026-08-17T00:00:00.000Z"
            }),
        )
        .await
        .expect_success();
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("deaf-abort-thread")
        .await;
    assert_ne!(
        snapshot["thread"]["session"]["status"], "running",
        "the conversation is still showing work nothing is doing: {:#?}",
        snapshot["thread"]["session"]
    );

    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn failed_queued_delivery_keeps_the_message_retryable() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::with_followup_delivery_failure(Arc::clone(&release)).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("delivery-failure-project", workspace.path())).await.expect_success();
    let mut create = create_thread("delivery-failure-project", "delivery-failure-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation("delivery-failure-thread").await;
    let mut a = start_turn("delivery-failure-thread", "delivery-failure-a", "A");
    a["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", a).await.expect_success();
    client.events_until_streaming(&subscription).await;
    let mut b = follow_up("delivery-failure-thread", "delivery-failure-b", "B");
    b["message"]["attachments"] = json!([{
        "type":"image", "name":"retry.png", "mimeType":"image/png",
        "sizeBytes":2, "dataUrl":"data:image/png;base64,aGk="
    }]);
    client.call("orchestration.dispatchCommand", b).await.expect_success();
    release.notify_one();
    client.values_until(&subscription, |item| item["event"]["payload"]["deliveryState"] == "retryable").await;
    let snapshot = server.connect().await.into_thread_snapshot("delivery-failure-thread").await;
    let message = snapshot["thread"]["messages"].as_array().unwrap().iter().find(|message| message["id"] == "delivery-failure-b").unwrap();
    assert_eq!(message["deliveryState"], "retryable");
    assert_eq!(message["attachments"], json!([{"type":"image","id":"delivery-failure-b-0","name":"retry.png","mimeType":"image/png","sizeBytes":2}]));
    assert_ne!(snapshot["thread"]["session"]["status"], "running");
    let failed_parts = peer.requests().await.into_iter().filter(|request| request["operation"] == "prompt").next_back().unwrap()["body"]["parts"].clone();
    client.call("orchestration.dispatchCommand", json!({"type":"thread.turn.retry","commandId":"test:retry:delivery-failure","threadId":"delivery-failure-thread","createdAt":"2026-08-17T00:00:01.000Z"})).await.expect_success();
    client.values_until(&subscription, |item| item["event"]["payload"]["deliveryState"] == "retryable").await;
    let retried_parts = peer.requests().await.into_iter().filter(|request| request["operation"] == "prompt").next_back().unwrap()["body"]["parts"].clone();
    assert_eq!(retried_parts, failed_parts, "retry changed the original text/image request");
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn failed_interrupt_reconciliation_is_reported_once_and_later_turns_still_run() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::with_reconciliation_failure(Arc::clone(&release)).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("reconcile-failure-project", workspace.path())).await.expect_success();
    let mut create = create_thread("reconcile-failure-project", "reconcile-failure-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation("reconcile-failure-thread").await;
    let mut a = start_turn("reconcile-failure-thread", "reconcile-failure-a", "A");
    a["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", a).await.expect_success();
    let events = client.events_until_streaming(&subscription).await;
    let active = last_session(&events, "running before failed reconciliation")["payload"]["session"]["activeTurnId"].as_str().unwrap();
    client.call("orchestration.dispatchCommand", interrupt_turn("reconcile-failure-thread", Some(active))).await.expect_success();
    client.values_until(&subscription, |item| {
        item["event"]["payload"]["activity"]["kind"] == "turn.interrupt-verification-failed"
    }).await;
    let failed = server.connect().await.into_thread_snapshot("reconcile-failure-thread").await;
    assert_eq!(failed["thread"]["session"]["status"], "running");
    let failures = failed["thread"]["activities"].as_array().unwrap().iter().filter(|row| row["kind"] == "turn.interrupt-verification-failed").collect::<Vec<_>>();
    assert_eq!(failures.len(), 1);
    let diagnostic = failures[0].to_string();
    for expected in ["openExternal", "ses_owned_1", "verifying", "last message count unknown"] {
        assert!(diagnostic.contains(expected), "missing {expected:?}: {diagnostic}");
    }

    release.notify_one();
    client.events_through_the_turn(&subscription).await;
    // A history that never answers can never prove quiet, so supervision ends
    // at the escalation window instead of running forever: the stop degrades to
    // an honest interrupted turn, reported once and settled once.
    let abandoned = server.connect().await.into_thread_snapshot("reconcile-failure-thread").await;
    assert_eq!(abandoned["thread"]["latestTurn"]["state"], "interrupted");
    let rows = abandoned["thread"]["activities"].as_array().unwrap();
    assert_eq!(rows.iter().filter(|row| row["kind"] == "turn.interrupt-verification-failed").count(), 1);
    assert_eq!(rows.iter().filter(|row| row["kind"] == "turn.completed").count(), 1);
    let mut b = follow_up("reconcile-failure-thread", "reconcile-failure-b", "B");
    b["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", b).await.expect_success();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while peer.prompts.load(Ordering::SeqCst) < 2 { tokio::task::yield_now().await; }
    }).await.expect("a later turn reaches OpenCode after reconciliation failed");
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn stopped_queued_opencode_work_survives_restart_and_retries_once_in_order() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(release)).await;
    let data = tempfile::tempdir().unwrap();
    let workspace = Workspace::with(&["src/"]);
    let first = TestServer::start_persistent_with_config_in(data.path(), peer.config(None)).await;
    let mut client = first.connect().await;
    client.call("orchestration.dispatchCommand", create_project("unsent-project", workspace.path())).await.expect_success();
    let mut create = create_thread("unsent-project", "unsent-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let subscription = client.watch_conversation("unsent-thread").await;
    let mut a = start_turn("unsent-thread", "unsent-a", "A");
    a["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", a).await.expect_success();
    client.events_until_streaming(&subscription).await;
    let mut b = follow_up("unsent-thread", "unsent-b", "B");
    b["titleSeed"] = json!("preserved queued title seed");
    b["sourceProposedPlan"] = json!({"threadId":"source-thread","planId":"source-plan"});
    b["message"]["attachments"] = json!([{
        "type":"image", "name":"b.png", "mimeType":"image/png",
        "sizeBytes":2, "dataUrl":"data:image/png;base64,aGk="
    }]);
    client.call("orchestration.dispatchCommand", b).await.expect_success();
    let mut c = follow_up("unsent-thread", "unsent-c", "C");
    c["message"]["attachments"] = json!([{
        "type":"image", "name":"c.gif", "mimeType":"image/gif",
        "sizeBytes":3, "dataUrl":"data:image/gif;base64,dHdv"
    }]);
    client.call("orchestration.dispatchCommand", c).await.expect_success();
    client.call("orchestration.dispatchCommand", json!({"type":"thread.session.stop","commandId":"test:stop:unsent","threadId":"unsent-thread","createdAt":"2026-08-15T00:00:00.000Z"})).await.expect_success();
    client.values_until(&subscription, |item| item["event"]["payload"]["session"]["status"] == "stopped").await;
    let stopped = first.connect().await.into_thread_snapshot("unsent-thread").await;
    for id in ["unsent-b", "unsent-c"] {
        assert_eq!(stopped["thread"]["messages"].as_array().unwrap().iter().find(|message| message["id"] == id).unwrap()["deliveryState"], "retryable");
    }
    assert_eq!(peer.requests().await.iter().filter(|request| request["operation"] == "prompt").count(), 1);
    client.close().await;
    first.stop().await;

    let restarted = TestServer::start_persistent_with_config_in(data.path(), peer.config(None)).await;
    let snapshot = restarted.connect().await.into_thread_snapshot("unsent-thread").await;
    let messages = snapshot["thread"]["messages"].as_array().unwrap();
    let restored_b = messages.iter().find(|message| message["id"] == "unsent-b").unwrap();
    let restored_c = messages.iter().find(|message| message["id"] == "unsent-c").unwrap();
    assert_eq!(restored_b["deliveryState"], "retryable");
    assert_eq!(restored_b["attachments"], json!([{"type":"image","id":"unsent-b-0","name":"b.png","mimeType":"image/png","sizeBytes":2}]));
    assert_eq!(restored_c["attachments"], json!([{"type":"image","id":"unsent-c-0","name":"c.gif","mimeType":"image/gif","sizeBytes":3}]));
    assert_eq!(snapshot["thread"]["latestTurn"]["sourceProposedPlan"], json!({"threadId":"source-thread","planId":"source-plan"}));
    assert_eq!(peer.requests().await.iter().filter(|request| request["operation"] == "prompt").count(), 1, "restart submitted unsent work");
    let mut client = restarted.connect().await;
    for (attachment_id, expected) in [("unsent-b-0", "hi"), ("unsent-c-0", "two")] {
        let issued = client.call("assets.createUrl", json!({"resource":{"_tag":"attachment","attachmentId":attachment_id}})).await.expect_success();
        let fetched = restarted.get(issued["relativeUrl"].as_str().unwrap()).await;
        assert_eq!(fetched.status, 200);
        assert_eq!(fetched.text, expected);
    }
    client.call("orchestration.dispatchCommand", json!({"type":"thread.turn.retry","commandId":"test:retry:unsent","threadId":"unsent-thread","createdAt":"2026-08-15T00:00:01.000Z"})).await.expect_success();
    let requests = peer.requests_through(6).await;
    let retry = requests.iter().filter(|request| request["operation"] == "prompt").last().unwrap();
    let parts = retry["body"]["parts"].as_array().unwrap();
    assert_eq!(part_labels(parts), vec![("text", "B"), ("file", "b.png"), ("text", "C"), ("file", "c.gif")]);
    client.close().await;
    restarted.stop().await;
    peer.task.abort();
}

async fn assert_external_turn(password: Option<&str>) {
    let peer = ExternalOpenCode::start(password).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(password)).await;
    let mut client = server.connect().await;
    if password.is_some() {
        let mut instances = client
            .call("server.getSettings", json!({}))
            .await
            .expect_success()["providerInstances"]
            .clone();
        assert!(instances["openExternal"]["config"]
            .get("serverPassword")
            .is_none());
        instances["openExternal"]["config"]["customModels"] =
            json!(["openai/gpt-5", "openai/gpt-5-mini"]);
        let updated = client
            .call(
                "server.updateSettings",
                json!({"patch":{"providerInstances":instances}}),
            )
            .await
            .expect_success();
        assert!(!updated
            .to_string()
            .contains("external-secret-that-must-stay-private"));
    }
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("external-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("external-project", "external-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("external-thread").await;
    let mut command = start_turn("external-thread", "external-message", "say hello");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        assistant_sends(&events).last().unwrap().0,
        "hello from OpenCode"
    );
    assert_eq!(
        activity(&events, "context-window.updated")["payload"]["activity"]["payload"]
            ["maxTokens"],
        200_000,
        "an external endpoint uses the same authoritative catalogue limit"
    );
    let requests = peer.requests().await;
    assert_eq!(
        requests[0]["directory"],
        workspace.path().to_string_lossy().as_ref()
    );
    client.call("orchestration.dispatchCommand", json!({
        "type":"thread.session.stop","commandId":"test:stop:external","threadId":"external-thread",
        "createdAt":"2026-08-01T00:00:00.000Z"
    })).await.expect_success();
    assert!(
        tokio::net::TcpStream::connect(peer.endpoint.trim_start_matches("http://"))
            .await
            .is_ok(),
        "stopping a session must leave an operator-owned endpoint running"
    );
    client.close().await;
    server.stop().await;
    assert!(
        !peer.task.is_finished(),
        "server shutdown must not stop an operator-owned endpoint"
    );
    peer.task.abort();
}

#[tokio::test]
async fn an_unauthenticated_external_opencode_turn_crosses_the_socket_without_ownership() {
    assert_external_turn(None).await;
}

#[tokio::test]
async fn an_authenticated_external_opencode_turn_crosses_the_socket_without_exposing_its_password()
{
    assert_external_turn(Some("external-secret-that-must-stay-private")).await;
}

#[tokio::test]
async fn an_external_opencode_session_is_re_adopted_exactly_after_a_restart() {
    let peer = ExternalOpenCode::start(None).await;
    let workspace = Workspace::with(&["src/"]);
    let registry = tempfile::tempdir().unwrap();
    let database = registry.path().join("registry.sqlite");
    let config = peer.config(None);

    let first = TestServer::start_at_with_config(&database, config.clone()).await;
    let mut client = first.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("restart-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("restart-project", "restart-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let watch = client.watch_conversation("restart-thread").await;
    let mut first_turn = start_turn("restart-thread", "restart-message-1", "remember this");
    first_turn["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", first_turn)
        .await
        .expect_success();
    client.events_through_the_turn(&watch).await;
    client.close().await;
    first.stop().await;

    peer.reset_prompts();
    let second = TestServer::start_at_with_config(&database, config).await;
    let mut client = second.connect().await;
    let watch = client.watch_conversation("restart-thread").await;
    let mut second_turn = start_turn("restart-thread", "restart-message-2", "what did I say?");
    second_turn["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", second_turn)
        .await
        .expect_success();
    let events = client.events_through_the_turn(&watch).await;
    assert_eq!(assistant_sends(&events).last().unwrap().0, "hello from OpenCode");

    let requests = peer.requests_through(5).await;
    let operations = requests
        .iter()
        .map(|request| request["operation"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(operations, vec!["create", "prompt", "get", "update", "prompt"]);
    assert_eq!(requests[2]["sessionId"], "ses_owned_1");
    assert_eq!(requests[3]["body"]["permission"][0]["action"], "allow");

    client.close().await;
    second.stop().await;
    assert!(
        !peer.task.is_finished(),
        "restart must not take ownership of the external peer"
    );
    peer.task.abort();
}

async fn seed_external_opencode_thread(
    peer: &ExternalOpenCode,
    database: &Path,
    workspace: &Workspace,
) {
    let first = TestServer::start_at_with_config(database, peer.config(None)).await;
    let mut client = first.connect().await;
    client.call("orchestration.dispatchCommand", create_project("resume-project", workspace.path())).await.expect_success();
    let mut create = create_thread("resume-project", "resume-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let watch = client.watch_conversation("resume-thread").await;
    let mut turn = start_turn("resume-thread", "resume-message-1", "remember this");
    turn["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", turn).await.expect_success();
    client.events_through_the_turn(&watch).await;
    client.close().await;
    first.stop().await;
    peer.reset_prompts();
}

fn stored_opencode_cursor(database: &Path) -> String {
    rusqlite::Connection::open(database).unwrap().query_row(
        "SELECT provider_resume_cursor FROM threads WHERE id = 'resume-thread'",
        [],
        |row| row.get(0),
    ).unwrap()
}

async fn checkpointed_external_thread(
    peer: &ExternalOpenCode,
    database: &Path,
    workspace: &Workspace,
) {
    let first = TestServer::start_at_with_config(database, peer.config(None)).await;
    let mut client = first.connect().await;
    client.call("orchestration.dispatchCommand", create_project("rollback-project", workspace.path())).await.expect_success();
    let mut create = create_thread("rollback-project", "rollback-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let watch = client.watch_conversation("rollback-thread").await;
    let mut turn = start_turn("rollback-thread", "rollback-message-1", "first");
    turn["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", turn).await.expect_success();
    // The event a live client folds carries the checkpoint bound to its turn's
    // reply — the mapping the UI's per-message revert is keyed through. A
    // capture that reached the wire unbound would draw no revert affordance
    // even while the server still held the conversation.
    let seen = client.events_through_the_checkpoint(&watch, 1).await;
    assert!(seen.iter().any(|item| {
        let payload = &item["event"]["payload"];
        payload["checkpointTurnCount"] == json!(1) && payload["assistantMessageId"].is_string()
    }), "the turn-diff event must name the assistant message the diff belongs to");
    client.close().await;
    first.stop().await;

    workspace.put("tracked.txt", "after\n");
    workspace.put("later.txt", "only in the removed turn\n");
    peer.reset_prompts();
    let second = TestServer::start_at_with_config(database, peer.config(None)).await;
    let mut client = second.connect().await;
    let watch = client.watch_conversation("rollback-thread").await;
    let mut turn = follow_up("rollback-thread", "rollback-message-2", "second");
    turn["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", turn).await.expect_success();
    client.events_through_the_checkpoint(&watch, 2).await;
    client.close().await;
    second.stop().await;
    peer.reset_prompts();
}

async fn rollback_outcome(fails: bool) {
    let data = tempfile::tempdir().unwrap();
    let database = data.path().join("registry.sqlite");
    let workspace = Workspace::with(&["tracked.txt"]);
    workspace.put("tracked.txt", "before\n");
    workspace.init_repository().commit("initial");
    let probe = workspace.path().join("tracked.txt");
    let peer = ExternalOpenCode::for_rollback(probe, fails).await;
    checkpointed_external_thread(&peer, &database, &workspace).await;
    // The durable row carries the resolution the fold made, not the driver's
    // unbound capture — a restart hydrates checkpoints from here, and a null
    // `assistant_message_id` is a conversation whose per-message revert
    // disappears across an update.
    {
        let conn = rusqlite::Connection::open(&database).unwrap();
        let mut stmt = conn
            .prepare("SELECT assistant_message_id FROM thread_checkpoints WHERE thread_id = 'rollback-thread'")
            .unwrap();
        let rows: Vec<Option<String>> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(!rows.is_empty(), "the turns recorded no checkpoints at all");
        assert!(
            rows.iter().all(|assistant| assistant.is_some()),
            "a stored OpenCode checkpoint has no assistant message bound to it"
        );
    }
    let cursor_before = rusqlite::Connection::open(&database).unwrap().query_row::<String, _, _>(
        "SELECT provider_resume_cursor FROM threads WHERE id = 'rollback-thread'",
        [],
        |row| row.get(0),
    ).unwrap();

    let restarted = TestServer::start_at_with_config(&database, peer.config(None)).await;
    let mut client = restarted.connect().await;
    let watch = client.watch_conversation("rollback-thread").await;
    client.call("projects.listEntries", json!({"cwd": workspace.cwd()})).await.expect_success();
    client.call("orchestration.dispatchCommand", revert_checkpoint("rollback-thread", 1)).await.expect_success();
    let seen = client.values_until(&watch, |item| {
        if fails {
            item["event"]["payload"]["activity"]["kind"] == "revert.failed"
        } else {
            item["event"]["type"] == "thread.reverted"
        }
    }).await;
    assert_eq!(workspace.read("tracked.txt"), "before\n");
    let search = client.call(
        "projects.searchEntries",
        json!({"cwd":workspace.cwd(),"query":"later.txt","limit":10}),
    ).await.expect_success();
    assert_eq!(search["entries"], json!([]), "workspace search must use the refreshed restored index");
    let reference = laplus_server::checkpoints::reference("rollback-thread", 2);
    let later_ref_exists = workspace.try_git(&["rev-parse", "--verify", "--quiet", &reference]).status.success();
    assert_eq!(later_ref_exists, fails, "later refs are pruned only after provider success");
    assert_eq!(
        rusqlite::Connection::open(&database).unwrap().query_row::<String, _, _>(
            "SELECT provider_resume_cursor FROM threads WHERE id = 'rollback-thread'",
            [],
            |row| row.get(0),
        ).unwrap(),
        cursor_before,
        "rollback must not replace the adopted continuation cursor"
    );
    assert_eq!(
        seen.iter().any(|item| item["event"]["type"] == "thread.reverted"),
        !fails,
        "failure must not publish false completion"
    );

    let requests = peer.requests_through(9).await;
    let tail = requests.iter().rev().take(4).rev().map(|entry| entry["operation"].as_str().unwrap()).collect::<Vec<_>>();
    assert_eq!(tail, vec!["get", "update", "messages", "revert"]);
    assert_eq!(requests.last().unwrap()["body"], json!({"messageID":"assistant-1"}));
    client.close().await;
    restarted.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn opencode_checkpoint_rollback_orders_tree_history_refs_and_completion() {
    rollback_outcome(false).await;
}

#[tokio::test]
async fn opencode_checkpoint_rollback_keeps_the_recoverable_partial_state_on_provider_failure() {
    rollback_outcome(true).await;
}

async fn resume_external_turn(peer: &ExternalOpenCode, database: &Path) -> Vec<Value> {
    let restarted = TestServer::start_at_with_config(database, peer.config(None)).await;
    let mut client = restarted.connect().await;
    let watch = client.watch_conversation("resume-thread").await;
    let mut turn = follow_up("resume-thread", "resume-message-2", "continue");
    turn["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", turn).await.expect_success();
    let events = client.events_through_the_turn(&watch).await;
    client.close().await;
    restarted.stop().await;
    events
}

#[tokio::test]
async fn a_structured_missing_opencode_session_starts_fresh_and_replaces_the_cursor() {
    let peer = ExternalOpenCode::for_resume(ResumeBehavior::Missing).await;
    let data = tempfile::tempdir().unwrap();
    let database = data.path().join("registry.sqlite");
    let workspace = Workspace::with(&["src/"]);
    seed_external_opencode_thread(&peer, &database, &workspace).await;
    let before = stored_opencode_cursor(&database);

    let events = resume_external_turn(&peer, &database).await;
    assert_eq!(assistant_sends(&events).last().unwrap().0, "continued OpenCode session");
    assert_ne!(stored_opencode_cursor(&database), before);
    assert!(stored_opencode_cursor(&database).contains("ses_fresh_2"));
    let operations = peer.requests_through(5).await;
    assert_eq!(operations[2]["operation"], "get");
    assert_eq!(operations[3]["operation"], "create");
    assert_eq!(operations[4]["operation"], "prompt");
    peer.task.abort();
}

#[tokio::test]
async fn incompatible_opencode_cursors_fail_closed_without_contacting_or_replacing_the_peer() {
    for cursor in [json!({"version":1}), json!({"version":2,"sessionId":"future"})] {
        let peer = ExternalOpenCode::start(None).await;
        let data = tempfile::tempdir().unwrap();
        let database = data.path().join("registry.sqlite");
        let workspace = Workspace::with(&["src/"]);
        seed_external_opencode_thread(&peer, &database, &workspace).await;
        rusqlite::Connection::open(&database).unwrap().execute(
            "UPDATE threads SET provider_resume_cursor = ?1 WHERE id = 'resume-thread'",
            [cursor.to_string()],
        ).unwrap();

        let events = resume_external_turn(&peer, &database).await;
        let failed = &activity(&events, "session.failed")["payload"]["activity"];
        assert!(failed["summary"].as_str().unwrap().contains("cursor"));
        assert_eq!(stored_opencode_cursor(&database), cursor.to_string());
        assert_eq!(peer.requests_through(2).await.len(), 2, "invalid cursor contacted OpenCode");
        peer.task.abort();
    }
}

#[tokio::test]
async fn opencode_resume_failures_preserve_the_verified_cursor() {
    for behavior in [
        ResumeBehavior::GetFailure,
        ResumeBehavior::UpdateFailure,
        ResumeBehavior::ForkFailure,
        ResumeBehavior::MoveFailure,
        ResumeBehavior::VerificationFailure,
    ] {
        let peer = ExternalOpenCode::for_resume(behavior).await;
        let data = tempfile::tempdir().unwrap();
        let database = data.path().join("registry.sqlite");
        let original = Workspace::with(&["old/"]);
        seed_external_opencode_thread(&peer, &database, &original).await;
        let before = stored_opencode_cursor(&database);
        if !matches!(behavior, ResumeBehavior::GetFailure | ResumeBehavior::UpdateFailure) {
            let moved = Workspace::with(&["new/"]);
            rusqlite::Connection::open(&database).unwrap().execute(
                "UPDATE projects SET workspace_root = ?1, canonical_root = ?1 WHERE id = 'resume-project'",
                [moved.path().display().to_string()],
            ).unwrap();
            // Keep the replacement workspace alive through recovery.
            let events = resume_external_turn(&peer, &database).await;
            assert!(activity(&events, "session.failed")["payload"]["activity"]["summary"].is_string());
            assert_eq!(stored_opencode_cursor(&database), before);
            drop(moved);
        } else {
            let events = resume_external_turn(&peer, &database).await;
            assert!(activity(&events, "session.failed")["payload"]["activity"]["summary"].is_string());
            assert_eq!(stored_opencode_cursor(&database), before);
        }
        peer.task.abort();
    }
}

#[tokio::test]
async fn cwd_migration_adopts_only_verified_forks_with_and_without_move() {
    for behavior in [ResumeBehavior::ForkTarget, ResumeBehavior::ForkThenMove] {
        let peer = ExternalOpenCode::for_resume(behavior).await;
        let data = tempfile::tempdir().unwrap();
        let database = data.path().join("registry.sqlite");
        let original = Workspace::with(&["old/"]);
        seed_external_opencode_thread(&peer, &database, &original).await;
        let moved = Workspace::with(&["new/"]);
        rusqlite::Connection::open(&database).unwrap().execute(
            "UPDATE projects SET workspace_root = ?1, canonical_root = ?1 WHERE id = 'resume-project'",
            [moved.path().display().to_string()],
        ).unwrap();

        let events = resume_external_turn(&peer, &database).await;
        assert_eq!(assistant_sends(&events).last().unwrap().0, "continued OpenCode session");
        assert!(stored_opencode_cursor(&database).contains("ses_forked_1"));
        let requests = peer.requests_through(if matches!(behavior, ResumeBehavior::ForkTarget) { 6 } else { 8 }).await;
        let operations = requests.iter().map(|row| row["operation"].as_str().unwrap()).collect::<Vec<_>>();
        assert!(operations.windows(2).any(|ops| ops == ["get", "fork"]));
        if matches!(behavior, ResumeBehavior::ForkTarget) {
            assert!(!operations.contains(&"move"));
        } else {
            assert!(operations.windows(2).any(|ops| ops == ["move", "get"]));
            let movement = requests.iter().find(|row| row["operation"] == "move").unwrap();
            assert_eq!(movement["body"]["sessionID"], "ses_forked_1");
            assert_eq!(movement["body"]["moveChanges"], false);
        }
        peer.task.abort();
    }
}

#[tokio::test]
async fn an_owned_opencode_session_is_re_adopted_after_a_server_restart() {
    let first_opencode = FakeOpenCode::new();
    let data = tempfile::tempdir().unwrap();
    let database = data.path().join("registry.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let first = TestServer::start_at_with_config(&database, opencode_config(&first_opencode)).await;
    let mut client = first.connect().await;
    client.call("orchestration.dispatchCommand", create_project("owned-resume-project", workspace.path())).await.expect_success();
    let mut create = create_thread("owned-resume-project", "owned-resume-thread");
    create["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", create).await.expect_success();
    let watch = client.watch_conversation("owned-resume-thread").await;
    let mut turn = start_turn("owned-resume-thread", "owned-message-1", "remember");
    turn["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", turn).await.expect_success();
    client.events_through_the_turn(&watch).await;
    let first_requests = first_opencode.requests_through(4).await;
    let first_port = first_requests[0]["port"].as_u64().expect("the first owned port") as u16;
    client.close().await;
    first.stop().await;
    wait_until_port_closes(first_port, "the first owned OpenCode child stops before restart").await;

    let second = TestServer::start_at_with_config(&database, opencode_config(&first_opencode)).await;
    let mut client = second.connect().await;
    let watch = client.watch_conversation("owned-resume-thread").await;
    let mut turn = follow_up("owned-resume-thread", "owned-message-2", "continue");
    turn["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client.call("orchestration.dispatchCommand", turn).await.expect_success();
    client.events_through_the_turn(&watch).await;
    let requests = first_opencode.requests_through(9).await;
    assert_eq!(requests[4..].iter().map(|row| row["operation"].as_str().unwrap()).collect::<Vec<_>>(), vec!["launch", "mcp.add", "get", "update", "prompt"]);
    client.close().await;
    second.stop().await;
}

#[tokio::test]
async fn opencode_reasoning_reaches_the_socket_before_the_turn_settles() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(idle_release.clone())).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("reasoning-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("reasoning-project", "reasoning-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("reasoning-thread").await;
    let mut command = start_turn("reasoning-thread", "reasoning-message", "say hello");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();

    let before_idle = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["thinking"] == "check the stream"
        }),
    )
    .await
    .expect("reasoning reaches the socket before OpenCode reports idle");
    assert!(!before_idle.iter().any(|item| {
        item["event"]["type"] == "thread.session-set"
            && item["event"]["payload"]["session"]["status"] == "ready"
    }));

    idle_release.notify_one();
    let after_idle = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        before_idle
            .iter()
            .chain(&after_idle)
            .filter(|item| {
                item["event"]["payload"]["activity"]["payload"]["thinking"] == "check the stream"
            })
            .count(),
        1,
        "settlement must not publish the streamed reasoning again"
    );
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
#[ignore]
async fn opencode_peer_child() {
    let port = std::env::var("OPENCODE_TEST_PORT")
        .expect("peer port")
        .trim_start_matches("--port=")
        .parse::<u16>()
        .expect("numeric peer port");
    let log = PathBuf::from(std::env::var("OPENCODE_TEST_LOG").expect("peer log"));
    append(&log, json!({"operation":"launch","port":port}));
    let state = PeerState {
        log: Arc::new(log),
        healthy: std::env::var("OPENCODE_TEST_HEALTHY").as_deref() != Ok("false"),
        mcp_failed: std::env::var("OPENCODE_TEST_MCP_FAIL").as_deref() == Ok("true"),
        idle_release: (std::env::var("OPENCODE_TEST_GATED").as_deref() == Ok("true"))
            .then(|| Arc::new(Notify::new())),
        subagent: std::env::var("OPENCODE_TEST_SUBAGENT").as_deref() == Ok("true"),
        output_changes_during_stop: std::env::var("OPENCODE_TEST_RUNAWAY").as_deref()
            == Ok("true"),
        delayed_catalogue: std::env::var("OPENCODE_TEST_DELAY_CATALOGUE").as_deref()
            == Ok("true"),
        ..Default::default()
    };
    let app = Router::new()
        .route("/global/health", get(health))
        .route("/provider", get(providers))
        .route("/config", get(opencode_config_snapshot))
        .route("/mcp", post(add_mcp))
        .route("/event", get(events))
        .route("/session", post(create_session))
        .route("/session/{id}", get(get_session).patch(update_session))
        .route("/session/{id}/prompt_async", post(prompt))
        .route("/session/{id}/abort", post(abort))
        .route("/session/{id}/message", get(session_messages))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind scripted OpenCode peer");
    axum::serve(listener, app)
        .await
        .expect("serve scripted OpenCode peer");
}

/// OpenCode runs a subagent as a **session of its own**, whose events arrive on
/// the same stream as its parent's. They used to be discarded — correctly, as
/// another conversation's — which left a subagent as a row that said nothing
/// between starting and finishing, however long that was.
///
/// The row is the same one the Claude driver builds, from the same
/// `SubagentTask`, so a subagent reads the same way whichever agent is running.
/// What differs is only where the pieces come from: the `task` call names the
/// agent and carries the answer, and the child session is what it says meanwhile.
#[tokio::test]
async fn a_subagent_gets_a_row_of_its_own_and_says_what_it_is_doing() {
    let SocketTurn {
        _workspace,
        // Bound, not discarded: the fake owns a `TempDir` holding the peer's
        // request log, and dropping it deletes the directory out from under the
        // running child.
        opencode: _opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(
        FakeOpenCode::spawning_a_subagent(),
        "project-1",
        "thread-open",
    )
    .await;
    let events = client.events_through_the_turn(&subscription).await;

    let rows: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .filter(|activity| activity["payload"]["data"]["toolCallId"] == "subagent:call_task_1")
        .collect();
    assert!(!rows.is_empty(), "the subagent was invisible");

    // It is a subagent rather than a tool called `task`, and it is named.
    assert_eq!(rows[0]["payload"]["itemType"], "collab_agent_tool_call");
    assert_eq!(rows[0]["payload"]["title"], "Subagent explore");
    assert!(
        rows.iter()
            .all(|row| row["payload"]["title"] == "Subagent explore"),
        "the row was renamed part-way: {rows:#?}"
    );

    // And it said what it was doing while it did it, which is the whole point.
    let details: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["payload"]["detail"].as_str())
        .collect();
    assert!(
        details.contains(&"looking through the directory"),
        "the subagent's own words never reached its row: {details:?}"
    );

    // Its answer is the last thing the row says, and nothing it said afterwards
    // replaced it.
    let last = rows.last().expect("a subagent row");
    assert_eq!(last["kind"], "tool.completed");
    assert_eq!(last["payload"]["status"], "completed");
    assert_eq!(last["payload"]["detail"], "eleven files");

    // The subagent's session is not the developer's conversation.
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-open")
        .await;
    let said: Vec<String> = snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["text"].as_str())
        .map(str::to_string)
        .collect();
    assert!(
        !said
            .iter()
            .any(|text| text.contains("looking through the directory")),
        "a subagent's words reached the transcript: {said:#?}"
    );

    client.close().await;
    server.stop().await;
}

/// The other half of [`a_subagent_gets_a_row_of_its_own_and_says_what_it_is_doing`],
/// and the half that was only ever assumed.
///
/// `normalize_subagent` returns `None` for two opposite reasons — "not a subagent",
/// which is an invitation to draw the ordinary tool row, and "a subagent that
/// cannot be named yet", which is a request to draw nothing at all. The caller
/// cannot tell them apart, so the `pending` `task` part OpenCode opens every
/// subagent with fell through to `tool_activity` and put a second row on the
/// thread, keyed `call_task_1` beside the subagent's own `subagent:call_task_1`.
///
/// One call, one row. The sibling test filters on the prefixed key and so cannot
/// see the other one however wrong it gets, which is why this is a test rather
/// than an assertion added over there.
#[tokio::test]
async fn a_subagent_is_not_also_drawn_as_a_tool_called_task() {
    let SocketTurn {
        _workspace,
        opencode: _opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(
        FakeOpenCode::spawning_a_subagent(),
        "project-1",
        "thread-open",
    )
    .await;
    let events = client.events_through_the_turn(&subscription).await;

    let strays: Vec<String> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .filter(|activity| activity["payload"]["data"]["toolCallId"] == "call_task_1")
        .map(|activity| {
            format!(
                "{} {:?}",
                activity["kind"].as_str().unwrap_or("?"),
                activity["payload"]["title"].as_str().unwrap_or("?")
            )
        })
        .collect();

    assert!(
        strays.is_empty(),
        "the subagent was drawn a second time as a tool called `task`: {strays:#?}"
    );

    client.close().await;
    server.stop().await;
}

/// Open one delegated child, drive it, and read it back through the socket.
///
/// The tracer bullet for the whole subagent-ux feature, and everything it
/// asserts is asserted the way a client would learn it: the compact row a
/// developer clicks, the subscription that row addresses, and the thread
/// snapshot a reload takes. Nothing here reaches into the adapter or the
/// database.
///
/// The peer stops half way through the child's work, so the subscription is
/// opened while the child is genuinely working. That is the replay/live
/// boundary: a client that lost an entry there, or was handed one twice, or saw
/// them out of order, would be indistinguishable from one that never had the
/// entry at all once the stream ended.
#[tokio::test]
async fn an_opencode_child_work_stream_replays_and_then_continues_live() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_subagent(release.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("child-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("child-project", "child-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("child-thread").await;
    let mut command = start_turn("child-thread", "child-message", "count the files");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();

    // The launcher. One compact row, naming the child and carrying the
    // reference the stream is addressed by.
    let opening = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the compact child row reaches the socket while the child is working");
    let row = opening
        .iter()
        .filter_map(|item| item["event"]["payload"].get("activity"))
        .find(|activity| activity["payload"]["data"]["childId"] == "call_task_1")
        .expect("a row carrying the stream reference");
    assert_eq!(row["payload"]["itemType"], "collab_agent_tool_call");
    assert_eq!(row["payload"]["title"], "Subagent explore");

    // A second window, opening the child while it is still working. Its own
    // connection, because a developer inspecting a child has not closed the
    // conversation they are inspecting it from.
    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-thread", "childId": "call_task_1"}),
        )
        .await;
    let replayed = inspector.next_chunk(&stream).await;
    inspector.ack(&stream).await;
    let snapshot = replayed
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("a child stream opens with itself")["snapshot"]
        .clone();
    assert_eq!(snapshot["stream"]["childId"], "call_task_1");
    assert_eq!(snapshot["stream"]["name"], "explore");
    assert_eq!(snapshot["stream"]["assignment"], "Count the files");
    assert_eq!(
        snapshot["stream"]["state"], "working",
        "a child that is still working must not read as finished: {snapshot:#?}"
    );
    assert_eq!(snapshot["stream"]["outcome"], Value::Null);

    // Now let the child finish, and fold what arrives the way a client does:
    // upsert by entry id, order by sequence.
    release.notify_one();
    let live = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inspector.values_until(&stream, |item| {
            item["kind"] == "stream-updated" && item["stream"]["state"] == "completed"
        }),
    )
    .await
    .expect("the child's conclusion reaches its stream");

    let mut folded: Vec<Value> = snapshot["entries"]
        .as_array()
        .expect("the snapshot carries the entries so far")
        .clone();
    // Every entry id the wire ever carried, replay and live together, in the
    // order it first appeared.
    let mut seen_ids: Vec<String> = folded
        .iter()
        .map(|entry| entry["id"].as_str().expect("an entry id").to_string())
        .collect();
    for item in replayed.iter().chain(live.iter()) {
        let Some(entry) = item["entry"].as_object() else {
            continue;
        };
        let id = entry["id"].as_str().expect("an entry id").to_string();
        if !seen_ids.contains(&id) {
            seen_ids.push(id.clone());
        }
        match folded.iter().position(|held| held["id"] == entry["id"]) {
            Some(index) => folded[index] = item["entry"].clone(),
            None => folded.push(item["entry"].clone()),
        }
    }
    folded.sort_by_key(|entry| entry["sequence"].as_i64().unwrap_or_default());

    let read: Vec<(i64, &str, &str)> = folded
        .iter()
        .map(|entry| {
            (
                entry["sequence"].as_i64().expect("a sequence"),
                entry["kind"].as_str().expect("a kind"),
                entry["payload"]["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            (1, "message", "looking through the directory"),
            (2, "message", "eleven so far"),
            (3, "outcome", "eleven files"),
        ],
        "the child's stream lost, repeated or reordered work across the \
         replay/live boundary: {folded:#?}"
    );
    assert_eq!(
        folded.len(),
        3,
        "a part OpenCode resent carrying the prose so far became a second entry"
    );
    // And the wire carried no fourth entry to be folded away. Without this the
    // claim above would only be that the client's fold is idempotent, which is
    // true of a server sending anything at all.
    assert_eq!(
        seen_ids,
        vec![
            "call_task_1:k:child-prt-2".to_string(),
            "call_task_1:k:child-prt-3".to_string(),
            "call_task_1:k:outcome".to_string(),
        ],
        "the child's stream carried an entry nothing asked for"
    );

    // The conclusion is the stream's, not a replacement for it.
    let concluded = live
        .iter()
        .rfind(|item| item["kind"] == "stream-updated")
        .expect("the child settles")["stream"]
        .clone();
    assert_eq!(concluded["state"], "completed");
    assert_eq!(concluded["outcome"]["kind"], "completed");
    assert_eq!(concluded["outcome"]["text"], "eleven files");

    client.events_through_the_turn(&subscription).await;
    inspector.close().await;
    client.close().await;

    // A reload: a connection that watched none of it replays the same stream,
    // and the conversation it belongs to still carries only the compact row.
    let mut reloaded = server.connect().await;
    let stream = reloaded
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-thread", "childId": "call_task_1"}),
        )
        .await;
    let replayed = reloaded.next_chunk(&stream).await;
    let snapshot = replayed
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("a completed child replays")["snapshot"]
        .clone();
    let replayed_read: Vec<(i64, &str, &str)> = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["sequence"].as_i64().expect("a sequence"),
                entry["kind"].as_str().expect("a kind"),
                entry["payload"]["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(replayed_read, read, "the replay is not the same stream");
    assert_eq!(snapshot["stream"]["outcome"]["text"], "eleven files");
    reloaded.close().await;

    let thread = server
        .connect()
        .await
        .into_thread_snapshot("child-thread")
        .await["thread"]
        .clone();
    let said: Vec<&str> = thread["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["text"].as_str())
        .collect();
    assert!(
        !said
            .iter()
            .any(|text| text.contains("looking through the directory")),
        "the child's prose reached the parent transcript: {said:#?}"
    );
    let carried: Vec<&Value> = thread["activities"]
        .as_array()
        .expect("activities")
        .iter()
        .filter(|activity| activity["payload"]["data"]["childId"] == "call_task_1")
        .collect();
    assert!(!carried.is_empty(), "the snapshot lost the compact child row");
    // Every one of them is the same compact row the client collapses on, and
    // none of them is the stream: no entries, no ordering, no history. The work
    // is only ever reached through the subscription above.
    assert!(
        carried.iter().all(|activity| {
            activity["payload"]["data"]["toolCallId"] == "subagent:call_task_1"
                && activity["payload"].get("entries").is_none()
        }),
        "an ordinary thread snapshot carried the child's work rather than its \
         index row: {carried:#?}"
    );
    assert!(
        thread.get("subagents").is_none(),
        "the thread snapshot grew a child-stream field: {thread:#?}"
    );

    server.stop().await;
    peer.task.abort();
}

/// A conversation with a delegated child under way, and the compact row that
/// launches it already on the wire.
///
/// The four child tests below differ only in what their peer scripts, so the
/// eleven lines that get one delegated are here once. What each of them does
/// with the returned client is the test.
async fn a_delegating_turn(
    peer: &ExternalOpenCode,
    suffix: &str,
) -> (Workspace, TestServer, SocketClient, String, String) {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    let project = format!("child-{suffix}-project");
    let thread = format!("child-{suffix}-thread");
    client
        .call(
            "orchestration.dispatchCommand",
            create_project(&project, workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread(&project, &thread);
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation(&thread).await;
    let mut command = start_turn(&thread, &format!("child-{suffix}-message"), "count the files");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the child is delegated");
    (workspace, server, client, subscription, thread)
}


/// The whole of what OpenCode exposes about a child's work, in the order it
/// happened, read back through the subscription a tab opens.
///
/// The stream is what "enter the child's work" *means*, so this asserts the
/// shape a client renders from rather than that some events arrived: a command
/// with its command line and its output, a read and a search with the file and
/// the pattern, an edit with the file it changed, a tool call that failed with
/// its error, a warning in its chronological place, and prose between them.
///
/// It also asserts two silences. The `pending` `tool` part OpenCode opens every
/// call with produces **no** entry — it carries nothing but an id, and a row
/// that appeared nameless to be renamed a beat later is the partial state the
/// spec asks to be left out. And an event kind this build has never seen
/// produces no entry and breaks neither the child's stream nor the parent's
/// turn, which is the drift policy applied to a descendant.
#[tokio::test]
async fn a_child_stream_preserves_the_whole_of_what_the_child_did() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_working_subagent(release.clone()).await;
    let (_workspace, server, mut client, subscription, _thread) =
        a_delegating_turn(&peer, "work").await;

    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-work-thread", "childId": "call_task_1"}),
        )
        .await;
    let replayed = inspector.next_chunk(&stream).await;
    inspector.ack(&stream).await;
    let snapshot = replayed
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("a child stream opens with itself")["snapshot"]
        .clone();

    release.notify_one();
    let live = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inspector.values_until(&stream, |item| {
            item["kind"] == "stream-updated" && item["stream"]["state"] == "completed"
        }),
    )
    .await
    .expect("the child's conclusion reaches its stream");
    let folded = folded_entries(&snapshot, &replayed.iter().chain(live.iter()).cloned().collect::<Vec<_>>());

    let read: Vec<(&str, &str)> = folded
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().expect("a kind"),
                entry["payload"]["text"]
                    .as_str()
                    .or_else(|| entry["payload"]["title"].as_str())
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            ("message", "looking through the directory"),
            ("message", "counting them"),
            ("command", "ls -1 src | wc -l"),
            ("read", "src/main.rs"),
            ("read", "grep fn main"),
            ("edit", "src/counted.rs"),
            ("tool", "webfetch"),
            ("notice", "Retrying the child's request"),
            ("message", "eleven so far"),
            ("outcome", "eleven files"),
        ],
        "the child's work was lost, reordered, or read as the wrong kind: {folded:#?}"
    );

    let at = |index: usize| folded[index]["payload"].clone();
    assert_eq!(at(2)["command"], "ls -1 src | wc -l", "a command entry carries what was run");
    assert_eq!(at(2)["detail"], "11", "a command entry carries its output");
    assert_eq!(at(2)["status"], "completed");
    assert_eq!(at(3)["paths"], json!(["src/main.rs"]), "a read names the file it examined");
    assert_eq!(at(4)["query"], "fn main", "a search names what it looked for");
    assert_eq!(
        at(4)["paths"],
        json!([]),
        "a search's directory is not a file the developer can be offered to open"
    );
    assert_eq!(
        at(5)["paths"],
        json!(["src/counted.rs"]),
        "an edit names the file it changed, which is what diff navigation is offered from"
    );
    assert_eq!(at(6)["status"], "failed", "a tool that errored says so");
    assert_eq!(at(6)["detail"], "could not resolve host");
    assert_eq!(at(7)["level"], "warning");
    assert_eq!(folded[9]["payload"]["kind"], "completed");
    assert_eq!(folded[9]["payload"]["text"], "eleven files");

    // The `pending` part and the unknown event both produced nothing, and the
    // parent's turn ended normally in spite of the second.
    assert!(
        folded.iter().all(|entry| entry["payload"]["status"] != "inProgress"),
        "an announced-but-empty call was drawn: {folded:#?}"
    );
    client.events_through_the_turn(&subscription).await;

    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// The compact row is an index, not a log: it shows the latest meaningful thing
/// the child did while it runs, and what came back once it has finished.
///
/// Both halves are one test because the second is only interesting given the
/// first — a terminal row that happens to be blank would satisfy "no stale
/// activity" while telling the developer nothing.
#[tokio::test]
async fn the_compact_row_follows_the_child_and_then_reports_it() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_working_subagent(release.clone()).await;
    let (_workspace, server, mut client, subscription, _thread) =
        a_delegating_turn(&peer, "row").await;
    release.notify_one();
    let events = client.events_through_the_turn(&subscription).await;

    let rows: Vec<&Value> = events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .filter(|activity| activity["payload"]["data"]["childId"] == "call_task_1")
        .collect();
    let details: Vec<&str> = rows
        .iter()
        .filter_map(|row| row["payload"]["detail"].as_str())
        .collect();

    assert!(
        details.contains(&"ls -1 src | wc -l"),
        "the row never showed what the child ran: {details:#?}"
    );
    assert!(
        details.contains(&"src/main.rs"),
        "the row never showed what the child examined: {details:#?}"
    );
    // Transport noise and partial states never reach it. `bash` is what the row
    // would have said had the announced-but-empty call been drawn, and `task` is
    // the tool the whole subagent is.
    assert!(
        !details.iter().any(|detail| *detail == "bash" || *detail == "task"),
        "an unhelpful partial state reached the compact row: {details:#?}"
    );

    // And the answer replaces all of it, atomically: the last row is the
    // conclusion and nothing the child said on the way to it survives beside it.
    let last = rows.last().expect("a subagent row");
    assert_eq!(last["payload"]["status"], "completed");
    assert_eq!(last["payload"]["detail"], "eleven files");

    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// A subagent that stops for a permission it cannot grant itself.
///
/// The whole route, end to end and through the boundary a client actually uses:
/// the child's own stream records that it waited, the **main conversation** gets
/// the ordinary request row naming the child that raised it, the developer's
/// decision is answered with no child surface open at all, OpenCode receives it
/// on the descendant's own request identity, and the child's stream records how
/// it resolved.
///
/// The tab is deliberately closed before the answer. A blocker that is only
/// actionable while its child's tab is open is a blocker that hides.
#[tokio::test]
async fn a_descendant_permission_is_recorded_in_the_child_and_answered_from_the_conversation() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_blocked_subagent(release.clone(), "permission").await;
    let (_workspace, server, mut client, subscription, thread) =
        a_delegating_turn(&peer, "block").await;

    let asked = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "approval.requested"
        }),
    )
    .await
    .expect("a descendant's permission reaches the main conversation");
    let request = activity(&asked, "approval.requested")["payload"]["activity"].clone();
    let request_id = request["payload"]["requestId"]
        .as_str()
        .expect("a request id")
        .to_string();
    assert_eq!(request_id, "child-per-1");
    assert_eq!(
        request["payload"]["subagent"]["childId"], "call_task_1",
        "the request did not say which child is waiting: {request:#?}"
    );
    assert_eq!(request["payload"]["subagent"]["name"], "explore");
    assert!(
        request["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("Subagent explore")),
        "the request the developer reads does not name the waiting child: {request:#?}"
    );

    // Open the child's tab, see that it says it is waiting, and close it again.
    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": thread, "childId": "call_task_1"}),
        )
        .await;
    let opened = inspector.next_chunk(&stream).await;
    let snapshot = opened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child opens")["snapshot"]
        .clone();
    assert_eq!(
        snapshot["stream"]["state"], "blocked",
        "a child waiting on the developer did not say so: {snapshot:#?}"
    );
    let waiting = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["kind"] == "blocker")
        .expect("the child's history says why it waited")
        .clone();
    assert_eq!(waiting["payload"]["requestId"], "child-per-1");
    assert_eq!(waiting["payload"]["blocker"], "permission");
    assert_eq!(waiting["payload"]["resolution"], Value::Null);
    inspector.interrupt(&stream).await;

    // Answered with no child surface open anywhere.
    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval(&thread, &request_id, "accept"),
        )
        .await
        .expect_success();

    let resolved = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "approval.resolved"
        }),
    )
    .await
    .expect("the decision closes the request in the main conversation");
    assert!(
        activity(&resolved, "approval.resolved")["payload"]["activity"]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("Subagent explore")),
        "the resolution did not say whose request it answered"
    );

    // OpenCode received it on the descendant's own request identity.
    let requests = peer.requests_through(3).await;
    let reply = requests
        .iter()
        .find(|request| request["operation"] == "permission.reply")
        .expect("the decision reached OpenCode");
    assert_eq!(reply["requestId"], "child-per-1");
    assert_eq!(reply["body"]["reply"], "once");

    release.notify_one();
    client.events_through_the_turn(&subscription).await;

    // And the child's own history records how it resolved, on the same entry.
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": thread, "childId": "call_task_1"}),
        )
        .await;
    let reopened = inspector.next_chunk(&stream).await;
    let snapshot = reopened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child reopens")["snapshot"]
        .clone();
    let blockers: Vec<&Value> = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .filter(|entry| entry["kind"] == "blocker")
        .collect();
    assert_eq!(
        blockers.len(),
        1,
        "asking and answering became two rows rather than one blocker: {blockers:#?}"
    );
    assert_eq!(blockers[0]["payload"]["resolution"], "approved");
    assert_eq!(snapshot["stream"]["state"], "completed");

    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// An entry that changes **in place** reaches a client that is already watching.
///
/// The mechanism this guards is easy to get wrong and silent when it is. A tool
/// call and a blocker are both one entry that *moves* — `inProgress` becomes
/// `completed`, `resolution: null` becomes a decision — so what a live reader
/// receives is an upsert of an entry it already holds rather than a new one. The
/// stream head moves too, but `updatedAt` is millisecond-resolution, so two
/// writes inside one millisecond stamp identically: a client that noticed
/// changes by watching the head would miss both of these.
///
/// So this folds **only the `entry-upserted` frames** and throws every
/// `stream-updated` away. What survives that is what a reader learns from the
/// entries alone, which is the only thing it may depend on.
#[tokio::test]
async fn an_entry_that_changes_in_place_reaches_a_watching_client() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_working_subagent(release.clone()).await;
    let (_workspace, server, mut client, subscription, thread) =
        a_delegating_turn(&peer, "live").await;

    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": thread, "childId": "call_task_1"}),
        )
        .await;
    let opened = inspector.next_chunk(&stream).await;
    inspector.ack(&stream).await;
    let snapshot = opened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child opens")["snapshot"]
        .clone();

    // The call is in flight when the tab opens, which is what makes what follows
    // an update rather than an arrival.
    let running = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["kind"] == "command")
        .expect("the child's command is already in the stream")
        .clone();
    assert_eq!(
        running["payload"]["status"], "inProgress",
        "the scripted call had already finished, so nothing here is an in-place update: {snapshot:#?}"
    );

    release.notify_one();
    let live = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inspector.values_until(&stream, |item| {
            item["kind"] == "stream-updated" && item["stream"]["state"] == "completed"
        }),
    )
    .await
    .expect("the child's conclusion reaches its stream");

    // Everything the wire carried as an entry, and nothing it carried as a head.
    let upserted: Vec<&Value> = live
        .iter()
        .filter(|item| item["kind"] == "entry-upserted")
        .map(|item| &item["entry"])
        .collect();
    let moved = upserted
        .iter()
        .find(|entry| entry["id"] == running["id"])
        .unwrap_or_else(|| {
            panic!(
                "the call finished and no client watching it was told: {upserted:#?}"
            )
        });
    assert_eq!(moved["payload"]["status"], "completed");
    assert_eq!(moved["payload"]["detail"], "11", "the output never arrived");
    assert_eq!(
        moved["sequence"], running["sequence"],
        "an in-place update moved the entry to the end of the child's history"
    );

    client.events_through_the_turn(&subscription).await;
    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// The same guarantee for a blocker's resolution, which is the other entry in
/// this feature that changes under its own key — and the one whose staleness a
/// developer would read as "the child is still waiting for me".
#[tokio::test]
async fn a_blockers_resolution_reaches_a_watching_client() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_blocked_subagent(release.clone(), "permission").await;
    let (_workspace, server, mut client, subscription, thread) =
        a_delegating_turn(&peer, "livefix").await;

    let asked = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "approval.requested"
        }),
    )
    .await
    .expect("the descendant's permission reaches the conversation");
    let request_id = activity(&asked, "approval.requested")["payload"]["activity"]["payload"]
        ["requestId"]
        .as_str()
        .expect("a request id")
        .to_string();

    // Watching throughout — the tab stays open across the answer, which is the
    // case the close-and-reopen test cannot cover.
    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": thread, "childId": "call_task_1"}),
        )
        .await;
    let opened = inspector.next_chunk(&stream).await;
    inspector.ack(&stream).await;
    let waiting = opened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child opens")["snapshot"]["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["kind"] == "blocker")
        .expect("the child says it is waiting")
        .clone();
    assert_eq!(waiting["payload"]["resolution"], Value::Null);

    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval(&thread, &request_id, "accept"),
        )
        .await
        .expect_success();

    let live = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inspector.values_until(&stream, |item| {
            item["kind"] == "entry-upserted" && item["entry"]["kind"] == "blocker"
        }),
    )
    .await
    .expect("the answer reaches the open tab");
    let resolved = live
        .iter()
        .filter(|item| item["kind"] == "entry-upserted")
        .map(|item| &item["entry"])
        .find(|entry| entry["id"] == waiting["id"])
        .expect("the blocker the developer answered moved")
        .clone();
    assert_eq!(resolved["payload"]["resolution"], "approved");

    release.notify_one();
    client.events_through_the_turn(&subscription).await;
    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// A decision the developer made that never reached the child.
///
/// The conversation and the child's stream have to agree, and the honest thing
/// for both to say is "you decided, and it could not be delivered" — not that
/// the child was told. So the panel clears (the developer has done all they
/// can), the conversation carries the failure, and the child stays **blocked**
/// with its blocker recording `undelivered`. A child left reading "still
/// waiting" beside a conversation reading "approved" would be the two halves of
/// one blocker disagreeing about what happened.
#[tokio::test]
async fn a_decision_that_could_not_be_delivered_says_so_in_both_places() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::refusing_the_decision(release.clone()).await;
    let (_workspace, server, mut client, subscription, thread) =
        a_delegating_turn(&peer, "undeliv").await;

    let asked = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "approval.requested"
        }),
    )
    .await
    .expect("the descendant's permission reaches the conversation");
    let request_id = activity(&asked, "approval.requested")["payload"]["activity"]["payload"]
        ["requestId"]
        .as_str()
        .expect("a request id")
        .to_string();

    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval(&thread, &request_id, "accept"),
        )
        .await
        .expect_success();

    let after = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "approval.resolved"
        }),
    )
    .await
    .expect("the panel clears even though the decision could not be sent");
    // Both halves of the conversation's account: the developer decided, and the
    // agent could not be told.
    assert!(
        harness::conversation::find_activity(&after, "session.failed").is_some(),
        "the conversation never said the decision could not be sent: {:?}",
        harness::conversation::kinds(&after)
    );

    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": thread, "childId": "call_task_1"}),
        )
        .await;
    let reopened = inspector.next_chunk(&stream).await;
    let snapshot = reopened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child opens")["snapshot"]
        .clone();
    let blocker = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["kind"] == "blocker")
        .expect("the child's history explains the pause")
        .clone();
    assert_eq!(
        blocker["payload"]["resolution"], "undelivered",
        "the child's stream still reads as waiting for an answer that will never come: {snapshot:#?}"
    );
    assert_eq!(
        snapshot["stream"]["state"], "blocked",
        "a child nothing reached was reported as working again: {snapshot:#?}"
    );

    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// The legacy permission route is **session-scoped**, and the session it needs
/// is the child's.
///
/// This is the one place a descendant's identity is load-bearing beyond its
/// request id: `POST /session/{id}/permissions/{id}` addressed at the
/// conversation's own session is a reply to a request that session never made.
/// The modern route carries the id alone and cannot show the mistake, so
/// without this the `raised_in` lookup would be untested.
#[tokio::test]
async fn a_legacy_descendant_permission_is_answered_on_the_childs_session() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_blocked_subagent(release.clone(), "legacy").await;
    let (_workspace, server, mut client, subscription, thread) =
        a_delegating_turn(&peer, "legacy").await;

    let asked = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "approval.requested"
        }),
    )
    .await
    .expect("a legacy descendant permission reaches the conversation");
    let request = activity(&asked, "approval.requested")["payload"]["activity"].clone();
    assert_eq!(request["payload"]["subagent"]["childId"], "call_task_1");
    let request_id = request["payload"]["requestId"]
        .as_str()
        .expect("a request id")
        .to_string();

    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval(&thread, &request_id, "accept"),
        )
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "approval.resolved"
        }),
    )
    .await
    .expect("the decision closes the request");

    let requests = peer.requests_through(3).await;
    let reply = requests
        .iter()
        .find(|request| request["operation"] == "permission.reply.legacy")
        .expect("the decision reached OpenCode's session-scoped route");
    assert_eq!(
        reply["sessionId"], "ses_child_1",
        "the descendant's decision was addressed at the conversation's session: {reply}"
    );
    assert_eq!(reply["requestId"], "child-legacy-1");

    release.notify_one();
    client.events_through_the_turn(&subscription).await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// The same route for a descendant's *question*, which OpenCode raises and
/// answers on a different pair of endpoints entirely.
#[tokio::test]
async fn a_descendant_question_is_recorded_in_the_child_and_answered_from_the_conversation() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_blocked_subagent(release.clone(), "question").await;
    let (_workspace, server, mut client, subscription, thread) =
        a_delegating_turn(&peer, "ask").await;

    let (asked, request_id) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.events_until_user_input(&subscription),
    )
    .await
    .expect("a descendant's question reaches the main conversation");
    let question = activity(&asked, "user-input.requested")["payload"]["activity"].clone();
    assert_eq!(question["payload"]["subagent"]["childId"], "call_task_1");
    assert_eq!(question["payload"]["subagent"]["name"], "explore");
    assert_eq!(question["payload"]["questions"][0]["question"], "Count tests too?");

    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_user_input(
                &thread,
                &request_id,
                json!({"question-0-scope": ["Yes"]}),
            ),
        )
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "user-input.resolved"
        }),
    )
    .await
    .expect("the answers close the question in the main conversation");

    let requests = peer.requests_through(3).await;
    let reply = requests
        .iter()
        .find(|request| request["operation"] == "question.reply")
        .expect("the answers reached OpenCode");
    assert_eq!(reply["requestId"], "child-que-1");
    assert_eq!(reply["body"]["answers"], json!([["Yes"]]));

    release.notify_one();
    client.events_through_the_turn(&subscription).await;

    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": thread, "childId": "call_task_1"}),
        )
        .await;
    let reopened = inspector.next_chunk(&stream).await;
    let snapshot = reopened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child reopens")["snapshot"]
        .clone();
    let blocker = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["kind"] == "blocker")
        .expect("the child's history says why it waited")
        .clone();
    assert_eq!(blocker["payload"]["blocker"], "question");
    assert_eq!(blocker["payload"]["resolution"], "answered");

    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// Closing the tab is presentation only: the server goes on recording the child
/// while nobody is watching, and reopening replays what it recorded meanwhile.
///
/// The half of "closing a child tab hides only the view" that a client-state
/// test cannot reach. The right-panel store has no way to send a provider
/// command, which is an argument rather than evidence; this drives the actual
/// unsubscribe against a child that is still working and then asks the server
/// what it has.
#[tokio::test]
async fn closing_a_child_surface_does_not_stop_the_server_recording_it() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_subagent(release.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("child-hidden-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("child-hidden-project", "child-hidden-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("child-hidden-thread").await;
    let mut command = start_turn("child-hidden-thread", "child-hidden-message", "count");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the child is delegated");

    // Open the child, then close the tab while it is still working.
    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-hidden-thread", "childId": "call_task_1"}),
        )
        .await;
    let opened = inspector.next_chunk(&stream).await;
    let watching = opened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child opens")["snapshot"]["entries"]
        .as_array()
        .expect("entries")
        .len();
    inspector.interrupt(&stream).await;

    // Everything the child does from here happens with no surface open.
    release.notify_one();
    client.events_through_the_turn(&subscription).await;

    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-hidden-thread", "childId": "call_task_1"}),
        )
        .await;
    let reopened = inspector.next_chunk(&stream).await;
    let snapshot = reopened
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("the child reopens")["snapshot"]
        .clone();
    let entries = snapshot["entries"].as_array().expect("entries");
    assert!(
        entries.len() > watching,
        "the child stopped being recorded when its tab closed: {watching} entries open, \
         {} after", entries.len()
    );
    assert_eq!(
        snapshot["stream"]["state"], "completed",
        "a child nobody was watching never reached its conclusion: {snapshot:#?}"
    );
    assert_eq!(snapshot["stream"]["outcome"]["text"], "eleven files");

    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// The complete child stream replays after the application restarts.
///
/// Two servers over one database file with nothing shared but the path, which
/// is the shape [`an_owned_opencode_session_is_re_adopted_after_a_server_restart`]
/// uses. This is what exercises `Shell::new`'s restore of the stored streams —
/// a socket test against one running process proves the memory, not the disk.
#[tokio::test]
async fn a_child_work_stream_replays_after_the_server_restarts() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_subagent(release.clone()).await;
    let data = tempfile::tempdir().expect("a data directory");
    let database = data.path().join("registry.sqlite");
    let workspace = Workspace::with(&["src/"]);

    let first = TestServer::start_at_with_config(&database, peer.config(None)).await;
    let mut client = first.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("child-restart-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("child-restart-project", "child-restart-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("child-restart-thread").await;
    let mut command = start_turn("child-restart-thread", "child-restart-message", "count");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the child is delegated");
    release.notify_one();
    client.events_through_the_turn(&subscription).await;
    client.close().await;
    first.stop().await;

    // A second process, which has never seen this child produce anything.
    let second = TestServer::start_at_with_config(&database, peer.config(None)).await;
    let mut reopened = second.connect().await;
    let stream = reopened
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-restart-thread", "childId": "call_task_1"}),
        )
        .await;
    let replayed = reopened.next_chunk(&stream).await;
    let snapshot = replayed
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .expect("a restored child replays")["snapshot"]
        .clone();

    assert_eq!(snapshot["stream"]["name"], "explore");
    assert_eq!(snapshot["stream"]["assignment"], "Count the files");
    assert_eq!(snapshot["stream"]["state"], "completed");
    assert_eq!(snapshot["stream"]["outcome"]["kind"], "completed");
    assert_eq!(snapshot["stream"]["outcome"]["text"], "eleven files");
    let read: Vec<(i64, &str, &str)> = snapshot["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|entry| {
            (
                entry["sequence"].as_i64().expect("a sequence"),
                entry["kind"].as_str().expect("a kind"),
                entry["payload"]["text"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        read,
        vec![
            (1, "message", "looking through the directory"),
            (2, "message", "eleven so far"),
            (3, "outcome", "eleven files"),
        ],
        "the restart did not bring the child's work back in order: {snapshot:#?}"
    );

    reopened.close().await;
    second.stop().await;
    peer.task.abort();
}

/// Removing the conversation removes the work its children did, which is the
/// whole of "retained for as long as its parent thread exists".
#[tokio::test]
async fn deleting_the_parent_thread_deletes_its_child_work_stream() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_subagent(release.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("child-delete-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("child-delete-project", "child-delete-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("child-delete-thread").await;
    let mut command = start_turn("child-delete-thread", "child-delete-message", "count");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the child is delegated");
    release.notify_one();
    client.events_through_the_turn(&subscription).await;

    let mut inspector = server.connect().await;
    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-delete-thread", "childId": "call_task_1"}),
        )
        .await;
    assert_eq!(
        inspector.next_frame_for(&stream).await["_tag"],
        "Chunk",
        "the child stream is there before the conversation is deleted"
    );

    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.delete",
                "commandId": "cmd-delete-child-thread",
                "threadId": "child-delete-thread",
            }),
        )
        .await
        .expect_success();

    let stream = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId": "child-delete-thread", "childId": "call_task_1"}),
        )
        .await;
    let refusal = inspector.next_frame_for(&stream).await;
    assert_eq!(
        refusal["exit"]["cause"][0]["error"]["_tag"],
        "OrchestrationGetSnapshotError",
        "a deleted conversation's child work stream was still readable: {refusal}"
    );

    inspector.close().await;
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn an_owned_opencode_turn_crosses_the_socket_and_reaps_its_server() {
    let SocketTurn {
        _workspace,
        opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(FakeOpenCode::resisting_stop(), "project-1", "thread-open").await;
    let events = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        assistant_sends(&events),
        vec![
            ("hello ".to_string(), true),
            ("from OpenCode".to_string(), true),
            ("hello from OpenCode".to_string(), false),
        ]
    );
    assert!(events
        .iter()
        .any(|item| item["event"]["type"] == "thread.meta-updated"
            && item["event"]["payload"]["title"] == "Upstream title"));
    assert!(events
        .iter()
        .any(|item| item["event"]["payload"]["activity"]["kind"] == "runtime.warning"));
    assert!(events.iter().any(
        |item| item["event"]["payload"]["activity"]["payload"]["thinking"] == "check the stream"
    ));
    let usage = activity(&events, "context-window.updated");
    assert_eq!(usage["payload"]["activity"]["payload"], json!({
        "usedTokens": 21_600,
        "lastUsedTokens": 21_600,
        "totalProcessedTokens": null,
        "maxTokens": 200_000,
        "inputTokens": 21_100,
        "outputTokens": 500,
        "compactsAutomatically": false
    }));
    let snapshot = server.connect().await.into_thread_snapshot("thread-open").await;
    assert!(snapshot["thread"]["activities"].as_array().unwrap().iter().any(|row| {
        row["kind"] == "context-window.updated"
            && row["payload"]["usedTokens"] == 21_600
            && row["payload"]["maxTokens"] == 200_000
            && row["payload"]["compactsAutomatically"] == false
    }));
    assert_eq!(
        events
            .iter()
            .filter(|item| item["event"]["payload"]["activity"]["kind"] == "turn.completed")
            .count(),
        1
    );
    let settled = events
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("ready session event");
    assert_eq!(settled["event"]["payload"]["session"]["status"], "ready");
    let requests = opencode.requests().await;
    assert_eq!(
        requests.iter().map(|request| request["operation"].as_str().unwrap()).collect::<Vec<_>>(),
        vec!["launch", "mcp.add", "create", "prompt"],
        "owned OpenCode connects Laplus MCP before its session and first prompt"
    );
    assert_eq!(requests[1]["authorizationPresent"], true);
    assert_eq!(requests[1]["oauth"], false);
    assert!(!serde_json::to_string(&requests).unwrap().contains("Bearer "));
    assert_eq!(server.live_mcp_sessions(), 1);
    assert_eq!(requests[3]["body"]["parts"][0]["text"], "say hello");
    assert_eq!(
        requests[3]["body"]["model"],
        json!({"providerID":"openai","modelID":"gpt-5"})
    );
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type":"thread.session.stop",
                "commandId":"test:stop:thread-open",
                "threadId":"thread-open",
                "createdAt":"2026-08-01T00:00:00.000Z"
            }),
        )
        .await
        .expect_success();
    wait_until_port_closes(
        port,
        "the stop command did not reap the owned OpenCode server",
    )
    .await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while server.live_mcp_sessions() != 0 { tokio::task::yield_now().await; }
    })
    .await
    .expect("the owned MCP session is released with its server");
    assert_eq!(server.live_mcp_sessions(), 0);
    client.close().await;
    server.stop().await;
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "stopping Laplus reaps the owned OpenCode server"
    );
}

#[tokio::test]
async fn an_owned_runaway_is_killed_once_and_the_follow_up_resumes_its_session() {
    let SocketTurn {
        _workspace,
        opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(FakeOpenCode::runaway(), "runaway-project", "runaway-thread").await;
    let before = client.events_until_streaming(&subscription).await;
    let turn_id = last_session(&before, "owned runaway before stop")["payload"]["session"]
        ["activeTurnId"]
        .as_str()
        .unwrap()
        .to_string();
    let first_port = opencode.requests_through(4).await[0]["port"]
        .as_u64()
        .expect("owned runaway port") as u16;

    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("runaway-thread", Some(&turn_id)),
        )
        .await
        .expect_success();

    client.values_until(&subscription, |item| {
        item["event"]["type"] == "thread.turn-interrupt-requested"
    }).await;
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while tokio::net::TcpStream::connect(("127.0.0.1", first_port))
            .await
            .is_ok()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the escalation kills the owned runaway");
    let interrupted = server.connect().await.into_thread_snapshot("runaway-thread").await;
    assert_eq!(interrupted["thread"]["latestTurn"]["state"], "interrupted");
    assert_eq!(
        interrupted["thread"]["activities"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["kind"] == "turn.interrupted")
            .count(),
        1,
        "owned escalation settles the stopped turn exactly once"
    );
    let child = child_row(&server, "runaway-thread", "call_task_1").await;
    assert_eq!(child["payload"]["status"], "stopped");
    assert_eq!(child["payload"]["detail"], "Interrupted");

    let mut follow_up = follow_up("runaway-thread", "runaway-follow-up", "continue");
    follow_up["modelSelection"] = json!({"instanceId":"openLocal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", follow_up)
        .await
        .expect_success();
    let requests = opencode.requests_through(14).await;
    let restart = requests
        .iter()
        .rposition(|request| request["operation"] == "launch")
        .expect("the owned peer relaunched");
    assert!(restart > 0, "the first launch was mistaken for a restart: {requests:#?}");
    let after_restart = &requests[restart..];
    assert!(after_restart.iter().any(|request| request["operation"] == "get"), "restart did not adopt the durable session: {requests:#?}");
    assert!(after_restart.iter().any(|request| request["operation"] == "prompt"), "follow-up did not reach restarted peer: {requests:#?}");
    assert_eq!(
        after_restart
            .iter()
            .find(|request| request["operation"] == "get")
            .unwrap()["sessionId"],
        "ses_owned_1",
        "the restarted peer resumes by durable provider session id"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn an_owned_turn_waits_for_a_catalogue_that_populates_after_health_is_ready() {
    let SocketTurn {
        _workspace,
        opencode: _opencode,
        server,
        mut client,
        subscription,
    } = start_socket_turn(
        FakeOpenCode::delayed_catalogue(),
        "delayed-catalogue-project",
        "delayed-catalogue-thread",
    )
    .await;

    let events = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        activity(&events, "context-window.updated")["payload"]["activity"]["payload"]
            ["maxTokens"],
        200_000,
        "the first two empty catalogues are retried before the turn opens"
    );

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn stopping_busy_owned_opencode_aborts_and_reaps_its_server() {
    let SocketTurn {
        _workspace,
        opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(
        FakeOpenCode::busy(),
        "project-busy-stop",
        "thread-busy-stop",
    )
    .await;
    client.events_until_streaming(&subscription).await;
    let requests = opencode.requests().await;
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type":"thread.session.stop","commandId":"test:stop:busy-owned",
                "threadId":"thread-busy-stop","createdAt":"2026-08-01T00:00:00.000Z"
            }),
        )
        .await
        .expect_success();
    let requests = opencode.requests_through(5).await;
    assert!(requests
        .iter()
        .any(|request| request["operation"] == "abort"));
    wait_until_port_closes(
        port,
        "stopping a busy owned session did not reap its OpenCode server",
    )
    .await;
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn an_owned_server_that_exits_during_startup_becomes_a_visible_session_failure() {
    let SocketTurn {
        _workspace,
        opencode: _opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(FakeOpenCode::exiting(), "project-1", "thread-failed").await;
    let events = client.events_through_the_turn(&subscription).await;
    let failed = events
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("failed session event");
    assert_eq!(failed["event"]["payload"]["session"]["status"], "error");
    assert!(failed["event"]["payload"]["session"]["lastError"]
        .as_str()
        .expect("visible startup failure")
        .contains("exited before becoming ready"));
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn owned_mcp_registration_failure_is_visible_and_releases_the_grant() {
    let SocketTurn { _workspace, opencode, server, mut client, subscription, .. } =
        start_socket_turn(FakeOpenCode::mcp_failure(), "project-mcp-failed", "thread-mcp-failed").await;
    let events = client.events_through_the_turn(&subscription).await;
    let failed = events.iter().rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("failed session event");
    assert_eq!(failed["event"]["payload"]["session"]["status"], "error");
    assert!(failed["event"]["payload"]["session"]["lastError"].as_str().unwrap()
        .contains("OpenCode MCP registration failed"));
    let requests = opencode.requests_through(2).await;
    assert_eq!(requests.iter().map(|row| row["operation"].as_str().unwrap()).collect::<Vec<_>>(), vec!["launch", "mcp.add"]);
    assert_eq!(server.live_mcp_sessions(), 0);
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn an_owned_server_readiness_timeout_becomes_a_visible_session_failure() {
    let SocketTurn {
        _workspace,
        opencode: _opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(FakeOpenCode::never_ready(), "project-timeout", "thread-timeout").await;
    let events = client.events_through_the_turn(&subscription).await;
    let failed = events
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("timed-out session event");
    assert_eq!(failed["event"]["payload"]["session"]["status"], "error");
    assert!(failed["event"]["payload"]["session"]["lastError"]
        .as_str()
        .expect("visible startup timeout")
        .contains("did not become ready within 30 seconds"));
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn server_shutdown_reaps_a_live_owned_opencode_server() {
    let SocketTurn {
        _workspace,
        opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(FakeOpenCode::new(), "project-shutdown", "thread-shutdown").await;
    client.events_through_the_turn(&subscription).await;
    let requests = opencode.requests().await;
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    client.close().await;
    tokio::time::timeout(std::time::Duration::from_secs(5), server.stop())
        .await
        .expect("server shutdown completes while OpenCode is live");
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "server shutdown reaps the owned OpenCode process"
    );
}

#[tokio::test]
async fn project_closure_reaps_its_threads_live_owned_opencode_server() {
    let SocketTurn {
        _workspace,
        opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(FakeOpenCode::new(), "project-delete", "thread-delete").await;
    client.events_through_the_turn(&subscription).await;
    let requests = opencode.requests().await;
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    client
        .call(
            "orchestration.dispatchCommand",
            json!({"type":"project.delete","commandId":"test:delete:project-delete","projectId":"project-delete"}),
        )
        .await
        .expect_success();
    wait_until_port_closes(
        port,
        "project closure did not reap the thread's owned OpenCode server",
    )
    .await;
    client.close().await;
    server.stop().await;
}

/// **Ticket 06.** Stopping the parent stops the delegation tree, and the tree is
/// what the conversation reports itself working on.
///
/// Four things at one seam, and they are one behaviour. The child is genuinely
/// mid-flight — the scripted peer holds its session open until this test lets it
/// go — so the stop lands on live work rather than on a child that had already
/// finished:
///
/// - the developer's stop reaches every known active descendant, not only the
///   root agent;
/// - each of those records an **interrupted** terminal state and the terminal
///   entry that says so, which is what makes the ending auditable rather than a
///   stream that simply goes quiet;
/// - the compact row in the parent's transcript says it too, drawn on the
///   developer's own command because no provider will ever draw it;
/// - nothing ordinary reaches the child afterwards: the peer is released and
///   goes on narrating a child laplus has already ended, and none of it lands —
///   neither in the stream nor on the row;
/// - and with nothing left in the tree the conversation stops reporting itself
///   as working.
#[tokio::test]
async fn stopping_the_parent_stops_its_delegation_tree() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_subagent(release.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("stop-tree-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("stop-tree-project", "stop-tree-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("stop-tree-thread").await;
    let mut command = start_turn("stop-tree-thread", "stop-tree-message", "count");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the child is delegated");

    let working = child_stream(&server, "stop-tree-thread", "call_task_1").await;
    assert_eq!(
        working["stream"]["state"], "working",
        "the child had already finished, so nothing here is about stopping one: {working:#?}"
    );
    let before = working["entries"].as_array().expect("entries").len();

    // The composer's stop button, with no turn named — which is what it sends.
    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("stop-tree-thread", None),
        )
        .await
        .expect_success();

    let stopped = child_stream(&server, "stop-tree-thread", "call_task_1").await;
    assert_eq!(stopped["stream"]["state"], "interrupted");
    assert_eq!(stopped["stream"]["outcome"]["kind"], "interrupted");
    let terminal = stopped["entries"]
        .as_array()
        .expect("entries")
        .last()
        .expect("a terminal entry")
        .clone();
    assert_eq!(
        terminal["kind"], "outcome",
        "the interruption is the last entry of the child's own stream: {stopped:#?}"
    );
    assert_eq!(terminal["payload"]["kind"], "interrupted");
    let interrupted_at = stopped["entries"].as_array().expect("entries").len();
    assert_eq!(interrupted_at, before + 1, "{stopped:#?}");

    // The compact row says the same thing, on the same command. It is the
    // surface the developer is more likely to be reading — the child's tab may
    // never have been opened — and no provider will ever draw this row, because
    // no provider is told its subagent was abandoned.
    let ended = child_row(&server, "stop-tree-thread", "call_task_1").await;
    assert_eq!(
        ended["payload"]["status"], "stopped",
        "the compact row disagreed with the stopped child's stream: {ended:#?}"
    );
    assert_eq!(ended["kind"], "tool.completed", "{ended:#?}");
    assert_eq!(
        ended["payload"]["data"]["toolCallId"], "subagent:call_task_1",
        "the ending landed beside the child's row instead of on it: {ended:#?}"
    );
    assert_eq!(
        ended["payload"]["detail"], "Interrupted",
        "the row kept the line the child was on when it was stopped: {ended:#?}"
    );

    // And the provider goes on narrating a child that has already ended.
    release.notify_one();
    client.events_through_the_turn(&subscription).await;

    let after = child_stream(&server, "stop-tree-thread", "call_task_1").await;
    assert_eq!(
        after["entries"], stopped["entries"],
        "an interrupted child went on taking live work: {after:#?}"
    );
    assert_eq!(after["stream"]["state"], "interrupted");
    assert_eq!(after["stream"]["outcome"]["kind"], "interrupted");

    // None of it moves the row either, which is the half that produced the
    // worst outcome: a row settling on the answer the developer declined to
    // wait for, beside a stream that says it was interrupted.
    let after_row = child_row(&server, "stop-tree-thread", "call_task_1").await;
    assert_eq!(
        after_row, ended,
        "narration after a Stop moved the stopped child's row: {after_row:#?}"
    );

    // Nothing is left in the tree, so nothing is holding the conversation open.
    let session = server
        .connect()
        .await
        .into_thread_snapshot("stop-tree-thread")
        .await["thread"]["session"]
        .clone();
    assert_ne!(
        session["status"], "running",
        "a stopped delegation tree still reports the conversation as working: {session:#?}"
    );

    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// **Ticket 06.** The conversation reports itself working while a descendant is,
/// and stops only when the last of them is terminal.
///
/// The sidebar draws *Working* from one thing — a session whose status is
/// `running` (`Sidebar.logic.ts`, `resolveThreadStatusPill`) — so this asserts
/// exactly that, at the two moments it has to be true and false. What makes it a
/// claim about the *tree* rather than about the root is the middle: the root's
/// own turn has settled and no turn is in flight, and the conversation is still
/// working because the child is.
#[tokio::test]
async fn the_conversation_stays_working_while_its_child_does() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_subagent(release.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("tree-working-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("tree-working-project", "tree-working-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("tree-working-thread").await;
    let mut command = start_turn("tree-working-thread", "tree-working-message", "count");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the child is delegated");

    // The child is still working, and the conversation says so.
    let working = child_stream(&server, "tree-working-thread", "call_task_1").await;
    assert_eq!(working["stream"]["state"], "working");
    let session = server
        .connect()
        .await
        .into_thread_snapshot("tree-working-thread")
        .await["thread"]["session"]
        .clone();
    assert_eq!(session["status"], "running", "{session:#?}");

    // Let it finish. The conversation leaves Working with the last descendant.
    release.notify_one();
    client.events_through_the_turn(&subscription).await;
    let done = child_stream(&server, "tree-working-thread", "call_task_1").await;
    assert_eq!(done["stream"]["state"], "completed");
    let quiet = server
        .connect()
        .await
        .into_thread_snapshot("tree-working-thread")
        .await["thread"]["session"]
        .clone();
    assert_ne!(
        quiet["status"], "running",
        "every descendant is terminal and the conversation is still working: {quiet:#?}"
    );

    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// **Ticket 06.** Ending the session ends the tree it was running, too.
///
/// `thread.session.stop` is not the composer's stop button — it ends the agent
/// *process* and keeps the conversation, and the real client reaches for it
/// before deleting a thread and before moving a worktree (`useThreadActions.ts`,
/// `BranchToolbarBranchSelector.tsx`). It is included in the delegation tree's
/// Stop for the reason the tree is stopped at all: when the process behind a
/// child is gone, nothing will ever report on that child again, so a stream left
/// at `working` is a claim no later event can correct. Recording the
/// interruption is what keeps the ending auditable and stops the conversation
/// reporting itself as working for ever.
///
/// This is the one Stop path the criteria do not name, so it is asserted rather
/// than assumed.
#[tokio::test]
async fn ending_the_session_ends_the_delegation_tree_with_it() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::spawning_a_subagent(release.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(peer.config(None)).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("stop-session-project", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("stop-session-project", "stop-session-thread");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("stop-session-thread").await;
    let mut command = start_turn("stop-session-thread", "stop-session-message", "count");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        client.values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "call_task_1"
        }),
    )
    .await
    .expect("the child is delegated");
    let working = child_stream(&server, "stop-session-thread", "call_task_1").await;
    assert_eq!(working["stream"]["state"], "working");

    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.session.stop",
                "commandId": "test:stop-session",
                "threadId": "stop-session-thread",
                "createdAt": "2026-08-18T00:00:00.000Z",
            }),
        )
        .await
        .expect_success();

    let stopped = child_stream(&server, "stop-session-thread", "call_task_1").await;
    assert_eq!(stopped["stream"]["state"], "interrupted");
    assert_eq!(stopped["stream"]["outcome"]["kind"], "interrupted");
    assert_eq!(
        stopped["entries"]
            .as_array()
            .expect("entries")
            .last()
            .expect("a terminal entry")["kind"],
        "outcome"
    );

    release.notify_one();
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// The assistant transcript as a client that takes a snapshot sees it: one row
/// per message, in stored order.
fn assistant_texts(snapshot: &Value) -> Vec<String> {
    snapshot["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|message| message["text"].as_str().unwrap_or("").to_string())
        .collect()
}

/// Whether the assistant row reading `text` renders below the work row for
/// `call_id`.
///
/// Not a question about the transcript array: messages and work rows are two
/// lists in the snapshot, and the client interleaves them itself.
/// `deriveTimelineEntries` (`apps/web/src/session-logic.ts`) concatenates
/// message rows, proposed plans and work rows and sorts the lot by `createdAt`,
/// so on-screen placement of a message *relative to a tool call* is that one
/// comparison and nothing else — which is why this reads the persisted snapshot
/// rather than the event log. The log is in arrival order, where a row the
/// merge invents seconds after the tool activity is necessarily last however
/// the merge behaved.
fn reads_below_the_tool_row(snapshot: &Value, call_id: &str, text: &str) -> bool {
    let created_at = |row: &Value, what: &str| {
        row["createdAt"]
            .as_str()
            .unwrap_or_else(|| panic!("{what} carries the createdAt the client sorts by"))
            .to_string()
    };
    let tool_row = snapshot["thread"]["activities"]
        .as_array()
        .expect("the work log")
        .iter()
        .find(|activity| activity["payload"]["data"]["toolCallId"] == call_id)
        .unwrap_or_else(|| panic!("no work row for the {call_id} tool call"));
    let message_row = snapshot["thread"]["messages"]
        .as_array()
        .expect("the transcript")
        .iter()
        .find(|message| message["role"] == "assistant" && message["text"] == text)
        .unwrap_or_else(|| panic!("no assistant row reading {text:?}"));
    created_at(message_row, "an assistant row") > created_at(tool_row, "a work row")
}

async fn start_text_parts_turn(
    peer: &ExternalOpenCode,
    suffix: &str,
) -> (Workspace, TestServer, SocketClient, String) {
    start_text_parts_turn_at(peer, suffix, None).await
}

/// The same, optionally on a database that outlives the server, so the settled
/// transcript can be read again after a full reload.
async fn start_text_parts_turn_at(
    peer: &ExternalOpenCode,
    suffix: &str,
    database: Option<&Path>,
) -> (Workspace, TestServer, SocketClient, String) {
    let workspace = Workspace::with(&["src/"]);
    let config = peer.config(None);
    let server = match database {
        Some(database) => TestServer::start_at_with_config(database, config).await,
        None => TestServer::start_with(config).await,
    };
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project(&format!("parts-project-{suffix}"), workspace.path()),
        )
        .await
        .expect_success();
    let thread = format!("parts-thread-{suffix}");
    let mut create = create_thread(&format!("parts-project-{suffix}"), &thread);
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation(&thread).await;
    let mut command = start_turn(&thread, &format!("parts-message-{suffix}"), "look around");
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    (workspace, server, client, subscription)
}

/// Text spoken between tool calls reads between the tool rows — each provider
/// text part its own assistant message, in speech order, live and after a full
/// reload.
///
/// The scripted turn says something, runs a tool, thinks, then says more; it
/// also resends its second part's snapshot twice more than needed (a duplicate
/// and a stale regression), announces a third part and never fills it. What a
/// client may observe is exactly two messages with exactly the spoken texts,
/// the tool rows between their first appearances, the reasoning in the work log
/// and nowhere else.
#[tokio::test]
async fn opencode_text_parts_read_between_the_tool_rows_live_and_after_a_reload() {
    let peer = ExternalOpenCode::narrating_in_text_parts(None).await;
    let workspace = Workspace::with(&["src/"]);
    let registry = tempfile::tempdir().unwrap();
    let database = registry.path().join("registry.sqlite");
    let config = peer.config(None);
    let server = TestServer::start_at_with_config(&database, config.clone()).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("parts-project-interleave", workspace.path()),
        )
        .await
        .expect_success();
    let mut create = create_thread("parts-project-interleave", "parts-thread-interleave");
    create["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", create)
        .await
        .expect_success();
    let subscription = client.watch_conversation("parts-thread-interleave").await;
    let mut command = start_turn(
        "parts-thread-interleave",
        "parts-message-interleave",
        "look around",
    );
    command["modelSelection"] = json!({"instanceId":"openExternal","model":"openai/gpt-5"});
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    // Live, as a folding client saw it: each part streams under its own id
    // (each event carries only that event's suffix), then both close in
    // first-emission order. The duplicate and stale snapshots and the empty
    // third part are invisible here.
    assert_eq!(
        assistant_sends(&events),
        vec![
            ("Reading the tree first. ".to_string(), true),
            ("The tree holds ".to_string(), true),
            ("eleven files.".to_string(), true),
            ("Reading the tree first. ".to_string(), false),
            ("The tree holds eleven files.".to_string(), false),
        ]
    );

    // Placement: the first part opened before the tool row and the second part
    // only after the tool had finished, so text spoken after the call reads
    // below it.
    let position = |wanted: &dyn Fn(&Value) -> bool| {
        events
            .iter()
            .position(|item| wanted(item))
            .expect("the event a placement assertion turns on")
    };
    let first_part = position(&|item| {
        item["event"]["type"] == "thread.message-sent"
            && item["event"]["payload"]["role"] == "assistant"
    });
    let tool_row = position(&|item| {
        item["event"]["payload"]["activity"]["payload"]["data"]["toolCallId"] == "call-parts-1"
    });
    let second_part = position(&|item| {
        item["event"]["type"] == "thread.message-sent"
            && item["event"]["payload"]["role"] == "assistant"
            && item["event"]["payload"]["text"] == "The tree holds "
    });
    assert!(first_part < tool_row, "the first part precedes the tool");
    assert!(tool_row < second_part, "the tool precedes the second part");

    // Reasoning stays a work-log entry: said once, and never as a bubble.
    assert_eq!(
        events
            .iter()
            .filter(|item| item["event"]["payload"]["activity"]["payload"]["thinking"]
                == "weighing what changed")
            .count(),
        1
    );
    assert!(!events.iter().any(|item| {
        item["event"]["type"] == "thread.message-sent"
            && item["event"]["payload"]["text"]
                .as_str()
                .is_some_and(|text| text.contains("weighing"))
    }));

    let thread = "parts-thread-interleave";
    let settled = server.connect().await.into_thread_snapshot(thread).await;
    assert_eq!(
        assistant_texts(&settled),
        vec![
            "Reading the tree first. ".to_string(),
            "The tree holds eleven files.".to_string(),
        ],
        "both parts survive settlement as their own messages, nothing doubled, no empty bubble"
    );
    client.close().await;
    server.stop().await;

    // A full reload reads the same transcript in the same order.
    let restarted = TestServer::start_at_with_config(&database, config).await;
    let reloaded = restarted
        .connect()
        .await
        .into_thread_snapshot("parts-thread-interleave")
        .await;
    assert_eq!(assistant_texts(&reloaded), assistant_texts(&settled));
    restarted.stop().await;
    peer.task.abort();
}

/// Interrupting mid-turn closes every open part with whatever it held when the
/// stop landed — the first part whole, the second part at exactly the words
/// that had arrived, and nothing invented afterwards.
///
/// The gate is held *through* the ending: no idle ever confirms quiet, so it is
/// the bounded interrupt reconciliation that proves the outcome and closes the
/// partials from per-part state alone. What the gate releases afterwards is
/// late output — the second part's completion, a duplicate and a stale snapshot,
/// two idles — and none of it may revise a settled turn.
#[tokio::test]
async fn interrupting_opencode_keeps_each_partial_text_part_exactly_as_it_arrived() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::narrating_in_text_parts(Some(Arc::clone(&idle_release))).await;
    let (_workspace, server, mut client, subscription) =
        start_text_parts_turn(&peer, "partial").await;
    let before = client.events_until_streaming(&subscription).await;
    let turn_id = last_session(&before, "running the partial turn")["payload"]["session"]
        ["activeTurnId"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("parts-thread-partial", Some(&turn_id)),
        )
        .await
        .expect_success();

    // A stop request is not proof that OpenCode stopped. Until message-history
    // verification observes a bounded quiet window, the turn remains active
    // and the work log says that verification is in progress.
    let stopping = server
        .connect()
        .await
        .into_thread_snapshot("parts-thread-partial")
        .await;
    assert_eq!(stopping["thread"]["latestTurn"]["state"], "running");
    assert_eq!(stopping["thread"]["session"]["status"], "running");
    let activities = stopping["thread"]["activities"]
        .as_array()
        .expect("the durable work log");
    assert!(activities.iter().any(|activity| activity["kind"] == "turn.stopping"));
    assert!(!activities.iter().any(|activity| activity["kind"] == "turn.interrupted"));

    // The gate holds: no idle, no completion. The bounded reconciliation is
    // what settles the turn — asked for, answered by the provider going silent,
    // and closed out of what had actually streamed.
    let after = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        after
            .iter()
            .filter(|item| item["event"]["payload"]["activity"]["kind"] == "turn.completed")
            .count(),
        1
    );
    // create, prompt, abort, then two message snapshots. Status is
    // intentionally not consulted as proof of quiescence.
    let requests = peer.requests_through(5).await;
    assert!(requests.iter().any(|request| request["operation"] == "abort"));
    assert!(requests.iter().any(|request| request["operation"] == "messages"));
    let interrupted = server
        .connect()
        .await
        .into_thread_snapshot("parts-thread-partial")
        .await;
    assert_eq!(interrupted["thread"]["latestTurn"]["state"], "interrupted");
    assert_eq!(interrupted["thread"]["session"]["status"], "interrupted");
    assert_eq!(interrupted["thread"]["session"]["lastError"], Value::Null);
    assert_eq!(
        assistant_texts(&interrupted),
        vec![
            "Reading the tree first. ".to_string(),
            // Exactly the delta that had arrived; the completion behind the gate
            // was never spoken before the stop, so no closing may claim it.
            "The tree holds ".to_string(),
        ]
    );

    // Late output arrives now, after settlement. The drain marker proves every
    // queued event was delivered; nothing may reopen or rewrite the transcript.
    idle_release.notify_one();
    client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.meta-updated"
                && item["event"]["payload"]["title"] == "Late marker"
        })
        .await;
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let after_the_fact = server
        .connect()
        .await
        .into_thread_snapshot("parts-thread-partial")
        .await;
    assert_eq!(
        after_the_fact["thread"]["latestTurn"]["state"],
        "interrupted"
    );
    assert_eq!(assistant_texts(&after_the_fact), assistant_texts(&interrupted));
    client.close().await;
    server.stop().await;
    peer.task.abort();
}

/// A recovered transcript reads like a live one: the block that was cut off
/// closes with the rest of what it said, the blocks the stream never delivered
/// arrive as rows of their own in provider order below the tool call, and what
/// was already on screen is left exactly as the developer read it.
///
/// The stream stops after the second block's first delta and no idle ever
/// arrives, so the bounded interrupt reconciliation is the only thing that can
/// account for anything beyond it. The history it reads holds more of the turn
/// than the stream delivered — and disagrees with the stream about the first
/// block, which is the one thing it is not allowed to act on.
#[tokio::test]
async fn opencode_reconcile_lands_a_lost_suffix_in_its_own_rows_below_the_tool() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::narrating_past_a_lost_suffix(Arc::clone(&idle_release)).await;
    let registry = tempfile::tempdir().unwrap();
    let database = registry.path().join("registry.sqlite");
    let (_workspace, server, mut client, subscription) =
        start_text_parts_turn_at(&peer, "lost-suffix", Some(&database)).await;
    let before = client.events_until_streaming(&subscription).await;
    let turn_id = last_session(&before, "running the turn whose stream was lost")["payload"]
        ["session"]["activeTurnId"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("parts-thread-lost-suffix", Some(&turn_id)),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    // The identity the merge turns on: the cut-off block's message, minted at
    // its first delta while the stream was alive.
    let partial = events
        .iter()
        .find(|item| {
            item["event"]["type"] == "thread.message-sent"
                && item["event"]["payload"]["text"] == "The tree holds "
        })
        .expect("the block that was streaming when the stream died")["event"]["payload"]
        ["messageId"]
        .as_str()
        .expect("the streamed block's message id")
        .to_string();

    let interrupted = server
        .connect()
        .await
        .into_thread_snapshot("parts-thread-lost-suffix")
        .await;
    assert_eq!(interrupted["thread"]["latestTurn"]["state"], "interrupted");
    assert_eq!(
        assistant_texts(&interrupted),
        vec![
            // Byte-identical to what was on screen. History reads
            // "Reading the forest first. " for this part; a snapshot that
            // disagrees may not retract words the developer has already read.
            "Reading the tree first. ".to_string(),
            // Extended by exactly the suffix the stream never delivered.
            "The tree holds eleven files.".to_string(),
            // Never streamed at all: each its own row, in provider order.
            "Then I looked again.".to_string(),
            "Nothing else to add.".to_string(),
        ],
        "the recovered turn reads as the provider spoke it"
    );
    let rows = interrupted["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "assistant")
        .map(|message| {
            (
                message["id"].as_str().unwrap().to_string(),
                message["text"].as_str().unwrap().to_string(),
            )
        })
        .collect::<Vec<_>>();
    let cut_off = rows.get(1).unwrap_or_else(|| {
        panic!("the merge left the transcript as {rows:?}, which has no second assistant row")
    });
    assert_eq!(
        *cut_off,
        (partial, "The tree holds eleven files.".to_string()),
        "the cut-off block closes under the identity it streamed with"
    );

    // Placement, read the way the client lays it out rather than the way the
    // events happened to arrive. `deriveTimelineEntries`
    // (`apps/web/src/session-logic.ts`) sorts message rows and work rows
    // together by `createdAt`, so "below the tool call" is a claim about that
    // timestamp on the persisted snapshot. It could not be one about arrival
    // order: the merge invents these rows seconds after the tool activity, so
    // in the event log they cannot be anywhere but last, whatever the code did.
    //
    // Between themselves the two recovered rows are minted in one pass and can
    // share a millisecond, where the client's sort is stable and falls back to
    // the order the snapshot lists them in — which is the order
    // `assistant_texts` above already fixes, and which the reload below fixes
    // again.
    for recovered in ["Then I looked again.", "Nothing else to add."] {
        assert!(
            reads_below_the_tool_row(&interrupted, "call-parts-1", recovered),
            "a block spoken after the tool call reads below it: {recovered:?}"
        );
    }

    client.close().await;
    server.stop().await;

    // Ordinals: a full reload reads the recovered transcript in the live order,
    // tool row included — placement that only holds until the window closes is
    // not placement.
    let restarted = TestServer::start_at_with_config(&database, peer.config(None)).await;
    let reloaded = restarted
        .connect()
        .await
        .into_thread_snapshot("parts-thread-lost-suffix")
        .await;
    assert_eq!(assistant_texts(&reloaded), assistant_texts(&interrupted));
    for recovered in ["Then I looked again.", "Nothing else to add."] {
        assert!(
            reads_below_the_tool_row(&reloaded, "call-parts-1", recovered),
            "a reloaded block still reads below the tool call: {recovered:?}"
        );
    }
    restarted.stop().await;
    peer.task.abort();
}

#[tokio::test]
async fn an_external_runaway_is_reported_once_and_remains_supervised_without_a_kill() {
    let release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::changing_output_during_stop(Arc::clone(&release)).await;
    let (_workspace, server, mut client, subscription) =
        start_text_parts_turn(&peer, "stop-pause").await;
    let before = client.events_until_streaming(&subscription).await;
    let turn_id = last_session(&before, "running turn before stop verification")["payload"]
        ["session"]["activeTurnId"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("parts-thread-stop-pause", Some(&turn_id)),
        )
        .await
        .expect_success();

    // create, prompt, abort, and two identical message snapshots. The old
    // point-sample policy settled here even though the provider resumes output
    // in its next authoritative history response.
    peer.requests_through(5).await;
    let paused = server
        .connect()
        .await
        .into_thread_snapshot("parts-thread-stop-pause")
        .await;
    assert_eq!(paused["thread"]["latestTurn"]["state"], "running");
    assert_eq!(paused["thread"]["session"]["status"], "running");

    client.values_until(&subscription, |item| {
        item["event"]["payload"]["activity"]["kind"]
            == "turn.interrupt-verification-failed"
    }).await;
    // One more authoritative snapshot after the visible failure proves the
    // ladder remains armed and supervising rather than ending the loop there.
    peer.requests_through(9).await;
    let supervised = server
        .connect()
        .await
        .into_thread_snapshot("parts-thread-stop-pause")
        .await;
    assert_eq!(supervised["thread"]["latestTurn"]["state"], "running");
    assert_eq!(supervised["thread"]["session"]["status"], "running");
    let failures = supervised["thread"]["activities"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|activity| activity["kind"] == "turn.interrupt-verification-failed")
        .collect::<Vec<_>>();
    assert_eq!(failures.len(), 1, "external escalation reports the runaway once");
    let diagnostic = failures[0].to_string();
    for expected in ["OpenCode ignored the stop request", "openExternal", "ses_owned_1", "escalated", "last message count 2"] {
        assert!(diagnostic.contains(expected), "missing {expected:?}: {diagnostic}");
    }
    assert!(
        tokio::net::TcpStream::connect(peer.endpoint.trim_start_matches("http://"))
            .await
            .is_ok(),
        "external escalation never kills the operator-owned peer"
    );

    client.call("orchestration.dispatchCommand", json!({
        "type":"thread.session.stop",
        "commandId":"test:stop:external-runaway",
        "threadId":"parts-thread-stop-pause",
        "createdAt":"2026-08-22T00:00:00.000Z"
    })).await.expect_success();
    release.notify_one();
    client.close().await;
    server.stop().await;
    peer.task.abort();
}
