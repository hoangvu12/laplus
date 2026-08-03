//! Stopping, forgetting and deleting a Cloudflare tunnel, through the real
//! routes.
//!
//! **Three verbs that are deliberately not each other.** Stop changes whether a
//! connector runs and nothing else. Forget removes laplus's own local
//! configuration and secrets, after stopping the connector using them, and
//! touches nothing at Cloudflare for any ownership. Delete everywhere removes
//! the tunnel and the DNS record laplus *created* — and only those — and then
//! does what forget does. The whole of ticket 07's acceptance matrix is which of
//! those three a given ownership may reach.
//!
//! **The refusals matter more than the successes here.** An adopted tunnel and
//! an external endpoint may never reach a Cloudflare deletion command "including
//! through repeated, stale, or forged client requests", so most of what is
//! asserted below is that a command did *not* run: `fake.invocations("delete")`
//! staying at zero is the acceptance box, and the fake appends to its trace
//! before it dispatches, so a zero means the server never asked rather than that
//! the fake said no.
#![cfg(unix)]

mod harness;

use harness::cloudflare::{
    client_with, FakeCloudflareApi, FakeCloudflared, VerifiedEndpoint, CERTIFICATE, DNS_API_TOKEN,
    RECORD_ID, TUNNEL_CREDENTIAL_SECRET, ZONE_ID,
};
use harness::TestServer;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Mutex;

/// The inactive tunnel in the fake's default listing, and the one adoption
/// dedicates.
const SPARE: &str = "22222222-2222-2222-2222-222222222222";
/// What `tunnel create` allocates — a UUID, never the name it was asked for.
const CREATED: &str = harness::cloudflare::CREATED_TUNNEL_ID;
/// A hostname inside the zone the fake API lists, because a deletion has to
/// resolve the recorded name to a zone before it can address the record.
const CREATED_HOSTNAME: &str = "stable.example.com";

/// `TUNNEL_ORIGIN_CERT` and `LAPLUS_CLOUDFLARE_API` are both process-wide —
/// cloudflared's own variable and the loopback-only API override — so these run
/// one at a time.
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

/// Consent to the account certificate and list the account's tunnels.
async fn sign_in(server: &TestServer, fake: &FakeCloudflared) {
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
}

/// Everything ticket 06 does, so this ticket has a laplus-created tunnel to
/// take apart.
async fn create_a_tunnel(server: &TestServer, fake: &FakeCloudflared) {
    sign_in(server, fake).await;
    let created = server
        .post_json(
            "/api/access/cloudflare/account/create",
            &json!({"executablePath": fake.executable, "name": "laplus-desk",
                    "hostname": CREATED_HOSTNAME}),
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.text);
    assert_eq!(created.body["step"], "creating");
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "laplus-created");
    assert_eq!(endpoint.body["deletableAtCloudflare"], true);
}

/// Everything ticket 05 does, so this ticket has an adopted tunnel to refuse a
/// deletion for.
async fn adopt_a_tunnel(server: &TestServer, fake: &FakeCloudflared) {
    sign_in(server, fake).await;
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
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "adopted");
    assert_eq!(endpoint.body["deletableAtCloudflare"], false);
}

/// Ask for a destructive confirmation, and answer with the value that spends it.
async fn offered_confirmation(server: &TestServer) -> Value {
    let offered = server
        .post_json("/api/access/cloudflare/account/deletion", &json!({}))
        .await;
    assert_eq!(offered.status, 200, "{}", offered.text);
    offered.body
}

/// Where laplus keeps its own files, all of them inside one private directory.
struct Owned {
    configuration: std::path::PathBuf,
    credential: std::path::PathBuf,
    settings: std::path::PathBuf,
    account: std::path::PathBuf,
}

fn owned_files(directory: &Path) -> Owned {
    let cloudflare = directory.join("cloudflare");
    Owned {
        configuration: cloudflare.join("connector.yml"),
        credential: cloudflare.join("tunnel.json"),
        settings: cloudflare.join("connector.json"),
        account: cloudflare.join("account.json"),
    }
}

/// Ticket 07's checkbox 7, as an assertion rather than as a promise.
///
/// **Cleanup never revokes a Cloudflare account token and never copies,
/// replaces, moves or deletes an account certificate.** The certificate is
/// cloudflared's, is used in place, and every other copy of it on every other
/// machine keeps working — which is why revocation is out of scope rather than
/// merely unimplemented (ADR-0045, and the certificate-lifecycle finding in
/// `research.md`). Checked against the file's *bytes*, because a certificate
/// that was rewritten with the same length would pass an existence check.
fn the_account_certificate_is_untouched(fake: &FakeCloudflared, before: &[u8]) {
    assert!(fake.certificate.is_file(), "the account certificate was removed");
    assert_eq!(
        std::fs::read(&fake.certificate).expect("the certificate reads"),
        before,
        "the account certificate was rewritten"
    );
    let argv = std::fs::read_to_string(&fake.trace).unwrap_or_default();
    // The certificate reaches cloudflared as `--origincert <path>` and never as
    // contents, and nothing ever asks cloudflared to sign in again — a login
    // would write a *second* certificate, which is the closest thing to
    // revocation this CLI has.
    assert!(!argv.contains(CERTIFICATE), "the account certificate was copied into an argument");
    assert_eq!(
        argv.lines().filter(|line| line.contains("\"login\"")).count(),
        0,
        "cleanup started a Cloudflare sign-in"
    );
}

/// Every place the DNS API token could leak, checked at once.
///
/// The same four haystacks ticket 05 hunts a run credential through, because
/// each has caught a different bug in this feature: what the route answered,
/// what a snapshot serialises, what shows in a process listing, and what a
/// durable non-secret record happens to hold. The database is read as *bytes* —
/// a value in a page SQLite has not vacuumed is still on the disk.
fn nothing_leaks(texts: &[&str], trace: &Path, database: &Path) {
    for text in texts {
        for secret in [DNS_API_TOKEN, TUNNEL_CREDENTIAL_SECRET, CERTIFICATE] {
            assert!(!text.contains(secret), "{secret} reached a response or a snapshot");
        }
    }
    let argv = std::fs::read_to_string(trace).unwrap_or_default();
    assert!(!argv.contains(DNS_API_TOKEN), "the DNS API token reached a command line");
    assert!(!argv.contains(TUNNEL_CREDENTIAL_SECRET), "a run credential reached a command line");
    let persisted = std::fs::read(database).expect("a database file to scan");
    for secret in [DNS_API_TOKEN, TUNNEL_CREDENTIAL_SECRET] {
        assert!(
            !persisted
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()),
            "{secret} reached non-secret persistence"
        );
    }
}

/// Stop turns the connector off and changes nothing else at all.
///
/// **The point is the list of things that survive.** ADR-0045 gives every
/// lifecycle action one owner, and stopping is the action that owns the least:
/// the tunnel, the DNS record, the credential, the configuration and the
/// recorded ownership are all still there afterwards, and starting again is one
/// request rather than a setup.
#[tokio::test]
async fn stopping_a_connector_keeps_every_resource_and_stays_restartable() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let owned = owned_files(directory.path());

    let stopped = server
        .post_json("/api/access/cloudflare/connector/stop", &json!({}))
        .await;
    assert_eq!(stopped.status, 200, "{}", stopped.text);
    let settled = wait_for_connector(&server, |body| body["connectorState"] == "stopped").await;
    assert_eq!(settled["desiredState"], "stopped");
    // Still configured, still laplus-created, still able to be started again.
    assert_eq!(settled["configured"], true);
    assert_eq!(settled["tunnelOwnership"], "laplus-created");
    assert!(fake.stopped_gracefully(), "the connector was not asked to stop");

    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "laplus-created");
    assert_eq!(endpoint.body["httpsOrigin"], format!("https://{CREATED_HOSTNAME}"));
    assert_eq!(endpoint.body["cleanup"]["state"], "stopped");
    assert_eq!(endpoint.body["cleanup"]["remaining"], json!([]));
    // Nothing at Cloudflare was asked to change, and nothing on disk went.
    assert_eq!(fake.invocations("delete"), 0);
    assert_eq!(fake.invocations("route"), 1);
    assert!(owned.configuration.is_file() && owned.credential.is_file());
    assert!(owned.settings.is_file());

    let started = server
        .post_json("/api/access/cloudflare/connector/start", &json!({}))
        .await;
    assert_eq!(started.status, 200, "{}", started.text);
    let running = wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    assert_eq!(running["desiredState"], "running");
    assert_eq!(running["tunnelOwnership"], "laplus-created");
    // A restart is not a second allocation, a second route, or a second
    // credential retrieval.
    assert_eq!(fake.invocations("create"), 1);
    assert_eq!(fake.invocations("route"), 1);
    assert_eq!(fake.invocations("token"), 0);
    assert_eq!(server.get("/api/access/cloudflare").await.body["cleanup"]["state"], "intact");
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// Forget stops the connector, removes laplus's own setup, and leaves
/// everything else exactly where it was.
///
/// **Run against an adopted tunnel on purpose.** Ticket 05 left this box
/// explicitly unticked: forget removed the endpoint row and stopped nothing, so
/// forgetting one left a `cloudflared` serving a public hostname nothing
/// recorded and a `tunnel.json` on disk that made every later creation refuse.
/// Both halves of that are what this pins — and an adopted tunnel is also the
/// ownership where "nothing at Cloudflare" has to hold most strictly, because
/// laplus is running a tunnel it has no authority to remove.
#[tokio::test]
async fn forget_stops_the_connector_and_removes_only_what_laplus_owns() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    let certificate_before = std::fs::read(&fake.certificate).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    adopt_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let owned = owned_files(directory.path());
    assert!(owned.configuration.is_file() && owned.credential.is_file());
    // An executable laplus installed for itself lives in the same private
    // directory, and forget must leave it there — see ADR-0052.
    let tool = directory.path().join("cloudflare").join("tools");
    std::fs::create_dir_all(&tool).unwrap();
    std::fs::write(tool.join("cloudflared-2026.7.3"), "an app-managed executable").unwrap();

    let forgotten = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(forgotten.status, 200, "{}", forgotten.text);
    assert_eq!(forgotten.body["configured"], false);
    assert_eq!(forgotten.body["cleanup"]["state"], "forgotten");
    assert_eq!(
        forgotten.body["cleanup"]["completed"],
        json!(["configuration-remove", "credential-remove"])
    );
    assert_eq!(forgotten.body["cleanup"]["remaining"], json!([]));

    // **The connector was stopped rather than orphaned**, which is the half
    // ticket 05 could not build.
    assert_eq!(fake.graceful_stops(), 1, "forget left the connector running");
    let connector = server.get("/api/access/cloudflare/connector").await;
    assert_eq!(connector.body["configured"], false);

    // **What laplus owned is gone.**
    assert!(!owned.configuration.exists(), "laplus's own ingress file survived");
    assert!(!owned.credential.exists(), "the run credential survived");
    assert!(!owned.settings.exists(), "the connector settings survived");
    // **What laplus did not own is untouched.** The tunnel is somebody else's
    // allocation and its DNS route is somebody else's record: no command that
    // could change either was ever run.
    assert_eq!(fake.invocations("delete"), 0, "forget deleted a Cloudflare tunnel");
    assert_eq!(fake.invocations("route"), 0, "forget touched a DNS record");
    the_account_certificate_is_untouched(&fake, &certificate_before);
    assert!(
        tool.join("cloudflared-2026.7.3").is_file(),
        "forget removed an executable"
    );
    // The wizard's own answers survive, minus the tunnel that no longer is one.
    assert!(owned.account.is_file());
    let account = server.get("/api/access/cloudflare/account").await;
    assert_eq!(account.body["step"], "choose-tunnel");
    assert!(account.body["selection"].is_null());
    assert!(!account.body["certificateConsentedAt"].is_null());

    // **The credential that blocked a creation is released.** Ticket 06 refuses
    // to create while another dedicated tunnel's credential is on disk, and
    // today's row-only forget was a dead end from that path too.
    let created = server
        .post_json(
            "/api/access/cloudflare/account/create",
            &json!({"executablePath": fake.executable, "name": "laplus-desk",
                    "hostname": CREATED_HOSTNAME}),
        )
        .await;
    assert_eq!(created.status, 200, "{}", created.text);
    let endpoint = server.get("/api/access/cloudflare").await;
    assert_eq!(endpoint.body["ownership"], "laplus-created");
    // A live setup is not a report about a removal that happened before it.
    assert_eq!(endpoint.body["cleanup"]["state"], "intact");
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// Delete everywhere removes the exact recorded tunnel and DNS record, and then
/// laplus's own setup.
#[tokio::test]
async fn deleting_everywhere_targets_the_exact_recorded_resources_and_nothing_else() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    let certificate_before = std::fs::read(&fake.certificate).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let api = FakeCloudflareApi::start(CREATED_HOSTNAME).await;
    std::env::set_var("LAPLUS_CLOUDFLARE_API", &api.origin);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let owned = owned_files(directory.path());

    // **The confirmation names what will go, and the names are the row's.**
    let offer = offered_confirmation(&server).await;
    assert_eq!(offer["tunnelId"], CREATED);
    assert_eq!(offer["tunnelName"], "laplus-desk");
    assert_eq!(offer["dnsRecordName"], CREATED_HOSTNAME);
    assert_eq!(offer["httpsOrigin"], format!("https://{CREATED_HOSTNAME}"));
    assert_eq!(
        offer["steps"],
        json!(["dns-record-delete", "tunnel-delete", "configuration-remove", "credential-remove"])
    );

    let deleted = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(deleted.status, 200, "{}", deleted.text);
    assert_eq!(deleted.body["configured"], false);
    assert_eq!(deleted.body["cleanup"]["state"], "fully-removed");
    assert_eq!(deleted.body["cleanup"]["remaining"], json!([]));
    assert_eq!(deleted.body["cleanup"]["tunnelId"], CREATED);
    assert_eq!(deleted.body["cleanup"]["dnsRecordName"], CREATED_HOSTNAME);

    // **The exact record, addressed rather than named.** The row carried a name
    // and no identifiers, because `route dns` reports none (ADR-0051), so the
    // deletion resolved it through the zone the token can see and then deleted
    // that one record.
    assert!(api.records().is_empty(), "the recorded DNS record survived");
    assert!(api.requests().contains(&(
        "DELETE".to_string(),
        format!("/client/v4/zones/{ZONE_ID}/dns_records/{RECORD_ID}")
    )));
    assert_eq!(api.calls("DELETE"), 1, "more than the recorded record was deleted");

    // **The tunnel, by the UUID Cloudflare allocated rather than by the name it
    // was asked for.**
    assert_eq!(fake.invocations("delete"), 1);
    let argv = std::fs::read_to_string(&fake.trace).unwrap();
    assert!(argv.contains(CREATED), "the deletion did not target the allocated tunnel");
    // The connector was asked to stop and had the chance to answer, before the
    // tunnel it was serving was deleted — `tunnel delete` refuses outright while
    // a tunnel still has connections.
    assert_eq!(fake.graceful_stops(), 1, "the connector was still running at deletion");

    // **And then what forget does.**
    assert!(!owned.configuration.exists() && !owned.credential.exists());
    assert!(!owned.settings.exists());
    the_account_certificate_is_untouched(&fake, &certificate_before);
    let account = server.get("/api/access/cloudflare/account").await;
    assert!(account.body["selection"].is_null());
    assert!(account.body["unfinishedCreation"].is_null());

    nothing_leaks(
        &[&deleted.text, &offer.to_string(), &account.text],
        &fake.trace,
        &directory.path().join("state.sqlite"),
    );
    server.stop().await;
    api.stop();
    std::env::remove_var("LAPLUS_CLOUDFLARE_API");
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// An adopted tunnel and an external endpoint can never reach a deletion
/// command — including through a repeated, stale or forged request.
///
/// **This is the ticket's security core, and a hidden button is not an
/// implementation of it.** The only thing that authorizes a deletion is the
/// ownership persisted on the endpoint row, read at the moment the command runs.
/// So the interesting case is the last one: a confirmation minted while the row
/// said `laplus-created`, replayed after the row has become something else.
#[tokio::test]
async fn an_adopted_or_external_tunnel_can_never_reach_a_deletion_command() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    let certificate_before = std::fs::read(&fake.certificate).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let api = FakeCloudflareApi::start(CREATED_HOSTNAME).await;
    std::env::set_var("LAPLUS_CLOUDFLARE_API", &api.origin);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;

    // A hostname somebody else's connector serves. Nothing here is laplus's.
    let registered = server
        .post_json("/api/access/cloudflare", &json!({"hostname": "elsewhere.example.com"}))
        .await;
    assert_eq!(registered.status, 200, "{}", registered.text);
    for path in [
        "/api/access/cloudflare/account/deletion",
        "/api/access/cloudflare/account/delete",
    ] {
        let refused = server
            .post_json(
                path,
                &json!({"executablePath": fake.executable, "confirmation": "forged",
                        "dnsApiToken": DNS_API_TOKEN}),
            )
            .await;
        assert_eq!(refused.status, 409, "{path}: {}", refused.text);
        assert_eq!(refused.body["reason"], "not-laplus-created", "{path}");
    }
    assert_eq!(server.get("/api/access/cloudflare").await.body["ownership"], "external");

    // **The laundering route, walked forwards.** A laplus-created tunnel is made,
    // a confirmation for it is taken, and then the setup is forgotten and an
    // adopted tunnel is dedicated in its place. The confirmation now names a
    // tunnel this environment no longer records, and the row it does record
    // authorizes nothing.
    server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let stale = offered_confirmation(&server).await;
    assert_eq!(stale["tunnelId"], CREATED);

    let forgotten = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(forgotten.status, 200, "{}", forgotten.text);
    adopt_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;

    // Both the offer and the command refuse, from the same recorded value, and
    // the deletion refuses whether the confirmation is replayed or forged.
    let refused_offer = server
        .post_json("/api/access/cloudflare/account/deletion", &json!({}))
        .await;
    assert_eq!(refused_offer.status, 409, "{}", refused_offer.text);
    assert_eq!(refused_offer.body["reason"], "not-laplus-created");
    for confirmation in [stale["confirmation"].as_str().unwrap(), "forged", ""] {
        let replayed = server
            .post_json(
                "/api/access/cloudflare/account/delete",
                &json!({"executablePath": fake.executable, "confirmation": confirmation,
                        "dnsApiToken": DNS_API_TOKEN}),
            )
            .await;
        assert_eq!(replayed.status, 409, "{}", replayed.text);
        assert_eq!(replayed.body["reason"], "not-laplus-created");
    }

    // Nothing was removed anywhere, by any of them.
    assert_eq!(fake.invocations("delete"), 0, "a tunnel deletion was attempted");
    assert_eq!(api.calls("DELETE"), 0, "a DNS record deletion was attempted");
    assert_eq!(api.records().len(), 1);
    the_account_certificate_is_untouched(&fake, &certificate_before);
    let held = server.get("/api/access/cloudflare").await;
    assert_eq!(held.body["ownership"], "adopted");
    assert_eq!(held.body["deletableAtCloudflare"], false);

    // ADR-0047: a session without the scope learns which scope, and nothing
    // about the Cloudflare state behind the refusal.
    let reader = client_with(&server, &["access:read"]).await;
    for path in [
        "/api/access/cloudflare/account/deletion",
        "/api/access/cloudflare/account/delete",
        "/api/access/cloudflare/forget",
    ] {
        let refused = server
            .post_json_as(
                path,
                &reader,
                &json!({"executablePath": fake.executable, "confirmation": "anything",
                        "dnsApiToken": DNS_API_TOKEN}),
            )
            .await;
        assert_eq!(refused.status, 403, "{path}: {}", refused.text);
        assert_eq!(refused.body["_tag"], "EnvironmentScopeRequiredError", "{path}");
        assert_eq!(refused.body["requiredScope"], "access:write", "{path}");
        assert!(refused.body.get("reason").is_none(), "{path}");
    }
    server.stop().await;
    api.stop();
    std::env::remove_var("LAPLUS_CLOUDFLARE_API");
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// A deletion is authorized by a confirmation for the exact recorded resources,
/// spent once.
///
/// **What "fresh `access:write`" means here.** A session scope answers who may
/// ask; it cannot answer what they were shown. So the destructive path needs a
/// value this server minted for these resources, and a second use of it is a
/// repeat rather than a second authorization — which is the whole of what stops
/// a replayed request. ADR-0052.
#[tokio::test]
async fn a_deletion_needs_a_confirmation_this_server_minted_and_spends_it_once() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let api = FakeCloudflareApi::start(CREATED_HOSTNAME).await;
    std::env::set_var("LAPLUS_CLOUDFLARE_API", &api.origin);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;

    // No confirmation at all, and one this server never minted.
    for confirmation in ["", "forged-confirmation"] {
        let refused = server
            .post_json(
                "/api/access/cloudflare/account/delete",
                &json!({"executablePath": fake.executable, "confirmation": confirmation,
                        "dnsApiToken": DNS_API_TOKEN}),
            )
            .await;
        assert_eq!(refused.status, 409, "{}", refused.text);
        assert_eq!(refused.body["reason"], "confirmation-required");
    }
    assert_eq!(fake.invocations("delete"), 0);
    assert_eq!(api.calls("DELETE"), 0);

    // **A restart is a re-confirmation.** The offer lives in this server's
    // memory rather than in its database, so an offer a developer left on screen
    // yesterday is not standing authority over a tunnel today.
    let across_a_restart = offered_confirmation(&server).await;
    server.stop().await;
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    let after_restart = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable,
                    "confirmation": across_a_restart["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(after_restart.status, 409, "{}", after_restart.text);
    assert_eq!(after_restart.body["reason"], "confirmation-required");
    assert_eq!(fake.invocations("delete"), 0);

    // A fresh one works, and works exactly once.
    let offer = offered_confirmation(&server).await;
    let deleted = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(deleted.status, 200, "{}", deleted.text);
    assert_eq!(deleted.body["cleanup"]["state"], "fully-removed");

    // The repeat finds it spent, and finds nothing recorded to delete either —
    // both refusals hold, and the earlier one is the one that answers.
    let repeated = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(repeated.status, 409, "{}", repeated.text);
    assert_eq!(repeated.body["reason"], "not-laplus-created");
    assert_eq!(fake.invocations("delete"), 1, "a repeated request deleted a second time");
    assert_eq!(api.calls("DELETE"), 1);
    server.stop().await;
    api.stop();
    std::env::remove_var("LAPLUS_CLOUDFLARE_API");
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// Missing Cloudflare DNS authority leaves a recoverable state rather than a
/// weaker deletion.
///
/// **The temptation this refuses.** `cloudflared tunnel delete` needs only the
/// account certificate, so a deletion could always remove the tunnel and report
/// success while quietly leaving a CNAME pointing at nothing. That is the
/// "weakened operation" the acceptance box forbids: the developer asked for the
/// resources laplus created to be gone, and a hostname that now answers with a
/// Cloudflare error page is not that.
///
/// So a missing token, and a token that can see no zone containing the recorded
/// name, both refuse before anything is attempted — which is the most
/// recoverable state there is, because nothing happened.
#[tokio::test]
async fn missing_dns_authority_refuses_before_anything_is_deleted() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    // A token whose zones do not contain this hostname: valid authority over
    // somebody else's zone is no authority over this record.
    let api =
        FakeCloudflareApi::start_with(vec![json!({"id": "zone-other", "name": "elsewhere.test"})],
                                      CREATED_HOSTNAME)
            .await;
    std::env::set_var("LAPLUS_CLOUDFLARE_API", &api.origin);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let owned = owned_files(directory.path());

    for (why, token) in [
        ("no token at all", ""),
        ("a token Cloudflare rejects", "not-the-right-token"),
        ("a token that can see no zone for this name", DNS_API_TOKEN),
    ] {
        let offer = offered_confirmation(&server).await;
        let refused = server
            .post_json(
                "/api/access/cloudflare/account/delete",
                &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                        "dnsApiToken": token}),
            )
            .await;
        assert_eq!(refused.status, 409, "{why}: {}", refused.text);
        assert_eq!(refused.body["reason"], "dns-authority-required", "{why}");
        // Nothing done, everything outstanding — and never a claim that some of
        // it was rolled back.
        assert_eq!(refused.body["completed"], json!([]), "{why}");
        assert_eq!(
            refused.body["remaining"],
            json!([
                "dns-record-delete",
                "tunnel-delete",
                "configuration-remove",
                "credential-remove"
            ]),
            "{why}"
        );
        assert!(!refused.text.contains(DNS_API_TOKEN), "{why}: the token was quoted back");
    }

    // **Nothing was weakened.** The tunnel is still there, the record is still
    // there, laplus's own setup is still there, and the endpoint is still the
    // one it was.
    assert_eq!(fake.invocations("delete"), 0);
    assert_eq!(api.calls("DELETE"), 0);
    assert_eq!(api.records().len(), 1);
    assert!(owned.configuration.is_file() && owned.credential.is_file());
    let held = server.get("/api/access/cloudflare").await;
    assert_eq!(held.body["ownership"], "laplus-created");
    assert_eq!(held.body["httpsOrigin"], format!("https://{CREATED_HOSTNAME}"));
    // No residue, because no step was ever begun.
    assert_eq!(held.body["cleanup"]["state"], "stopped");
    assert_eq!(held.body["cleanup"]["remaining"], json!([]));
    server.stop().await;
    api.stop();
    std::env::remove_var("LAPLUS_CLOUDFLARE_API");
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// A deletion that fails half way preserves the exact remaining work, across a
/// restart, and finishes without repeating what it already did.
///
/// **Two resources at two places, and either can be the last.** The DNS record
/// goes first, and `cloudflared tunnel delete` then refuses — which leaves a real
/// half-deleted world: a tunnel that still exists and a hostname that no longer
/// routes to it. The refusal has to say exactly that rather than imply a
/// rollback, the state has to survive a restart, and the retry has to read
/// Cloudflare's own `81044` for the already-deleted record as work already done.
#[tokio::test]
async fn a_partial_deletion_preserves_its_remaining_work_and_retries_idempotently() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let api = FakeCloudflareApi::start(CREATED_HOSTNAME).await;
    std::env::set_var("LAPLUS_CLOUDFLARE_API", &api.origin);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let owned = owned_files(directory.path());

    fake.rehearse("delete-fails");
    let offer = offered_confirmation(&server).await;
    let partial = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(partial.status, 400, "{}", partial.text);
    assert_eq!(partial.body["reason"], "command-failed");
    assert_eq!(partial.body["completed"], json!(["dns-record-delete"]));
    assert_eq!(
        partial.body["remaining"],
        json!(["tunnel-delete", "configuration-remove", "credential-remove"])
    );
    // The record really is gone, and laplus's own setup really is still there —
    // the refusal claims neither more nor less than that.
    assert!(api.records().is_empty());
    assert!(owned.configuration.is_file() && owned.credential.is_file());

    // The zone is asked for by name, one suffix at a time, longest first — so a
    // record in `stable.example.com` costs a lookup for that and one for
    // `example.com`, and never a paged listing that a >50-zone account would
    // fall off the end of.
    let zone_lookups =
        || api.requests().iter().filter(|(_, path)| path == "/client/v4/zones").count();
    assert_eq!(zone_lookups(), 2);
    let looked_up = zone_lookups();

    // **Across a restart**, because the journal is what survives a process
    // ending mid-cleanup and nothing in memory does.
    server.stop().await;
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    let interrupted = server.get("/api/access/cloudflare").await;
    assert_eq!(interrupted.body["cleanup"]["state"], "partially-deleted");
    assert_eq!(interrupted.body["cleanup"]["completed"], json!(["dns-record-delete"]));
    assert_eq!(
        interrupted.body["cleanup"]["remaining"],
        json!(["tunnel-delete", "configuration-remove", "credential-remove"])
    );
    assert_eq!(interrupted.body["cleanup"]["tunnelId"], CREATED);
    assert_eq!(interrupted.body["cleanup"]["dnsRecordName"], CREATED_HOSTNAME);
    // **And it is no longer advertised.** Verification still says `verified` —
    // that is a fact about the last attempt — but the hostname's DNS record has
    // been deleted, so offering it for pairing would be offering a hostname that
    // no longer resolves.
    assert_eq!(interrupted.body["verificationState"], "verified");
    assert!(interrupted.body["advertisedEndpoint"].is_null());

    fake.behave();
    let offer = offered_confirmation(&server).await;
    let finished = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(finished.status, 200, "{}", finished.text);
    assert_eq!(finished.body["cleanup"]["state"], "fully-removed");
    assert_eq!(finished.body["configured"], false);

    // **The retry repeated nothing.** One DNS deletion across both attempts,
    // because the completed step was skipped; two tunnel deletions, because the
    // first was refused and the second is the one that worked. And no second
    // resolution either — the identifiers the first attempt looked up were
    // written back onto the row (ADR-0051).
    assert_eq!(api.calls("DELETE"), 1, "the retry deleted the DNS record again");
    assert_eq!(zone_lookups(), looked_up, "the retry resolved the record a second time");
    assert_eq!(fake.invocations("delete"), 2);
    assert!(!owned.configuration.exists() && !owned.credential.exists());
    server.stop().await;
    api.stop();
    std::env::remove_var("LAPLUS_CLOUDFLARE_API");
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// A forget interrupted between its two removals says so, and finishing it is a
/// repeat.
///
/// The local twin of the partial deletion above. Nothing at Cloudflare is
/// involved at all, which is why its outstanding state is `cleanup-required`
/// rather than `partially-deleted`: what is left to do is on this machine.
#[tokio::test]
async fn a_forget_that_stopped_half_way_reports_cleanup_required_and_finishes_on_a_repeat() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let owned = owned_files(directory.path());

    // A credential that will not go: a directory cannot be removed as a file,
    // which is the same shape as the failure a locked or read-only file gives.
    std::fs::remove_file(&owned.credential).unwrap();
    std::fs::create_dir(&owned.credential).unwrap();
    let refused = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert_eq!(refused.body["reason"], "local-setup-failed");
    assert_eq!(refused.body["completed"], json!(["configuration-remove"]));
    assert_eq!(refused.body["remaining"], json!(["credential-remove"]));

    // The row survives a forget that could not finish, because it is still the
    // record of an exposure some of whose setup is still here.
    server.stop().await;
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    let interrupted = server.get("/api/access/cloudflare").await;
    assert_eq!(interrupted.body["cleanup"]["state"], "cleanup-required");
    assert_eq!(interrupted.body["cleanup"]["completed"], json!(["configuration-remove"]));
    assert_eq!(interrupted.body["cleanup"]["remaining"], json!(["credential-remove"]));
    assert!(interrupted.body["advertisedEndpoint"].is_null());

    std::fs::remove_dir(&owned.credential).unwrap();
    let finished = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(finished.status, 200, "{}", finished.text);
    assert_eq!(finished.body["cleanup"]["state"], "forgotten");
    assert_eq!(finished.body["configured"], false);
    assert_eq!(fake.invocations("delete"), 0);
    server.stop().await;
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}

/// A cleanup, a fresh setup, and a second cleanup — which must actually remove
/// the second setup.
///
/// **The defect this exists for reported success having removed nothing.** A
/// cleanup journal outlives the endpoint it was about on purpose, because that
/// residue is how an unfinished one survives a restart. But a `completed` entry
/// stays completed however the world moves on, so a forget, a fresh creation and
/// a second forget found both steps already done, skipped them, deleted the row
/// and answered `200` with `forgotten` — while the new `tunnel.json` and
/// configuration sat on disk, and every later creation was refused with
/// `ownership-conflict` for a credential nothing would release.
///
/// The delete-everywhere twin is worse: all four steps read as done, so no
/// `cloudflared tunnel delete` and no DNS call ran at all, and the answer said
/// `fully-removed` about a tunnel and a CNAME that were both still there.
///
/// Registering an endpoint is what clears the residue now — it is the one moment
/// that means "there is something set up here again".
#[tokio::test]
async fn a_second_cleanup_after_a_fresh_setup_removes_the_second_setup() {
    let _serial = serially();
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeCloudflared::write_into(directory.path());
    std::fs::write(&fake.certificate, CERTIFICATE).unwrap();
    std::env::set_var("TUNNEL_ORIGIN_CERT", &fake.certificate);
    let api = FakeCloudflareApi::start(CREATED_HOSTNAME).await;
    std::env::set_var("LAPLUS_CLOUDFLARE_API", &api.origin);
    let server =
        TestServer::start_persistent_in(directory.path(), std::sync::Arc::new(VerifiedEndpoint))
            .await;
    let owned = owned_files(directory.path());

    // Set up, forget, set up again — and the report must describe the setup that
    // exists rather than the removal that preceded it.
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let forgotten = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(forgotten.body["cleanup"]["state"], "forgotten");
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    assert!(owned.configuration.is_file() && owned.credential.is_file());
    assert_eq!(
        server.get("/api/access/cloudflare").await.body["cleanup"]["state"],
        "intact",
        "a live setup was described by the removal before it"
    );

    // **The second forget removes the second setup.**
    let again = server
        .post_json("/api/access/cloudflare/forget", &json!({}))
        .await;
    assert_eq!(again.status, 200, "{}", again.text);
    assert_eq!(again.body["cleanup"]["state"], "forgotten");
    assert!(!owned.configuration.exists(), "the second forget removed nothing");
    assert!(!owned.credential.exists(), "the second forget left the credential behind");

    // **And the second deletion really deletes.**
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let offer = offered_confirmation(&server).await;
    let deleted = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(deleted.status, 200, "{}", deleted.text);
    assert_eq!(deleted.body["cleanup"]["state"], "fully-removed");
    assert_eq!(api.calls("DELETE"), 1, "the DNS record was not actually deleted");
    assert!(api.records().is_empty());
    assert_eq!(fake.invocations("delete"), 1, "the tunnel was not actually deleted");
    assert!(!owned.configuration.exists() && !owned.credential.exists());
    let zone_lookups =
        || api.requests().iter().filter(|(_, path)| path == "/client/v4/zones").count();
    let looked_up = zone_lookups();

    // Set up once more and delete again: still one real deletion per setup.
    create_a_tunnel(&server, &fake).await;
    wait_for_connector(&server, |body| body["connectorState"] == "ready").await;
    let offer = offered_confirmation(&server).await;
    let deleted_again = server
        .post_json(
            "/api/access/cloudflare/account/delete",
            &json!({"executablePath": fake.executable, "confirmation": offer["confirmation"],
                    "dnsApiToken": DNS_API_TOKEN}),
        )
        .await;
    assert_eq!(deleted_again.status, 200, "{}", deleted_again.text);
    assert_eq!(deleted_again.body["cleanup"]["state"], "fully-removed");
    assert_eq!(fake.invocations("delete"), 2, "the second deletion deleted nothing");
    // **It looked, and there was nothing there.** The stand-in `cloudflared`'s
    // `route dns` and the fake DNS API are two fixtures, so the second creation
    // routed a name the API never got a record for — and a name that resolves to
    // no record is that step having already happened rather than a failure,
    // which is the same reading Cloudflare's own `81044` gets. What matters is
    // that the deletion went and asked rather than assuming from an old log.
    assert!(zone_lookups() > looked_up, "the second deletion never looked for the record");
    assert_eq!(api.calls("DELETE"), 1);
    server.stop().await;
    api.stop();
    std::env::remove_var("LAPLUS_CLOUDFLARE_API");
    std::env::remove_var("TUNNEL_ORIGIN_CERT");
}
