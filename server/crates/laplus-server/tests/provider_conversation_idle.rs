//! Idle eviction must release Claude and Codex processes and preserve continuation.
mod harness;

use harness::{
    agent::ScriptedAgent,
    codex::ScriptedCodex,
    conversation::{create_project, create_thread, follow_up, start_turn},
    workspace::Workspace,
    TestServer,
};
use laplus_server::config::ServerConfig;
use serde_json::json;
use std::time::Duration;

const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s-idle","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#;
const ANSWER: &str = r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"continued"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":11,"total_cost_usd":0.001}"#;

async fn await_eviction(server: &TestServer) {
    tokio::time::timeout(Duration::from_secs(15), server.await_live_agents(0))
        .await
        .expect("idle provider retained its process instead of being evicted");
}

#[tokio::test]
async fn idle_claude_releases_its_process_and_resumes_the_saved_session() {
    std::env::set_var("LAPLUS_TEST_CONVERSATION_IDLE_SECS", "1");
    let agent = ScriptedAgent::resuming_after_a_death(&[
        vec![INIT, ANSWER, DONE],
        vec![INIT, ANSWER, DONE],
    ]);
    let workspace = Workspace::with(&["src/"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.claude_agent.binary_path = agent.configured();
    // Title generation must not share this conversation-only scripted process.
    config.settings.text_generation_model_selection = json!({});
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "first", "hello"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    await_eviction(&server).await;
    assert_eq!(agent.starts(), 1);
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "second", "continue"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;
    assert_eq!(agent.starts(), 2);
    assert!(
        agent.arguments()[1].contains("--resume s-idle"),
        "continuation was lost: {:?}",
        agent.arguments()
    );
    await_eviction(&server).await;
    server.stop().await;
}

#[tokio::test]
async fn idle_codex_releases_its_process_and_resumes_the_saved_thread() {
    std::env::set_var("LAPLUS_TEST_CONVERSATION_IDLE_SECS", "1");
    let codex = ScriptedCodex::plain_conversation();
    let workspace = Workspace::with(&["src/"]);
    let mut config = ServerConfig::detect();
    config.settings.providers.codex.binary_path = codex.configured();
    // Title generation must not overwrite the conversation process capture.
    config.settings.text_generation_model_selection = json!({});
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    let mut thread = create_thread("project-1", "thread-1");
    thread["modelSelection"] = json!({"instanceId":"codex","model":"gpt-5.4-mini"});
    client
        .call("orchestration.dispatchCommand", thread)
        .await
        .expect_success();
    let subscription = client.watch_conversation("thread-1").await;
    for (index, message) in ["first", "second"].iter().enumerate() {
        let mut prompt = follow_up("thread-1", message, "continue");
        prompt["modelSelection"] = json!({"instanceId":"codex","model":"gpt-5.4-mini"});
        client
            .call("orchestration.dispatchCommand", prompt)
            .await
            .expect_success();
        client.events_through_the_turn(&subscription).await;
        await_eviction(&server).await;
        codex.assert_conversation_reaped();
        assert_eq!(codex.thread_requests().len(), index + 1);
    }
    let requests = codex.thread_requests();
    assert_eq!(requests[0]["method"], "thread/start");
    assert_eq!(requests[1]["method"], "thread/resume");
    assert_eq!(requests[1]["params"]["threadId"], "codex-thread-1");
    server.stop().await;
}
