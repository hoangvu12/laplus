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
    std::env::set_var("LAPLUS_USAGE_PRICING_URL", "disabled");
    let mut config = ServerConfig::detect();
    let home = home.display().to_string();
    config.settings.providers.claude_agent.home_path = home.clone();
    config.settings.provider_instances["claudeAgent"]["config"]["homePath"] = json!(home);
    config
}

fn configured_provider_homes(claude: &std::path::Path, codex: &std::path::Path) -> ServerConfig {
    let mut config = configured_claude_home(claude);
    let codex = codex.display().to_string();
    config.settings.providers.codex.enabled = true;
    config.settings.providers.codex.binary_path = "unused-codex-fixture".to_string();
    config.settings.providers.codex.home_path = codex.clone();
    config.settings.provider_instances["codex"]["enabled"] = json!(true);
    config.settings.provider_instances["codex"]["config"]["binaryPath"] =
        json!("unused-codex-fixture");
    config.settings.provider_instances["codex"]["config"]["homePath"] = json!(codex);
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
            "request_id": "req_usage_fixture",
            "costUSD": 1.25
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

fn write_one_codex_rollout(home: &std::path::Path) {
    let sessions = home.join("sessions").join("2026").join("08").join("09");
    fs::create_dir_all(&sessions).expect("the Codex session directory is created");
    let rows = [
        json!({"timestamp":"2026-08-09T11:00:00Z","type":"session_meta","payload":{"id":"codex-session-1"}}),
        json!({"timestamp":"2026-08-09T11:00:01Z","type":"turn_context","payload":{"model":"gpt-usage-fixture"}}),
        json!({"timestamp":"2026-08-09T11:00:02Z","type":"response_item","payload":{"text":PRIVATE_REPLY}}),
        json!({"timestamp":"2026-08-09T11:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":23,"cached_input_tokens":7,"output_tokens":11,"reasoning_output_tokens":5}}}}),
        json!({"timestamp":"2026-08-09T11:00:04Z","type":"turn_context","payload":{"model":"unknown-usage-fixture"}}),
        json!({"timestamp":"2026-08-09T11:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":3,"cached_input_tokens":0,"output_tokens":2,"reasoning_output_tokens":1}}}}),
    ];
    fs::write(
        sessions.join("rollout-usage.jsonl"),
        rows.into_iter()
            .map(|row| row.to_string())
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("the Codex rollout is written");
}

#[tokio::test]
async fn claude_and_codex_history_join_one_aggregate_without_content() {
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    let codex_home = tempfile::tempdir().expect("a temporary Codex home");
    write_one_claude_record(claude_home.path());
    write_one_codex_rollout(codex_home.path());
    let server = TestServer::start_with(configured_provider_homes(
        claude_home.path(),
        codex_home.path(),
    ))
    .await;
    let mut client = server.connect().await;

    let summary = client
        .call("server.getUsageSummary", input())
        .await
        .expect_success();
    assert_eq!(summary["sources"].as_array().map(Vec::len), Some(2));
    let codex = summary["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|bucket| bucket["provider"] == "codex")
        .expect("a Codex bucket");
    assert_eq!(codex["model"], "gpt-usage-fixture");
    assert_eq!(
        codex["totals"],
        json!({
            "uncachedInputTokens":16,"cachedInputTokens":7,"cacheCreationTokens":0,
            "outputTokens":11,"reasoningTokens":5
        })
    );
    let wire = summary.to_string();
    assert!(!wire.contains(PRIVATE_REPLY));

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn two_servers_on_one_machine_fingerprint_the_same_physical_home_identically() {
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    write_one_claude_record(claude_home.path());
    let first = TestServer::start_with(configured_claude_home(claude_home.path())).await;
    let second = TestServer::start_with(configured_claude_home(claude_home.path())).await;
    let mut first_client = first.connect().await;
    let mut second_client = second.connect().await;

    let first_config = first_client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let second_config = second_client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    let first_summary = first_client
        .call("server.getUsageSummary", input())
        .await
        .expect_success();
    let second_summary = second_client
        .call("server.getUsageSummary", input())
        .await
        .expect_success();
    assert_ne!(
        first_config["environment"]["environmentId"],
        second_config["environment"]["environmentId"]
    );
    assert_eq!(
        first_summary["sources"][0]["fingerprint"],
        second_summary["sources"][0]["fingerprint"]
    );

    first_client.close().await;
    second_client.close().await;
    first.stop().await;
    second.stop().await;
}

#[tokio::test]
async fn provider_reported_model_priced_and_unknown_records_share_one_summary() {
    std::env::set_var("LAPLUS_USAGE_PRICING_URL", "disabled");
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    let codex_home = tempfile::tempdir().expect("a temporary Codex home");
    let pricing_home = tempfile::tempdir().expect("temporary server preferences");
    write_one_claude_record(claude_home.path());
    write_one_codex_rollout(codex_home.path());
    let fetched_at = chrono::Utc::now().timestamp_millis();
    fs::write(
        pricing_home.path().join("usage-model-rates.json"),
        json!({
            "fetchedAtMs": fetched_at,
            "document": {
                "openai/gpt-usage-fixture": {
                    "input_cost_per_token": 0.01,
                    "output_cost_per_token": 0.02,
                    "cache_read_input_token_cost": 0.001
                }
            }
        })
        .to_string(),
    )
    .unwrap();
    let mut config = configured_provider_homes(claude_home.path(), codex_home.path());
    config.preferences = pricing_home.path().to_path_buf();
    let server = TestServer::start_with(config).await;
    let mut client = server.connect().await;

    let summary = client
        .call("server.getUsageSummary", input())
        .await
        .expect_success();
    assert_eq!(summary["pricing"]["status"], "fresh");
    let buckets = summary["buckets"].as_array().unwrap();
    let claude = buckets
        .iter()
        .find(|bucket| bucket["provider"] == "claude")
        .unwrap();
    assert_eq!(claude["costSource"], "providerReported");
    assert_eq!(claude["costUsd"], 1.25);
    let priced = buckets
        .iter()
        .find(|bucket| bucket["model"] == "gpt-usage-fixture")
        .unwrap();
    assert_eq!(priced["costSource"], "modelPriced");
    assert_eq!(priced["unpricedRecords"], 0);
    let unknown = buckets
        .iter()
        .find(|bucket| bucket["model"] == "unknown-usage-fixture")
        .unwrap();
    assert_eq!(unknown["costSource"], "unpriced");
    assert_eq!(unknown["unpricedRecords"], 1);

    client.close().await;
    server.stop().await;
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
            "costUsd": 1.25,
            "cacheSavingsUsd": 0.0,
            "costSource": "providerReported",
            "records": 1,
            "unpricedRecords": 0,
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

#[tokio::test]
async fn an_unknown_caller_zone_degrades_to_utc_without_losing_usage() {
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    write_one_claude_record(claude_home.path());
    let server = TestServer::start_with(configured_claude_home(claude_home.path())).await;
    let mut client = server.connect().await;

    let summary = client
        .call(
            "server.getUsageSummary",
            json!({
                "sinceDay": "2026-08-09",
                "untilDay": "2026-08-09",
                "timeZone": "Not/A_Real_Zone",
            }),
        )
        .await
        .expect_success();

    assert_eq!(summary["timeZone"], "Not/A_Real_Zone");
    assert_eq!(summary["buckets"].as_array().map(Vec::len), Some(1));

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_warm_cache_rebuckets_records_when_the_caller_zone_changes() {
    let claude_home = tempfile::tempdir().expect("a temporary Claude home");
    write_one_claude_record(claude_home.path());
    let transcript = claude_home
        .path()
        .join("projects/-tmp-usage-fixture/usage-session-1.jsonl");
    let text = fs::read_to_string(&transcript)
        .unwrap()
        .replace("2026-08-09T12:00:01.000Z", "2026-08-09T23:30:01.000Z");
    fs::write(&transcript, text).unwrap();
    let server = TestServer::start_with(configured_claude_home(claude_home.path())).await;
    let mut client = server.connect().await;

    client
        .call("server.getUsageSummary", input())
        .await
        .expect_success();
    let summary = client
        .call(
            "server.getUsageSummary",
            json!({
                "sinceDay":"2026-08-10", "untilDay":"2026-08-10", "timeZone":"Asia/Tokyo"
            }),
        )
        .await
        .expect_success();
    assert_eq!(summary["buckets"].as_array().map(Vec::len), Some(1));
    assert_eq!(summary["buckets"][0]["day"], "2026-08-10");

    client.close().await;
    server.stop().await;
}
