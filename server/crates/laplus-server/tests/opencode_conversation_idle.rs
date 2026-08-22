//! A conversation-owned OpenCode server is given up when idle, and the next
//! message resumes by durable session id against the freshly spawned one.
//!
//! The reap decision itself is pure and asserted without arming anything
//! (ADR-0043's pattern). The integration half uses the scripted-peer seam:
//! the configured binary is a small platform script re-entering this test
//! executable as the ignored HTTP/SSE peer below, so a whole server lifetime —
//! spawn, turn, idle, kill, respawn, resume — is driven through the real child,
//! HTTP, session and socket paths with no wall-clock assertions. The one sleep
//! in these tests exists to stand *past* a policy boundary that has been
//! shortened to one second; timeouts catch hangs and assert nothing about how
//! long anything took.

mod harness;

use std::{
    convert::Infallible,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use futures_util::stream;
use harness::{
    conversation::{
        activities, assistant_sends, create_project, create_thread, follow_up,
        respond_to_approval, start_turn,
    },
    workspace::Workspace,
    SocketClient, TestServer,
};
use laplus_server::{
    config::ServerConfig,
    session::{
        conversation_idle_decision, ConversationIdleDecision, IdleSession,
        CONVERSATION_IDLE_WINDOW,
    },
};
use serde_json::{json, Value};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// The pure decision
// ---------------------------------------------------------------------------

fn quiet() -> IdleSession {
    IdleSession {
        owned_opencode_server: true,
        turn_in_flight: false,
        prompt_waiting: false,
        request_outstanding: false,
    }
}

#[test]
fn an_idle_owned_server_reaps_once_the_window_has_passed() {
    assert_eq!(
        conversation_idle_decision(CONVERSATION_IDLE_WINDOW - Duration::from_secs(1), quiet()),
        ConversationIdleDecision::Keep
    );
    assert_eq!(
        conversation_idle_decision(CONVERSATION_IDLE_WINDOW, quiet()),
        ConversationIdleDecision::Reap
    );
    assert_eq!(
        conversation_idle_decision(CONVERSATION_IDLE_WINDOW * 30, quiet()),
        ConversationIdleDecision::Reap
    );
}

#[test]
fn an_active_turn_refuses_the_reap_at_any_age() {
    let mut busy = quiet();
    busy.turn_in_flight = true;
    assert_eq!(
        conversation_idle_decision(CONVERSATION_IDLE_WINDOW * 1000, busy),
        ConversationIdleDecision::Keep
    );
}

#[test]
fn a_waiting_prompt_refuses_the_reap_at_any_age() {
    let mut queued = quiet();
    queued.prompt_waiting = true;
    assert_eq!(
        conversation_idle_decision(CONVERSATION_IDLE_WINDOW * 1000, queued),
        ConversationIdleDecision::Keep
    );
}

/// Approvals and questions are one condition here because they are one thing
/// to the loop: both live in the same outstanding map, and either one is an
/// agent stopped for an answer it must still receive.
#[test]
fn an_unanswered_request_refuses_the_reap_at_any_age() {
    let mut held = quiet();
    held.request_outstanding = true;
    assert_eq!(
        conversation_idle_decision(CONVERSATION_IDLE_WINDOW * 1000, held),
        ConversationIdleDecision::Keep
    );
}

#[test]
fn an_external_endpoint_is_never_a_reap_candidate() {
    let mut external = quiet();
    external.owned_opencode_server = false;
    assert_eq!(
        conversation_idle_decision(CONVERSATION_IDLE_WINDOW * 1000, external),
        ConversationIdleDecision::Keep
    );
}

// ---------------------------------------------------------------------------
// The scripted owned peer
// ---------------------------------------------------------------------------

const TEST_IDLE_SECS: &str = "1";

/// Shorten laplus's own window so the integration cases reach the decision
/// boundary in seconds. Read by [`laplus_server::session`]'s arming code; see
/// its doc comment for why this is a test seam and not a setting.
fn use_one_second_window() {
    std::env::set_var("LAPLUS_TEST_CONVERSATION_IDLE_SECS", TEST_IDLE_SECS);
}

struct FakeOpenCode {
    directory: tempfile::TempDir,
    log: PathBuf,
}

#[derive(Clone, Copy)]
enum Mode {
    /// A first turn that completes, and a resumed turn after adoption.
    Healthy,
    /// A first turn that streams a little and never finishes — busy forever.
    Stall,
    /// A first turn that stops on a permission request and answers only when
    /// the reply route is hit.
    Hold,
}

impl Mode {
    fn as_env(self) -> &'static str {
        match self {
            Mode::Healthy => "healthy",
            Mode::Stall => "stall",
            Mode::Hold => "hold",
        }
    }
}

impl FakeOpenCode {
    fn new(mode: Mode) -> Self {
        let directory = tempfile::tempdir().expect("temporary OpenCode directory");
        let log = directory.path().join("requests.jsonl");
        let executable = std::env::current_exe().expect("the test executable");
        let path = directory
            .path()
            .join(if cfg!(windows) { "opencode.cmd" } else { "opencode" });
        let serve = if cfg!(windows) {
            "set OPENCODE_TEST_PORT=%~3\r\nset OPENCODE_TEST_LOG={log}\r\nset OPENCODE_TEST_MODE={mode}\r\n\"{executable}\" --exact conversation_idle_peer_child --ignored --nocapture".to_string()
        } else {
            "OPENCODE_TEST_PORT=\"$3\" OPENCODE_TEST_LOG='{log}' OPENCODE_TEST_MODE='{mode}' exec '{executable}' --exact conversation_idle_peer_child --ignored --nocapture".to_string()
        };
        let script = if cfg!(windows) {
            format!(
                "@echo off\r\nif \"%~1\"==\"--version\" (echo 1.18.10& exit /b 0)\r\nif \"%~1\"==\"models\" (echo openai/gpt-5& echo {{\"id\":\"gpt-5\",\"name\":\"GPT 5\",\"variants\":{{}}}}& exit /b 0)\r\nif \"%~1\"==\"agent\" (echo build ^(primary^)& exit /b 0)\r\n{serve}\r\n"
            )
        } else {
            format!(
                "#!/bin/sh\ncase \"$1\" in\n  --version) echo 1.18.10; exit 0;;\n  models) printf '%s\\n' 'openai/gpt-5' '{{\"id\":\"gpt-5\",\"name\":\"GPT 5\",\"variants\":{{}}}}'; exit 0;;\n  agent) echo 'build (primary)'; exit 0;;\nesac\n{serve}\n"
            )
        }
        .replace("{mode}", mode.as_env())
        .replace("{log}", &log.display().to_string())
        .replace("{executable}", &executable.display().to_string());
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

    async fn requests_through(&self, count: usize) -> Vec<Value> {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                if let Ok(contents) = std::fs::read_to_string(&self.log) {
                    if !contents.is_empty() && !contents.ends_with('\n') {
                        tokio::time::sleep(Duration::from_millis(10)).await;
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
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("OpenCode records its launch, session and prompt requests")
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

async fn wait_for_port_close(port: u16, because: &str) {
    tokio::time::timeout(Duration::from_secs(30), async {
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

async fn port_is_open(port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_ok()
}

async fn dispatch(client: &mut SocketClient, command: Value) {
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
}

fn model_selection() -> Value {
    json!({"instanceId":"openLocal","model":"openai/gpt-5"})
}

#[derive(Clone)]
struct PeerState {
    log: Arc<PathBuf>,
    subscriber: Arc<Mutex<Option<mpsc::Sender<Result<String, Infallible>>>>>,
    mode: Arc<Mutex<String>>,
    /// Set once this child has been asked for `ses_owned_1` by id — the durable
    /// resume. Both children mint the same session id, so this flag, not a
    /// prompt counter, tells the first turn from the resumed one.
    adopted: Arc<AtomicBool>,
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

async fn health() -> Json<Value> {
    Json(json!({"healthy":true,"version":"1.18.10"}))
}

async fn providers() -> Json<Value> {
    Json(json!({
        "providers": [{
            "id": "openai",
            "models": {"gpt-5": {"id": "gpt-5", "name": "GPT 5", "limit": {"context": 200_000}}}
        }],
        "connected": ["openai"]
    }))
}

async fn config_snapshot() -> Json<Value> {
    Json(json!({"compaction": {"auto": false}}))
}

async fn add_mcp(State(state): State<PeerState>, Json(body): Json<Value>) -> Json<Value> {
    let endpoint = body
        .pointer("/config/url")
        .and_then(Value::as_str)
        .expect("MCP endpoint")
        .to_string();
    let authorization = body
        .pointer("/config/headers/Authorization")
        .and_then(Value::as_str)
        .expect("MCP authorization")
        .to_string();
    append(&state.log, json!({"operation":"mcp.add"}));
    let response = reqwest::Client::new()
        .post(endpoint)
        .header(header::AUTHORIZATION.as_str(), authorization)
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"opencode-test","version":"1.18.10"}
        }}))
        .send()
        .await
        .expect("connect to Laplus MCP");
    assert_eq!(response.status(), StatusCode::OK);
    Json(json!({"laplus":{"status":"connected"}}))
}

async fn events(State(state): State<PeerState>) -> Response {
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

async fn create_session(State(state): State<PeerState>, Json(body): Json<Value>) -> Json<Value> {
    append(
        &state.log,
        json!({"operation":"create","body":body}),
    );
    Json(json!({"id":"ses_owned_1"}))
}

async fn get_session(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Json<Value> {
    append(
        &state.log,
        json!({"operation":"get","sessionId":session_id,"directory":query.get("directory")}),
    );
    state.adopted.store(true, Ordering::SeqCst);
    Json(json!({"id":session_id,"directory":query.get("directory")}))
}

async fn update_session(
    AxumPath(session_id): AxumPath<String>,
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    append(
        &state.log,
        json!({"operation":"update","sessionId":session_id,"body":body}),
    );
    Json(json!({"id":session_id}))
}

async fn abort(AxumPath(session_id): AxumPath<String>, State(state): State<PeerState>) -> Json<Value> {
    append(&state.log, json!({"operation":"abort","sessionId":session_id}));
    Json(json!(true))
}

async fn reply_permission(
    AxumPath(request_id): AxumPath<String>,
    State(state): State<PeerState>,
    Json(body): Json<Value>,
) -> Json<Value> {
    append(
        &state.log,
        json!({"operation":"permission.reply","requestId":request_id,"body":body}),
    );
    let sender = state
        .subscriber
        .lock()
        .expect("subscriber lock")
        .clone()
        .expect("event subscription precedes permission reply");
    for event in [
        r#"data: {"id":"evt-reply-1","type":"permission.replied","properties":{"sessionID":"ses_owned_1","requestID":"per-1","reply":"once"}}"#,
        r#"data: {"type":"message.part.updated","properties":{"part":{"id":"part-tool-1","messageID":"message-1","sessionID":"ses_owned_1","type":"tool","callID":"call-1","tool":"mystery","state":{"status":"completed","input":{"secret":42},"output":"done","title":"Mystery","time":{"start":1,"end":2}}}}}"#,
        r#"data: {"type":"message.part.updated","properties":{"part":{"id":"text-2","messageID":"message-2","sessionID":"ses_owned_1","type":"text","text":""}}}}"#,
        r#"data: {"type":"message.updated","properties":{"info":{"id":"message-2","sessionID":"ses_owned_1","role":"assistant"}}}"#,
        r#"data: {"type":"session.status","properties":{"sessionID":"ses_owned_1","status":{"type":"busy"}}}"#,
        r#"data: {"type":"message.part.delta","properties":{"sessionID":"ses_owned_1","messageID":"message-2","partID":"text-2","field":"text","delta":"answered "}}"#,
        r#"data: {"type":"message.part.updated","properties":{"part":{"id":"text-2","messageID":"message-2","sessionID":"ses_owned_1","type":"text","text":"answered after approval"}}}}"#,
        r#"data: {"type":"session.idle","properties":{"sessionID":"ses_owned_1"}}"#,
    ] {
        sender
            .send(Ok(format!("{event}\n\n")))
            .await
            .expect("send scripted permission-reply SSE event");
    }
    Json(json!(true))
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
    let mode = state.mode.lock().expect("mode lock").clone();
    let events: Vec<&str> = if state.adopted.load(Ordering::SeqCst) {
        vec![
            r#"data: {"type":"message.part.updated","properties":{"part":{"id":"text-9","messageID":"message-9","sessionID":"ses_owned_1","type":"text","text":""}}}"#,
            r#"data: {"type":"message.updated","properties":{"info":{"id":"message-9","sessionID":"ses_owned_1","role":"assistant"}}}"#,
            r#"data: {"type":"session.status","properties":{"sessionID":"ses_owned_1","status":{"type":"busy"}}}"#,
            r#"data: {"type":"message.part.delta","properties":{"sessionID":"ses_owned_1","messageID":"message-9","partID":"text-9","field":"text","delta":"picked up "}}"#,
            r#"data: {"type":"message.part.updated","properties":{"part":{"id":"text-9","messageID":"message-9","sessionID":"ses_owned_1","type":"text","text":"picked up where we left off"}}}}"#,
            r#"data: {"type":"session.idle","properties":{"sessionID":"ses_owned_1"}}"#,
        ]
    } else if mode == "stall" {
        vec![
            r#"data: {"type":"message.part.updated","properties":{"part":{"id":"text-1","messageID":"message-1","sessionID":"ses_owned_1","type":"text","text":""}}}}"#,
            r#"data: {"type":"message.updated","properties":{"info":{"id":"message-1","sessionID":"ses_owned_1","role":"assistant"}}}"#,
            r#"data: {"type":"session.status","properties":{"sessionID":"ses_owned_1","status":{"type":"busy"}}}"#,
            r#"data: {"type":"message.part.delta","properties":{"sessionID":"ses_owned_1","messageID":"message-1","partID":"text-1","field":"text","delta":"working "}}"#,
        ]
    } else if mode == "hold" {
        vec![
            r#"data: {"type":"message.updated","properties":{"info":{"id":"message-1","sessionID":"ses_owned_1","role":"assistant"}}}"#,
            r#"data: {"id":"evt-tool-1","type":"message.part.updated","properties":{"sessionID":"ses_owned_1","part":{"id":"part-tool-1","messageID":"message-1","sessionID":"ses_owned_1","type":"tool","callID":"call-1","tool":"mystery","state":{"status":"running","input":{"secret":42},"time":{"start":1}}}}}"#,
            r#"data: {"id":"evt-per-1","type":"permission.asked","properties":{"id":"per-1","sessionID":"ses_owned_1","permission":"bash","patterns":["cargo test"],"metadata":{"command":"cargo test"},"always":[],"tool":{"messageID":"message-1","callID":"call-1"}}}"#,
        ]
    } else {
        vec![
            r#"data: {"type":"message.part.updated","properties":{"part":{"id":"text-1","messageID":"message-1","sessionID":"ses_owned_1","type":"text","text":""}}}}"#,
            r#"data: {"type":"message.updated","properties":{"info":{"id":"message-1","sessionID":"ses_owned_1","role":"assistant"}}}"#,
            r#"data: {"type":"session.status","properties":{"sessionID":"ses_owned_1","status":{"type":"busy"}}}"#,
            r#"data: {"type":"message.part.delta","properties":{"sessionID":"ses_owned_1","messageID":"message-1","partID":"text-1","field":"text","delta":"hello "}}"#,
            r#"data: {"type":"message.part.updated","properties":{"part":{"id":"text-1","messageID":"message-1","sessionID":"ses_owned_1","type":"text","text":"first answer"}}}}"#,
            r#"data: {"type":"session.status","properties":{"sessionID":"ses_owned_1","status":{"type":"idle"}}}"#,
            r#"data: {"type":"session.idle","properties":{"sessionID":"ses_owned_1"}}"#,
        ]
    };
    for event in events {
        sender
            .send(Ok(format!("{event}\n\n")))
            .await
            .expect("send scripted SSE event");
    }
    StatusCode::NO_CONTENT
}

#[tokio::test]
#[ignore]
async fn conversation_idle_peer_child() {
    let port = std::env::var("OPENCODE_TEST_PORT")
        .expect("peer port")
        .trim_start_matches("--port=")
        .parse::<u16>()
        .expect("numeric peer port");
    let log = PathBuf::from(std::env::var("OPENCODE_TEST_LOG").expect("peer log"));
    append(&log, json!({"operation":"launch","port":port}));
    let state = PeerState {
        log: Arc::new(log),
        subscriber: Arc::new(Mutex::new(None)),
        mode: Arc::new(Mutex::new(
            std::env::var("OPENCODE_TEST_MODE").unwrap_or_else(|_| "healthy".to_string()),
        )),
        adopted: Arc::new(AtomicBool::new(false)),
    };
    let app = Router::new()
        .route("/global/health", get(health))
        .route("/provider", get(providers))
        .route("/config", get(config_snapshot))
        .route("/mcp", post(add_mcp))
        .route("/event", get(events))
        .route("/session", post(create_session))
        .route("/session/{id}", get(get_session).patch(update_session))
        .route("/session/{id}/prompt_async", post(prompt))
        .route("/session/{id}/abort", post(abort))
        .route("/permission/{id}/reply", post(reply_permission))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind scripted OpenCode peer");
    axum::serve(listener, app)
        .await
        .expect("serve scripted OpenCode peer");
}

// ---------------------------------------------------------------------------
// Integration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_idle_conversation_gives_up_its_server_and_resumes_by_session_id() {
    use_one_second_window();
    let opencode = FakeOpenCode::new(Mode::Healthy);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(opencode_config(&opencode)).await;
    let mut client = server.connect().await;
    dispatch(
        &mut client,
        create_project("idle-project", workspace.path()),
    )
    .await;
    let mut create = create_thread("idle-project", "idle-thread");
    create["modelSelection"] = model_selection();
    dispatch(&mut client, create).await;
    let watch = client.watch_conversation("idle-thread").await;
    let mut turn = start_turn("idle-thread", "idle-message-1", "say something");
    turn["modelSelection"] = model_selection();
    dispatch(&mut client, turn).await;

    let first = client.events_through_the_turn(&watch).await;
    assert_eq!(assistant_sends(&first).last().expect("a reply").0, "first answer");
    assert!(!activities(&first).iter().any(|activity| {
        activity["kind"] == "session.failed" || activity["kind"] == "turn.delivery-failed"
    }));

    let requests = opencode.requests_through(5).await;
    assert_eq!(
        requests[..5]
            .iter()
            .map(|row| row["operation"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["launch", "mcp.add", "create", "update", "prompt"],
        "the first server's lifetime"
    );
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    // The window is one second; everything past this point is bounded by a
    // hang-catching timeout, not by any claim about how long the machinery
    // takes.
    wait_for_port_close(
        port,
        "the idle conversation's OpenCode server should be given up between turns",
    )
    .await;

    // Reaping was invisible: a client arriving between the reap and the next
    // message reads a conversation that simply looks ready — not stopped, not
    // errored, nothing failed anywhere.
    let mid_reap = server.connect().await.into_thread_snapshot("idle-thread").await;
    assert_eq!(
        mid_reap["thread"]["session"]["status"],
        json!("ready"),
        "a reaped conversation must not read as broken or stopped"
    );
    assert!(mid_reap["thread"]["session"]["lastError"].is_null());
    let said: Vec<String> = mid_reap["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(said, vec!["say something", "first answer"], "{said:?}");

    let mut followup = follow_up("idle-thread", "idle-message-2", "and again");
    followup["modelSelection"] = model_selection();
    dispatch(&mut client, followup).await;
    let second = client.events_through_the_next_turn(&watch).await;
    assert_eq!(
        assistant_sends(&second).last().expect("a resumed reply").0,
        "picked up where we left off"
    );

    // The fresh server adopted the *same* session id before prompting: get,
    // update, prompt — the durable-cursor resume, nothing else.
    let requests = opencode.requests_through(10).await;
    assert_eq!(
        requests[5..]
            .iter()
            .map(|row| row["operation"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["launch", "mcp.add", "get", "update", "prompt"],
        "the resumed server adopts by session id before the next turn"
    );
    assert_eq!(
        requests
            .iter()
            .find(|row| row["operation"] == "get")
            .expect("resume asks for the session by id")["sessionId"],
        "ses_owned_1"
    );

    // And the transcript a late reader gets holds both turns in order.
    let final_snapshot = server.connect().await.into_thread_snapshot("idle-thread").await;
    assert_eq!(
        final_snapshot["thread"]["session"]["status"],
        json!("ready"),
        "the conversation survives the reap"
    );
    let said: Vec<String> = final_snapshot["thread"]["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .map(|message| message["text"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(said.contains(&"picked up where we left off".to_string()), "{said:?}");
    assert!(!activities(&first)
        .into_iter()
        .chain(activities(&second))
        .any(|activity| {
            activity["kind"] == "session.failed" || activity["kind"] == "turn.delivery-failed"
        }));

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn an_active_turn_is_never_reaped_regardless_of_age() {
    use_one_second_window();
    let opencode = FakeOpenCode::new(Mode::Stall);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(opencode_config(&opencode)).await;
    let mut client = server.connect().await;
    dispatch(
        &mut client,
        create_project("busy-project", workspace.path()),
    )
    .await;
    let mut create = create_thread("busy-project", "busy-thread");
    create["modelSelection"] = model_selection();
    dispatch(&mut client, create).await;
    let watch = client.watch_conversation("busy-thread").await;
    let mut turn = start_turn("busy-thread", "busy-message-1", "take your time");
    turn["modelSelection"] = model_selection();
    dispatch(&mut client, turn).await;

    client.events_until_streaming(&watch).await;
    let requests = opencode.requests_through(5).await;
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    // Stand well past the shortened window while the provider is mid-turn.
    // Nothing may have killed the server underneath the developer.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        port_is_open(port).await,
        "an active turn holds its server past the idle window"
    );

    // Ending the session is what finally gives the server up.
    client
        .call(
            "orchestration.dispatchCommand",
            json!({"type":"thread.session.stop","commandId":"test:stop-busy","threadId":"busy-thread","createdAt":"2026-08-22T00:00:00.000Z"}),
        )
        .await
        .expect_success();
    wait_for_port_close(port, "an explicitly stopped session gives up its server").await;

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn an_unanswered_permission_holds_past_the_window_and_answering_still_reaches_the_agent() {
    use_one_second_window();
    let opencode = FakeOpenCode::new(Mode::Hold);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with(opencode_config(&opencode)).await;
    let mut client = server.connect().await;
    dispatch(
        &mut client,
        create_project("hold-project", workspace.path()),
    )
    .await;
    let mut create = create_thread("hold-project", "hold-thread");
    create["modelSelection"] = model_selection();
    dispatch(&mut client, create).await;
    let watch = client.watch_conversation("hold-thread").await;
    let mut turn = start_turn("hold-thread", "hold-message-1", "may I?");
    turn["modelSelection"] = model_selection();
    dispatch(&mut client, turn).await;

    let (_, request_id) = client.events_until_permission(&watch).await;
    assert_eq!(request_id, "per-1");
    let requests = opencode.requests_through(5).await;
    let port = requests[0]["port"].as_u64().expect("the owned port") as u16;

    // Past the window with nobody answering, the server is still exactly where
    // it was: the agent is waiting on the answer.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        port_is_open(port).await,
        "a pending permission request holds the server past the idle window"
    );

    dispatch(
        &mut client,
        respond_to_approval("hold-thread", "per-1", "accept"),
    )
    .await;
    let settled = client.events_through_the_turn(&watch).await;
    assert_eq!(
        assistant_sends(&settled).last().expect("a reply after approval").0,
        "answered after approval"
    );
    let requests = opencode.requests_through(6).await;
    let reply = requests
        .iter()
        .find(|row| row["operation"] == "permission.reply")
        .expect("the decision reaches the agent that asked");
    assert_eq!(reply["requestId"], "per-1");
    assert_eq!(reply["body"], json!({"reply":"once"}));

    // Answered and settled, the conversation goes idle again — and the hold
    // releases with it.
    wait_for_port_close(
        port,
        "once answered and settled, the idle hold releases the server",
    )
    .await;

    client.close().await;
    server.stop().await;
}
