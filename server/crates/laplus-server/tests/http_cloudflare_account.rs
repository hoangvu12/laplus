//! Cloudflare sign-in and existing-tunnel discovery, through the real routes.
//!
//! The `cloudflared` here is a stand-in that prints what the real one prints and
//! writes what the real one writes. What is under test is everything around it:
//! that authorization is tracked and resumable, that a certificate laplus did
//! not create grants nothing until the developer says so, that a listing is read
//! for what it proves, and that an active tunnel stays somebody else's.
#![cfg(unix)]

mod harness;

use harness::cloudflare::{client_with, FakeCloudflared, CERTIFICATE};
use harness::TestServer;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::sync::Mutex;

/// `TUNNEL_ORIGIN_CERT` is process-wide — cloudflared's own variable, and the
/// only honest way to point certificate discovery somewhere throwaway. One
/// binary, one lock, one certificate per test.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serially() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn wait_for_authorization_url(server: &TestServer) -> String {
    for _ in 0..300 {
        let response = server.get("/api/access/cloudflare/account").await;
        if let Some(url) = response.body["authorizationUrl"].as_str() {
            return url.to_string();
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("cloudflared never announced an authorization URL")
}

async fn wait_for_login(server: &TestServer, state: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..300 {
        let response = server.get("/api/access/cloudflare/account").await;
        last = response.body.clone();
        if response.body["loginState"] == state {
            return response.body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("sign-in never reached {state}: {last}")
}

#[tokio::test]
async fn browser_authorization_is_tracked_consented_and_survives_a_restart() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    std::fs::write(&fake.mode, "await").unwrap();
    let server = TestServer::start_configured_in(directory.path()).await;

    let before = server.get("/api/access/cloudflare/account").await;
    assert_eq!(before.status, 200, "{}", before.text);
    assert_eq!(before.body["certificateDetected"], false);
    assert_eq!(before.body["loginState"], "not-started");
    assert_eq!(before.body["step"], "sign-in");
    assert!(before.body["certificateWarning"]
        .as_str()
        .unwrap()
        .contains("every tunnel in your account"));

    let started = server
        .post_json(
            "/api/access/cloudflare/account/login",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(started.status, 200, "{}", started.text);
    let awaiting = wait_for_login(&server, "awaiting-browser").await;
    assert_eq!(awaiting["loginState"], "awaiting-browser");
    assert_eq!(
        wait_for_authorization_url(&server).await,
        "https://dash.cloudflare.com/argotunnel?callback=test-callback"
    );

    // Asking again while a browser is open must not open a second one.
    let repeated = server
        .post_json(
            "/api/access/cloudflare/account/login",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(repeated.status, 200, "{}", repeated.text);
    assert_eq!(fake.invocations("login"), 1);

    std::fs::write(&fake.signal, "authorized").unwrap();
    let complete = wait_for_login(&server, "complete").await;
    assert_eq!(complete["certificateDetected"], true);
    assert!(complete["certificateConsentedAt"].is_string());
    assert_eq!(complete["step"], "choose-tunnel");
    assert!(!complete.to_string().contains(CERTIFICATE));
    server.stop().await;

    let restarted = TestServer::start_configured_in(directory.path()).await;
    let resumed = restarted.get("/api/access/cloudflare/account").await;
    assert_eq!(resumed.body["loginState"], "complete");
    assert!(resumed.body["certificateConsentedAt"].is_string());
    assert_eq!(resumed.body["step"], "choose-tunnel");
    restarted.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

#[tokio::test]
async fn a_detected_certificate_grants_nothing_until_consent_and_is_never_touched() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let before = std::fs::metadata(&fake.certificate).unwrap();
    let server = TestServer::start_configured_in(directory.path()).await;

    let detected = server.get("/api/access/cloudflare/account").await;
    assert_eq!(detected.body["certificateDetected"], true);
    assert_eq!(detected.body["certificateConsentedAt"], Value::Null);
    assert_eq!(detected.body["step"], "consent");
    assert!(!detected.text.contains(CERTIFICATE));

    let refused = server
        .post_json(
            "/api/access/cloudflare/account/tunnels",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(refused.status, 409, "{}", refused.text);
    // A tagged refusal, not a bare `{ message }`. The reason is a closed word
    // the wizard branches on; the sentence is what it puts on screen. Both used
    // to be thrown away at the boundary — Gap 4 in the parity ledger.
    assert_eq!(refused.body["_tag"], "EnvironmentPublicExposurePreconditionError");
    assert_eq!(refused.body["code"], "public_exposure_refused");
    assert_eq!(refused.body["reason"], "consent-required");
    assert!(refused.body["message"].as_str().unwrap().contains("Confirm"));
    assert!(refused.body["traceId"].is_string());
    // Nothing was mutated, so the refusal claims nothing about work done.
    assert_eq!(refused.body["completed"], json!([]));
    assert_eq!(refused.body["remaining"], json!([]));
    // And still no secret, in the new shape as in the old one.
    assert!(!refused.text.contains(CERTIFICATE));
    assert_eq!(fake.invocations("list"), 0);

    let consented = server
        .post_json(
            "/api/access/cloudflare/account/consent",
            &json!({"consented": true}),
        )
        .await;
    assert_eq!(consented.status, 200, "{}", consented.text);
    assert!(consented.body["certificateConsentedAt"].is_string());

    let listed = server
        .post_json(
            "/api/access/cloudflare/account/tunnels",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(listed.status, 200, "{}", listed.text);
    assert_eq!(listed.body["tunnels"].as_array().unwrap().len(), 2);
    assert!(!listed.text.contains(CERTIFICATE));

    // Withdrawing consent takes everything the certificate produced with it.
    // A selection kept here would let a later re-consent resume at a tunnel
    // nothing had re-listed, under an authority that had been withdrawn.
    let selected = server
        .post_json(
            "/api/access/cloudflare/account/select",
            &json!({"tunnelId": "22222222-2222-2222-2222-222222222222",
                    "hostname": "spare.example.com"}),
        )
        .await;
    assert_eq!(selected.status, 200, "{}", selected.text);
    assert_eq!(selected.body["step"], "confirm-adoption");
    let withdrawn = server
        .post_json(
            "/api/access/cloudflare/account/consent",
            &json!({"consented": false}),
        )
        .await;
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.text);
    assert_eq!(withdrawn.body["certificateConsentedAt"], Value::Null);
    assert_eq!(withdrawn.body["selection"], Value::Null);
    assert_eq!(withdrawn.body["tunnels"].as_array().unwrap().len(), 0);
    assert_eq!(withdrawn.body["step"], "consent");

    // Used where it is: same file, same bytes, still there.
    let after = std::fs::metadata(&fake.certificate).unwrap();
    assert_eq!(
        std::fs::read_to_string(&fake.certificate).unwrap(),
        CERTIFICATE
    );
    assert_eq!(before.len(), after.len());
    assert!(!directory.path().join("cloudflare").join("cert.pem").exists());
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

#[tokio::test]
async fn discovery_branches_on_activity_without_inventing_a_hostname() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server = TestServer::start_configured_in(directory.path()).await;
    assert_eq!(
        server
            .post_json(
                "/api/access/cloudflare/account/consent",
                &json!({"consented": true})
            )
            .await
            .status,
        200
    );

    let listed = server
        .post_json(
            "/api/access/cloudflare/account/tunnels",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(listed.status, 200, "{}", listed.text);
    let tunnels = listed.body["tunnels"].as_array().unwrap();
    assert_eq!(tunnels.len(), 2, "a deleted tunnel is not a choice");
    assert_eq!(tunnels[0]["id"], "11111111-1111-1111-1111-111111111111");
    assert_eq!(tunnels[0]["name"], "already-running");
    assert_eq!(tunnels[0]["createdAt"], "2026-01-01T00:00:00Z");
    assert_eq!(tunnels[0]["connectionCount"], 2);
    assert_eq!(tunnels[0]["activity"], "active");
    assert_eq!(tunnels[0]["classification"], "external");
    assert_eq!(tunnels[1]["activity"], "inactive");
    assert_eq!(tunnels[1]["classification"], "adoptable");
    assert!(tunnels[0].get("hostname").is_none());
    assert!(tunnels[0].get("managementMode").is_none());
    assert!(listed.body["listedAt"].is_string());

    // An inactive tunnel is a candidate and nothing more until adoption is
    // separately confirmed: no endpoint, no connector, no Cloudflare change.
    let chosen = server
        .post_json(
            "/api/access/cloudflare/account/select",
            &json!({
                "tunnelId": "22222222-2222-2222-2222-222222222222",
                "hostname": "spare.example.com"
            }),
        )
        .await;
    assert_eq!(chosen.status, 200, "{}", chosen.text);
    assert_eq!(chosen.body["selection"]["classification"], "adoptable");
    assert_eq!(chosen.body["selection"]["adoptionConfirmed"], false);
    assert_eq!(chosen.body["step"], "confirm-adoption");
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["configured"], false);
    let connector = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(connector.body["connectorState"], "unconfigured");

    // An active one is external: verified and advertised, never operated.
    let external = server
        .post_json(
            "/api/access/cloudflare/account/select",
            &json!({
                "tunnelId": "11111111-1111-1111-1111-111111111111",
                "hostname": " Already-Running.EXAMPLE.com. "
            }),
        )
        .await;
    assert_eq!(external.status, 200, "{}", external.text);
    assert_eq!(external.body["selection"]["classification"], "external");
    assert_eq!(
        external.body["selection"]["httpsOrigin"],
        "https://already-running.example.com"
    );
    assert_eq!(external.body["step"], "verify-hostname");
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["httpsOrigin"], "https://already-running.example.com");
    assert_eq!(endpoint.body["ownership"], "external");
    let connector = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(connector.body["connectorState"], "unconfigured");

    // Listing again reconciles rather than repeating anything at Cloudflare.
    let again = server
        .post_json(
            "/api/access/cloudflare/account/tunnels",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(again.status, 200, "{}", again.text);
    assert_eq!(again.body["selection"]["tunnelId"], "11111111-1111-1111-1111-111111111111");
    assert_eq!(fake.invocations("list"), 2);
    assert_eq!(fake.invocations("create"), 0);
    assert_eq!(fake.invocations("route"), 0);

    let unknown = server
        .post_json(
            "/api/access/cloudflare/account/select",
            &json!({"tunnelId": "44444444-4444-4444-4444-444444444444", "hostname": "x.example.com"}),
        )
        .await;
    assert_eq!(unknown.status, 409, "{}", unknown.text);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

#[tokio::test]
async fn a_cancelled_or_failed_authorization_leaves_setup_resumable() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    std::fs::write(&fake.mode, "await").unwrap();
    let server = TestServer::start_configured_in(directory.path()).await;

    assert_eq!(
        server
            .post_json(
                "/api/access/cloudflare/account/login",
                &json!({"executablePath": fake.executable})
            )
            .await
            .status,
        200
    );
    // Cancel once the browser step is genuinely under way. Cancelling in the
    // gap before cloudflared has started would test the harness's timing rather
    // than the wizard's behaviour.
    wait_for_login(&server, "awaiting-browser").await;
    wait_for_authorization_url(&server).await;
    let cancelled = server
        .post_json("/api/access/cloudflare/account/login/cancel", &json!({}))
        .await;
    assert_eq!(cancelled.status, 200, "{}", cancelled.text);
    let stopped = wait_for_login(&server, "cancelled").await;
    assert_eq!(stopped["certificateDetected"], false);
    assert_eq!(stopped["step"], "sign-in");
    assert!(stopped["failureMessage"].is_string());

    std::fs::write(&fake.mode, "fail").unwrap();
    assert_eq!(
        server
            .post_json(
                "/api/access/cloudflare/account/login",
                &json!({"executablePath": fake.executable})
            )
            .await
            .status,
        200
    );
    let failed = wait_for_login(&server, "failed").await;
    assert!(failed["failureMessage"].is_string());
    assert_eq!(failed["step"], "sign-in");

    std::fs::write(&fake.mode, "ok").unwrap();
    assert_eq!(
        server
            .post_json(
                "/api/access/cloudflare/account/login",
                &json!({"executablePath": fake.executable})
            )
            .await
            .status,
        200
    );
    let completed = wait_for_login(&server, "complete").await;
    assert_eq!(completed["certificateDetected"], true);
    assert_eq!(fake.invocations("login"), 3);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

#[tokio::test]
async fn account_state_requires_read_and_every_account_action_requires_write() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server = TestServer::start_configured_in(directory.path()).await;

    let ordinary = client_with(&server, &["orchestration:read"]).await;
    let hidden = server
        .get_as("/api/access/cloudflare/account", &ordinary)
        .await;
    assert_eq!(hidden.status, 403);
    assert_eq!(hidden.body["requiredScope"], "access:read");
    assert!(hidden.body.get("certificateDetected").is_none());
    assert!(hidden.body.get("tunnels").is_none());

    let reader = client_with(&server, &["access:read"]).await;
    let readable = server
        .get_as("/api/access/cloudflare/account", &reader)
        .await;
    assert_eq!(readable.status, 200, "{}", readable.text);
    assert_eq!(readable.body["certificateDetected"], true);

    for (path, body) in [
        (
            "/api/access/cloudflare/account/login",
            json!({"executablePath": fake.executable}),
        ),
        ("/api/access/cloudflare/account/login/cancel", json!({})),
        (
            "/api/access/cloudflare/account/consent",
            json!({"consented": true}),
        ),
        (
            "/api/access/cloudflare/account/tunnels",
            json!({"executablePath": fake.executable}),
        ),
        (
            "/api/access/cloudflare/account/select",
            json!({"tunnelId": "11111111-1111-1111-1111-111111111111", "hostname": "a.example.com"}),
        ),
    ] {
        let refused = server.post_json_as(path, &reader, &body).await;
        assert_eq!(refused.status, 403, "{path}");
        assert_eq!(refused.body["requiredScope"], "access:write");
        assert!(refused.body.get("certificateDetected").is_none());
        assert!(refused.body.get("tunnels").is_none());
        // ADR-0047: a refusal discloses nothing. The scope answer is not the
        // public-exposure refusal, precisely so that a client without the scope
        // cannot read a `reason` off it — every one of those words would say
        // something about Cloudflare state, and a session that may not read the
        // snapshot may not learn it a sentence at a time either.
        assert_eq!(refused.body["_tag"], "EnvironmentScopeRequiredError", "{path}");
        assert!(refused.body.get("reason").is_none(), "{path}");
        assert!(refused.body.get("message").is_none(), "{path}");
        assert!(refused.body.get("completed").is_none(), "{path}");
    }
    assert_eq!(fake.invocations("login"), 0);
    assert_eq!(fake.invocations("list"), 0);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// The commands tickets 06 and 07 will drive, pinned before they drive them.
///
/// **Nothing in the server invokes these yet**, and the test above asserts it
/// stays that way for ticket 04's sake. What this pins is the *fixture*: the
/// argument shapes `cloudflared` accepts, that `--credentials-file` is what
/// keeps the narrow run credential inside laplus's private directory, that
/// `create` reports an allocated id different from the name it was asked for,
/// and that a partial creation is rehearsable. A fixture nothing exercises rots
/// silently, and this one is the thing those tickets cannot start without.
#[tokio::test]
async fn the_fixture_answers_every_command_a_dedicated_tunnel_needs() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let credentials = directory.path().join("private").join("tunnel.json");
    std::fs::create_dir_all(credentials.parent().unwrap()).unwrap();

    let run = |arguments: Vec<String>| {
        let executable = fake.executable.clone();
        let certificate = fake.certificate.clone();
        async move {
            tokio::process::Command::new(&executable)
                .args(&arguments)
                .env("TUNNEL_ORIGIN_CERT", &certificate)
                .output()
                .await
                .expect("the fake runs")
        }
    };
    let origincert = fake.certificate.to_string_lossy().to_string();
    // `--config`, for the same reason `--origincert` is here: the shape a real
    // account command takes is what this pins, and `Account::consented_command`
    // passes one on every single call. In laplus's own directory rather than
    // cloudflared's — which is what the flag is *for*, and what the fixture
    // asserts by refusing a config that sits beside the certificate.
    let config = directory.path().join("private").join("account.yml");
    let config = config.to_string_lossy().to_string();
    let argv = |rest: &[&str]| {
        let mut arguments = vec![
            "tunnel".to_string(),
            "--config".into(),
            config.clone(),
            "--origincert".into(),
            origincert.clone(),
        ];
        arguments.extend(rest.iter().map(|word| word.to_string()));
        arguments
    };

    let created = run(argv(&[
        "create",
        "--credentials-file",
        &credentials.to_string_lossy(),
        "--output",
        "json",
        "laplus",
    ]))
    .await;
    assert!(created.status.success(), "{}", String::from_utf8_lossy(&created.stderr));
    let reported: Value = serde_json::from_slice(&created.stdout).expect("structured output");
    // The allocation is the tunnel's identity, and it is not the name that was
    // asked for — cleanup has to target the first and a confirmation shows both.
    assert_eq!(reported["id"], harness::cloudflare::CREATED_TUNNEL_ID);
    assert_eq!(reported["name"], "laplus");
    // The narrow run credential lands where laplus said, not in cloudflared's
    // own default location, and is private.
    assert!(credentials.is_file());
    assert_eq!(
        std::fs::metadata(&credentials).unwrap().permissions().mode() & 0o077,
        0
    );
    let held: Value = serde_json::from_slice(&std::fs::read(&credentials).unwrap()).unwrap();
    assert_eq!(held["TunnelID"], harness::cloudflare::CREATED_TUNNEL_ID);
    assert_eq!(fake.credential_written_to().as_deref(), Some(credentials.as_path()));

    let routed = run(argv(&["route", "dns", harness::cloudflare::CREATED_TUNNEL_ID, "laplus.example.com"])).await;
    assert!(routed.status.success(), "{}", String::from_utf8_lossy(&routed.stderr));

    let deleted = run(argv(&["delete", harness::cloudflare::CREATED_TUNNEL_ID])).await;
    assert!(deleted.status.success(), "{}", String::from_utf8_lossy(&deleted.stderr));

    // Ticket 06's partial creation: the tunnel exists and its route does not.
    fake.rehearse("route-fails");
    let failed = run(argv(&["route", "dns", harness::cloudflare::CREATED_TUNNEL_ID, "laplus.example.com"])).await;
    assert!(!failed.status.success());
    // Ticket 07's partial remote cleanup: the DNS record outlives the attempt.
    fake.rehearse("delete-fails");
    let refused = run(argv(&["delete", harness::cloudflare::CREATED_TUNNEL_ID])).await;
    assert!(!refused.status.success());
    fake.behave();

    assert_eq!(fake.invocations("create"), 1);
    assert_eq!(fake.invocations("route"), 2);
    assert_eq!(fake.invocations("delete"), 2);
    // No secret in the trace: the credential is a path there, never contents.
    let trace = std::fs::read_to_string(&fake.trace).unwrap();
    assert!(!trace.contains("FAKE-TUNNEL-CREDENTIAL-SECRET"));
    assert!(!trace.contains(CERTIFICATE));
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// `cloudflared` has no `route dns delete`.
///
/// Ticket 07's "Delete everywhere" therefore cannot be `cloudflared tunnel
/// delete` alone: removing the recorded DNS record is a Cloudflare API call
/// needing DNS authority of its own — the cleanup asymmetry in
/// `.scratch/cloudflare-tunnel/research.md`. Modelling it as a CLI verb would
/// let that ticket be built against a command that does not exist, so the fake
/// is a real local HTTP server, in the way `FakeRelease` models the download
/// feed.
#[tokio::test]
async fn deleting_a_dns_record_is_an_api_call_the_cli_cannot_make() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    let api = harness::cloudflare::FakeCloudflareApi::start("laplus.example.com").await;
    let record = format!(
        "/client/v4/zones/{}/dns_records/{}",
        harness::cloudflare::ZONE_ID,
        harness::cloudflare::RECORD_ID
    );

    // There is no such verb, and the fixture must not invent one. Invoked in
    // exactly the shape a real `route dns` takes — `--origincert`, `--config`
    // and all — so that what is refused is the *verb* and not a malformed
    // argument list; the first version of this test passed because it omitted
    // `--origincert`, and would have gone on passing if the fixture had grown a
    // `delete` branch. `--config` joined that shape when
    // `Account::consented_command` started passing one on every call, and this
    // test kept the old argv — which the fixture then refused, on Linux only,
    // because the whole file is `cfg(unix)`.
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    let origincert = fake.certificate.to_string_lossy().to_string();
    let config = directory.path().join("private").join("account.yml");
    let config = config.to_string_lossy().to_string();
    let created = tokio::process::Command::new(&fake.executable)
        .args([
            "tunnel", "--config", &config, "--origincert", &origincert, "route", "dns", "t",
            "laplus.example.com",
        ])
        .env("TUNNEL_ORIGIN_CERT", &fake.certificate)
        .output()
        .await
        .expect("the fake runs");
    assert!(
        created.status.success(),
        "`route dns` must work, or the refusal below proves nothing: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let attempted = tokio::process::Command::new(&fake.executable)
        .args([
            "tunnel", "--config", &config, "--origincert", &origincert, "route", "dns", "delete",
            "laplus.example.com",
        ])
        .env("TUNNEL_ORIGIN_CERT", &fake.certificate)
        .output()
        .await
        .expect("the fake runs");
    assert!(
        !attempted.status.success(),
        "the CLI has no `route dns delete`; the fixture must not pretend otherwise"
    );
    assert!(String::from_utf8_lossy(&attempted.stderr).contains("unknown command"));
    std::env::remove_var("TUNNEL_ORIGIN_CERT");

    // The API needs DNS authority of its own, which is the whole reason the
    // deletion cannot ride on the account certificate: an unauthorized caller is
    // answered exactly as Cloudflare answers one.
    let client = reqwest::Client::new();
    let anonymous = client
        .delete(format!("{}{record}", api.origin))
        .send()
        .await
        .expect("the fake API answers");
    assert_eq!(anonymous.status(), 403);
    assert_eq!(api.records().len(), 1, "an unauthorized delete removed a record");

    let deleted = client
        .delete(format!("{}{record}", api.origin))
        .bearer_auth(harness::cloudflare::DNS_API_TOKEN)
        .send()
        .await
        .expect("the fake API answers");
    assert_eq!(deleted.status(), 200);
    assert!(api.records().is_empty());

    // Idempotent retry after a partial cleanup: a record already gone reads as
    // already done rather than as a new failure, which is what lets ticket 07
    // resume from observed state.
    let repeated = client
        .delete(format!("{}{record}", api.origin))
        .bearer_auth(harness::cloudflare::DNS_API_TOKEN)
        .send()
        .await
        .expect("the fake API answers");
    assert_eq!(repeated.status(), 404);
    let body: Value = repeated.json().await.unwrap();
    assert_eq!(body["errors"][0]["code"], 81044);

    // The exact recorded resource was targeted, and no other.
    assert_eq!(
        api.requests(),
        vec![
            ("DELETE".to_string(), record.clone()),
            ("DELETE".to_string(), record.clone()),
            ("DELETE".to_string(), record.clone()),
        ]
    );
    api.stop();
}
