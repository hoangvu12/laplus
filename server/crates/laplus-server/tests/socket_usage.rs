//! The Usage report tracer bullet, through the real authenticated WebSocket.

mod harness;

use std::fs;

use harness::{ClientIdentity, Outcome, TestServer};
use laplus_server::config::ServerConfig;
use serde_json::{json, Value};

const GRANT_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange";
const BOOTSTRAP_TOKEN_TYPE: &str = "urn%3At3%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap";
const ACCESS_TOKEN_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token";

const PRIVATE_PROMPT: &str = "PRIVATE usage fixture prompt must stay on disk";
const PRIVATE_REPLY: &str = "PRIVATE usage fixture response must stay on disk";
const PRIVATE_TOOL: &str = "PRIVATE usage fixture tool output must stay on disk";

fn input() -> Value {
    json!({
        "sinceDay": "2026-08-09",
        "untilDay": "2026-08-09",
        "timeZone": "UTC",
    })
}

fn configured_claude_home(home: &std::path::Path) -> ServerConfig {
    let mut config = ServerConfig::detect();
    let home = home.display().to_string();
    config.settings.providers.claude_agent.home_path = home.clone();
    config.settings.provider_instances["claudeAgent"]["config"]["homePath"] = json!(home);
    config
}

fn write_one_claude_record(home: &std::path::Path) {
    let project = home.join("projects").join("-tmp-usage-fixture");
    fs::create_dir_all(&project).expect("the fixture project directory is created");
    let rows = [
        json!({
            "type": "user",
            "message": { "role": "user", "content": PRIVATE_PROMPT },
            "session_id": "usage-session-1",
            "uuid": "usage-user-1",
            "timestamp": "2026-08-09T12:00:00.000Z"
        }),
        json!({
            "type": "tool_result",
            "content": PRIVATE_TOOL,
            "session_id": "usage-session-1",
            "uuid": "usage-tool-1",
            "timestamp": "2026-08-09T12:00:00.500Z"
        }),
        json!({
            "type": "assistant",
            "message": {
                "model": "claude-usage-fixture",
                "id": "msg_usage_fixture",
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "text", "text": PRIVATE_REPLY }],
                "usage": {
                    "input_tokens": 11,
                    "cache_creation_input_tokens": 13,
                    "cache_read_input_tokens": 17,
                    "output_tokens": 19
                }
            },
            "session_id": "usage-session-1",
            "uuid": "usage-assistant-1",
            "timestamp": "2026-08-09T12:00:01.000Z",
            "request_id": "req_usage_fixture"
        }),
    ];
    fs::write(
        project.join("usage-session-1.jsonl"),
        rows.into_iter()
            .map(|row| serde_json::to_string(&row).expect("fixture row encodes"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("the fixture transcript is written");
}

#[tokio::test]
async fn an_authorized_claude_record_reaches_the_usage_summary_without_its_content() {
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    write_one_claude_record(claude_home.path());
    let server = TestServer::start_with(configured_claude_home(claude_home.path())).await;
    let mut client = server.connect().await;

    let summary = client
        .call("server.getUsageSummary", input())
        .await
        .expect_success();

    assert_eq!(summary["contractVersion"], 3);
    assert_eq!(summary["sinceDay"], "2026-08-09");
    assert_eq!(summary["untilDay"], "2026-08-09");
    assert_eq!(summary["timeZone"], "UTC");
    assert_eq!(
        summary["buckets"],
        json!([{
            "day": "2026-08-09",
            "provider": "claude",
            "model": "claude-usage-fixture",
            "totals": {
                "uncachedInputTokens": 11,
                "cachedInputTokens": 17,
                "cacheCreationTokens": 13,
                "outputTokens": 19,
                "reasoningTokens": 0
            },
            "costUsd": 0.0,
            "cacheSavingsUsd": 0.0,
            "costSource": "unpriced",
            "records": 1,
            "unpricedRecords": 1,
            "sessions": 1
        }])
    );

    let wire = serde_json::to_string(&summary).expect("the summary encodes");
    for secret in [PRIVATE_PROMPT, PRIVATE_REPLY, PRIVATE_TOOL] {
        assert!(
            !wire.contains(secret),
            "raw transcript content crossed the wire: {wire}"
        );
    }

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_real_grant_without_orchestration_read_gets_only_a_scope_refusal() {
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    write_one_claude_record(claude_home.path());
    let server = TestServer::start_with(configured_claude_home(claude_home.path())).await;

    let minted = server
        .post_json(
            "/api/auth/pairing-token",
            &json!({ "scopes": ["relay:read"] }),
        )
        .await;
    assert_eq!(minted.status, 200, "{}", minted.text);
    let credential = minted.body["credential"]
        .as_str()
        .expect("a pairing credential");
    let exchange = format!(
        "grant_type={GRANT_TYPE}&subject_token={credential}\
         &subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type={ACCESS_TOKEN_TYPE}"
    );
    let exchanged = server.post_form("/oauth/token", &exchange).await;
    assert_eq!(exchanged.status, 200, "{}", exchanged.text);
    let bearer = exchanged.body["access_token"]
        .as_str()
        .expect("an access token");
    let ticketed = server
        .post_as(
            "/api/auth/websocket-ticket",
            &ClientIdentity::anonymous().with_bearer(bearer),
        )
        .await;
    assert_eq!(ticketed.status, 200, "{}", ticketed.text);
    let ticket = ticketed.body["ticket"].as_str().expect("a socket ticket");
    let mut client = server
        .connect_as(ClientIdentity::anonymous().with_ticket(ticket))
        .await
        .expect("the limited grant opens a socket");

    let error = client
        .call("server.getUsageSummary", input())
        .await
        .expect_declared("EnvironmentAuthorizationError");
    assert_eq!(error["requiredScope"], "orchestration:read");
    let wire = serde_json::to_string(&error).expect("the refusal encodes");
    for secret in [
        PRIVATE_PROMPT,
        PRIVATE_REPLY,
        PRIVATE_TOOL,
        "claude-usage-fixture",
    ] {
        assert!(
            !wire.contains(secret),
            "the refusal disclosed transcript-derived data: {wire}"
        );
    }

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_usage_scan_does_not_hold_the_socket_read_loop() {
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    write_one_claude_record(claude_home.path());
    let server = TestServer::start_with(configured_claude_home(claude_home.path())).await;
    let mut client = server.connect().await;

    let usage = client.send_request("server.getUsageSummary", input()).await;
    let probe = client.send_request("server.probe", json!({})).await;
    assert!(
        matches!(client.await_outcome(&probe).await, Outcome::Success(value) if value == json!({}))
    );
    assert!(matches!(
        client.await_outcome(&usage).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}
