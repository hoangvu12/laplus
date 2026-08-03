#![cfg(unix)]

mod harness;

use harness::cloudflare::{CountingVerifiedEndpoint, FakeCloudflared, VerifiedEndpoint};
use harness::TestServer;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

async fn wait_for_json(
    server: &TestServer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    // A hang detector, not a budget — see `READ_TIMEOUT` in the harness. Wide
    // enough that a supervisor waiting out an unanswered probe still converges
    // here, and a genuine wedge is still a failure rather than a hung suite.
    for _ in 0..500 {
        let response = server.get("/api/access/cloudflare/connector").await;
        last = response.body.clone();
        if response.status == 200 && predicate(&response.body) {
            return response.body;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("connector state did not converge: {last}")
}

#[tokio::test]
async fn managed_connector_uses_private_files_becomes_ready_and_survives_restart() {
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    let server = TestServer::start_configured_in_with_endpoint_verifier(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;

    let configured = server
        .post_json(
            "/api/access/cloudflare/connector/configure",
            &json!({
                "hostname": "laplus.example.com",
                "executablePath": fake.executable,
                "connectorToken": "connector-secret"
            }),
        )
        .await;
    assert_eq!(configured.status, 200, "{}", configured.text);
    assert_eq!(configured.body["ownership"], "laplus");
    assert_eq!(configured.body["desiredState"], "running");
    assert!(configured.text.find("connector-secret").is_none());

    let ready = wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(ready["readiness"], true);
    let verified = wait_for_json(&server, |body| body["verificationState"] == "verified").await;
    assert_eq!(verified["connectorState"], "ready");
    let invocation = std::fs::read_to_string(&fake.trace).unwrap();
    assert!(invocation.contains("--config"));
    assert!(invocation.contains("--token-file"));
    assert!(invocation.contains("--metrics"));
    assert!(!invocation.contains("connector-secret"));
    let token_file = directory.path().join("cloudflare").join("connector.token");
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap(),
        "connector-secret"
    );
    assert_eq!(
        std::fs::metadata(&token_file).unwrap().permissions().mode() & 0o077,
        0
    );
    let updated = server
        .post_json(
            "/api/access/cloudflare/connector/configure",
            &json!({
                "hostname": "laplus.example.com",
                "executablePath": fake.executable,
                "connectorToken": ""
            }),
        )
        .await;
    assert_eq!(updated.status, 200, "{}", updated.text);
    assert_eq!(
        std::fs::read_to_string(&token_file).unwrap(),
        "connector-secret"
    );
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    server.stop().await;

    let restarted = TestServer::start_configured_in_with_endpoint_verifier(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    let restored = wait_for_json(&restarted, |body| body["connectorState"] == "ready").await;
    assert_eq!(restored["httpsOrigin"], "https://laplus.example.com");
    assert!(restored.to_string().find("connector-secret").is_none());
    // **Each field checkbox 3 names, read back by name.** A restored connector
    // that merely reaches `ready` proves these survived only by implication —
    // and an implication is what the previous `ownership` was, written every
    // boot and read back by nothing.
    assert_eq!(restored["executablePath"], fake.executable.to_string_lossy().as_ref());
    assert_eq!(restored["desiredState"], "running");
    assert_eq!(restored["tunnelOwnership"], "external");
    assert_eq!(
        restored["credentialPath"],
        token_file.to_string_lossy().as_ref(),
        "the secret is referenced by path and never by value"
    );
    // The loopback origin is persisted and then re-pointed at whatever port this
    // boot actually bound — the one field that must *not* survive verbatim,
    // because a connector restored onto last run's port would forward the public
    // hostname to nothing. What survives is the promise, not the number.
    let loopback = restored["loopbackOrigin"].as_str().expect("a loopback origin");
    assert_eq!(
        loopback,
        format!("http://127.0.0.1:{}", restarted.addr().port()),
        "it names this server, not the last one"
    );
    wait_for_json(&restarted, |body| body["verificationState"] == "verified").await;
    restarted.stop().await;
}

#[tokio::test]
async fn exhausted_connector_requires_retry_and_reconciles_repeated_start_stop_commands() {
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    fake.rehearse("crash");
    // A verifier that counts, because checkbox 8's "a later start … re-verifies
    // the endpoint" is not visible in the endpoint row: a stopped connector's
    // row still reads `verified`, since verification is a fact about the last
    // attempt and a stop is not an attempt. The count is the only place the
    // second check exists.
    let verifier = std::sync::Arc::new(CountingVerifiedEndpoint::default());
    let server = TestServer::start_configured_in_with_endpoint_verifier(
        directory.path(),
        verifier.clone(),
    )
    .await;
    let configured = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":fake.executable,"connectorToken":"connector-secret"}),
    ).await;
    assert_eq!(configured.status, 200, "{}", configured.text);
    let exhausted = wait_for_json(&server, |body| {
        body["connectorState"] == "restart-exhausted"
    })
    .await;
    assert_eq!(exhausted["restartCount"], 3);
    assert!(exhausted.to_string().contains("[REDACTED]"));
    assert!(!exhausted.to_string().contains("connector-secret"));
    assert_eq!(
        std::fs::read_to_string(&fake.trace)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with('['))
            .count(),
        3
    );

    // The shape changed and the status did not: this route has answered `400`
    // since ticket 01, and what was missing was a body a client could decode.
    // The reason is what the wizard branches on; the sentence is what it shows.
    let refused = server
        .post_json("/api/access/cloudflare/connector/start", &json!({}))
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert_eq!(refused.body["_tag"], "EnvironmentPublicExposureRejectedError");
    assert_eq!(refused.body["reason"], "restarts-exhausted");
    assert!(refused.body["message"].as_str().unwrap().contains("use Retry"));
    // Nothing was mutated, so nothing is claimed as done or outstanding.
    assert_eq!(refused.body["completed"], json!([]));
    assert_eq!(refused.body["remaining"], json!([]));

    fake.behave();
    let retried = server
        .post_json("/api/access/cloudflare/connector/retry", &json!({}))
        .await;
    assert_eq!(retried.status, 200, "{}", retried.text);
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    let launches = std::fs::read_to_string(&fake.trace)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with('['))
        .count();

    for _ in 0..2 {
        let started = server
            .post_json("/api/access/cloudflare/connector/start", &json!({}))
            .await;
        assert_eq!(started.status, 200);
    }
    tokio::task::yield_now().await;
    assert_eq!(
        std::fs::read_to_string(&fake.trace)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with('['))
            .count(),
        launches
    );

    assert_eq!(
        server
            .post_json("/api/access/cloudflare/connector/stop", &json!({}))
            .await
            .status,
        200
    );
    assert_eq!(
        server
            .post_json("/api/access/cloudflare/connector/start", &json!({}))
            .await
            .status,
        200
    );
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    let launches = launches + 1;
    assert_eq!(
        std::fs::read_to_string(&fake.trace)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with('['))
            .count(),
        launches
    );

    let stopped = server
        .post_json("/api/access/cloudflare/connector/stop", &json!({}))
        .await;
    assert_eq!(stopped.status, 200);
    wait_for_json(&server, |body| body["connectorState"] == "stopped").await;
    assert!(directory.path().join("cloudflare/connector.token").exists());
    // The row still says what the last attempt found, which is exactly why the
    // count below is the assertion and this is not.
    let after_stop = verifier.count();
    let started = server
        .post_json("/api/access/cloudflare/connector/start", &json!({}))
        .await;
    assert_eq!(started.status, 200);
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(
        std::fs::read_to_string(&fake.trace)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with('['))
            .count(),
        launches + 1
    );
    for _ in 0..500 {
        if verifier.count() > after_stop {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        verifier.count() > after_stop,
        "starting re-verified the endpoint: {after_stop} then {}",
        verifier.count()
    );
    server.stop().await;
}

/// A connector that accepts a readiness probe and never answers it must still
/// be stoppable.
///
/// The probe is the supervision loop's body, so an unbounded one is a connector
/// that cannot be turned off: the stop is recorded, the desired state changes,
/// and nothing acts on it because the task is still inside the request. This
/// went unnoticed until the whole suite ran at once and the wedge became likely
/// enough to happen. The budget is a hang detector — the assertion is that the
/// connector stops, never how long it took.
#[tokio::test]
async fn a_connector_that_never_answers_a_probe_can_still_be_stopped() {
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    fake.rehearse("hang");
    let server = TestServer::start_configured_in(directory.path()).await;
    let configured = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":fake.executable,"connectorToken":"connector-secret"}),
    ).await;
    assert_eq!(configured.status, 200, "{}", configured.text);
    let starting = wait_for_json(&server, |body| body["connectorState"] == "starting").await;
    assert_eq!(starting["readiness"], false);

    let stopped = server
        .post_json("/api/access/cloudflare/connector/stop", &json!({}))
        .await;
    assert_eq!(stopped.status, 200, "{}", stopped.text);
    let settled = wait_for_json(&server, |body| body["connectorState"] == "stopped").await;
    assert_eq!(settled["readiness"], serde_json::Value::Null);
    assert_eq!(settled["desiredState"], "stopped");
    server.stop().await;
}

#[tokio::test]
async fn incompatible_selected_executable_is_actionable_without_echoing_the_token() {
    let directory = tempfile::tempdir().unwrap();
    let outdated = directory.path().join("old-cloudflared");
    std::fs::write(&outdated, "#!/bin/sh\necho 'cloudflared version 2023.10.0'\n").unwrap();
    let mut permissions = std::fs::metadata(&outdated).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&outdated, permissions).unwrap();
    let server = TestServer::start_configured_in(directory.path()).await;
    let response = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":outdated,"connectorToken":"never-echo-this"}),
    ).await;
    assert_eq!(response.status, 400);
    assert!(response.text.contains("incompatible"));
    assert!(!response.text.contains("never-echo-this"));
    assert!(!directory.path().join("cloudflare/connector.token").exists());
    server.stop().await;
}

#[tokio::test]
async fn an_external_endpoint_never_acquires_a_managed_connector_lifecycle() {
    let server = TestServer::start().await;
    let registered = server
        .post_json(
            "/api/access/cloudflare",
            &json!({"hostname":"external.example.com"}),
        )
        .await;
    assert_eq!(registered.status, 200);
    let connector = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(connector.status, 200);
    assert_eq!(connector.body["configured"], false);
    assert_eq!(connector.body["connectorState"], "unconfigured");
    server.stop().await;
}

/// The same rule read from the other end, and the one the UI cannot enforce.
///
/// Hiding the register control for a configured connector keeps a *person* from
/// claiming laplus's own hostname as somebody else's. This is the route saying
/// no, which is what stops a stale tab, a scripted client or a second window
/// from overwriting the endpoint record the connector restores itself from.
/// ADR-0045: every lifecycle action has one owner.
#[tokio::test]
async fn a_supervised_connector_refuses_to_have_its_exposure_claimed_as_external() {
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    let server = TestServer::start_configured_in(directory.path()).await;
    let configured = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":fake.executable,"connectorToken":"connector-secret"}),
    ).await;
    assert_eq!(configured.status, 200, "{}", configured.text);

    let claimed = server
        .post_json(
            "/api/access/cloudflare",
            &json!({"hostname":"laplus.example.com"}),
        )
        .await;
    assert_eq!(claimed.status, 409, "{}", claimed.text);
    assert!(claimed.text.contains("already runs a connector"));

    let selected = server
        .post_json(
            "/api/access/cloudflare/account/select",
            &json!({"tunnelId":"11111111-1111-1111-1111-111111111111",
                    "hostname":"laplus.example.com"}),
        )
        .await;
    assert_eq!(selected.status, 409, "{}", selected.text);

    // The connector still owns what it owned, and still says so.
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["httpsOrigin"], "https://laplus.example.com");
    let connector = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(connector.body["configured"], true);
    assert_eq!(connector.body["httpsOrigin"], "https://laplus.example.com");
    server.stop().await;
}

#[tokio::test]
async fn a_ready_replacement_is_adopted_without_launching_a_duplicate_and_stops_with_its_owner() {
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    fake.rehearse("replace");
    let server = TestServer::start_configured_in(directory.path()).await;
    let configured = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":fake.executable,"connectorToken":"connector-secret"}),
    ).await;
    assert_eq!(configured.status, 200, "{}", configured.text);
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(
        std::fs::read_to_string(&fake.trace)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with('['))
            .count(),
        1
    );
    let stopped = server
        .post_json("/api/access/cloudflare/connector/stop", &json!({}))
        .await;
    assert_eq!(stopped.status, 200);
    wait_for_json(&server, |body| body["connectorState"] == "stopped").await;
    assert!(std::fs::read_to_string(&fake.trace).unwrap().contains("stopped"));
    server.stop().await;
}

/// The connector-token path is the one that reaches `configure`, and its tunnel
/// is Cloudflare's: laplus receives run authority and no more. It was recorded
/// as `"adopted"` — written unconditionally by `configure` and never read back —
/// while the snapshot printed `"ownership":"laplus"` and
/// `"remoteOwnership":"cloudflare"` as string literals.
///
/// Persistence across a restart is `http_public_exposure.rs`, which is where
/// the harness gives a server a database on disk.
#[tokio::test]
async fn a_connector_token_tunnel_is_owned_by_cloudflare_and_run_by_laplus() {
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    let server = TestServer::start_configured_in_with_endpoint_verifier(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    let configured = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":fake.executable,"connectorToken":"connector-secret"}),
    ).await;
    assert_eq!(configured.status, 200, "{}", configured.text);

    // Two answers, not one: who runs the process, and who owns the tunnel.
    assert_eq!(configured.body["ownership"], "laplus");
    assert_eq!(configured.body["tunnelOwnership"], "external");
    assert!(configured.body.get("remoteOwnership").is_none());

    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "external");
    // The endpoint row knows laplus runs the connector in front of it, rather
    // than reporting `external` here while laplus supervised it.
    assert_eq!(endpoint.body["health"]["connector"], "laplus");
    server.stop().await;
}
