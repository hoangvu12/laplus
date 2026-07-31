//! Codex turns at the socket boundary used by the real composer.

mod harness;

use harness::codex::ScriptedCodex;
use harness::agent::ScriptedAgent;
use harness::conversation::{assistant_sends, create_project, create_thread, follow_up};
use harness::workspace::Workspace;
use harness::TestServer;
use laplus_server::config::ServerConfig;
use serde_json::{json, Value};

fn codex_thread() -> Value {
    json!({
        "type": "thread.create",
        "commandId": "test:thread:codex-thread",
        "threadId": "codex-thread",
        "projectId": "project-1",
        "title": "A Codex conversation",
        "modelSelection": {"instanceId": "codex", "model": "gpt-5.4-mini"},
        "runtimeMode": "full-access",
        "interactionMode": "default",
        "branch": Value::Null,
        "worktreePath": Value::Null,
        "createdAt": "2026-07-31T00:00:00.000Z"
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
            codex_thread(),
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
        codex.conversation_cwd(),
        workspace.path().display().to_string()
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
        .call("orchestration.dispatchCommand", codex_thread())
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
        .call("orchestration.dispatchCommand", codex_thread())
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
        .call("orchestration.dispatchCommand", codex_thread())
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
