//! App-managed `cloudflared` installation, driven through the real routes.
//!
//! The release feed and the artifact are local fakes: nothing here contacts
//! Cloudflare, downloads a real executable, or runs one. What is under test is
//! the supply chain around the download — approval of an identified release,
//! checksum verification, atomic promotion, ownership — and that the executable
//! laplus installs is the one the connector path then uses.
#![cfg(unix)]

mod harness;

use harness::{ClientIdentity, TestServer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The public path is already covered end to end by the verification tests; what
/// matters here is that an installed executable reaches it at all.
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

/// The release-feed override is process-wide, so these tests take turns.
///
/// It is an environment variable rather than a constructor argument because the
/// installer is reached through the server's routes, and the harness builds the
/// server. One binary, one lock, and each test still gets its own fake feed.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serially() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

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

/// A stand-in for Cloudflare's published release, and for the executable.
///
/// The artifact is the same fake connector the supervision tests run, so an
/// installed copy can be pointed at `configure` and actually become ready —
/// which is the only way to check that what was installed is usable rather than
/// merely present.
struct FakeRelease {
    origin: String,
    state: Arc<Mutex<FeedState>>,
    artifact: Arc<Vec<u8>>,
}

#[derive(Clone)]
struct FeedState {
    version: String,
    /// `ok`, `corrupt`, `truncate`, `no-checksum`, or `no-asset`.
    mode: String,
}

fn connector_artifact(trace: &std::path::Path) -> Vec<u8> {
    format!(
        r#"#!/usr/bin/env python3
import http.server, json, signal, sys
TRACE = {trace:?}
if '--version' in sys.argv:
    print('cloudflared version 2026.7.3')
    raise SystemExit(0)
with open(TRACE, 'a') as f:
    f.write(json.dumps(sys.argv[1:]) + '\n')
metrics = sys.argv[sys.argv.index('--metrics') + 1]
class Ready(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200 if self.path == '/ready' else 404)
        self.end_headers()
    def log_message(self, *args): pass
host, port = metrics.rsplit(':', 1)
server = http.server.HTTPServer((host, int(port)), Ready)
signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
server.serve_forever()
"#,
        trace = trace.display().to_string()
    )
    .into_bytes()
}

impl FakeRelease {
    async fn start(version: &str, artifact: Vec<u8>) -> FakeRelease {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(FeedState {
            version: version.into(),
            mode: "ok".into(),
        }));
        let artifact = Arc::new(artifact);
        let serving = FakeRelease {
            origin: origin.clone(),
            state: Arc::clone(&state),
            artifact: Arc::clone(&artifact),
        };
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let (origin, state, artifact) =
                    (origin.clone(), Arc::clone(&state), Arc::clone(&artifact));
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 2048];
                    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => request.extend_from_slice(&buffer[..read]),
                        }
                    }
                    let head = String::from_utf8_lossy(&request).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let feed = state.lock().unwrap().clone();
                    if path.ends_with("/releases/latest") {
                        let body = release_feed(&origin, &feed, &artifact);
                        let _ = stream
                            .write_all(
                                format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                    body.len()
                                )
                                .as_bytes(),
                            )
                            .await;
                        return;
                    }
                    if !path.starts_with("/download/") {
                        let _ = stream
                            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                            .await;
                        return;
                    }
                    let bytes: Vec<u8> = if feed.mode == "corrupt" {
                        b"#!/bin/sh\nexit 1\n".to_vec()
                    } else {
                        served(&feed, &artifact)
                    };
                    let _ = stream
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                bytes.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                    if feed.mode == "truncate" {
                        let _ = stream.write_all(&bytes[..bytes.len() / 2]).await;
                        return;
                    }
                    let _ = stream.write_all(&bytes).await;
                });
            }
        });
        serving
    }

    fn set(&self, version: &str, mode: &str) {
        let mut state = self.state.lock().unwrap();
        state.version = version.into();
        state.mode = mode.into();
    }

    fn checksum(&self) -> String {
        format!("{:x}", Sha256::digest(self.artifact.as_ref()))
    }
}

/// What this feed would serve, so the published digest and the download agree.
///
/// `corrupt` is the deliberate exception — there the download is *not* what the
/// notes said it would be, which is the whole point of that mode.
fn served(feed: &FeedState, artifact: &[u8]) -> Vec<u8> {
    match feed.mode.as_str() {
        "incompatible" => b"#!/bin/sh\necho 'cloudflared version 2019.1.0'\n".to_vec(),
        _ => artifact.to_vec(),
    }
}

fn release_feed(origin: &str, feed: &FeedState, artifact: &[u8]) -> String {
    let asset = laplus_server::cloudflare_install::asset_name()
        .expect("this platform has a published cloudflared artifact");
    let checksum = format!("{:x}", Sha256::digest(served(feed, artifact)));
    let body = match feed.mode.as_str() {
        "no-checksum" => "### SHA256 Checksums:\n```\ncloudflared-something-else: abc\n```".into(),
        _ => format!("### SHA256 Checksums:\n```\n{asset}: {checksum}\n```"),
    };
    let assets = if feed.mode == "no-asset" {
        json!([])
    } else {
        json!([{
            "name": asset,
            "browser_download_url": format!("{origin}/download/{asset}"),
        }])
    };
    json!({ "tag_name": feed.version, "body": body, "assets": assets }).to_string()
}

async fn wait_for_connector(server: &TestServer, state: &str) -> Value {
    let mut last = Value::Null;
    for _ in 0..200 {
        let response = server.get("/api/access/cloudflare/connector").await;
        last = response.body.clone();
        if response.body["connectorState"] == state {
            return response.body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("connector never reached {state}: {last}")
}

async fn wait_for_verification(server: &TestServer) -> Value {
    let mut last = Value::Null;
    for _ in 0..200 {
        let response = server.get("/api/access/cloudflare/connector").await;
        last = response.body.clone();
        if response.body["verificationState"] == "verified" {
            return response.body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the installed connector's endpoint was never verified: {last}")
}

fn installed_files(preferences: &std::path::Path) -> Vec<String> {
    let tools = preferences.join("cloudflare").join("tools");
    let Ok(entries) = std::fs::read_dir(&tools) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn an_approved_release_is_verified_promoted_and_runs_the_connector() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let trace = directory.path().join("cloudflared.trace");
    let release = FakeRelease::start("2026.7.3", connector_artifact(&trace)).await;
    std::env::set_var("LAPLUS_CLOUDFLARED_RELEASE_API", &release.origin);
    let server = TestServer::start_configured_in_with_endpoint_verifier(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;

    let preview = server.get("/api/access/cloudflare/install").await;
    assert_eq!(preview.status, 200, "{}", preview.text);
    assert_eq!(preview.body["supported"], true);
    assert_eq!(preview.body["ownership"], "app-managed");
    assert_eq!(preview.body["state"], "not-installed");
    assert_eq!(preview.body["release"]["version"], "2026.7.3");
    assert_eq!(preview.body["release"]["checksum"], release.checksum());
    assert!(preview.body["release"]["downloadUrl"]
        .as_str()
        .unwrap()
        .starts_with(&release.origin));
    assert!(preview.body["platform"].as_str().is_some());
    assert!(preview.body["architecture"].as_str().is_some());

    let stale = server
        .post_json(
            "/api/access/cloudflare/install",
            &json!({"version": "2020.1.1", "checksum": release.checksum()}),
        )
        .await;
    assert_eq!(stale.status, 409, "{}", stale.text);
    assert!(installed_files(directory.path()).is_empty());

    let installed = server
        .post_json(
            "/api/access/cloudflare/install",
            &json!({"version": "2026.7.3", "checksum": release.checksum()}),
        )
        .await;
    assert_eq!(installed.status, 200, "{}", installed.text);
    assert_eq!(installed.body["state"], "installed");
    assert_eq!(installed.body["installedVersion"], "2026.7.3");
    let path = std::path::PathBuf::from(installed.body["installedPath"].as_str().unwrap());
    assert!(path.starts_with(directory.path().join("cloudflare").join("tools")));
    let permissions = std::fs::metadata(&path).unwrap().permissions();
    assert_eq!(permissions.mode() & 0o777, 0o700);
    let mut expected = vec![
        "installed.json".to_string(),
        path.file_name().unwrap().to_string_lossy().into_owned(),
    ];
    expected.sort();
    assert_eq!(installed_files(directory.path()), expected);

    let discovered = server.get("/api/access/cloudflare/executables").await;
    let managed = discovered.body["executables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["source"] == "app-managed")
        .expect("the app-managed executable is discoverable");
    assert_eq!(managed["path"], path.to_string_lossy().as_ref());
    assert_eq!(managed["compatibility"], "compatible");

    let configured = server
        .post_json(
            "/api/access/cloudflare/connector/configure",
            &json!({
                "hostname": "laplus.example.com",
                "executablePath": path,
                "connectorToken": "connector-secret"
            }),
        )
        .await;
    assert_eq!(configured.status, 200, "{}", configured.text);
    wait_for_connector(&server, "ready").await;
    assert!(std::fs::read_to_string(&trace).unwrap().contains("--token-file"));
    let verified = wait_for_verification(&server).await;
    assert_eq!(verified["httpsOrigin"], "https://laplus.example.com");
    let paired = server
        .post_json("/api/auth/pairing-token", &json!({"scopes": ["orchestration:read"]}))
        .await;
    assert_eq!(paired.status, 200, "{}", paired.text);
    server.stop().await;

    let restarted = TestServer::start_configured_in_with_endpoint_verifier(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    let after = restarted.get("/api/access/cloudflare/install").await;
    assert_eq!(after.body["state"], "installed");
    assert_eq!(after.body["installedVersion"], "2026.7.3");
    restarted.stop().await;
    std::env::remove_var("LAPLUS_CLOUDFLARED_RELEASE_API");
}

#[tokio::test]
async fn a_corrupt_or_interrupted_download_is_never_promoted_and_stays_retryable() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let trace = directory.path().join("cloudflared.trace");
    let release = FakeRelease::start("2026.7.3", connector_artifact(&trace)).await;
    std::env::set_var("LAPLUS_CLOUDFLARED_RELEASE_API", &release.origin);
    let server = TestServer::start_configured_in(directory.path()).await;
    let approval = json!({"version": "2026.7.3", "checksum": release.checksum()});

    release.set("2026.7.3", "corrupt");
    let corrupted = server
        .post_json("/api/access/cloudflare/install", &approval)
        .await;
    assert_eq!(corrupted.status, 400, "{}", corrupted.text);
    assert!(corrupted.text.contains("checksum"), "{}", corrupted.text);
    assert!(installed_files(directory.path()).is_empty());
    let state = server.get("/api/access/cloudflare/install").await;
    assert_eq!(state.body["state"], "failed");
    assert!(state.body["failureMessage"].as_str().is_some());

    release.set("2026.7.3", "truncate");
    let interrupted = server
        .post_json("/api/access/cloudflare/install", &approval)
        .await;
    assert_eq!(interrupted.status, 400, "{}", interrupted.text);
    assert!(installed_files(directory.path()).is_empty());
    server.stop().await;

    let restarted = TestServer::start_configured_in(directory.path()).await;
    let truthful = restarted.get("/api/access/cloudflare/install").await;
    assert_eq!(truthful.body["state"], "not-installed");
    assert_eq!(truthful.body["installedPath"], Value::Null);

    release.set("2026.7.3", "ok");
    let retried = restarted
        .post_json("/api/access/cloudflare/install", &approval)
        .await;
    assert_eq!(retried.status, 200, "{}", retried.text);
    assert_eq!(retried.body["state"], "installed");
    restarted.stop().await;
    std::env::remove_var("LAPLUS_CLOUDFLARED_RELEASE_API");
}

#[tokio::test]
async fn an_unpublished_artifact_or_checksum_is_refused_before_anything_is_downloaded() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let trace = directory.path().join("cloudflared.trace");
    let release = FakeRelease::start("2026.7.3", connector_artifact(&trace)).await;
    std::env::set_var("LAPLUS_CLOUDFLARED_RELEASE_API", &release.origin);
    let server = TestServer::start_configured_in(directory.path()).await;
    let approval = json!({"version": "2026.7.3", "checksum": release.checksum()});

    release.set("2026.7.3", "no-asset");
    let missing = server
        .post_json("/api/access/cloudflare/install", &approval)
        .await;
    assert_eq!(missing.status, 400, "{}", missing.text);
    let previewed = server.get("/api/access/cloudflare/install").await;
    assert_eq!(previewed.body["release"], Value::Null);
    assert!(previewed.body["releaseFailureMessage"].as_str().is_some());

    release.set("2026.7.3", "no-checksum");
    let unverifiable = server
        .post_json("/api/access/cloudflare/install", &approval)
        .await;
    assert_eq!(unverifiable.status, 400, "{}", unverifiable.text);
    assert!(installed_files(directory.path()).is_empty());
    server.stop().await;
    std::env::remove_var("LAPLUS_CLOUDFLARED_RELEASE_API");
}

/// Reinstalling the version already installed promotes onto the same name, so a
/// failure *after* that rename has removed the copy the record names. The
/// wizard has to say so: an installation that reports "installed" while
/// pointing at a file that is gone sends the developer to configure a connector
/// with an executable that does not exist.
#[tokio::test]
async fn a_failed_reinstall_of_the_installed_version_stops_claiming_to_be_installed() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let trace = directory.path().join("cloudflared.trace");
    let release = FakeRelease::start("2026.7.3", connector_artifact(&trace)).await;
    std::env::set_var("LAPLUS_CLOUDFLARED_RELEASE_API", &release.origin);
    let server = TestServer::start_configured_in(directory.path()).await;

    let installed = server
        .post_json(
            "/api/access/cloudflare/install",
            &json!({"version": "2026.7.3", "checksum": release.checksum()}),
        )
        .await;
    assert_eq!(installed.status, 200, "{}", installed.text);
    let path = std::path::PathBuf::from(installed.body["installedPath"].as_str().unwrap());

    // The same version, republished as something this laplus cannot run.
    release.set("2026.7.3", "incompatible");
    let preview = server.get("/api/access/cloudflare/install").await;
    let checksum = preview.body["release"]["checksum"].as_str().unwrap().to_string();
    let refused = server
        .post_json(
            "/api/access/cloudflare/install",
            &json!({"version": "2026.7.3", "checksum": checksum}),
        )
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert!(refused.text.contains("incompatible"), "{}", refused.text);

    assert!(!path.exists(), "an unrunnable executable is not left behind");
    let truthful = server.get("/api/access/cloudflare/install").await;
    assert_eq!(truthful.body["state"], "failed");
    assert_eq!(truthful.body["installedPath"], Value::Null);
    assert_eq!(truthful.body["installedVersion"], Value::Null);
    assert!(truthful.body["failureMessage"].as_str().is_some());
    server.stop().await;
    std::env::remove_var("LAPLUS_CLOUDFLARED_RELEASE_API");
}

#[tokio::test]
async fn installing_replaces_only_the_copy_laplus_owns() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let trace = directory.path().join("cloudflared.trace");
    let elsewhere = directory.path().join("system-cloudflared");
    std::fs::write(&elsewhere, "#!/bin/sh\necho 'cloudflared version 2026.1.0'\n").unwrap();
    std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
    let release = FakeRelease::start("2026.7.3", connector_artifact(&trace)).await;
    std::env::set_var("LAPLUS_CLOUDFLARED_RELEASE_API", &release.origin);
    let server = TestServer::start_configured_in(directory.path()).await;

    let first = server
        .post_json(
            "/api/access/cloudflare/install",
            &json!({"version": "2026.7.3", "checksum": release.checksum()}),
        )
        .await;
    assert_eq!(first.status, 200, "{}", first.text);
    let first_path = first.body["installedPath"].as_str().unwrap().to_string();

    release.set("2026.8.1", "ok");
    let second = server
        .post_json(
            "/api/access/cloudflare/install",
            &json!({"version": "2026.8.1", "checksum": release.checksum()}),
        )
        .await;
    assert_eq!(second.status, 200, "{}", second.text);
    let second_path = second.body["installedPath"].as_str().unwrap().to_string();
    assert_ne!(first_path, second_path);
    assert!(!std::path::Path::new(&first_path).exists());
    assert!(std::path::Path::new(&second_path).exists());
    assert!(elsewhere.exists(), "a system executable is never touched");
    assert_eq!(
        std::fs::read_to_string(&elsewhere).unwrap(),
        "#!/bin/sh\necho 'cloudflared version 2026.1.0'\n"
    );
    server.stop().await;
    std::env::remove_var("LAPLUS_CLOUDFLARED_RELEASE_API");
}

#[tokio::test]
async fn installation_requires_write_and_state_requires_read() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let trace = directory.path().join("cloudflared.trace");
    let release = FakeRelease::start("2026.7.3", connector_artifact(&trace)).await;
    std::env::set_var("LAPLUS_CLOUDFLARED_RELEASE_API", &release.origin);
    let server = TestServer::start_configured_in(directory.path()).await;

    let ordinary = client_with(&server, &["orchestration:read"]).await;
    let hidden = server
        .get_as("/api/access/cloudflare/install", &ordinary)
        .await;
    assert_eq!(hidden.status, 403);
    assert_eq!(hidden.body["requiredScope"], "access:read");
    assert!(hidden.body.get("state").is_none());
    assert!(hidden.body.get("release").is_none());

    let reader = client_with(&server, &["access:read"]).await;
    let readable = server
        .get_as("/api/access/cloudflare/install", &reader)
        .await;
    assert_eq!(readable.status, 200, "{}", readable.text);
    let refused = server
        .post_json_as(
            "/api/access/cloudflare/install",
            &reader,
            &json!({"version": "2026.7.3", "checksum": release.checksum()}),
        )
        .await;
    assert_eq!(refused.status, 403);
    assert_eq!(refused.body["requiredScope"], "access:write");
    assert!(refused.body.get("state").is_none());
    assert!(installed_files(directory.path()).is_empty());
    server.stop().await;
    std::env::remove_var("LAPLUS_CLOUDFLARED_RELEASE_API");
}
