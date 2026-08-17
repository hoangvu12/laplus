//! Codex turns at the socket boundary used by the real composer.

mod harness;

use harness::codex::ScriptedCodex;
use harness::agent::ScriptedAgent;
use harness::conversation::{
    activities, activity, assistant_sends, create_project, create_thread, follow_up, follow_up_in,
    interrupt_turn, last_session, respond_to_approval,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use laplus_server::config::ServerConfig;
use serde_json::{json, Value};

fn codex_thread(runtime_mode: &str) -> Value {
    json!({
        "type": "thread.create",
        "commandId": "test:thread:codex-thread",
        "threadId": "codex-thread",
        "projectId": "project-1",
        "title": "A Codex conversation",
        "modelSelection": {"instanceId": "codex", "model": "gpt-5.4-mini"},
        "runtimeMode": runtime_mode,
        "interactionMode": "default",
        "branch": Value::Null,
        "worktreePath": Value::Null,
        "createdAt": "2026-07-31T00:00:00.000Z"
    })
}

fn codex_follow_up_in(
    message_id: &str,
    text: &str,
    model: &str,
    runtime_mode: &str,
) -> Value {
    json!({
        "type": "thread.turn.start",
        "commandId": format!("test:turn:{message_id}"),
        "threadId": "codex-thread",
        "message": {
            "messageId": message_id,
            "role": "user",
            "text": text,
            "attachments": [],
        },
        "modelSelection": {"instanceId": "codex", "model": model},
        "runtimeMode": runtime_mode,
        "interactionMode": "default",
        "createdAt": "2026-07-31T00:00:01.000Z",
    })
}

fn codex_follow_up_using_thread_settings(message_id: &str, text: &str) -> Value {
    json!({
        "type": "thread.turn.start",
        "commandId": format!("test:turn:{message_id}"),
        "threadId": "codex-thread",
        "message": {
            "messageId": message_id,
            "role": "user",
            "text": text,
            "attachments": [],
        },
        "createdAt": "2026-07-31T00:00:01.000Z",
    })
}

fn codex_follow_up_with_attachments(message_id: &str, text: &str, attachments: Value) -> Value {
    let mut command = codex_follow_up_using_thread_settings(message_id, text);
    command["message"]["attachments"] = attachments;
    command
}

fn set_codex_runtime_mode(runtime_mode: &str) -> Value {
    json!({
        "type": "thread.runtime-mode.set",
        "commandId": format!("test:runtime-mode:{runtime_mode}"),
        "threadId": "codex-thread",
        "runtimeMode": runtime_mode,
        "createdAt": "2026-07-31T00:00:01.000Z",
    })
}

fn aggregates(events: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = events
        .iter()
        .filter(|item| item["kind"] == "event")
        .filter_map(|item| item["event"]["aggregateId"].as_str().map(str::to_string))
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

async fn open_shell(client: &mut SocketClient) -> String {
    let subscription = client
        .subscribe("orchestration.subscribeShell", json!({}))
        .await;
    client.next_chunk(&subscription).await;
    client.ack(&subscription).await;
    subscription
}

#[tokio::test]
async fn codex_receives_text_and_images_as_complete_ordered_turn_inputs() {
    let codex = ScriptedCodex::plain_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    client.call("orchestration.dispatchCommand", codex_thread("full-access")).await.expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client.call("orchestration.dispatchCommand", codex_follow_up_with_attachments(
        "codex-images", "compare in order", json!([
            {"type":"image","name":"one.png","mimeType":"image/png","sizeBytes":2,"dataUrl":"data:image/png;base64,aGk="},
            {"type":"image","name":"two.webp","mimeType":"image/webp","sizeBytes":3,"dataUrl":"data:image/webp;base64,dHdv"}
        ]),
    )).await.expect_success();
    client.events_through_the_turn(&subscription).await;
    let turns = codex.turn_start_requests();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["params"]["input"], json!([
        {"type":"text","text":"compare in order"},
        {"type":"image","url":"data:image/png;base64,aGk="},
        {"type":"image","url":"data:image/webp;base64,dHdv"}
    ]));
    client.close().await; server.stop().await; codex.assert_conversation_reaped();
}

#[tokio::test]
async fn codex_receives_image_only_bootstrap_and_refuses_an_unresolved_image() {
    const IMAGE_ONLY: &str = "[User attached one or more images without additional text. Respond using the conversation context and the attached image(s).]";
    let codex = ScriptedCodex::plain_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    client.call("orchestration.dispatchCommand", codex_thread("full-access")).await.expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client.call("orchestration.dispatchCommand", codex_follow_up_with_attachments(
        "codex-image-only", IMAGE_ONLY, json!([
            {"type":"image","name":"only.gif","mimeType":"image/gif","sizeBytes":2,"dataUrl":"data:image/gif;base64,aGk="}
        ]),
    )).await.expect_success();
    client.events_through_the_turn(&subscription).await;
    assert_eq!(codex.turn_start_requests()[0]["params"]["input"], json!([
        {"type":"text","text":IMAGE_ONLY}, {"type":"image","url":"data:image/gif;base64,aGk="}
    ]));
    client.call("orchestration.dispatchCommand", codex_follow_up_with_attachments(
        "codex-missing", "do not send", json!([
            {"type":"image","id":"missing-image","name":"missing.png","mimeType":"image/png","sizeBytes":2}
        ]),
    )).await.expect_declared("OrchestrationDispatchCommandError");
    assert_eq!(codex.turn_start_requests().len(), 1);
    client.close().await; server.stop().await; codex.assert_conversation_reaped();
}

#[tokio::test]
async fn codex_refuses_a_stored_image_that_becomes_unreadable_before_dispatch() {
    let codex = ScriptedCodex::conversation_paused_after_first_delta();
    let workspace = Workspace::with(&["src/main.rs"]);
    let preferences = tempfile::tempdir().expect("persistent test preferences");
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_persistent_with_config_in(preferences.path(), config).await;
    let mut client = server.connect().await;
    client.call("orchestration.dispatchCommand", create_project("project-1", workspace.path())).await.expect_success();
    client.call("orchestration.dispatchCommand", codex_thread("full-access")).await.expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client.call("orchestration.dispatchCommand", follow_up("codex-thread", "message-1", "First turn.")).await.expect_success();
    client.events_until_streaming(&subscription).await;
    client.call("orchestration.dispatchCommand", codex_follow_up_with_attachments(
        "queued-image", "inspect", json!([
            {"type":"image","name":"queued.png","mimeType":"image/png","sizeBytes":2,"dataUrl":"data:image/png;base64,aGk="}
        ]),
    )).await.expect_success();
    std::fs::remove_file(preferences.path().join("attachments/queued-image-0.png")).expect("removes stored image before dispatch");
    codex.release_turn();
    let mut events = client.events_through_the_turn(&subscription).await;
    while !events.iter().any(|item| item["event"]["payload"]["session"]["status"] == "error") {
        events.extend(client.events_through_the_turn(&subscription).await);
    }
    assert_eq!(codex.turn_start_requests().len(), 1, "Codex received an incomplete queued turn");
    assert!(events.iter().any(|item| item["event"]["payload"]["session"]["lastError"]
        .as_str().is_some_and(|error| error.contains("could not be read"))));
    client.close().await; server.stop().await; codex.assert_conversation_reaped();
}

fn title_on_shell(snapshot: &Value, thread_id: &str) -> String {
    snapshot["threads"]
        .as_array()
        .expect("the shell carries threads")
        .iter()
        .find(|thread| thread["id"] == thread_id)
        .unwrap_or_else(|| panic!("{thread_id} is absent from the shell: {snapshot:#?}"))["title"]
        .as_str()
        .expect("the thread has a title")
        .to_string()
}

#[tokio::test]
async fn a_native_codex_name_updates_every_title_projection_and_survives_restart() {
    let codex = ScriptedCodex::plain_conversation();
    let data = tempfile::tempdir().expect("a temporary data directory");
    let database = data.path().join("registry.db");
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_at_with_config(&database, config.clone()).await;
    let mut author = server.connect().await;
    author
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    author
        .call("orchestration.dispatchCommand", codex_thread("full-access"))
        .await
        .expect_success();

    let mut watcher = server.connect().await;
    let thread = watcher.watch_conversation("codex-thread").await;
    let shell = open_shell(&mut watcher).await;
    author
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "Give this conversation a useful name."),
        )
        .await
        .expect_success();

    let events = watcher.events_through_the_turn(&thread).await;
    let title_events: Vec<&Value> = events
        .iter()
        .filter(|item| item["event"]["type"] == "thread.meta-updated")
        .collect();
    assert_eq!(title_events.len(), 1, "foreign or blank names leaked: {events:#?}");
    assert_eq!(
        title_events[0]["event"]["payload"]["title"],
        "A native Codex title"
    );
    watcher
        .values_until(&shell, |item| {
            item["kind"] == "thread-upserted"
                && item["thread"]["title"] == "A native Codex title"
        })
        .await;

    let fresh_thread = server
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    assert_eq!(fresh_thread["thread"]["title"], "A native Codex title");
    let fresh_shell = server.connect().await.into_shell_snapshot().await;
    assert_eq!(title_on_shell(&fresh_shell, "codex-thread"), "A native Codex title");
    author.close().await;
    watcher.close().await;
    server.stop().await;

    let restarted = TestServer::start_at_with_config(&database, config).await;
    let restored_thread = restarted
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    assert_eq!(restored_thread["thread"]["title"], "A native Codex title");
    let restored_shell = restarted.connect().await.into_shell_snapshot().await;
    assert_eq!(title_on_shell(&restored_shell, "codex-thread"), "A native Codex title");
    restarted.stop().await;
    codex.assert_conversation_reaped();
}

async fn complete_first_codex_turn(
    codex: &ScriptedCodex,
    database: &std::path::Path,
    workspace: &Workspace,
) {
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_at_with_config(database, config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up(
                "codex-thread",
                "message-1",
                "Reply with exactly one short sentence saying hello. Do not use any tools.",
            ),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_codex_thread_id_survives_a_restart_and_resumes_the_captured_context() {
    let codex = ScriptedCodex::resumable_conversation();
    let data = tempfile::tempdir().expect("a temporary data directory");
    let database = data.path().join("registry.db");
    let workspace = Workspace::with(&["src/main.rs"]);
    complete_first_codex_turn(&codex, &database, &workspace).await;

    rusqlite::Connection::open(&database)
        .expect("opens the stored conversation")
        .execute(
            "UPDATE threads SET agent_session_id = 'obsolete-legacy-thread' WHERE id = 'codex-thread'",
            [],
        )
        .expect("makes the legacy continuation disagree with the migrated cursor");

    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_at_with_config(&database, config).await;
    let mut client = server.connect().await;
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up(
                "codex-thread",
                "message-2",
                "What exactly did I ask you in my previous message? Quote it.",
            ),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        assistant_sends(&events).last(),
        Some(&(
            "\u{201c}Reply with exactly one short sentence saying hello. Do not use any tools.\u{201d}"
                .to_string(),
            false,
        )),
        "the app-server did not replay the answer that demonstrates earlier context"
    );
    let requests: Vec<Value> = codex.thread_requests().into_iter().skip(1).collect();
    let [resume] = requests.as_slice() else {
        panic!("the restarted conversation made unexpected thread requests: {requests:?}");
    };
    assert_eq!(resume["method"], "thread/resume");
    assert_eq!(resume["params"]["threadId"], "codex-thread-1");
    assert_eq!(resume["params"]["approvalPolicy"], "never");
    assert_eq!(resume["params"]["sandbox"], "danger-full-access");
    let resumed_turn = codex
        .turn_start_requests()
        .into_iter()
        .last()
        .expect("the resumed turn request");
    assert_eq!(resumed_turn["params"]["model"], "gpt-5.4-mini");
    assert_eq!(resumed_turn["params"]["approvalPolicy"], "never");
    assert_eq!(
        resumed_turn["params"]["sandboxPolicy"],
        json!({"type": "dangerFullAccess"})
    );
    assert_eq!(resumed_turn["params"]["approvalsReviewer"], "user");

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn incompatible_codex_cursors_fail_visibly_after_a_restart() {
    for (cursor, expected) in [
        (json!({"version": 1}), "incompatible"),
        (
            json!({"version": 2, "threadId": "future-thread"}),
            "newer than this build supports",
        ),
    ] {
        let codex = ScriptedCodex::resumable_conversation();
        let data = tempfile::tempdir().expect("a temporary data directory");
        let database = data.path().join("registry.db");
        let workspace = Workspace::with(&["src/main.rs"]);
        complete_first_codex_turn(&codex, &database, &workspace).await;

        rusqlite::Connection::open(&database)
            .expect("opens the stored conversation")
            .execute(
                "UPDATE threads SET provider_resume_cursor = ?1 WHERE id = 'codex-thread'",
                [cursor.to_string()],
            )
            .expect("writes the incompatible cursor");

        let mut config = ServerConfig::detect();
        config.settings.providers.codex.binary_path = codex.configured();
        let restarted = TestServer::start_at_with_config(&database, config).await;
        let mut client = restarted.connect().await;
        let subscription = client.watch_conversation("codex-thread").await;
        client
            .call(
                "orchestration.dispatchCommand",
                follow_up("codex-thread", "message-2", "after the restart"),
            )
            .await
            .expect_success();
        let events = client.events_through_the_turn(&subscription).await;
        let failed = &activity(&events, "session.failed")["payload"]["activity"];
        assert!(failed["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains(expected)));
        assert_eq!(
            codex.thread_requests().len(),
            1,
            "an incompatible cursor started Codex"
        );
        client.close().await;
        restarted.stop().await;
    }
}

#[tokio::test]
async fn malformed_startup_traffic_is_counted_after_the_correlated_response_and_open_continues() {
    let codex = ScriptedCodex::initialization_drift_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "Say hello after startup noise."),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        assistant_sends(&events).last(),
        Some(&("Hello.".to_string(), false))
    );
    assert_eq!(
        last_session(&events, "the turn after startup drift")["payload"]["session"]["status"],
        "ready"
    );
    let completed = &activity(&events, "turn.completed")["payload"]["activity"];
    assert!(completed["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("4 unrecognised events and 2 unreadable lines")));
    assert_eq!(completed["payload"]["unknownEvents"], 4);
    assert_eq!(completed["payload"]["parseErrors"], 2);
    assert_eq!(
        codex.unsupported_answers(),
        vec![json!({
            "jsonrpc": "2.0",
            "id": "startup-request",
            "error": {
                "code": -32601,
                "message": "laplus does not handle app-server request 'future/request' during a provider probe"
            }
        })]
    );

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn malformed_noise_before_resume_does_not_replace_the_captured_continuity() {
    let codex = ScriptedCodex::resumable_conversation();
    let data = tempfile::tempdir().expect("a temporary data directory");
    let database = data.path().join("registry.db");
    let workspace = Workspace::with(&["src/main.rs"]);
    complete_first_codex_turn(&codex, &database, &workspace).await;
    codex.add_resume_drift();

    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_at_with_config(&database, config).await;
    let mut client = server.connect().await;
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up(
                "codex-thread",
                "message-2",
                "What exactly did I ask you in my previous message? Quote it.",
            ),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        assistant_sends(&events).last(),
        Some(&(
            "\u{201c}Reply with exactly one short sentence saying hello. Do not use any tools.\u{201d}"
                .to_string(),
            false,
        )),
        "malformed startup noise caused a fresh thread to lose captured context"
    );
    assert!(!activities(&events)
        .iter()
        .any(|row| row["kind"] == "session.resume-failed"));
    let requests: Vec<Value> = codex.thread_requests().into_iter().skip(1).collect();
    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        vec!["thread/resume"]
    );
    let completed = &activity(&events, "turn.completed")["payload"]["activity"];
    assert!(completed["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("1 unrecognised event and 1 unreadable line")));
    assert_eq!(completed["payload"]["unknownEvents"], 1);
    assert_eq!(completed["payload"]["parseErrors"], 1);

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn an_empty_correlated_turn_start_result_is_counted_and_the_completion_still_settles() {
    let codex = ScriptedCodex::malformed_turn_start_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "Settle despite a malformed result."),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        last_session(&events, "the malformed correlated turn")["payload"]["session"]["status"],
        "ready"
    );
    let completed = &activity(&events, "turn.completed")["payload"]["activity"];
    assert!(completed["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("1 unrecognised event")));
    assert_eq!(completed["payload"]["unknownEvents"], 1);
    assert_eq!(completed["payload"]["parseErrors"], 0);

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn the_captured_missing_rollout_starts_fresh_and_tells_the_developer() {
    let data = tempfile::tempdir().expect("a temporary data directory");
    let database = data.path().join("registry.db");
    let workspace = Workspace::with(&["src/main.rs"]);
    let initial = ScriptedCodex::plain_conversation();
    complete_first_codex_turn(&initial, &database, &workspace).await;
    initial.assert_conversation_reaped();

    let codex = ScriptedCodex::missing_resume_conversation();
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_at_with_config(&database, config).await;
    let mut client = server.connect().await;
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-2", "Carry on after the restart."),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let failed = &activity(&events, "session.resume-failed")["payload"]["activity"];
    assert_eq!(failed["tone"], "error");
    let summary = failed["summary"].as_str().expect("a resume explanation");
    assert!(summary.contains("codex-thread-1"), "{summary}");
    assert!(summary.contains("previous context"), "{summary}");
    assert!(
        summary.contains("no rollout found for thread id codex-thread-1"),
        "{summary}"
    );
    assert_eq!(
        last_session(&events, "the fallback turn")["payload"]["session"]["status"],
        "ready",
        "the recoverable resume failure killed the conversation: {events:#?}"
    );
    let requests = codex.thread_requests();
    assert_eq!(
        requests
            .iter()
            .map(|request| request["method"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        vec!["thread/resume", "thread/start"]
    );
    assert_eq!(
        codex
            .turn_start_requests()
            .last()
            .expect("the fallback turn request")["params"]["threadId"],
        "codex-thread-fresh"
    );
    codex.assert_missing_resume_capture_prefix();

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn an_unclassified_resume_error_takes_the_same_recoverable_fallback() {
    const REFUSAL: &str = "history service refused this account";
    let data = tempfile::tempdir().expect("a temporary data directory");
    let database = data.path().join("registry.db");
    let workspace = Workspace::with(&["src/main.rs"]);
    let initial = ScriptedCodex::plain_conversation();
    complete_first_codex_turn(&initial, &database, &workspace).await;
    initial.assert_conversation_reaped();

    let codex = ScriptedCodex::arbitrary_resume_failure_conversation(REFUSAL);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_at_with_config(&database, config).await;
    let mut client = server.connect().await;
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-2", "Try after an unrelated error."),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let summary = activity(&events, "session.resume-failed")["payload"]["activity"]["summary"]
        .as_str()
        .expect("a resume explanation");
    assert!(summary.contains(REFUSAL), "{summary}");
    assert_eq!(
        last_session(&events, "the fallback turn")["payload"]["session"]["status"],
        "ready"
    );
    let methods: Vec<String> = codex
        .thread_requests()
        .into_iter()
        .filter_map(|request| request["method"].as_str().map(str::to_string))
        .collect();
    assert_eq!(methods, vec!["thread/resume", "thread/start"]);

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

struct AskedCodex {
    server: TestServer,
    client: SocketClient,
    subscription: String,
    request_id: String,
    events: Vec<Value>,
    _workspace: Workspace,
}

async fn ask_codex(codex: &ScriptedCodex) -> AskedCodex {
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call("orchestration.dispatchCommand", codex_thread("approval-required"))
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_in(
                "codex-thread",
                "message-1",
                "Write hi to hello.txt with a shell command.",
                "approval-required",
            ),
        )
        .await
        .expect_success();
    let (events, request_id) = client.events_until_permission(&subscription).await;
    AskedCodex {
        server,
        client,
        subscription,
        request_id,
        events,
        _workspace: workspace,
    }
}

#[tokio::test]
async fn a_captured_sandbox_escape_waits_with_only_its_supported_decisions() {
    let codex = ScriptedCodex::approval_conversation();
    let asked = ask_codex(&codex).await;
    let work_log = activities(&asked.events);
    let request = activity(&asked.events, "approval.requested");
    let payload = &request["payload"]["activity"]["payload"];

    assert_eq!(payload["requestId"], asked.request_id);
    assert_eq!(payload["requestKind"], "command");
    assert_eq!(payload["availableDecisions"], json!(["accept", "cancel"]));
    assert_eq!(payload["data"]["toolCallId"], "command-3");
    assert_eq!(codex.approval_answers(), Vec::<Value>::new());
    let relevant: Vec<&str> = work_log
        .iter()
        .filter_map(|row| row["kind"].as_str())
        .filter(|kind| *kind == "tool.updated" || *kind == "approval.requested")
        .collect();
    assert_eq!(relevant, vec!["tool.updated", "approval.requested"]);

    asked.client.close().await;
    asked.server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn every_runtime_mode_reaches_codex_as_its_approval_policy_and_sandbox() {
    for (mode, approval_policy, sandbox) in [
        ("approval-required", "untrusted", "read-only"),
        ("auto-accept-edits", "on-request", "workspace-write"),
        ("auto", "on-request", "workspace-write"),
        ("full-access", "never", "danger-full-access"),
    ] {
        let codex = ScriptedCodex::conversation_paused_after_first_delta();
        let workspace = Workspace::with(&["src/main.rs"]);
        let mut config = ServerConfig::detect();
        config.settings.providers.codex.binary_path = codex.configured();
        let server = TestServer::start_with(config).await;
        let mut client = server.connect().await;
        client
            .call(
                "orchestration.dispatchCommand",
                create_project("project-1", workspace.path()),
            )
            .await
            .expect_success();
        client
            .call("orchestration.dispatchCommand", codex_thread(mode))
            .await
            .expect_success();
        let subscription = client.watch_conversation("codex-thread").await;
        client
            .call(
                "orchestration.dispatchCommand",
                follow_up_in("codex-thread", "message-1", "Say hello.", mode),
            )
            .await
            .expect_success();
        client
            .values_until(&subscription, |item| {
                item["event"]["type"] == "thread.message-sent"
                    && item["event"]["payload"]["role"] == "assistant"
            })
            .await;

        let requests = codex.thread_requests();
        let [opened] = requests.as_slice() else {
            panic!("{mode} opened Codex with unexpected requests: {requests:?}");
        };
        assert_eq!(opened["params"]["approvalPolicy"], approval_policy);
        assert_eq!(opened["params"]["sandbox"], sandbox);
        assert_eq!(opened["params"]["approvalsReviewer"], "user");

        codex.release_turn();
        client.events_through_the_turn(&subscription).await;
        client.close().await;
        server.stop().await;
        codex.assert_conversation_reaped();
    }
}

#[tokio::test]
async fn a_model_changed_between_codex_turns_applies_without_replacing_the_conversation() {
    let codex = ScriptedCodex::command_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "First turn."),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    client
        .call(
            "orchestration.dispatchCommand",
            codex_follow_up_in("message-2", "Second turn.", "gpt-5.5", "full-access"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let turns = codex.turn_start_requests();
    assert_eq!(turns.len(), 2, "unexpected Codex turns: {turns:#?}");
    assert_eq!(turns[1]["params"]["model"], "gpt-5.5");
    assert_eq!(turns[1]["params"]["approvalPolicy"], "never");
    assert_eq!(turns[1]["params"]["sandboxPolicy"], json!({"type": "dangerFullAccess"}));
    assert_eq!(turns[1]["params"]["approvalsReviewer"], "user");
    assert_eq!(codex.conversation_starts(), 1);
    assert_eq!(codex.thread_requests().len(), 1);
    assert!(!activities(&events)
        .iter()
        .any(|activity| activity["kind"] == "session.retune-failed"));

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn codex_token_usage_fills_the_context_window_meter() {
    let codex = ScriptedCodex::context_usage_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call("orchestration.dispatchCommand", codex_thread("full-access"))
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "Say hello."),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let usage = activity(&events, "context-window.updated");
    assert_eq!(usage["payload"]["activity"]["payload"], json!({
        "usedTokens": 12_500,
        "totalProcessedTokens": 42_000,
        "maxTokens": 200_000,
        "inputTokens": 12_000,
        "outputTokens": 500,
        "lastUsedTokens": 12_500,
        "compactsAutomatically": true
    }));

    let snapshot = server.connect().await.into_thread_snapshot("codex-thread").await;
    assert!(snapshot["thread"]["activities"].as_array().unwrap().iter().any(|row| {
        row["kind"] == "context-window.updated" && row["payload"]["usedTokens"] == 12_500
    }));
    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn an_access_mode_changed_between_codex_turns_applies_consistently_to_the_next_turn() {
    let codex = ScriptedCodex::command_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "First turn."),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    client
        .call(
            "orchestration.dispatchCommand",
            set_codex_runtime_mode("approval-required"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            codex_follow_up_using_thread_settings("message-2", "Second turn."),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let turns = codex.turn_start_requests();
    assert_eq!(turns.len(), 2, "unexpected Codex turns: {turns:#?}");
    assert_eq!(turns[1]["params"]["model"], "gpt-5.4-mini");
    assert_eq!(turns[1]["params"]["approvalPolicy"], "untrusted");
    assert_eq!(turns[1]["params"]["sandboxPolicy"], json!({"type": "readOnly"}));
    assert_eq!(turns[1]["params"]["approvalsReviewer"], "user");
    let sessions: Vec<&Value> = events
        .iter()
        .filter(|item| item["event"]["type"] == "thread.session-set")
        .map(|item| &item["event"]["payload"]["session"])
        .collect();
    assert!(sessions.iter().any(|session| session["status"] == "starting"));
    assert!(sessions.iter().any(|session| session["status"] == "running"));
    assert!(sessions.iter().any(|session| session["status"] == "ready"));
    assert!(sessions
        .iter()
        .all(|session| session["runtimeMode"] == "approval-required"));
    assert_eq!(codex.conversation_starts(), 1);
    assert_eq!(codex.thread_requests().len(), 1);
    assert!(!activities(&events)
        .iter()
        .any(|activity| activity["kind"] == "session.retune-failed"));

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn queued_codex_prompts_keep_the_model_and_access_mode_they_were_sent_with() {
    let codex = ScriptedCodex::conversation_paused_after_first_delta();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "First turn."),
        )
        .await
        .expect_success();
    let mut events = client.events_until_streaming(&subscription).await;

    client
        .call(
            "orchestration.dispatchCommand",
            codex_follow_up_in("message-2", "Second turn.", "gpt-5.5", "auto"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            codex_follow_up_in(
                "message-3",
                "Third turn.",
                "gpt-5.6",
                "approval-required",
            ),
        )
        .await
        .expect_success();

    assert_eq!(
        codex.turn_start_requests().len(),
        1,
        "a queued prompt retuned the turn already running"
    );
    codex.release_turn();
    let mut completed = 0;
    while completed < 3 {
        let next = client.events_through_the_turn(&subscription).await;
        completed += activities(&next)
            .iter()
            .filter(|activity| activity["kind"] == "turn.completed")
            .count();
        events.extend(next);
    }

    let turns = codex.turn_start_requests();
    assert_eq!(turns.len(), 3, "unexpected Codex turns: {turns:#?}");
    assert!(turns[0]["params"].get("model").is_none());
    assert!(turns[0]["params"].get("approvalPolicy").is_none());
    assert_eq!(turns[1]["params"]["model"], "gpt-5.5");
    assert_eq!(turns[1]["params"]["approvalPolicy"], "on-request");
    assert_eq!(turns[1]["params"]["sandboxPolicy"], json!({"type": "workspaceWrite"}));
    assert_eq!(turns[1]["params"]["approvalsReviewer"], "user");
    assert_eq!(turns[2]["params"]["model"], "gpt-5.6");
    assert_eq!(turns[2]["params"]["approvalPolicy"], "untrusted");
    assert_eq!(turns[2]["params"]["sandboxPolicy"], json!({"type": "readOnly"}));
    assert_eq!(turns[2]["params"]["approvalsReviewer"], "user");

    // Model selection is a thread/turn property in the TypeScript contract;
    // OrchestrationSession carries runtimeMode but deliberately has no model.
    let requested: Vec<(&str, &str)> = events
        .iter()
        .filter(|item| item["event"]["type"] == "thread.turn-start-requested")
        .map(|item| {
            let payload = &item["event"]["payload"];
            (
                payload["modelSelection"]["model"].as_str().unwrap_or(""),
                payload["runtimeMode"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        requested,
        vec![
            ("gpt-5.4-mini", "full-access"),
            ("gpt-5.5", "auto"),
            ("gpt-5.6", "approval-required"),
        ]
    );

    let mut session_modes = std::collections::HashMap::<String, Vec<String>>::new();
    for session in events
        .iter()
        .filter(|item| item["event"]["type"] == "thread.session-set")
        .map(|item| &item["event"]["payload"]["session"])
    {
        let Some(turn_id) = session["activeTurnId"].as_str() else {
            continue;
        };
        session_modes
            .entry(turn_id.to_string())
            .or_default()
            .push(session["runtimeMode"].as_str().unwrap_or("").to_string());
    }
    let modes: Vec<String> = session_modes
        .values()
        .map(|modes| {
            assert!(modes.iter().all(|mode| mode == &modes[0]), "{modes:?}");
            modes[0].clone()
        })
        .collect();
    assert_eq!(session_modes.len(), 3, "session events: {session_modes:?}");
    for expected in ["full-access", "auto", "approval-required"] {
        assert!(modes.iter().any(|mode| mode == expected), "{session_modes:?}");
    }
    let snapshot = server.connect().await.into_thread_snapshot("codex-thread").await;
    assert_eq!(
        snapshot["thread"]["modelSelection"]["model"],
        "gpt-5.6",
        "the thread did not retain the model published for its latest turn"
    );
    assert_eq!(codex.conversation_starts(), 1);
    assert_eq!(codex.thread_requests().len(), 1);

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn a_codex_turn_that_refuses_its_retune_reports_the_failure_to_the_developer() {
    let codex = ScriptedCodex::command_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "First turn."),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    codex.reject_turns();
    client
        .call(
            "orchestration.dispatchCommand",
            codex_follow_up_in(
                "message-2",
                "Retuned turn.",
                "gpt-5.5",
                "approval-required",
            ),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let failed = activity(&events, "turn.completed");
    assert_eq!(failed["payload"]["activity"]["tone"], "error");
    assert!(failed["payload"]["activity"]["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("fixture turn start rejected")));
    let session = last_session(&events, "the refused retuned turn");
    assert_eq!(session["payload"]["session"]["status"], "error");
    assert!(session["payload"]["session"]["lastError"]
        .as_str()
        .is_some_and(|error| error.contains("fixture turn start rejected")));
    let request = codex
        .turn_start_requests()
        .into_iter()
        .last()
        .expect("the refused retuned request");
    assert_eq!(request["params"]["model"], "gpt-5.5");
    assert_eq!(request["params"]["approvalPolicy"], "untrusted");
    assert_eq!(request["params"]["sandboxPolicy"], json!({"type": "readOnly"}));
    assert_eq!(request["params"]["approvalsReviewer"], "user");

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn a_retuned_codex_turn_whose_request_cannot_be_written_reports_the_failure() {
    let codex = ScriptedCodex::command_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    codex.fail_next_turn_write();
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "First turn."),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    client
        .call(
            "orchestration.dispatchCommand",
            codex_follow_up_in("message-2", "Retuned turn.", "gpt-5.5", "full-access"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let failed = &activity(&events, "session.failed")["payload"]["activity"];
    assert_eq!(failed["tone"], "error");
    assert!(failed["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("could not be sent to the agent")));
    let session = last_session(&events, "the unwritable retuned turn");
    assert_eq!(session["payload"]["session"]["status"], "error");
    assert!(session["payload"]["session"]["lastError"]
        .as_str()
        .is_some_and(|error| error.contains("could not be sent to the agent")));
    assert_eq!(codex.turn_start_requests().len(), 1);

    let snapshot = server.connect().await.into_thread_snapshot("codex-thread").await;
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "error");

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn a_full_access_sandbox_escape_runs_without_a_permission_question() {
    let codex = ScriptedCodex::unrestricted_write_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up(
                "codex-thread",
                "message-1",
                "Write hi to hello.txt with a shell command.",
            ),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;
    let work_log = activities(&events);

    assert!(work_log.iter().any(|activity| {
        activity["kind"] == "tool.completed"
            && activity["payload"]["data"]["toolCallId"] == "command-3"
    }));
    assert!(!work_log
        .iter()
        .any(|activity| activity["kind"] == "approval.requested"));
    let requests = codex.thread_requests();
    let [opened] = requests.as_slice() else {
        panic!("full access did not open exactly one Codex thread");
    };
    assert_eq!(opened["params"]["approvalPolicy"], "never");
    assert_eq!(opened["params"]["sandbox"], "danger-full-access");

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn interrupt_keeps_late_deltas_and_the_same_codex_takes_the_correction() {
    let codex = ScriptedCodex::interrupted_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let data = tempfile::tempdir().expect("a temporary data directory");
    let database = data.path().join("registry.db");
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_at_with_config(&database, config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up(
                "codex-thread",
                "message-1",
                "Write a long essay about the history of text editors.",
            ),
        )
        .await
        .expect_success();
    let streaming = client.events_until_streaming(&subscription).await;
    let turn_id = harness::conversation::last_session(&streaming, "the running Codex turn")
        ["payload"]["session"]["activeTurnId"]
        .as_str()
        .expect("the running turn has an id")
        .to_string();

    client
        .call(
            "orchestration.dispatchCommand",
            interrupt_turn("codex-thread", Some(&turn_id)),
        )
        .await
        .expect_success();
    let before_acknowledgement = client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.message-sent"
                && item["event"]["payload"]["role"] == "assistant"
                && item["event"]["payload"]["text"] == " of text editors"
                && item["event"]["payload"]["streaming"] == true
        })
        .await;
    assert!(!activities(&before_acknowledgement)
        .iter()
        .any(|activity| activity["kind"] == "turn.completed"));
    codex.release_interrupt();
    let interrupted = client.events_through_the_turn(&subscription).await;

    let requests = codex.interrupt_requests();
    let [request] = requests.as_slice() else {
        panic!("Codex received unexpected interrupt requests: {requests:?}");
    };
    assert_eq!(request["params"]["threadId"], "codex-thread-4");
    assert_eq!(request["params"]["turnId"], "codex-turn-4");
    assert_eq!(
        harness::conversation::last_session(&interrupted, "the interrupted Codex turn")
            ["payload"]["session"]["status"],
        "interrupted"
    );
    let completed = activity(&interrupted, "turn.completed");
    assert_eq!(completed["payload"]["activity"]["payload"]["interrupted"], true);
    assert_eq!(completed["payload"]["activity"]["payload"]["isError"], false);
    assert_eq!(server.reconciliation().reconciled, 0);
    assert_eq!(server.live_agents(), 1, "the interrupt stopped app-server");

    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    let partial = snapshot["thread"]["messages"]
        .as_array()
        .expect("thread messages")
        .iter()
        .rfind(|message| message["role"] == "assistant")
        .expect("the partial Codex reply");
    assert_eq!(partial["text"], "The history of text editors");
    assert_eq!(partial["streaming"], false);

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-2", "Never mind, just say hello."),
        )
        .await
        .expect_success();
    let correction = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        assistant_sends(&correction).last(),
        Some(&("Hello.".to_string(), false))
    );
    assert_eq!(codex.conversation_starts(), 1);
    assert_eq!(codex.thread_requests().len(), 1);
    assert_eq!(codex.turn_requests(), 2);
    let turns = codex.turn_start_requests();
    assert_eq!(turns[1]["params"]["threadId"], "codex-thread-4");

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();

    let restarted = TestServer::start_at(&database).await;
    let restored = restarted
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    let partial = restored["thread"]["messages"]
        .as_array()
        .expect("restored messages")
        .iter()
        .find(|message| message["text"] == "The history of text editors")
        .expect("the interrupted reply survived restart");
    assert_eq!(partial["streaming"], false);
    restarted.stop().await;
}

#[tokio::test]
async fn file_approvals_publish_their_tool_context_before_the_panel() {
    for (codex, request_kind) in [
        (ScriptedCodex::file_read_approval_conversation(), "file-read"),
        (
            ScriptedCodex::file_change_approval_conversation(),
            "file-change",
        ),
    ] {
        let asked = ask_codex(&codex).await;
        let work_log = activities(&asked.events);
        let relevant: Vec<&Value> = work_log
            .iter()
            .copied()
            .filter(|row| row["kind"] == "tool.updated" || row["kind"] == "approval.requested")
            .collect();
        let [tool, approval] = relevant.as_slice() else {
            panic!("{request_kind} context and panel were not both published: {work_log:?}");
        };

        assert_eq!(tool["kind"], "tool.updated");
        assert_eq!(approval["kind"], "approval.requested");
        assert_eq!(approval["payload"]["requestKind"], request_kind);
        assert_eq!(
            tool["payload"]["data"]["toolCallId"],
            approval["payload"]["data"]["toolCallId"]
        );

        asked.client.close().await;
        asked.server.stop().await;
        codex.assert_conversation_reaped();
    }
}

#[tokio::test]
async fn approving_a_codex_request_answers_its_json_rpc_id_and_releases_the_turn() {
    let codex = ScriptedCodex::approval_conversation();
    let mut asked = ask_codex(&codex).await;
    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("codex-thread", &asked.request_id, "accept"),
        )
        .await
        .expect_success();
    let rest = asked.client.events_through_the_turn(&asked.subscription).await;

    assert_eq!(
        codex.approval_answers_through(1).await,
        vec![json!({"jsonrpc": "2.0", "id": 0, "result": {"decision": "accept"}})]
    );
    assert_eq!(
        activity(&rest, "approval.resolved")["payload"]["activity"]["payload"]["decision"],
        "accept"
    );
    assert_eq!(
        harness::conversation::last_session(&rest, "the approved Codex turn")["payload"]
            ["session"]["status"],
        "ready"
    );

    asked.client.close().await;
    asked.server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn declining_when_codex_offers_it_returns_control_without_stopping_the_turn() {
    let codex = ScriptedCodex::declinable_approval_conversation();
    let mut asked = ask_codex(&codex).await;
    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("codex-thread", &asked.request_id, "decline"),
        )
        .await
        .expect_success();
    let rest = asked
        .client
        .values_until(&asked.subscription, |item| {
            item["event"]["type"] == "thread.activity-appended"
                && item["event"]["payload"]["activity"]["kind"] == "approval.resolved"
        })
        .await;

    assert_eq!(
        codex.approval_answers_through(1).await[0]["result"]["decision"],
        "decline"
    );
    assert!(!rest
        .iter()
        .any(|item| item["event"]["type"] == "thread.turn-interrupt-requested"));
    assert_eq!(asked.server.live_agents(), 1, "decline ended the Codex session");

    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.session.stop",
                "commandId": "test:stop:declined-codex",
                "threadId": "codex-thread",
                "createdAt": "2026-07-31T00:00:02.000Z"
            }),
        )
        .await
        .expect_success();
    asked.server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn cancelling_a_codex_request_interrupts_the_turn() {
    let codex = ScriptedCodex::approval_conversation();
    let mut asked = ask_codex(&codex).await;
    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("codex-thread", &asked.request_id, "cancel"),
        )
        .await
        .expect_success();
    let rest = asked
        .client
        .values_until(&asked.subscription, |item| {
            item["event"]["type"] == "thread.turn-interrupt-requested"
        })
        .await;

    assert_eq!(
        codex.approval_answers_through(1).await[0]["result"]["decision"],
        "cancel"
    );
    let stopped = rest
        .iter()
        .find(|item| item["event"]["type"] == "thread.turn-interrupt-requested")
        .expect("cancel immediately marks the turn stopped");
    assert!(stopped["event"]["payload"]["turnId"].is_string());

    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.session.stop",
                "commandId": "test:stop:cancelled-codex",
                "threadId": "codex-thread",
                "createdAt": "2026-07-31T00:00:02.000Z"
            }),
        )
        .await
        .expect_success();
    asked.server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn an_unanswered_codex_approval_is_closed_when_the_session_ends() {
    let codex = ScriptedCodex::approval_conversation();
    let mut asked = ask_codex(&codex).await;
    asked
        .client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.session.stop",
                "commandId": "test:stop:codex-approval",
                "threadId": "codex-thread",
                "createdAt": "2026-07-31T00:00:01.000Z"
            }),
        )
        .await
        .expect_success();
    let ending = asked
        .client
        .values_until(&asked.subscription, |item| {
            item["event"]["type"] == "thread.session-set"
                && item["event"]["payload"]["session"]["status"] == "stopped"
        })
        .await;
    let resolved = activity(&ending, "approval.resolved");
    assert_eq!(
        resolved["payload"]["activity"]["payload"]["requestId"],
        asked.request_id
    );
    assert_eq!(
        resolved["payload"]["activity"]["payload"]["decision"],
        "cancel"
    );

    let snapshot = asked
        .server
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    let pending = snapshot["thread"]["activities"]
        .as_array()
        .expect("stored activities")
        .iter()
        .filter(|row| row["kind"] == "approval.requested")
        .count()
        - snapshot["thread"]["activities"]
            .as_array()
            .expect("stored activities")
            .iter()
            .filter(|row| row["kind"] == "approval.resolved")
            .count();
    assert_eq!(pending, 0, "the stored work log still has an open approval");

    asked.client.close().await;
    asked.server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn a_codex_turn_streams_settles_reuses_its_process_and_is_reaped() {
    let codex = ScriptedCodex::conversation_paused_after_first_delta();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up(
                "codex-thread",
                "message-1",
                "Reply with exactly one short sentence saying hello. Do not use any tools.",
            ),
        )
        .await
        .expect_success();

    let streaming = client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.message-sent"
                && item["event"]["payload"]["role"] == "assistant"
        })
        .await;
    assert_eq!(
        assistant_sends(&streaming),
        vec![("Hello".to_string(), true)]
    );
    let running = streaming
        .iter()
        .filter_map(|item| item["event"]["payload"]["session"]["status"].as_str())
        .collect::<Vec<_>>();
    assert!(running.contains(&"running"), "statuses before release: {running:?}");
    assert_eq!(server.live_agents(), 1);

    codex.release_turn();
    let completed = client.events_through_the_turn(&subscription).await;
    let sends = assistant_sends(&completed);
    assert_eq!(sends.last(), Some(&("Hello.".to_string(), false)));
    assert!(streaming.iter().chain(&completed).any(|item| {
        item["event"]["type"] == "thread.activity-appended"
            && item["event"]["payload"]["activity"]["kind"] == "task.progress"
    }));
    let settled = completed
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("the Codex turn settles");
    assert_eq!(settled["event"]["payload"]["session"]["status"], "ready");
    assert_eq!(
        settled["event"]["payload"]["session"]["providerInstanceId"],
        "codex"
    );

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-2", "Say hello again."),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    assert_eq!(codex.conversation_starts(), 1);
    assert_eq!(codex.turn_requests(), 2);
    assert_eq!(
        std::fs::canonicalize(codex.conversation_cwd()).expect("the recorded conversation cwd"),
        std::fs::canonicalize(workspace.path()).expect("the workspace path")
    );

    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.session.stop",
                "commandId": "test:stop:codex-thread",
                "threadId": "codex-thread",
                "createdAt": "2026-07-31T00:00:01.000Z"
            }),
        )
        .await
        .expect_success();
    client
        .values_until(&subscription, |item| {
            item["event"]["type"] == "thread.session-set"
                && item["event"]["payload"]["session"]["status"] == "stopped"
        })
        .await;
    codex.assert_conversation_reaped();

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn codex_subagent_calls_publish_separate_operation_and_agent_lifecycles() {
    let codex = ScriptedCodex::subagent_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call("orchestration.dispatchCommand", codex_thread("full-access"))
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "Delegate a review."),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;
    let activities = events
        .iter()
        .filter(|event| event["event"]["type"] == "thread.activity-appended")
        .map(|event| &event["event"]["payload"]["activity"])
        .filter(|activity| activity["payload"]["itemType"] == "collab_agent_tool_call")
        .collect::<Vec<_>>();

    assert!(activities.iter().any(|activity| {
        activity["kind"] == "tool.updated"
            && activity["summary"] == "Starting subagent"
            && activity["payload"]["status"] == "inProgress"
            && activity["payload"]["data"]["toolCallId"] == "spawn-call-1"
    }));
    assert!(activities.iter().any(|activity| {
        activity["kind"] == "tool.completed"
            && activity["summary"] == "Spawned subagent"
            && activity["payload"]["status"] == "completed"
    }));
    assert!(activities.iter().any(|activity| {
        activity["kind"] == "tool.updated"
            && activity["payload"]["status"] == "inProgress"
            && activity["payload"]["data"]["toolCallId"] == "agent:child-thread-12345678"
    }));
    assert!(
        activities.iter().any(|activity| {
            activity["kind"] == "tool.completed"
                && activity["payload"]["status"] == "completed"
                && activity["payload"]["detail"] == "No defects found."
                && activity["payload"]["data"]["toolCallId"] == "agent:child-thread-12345678"
        }),
        "subagent activities: {activities:#?}"
    );
    assert_eq!(
        last_session(&events, "the subagent Codex turn")["payload"]["session"]["status"],
        "ready"
    );

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn codex_protocol_drift_is_per_turn_the_payload_is_cumulative_and_the_session_carries_on() {
    let codex = ScriptedCodex::synthetic_drift_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call("orchestration.dispatchCommand", codex_thread("full-access"))
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    for (message_id, expected_turn, expected_total, expected_parse_errors) in
        [("message-1", 17, 17, 1), ("message-2", 16, 33, 2)]
    {
        client
            .call(
                "orchestration.dispatchCommand",
                follow_up(
                    "codex-thread",
                    message_id,
                    "Keep going after protocol drift.",
                ),
            )
            .await
            .expect_success();
        let events = client.events_through_the_turn(&subscription).await;

        assert_eq!(
            assistant_sends(&events).last(),
            Some(&("Still here.".to_string(), false)),
            "recognized output after drift was lost"
        );
        assert_eq!(
            last_session(&events, "the drifting Codex turn")["payload"]["session"]["status"],
            "ready",
            "protocol drift killed the Codex session"
        );
        let completed = &activity(&events, "turn.completed")["payload"]["activity"];
        let summary = completed["summary"].as_str().expect("a turn summary");
        assert!(
            summary.contains(&format!(
                "{expected_turn} unrecognised events and 1 unreadable line"
            )),
            "the turn did not report only its own drift: {summary}"
        );
        assert_eq!(completed["payload"]["unknownEvents"], expected_total);
        assert_eq!(
            completed["payload"]["parseErrors"],
            expected_parse_errors,
            "the payload is the session's cumulative total"
        );
    }
    assert_eq!(codex.conversation_starts(), 1, "drift replaced the session");

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn a_captured_codex_command_reaches_the_socket_work_log() {
    let codex = ScriptedCodex::command_conversation();
    let workspace = Workspace::with(&["README.md", "main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("approval-required"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_in(
                "codex-thread",
                "message-1",
                "Run the shell command `ls` in the current directory, then tell me the file names you saw.",
                "approval-required",
            ),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;
    let work_log = activities(&events);
    let commands: Vec<&Value> = work_log
        .iter()
        .copied()
        .filter(|activity| activity["payload"]["itemType"] == "command_execution")
        .collect();

    assert_eq!(commands.len(), 2, "work log: {work_log:?}");
    let [started, completed] = commands.as_slice() else {
        unreachable!("the length was checked")
    };
    assert_eq!(started["kind"], "tool.updated");
    assert_eq!(started["tone"], "tool");
    assert_eq!(started["payload"]["status"], "inProgress");
    assert_eq!(started["payload"]["data"]["command"], "/bin/bash -lc ls");
    assert_eq!(started["payload"]["data"]["input"]["cwd"], "<workspace>");
    assert_eq!(started["payload"]["data"]["input"]["processId"], "40283");
    assert_eq!(completed["kind"], "tool.completed");
    assert_eq!(completed["payload"]["status"], "completed");
    assert_eq!(completed["payload"]["data"]["result"]["exitCode"], 0);
    assert_eq!(completed["payload"]["data"]["result"]["output"], "README.md\nmain.rs\n");
    assert_eq!(
        started["payload"]["data"]["toolCallId"],
        completed["payload"]["data"]["toolCallId"]
    );
    assert_eq!(
        completed["sequence"].as_i64(),
        started["sequence"].as_i64().map(|sequence| sequence + 1),
        "paired lifecycle rows stay adjacent for the UI fold"
    );
    assert!(!work_log
        .iter()
        .any(|activity| activity["kind"] == "approval.requested"));

    let completed_messages: Vec<String> = assistant_sends(&events)
        .into_iter()
        .filter_map(|(text, streaming)| (!streaming).then_some(text))
        .collect();
    assert_eq!(
        completed_messages,
        vec![
            "I'm checking the current directory contents with `ls`, then I'll report the names exactly as shown.".to_string(),
            "`README.md`\n`main.rs`".to_string(),
        ],
        "commentary and the final answer are separate transcript messages"
    );

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn turn_completed_error_fails_the_turn_and_records_codexs_reason() {
    let codex = ScriptedCodex::failed_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "fail this turn"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;

    let completed = events
        .iter()
        .find(|item| {
            item["event"]["type"] == "thread.activity-appended"
                && item["event"]["payload"]["activity"]["kind"] == "turn.completed"
        })
        .expect("the failed turn has a completion activity");
    assert_eq!(completed["event"]["payload"]["activity"]["tone"], "error");
    assert!(completed["event"]["payload"]["activity"]["summary"]
        .as_str()
        .is_some_and(|summary| summary.contains("fixture turn failed")));
    let session = events
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("the failed turn settles the session");
    assert_eq!(session["event"]["payload"]["session"]["status"], "error");
    assert!(session["event"]["payload"]["session"]["lastError"]
        .as_str()
        .is_some_and(|error| error.contains("fixture turn failed")));

    let snapshot = server
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    assert_eq!(snapshot["thread"]["latestTurn"]["state"], "error");

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn a_rejected_turn_start_fails_the_turn_with_the_correlated_reason() {
    let codex = ScriptedCodex::rejected_conversation();
    let workspace = Workspace::with(&["src/main.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    let server = TestServer::start_with(config).await;
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
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "message-1", "reject this turn"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&subscription).await;
    let session = events
        .iter()
        .rfind(|item| item["event"]["type"] == "thread.session-set")
        .expect("the rejected request settles the session");
    assert_eq!(session["event"]["payload"]["session"]["status"], "error");
    assert!(session["event"]["payload"]["session"]["lastError"]
        .as_str()
        .is_some_and(|error| error.contains("fixture turn start rejected")));

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}

#[tokio::test]
async fn codex_and_claude_conversations_run_concurrently_without_crossing() {
    let codex = ScriptedCodex::conversation_paused_after_first_delta();
    let claude = ScriptedAgent::replaying("02-streamed-turn");
    let codex_workspace = Workspace::with(&["codex.rs"]);
    let claude_workspace = Workspace::with(&["claude.rs"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    config.settings.providers.claude_agent.binary_path = claude.configured();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;

    for (id, workspace) in [
        ("project-1", &codex_workspace),
        ("project-2", &claude_workspace),
    ] {
        client
            .call(
                "orchestration.dispatchCommand",
                create_project(id, workspace.path()),
            )
            .await
            .expect_success();
    }
    client
        .call(
            "orchestration.dispatchCommand",
            codex_thread("full-access"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            create_thread("project-2", "claude-thread"),
        )
        .await
        .expect_success();
    let codex_subscription = client.watch_conversation("codex-thread").await;
    let claude_subscription = client.watch_conversation("claude-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("codex-thread", "codex-message", "say hello"),
        )
        .await
        .expect_success();
    let codex_started = client
        .values_until(&codex_subscription, |item| {
            item["event"]["type"] == "thread.message-sent"
                && item["event"]["payload"]["role"] == "assistant"
        })
        .await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("claude-thread", "claude-message", "say ok"),
        )
        .await
        .expect_success();
    let claude_events = client.events_through_the_turn(&claude_subscription).await;
    assert_eq!(server.live_agents(), 2);
    assert_eq!(aggregates(&codex_started), vec!["codex-thread"]);
    assert_eq!(aggregates(&claude_events), vec!["claude-thread"]);

    let while_codex_thinks = server
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    let claude_done = server
        .connect()
        .await
        .into_thread_snapshot("claude-thread")
        .await;
    assert_eq!(while_codex_thinks["thread"]["session"]["status"], "running");
    assert_eq!(
        while_codex_thinks["thread"]["session"]["providerInstanceId"],
        "codex"
    );
    assert_eq!(claude_done["thread"]["session"]["status"], "ready");
    assert_eq!(
        claude_done["thread"]["session"]["providerInstanceId"],
        "claudeAgent"
    );
    assert_eq!(claude_done["thread"]["messages"][1]["text"], "ok");
    assert_eq!(while_codex_thinks["thread"]["messages"][1]["text"], "Hello");

    codex.release_turn();
    client.events_through_the_turn(&codex_subscription).await;
    let codex_done = server
        .connect()
        .await
        .into_thread_snapshot("codex-thread")
        .await;
    assert_eq!(codex_done["thread"]["messages"][1]["text"], "Hello.");
    assert_eq!(claude_done["thread"]["messages"][1]["text"], "ok");
    assert_eq!(codex_done["thread"]["messages"].as_array().map(Vec::len), Some(2));
    assert_eq!(claude_done["thread"]["messages"].as_array().map(Vec::len), Some(2));

    client.close().await;
    server.stop().await;
    codex.assert_conversation_reaped();
}
