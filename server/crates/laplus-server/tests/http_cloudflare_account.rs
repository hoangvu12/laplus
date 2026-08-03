//! Cloudflare sign-in and existing-tunnel discovery, through the real routes.
//!
//! The `cloudflared` here is a stand-in that prints what the real one prints and
//! writes what the real one writes. What is under test is everything around it:
//! that authorization is tracked and resumable, that a certificate laplus did
//! not create grants nothing until the developer says so, that a listing is read
//! for what it proves, and that an active tunnel stays somebody else's.
#![cfg(unix)]

mod harness;

use harness::{ClientIdentity, TestServer};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

const CERTIFICATE: &str = "FAKE-ACCOUNT-CERTIFICATE-SECRET";
const GRANT_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange";
const BOOTSTRAP_TOKEN_TYPE: &str = "urn%3At3%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap";
const ACCESS_TOKEN_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token";

async fn client_with(server: &TestServer, scopes: &[&str]) -> ClientIdentity {
    let minted = server
        .post_json("/api/auth/pairing-token", &json!({ "scopes": scopes }))
        .await;
    let credential = minted.body["credential"].as_str().unwrap();
    let exchanged = server.post_form(
        "/oauth/token",
        &format!("grant_type={GRANT_TYPE}&subject_token={credential}&subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type={ACCESS_TOKEN_TYPE}"),
    ).await;
    ClientIdentity::anonymous().with_bearer(exchanged.body["access_token"].as_str().unwrap())
}

struct Cloudflared {
    executable: PathBuf,
    trace: PathBuf,
    mode: PathBuf,
    signal: PathBuf,
    tunnels: PathBuf,
    certificate: PathBuf,
}

/// A `cloudflared` that signs in and lists, and nothing else.
///
/// It writes the certificate where `TUNNEL_ORIGIN_CERT` says, which is where
/// the real one looks — so laplus finds it the same way in the suite as on a
/// developer's machine.
fn fake_cloudflared(directory: &Path) -> Cloudflared {
    let fake = Cloudflared {
        executable: directory.join("cloudflared-fake.py"),
        trace: directory.join("cloudflared.trace"),
        mode: directory.join("cloudflared.mode"),
        signal: directory.join("browser.signal"),
        tunnels: directory.join("tunnels.json"),
        certificate: directory.join("cert.pem"),
    };
    let source = format!(
        r#"#!/usr/bin/env python3
import json, os, sys, time
ARGS = sys.argv[1:]
TRACE = {trace:?}
MODE = {mode:?}
SIGNAL = {signal:?}
TUNNELS = {tunnels:?}
if '--version' in ARGS:
    print('cloudflared version 2026.7.3')
    raise SystemExit(0)
with open(TRACE, 'a') as f:
    f.write(json.dumps(ARGS) + '\n')
mode = open(MODE).read().strip() if os.path.exists(MODE) else 'ok'
certificate = os.environ.get('TUNNEL_ORIGIN_CERT', '')
if ARGS[:2] == ['tunnel', 'login']:
    print('Please open the following URL and log in with your Cloudflare account:')
    print('https://dash.cloudflare.com/argotunnel?callback=test-callback')
    sys.stdout.flush()
    if mode == 'fail':
        print('failed to reach the Cloudflare login page', file=sys.stderr)
        raise SystemExit(1)
    if mode == 'await':
        while not os.path.exists(SIGNAL):
            time.sleep(0.02)
    with open(certificate, 'w') as f:
        f.write({content:?})
    raise SystemExit(0)
if 'list' in ARGS:
    assert ARGS[1] == '--origincert' and ARGS[2] == certificate, ARGS
    if not os.path.exists(certificate):
        print('Cannot determine default origin certificate path', file=sys.stderr)
        raise SystemExit(1)
    print(open(TUNNELS).read())
    raise SystemExit(0)
raise SystemExit(2)
"#,
        trace = fake.trace.display().to_string(),
        mode = fake.mode.display().to_string(),
        signal = fake.signal.display().to_string(),
        tunnels = fake.tunnels.display().to_string(),
        content = CERTIFICATE,
    );
    std::fs::write(&fake.executable, source).unwrap();
    std::fs::set_permissions(&fake.executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(
        &fake.tunnels,
        r#"[
          {"id":"11111111-1111-1111-1111-111111111111","name":"already-running",
           "created_at":"2026-01-01T00:00:00Z","deleted_at":null,
           "connections":[{"id":"c1","origin_ip":"203.0.113.5"},{"id":"c2","origin_ip":"203.0.113.6"}]},
          {"id":"22222222-2222-2222-2222-222222222222","name":"spare",
           "created_at":"2026-02-02T00:00:00Z","deleted_at":null,"connections":[]},
          {"id":"33333333-3333-3333-3333-333333333333","name":"removed",
           "created_at":"2026-03-03T00:00:00Z","deleted_at":"2026-04-04T00:00:00Z","connections":[]}
        ]"#,
    )
    .unwrap();
    fake
}

impl Cloudflared {
    fn invocations(&self, verb: &str) -> usize {
        std::fs::read_to_string(&self.trace)
            .unwrap_or_default()
            .lines()
            .filter(|line| line.contains(verb))
            .count()
    }
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
    let fake = fake_cloudflared(directory.path());
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
    let fake = fake_cloudflared(directory.path());
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
    assert!(refused.text.contains("Confirm"));
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
    let fake = fake_cloudflared(directory.path());
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
    let fake = fake_cloudflared(directory.path());
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
    let fake = fake_cloudflared(directory.path());
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
    }
    assert_eq!(fake.invocations("login"), 0);
    assert_eq!(fake.invocations("list"), 0);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}
