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
        assistant_sends, create_project, create_thread, follow_up, interrupt_turn, last_session,
        start_turn,
    },
    workspace::Workspace,
    SocketClient, TestServer,
};
use laplus_server::config::ServerConfig;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

struct FakeOpenCode {
    directory: tempfile::TempDir,
    log: PathBuf,
}

#[derive(Clone, Copy)]
enum Startup {
    Healthy,
    Gated,
    ResistsStop,
    Exit,
    NeverReady,
}

impl FakeOpenCode {
    fn new() -> Self {
        Self::scripted(Startup::Healthy)
    }

    fn exiting() -> Self {
        Self::scripted(Startup::Exit)
    }

    fn resisting_stop() -> Self {
        Self::scripted(Startup::ResistsStop)
    }

    fn busy() -> Self {
        Self::scripted(Startup::Gated)
    }

    fn never_ready() -> Self {
        Self::scripted(Startup::NeverReady)
    }

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
            (Startup::Healthy, true) => "set OPENCODE_TEST_PORT=%3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::Gated, true) => "set OPENCODE_TEST_PORT=%3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\nset OPENCODE_TEST_GATED=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::ResistsStop, true) => "set OPENCODE_TEST_PORT=%3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::NeverReady, true) => "set OPENCODE_TEST_PORT=%3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=false\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::Healthy, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::Gated, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true OPENCODE_TEST_GATED=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::ResistsStop, false) => "trap '' TERM\nOPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
            (Startup::NeverReady, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=false exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
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
        self.requests_through(3).await
    }

    async fn requests_through(&self, count: usize) -> Vec<Value> {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&self.log) {
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

#[derive(Clone, Default)]
struct PeerState {
    subscriber: Arc<Mutex<Option<mpsc::Sender<Result<&'static str, Infallible>>>>>,
    log: Arc<PathBuf>,
    healthy: bool,
    authorization: Option<String>,
    idle_release: Option<Arc<Notify>>,
    prompts: Arc<AtomicUsize>,
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

async fn events(State(state): State<PeerState>, headers: HeaderMap) -> Response {
    assert_authorization(&headers, &state);
    let (tx, rx) = mpsc::channel(8);
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
    Json(json!({"id":"ses_owned_1"}))
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
        if let Some(release) = &state.idle_release {
            release.notify_one();
        }
        return StatusCode::NO_CONTENT;
    }
    let sender = state
        .subscriber
        .lock()
        .expect("subscriber lock")
        .clone()
        .expect("event subscription precedes prompt");
    for event in [
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"reason-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"reasoning\",\"text\":\"check the stream\"}}}\n\n",
        "data: {\"type\":\"message.updated\",\"properties\":{\"info\":{\"id\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"role\":\"assistant\"}}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"text-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"\"}}}\n\n",
        "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"busy\"}}}\n\n",
        "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"messageID\":\"message-1\",\"partID\":\"text-1\",\"field\":\"text\",\"delta\":\"hello \"}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"text-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"hello from OpenCode\"}}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"id\":\"text-1\",\"messageID\":\"message-1\",\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"hello\"}}}\n\n",
        "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"retry\",\"message\":\"Retrying upstream\"}}}\n\n",
        "data: {\"type\":\"session.updated\",\"properties\":{\"info\":{\"id\":\"ses_owned_1\",\"title\":\"Upstream title\"}}}\n\n",
        "data: {\"type\":\"future.event\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n",
    ] {
        sender.send(Ok(event)).await.expect("send scripted SSE event");
    }
    let gated = state.idle_release.is_some();
    let finish = async move {
        if let Some(release) = &state.idle_release {
            release.notified().await;
        }
        for event in [
            "data: {\"type\":\"session.status\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"status\":{\"type\":\"idle\"}}}\n\n",
            "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n",
            "data: {\"type\":\"session.error\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"error\":{\"name\":\"AbortError\",\"message\":\"request aborted\"}}}\n\n",
        ] {
            sender.send(Ok(event)).await.expect("send scripted SSE event");
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
    if let Some(release) = &state.idle_release {
        release.notify_one();
    }
    Json(json!(true))
}

struct ExternalOpenCode {
    _directory: tempfile::TempDir,
    endpoint: String,
    log: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl ExternalOpenCode {
    async fn start(password: Option<&str>) -> Self {
        Self::start_with_idle_release(password, None).await
    }

    async fn start_with_idle_release(
        password: Option<&str>,
        idle_release: Option<Arc<Notify>>,
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
            ..Default::default()
        };
        let app = Router::new()
            .route("/global/health", get(health))
            .route("/event", get(events))
            .route("/session", post(create_session))
            .route("/session/{id}/prompt_async", post(prompt))
            .route("/session/{id}/abort", post(abort))
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
        }
    }

    fn config(&self, password: Option<&str>) -> ServerConfig {
        let mut config = ServerConfig::detect();
        config.settings.provider_instances.insert(
            "openExternal".into(),
            json!({"driver":"opencode","displayName":"OpenCode External","config":{
                "binaryPath":"this-binary-must-never-be-started","serverUrl":self.endpoint,
                "serverPassword":password.unwrap_or_default(),"customModels":["openai/gpt-5"]
            }}),
        );
        config
    }

    async fn requests(&self) -> Vec<Value> {
        self.requests_through(2).await
    }

    async fn requests_through(&self, count: usize) -> Vec<Value> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
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

#[tokio::test]
async fn interrupting_opencode_aborts_and_keeps_partial_output_despite_duplicate_idle() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(idle_release)).await;
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
async fn a_busy_opencode_prompt_steers_the_active_turn_and_a_later_prompt_starts_another() {
    let idle_release = Arc::new(Notify::new());
    let peer = ExternalOpenCode::start_with_idle_release(None, Some(idle_release)).await;
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
    let requests = peer.requests_through(3).await;
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["operation"] == "prompt")
            .count(),
        2
    );
    let settled = client.events_through_the_turn(&subscription).await;
    assert!(!settled
        .iter()
        .any(|item| item["event"]["type"] == "thread.turn-start-requested"));
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("steer-thread")
        .await;
    let steer = snapshot["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["id"] == "message-2")
        .unwrap();
    assert_eq!(steer["turnId"], active);
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("steer-thread", "message-3", "new turn"),
        )
        .await
        .expect_success();
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
        idle_release: (std::env::var("OPENCODE_TEST_GATED").as_deref() == Ok("true"))
            .then(|| Arc::new(Notify::new())),
        ..Default::default()
    };
    let app = Router::new()
        .route("/global/health", get(health))
        .route("/event", get(events))
        .route("/session", post(create_session))
        .route("/session/{id}/prompt_async", post(prompt))
        .route("/session/{id}/abort", post(abort))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind scripted OpenCode peer");
    axum::serve(listener, app)
        .await
        .expect("serve scripted OpenCode peer");
}

#[tokio::test]
async fn an_owned_opencode_turn_crosses_the_socket_and_reaps_its_server() {
    let SocketTurn {
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
    assert_eq!(requests[2]["body"]["parts"][0]["text"], "say hello");
    assert_eq!(
        requests[2]["body"]["model"],
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
async fn stopping_busy_owned_opencode_aborts_and_reaps_its_server() {
    let SocketTurn {
        opencode,
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(FakeOpenCode::busy(), "project-busy-stop", "thread-busy-stop").await;
    client.events_until_streaming(&subscription).await;
    let requests = opencode.requests().await;
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    client.call("orchestration.dispatchCommand", json!({
        "type":"thread.session.stop","commandId":"test:stop:busy-owned",
        "threadId":"thread-busy-stop","createdAt":"2026-08-01T00:00:00.000Z"
    })).await.expect_success();
    let requests = opencode.requests_through(4).await;
    assert!(requests.iter().any(|request| request["operation"] == "abort"));
    wait_until_port_closes(port, "stopping a busy owned session did not reap its OpenCode server").await;
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn an_owned_server_that_exits_during_startup_becomes_a_visible_session_failure() {
    let SocketTurn {
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
async fn an_owned_server_readiness_timeout_becomes_a_visible_session_failure() {
    let SocketTurn {
        server,
        mut client,
        subscription,
        ..
    } = start_socket_turn(
        FakeOpenCode::never_ready(),
        "project-timeout",
        "thread-timeout",
    )
    .await;
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
