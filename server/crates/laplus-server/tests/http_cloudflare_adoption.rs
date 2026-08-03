//! Dedicating an inactive existing tunnel to this environment, through the real
//! routes.
//!
//! **What adoption is, in one sentence:** laplus retrieves the narrow run
//! credential of a tunnel somebody else allocated, writes its own isolated
//! configuration for it, and supervises a connector — while the Cloudflare
//! allocation and DNS route stay owned outside laplus and can never be deleted
//! from here (ADR-0045, ADR-0049).
//!
//! Three of these tests are about the ways that can go wrong rather than the way
//! it goes right: a connector that starts between the offer and the
//! confirmation, an adoption interrupted half way, and a client trying to
//! re-describe the result as somebody else's hostname.
#![cfg(unix)]

mod harness;

use harness::cloudflare::{
    client_with, FakeCloudflared, VerifiedEndpoint, CERTIFICATE, TUNNEL_CREDENTIAL_SECRET,
};
use harness::TestServer;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;

/// The inactive tunnel in the fake's default listing.
const SPARE: &str = "22222222-2222-2222-2222-222222222222";
/// The active one.
const RUNNING: &str = "11111111-1111-1111-1111-111111111111";

/// `TUNNEL_ORIGIN_CERT` is process-wide — cloudflared's own variable, and the
/// only honest way to point certificate discovery somewhere throwaway.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serially() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn wait_for_connector(
    server: &TestServer,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let mut last = Value::Null;
    // A hang detector, not a budget — `READ_TIMEOUT` in the harness carries the
    // reasoning. Nothing here asserts on elapsed time.
    for _ in 0..500 {
        let response = server.get("/api/access/cloudflare/connector").await;
        last = response.body.clone();
        if response.status == 200 && predicate(&response.body) {
            return response.body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("the connector never converged: {last}")
}

/// Consent, list, and choose the inactive tunnel — everything ticket 04 built,
/// which is where every test below starts.
async fn choose_the_spare_tunnel(server: &TestServer, fake: &FakeCloudflared) {
    let consented = server
        .post_json(
            "/api/access/cloudflare/account/consent",
            &json!({"consented": true}),
        )
        .await;
    assert_eq!(consented.status, 200, "{}", consented.text);
    let listed = server
        .post_json(
            "/api/access/cloudflare/account/tunnels",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(listed.status, 200, "{}", listed.text);
    let chosen = server
        .post_json(
            "/api/access/cloudflare/account/select",
            &json!({"tunnelId": SPARE, "hostname": "spare.example.com"}),
        )
        .await;
    assert_eq!(chosen.status, 200, "{}", chosen.text);
    assert_eq!(chosen.body["step"], "confirm-adoption");
    assert_eq!(chosen.body["selection"]["adoptionConfirmed"], false);
}

/// Every place a run credential could leak, checked at once.
///
/// Four haystacks rather than one, because each has caught a different bug in
/// this feature: what the route answered, what a snapshot serialises, what
/// shows in a process listing, and what a durable non-secret record happens to
/// hold. The database is read as *bytes* — a value in a page SQLite has not
/// vacuumed is still on the disk.
fn nothing_leaks(texts: &[&str], trace: &Path, database: &Path) {
    for text in texts {
        assert!(
            !text.contains(TUNNEL_CREDENTIAL_SECRET),
            "a run credential reached a response or a snapshot"
        );
    }
    let argv = std::fs::read_to_string(trace).unwrap_or_default();
    assert!(
        !argv.contains(TUNNEL_CREDENTIAL_SECRET),
        "a run credential reached a command line"
    );
    assert!(!argv.contains(CERTIFICATE), "the account certificate was copied into an argument");
    // Read, not `unwrap_or_default`: a scan of a file that is not there passes
    // for the wrong reason, and it did — the in-memory database every other
    // Cloudflare test uses has no file at all.
    let persisted = std::fs::read(database).expect("a database file to scan");
    assert!(
        !persisted
            .windows(TUNNEL_CREDENTIAL_SECRET.len())
            .any(|window| window == TUNNEL_CREDENTIAL_SECRET.as_bytes()),
        "a run credential reached non-secret persistence"
    );
}

#[tokio::test]
async fn dedicating_an_inactive_tunnel_runs_it_without_claiming_its_allocation() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    // cloudflared's own default configuration, which ADR-0045 puts out of scope
    // entirely. Written here so that "laplus never edits it" is a claim about a
    // file that exists rather than one that does not.
    let home = directory.path().join("home");
    let default_config = home.join(".cloudflared").join("config.yml");
    std::fs::create_dir_all(default_config.parent().unwrap()).unwrap();
    std::fs::write(&default_config, "# the developer's own\n").unwrap();

    let server = TestServer::start_persistent_in(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    choose_the_spare_tunnel(&server, &fake).await;

    // The loopback target the confirmation has to show, before anything is
    // configured. Nothing else on the wire knows it.
    let before = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(before.body["configured"], false);
    assert!(before.body["loopbackOrigin"]
        .as_str()
        .expect("a loopback target to confirm")
        .starts_with("http://127.0.0.1:"));

    let adopted = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(adopted.status, 200, "{}", adopted.text);
    assert_eq!(adopted.body["step"], "adopting");
    assert_eq!(adopted.body["selection"]["adoptionConfirmed"], true);
    assert_eq!(adopted.body["selection"]["tunnelId"], SPARE);

    // Persisted as dedicated and laplus-managed locally, and undeletable at
    // Cloudflare — which is the whole of what "adopted" means.
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "adopted");
    assert_eq!(endpoint.body["deletableAtCloudflare"], false);
    assert_eq!(endpoint.body["httpsOrigin"], "https://spare.example.com");

    let ready = wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(ready["tunnelOwnership"], "adopted");
    assert_eq!(ready["deletableAtCloudflare"], false);
    assert_eq!(ready["readiness"], true);
    assert_eq!(ready["desiredState"], "running");
    let verified =
        wait_for_connector(&server, |body| body["verificationState"] == "verified").await;
    assert_eq!(verified["connectorState"], "ready");

    // A verified adopted endpoint is advertised for pairing, as laplus-run.
    let advertised = server.get("/api/access/cloudflare").await;
    assert_eq!(advertised.body["health"]["connector"], "laplus");
    let offer = &advertised.body["advertisedEndpoint"];
    assert_eq!(offer["httpBaseUrl"], "https://spare.example.com");
    assert_eq!(offer["wsBaseUrl"], "wss://spare.example.com");
    assert_eq!(offer["status"], "available");
    assert_eq!(offer["source"], "server");
    let paired = server
        .post_json("/api/auth/pairing-token", &json!({"label": "Cloudflare Tunnel"}))
        .await;
    assert_eq!(paired.status, 200, "{}", paired.text);
    assert!(paired.body["credential"].as_str().is_some_and(|value| !value.is_empty()));

    // The credential was *retrieved* for a tunnel that already existed, never
    // created — adoption allocates nothing at Cloudflare.
    assert_eq!(fake.invocations("token"), 1);
    assert_eq!(fake.invocations("create"), 0);
    assert_eq!(fake.invocations("route"), 0);
    assert_eq!(fake.invocations("delete"), 0);
    let credential = directory.path().join("cloudflare").join("tunnel.json");
    assert_eq!(
        fake.retrieved_credential_path().as_deref(),
        Some(credential.as_path())
    );
    assert!(credential.is_file());
    assert_eq!(
        std::fs::metadata(&credential).unwrap().permissions().mode() & 0o077,
        0
    );
    assert!(std::fs::read_to_string(&credential)
        .unwrap()
        .contains(TUNNEL_CREDENTIAL_SECRET));

    // Only laplus's own configuration, and the developer's is byte-identical.
    let configuration = directory.path().join("cloudflare").join("connector.yml");
    let written = std::fs::read_to_string(&configuration).unwrap();
    assert!(written.contains(&format!("tunnel: {SPARE}")), "{written}");
    assert!(
        written.contains(&format!("credentials-file: {}", credential.display())),
        "{written}"
    );
    assert!(written.contains("hostname: spare.example.com"), "{written}");
    assert_eq!(
        std::fs::read_to_string(&default_config).unwrap(),
        "# the developer's own\n"
    );
    // No system service: every launch of cloudflared was a command laplus ran
    // itself, and `service install` is not among them.
    assert_eq!(fake.invocations("service"), 0);

    nothing_leaks(
        &[&adopted.text, &endpoint.text, &verified.to_string()],
        &fake.trace,
        &directory.path().join("state.sqlite"),
    );

    // Stop stays available for an adopted tunnel, and changes nothing at
    // Cloudflare. Ticket 07 owns Forget and Delete everywhere.
    let stopped = server
        .post_json("/api/access/cloudflare/connector/stop", &json!({}))
        .await;
    assert_eq!(stopped.status, 200, "{}", stopped.text);
    assert_eq!(stopped.body["desiredState"], "stopped");
    wait_for_connector(&server, |body| body["connectorState"] == "stopped").await;
    let after_stop = server.get("/api/access/cloudflare").await;
    assert_eq!(after_stop.body["ownership"], "adopted");
    assert_eq!(after_stop.body["httpsOrigin"], "https://spare.example.com");
    assert!(credential.is_file(), "stopping must not remove the credential");
    assert_eq!(fake.invocations("delete"), 0);

    let started = server
        .post_json("/api/access/cloudflare/connector/start", &json!({}))
        .await;
    assert_eq!(started.status, 200, "{}", started.text);
    server.stop().await;

    // The connector starts with its owner, and ownership survives with it.
    let restarted = TestServer::start_persistent_in(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    let restored = wait_for_connector(&restarted, |body| body["connectorState"] == "ready").await;
    assert_eq!(restored["tunnelOwnership"], "adopted");
    assert_eq!(restored["httpsOrigin"], "https://spare.example.com");
    let resumed = restarted.get("/api/access/cloudflare/account").await;
    assert_eq!(resumed.body["step"], "adopting");
    nothing_leaks(
        &[&restored.to_string(), &resumed.text],
        &fake.trace,
        &directory.path().join("state.sqlite"),
    );
    restarted.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// A connector that starts between the offer and the confirmation.
///
/// The listing that produced the dedication screen is evidence about the past,
/// and ADR-0045 makes an active tunnel externally managed. So the answer laplus
/// acts on is re-read immediately before the first mutation — and the fallback
/// is a complete external tunnel endpoint rather than a half-finished adoption.
#[tokio::test]
async fn a_tunnel_that_becomes_active_falls_back_to_external_ownership() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server = TestServer::start_persistent_in(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    choose_the_spare_tunnel(&server, &fake).await;

    // Somebody else's connector arrives. Nothing about the recorded selection
    // changes; what changes is what Cloudflare would answer now.
    std::fs::write(
        &fake.tunnels,
        json!([
            {"id": RUNNING, "name": "already-running", "created_at": "2026-01-01T00:00:00Z",
             "deleted_at": null, "connections": [{"id": "c1"}]},
            {"id": SPARE, "name": "spare", "created_at": "2026-02-02T00:00:00Z",
             "deleted_at": null, "connections": [{"id": "c9"}, {"id": "c10"}]},
        ])
        .to_string(),
    )
    .unwrap();

    let refused = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(refused.status, 409, "{}", refused.text);
    assert_eq!(refused.body["_tag"], "EnvironmentPublicExposurePreconditionError");
    assert_eq!(refused.body["reason"], "tunnel-became-active");
    // Nothing was mutated, and the refusal says so rather than claiming a
    // rollback it never performed.
    assert_eq!(refused.body["completed"], json!([]));
    assert_eq!(refused.body["remaining"], json!(["credential", "configuration"]));

    // No credential was fetched and no connector was configured.
    assert_eq!(fake.invocations("token"), 0);
    let connector = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(connector.body["configured"], false);
    assert_eq!(connector.body["connectorState"], "unconfigured");

    // The hostname is still verified and advertised — as somebody else's.
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["configured"], true);
    assert_eq!(endpoint.body["ownership"], "external");
    assert_eq!(endpoint.body["deletableAtCloudflare"], false);
    assert_eq!(endpoint.body["httpsOrigin"], "https://spare.example.com");
    let account = server.get("/api/access/cloudflare/account").await;
    assert_eq!(account.body["selection"]["classification"], "external");
    assert_eq!(account.body["selection"]["adoptionConfirmed"], false);
    assert_eq!(account.body["step"], "verify-hostname");

    // And asking again does not quietly adopt it after all.
    let again = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(again.status, 409, "{}", again.text);
    assert_eq!(again.body["reason"], "ownership-conflict");
    assert_eq!(fake.invocations("token"), 0);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// An adoption that fails after the credential and before the configuration.
///
/// The credential is the expensive half: it spends the account certificate at
/// Cloudflare. A retry that fetched it again would be repeating a mutation the
/// journal already recorded, which is exactly what the acceptance criterion
/// forbids — so the resume reconciles against the file that is actually there.
#[tokio::test]
async fn an_interrupted_adoption_resumes_without_repeating_the_credential() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server = TestServer::start_persistent_in(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    choose_the_spare_tunnel(&server, &fake).await;

    // Make the configuration write fail, and only that: a directory cannot be
    // replaced by a file, so laplus's own ingress file cannot be installed while
    // the credential retrieval before it succeeds normally.
    let configuration = directory.path().join("cloudflare").join("connector.yml");
    std::fs::create_dir_all(&configuration).unwrap();

    let partial = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(partial.status, 400, "{}", partial.text);
    assert_eq!(partial.body["_tag"], "EnvironmentPublicExposureRejectedError");
    assert_eq!(partial.body["reason"], "local-setup-failed");
    // Both halves named: what happened, and what is left.
    assert_eq!(partial.body["completed"], json!(["credential"]));
    assert_eq!(partial.body["remaining"], json!(["configuration"]));
    assert_eq!(fake.invocations("token"), 1);

    // The wizard has not moved on from work that did not finish.
    let stalled = server.get("/api/access/cloudflare/account").await;
    assert_eq!(stalled.body["step"], "confirm-adoption");
    assert_eq!(stalled.body["selection"]["adoptionConfirmed"], false);
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["configured"], false);

    // The credential the first attempt retrieved is still there, which is what
    // the resume reconciles against.
    let credential = directory.path().join("cloudflare").join("tunnel.json");
    assert!(credential.is_file());

    // **Across a restart, not merely across two calls.** The journal and the
    // credential are what survive a process ending mid-adoption, and they are
    // the only things the resume may rely on — nothing in memory does.
    server.stop().await;
    let server = TestServer::start_persistent_in(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    let interrupted = server.get("/api/access/cloudflare/account").await;
    assert_eq!(interrupted.body["step"], "confirm-adoption");

    std::fs::remove_dir(&configuration).unwrap();
    let resumed = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(resumed.status, 200, "{}", resumed.text);
    assert_eq!(resumed.body["step"], "adopting");
    // The point of the whole test: one credential retrieval, across two
    // attempts and a restart, because the second reconciled instead of
    // repeating.
    assert_eq!(fake.invocations("token"), 1);
    assert_eq!(fake.invocations("create"), 0);

    let ready = wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(ready["tunnelOwnership"], "adopted");
    nothing_leaks(
        &[&partial.text, &resumed.text, &ready.to_string()],
        &fake.trace,
        &directory.path().join("state.sqlite"),
    );

    // A repeated confirmation after success is a reconciliation, not a second
    // adoption — and must not mistake laplus's own connector for somebody
    // else's. The listing now shows connections, and they are this server's.
    std::fs::write(
        &fake.tunnels,
        json!([{"id": SPARE, "name": "spare", "created_at": "2026-02-02T00:00:00Z",
                "deleted_at": null, "connections": [{"id": "laplus-1"}]}])
        .to_string(),
    )
    .unwrap();
    let repeated = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(repeated.status, 200, "{}", repeated.text);
    assert_eq!(repeated.body["step"], "adopting");
    assert_eq!(fake.invocations("token"), 1);
    let held = server.get("/api/access/cloudflare").await;
    assert_eq!(held.body["ownership"], "adopted");

    // The same, with the endpoint row gone — which is the state a confirmation
    // interrupted between configuring the connector and recording the row would
    // leave, and the state today's local-only Forget leaves too. The connector
    // is the surviving record, and it is enough: the tunnel is re-recorded as
    // adopted rather than disowned because laplus is serving it.
    let forgotten = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(forgotten.status, 200, "{}", forgotten.text);
    assert_eq!(forgotten.body["configured"], false);
    let recovered = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(recovered.status, 200, "{}", recovered.text);
    let repaired = server.get("/api/access/cloudflare").await;
    assert_eq!(repaired.body["ownership"], "adopted");
    assert_eq!(repaired.body["httpsOrigin"], "https://spare.example.com");
    assert_eq!(fake.invocations("token"), 1);

    // **Forget as it exists today**: local only, and nothing at Cloudflare.
    // Ticket 07 owns the forget a supervised connector needs — stop it, remove
    // laplus's own configuration and credential, then the row — so what is
    // pinned here is the half that is true now: no `tunnel delete`, no `route`,
    // and the tunnel laplus never allocated is still allocated. That the
    // credential and configuration survive is the gap, not the behaviour.
    let forgotten = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(forgotten.status, 200, "{}", forgotten.text);
    assert_eq!(forgotten.body["configured"], false);
    assert_eq!(fake.invocations("delete"), 0);
    assert_eq!(fake.invocations("route"), 0);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// A credential retrieval that fails after writing something.
///
/// **The reason a resume cannot decide by file existence alone.** cloudflared
/// creates the file it was pointed at and can still exit non-zero, and a retry
/// that saw a truncated `<UUID>.json` and skipped the retrieval would configure
/// a connector against a credential that authenticates nothing — a failure that
/// would then look like Cloudflare rejecting laplus rather than like the
/// retrieval that never finished.
#[tokio::test]
async fn a_half_written_credential_is_not_mistaken_for_a_retrieved_one() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server = TestServer::start_persistent_in(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    choose_the_spare_tunnel(&server, &fake).await;

    fake.rehearse("token-fails");
    let refused = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert_eq!(refused.body["reason"], "command-failed");
    // Nothing completed, because nothing usable was produced — and the wreckage
    // was taken away rather than left for the next attempt to trust.
    assert_eq!(refused.body["completed"], json!([]));
    assert_eq!(refused.body["remaining"], json!(["credential", "configuration"]));
    let credential = directory.path().join("cloudflare").join("tunnel.json");
    assert!(!credential.exists(), "a failed retrieval left a credential behind");
    let stalled = server.get("/api/access/cloudflare/account").await;
    assert_eq!(stalled.body["step"], "confirm-adoption");

    fake.behave();
    let resumed = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(resumed.status, 200, "{}", resumed.text);
    assert_eq!(resumed.body["step"], "adopting");
    // Retrieved twice, because the first attempt produced nothing to reuse.
    assert_eq!(fake.invocations("token"), 2);
    assert!(std::fs::read_to_string(&credential)
        .unwrap()
        .contains(TUNNEL_CREDENTIAL_SECRET));
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// Adoption is an administrative mutation, and its result is not a value a
/// client may re-describe.
#[tokio::test]
async fn adoption_requires_write_and_its_ownership_is_not_the_clients_to_change() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server = TestServer::start_persistent_in(
        directory.path(),
        std::sync::Arc::new(VerifiedEndpoint),
    )
    .await;
    choose_the_spare_tunnel(&server, &fake).await;

    // ADR-0047: a session without the scope learns which scope, and nothing
    // about the Cloudflare state behind the refusal.
    let reader = client_with(&server, &["access:read"]).await;
    let refused = server
        .post_json_as(
            "/api/access/cloudflare/account/adopt",
            &reader,
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(refused.status, 403, "{}", refused.text);
    assert_eq!(refused.body["_tag"], "EnvironmentScopeRequiredError");
    assert_eq!(refused.body["requiredScope"], "access:write");
    assert!(refused.body.get("reason").is_none());
    assert!(refused.body.get("completed").is_none());
    assert_eq!(fake.invocations("token"), 0);

    let adopted = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(adopted.status, 200, "{}", adopted.text);

    // Every route that writes the endpoint row refuses to re-describe an
    // adopted tunnel as somebody else's. A hidden button stops a person; these
    // stop a stale tab, a script, and a repeated or forged request — which is
    // what makes "never offered for deletion" an answer rather than a layout.
    for (path, body) in [
        (
            "/api/access/cloudflare",
            json!({"hostname": "spare.example.com"}),
        ),
        (
            "/api/access/cloudflare/account/select",
            json!({"tunnelId": RUNNING, "hostname": "elsewhere.example.com"}),
        ),
        (
            "/api/access/cloudflare/connector/configure",
            json!({"hostname": "spare.example.com", "executablePath": fake.executable,
                   "connectorToken": "connector-secret"}),
        ),
    ] {
        let laundered = server.post_json(path, &body).await;
        assert_eq!(laundered.status, 409, "{path}: {}", laundered.text);
        assert_eq!(laundered.body["reason"], "ownership-conflict", "{path}");
        let held = server.get("/api/access/cloudflare").await;
        assert_eq!(held.body["ownership"], "adopted", "{path}");
        assert_eq!(held.body["deletableAtCloudflare"], false, "{path}");
        assert_eq!(held.body["httpsOrigin"], "https://spare.example.com", "{path}");
    }
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}
