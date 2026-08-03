//! Creating a stable dedicated tunnel for this environment, through the real
//! routes.
//!
//! **What creation is, in one sentence:** laplus allocates a Cloudflare tunnel,
//! routes a DNS name to it, writes its own isolated configuration, and
//! supervises a connector — and is the only owner that may ever delete the
//! allocation and the record it made (ADR-0045, ADR-0049).
//!
//! **The interesting half is the way it stops.** Creation is three mutations at
//! two different places, so the tests below spend most of their length on the
//! boundaries between them: a `tunnel create` that refuses, a `route dns` that
//! refuses after the tunnel exists, and a configuration that will not write
//! after both. At each one the refusal has to name what happened and what is
//! left, a restart has to reconcile the same way, and a retry has to allocate
//! nothing twice.
#![cfg(unix)]

mod harness;

use harness::cloudflare::{
    client_with, FakeCloudflared, VerifiedEndpoint, CERTIFICATE, CREATED_TUNNEL_ID,
    TUNNEL_CREDENTIAL_SECRET,
};
use harness::TestServer;
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;

/// The inactive tunnel in the fake's default listing — ticket 05's.
const SPARE: &str = "22222222-2222-2222-2222-222222222222";
const NAME: &str = "laplus-desk";
const HOSTNAME: &str = "stable.example.com";
const ORIGIN: &str = "https://stable.example.com";

/// `TUNNEL_ORIGIN_CERT` is process-wide — cloudflared's own variable, and the
/// only honest way to point certificate discovery somewhere throwaway.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn serially() -> std::sync::MutexGuard<'static, ()> {
    ONE_AT_A_TIME
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

async fn wait_for_connector(server: &TestServer, predicate: impl Fn(&Value) -> bool) -> Value {
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

/// Consent to the certificate that is already on this machine — everything
/// ticket 04 built, which is where creation starts. A tunnel does not have to be
/// listed first: creation makes one rather than choosing one.
async fn consent(server: &TestServer) {
    let consented = server
        .post_json(
            "/api/access/cloudflare/account/consent",
            &json!({"consented": true}),
        )
        .await;
    assert_eq!(consented.status, 200, "{}", consented.text);
}

async fn create(server: &TestServer, fake: &FakeCloudflared) -> harness::HttpResponse {
    server
        .post_json(
            "/api/access/cloudflare/account/create",
            &json!({"executablePath": fake.executable, "name": NAME, "hostname": HOSTNAME}),
        )
        .await
}

/// Every place a run credential could leak, checked at once.
///
/// Four haystacks rather than one, because each has caught a different bug in
/// this feature: what the route answered, what a snapshot serialises, what shows
/// in a process listing, and what a durable non-secret record happens to hold.
/// The database is read as *bytes* — a value in a page SQLite has not vacuumed
/// is still on the disk.
fn nothing_leaks(texts: &[&str], trace: &Path, database: &Path) {
    for text in texts {
        assert!(
            !text.contains(TUNNEL_CREDENTIAL_SECRET),
            "a run credential reached a response or a snapshot"
        );
        assert!(
            !text.contains(CERTIFICATE),
            "the account certificate reached a response or a snapshot"
        );
    }
    let argv = std::fs::read_to_string(trace).unwrap_or_default();
    assert!(
        !argv.contains(TUNNEL_CREDENTIAL_SECRET),
        "a run credential reached a command line"
    );
    assert!(
        !argv.contains(CERTIFICATE),
        "the account certificate was copied into an argument"
    );
    let persisted = std::fs::read(database).expect("a database file to scan");
    assert!(
        !persisted
            .windows(TUNNEL_CREDENTIAL_SECRET.len())
            .any(|window| window == TUNNEL_CREDENTIAL_SECRET.as_bytes()),
        "a run credential reached non-secret persistence"
    );
}

/// The whole path, and the ownership it is the only one to produce.
#[tokio::test]
async fn creating_a_tunnel_routes_it_and_leaves_laplus_the_only_owner_that_may_delete_it() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    // cloudflared's own default configuration, which ADR-0045 puts out of scope
    // entirely. Written here so that "laplus never edits it" is a claim about a
    // file that exists rather than one that does not.
    let default_config = directory.path().join("home").join(".cloudflared").join("config.yml");
    std::fs::create_dir_all(default_config.parent().unwrap()).unwrap();
    std::fs::write(&default_config, "# the developer's own\n").unwrap();

    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    consent(&server).await;

    // The loopback target and the credential location the preview has to show,
    // before anything is created. Nothing else on the wire knows either.
    let before = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(before.body["configured"], false);
    assert!(before.body["loopbackOrigin"]
        .as_str()
        .expect("a loopback target to confirm")
        .starts_with("http://127.0.0.1:"));
    assert_eq!(
        before.body["credentialPath"].as_str(),
        directory.path().join("cloudflare").join("tunnel.json").to_str()
    );

    let created = create(&server, &fake).await;
    assert_eq!(created.status, 200, "{}", created.text);
    assert_eq!(created.body["step"], "creating");
    assert_eq!(created.body["selection"]["created"], true);
    assert_eq!(created.body["selection"]["adoptionConfirmed"], false);
    // The UUID Cloudflare allocated, not the name laplus asked for — cleanup
    // targets the resource that exists.
    assert_eq!(created.body["selection"]["tunnelId"], CREATED_TUNNEL_ID);
    assert_eq!(created.body["selection"]["name"], NAME);

    // The one ownership that authorizes a Cloudflare deletion, and the exact
    // resources it authorizes it over.
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "laplus-created");
    assert_eq!(endpoint.body["deletableAtCloudflare"], true);
    assert_eq!(endpoint.body["httpsOrigin"], ORIGIN);

    let ready = wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(ready["tunnelOwnership"], "laplus-created");
    assert_eq!(ready["deletableAtCloudflare"], true);
    assert_eq!(ready["readiness"], true);
    assert_eq!(ready["desiredState"], "running");
    let verified =
        wait_for_connector(&server, |body| body["verificationState"] == "verified").await;
    assert_eq!(verified["connectorState"], "ready");

    // A verified laplus-created endpoint is advertised for pairing, as laplus-run.
    let advertised = server.get("/api/access/cloudflare").await;
    assert_eq!(advertised.body["health"]["connector"], "laplus");
    let offer = &advertised.body["advertisedEndpoint"];
    assert_eq!(offer["httpBaseUrl"], ORIGIN);
    assert_eq!(offer["wsBaseUrl"], "wss://stable.example.com");
    assert_eq!(offer["status"], "available");
    assert_eq!(offer["source"], "server");
    let paired = server
        .post_json("/api/auth/pairing-token", &json!({"label": "Cloudflare Tunnel"}))
        .await;
    assert_eq!(paired.status, 200, "{}", paired.text);
    assert!(paired.body["credential"].as_str().is_some_and(|value| !value.is_empty()));

    // Exactly one allocation and one route, and no credential *retrieval*:
    // `tunnel create --credentials-file` writes the credential itself, so a
    // `tunnel token` here would be a second trip for a file already on disk.
    assert_eq!(fake.invocations("create"), 1);
    assert_eq!(fake.invocations("route"), 1);
    assert_eq!(fake.invocations("token"), 0);
    assert_eq!(fake.invocations("delete"), 0);
    // No system service: every launch of cloudflared was a command laplus ran
    // itself, and `service install` is not among them.
    assert_eq!(fake.invocations("service"), 0);

    let credential = directory.path().join("cloudflare").join("tunnel.json");
    assert_eq!(fake.credential_written_to().as_deref(), Some(credential.as_path()));
    assert!(credential.is_file());
    assert_eq!(std::fs::metadata(&credential).unwrap().permissions().mode() & 0o077, 0);

    // Only laplus's own configuration, and the developer's is byte-identical.
    let configuration = directory.path().join("cloudflare").join("connector.yml");
    let written = std::fs::read_to_string(&configuration).unwrap();
    assert!(written.contains(&format!("tunnel: {CREATED_TUNNEL_ID}")), "{written}");
    assert!(
        written.contains(&format!("credentials-file: {}", credential.display())),
        "{written}"
    );
    assert!(written.contains(&format!("hostname: {HOSTNAME}")), "{written}");
    assert_eq!(
        std::fs::read_to_string(&default_config).unwrap(),
        "# the developer's own\n"
    );
    // The account certificate is used in place and left exactly as it was.
    assert_eq!(std::fs::read_to_string(&fake.certificate).unwrap(), CERTIFICATE);

    nothing_leaks(
        &[&created.text, &endpoint.text, &verified.to_string()],
        &fake.trace,
        &directory.path().join("state.sqlite"),
    );

    // Stop is available for a laplus-created tunnel and changes nothing at
    // Cloudflare: the tunnel, the DNS route, the credential and the
    // configuration are all still there afterwards. Ticket 07 owns Forget and
    // Delete everywhere.
    let stopped = server
        .post_json("/api/access/cloudflare/connector/stop", &json!({}))
        .await;
    assert_eq!(stopped.status, 200, "{}", stopped.text);
    assert_eq!(stopped.body["desiredState"], "stopped");
    wait_for_connector(&server, |body| body["connectorState"] == "stopped").await;
    let after_stop = server.get("/api/access/cloudflare").await;
    assert_eq!(after_stop.body["ownership"], "laplus-created");
    assert_eq!(after_stop.body["deletableAtCloudflare"], true);
    assert_eq!(after_stop.body["httpsOrigin"], ORIGIN);
    assert!(credential.is_file(), "stopping must not remove the credential");
    assert!(configuration.is_file(), "stopping must not remove the configuration");
    assert_eq!(fake.invocations("delete"), 0);
    let started = server
        .post_json("/api/access/cloudflare/connector/start", &json!({}))
        .await;
    assert_eq!(started.status, 200, "{}", started.text);
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;

    // The connector starts with its owner, and the ownership that authorizes a
    // deletion survives with it — which is the half of ticket 07's authority
    // that has to outlive a process.
    server.stop().await;
    let restarted =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    let restored = wait_for_connector(&restarted, |body| body["connectorState"] == "ready").await;
    assert_eq!(restored["tunnelOwnership"], "laplus-created");
    assert_eq!(restored["deletableAtCloudflare"], true);
    assert_eq!(restored["httpsOrigin"], ORIGIN);
    let resumed = restarted.get("/api/access/cloudflare/account").await;
    assert_eq!(resumed.body["step"], "creating");
    assert_eq!(resumed.body["selection"]["created"], true);
    restarted.stop().await;

    // **The narrow credential is the whole of steady state.** Account
    // authorization is not merely unused from here on — it is gone, and the
    // connector still comes back with its owner and still verifies. ADR-0045's
    // reason for never retaining the account certificate is only true if this
    // is.
    std::fs::remove_file(&fake.certificate).unwrap();
    let narrow =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    let running = wait_for_connector(&narrow, |body| body["connectorState"] == "ready").await;
    assert_eq!(running["tunnelOwnership"], "laplus-created");
    assert_eq!(running["readiness"], true);
    let reverified =
        wait_for_connector(&narrow, |body| body["verificationState"] == "verified").await;
    assert_eq!(reverified["httpsOrigin"], ORIGIN);
    // And account management really is unavailable, rather than merely unasked
    // for: a listing needs the certificate that is no longer there, and the
    // wizard says so rather than pretending the account is still authorized.
    let account = narrow.get("/api/access/cloudflare/account").await;
    assert_eq!(account.body["certificateDetected"], false);
    assert_eq!(account.body["step"], "sign-in");
    let listed = narrow
        .post_json(
            "/api/access/cloudflare/account/tunnels",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(listed.status, 409, "{}", listed.text);
    assert_eq!(listed.body["reason"], "sign-in-required");

    narrow.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// A creation interrupted at each of its three boundaries.
///
/// **This is the heart of the ticket.** Every resume reconciles against what is
/// observably there — the credential file names the tunnel Cloudflare allocated,
/// the journal is the only record a DNS route left, and the connector's own
/// configuration is the third — so a retry allocates nothing twice and every
/// refusal names both halves without claiming a rollback nothing performed.
#[tokio::test]
async fn a_creation_interrupted_at_any_boundary_resumes_without_duplicating_a_resource() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    consent(&server).await;

    // --- boundary one: the allocation itself refuses. Nothing exists. ---
    fake.rehearse("create-fails");
    let refused = create(&server, &fake).await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert_eq!(refused.body["_tag"], "EnvironmentPublicExposureRejectedError");
    assert_eq!(refused.body["reason"], "command-failed");
    assert_eq!(refused.body["completed"], json!([]));
    assert_eq!(
        refused.body["remaining"],
        json!(["tunnel-create", "dns-route", "configuration"])
    );
    let credential = directory.path().join("cloudflare").join("tunnel.json");
    assert!(!credential.exists(), "a refused allocation left a credential behind");
    assert_eq!(fake.invocations("route"), 0, "a route was made for a tunnel that does not exist");
    let stalled = server.get("/api/access/cloudflare/account").await;
    assert_eq!(stalled.body["step"], "choose-tunnel");
    assert_eq!(server.get("/api/access/cloudflare").await.body["configured"], false);

    // --- boundary two: the tunnel exists and the DNS route refuses. ---
    fake.rehearse("route-fails");
    let partial = create(&server, &fake).await;
    assert_eq!(partial.status, 400, "{}", partial.text);
    assert_eq!(partial.body["reason"], "command-failed");
    // The allocation happened and the refusal says so, rather than implying a
    // rollback laplus never performed — there is no `tunnel delete` here.
    assert_eq!(partial.body["completed"], json!(["tunnel-create"]));
    assert_eq!(partial.body["remaining"], json!(["dns-route", "configuration"]));
    assert_eq!(fake.invocations("create"), 2);
    assert_eq!(fake.invocations("delete"), 0, "a failed route deleted a tunnel");
    assert!(credential.is_file(), "the allocated tunnel's credential is the record of it");

    // **Across a restart, not merely across two calls.** The journal and the
    // credential are what survive a process ending mid-creation, and they are
    // the only things a resume may rely on — nothing in memory does.
    server.stop().await;
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;

    // And the developer can *see* it. `completed`/`remaining` in the body of the
    // request that failed is enough to retry from the screen you are standing on
    // and nothing at all after a restart — so the account snapshot answers what
    // an unfinished creation left, read from the journal a finished creation
    // clears. Without this a restart showed a wizard offering to create a tunnel
    // that already exists.
    let interrupted = server.get("/api/access/cloudflare/account").await;
    let unfinished = &interrupted.body["unfinishedCreation"];
    assert!(!unfinished.is_null(), "{}", interrupted.text);
    assert_eq!(unfinished["completed"], json!(["tunnel-create"]));
    assert_eq!(unfinished["remaining"], json!(["dns-route", "configuration"]));
    // The UUID that exists at Cloudflare, and the name it was asked for — which
    // survives on the first attempt's entry, because the entry of the attempt
    // that *succeeded* was settled with the id a cleanup has to target.
    assert_eq!(unfinished["tunnelId"], CREATED_TUNNEL_ID);
    assert_eq!(unfinished["name"], NAME);
    // Nothing at Cloudflare has a hostname yet: the route is the step that
    // failed, so there is no record of one to show.
    assert_eq!(unfinished["hostname"], Value::Null);

    // --- boundary three: the route succeeds and the configuration will not write. ---
    let configuration = directory.path().join("cloudflare").join("connector.yml");
    std::fs::create_dir_all(&configuration).unwrap();
    fake.behave();
    let third = create(&server, &fake).await;
    assert_eq!(third.status, 400, "{}", third.text);
    assert_eq!(third.body["reason"], "local-setup-failed");
    assert_eq!(third.body["completed"], json!(["tunnel-create", "dns-route"]));
    assert_eq!(third.body["remaining"], json!(["configuration"]));
    // The tunnel the second attempt allocated was reused, not allocated again.
    assert_eq!(fake.invocations("create"), 2, "a resume allocated a second tunnel");
    assert_eq!(fake.invocations("route"), 2);
    let waiting = server.get("/api/access/cloudflare/account").await;
    assert_eq!(waiting.body["step"], "choose-tunnel");

    // --- and the resume that finishes it, after another restart ---
    server.stop().await;
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    std::fs::remove_dir(&configuration).unwrap();
    let finished = create(&server, &fake).await;
    assert_eq!(finished.status, 200, "{}", finished.text);
    assert_eq!(finished.body["step"], "creating");
    assert_eq!(finished.body["selection"]["tunnelId"], CREATED_TUNNEL_ID);
    // One allocation and two routes across four attempts and three restarts:
    // the allocation is observable and was reconciled, the route is journaled
    // and the attempt that failed left nothing to reconcile against.
    assert_eq!(fake.invocations("create"), 2);
    assert_eq!(fake.invocations("route"), 2);
    // Nothing is outstanding any more, so the residue a resume reads is gone.
    assert_eq!(
        server.get("/api/access/cloudflare/account").await.body["unfinishedCreation"],
        Value::Null
    );

    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "laplus-created");
    assert_eq!(endpoint.body["deletableAtCloudflare"], true);
    let ready = wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(ready["tunnelOwnership"], "laplus-created");

    // **A repeated confirmation is a read.** Everything is recorded, so asking
    // again neither allocates nor routes anything.
    let again = create(&server, &fake).await;
    assert_eq!(again.status, 200, "{}", again.text);
    assert_eq!(again.body["step"], "creating");
    assert_eq!(fake.invocations("create"), 2);
    assert_eq!(fake.invocations("route"), 2);

    nothing_leaks(
        &[&refused.text, &partial.text, &third.text, &finished.text, &ready.to_string()],
        &fake.trace,
        &directory.path().join("state.sqlite"),
    );
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// Creation is an administrative mutation whose answers a client may not
/// re-describe, and whose inputs it may not make up.
#[tokio::test]
async fn creation_requires_write_validates_its_inputs_and_owns_the_result() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;

    // ADR-0047: a session without the scope learns which scope, and nothing
    // about the Cloudflare state behind the refusal.
    let reader = client_with(&server, &["access:read"]).await;
    let refused = server
        .post_json_as(
            "/api/access/cloudflare/account/create",
            &reader,
            &json!({"executablePath": fake.executable, "name": NAME, "hostname": HOSTNAME}),
        )
        .await;
    assert_eq!(refused.status, 403, "{}", refused.text);
    assert_eq!(refused.body["_tag"], "EnvironmentScopeRequiredError");
    assert_eq!(refused.body["requiredScope"], "access:write");
    assert!(refused.body.get("reason").is_none());
    assert_eq!(fake.invocations("create"), 0);

    // Consent is the gate on the account certificate, and it is asked before
    // anything is validated against Cloudflare.
    let unconsented = create(&server, &fake).await;
    assert_eq!(unconsented.status, 409, "{}", unconsented.text);
    assert_eq!(unconsented.body["reason"], "consent-required");
    assert_eq!(fake.invocations("create"), 0);
    consent(&server).await;

    // Both inputs are validated before a mutation, and each is its own reason.
    for (name, hostname, reason) in [
        (NAME, "http://stable.example.com", "hostname-invalid"),
        (NAME, "127.0.0.1", "hostname-invalid"),
        ("", HOSTNAME, "tunnel-name-invalid"),
        ("a tunnel/with slashes", HOSTNAME, "tunnel-name-invalid"),
    ] {
        let rejected = server
            .post_json(
                "/api/access/cloudflare/account/create",
                &json!({"executablePath": fake.executable, "name": name, "hostname": hostname}),
            )
            .await;
        assert_eq!(rejected.status, 400, "{name}/{hostname}: {}", rejected.text);
        assert_eq!(rejected.body["reason"], reason, "{name}/{hostname}");
        assert_eq!(rejected.body["completed"], json!([]));
    }
    assert_eq!(fake.invocations("create"), 0, "a rejected request still mutated Cloudflare");

    let created = create(&server, &fake).await;
    assert_eq!(created.status, 200, "{}", created.text);

    // Every route that writes the endpoint row refuses to re-describe a
    // laplus-created tunnel as somebody else's. A hidden button stops a person;
    // these stop a stale tab, a script, and a repeated or forged request — which
    // is what keeps "laplus deletes only what it created" an answer about
    // authority rather than about which control a client chose to draw.
    for (path, body) in [
        ("/api/access/cloudflare", json!({"hostname": HOSTNAME})),
        (
            "/api/access/cloudflare/account/select",
            json!({"tunnelId": "11111111-1111-1111-1111-111111111111",
                   "hostname": "elsewhere.example.com"}),
        ),
        (
            "/api/access/cloudflare/connector/configure",
            json!({"hostname": HOSTNAME, "executablePath": fake.executable,
                   "connectorToken": "connector-secret"}),
        ),
    ] {
        let laundered = server.post_json(path, &body).await;
        assert_eq!(laundered.status, 409, "{path}: {}", laundered.text);
        assert_eq!(laundered.body["reason"], "ownership-conflict", "{path}");
        let held = server.get("/api/access/cloudflare").await;
        assert_eq!(held.body["ownership"], "laplus-created", "{path}");
        assert_eq!(held.body["deletableAtCloudflare"], true, "{path}");
        assert_eq!(held.body["httpsOrigin"], ORIGIN, "{path}");
    }

    // A second creation under a different name is refused rather than allowed to
    // strand the first tunnel: this environment has one public endpoint, and the
    // one it has is the only tunnel laplus could later delete.
    let second = server
        .post_json(
            "/api/access/cloudflare/account/create",
            &json!({"executablePath": fake.executable, "name": "another",
                    "hostname": "second.example.com"}),
        )
        .await;
    assert_eq!(second.status, 409, "{}", second.text);
    assert_eq!(second.body["reason"], "ownership-conflict");
    assert_eq!(fake.invocations("create"), 1);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// The long way round to a deletion authority laplus never earned.
///
/// **An adopted tunnel's run credential lives at exactly the path a creation
/// writes one to.** So a creation that treated "there is a credential here" as
/// "this creation already allocated a tunnel" would skip `tunnel create`, route
/// DNS for somebody else's tunnel, and record it as `laplus-created` — handing
/// ticket 07 the authority to delete a tunnel laplus merely borrowed.
///
/// **The state is reached by an adoption that stopped half way**, which is where
/// it will now come from: ticket 07's Forget removes the credential as well as
/// the row, so adopt-forget-create no longer produces it. A dedication
/// interrupted between retrieving the credential and writing laplus's own
/// configuration still does, and it survives a restart — a borrowed tunnel's
/// credential on disk with nothing recording that laplus owns anything.
///
/// ADR-0049 puts ownership in one place precisely so that no request can launder
/// it, and this is that rule pointed at the one route that could earn a deletion
/// rather than lose one.
#[tokio::test]
async fn a_creation_cannot_adopt_an_existing_credential_and_call_the_tunnel_its_own() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    consent(&server).await;

    // Dedicate the inactive tunnel from ticket 05's listing, which leaves its
    // narrow run credential in laplus's private directory.
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
    let adopted = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(adopted.status, 200, "{}", adopted.text);
    assert_eq!(server.get("/api/access/cloudflare").await.body["ownership"], "adopted");

    // Creating is refused outright while the adopted endpoint is recorded.
    let blocked = create(&server, &fake).await;
    assert_eq!(blocked.status, 409, "{}", blocked.text);
    assert_eq!(blocked.body["reason"], "ownership-conflict");

    // **Now the state a creation must still refuse from**: the borrowed tunnel's
    // credential on disk, and nothing recording that laplus owns anything. Forget
    // takes both away since ticket 07, so this is reached the way a real one is —
    // a dedication that fails after retrieving the credential and before writing
    // laplus's own configuration, on an environment that has no endpoint yet.
    let forgotten = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(forgotten.status, 200, "{}", forgotten.text);
    let credential = directory.path().join("cloudflare").join("tunnel.json");
    assert!(!credential.exists(), "forget left a borrowed tunnel's credential behind");

    let configuration = directory.path().join("cloudflare").join("connector.yml");
    std::fs::create_dir_all(&configuration).unwrap();
    let chosen = server
        .post_json(
            "/api/access/cloudflare/account/select",
            &json!({"tunnelId": SPARE, "hostname": "spare.example.com"}),
        )
        .await;
    assert_eq!(chosen.status, 200, "{}", chosen.text);
    let interrupted = server
        .post_json(
            "/api/access/cloudflare/account/adopt",
            &json!({"executablePath": fake.executable}),
        )
        .await;
    assert_eq!(interrupted.status, 400, "{}", interrupted.text);
    assert_eq!(interrupted.body["completed"], json!(["credential"]));
    assert!(credential.is_file(), "the retrieval this refusal reported did not happen");
    assert_eq!(server.get("/api/access/cloudflare").await.body["configured"], false);
    std::fs::remove_dir(&configuration).unwrap();

    let laundered = create(&server, &fake).await;
    assert_eq!(laundered.status, 409, "{}", laundered.text);
    assert_eq!(laundered.body["reason"], "ownership-conflict");
    // Nothing was allocated, nothing was routed, and above all no row now claims
    // that a tunnel laplus only borrowed is one it may delete.
    assert_eq!(fake.invocations("create"), 0);
    assert_eq!(fake.invocations("route"), 0);
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_ne!(endpoint.body["ownership"], "laplus-created");
    assert_eq!(endpoint.body["deletableAtCloudflare"], false);
    let connector = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(connector.body["tunnelOwnership"], "external");
    assert_eq!(connector.body["deletableAtCloudflare"], false);

    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}
