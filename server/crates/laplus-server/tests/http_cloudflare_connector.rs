#![cfg(unix)]

mod harness;

use harness::TestServer;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;

#[derive(Debug)]
struct VerifiedEndpoint;

impl laplus_server::public_exposure::EndpointVerifier for VerifiedEndpoint {
    fn verify<'a>(
        &'a self,
        _origin: &'a str,
        _environment_id: &'a str,
        _http_token: &'a str,
        _ws_token: &'a str,
    ) -> laplus_server::public_exposure::VerificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }
}

fn fake_cloudflared(
    directory: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let executable = directory.join("cloudflared-fake.py");
    let trace = directory.join("cloudflared.trace");
    let mode = directory.join("cloudflared.mode");
    let source = format!(
        r#"#!/usr/bin/env python3
import http.server, json, os, signal, sys
TRACE = {trace:?}
MODE = {mode:?}
if '--version' in sys.argv:
    print('cloudflared version 2026.7.0')
    raise SystemExit(0)
with open(TRACE, 'a') as f:
    f.write(json.dumps(sys.argv[1:]) + '\n')
metrics = sys.argv[sys.argv.index('--metrics') + 1]
token_file = sys.argv[sys.argv.index('--token-file') + 1]
with open(token_file) as f:
    assert f.read() == 'connector-secret'
if os.path.exists(MODE) and open(MODE).read().strip() == 'crash':
    print('connector failed with connector-secret', file=sys.stderr)
    raise SystemExit(17)
if os.path.exists(MODE) and open(MODE).read().strip() == 'replace':
    os.remove(MODE)
    if os.fork() > 0:
        raise SystemExit(0)
class Ready(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == '/ready' else 404)
        self.end_headers()
    def log_message(self, *args): pass
host, port = metrics.rsplit(':', 1)
server = http.server.HTTPServer((host, int(port)), Ready)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
try:
    server.serve_forever()
finally:
    with open(TRACE, 'a') as f: f.write('stopped\n')
"#,
        trace = trace.display().to_string(),
        mode = mode.display().to_string()
    );
    std::fs::write(&executable, source).unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).unwrap();
    (executable, trace, mode)
}

async fn wait_for_json(
    server: &TestServer,
    predicate: impl Fn(&serde_json::Value) -> bool,
) -> serde_json::Value {
    let mut last = serde_json::Value::Null;
    for _ in 0..100 {
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
    let (executable, trace, _) = fake_cloudflared(directory.path());
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
                "executablePath": executable,
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
    let invocation = std::fs::read_to_string(&trace).unwrap();
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
                "executablePath": executable,
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
    wait_for_json(&restarted, |body| body["verificationState"] == "verified").await;
    restarted.stop().await;
}

#[tokio::test]
async fn exhausted_connector_requires_retry_and_reconciles_repeated_start_stop_commands() {
    let directory = tempfile::tempdir().unwrap();
    let (executable, trace, mode) = fake_cloudflared(directory.path());
    std::fs::write(&mode, "crash").unwrap();
    let server = TestServer::start_configured_in(directory.path()).await;
    let configured = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":executable,"connectorToken":"connector-secret"}),
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
        std::fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with('['))
            .count(),
        3
    );

    let refused = server
        .post_json("/api/access/cloudflare/connector/start", &json!({}))
        .await;
    assert_eq!(refused.status, 400);
    assert!(refused.text.contains("use Retry"));

    std::fs::write(&mode, "ready").unwrap();
    let retried = server
        .post_json("/api/access/cloudflare/connector/retry", &json!({}))
        .await;
    assert_eq!(retried.status, 200, "{}", retried.text);
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    let launches = std::fs::read_to_string(&trace)
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
        std::fs::read_to_string(&trace)
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
        std::fs::read_to_string(&trace)
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
    let started = server
        .post_json("/api/access/cloudflare/connector/start", &json!({}))
        .await;
    assert_eq!(started.status, 200);
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(
        std::fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with('['))
            .count(),
        launches + 1
    );
    server.stop().await;
}

#[tokio::test]
async fn incompatible_selected_executable_is_actionable_without_echoing_the_token() {
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("old-cloudflared");
    std::fs::write(
        &executable,
        "#!/bin/sh\necho 'cloudflared version 2023.10.0'\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let server = TestServer::start_configured_in(directory.path()).await;
    let response = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":executable,"connectorToken":"never-echo-this"}),
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

#[tokio::test]
async fn a_ready_replacement_is_adopted_without_launching_a_duplicate_and_stops_with_its_owner() {
    let directory = tempfile::tempdir().unwrap();
    let (executable, trace, mode) = fake_cloudflared(directory.path());
    std::fs::write(&mode, "replace").unwrap();
    let server = TestServer::start_configured_in(directory.path()).await;
    let configured = server.post_json(
        "/api/access/cloudflare/connector/configure",
        &json!({"hostname":"laplus.example.com","executablePath":executable,"connectorToken":"connector-secret"}),
    ).await;
    assert_eq!(configured.status, 200, "{}", configured.text);
    wait_for_json(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(
        std::fs::read_to_string(&trace)
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
    assert!(std::fs::read_to_string(&trace).unwrap().contains("stopped"));
    server.stop().await;
}
