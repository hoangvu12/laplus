//! OpenCode's first complete turn through the same WebSocket boundary as the UI.
//!
//! The configured binary is a small platform script which re-enters this test
//! executable as the ignored HTTP/SSE peer below. That keeps the test hermetic
//! while exercising the real child-process, HTTP, session and socket paths.

mod harness;

use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use futures_util::stream;
use harness::{
    conversation::{assistant_sends, create_project, create_thread, start_turn},
    workspace::Workspace,
    SocketClient, TestServer,
};
use laplus_server::config::ServerConfig;
use serde_json::{json, Value};
use tokio::sync::mpsc;

struct FakeOpenCode {
    directory: tempfile::TempDir,
    log: PathBuf,
}

#[derive(Clone, Copy)]
enum Startup {
    Healthy,
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
            (Startup::ResistsStop, true) => "set OPENCODE_TEST_PORT=%3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=true\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::NeverReady, true) => "set OPENCODE_TEST_PORT=%3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_HEALTHY=false\r\n\"{executable}\" --exact opencode_peer_child --ignored --nocapture",
            (Startup::Healthy, false) => "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_HEALTHY=true exec '{executable}' --exact opencode_peer_child --ignored --nocapture",
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
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&self.log) {
                    let values = contents
                        .lines()
                        .map(|line| serde_json::from_str(line).expect("logged JSON request"))
                        .collect::<Vec<_>>();
                    if values.len() >= 3 {
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

async fn events(State(state): State<PeerState>) -> Response {
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

async fn create_session(State(state): State<PeerState>, Json(body): Json<Value>) -> Json<Value> {
    append(&state.log, json!({"operation":"create","body":body}));
    Json(json!({"id":"ses_owned_1"}))
}

async fn prompt(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> StatusCode {
    append(
        &state.log,
        json!({"operation":"prompt","sessionId":session_id,"body":body}),
    );
    let sender = state
        .subscriber
        .lock()
        .expect("subscriber lock")
        .clone()
        .expect("event subscription precedes prompt");
    for event in [
        "data: {\"type\":\"message.part.delta\",\"properties\":{\"sessionID\":\"ses_owned_1\",\"field\":\"text\",\"delta\":\"hello \"}}\n\n",
        "data: {\"type\":\"message.part.updated\",\"properties\":{\"part\":{\"sessionID\":\"ses_owned_1\",\"type\":\"text\",\"text\":\"hello from OpenCode\"}}}\n\n",
        "data: {\"type\":\"session.idle\",\"properties\":{\"sessionID\":\"ses_owned_1\"}}\n\n",
    ] {
        sender.send(Ok(event)).await.expect("send scripted SSE event");
    }
    StatusCode::NO_CONTENT
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
        ..Default::default()
    };
    let app = Router::new()
        .route("/global/health", get(health))
        .route("/event", get(events))
        .route("/session", post(create_session))
        .route("/session/{id}/prompt_async", post(prompt))
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
            ("hello from OpenCode".to_string(), false),
        ]
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
