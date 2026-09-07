//! Real Laplus socket + supervised CLI bootstrap + authenticated HTTP/SSE peer.
//! The peer is scripted at the public bridge boundary, not inside session::Driver.
#[path = "harness/mimir_diffs.rs"]
mod diff_tests;
mod harness;
#[path = "harness/mimir_mcp.rs"]
mod mcp_tests;
#[path = "harness/mimir_recovery.rs"]
mod recovery_tests;

use axum::{
    body::Body,
    extract::State,
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use futures_util::stream;
use harness::{
    conversation::{create_project, create_thread, follow_up, respond_to_user_input},
    SocketClient, TestServer,
};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

const SESSION: &str = "sdk-session-1";
const PLAN: &str = "sdk-plan-42";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum McpMode {
    #[default]
    Unavailable,
    Ready,
    RefuseAttach,
    MalformedAttach,
    RefuseDetach,
    RefusePrompt,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SaveResync {
    #[default]
    None,
    Immediate,
    AfterNextCompletion,
}

#[derive(Default)]
struct PeerState {
    mcp_mode: McpMode,
    manual_turns: bool,
    tail_after_next_ready: Vec<Value>,
    tokens: Vec<String>,
    requests: Vec<Value>,
    events: Vec<String>,
    sinks: Vec<mpsc::UnboundedSender<String>>,
    cursors: Vec<Option<String>>,
    refuse_plan: bool,
    save_resync: SaveResync,
    save_resync_pending: usize,
    plan: Value,
    question: Value,
    configuration: Value,
    active: Option<String>,
    completed: Vec<String>,
}
struct Shared {
    paths: PathBuf,
    workspace: PathBuf,
    state: Mutex<PeerState>,
}
struct Peer {
    directory: tempfile::TempDir,
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl PeerState {
    fn emit(&mut self, event: Value) {
        let frame = format!(
            "id: random-opaque-epoch:{}\nevent: session\ndata: {}\n\n",
            self.events.len() + 1,
            json!({"version":1,"event":event})
        );
        self.events.push(frame.clone());
        self.sinks.retain(|tx| tx.send(frame.clone()).is_ok());
    }
    fn observe(&mut self, child: bool, event: Value) {
        self.emit(json!({"observation":{"sequence":self.events.len()+1,"source":{"session_id":SESSION,"run_id":self.active,"agent_id":if child{"child-1"}else{"root-1"},"parent_agent_id":if child{Some("root-1")}else{None},"turn":1,"model_attempt":0,"tool_call_id":null},"event":event}}));
    }
    fn finish(&mut self) {
        let id = self.active.take().unwrap();
        self.completed.push(id.clone());
        self.emit(json!({"completed":{"request_id":id,"stop_reason":"end_turn","error":null}}));
        for _ in 0..std::mem::take(&mut self.save_resync_pending) {
            self.emit(json!("resync"));
        }
    }
}
impl Shared {
    fn authenticate(&self, headers: &HeaderMap) {
        let paths = std::fs::read_to_string(&self.paths).unwrap_or_default();
        let mut state = self.state.lock().unwrap();
        for command in paths.lines() {
            if let Some((_, path)) = command.split_once(' ') {
                if let Ok(bytes) = std::fs::read(path) {
                    let launch: Value = serde_json::from_slice(&bytes).unwrap();
                    state
                        .tokens
                        .push(launch["token"].as_str().unwrap().to_string());
                    std::fs::remove_file(path).unwrap();
                }
            }
        }
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert!(
            state
                .tokens
                .iter()
                .any(|token| auth == format!("Bearer {token}")),
            "authenticated loopback transport"
        );
        assert!(headers.get("origin").is_none());
    }
    fn snapshot(&self, state: &PeerState) -> Value {
        let mut requests: Vec<Value> = state
            .completed
            .iter()
            .map(|id| json!({"id":id,"status":"completed"}))
            .collect();
        if let Some(id) = &state.active {
            requests.push(json!({"id":id,"status":"running"}));
        }
        json!({"info":{"id":SESSION,"cwd":self.workspace,"title":"Peer session","live":true},"configuration":state.configuration,"active_request":state.active,"requests":requests,"messages":[],"plan":state.plan,"user_request":state.question})
    }
}
async fn api(
    State(shared): State<Arc<Shared>>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Json<Value> {
    shared.authenticate(&headers);
    assert_eq!(request["version"], 1);
    if request["action"] == "attach_mcp" {
        return mcp_tests::attach(&shared, request).await;
    }
    let mut state = shared.state.lock().unwrap();
    state.requests.push(request.clone());
    let action = request["action"].as_str().unwrap();
    if action == "decide_plan" && state.refuse_plan {
        return Json(
            json!({"version":1,"error":{"code":"sdk_error","message":"Plan is no longer current"}}),
        );
    }
    if !matches!(action, "catalog" | "create") {
        assert_eq!(request["id"], SESSION, "every control uses SDK session ID");
    }
    if action == "prompt" && state.mcp_mode == McpMode::RefusePrompt {
        return Json(
            json!({"version":1,"error":{"code":"sdk_error","message":"prompt refused after attach"}}),
        );
    }
    let result = match action {
        "detach_mcp" => {
            assert_eq!(request["attachment_id"], "host-attachment-1");
            if state.mcp_mode == McpMode::RefuseDetach {
                return Json(
                    json!({"version":1,"error":{"code":"sdk_error","message":"detach failed"}}),
                );
            }
            json!({"detached":true})
        }
        "catalog" => {
            json!([{"id":"test","name":"Test","models":[{"id":"org/model","name":"Scripted Model","reasoning_levels":["low","high"]}]}])
        }
        "create" | "open" | "snapshot" => shared.snapshot(&state),
        "configure" => {
            state.configuration = request["configuration"].clone();
            state.configuration.clone()
        }
        "prompt" => {
            assert_eq!(
                request["delivery"], "start",
                "Laplus must not double-queue at SDK level"
            );
            assert!(
                state.active.is_none(),
                "a queued send must wait for prior completion"
            );
            let id = format!(
                "request-{}",
                state
                    .requests
                    .iter()
                    .filter(|r| r["action"] == "prompt")
                    .count()
            );
            state.active = Some(id.clone());
            // Native inspection is bare /goal. A tail such as `show` is an objective.
            if request["input"]["text"] == "/goal" {
                state.emit(json!({"display":"No goal is set for this session."}));
                state.finish();
                return Json(json!({"version":1,"result":{"accepted":id}}));
            }
            if state.manual_turns {
                return Json(json!({"version":1,"result":{"accepted":id}}));
            }
            state.observe(
                false,
                json!({"text_delta":{"index":0,"value":"Root answer"}}),
            );
            state.observe(false, json!({"content_block_stop":{"index":0}}));
            if id == "request-1" {
                state.observe(
                    true,
                    json!({"text_delta":{"index":0,"value":"Child evidence"}}),
                );
                state.observe(true,json!({"run_complete":{"message":{"assistant":{"content":{"text":"Child done"}}},"execution":{"terminal_cause":"completed"}}}));
                state.observe(false,json!({"tool_finished":{"id":"read-1","name":"read_file","input_json":"{\"path\":\"src/main.rs\"}","output":"file contents","is_error":false,"duration_ms":3,"output_profile":{"file_read":{"files":[{"path":"src/main.rs"}]}},"presentation":{"preview":"Read file"}}}));
                state.question = json!({"id":"question-request-8","questions":[{"id":"question-99","prompt":"Which option?","allow_multiple":false,"options":[{"label":"A","description":"first"},{"label":"B","description":"second"}]}]});
                let question = state.question.clone();
                state.observe(
                    false,
                    json!({"user_request_ready":{"tool_call_id":"ask-8","request":question}}),
                );
            } else {
                state.finish();
            }
            json!({"accepted":id})
        }
        "answer" => {
            assert_eq!(request["request_id"], "question-request-8");
            assert_eq!(request["answers"][0]["question_id"], "question-99");
            state.question = Value::Null;
            state.plan = json!({"id":PLAN,"name":"Peer plan","path":"plan.md","markdown":"# Peer plan\n\nBuild it.","status":"review_pending"});
            state.finish();
            json!({"answered":true})
        }
        "steer" => {
            assert!(state.active.is_some());
            json!({"steered":true})
        }
        "decide_plan" => {
            assert_eq!(request["plan_id"], PLAN);
            assert!(state.active.is_none());
            if request["decision"] == "implement" {
                state.plan["status"] = json!("implementing");
                state.active = Some("implementation-1".into());
                state.observe(
                    false,
                    json!({"text_delta":{"index":0,"value":"Implementation done"}}),
                );
                state.plan["status"] = json!("completed");
                state.finish();
                json!({"accepted":"implementation-1"})
            } else {
                assert_eq!(request["decision"], "save_and_stop");
                state.plan["status"] = json!("saved_stopped");
                match state.save_resync {
                    SaveResync::None => {}
                    SaveResync::Immediate => state.emit(json!("resync")),
                    SaveResync::AfterNextCompletion => {
                        state.save_resync_pending =
                            state.save_resync_pending.checked_add(1).unwrap()
                    }
                }
                json!({"accepted":null})
            }
        }
        "cancel" => {
            if state.active.is_some() {
                state.finish();
            }
            json!({"cancelled":true})
        }
        "release" => {
            // Last owned attachment cleanup ends active work and drops its pump.
            if state.active.is_some() {
                state.finish();
            }
            state.tail_after_next_ready.clear();
            state.events.clear();
            state.sinks.clear();
            json!({"released":SESSION})
        }
        _ => panic!("unexpected API action {action}"),
    };
    Json(json!({"version":1,"result":result}))
}
async fn events(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    shared.authenticate(&headers);
    let mut state = shared.state.lock().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    state.cursors.push(
        headers
            .get("last-event-id")
            .map(|v| v.to_str().unwrap().to_string()),
    );
    tx.send(format!(
        "event: ready\ndata: {}\n\n",
        json!({"version":1,"session_id":SESSION,"cursor":format!("random-opaque-epoch:{}", state.events.len())})
    ))
    .unwrap();
    let after = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit_once(':'))
        .and_then(|(_, n)| n.parse::<usize>().ok())
        .unwrap_or(0);
    for frame in state.events.iter().skip(after) {
        tx.send(frame.clone()).unwrap();
    }
    state.sinks.push(tx);
    // Simulate SDK events still pending in the pump when the bridge reports
    // its high-water mark. They appear *after* ready without changing either
    // terminal request snapshot, so a bridge cursor cannot fence this source.
    for event in std::mem::take(&mut state.tail_after_next_ready) {
        assert!(state.active.is_none());
        state.emit(event);
    }
    let stream = stream::unfold(rx, |mut rx| async {
        rx.recv()
            .await
            .map(|frame| (Ok::<_, Infallible>(frame), rx))
    });
    Response::builder()
        .header("Content-Type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}
impl Peer {
    async fn start() -> Self {
        Self::with_mcp(McpMode::Unavailable).await
    }
    async fn with_mcp(mcp_mode: McpMode) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let shared = Arc::new(Shared {
            paths: directory.path().join("launch-paths"),
            workspace,
            state: Mutex::new(PeerState {
                mcp_mode,
                ..Default::default()
            }),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/api", post(api))
            .route("/events", get(events))
            .with_state(shared.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let mut actions = vec![
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
        ];
        if mcp_mode != McpMode::Unavailable {
            actions.extend(["attach_mcp", "detach_mcp"]);
        }
        let ready=json!({"outcome":"display","message":json!({"version":1,"endpoint":endpoint,"capabilities":{"actions":actions}}).to_string()}).to_string();
        let script = if cfg!(windows) {
            format!(
                "@echo off\r\necho %~4>>\"{}\"\r\necho {}\r\nping -n 601 127.0.0.1 >nul\r\n",
                shared.paths.display(),
                ready
            )
        } else {
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$4\" >> '{}'\nprintf '%s\\n' '{}'\nexec sleep 600\n",
                shared.paths.display(),
                ready
            )
        };
        let path = directory
            .path()
            .join(if cfg!(windows) { "mimir.cmd" } else { "mimir" });
        std::fs::write(&path, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        Self {
            directory,
            shared,
            task,
        }
    }
    fn config(&self) -> laplus_server::config::ServerConfig {
        let mut config = laplus_server::config::ServerConfig::detect();
        config.settings.providers.claude_agent.enabled = false;
        config.settings.provider_instances.insert("mimirLocal".into(),json!({"driver":"mimir","displayName":"Mimir","enabled":true,"config":{"binaryPath":self.directory.path().join(if cfg!(windows){"mimir.cmd"}else{"mimir"}),"bridgeCommand":"/org.mimir.bridge:serve"}}));
        config
    }
}
async fn dispatch(client: &mut SocketClient, command: Value) {
    client
        .call("orchestration.dispatchCommand", command)
        .await
        .expect_success();
}
async fn open(peer: &Peer) -> (TestServer, SocketClient, String) {
    let server = TestServer::start_with(peer.config()).await;
    open_on(peer, server).await
}
async fn open_on(peer: &Peer, server: TestServer) -> (TestServer, SocketClient, String) {
    let mut client = server.connect().await;
    dispatch(
        &mut client,
        create_project("project-1", &peer.shared.workspace),
    )
    .await;
    let mut thread = create_thread("project-1", "thread-1");
    thread["modelSelection"] =
        json!({"instanceId":"mimirLocal","model":"test/org/model","options":{"reasoning":"high"}});
    thread["interactionMode"] = json!("plan");
    dispatch(&mut client, thread).await;
    let subscription = client.watch_conversation("thread-1").await;
    dispatch(
        &mut client,
        follow_up("thread-1", "message-1", "Make a plan"),
    )
    .await;
    let (_, question) = client.events_until_user_input(&subscription).await;
    assert_eq!(question, "question-request-8");
    (server, client, subscription)
}
#[tokio::test]
async fn mimir_native_slash_display_is_a_readable_reply_without_a_model_turn() {
    let peer = Peer::start().await;
    let (server, mut client, subscription) = open(&peer).await;
    dispatch(
        &mut client,
        respond_to_user_input("thread-1", "question-request-8", json!({"question-99":"A"})),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    dispatch(&mut client, follow_up("thread-1", "goal-inspect", "/goal")).await;
    recovery_tests::resumed_turn(&mut client, &subscription).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(snapshot["thread"]["session"]["status"], "ready");
    assert!(snapshot["thread"]["session"]["activeTurnId"].is_null());
    let replies: Vec<_> = snapshot["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| {
            message["role"] == "assistant" && message["text"] == "No goal is set for this session."
        })
        .collect();
    assert_eq!(
        replies.len(),
        1,
        "the actual command response uses the normal transcript, not a clipped activity row"
    );
    assert_eq!(replies[0]["streaming"], false);
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_refuses_queued_slash_commands_without_publishing_or_dispatching_them() {
    let peer = Peer::start().await;
    let (server, mut client, subscription) = open(&peer).await;
    // Include both command/command and text/command mixtures in the would-be
    // queue. Ordinary messages still coalesce, but native commands must not.
    for (index, text) in [
        "/goal pause",
        "First queued text",
        " /goal clear",
        "/compress",
        "Second queued text",
    ]
    .into_iter()
    .enumerate()
    {
        let reply = client
            .call(
                "orchestration.dispatchCommand",
                follow_up("thread-1", &format!("queue-{index}"), text),
            )
            .await;
        if text.trim_start().starts_with('/') {
            assert!(
                matches!(reply, harness::Outcome::Failure(_)),
                "queued native command was accepted: {text}"
            );
            assert!(format!("{reply:?}").contains("Wait for Mimir to become idle"));
        } else {
            reply.expect_success();
        }
    }
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert!(!snapshot["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"]
            .as_str()
            .is_some_and(|text| text.trim_start().starts_with('/'))));
    assert_eq!(
        peer.shared
            .state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|request| request["action"] == "prompt")
            .count(),
        1,
        "no rejected command was dispatched"
    );
    dispatch(
        &mut client,
        respond_to_user_input("thread-1", "question-request-8", json!({"question-99":"A"})),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    // The queue has its own turn and must still drain after command rejection.
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if peer.shared.state.lock().unwrap().completed.len() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let prompts: Vec<Value> = peer
        .shared
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .filter(|request| request["action"] == "prompt")
        .cloned()
        .collect();
    assert_eq!(prompts.len(), 2);
    assert_eq!(
        prompts[1]["input"]["text"],
        "First queued text\n\nSecond queued text"
    );
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_child_work_and_native_task_snapshot_survive_open_and_reload() {
    use harness::subagents::{child_stream, folded_entries};
    let peer = Peer::start().await;
    let database = peer.directory.path().join("children.sqlite");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let (mut client, subscription) = recovery_tests::start_manual(&peer, &server).await;
    {
        let mut state = peer.shared.state.lock().unwrap();
        state.observe(false, json!({"turn_started":{"turn":1}}));
        state.observe(
            true,
            json!({"text_delta":{"index":0,"value":"Actual child evidence"}}),
        );
    }
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["payload"]["data"]["childId"] == "child-1"
        })
        .await;
    let mut inspector = server.connect().await;
    let child = inspector
        .subscribe(
            "orchestration.subscribeSubagent",
            json!({"threadId":"thread-1","childId":"child-1"}),
        )
        .await;
    let opening = inspector.next_chunk(&child).await;
    inspector.ack(&child).await;
    let opening = opening
        .iter()
        .find(|item| item["kind"] == "snapshot")
        .unwrap()["snapshot"]
        .clone();
    assert_eq!(
        opening["entries"][0]["payload"]["text"],
        "Actual child evidence"
    );
    let tool = json!({"id":"child-read","name":"read_file","input_json":"{\"paths\":[\"src/main.rs\"]}","is_error":false,"output":"Observed file contents"});
    peer.shared
        .state
        .lock()
        .unwrap()
        .observe(true, json!({"tool_started":tool}));
    let mut live = inspector
        .values_until(&child, |item| item["entry"]["kind"] == "read")
        .await;
    let started = live
        .iter()
        .find(|item| item["entry"]["kind"] == "read")
        .unwrap();
    assert_eq!(
        started["entry"]["payload"]["status"], "inProgress",
        "the real subscription decoder rejects running"
    );
    let status = json!({"analysis":"Verified the child", "next_step":null,"items":[{"step":"Inspect child","active_form":"Inspecting child","status":"completed","blocked_by":[]}]});
    {
        let mut state = peer.shared.state.lock().unwrap();
        state.observe(true, json!({"tool_finished":tool}));
        state.observe(true, json!({"run_complete":{"message":{"assistant":{"content":{"text":"Child done"}}},"execution":{"terminal_cause":"completed"}}}));
        state.observe(false, json!({"tool_finished":{"id":"status-1","name":"update_status","details_json":status.to_string(),"is_error":false}}));
        state.finish();
    }
    live.extend(
        inspector
            .values_until(&child, |item| {
                item["stream"]["outcome"]["text"] == "Child done"
            })
            .await,
    );
    client.events_through_the_turn(&subscription).await;
    let completed = child_stream(&server, "thread-1", "child-1").await;
    assert_eq!(completed["stream"]["state"], "completed");
    assert_eq!(completed["stream"]["parentChildId"], Value::Null);
    assert_eq!(completed["entries"], json!(folded_entries(&opening, &live)));
    assert_eq!(completed["entries"][1]["payload"]["status"], "completed");
    assert_eq!(
        completed["entries"][1]["payload"]["detail"],
        "Observed file contents"
    );
    assert_eq!(
        completed["entries"].as_array().unwrap().len(),
        3,
        "text, one upserted tool, and the actual outcome"
    );
    inspector.close().await;
    client.close().await;
    server.stop().await;
    let reopened = TestServer::start_at_with_config(&database, peer.config()).await;
    assert_eq!(
        child_stream(&reopened, "thread-1", "child-1").await,
        completed
    );
    let thread = reopened
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    let task = thread["thread"]["activities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["kind"] == "turn.plan.updated")
        .unwrap();
    assert_eq!(
        task["payload"],
        json!({"plan":[{"step":"Inspect child","activeForm":"Inspecting child","status":"completed","blockedBy":[]}],"explanation":"Verified the child","nextStep":null})
    );
    reopened.stop().await;
}

#[tokio::test]
async fn mimir_socket_question_child_steer_queue_and_sdk_request_identity() {
    let peer = Peer::start().await;
    let (server, mut client, subscription) = open(&peer).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(
        snapshot["session"]["status"], "running",
        "child completion cannot settle root"
    );
    dispatch(&mut client,json!({"type":"thread.turn.steer","commandId":"steer-1","threadId":"thread-1","text":"Use the existing API"})).await;
    dispatch(&mut client, follow_up("thread-1", "message-2", "Next turn")).await;
    dispatch(
        &mut client,
        respond_to_user_input(
            "thread-1",
            "question-request-8",
            json!({"question-99":{"selectedOptions":["A"]}}),
        ),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    let snapshot = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let snapshot = server
                .connect()
                .await
                .into_thread_snapshot("thread-1")
                .await["thread"]
                .clone();
            if snapshot["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["role"] == "assistant")
                .count()
                == 2
                && snapshot["session"]["status"] == "ready"
            {
                break snapshot;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(snapshot["session"]["providerName"], "mimir");
    assert!(snapshot["activities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["payload"]["data"]["paths"] == json!(["src/main.rs"])));
    {
        let state = peer.shared.state.lock().unwrap();
        let prompts: Vec<_> = state
            .requests
            .iter()
            .filter(|r| r["action"] == "prompt")
            .collect();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0]["delivery"], "start");
        assert_eq!(state.configuration["reasoning"], "high");
        assert_eq!(state.configuration["model"], "org/model");
        assert!(state.requests.iter().any(|r| r["action"] == "steer"));
    }
    client.close().await;
    server.stop().await;
}
#[tokio::test]
async fn mimir_socket_saved_plan_decisions_have_stable_identity_and_are_native() {
    let peer = Peer::start().await;
    let (server, mut client, subscription) = open(&peer).await;
    dispatch(
        &mut client,
        respond_to_user_input("thread-1", "question-request-8", json!({"question-99":"A"})),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.proposed-plan-upserted"
        })
        .await;
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"save-1","threadId":"thread-1","planId":PLAN,"decision":"save-and-stop"})).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["proposedPlan"]["decision"] == "save-and-stop"
        })
        .await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["proposedPlans"][0]["id"], PLAN);
    assert_eq!(snapshot["proposedPlans"][0]["status"], "saved-stopped");
    assert_eq!(
        peer.shared.state.lock().unwrap().requests.last().unwrap()["action"],
        "decide_plan",
        "an accepted decision must not depend on a second HTTP read succeeding"
    );
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"implement-1","threadId":"thread-1","planId":PLAN,"decision":"implement"})).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["proposedPlan"]["decision"] == "implement"
        })
        .await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["proposedPlans"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["proposedPlans"][0]["id"], PLAN);
    client.close().await;
    server.stop().await;
}
#[tokio::test]
async fn mimir_socket_save_and_stop_resync_is_not_a_history_gap() {
    let peer = Peer::start().await;
    peer.shared.state.lock().unwrap().save_resync = SaveResync::Immediate;
    let (server, mut client, subscription) = open(&peer).await;
    dispatch(
        &mut client,
        respond_to_user_input("thread-1", "question-request-8", json!({"question-99":"A"})),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"save-1","threadId":"thread-1","planId":PLAN,"decision":"save-and-stop"})).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["proposedPlan"]["decision"] == "save-and-stop"
        })
        .await;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let state = peer.shared.state.lock().unwrap();
            if state.cursors.len() >= 2
                || state
                    .requests
                    .iter()
                    .any(|request| request["action"] == "release")
            {
                break;
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("save resync is either resumed or misclassified promptly");
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["session"]["status"], "ready");
    assert!(!snapshot.to_string().contains("provider.history-gap"));
    {
        let state = peer.shared.state.lock().unwrap();
        assert_eq!(
            state.cursors.len(),
            2,
            "resume the SSE stream after the save boundary"
        );
        let cursor = format!("random-opaque-epoch:{}", state.events.len());
        assert_eq!(
            state.cursors[1].as_deref(),
            Some(cursor.as_str()),
            "resume after the consumed resync cursor"
        );
        assert!(!state
            .requests
            .iter()
            .any(|request| request["action"] == "release"));
    }
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"implement-1","threadId":"thread-1","planId":PLAN,"decision":"implement"})).await;
    client.events_through_the_turn(&subscription).await;
    let implemented = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(implemented["thread"]["session"]["status"], "ready");
    assert!(implemented["thread"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["text"] == "Implementation done"));
    client.close().await;
    server.stop().await;
}
async fn delayed_save_resync_after_new_work(implement: bool, saves: usize) {
    let peer = Peer::start().await;
    peer.shared.state.lock().unwrap().save_resync = SaveResync::AfterNextCompletion;
    let (server, mut client, subscription) = open(&peer).await;
    dispatch(
        &mut client,
        respond_to_user_input("thread-1", "question-request-8", json!({"question-99":"A"})),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"save-1","threadId":"thread-1","planId":PLAN,"decision":"save-and-stop"})).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["proposedPlan"]["decision"] == "save-and-stop"
        })
        .await;

    for index in 1..saves {
        dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":format!("save-{index}"),"threadId":"thread-1","planId":PLAN,"decision":"save-and-stop"})).await;
    }
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let state = peer.shared.state.lock().unwrap();
            let accepted = state
                .requests
                .iter()
                .filter(|request| {
                    request["action"] == "decide_plan" && request["decision"] == "save_and_stop"
                })
                .count();
            if accepted == saves {
                break;
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("every save reached the bridge before newer work");
    if implement {
        dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"implement-1","threadId":"thread-1","planId":PLAN,"decision":"implement"})).await;
    } else {
        dispatch(
            &mut client,
            follow_up("thread-1", "after-save", "Continue after save"),
        )
        .await;
    }
    client.events_through_the_turn(&subscription).await;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let state = peer.shared.state.lock().unwrap();
            if state.cursors.len() >= 2
                || state
                    .requests
                    .iter()
                    .any(|request| request["action"] == "release")
            {
                break;
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("delayed save resync is either resumed or misclassified promptly");
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["session"]["status"], "ready");
    assert!(!snapshot.to_string().contains("provider.history-gap"));
    let expected = if implement {
        "Implementation done"
    } else {
        "Root answer"
    };
    let matching: Vec<_> = snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["text"] == expected)
        .collect();
    assert_eq!(
        matching.len(),
        if implement { 1 } else { 2 },
        "newer reply must neither be lost nor replayed"
    );
    if !implement {
        assert_ne!(matching[0]["turnId"], matching[1]["turnId"]);
    }
    {
        let state = peer.shared.state.lock().unwrap();
        assert_eq!(state.cursors.len(), saves + 1);
        let cursor = format!("random-opaque-epoch:{}", state.events.len());
        assert_eq!(
            state.cursors[saves].as_deref(),
            Some(cursor.as_str()),
            "resume after every delayed save cursor"
        );
        assert!(!state
            .requests
            .iter()
            .any(|request| request["action"] == "release"));
    }
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_socket_delayed_save_resync_survives_immediate_implement() {
    delayed_save_resync_after_new_work(true, 1).await;
}

#[tokio::test]
async fn mimir_socket_delayed_save_resync_survives_immediate_prompt() {
    delayed_save_resync_after_new_work(false, 1).await;
}

#[tokio::test]
async fn mimir_socket_two_delayed_saves_consume_two_resyncs() {
    delayed_save_resync_after_new_work(false, 2).await;
}

#[tokio::test]
async fn mimir_socket_excess_delayed_save_resync_remains_fatal() {
    let peer = Peer::start().await;
    peer.shared.state.lock().unwrap().save_resync = SaveResync::AfterNextCompletion;
    let (server, mut client, subscription) = open(&peer).await;
    dispatch(
        &mut client,
        respond_to_user_input("thread-1", "question-request-8", json!({"question-99":"A"})),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"save-1","threadId":"thread-1","planId":PLAN,"decision":"save-and-stop"})).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["proposedPlan"]["decision"] == "save-and-stop"
        })
        .await;
    {
        let mut state = peer.shared.state.lock().unwrap();
        assert_eq!(state.save_resync_pending, 1);
        state.save_resync_pending += 1;
    }
    dispatch(
        &mut client,
        follow_up("thread-1", "after-save", "Continue after save"),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if peer
                .shared
                .state
                .lock()
                .unwrap()
                .requests
                .iter()
                .any(|request| request["action"] == "release")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the excess resync retires the source");
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["session"]["status"], "error");
    assert!(snapshot.to_string().contains("provider.history-gap"));
    assert_eq!(
        peer.shared.state.lock().unwrap().cursors.len(),
        2,
        "only the correlated resync reconnects"
    );
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_socket_reconnect_uses_opaque_cursor_but_gap_requires_deliberate_reopen() {
    let peer = Peer::start().await;
    let (server, mut client, subscription) = open(&peer).await;
    peer.shared.state.lock().unwrap().sinks.clear();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let reconnected = {
                let state = peer.shared.state.lock().unwrap();
                state.cursors.len() >= 2
            };
            if reconnected {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("SSE reconnects");
    {
        let mut state = peer.shared.state.lock().unwrap();
        assert!(state.cursors[1]
            .as_ref()
            .unwrap()
            .starts_with("random-opaque-epoch:"));
        let id = state.active.take().unwrap();
        state.completed.push(id);
        state.question = Value::Null;
        // The terminal event was lost. Do not invent a successful completion
        // from an unfenced snapshot; retire before admitting any new turn.
        for sink in &state.sinks {
            sink.send(format!(
                "event: gap\ndata: {}\n\n",
                json!({"version":1,"reason":"replay_unavailable","cursor":"different-epoch:77"})
            ))
            .unwrap();
        }
    }
    client.events_through_the_turn(&subscription).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["session"]["status"], "error");
    assert!(snapshot["session"]["lastError"]
        .as_str()
        .unwrap()
        .contains("send again"));
    assert_eq!(
        snapshot["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["role"] == "assistant")
            .count(),
        1,
        "replay cannot duplicate existing text"
    );
    assert!(snapshot["activities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a["kind"] == "provider.history-gap"));
    dispatch(
        &mut client,
        follow_up("thread-1", "message-b", "Deliberately reopen"),
    )
    .await;
    recovery_tests::resumed_turn(&mut client, &subscription).await;
    let resumed = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(resumed["thread"]["session"]["status"], "ready");
    let actions: Vec<_> = peer
        .shared
        .state
        .lock()
        .unwrap()
        .requests
        .iter()
        .map(|r| r["action"].as_str().unwrap().to_string())
        .collect();
    let released = actions.iter().position(|a| a == "release").unwrap();
    assert!(actions[released + 1..].iter().any(|a| a == "open"));
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_socket_interrupt_preserves_partial_reply_and_reuses_session() {
    let peer = Peer::start().await;
    let (server, mut client, subscription) = open(&peer).await;
    dispatch(
        &mut client,
        harness::conversation::interrupt_turn("thread-1", None),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert!(snapshot["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "assistant" && m["text"] == "Root answer"));
    dispatch(&mut client, follow_up("thread-1", "after-stop", "Continue")).await;
    client.events_through_the_turn(&subscription).await;
    {
        let state = peer.shared.state.lock().unwrap();
        assert_eq!(
            state
                .requests
                .iter()
                .filter(|r| r["action"] == "create")
                .count(),
            1
        );
        assert!(state.requests.iter().any(|r| r["action"] == "cancel"));
    }
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn mimir_socket_saved_plan_survives_restart_and_failed_decision_stays_unanswered() {
    let peer = Peer::start().await;
    let database = peer.directory.path().join("laplus.sqlite");
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let (server, mut client, subscription) = open_on(&peer, server).await;
    dispatch(
        &mut client,
        respond_to_user_input("thread-1", "question-request-8", json!({"question-99":"A"})),
    )
    .await;
    client.events_through_the_turn(&subscription).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.proposed-plan-upserted"
        })
        .await;
    client.close().await;
    server.stop().await;
    let server = TestServer::start_at_with_config(&database, peer.config()).await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["proposedPlans"][0]["id"], PLAN);
    let mut client = server.connect().await;
    let subscription = client.watch_conversation("thread-1").await;
    peer.shared.state.lock().unwrap().refuse_plan = true;
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"failed-decision","threadId":"thread-1","planId":PLAN,"decision":"implement"})).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["activity"]["kind"] == "provider.control-failed"
        })
        .await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await["thread"]
        .clone();
    assert_eq!(snapshot["proposedPlans"][0]["status"], "review-pending");
    assert!(snapshot["proposedPlans"][0]["decision"].is_null());
    peer.shared.state.lock().unwrap().refuse_plan = false;
    dispatch(&mut client,json!({"type":"thread.plan.decide","commandId":"successful-save","threadId":"thread-1","planId":PLAN,"decision":"save-and-stop"})).await;
    client
        .values_until(&subscription, |item| {
            item["event"]["payload"]["proposedPlan"]["decision"] == "save-and-stop"
        })
        .await;
    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("thread-1")
        .await;
    assert_eq!(snapshot["thread"]["session"]["status"], "ready");
    assert!(snapshot["thread"]["session"]["activeTurnId"].is_null());
    assert_eq!(
        snapshot["thread"]["proposedPlans"][0]["status"],
        "saved-stopped"
    );
    {
        let state = peer.shared.state.lock().unwrap();
        assert_eq!(
            state
                .requests
                .iter()
                .filter(|r| r["action"] == "create")
                .count(),
            1
        );
        assert!(state
            .requests
            .iter()
            .any(|r| r["action"] == "open" && r["id"] == SESSION));
    }
    client.close().await;
    server.stop().await;
}
