//! Automatic first-turn titles at the real socket boundary.

mod harness;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::{
    extract::State,
    routing::{delete, post},
    Json, Router,
};
use harness::agent::ScriptedAgent;
use harness::conversation::{create_project, follow_up, start_turn};
use harness::workspace::Workspace;
use harness::TestServer;
use laplus_server::config::ServerConfig;
use serde_json::{json, Value};

#[derive(Clone)]
struct Generator {
    prompts: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
}

struct GeneratorControl {
    prompts: Arc<AtomicUsize>,
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
}

async fn create_session() -> Json<Value> {
    Json(json!({"id":"title-session"}))
}

async fn generate(State(generator): State<Generator>, Json(request): Json<Value>) -> Json<Value> {
    let index = generator.prompts.fetch_add(1, Ordering::SeqCst);
    generator.requests.lock().unwrap().push(request);
    generator.started.notify_one();
    generator.release.notified().await;
    let text = match index {
        0 => "{\"title\":\"Focused socket titles\"}",
        1 => "{\"title\":\"Older regeneration\"}",
        2 => "{\"title\":\"Newest regeneration\"}",
        3 => "{\"title\":\"Late generated title\"}",
        _ => "not structured output",
    };
    Json(json!({"parts":[{"type":"text","text":text}]}))
}

async fn delete_session() -> Json<Value> {
    Json(json!(true))
}

async fn generator() -> (String, GeneratorControl) {
    let prompts = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let state = Generator {
        prompts: prompts.clone(),
        started: started.clone(),
        release: release.clone(),
        requests: requests.clone(),
    };
    let app = Router::new()
        .route("/session", post(create_session))
        .route("/session/title-session/message", post(generate))
        .route("/session/title-session", delete(delete_session))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (endpoint, GeneratorControl { prompts, started, release, requests })
}

fn configured(agent: &ScriptedAgent, endpoint: &str) -> ServerConfig {
    let mut config = ServerConfig::detect();
    config.settings.providers.claude_agent.binary_path = agent.configured();
    config.settings.provider_instances.insert("titleGenerator".into(), json!({
        "driver":"opencode", "displayName":"Title generator", "enabled":true,
        "config":{"binaryPath":"unused","serverUrl":endpoint,"serverPassword":"","customModels":["openai/title"]}
    }));
    config.settings.text_generation_model_selection =
        json!({"instanceId":"titleGenerator","model":"openai/title"});
    config
}

async fn title(server: &TestServer, thread_id: &str) -> String {
    server.connect().await.into_thread_snapshot(thread_id).await["thread"]["title"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn wait_for_title(server: &TestServer, wanted: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if title(server, "thread-1").await == wanted {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the generated title is published");
}

#[tokio::test]
async fn a_first_turn_uses_the_configured_generator_and_persists_its_title() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let (endpoint, generator) = generator().await;
    let config = configured(&agent, &endpoint);
    let workspace = Workspace::with(&["src/"]);
    let database_directory = tempfile::tempdir().unwrap();
    let database = database_directory.path().join("laplus.sqlite");
    let server = TestServer::start_at_with_config(&database, config.clone()).await;
    let mut client = server.connect().await;
    let mut watcher = server.connect().await;
    let shell = watcher.subscribe("orchestration.subscribeShell", json!({})).await;
    watcher.next_chunk(&shell).await;
    watcher.ack(&shell).await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn(
                "thread-1",
                "message-1",
                "Explain why socket titles should be generated in the background",
            ),
        )
        .await
        .expect_success();
    generator.started.notified().await;
    let thread = watcher.watch_draft("thread-1").await;
    generator.release.notify_one();

    wait_for_title(&server, "Focused socket titles").await;
    assert_eq!(generator.prompts.load(Ordering::SeqCst), 1);
    watcher.values_until(&thread, |item| {
        item["event"]["type"] == "thread.meta-updated"
            && item["event"]["payload"]["title"] == "Focused socket titles"
    }).await;
    watcher.values_until(&shell, |item| {
        item["kind"] == "thread-upserted"
            && item["thread"]["title"] == "Focused socket titles"
    }).await;
    let shell = server.connect().await.into_shell_snapshot().await;
    assert_eq!(shell["threads"][0]["title"], "Focused socket titles");
    client.close().await;
    watcher.close().await;
    server.stop().await;

    let restarted = TestServer::start_at_with_config(&database, config).await;
    assert_eq!(title(&restarted, "thread-1").await, "Focused socket titles");
    let mut resumed = restarted.connect().await;
    let subscription = resumed.watch_conversation("thread-1").await;
    resumed.call(
        "orchestration.dispatchCommand",
        follow_up("thread-1", "message-2", "A historical follow-up"),
    ).await.expect_success();
    resumed.events_through_the_turn(&subscription).await;
    assert_eq!(generator.prompts.load(Ordering::SeqCst), 1);
    resumed.close().await;
    restarted.stop().await;
}

#[tokio::test]
async fn an_unsupported_generator_leaves_the_turn_and_provisional_title_intact() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let mut config = ServerConfig::detect();
    config.settings.providers.claude_agent.binary_path = agent.configured();
    config.settings.text_generation_model_selection =
        json!({"instanceId":"codex","model":"gpt-5.6-luna"});
    let server = TestServer::start_with(config).await;
    let workspace = Workspace::with(&["src/"]);
    let mut client = server.connect().await;
    client.call(
        "orchestration.dispatchCommand",
        create_project("project-1", workspace.path()),
    ).await.expect_success();
    client.call(
        "orchestration.dispatchCommand",
        start_turn("thread-1", "message-1", "A conversation"),
    ).await.expect_success();
    let subscription = client.watch_draft("thread-1").await;
    client.events_through_the_turn(&subscription).await;
    assert_eq!(title(&server, "thread-1").await, "A conversation");
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_rename_while_generation_is_pending_wins() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let (endpoint, generator) = generator().await;
    let server = TestServer::start_with(configured(&agent, &endpoint)).await;
    let workspace = Workspace::with(&["src/"]);
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "A provisional title"),
        )
        .await
        .expect_success();
    generator.started.notified().await;
    client.call("orchestration.dispatchCommand", json!({"type":"thread.meta.update","commandId":"manual","threadId":"thread-1","title":"My title"})).await.expect_success();
    generator.release.notify_one();
    wait_for_title(&server, "My title").await;
    assert_eq!(title(&server, "thread-1").await, "My title");
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn regeneration_uses_history_and_only_the_newest_still_valid_request_wins() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let (endpoint, generator) = generator().await;
    let config = configured(&agent, &endpoint);
    let database_directory = tempfile::tempdir().unwrap();
    let database = database_directory.path().join("laplus.sqlite");
    let mut server = TestServer::start_at_with_config(&database, config.clone()).await;
    let workspace = Workspace::with(&["src/"]);
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    client.call("orchestration.dispatchCommand", start_turn("thread-1", "message-1", "Explain the original topic")).await.expect_success();
    generator.started.notified().await;
    generator.release.notify_one();
    wait_for_title(&server, "Focused socket titles").await;

    let regenerate = |id: &str| json!({
        "type":"thread.meta.update", "commandId":id, "threadId":"thread-1", "regenerateTitle":true
    });
    let mut watcher = server.connect().await;
    let subscription = watcher.watch_draft("thread-1").await;
    client.call("orchestration.dispatchCommand", regenerate("older")).await.expect_success();
    generator.started.notified().await;
    watcher.values_until(&subscription, |item| {
        item["event"]["payload"]["titleRegeneration"]["requestId"] == "older"
    }).await;
    client.call("orchestration.dispatchCommand", regenerate("newest")).await.expect_success();
    generator.started.notified().await;
    generator.release.notify_waiters();
    wait_for_title(&server, "Newest regeneration").await;
    watcher.values_until(&subscription, |item| {
        item["event"]["payload"]["title"] == "Newest regeneration"
            && item["event"]["payload"]["titleRegeneration"].is_null()
    }).await;
    let requests = generator.requests.lock().unwrap();
    let regeneration_request = requests[1].to_string();
    assert!(regeneration_request.contains("Current title: Focused socket titles"));
    assert!(regeneration_request.contains("Explain the original topic"));
    drop(requests);

    client.close().await;
    watcher.close().await;
    server.stop().await;
    server = TestServer::start_at_with_config(&database, config).await;
    assert_eq!(title(&server, "thread-1").await, "Newest regeneration");
    client = server.connect().await;
    watcher = server.connect().await;
    let subscription = watcher.watch_draft("thread-1").await;

    client.call("orchestration.dispatchCommand", regenerate("manual-race")).await.expect_success();
    generator.started.notified().await;
    watcher.values_until(&subscription, |item| {
        item["event"]["payload"]["titleRegeneration"]["requestId"] == "manual-race"
    }).await;
    client.call("orchestration.dispatchCommand", json!({"type":"thread.meta.update","commandId":"manual","threadId":"thread-1","title":"Developer title"})).await.expect_success();
    watcher.values_until(&subscription, |item| {
        item["event"]["payload"]["title"] == "Developer title"
            && item["event"]["payload"]["titleRegeneration"].is_null()
    }).await;
    generator.release.notify_one();
    wait_for_title(&server, "Developer title").await;

    client.call("orchestration.dispatchCommand", regenerate("failure")).await.expect_success();
    generator.started.notified().await;
    watcher.values_until(&subscription, |item| {
        item["event"]["payload"]["titleRegeneration"]["requestId"] == "failure"
    }).await;
    generator.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
            if snapshot["thread"]["titleRegeneration"].is_null() {
                assert_eq!(snapshot["thread"]["title"], "Developer title");
                assert!(snapshot["thread"]["activities"].as_array().unwrap().iter().any(|activity| {
                    activity["kind"] == "thread.title-regeneration.failed"
                }));
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("failure clears pending state");
    watcher.values_until(&subscription, |item| {
        item["event"]["type"] == "thread.activity-appended"
            && item["event"]["payload"]["activity"]["kind"]
                == "thread.title-regeneration.failed"
    }).await;
    client.close().await;
    watcher.close().await;
    server.stop().await;
}

#[tokio::test]
async fn explicit_regeneration_supersedes_automatic_first_turn_generation() {
    let agent = ScriptedAgent::replaying("02-streamed-turn");
    let (endpoint, generator) = generator().await;
    let server = TestServer::start_with(configured(&agent, &endpoint)).await;
    let workspace = Workspace::with(&["src/"]);
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    client.call("orchestration.dispatchCommand", start_turn("thread-1", "message-1", "A first turn still being titled")).await.expect_success();
    generator.started.notified().await;
    client.call("orchestration.dispatchCommand", json!({
        "type":"thread.meta.update", "commandId":"explicit", "threadId":"thread-1", "regenerateTitle":true
    })).await.expect_success();
    generator.started.notified().await;
    generator.release.notify_waiters();
    wait_for_title(&server, "Older regeneration").await;
    let snapshot = server.connect().await.into_thread_snapshot("thread-1").await;
    assert!(snapshot["thread"]["titleRegeneration"].is_null());
    client.close().await;
    server.stop().await;
}
