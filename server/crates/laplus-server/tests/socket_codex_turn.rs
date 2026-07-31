//! Codex turns at the socket boundary used by the real composer.

mod harness;

use harness::codex::ScriptedCodex;
use harness::agent::ScriptedAgent;
use harness::conversation::{
    activities, activity, assistant_sends, create_project, create_thread, follow_up,
    respond_to_approval,
};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
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
        .call("orchestration.dispatchCommand", codex_thread())
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
        codex.approval_answers(),
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

    assert_eq!(codex.approval_answers()[0]["result"]["decision"], "decline");
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

    assert_eq!(codex.approval_answers()[0]["result"]["decision"], "cancel");
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
        .call("orchestration.dispatchCommand", codex_thread())
        .await
        .expect_success();
    let subscription = client.watch_conversation("codex-thread").await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up(
                "codex-thread",
                "message-1",
                "Run the shell command `ls` in the current directory, then tell me the file names you saw.",
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
