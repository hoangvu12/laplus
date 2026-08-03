mod harness;

use harness::cloudflare::client_with;
use harness::TestServer;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;
use futures_util::StreamExt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[derive(Debug)]
struct ScriptedVerifier {
    results: Mutex<VecDeque<Result<(), (&'static str, &'static str)>>>,
    credentials: Mutex<Vec<(String, String)>>,
}

impl ScriptedVerifier {
    fn new(results: impl IntoIterator<Item = Result<(), (&'static str, &'static str)>>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().collect()),
            credentials: Mutex::new(Vec::new()),
        }
    }
}

impl laplus_server::public_exposure::EndpointVerifier for ScriptedVerifier {
    fn verify<'a>(
        &'a self,
        _origin: &'a str,
        _environment_id: &'a str,
        http_token: &'a str,
        ws_token: &'a str,
    ) -> laplus_server::public_exposure::VerificationFuture<'a> {
        self.credentials
            .lock()
            .unwrap()
            .push((http_token.to_string(), ws_token.to_string()));
        let result = self.results.lock().unwrap().pop_front().expect("a scripted result");
        Box::pin(async move {
            result.map_err(|(kind, message)| {
                laplus_server::public_exposure::VerificationFailure { kind, message }
            })
        })
    }
}

#[derive(Debug, Default)]
struct BlockingVerifier {
    calls: AtomicUsize,
    release: Notify,
}

#[derive(Debug, Default)]
struct LoopbackChallengeVerifier {
    base_url: Mutex<Option<String>>,
}

impl laplus_server::public_exposure::EndpointVerifier for LoopbackChallengeVerifier {
    fn verify<'a>(
        &'a self,
        _origin: &'a str,
        _environment_id: &'a str,
        http_token: &'a str,
        ws_token: &'a str,
    ) -> laplus_server::public_exposure::VerificationFuture<'a> {
        let base_url = self.base_url.lock().unwrap().clone().expect("server URL installed");
        Box::pin(async move {
            let client = reqwest::Client::new();
            let http_url = format!("{base_url}/api/access/cloudflare/challenge");
            let wrong_protocol = client.get(&http_url).bearer_auth(ws_token).send().await.unwrap();
            assert_eq!(wrong_protocol.status(), reqwest::StatusCode::UNAUTHORIZED);
            let first = client.get(&http_url).bearer_auth(http_token).send().await.unwrap();
            assert_eq!(first.status(), reqwest::StatusCode::OK);
            let repeated = client.get(&http_url).bearer_auth(http_token).send().await.unwrap();
            assert_eq!(repeated.status(), reqwest::StatusCode::UNAUTHORIZED);

            let ws_url = format!(
                "{}/api/access/cloudflare/challenge/ws",
                base_url.replacen("http://", "ws://", 1)
            );
            let mut wrong_protocol_request = ws_url.clone().into_client_request().unwrap();
            wrong_protocol_request.headers_mut().insert(
                "Authorization",
                format!("Bearer {http_token}").parse().unwrap(),
            );
            let wrong_protocol =
                tokio_tungstenite::connect_async(wrong_protocol_request).await.unwrap_err();
            assert!(matches!(
                wrong_protocol,
                tokio_tungstenite::tungstenite::Error::Http(ref response)
                    if response.status() == 401
            ));
            let mut request = ws_url.clone().into_client_request().unwrap();
            request.headers_mut().insert(
                "Authorization",
                format!("Bearer {ws_token}").parse().unwrap(),
            );
            let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            assert_eq!(socket.next().await.unwrap().unwrap().into_text().unwrap(), "ok");

            let mut repeated_request = ws_url.into_client_request().unwrap();
            repeated_request.headers_mut().insert(
                "Authorization",
                format!("Bearer {ws_token}").parse().unwrap(),
            );
            let repeated = tokio_tungstenite::connect_async(repeated_request).await.unwrap_err();
            assert!(matches!(
                repeated,
                tokio_tungstenite::tungstenite::Error::Http(ref response)
                    if response.status() == 401
            ));
            Ok(())
        })
    }
}

impl laplus_server::public_exposure::EndpointVerifier for BlockingVerifier {
    fn verify<'a>(
        &'a self,
        _origin: &'a str,
        _environment_id: &'a str,
        _http_token: &'a str,
        _ws_token: &'a str,
    ) -> laplus_server::public_exposure::VerificationFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            self.release.notified().await;
            Ok(())
        })
    }
}

#[tokio::test]
async fn setup_state_requires_read_and_mutations_require_write_without_disclosing_state() {
    let server = TestServer::start().await;
    let ordinary = client_with(&server, &["orchestration:read"]).await;
    let refused = server.get_as("/api/access/cloudflare", &ordinary).await;
    assert_eq!(refused.status, 403);
    assert_eq!(refused.body["requiredScope"], "access:read");
    assert!(refused.body.get("httpsOrigin").is_none());

    let reader = client_with(&server, &["access:read"]).await;
    let refused = server.post_json_as(
        "/api/access/cloudflare", &reader, &json!({"hostname": "laplus.example.com"}),
    ).await;
    assert_eq!(refused.status, 403);
    assert_eq!(refused.body["requiredScope"], "access:write");
    assert!(refused.body.get("verificationState").is_none());
    for path in ["/api/access/cloudflare/test", "/api/access/cloudflare/forget"] {
        let refused = server.post_json_as(path, &reader, &json!({})).await;
        assert_eq!(refused.status, 403);
        assert_eq!(refused.body["requiredScope"], "access:write");
        assert!(refused.body.get("verificationState").is_none());
    }
    server.stop().await;
}

#[tokio::test]
async fn normalized_external_registration_survives_a_restart() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let first = TestServer::start_at(&path).await;
    let registered = first.post_json(
        "/api/access/cloudflare", &json!({"hostname": " EXAMPLE.COM. "}),
    ).await;
    assert_eq!(registered.status, 200, "{}", registered.text);
    assert_eq!(registered.body["httpsOrigin"], "https://example.com");
    assert_eq!(registered.body["ownership"], "external");
    assert_eq!(registered.body["verificationState"], "pending");
    first.stop().await;

    let restarted = TestServer::start_at(&path).await;
    let restored = restarted.get("/api/access/cloudflare").await;
    assert_eq!(restored.body["httpsOrigin"], "https://example.com");
    assert_eq!(restored.body["verificationState"], "pending");
    restarted.stop().await;
}

#[tokio::test]
async fn repeating_registration_keeps_the_verified_endpoint_available() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let database = laplus_server::store::Database::open(&path).unwrap();
    database
        .register_public_exposure_endpoint(laplus_server::store::NewPublicExposure::external(
            "https://laplus.example.com",
        ))
        .unwrap();
    database
        .record_public_exposure_verification("https://laplus.example.com", true, None, None)
        .unwrap();
    drop(database);

    let server = TestServer::start_at(&path).await;
    let repeated = server
        .post_json(
            "/api/access/cloudflare",
            &json!({"hostname": " LAPLUS.EXAMPLE.COM. "}),
        )
        .await;

    assert_eq!(repeated.status, 200, "{}", repeated.text);
    assert_eq!(repeated.body["verificationState"], "verified");
    assert!(repeated.body["lastVerifiedAt"].is_string());
    assert_eq!(repeated.body["advertisedEndpoint"]["status"], "available");
    server.stop().await;
}

#[tokio::test]
async fn registration_rejects_probe_shaped_and_private_destinations() {
    let server = TestServer::start().await;
    for hostname in ["http://example.com", "https://example.com/a", "https://127.0.0.1"] {
        let response = server.post_json("/api/access/cloudflare", &json!({"hostname": hostname})).await;
        assert_eq!(response.status, 400, "{hostname}: {}", response.text);
    }
    server.stop().await;
}

#[tokio::test]
async fn layered_verification_keeps_stale_success_and_never_exposes_diagnostic_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let verifier = Arc::new(ScriptedVerifier::new([
        Ok(()),
        Err(("dns", "DNS lookup failed.")),
        Err(("tls", "TLS negotiation failed.")),
        Err(("identity", "The endpoint was not a laplus environment.")),
        Err(("cloudflare-access", "Cloudflare Access intercepted the descriptor.")),
        Err(("wrong-environment", "The hostname reaches another environment.")),
        Err(("authentication", "The one-time HTTP challenge was refused.")),
        Err(("websocket", "The authenticated WebSocket upgrade failed.")),
    ]));
    let server = TestServer::start_at_with_endpoint_verifier(&path, verifier.clone()).await;
    server
        .post_json(
            "/api/access/cloudflare",
            &json!({"hostname": "laplus.example.com"}),
        )
        .await;

    let verified = server.post_json("/api/access/cloudflare/test", &json!({})).await;
    assert_eq!(verified.body["verificationState"], "verified");
    assert_eq!(verified.body["health"]["https"], "healthy");
    assert_eq!(verified.body["health"]["webSocket"], "healthy");
    assert_eq!(verified.body["advertisedEndpoint"]["status"], "available");
    let last_success = verified.body["lastVerifiedAt"].clone();

    for (kind, https, websocket) in [
        ("dns", "failed", "unknown"),
        ("tls", "failed", "unknown"),
        ("identity", "failed", "unknown"),
        ("cloudflare-access", "failed", "unknown"),
        ("wrong-environment", "failed", "unknown"),
        ("authentication", "failed", "unknown"),
        ("websocket", "healthy", "failed"),
    ] {
        let failed = server.post_json("/api/access/cloudflare/test", &json!({})).await;
        assert_eq!(failed.body["failureKind"], kind);
        assert_eq!(failed.body["health"]["https"], https);
        assert_eq!(failed.body["health"]["webSocket"], websocket);
        assert_eq!(failed.body["lastVerifiedAt"], last_success);
        assert!(failed.body["advertisedEndpoint"].is_null());
    }

    let snapshot = server.get("/api/access/cloudflare").await;
    let persisted = std::fs::read(&path).unwrap();
    for (http_token, ws_token) in verifier.credentials.lock().unwrap().iter() {
        assert_ne!(http_token, ws_token);
        assert!(!snapshot.text.contains(http_token));
        assert!(!snapshot.text.contains(ws_token));
        assert!(!persisted.windows(http_token.len()).any(|window| window == http_token.as_bytes()));
        assert!(!persisted.windows(ws_token.len()).any(|window| window == ws_token.as_bytes()));
    }
    server.stop().await;
}

#[tokio::test]
async fn concurrent_test_now_requests_share_one_bounded_verification() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let verifier = Arc::new(BlockingVerifier::default());
    let server = TestServer::start_at_with_endpoint_verifier(&path, verifier.clone()).await;
    server
        .post_json(
            "/api/access/cloudflare",
            &json!({"hostname": "laplus.example.com"}),
        )
        .await;

    let body = json!({});
    let first = server.post_json("/api/access/cloudflare/test", &body);
    let second = server.post_json("/api/access/cloudflare/test", &body);
    let release = async {
        while verifier.calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        verifier.release.notify_waiters();
    };
    let (first, second, ()) = tokio::join!(first, second, release);

    assert_eq!(verifier.calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.body["verificationState"], "verified");
    assert_eq!(second.body["verificationState"], "verified");
    server.stop().await;
}

#[tokio::test]
async fn diagnostic_http_and_websocket_credentials_are_distinct_and_single_use() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let verifier = Arc::new(LoopbackChallengeVerifier::default());
    let server = TestServer::start_at_with_endpoint_verifier(&path, verifier.clone()).await;
    *verifier.base_url.lock().unwrap() = Some(format!("http://{}", server.addr()));
    server
        .post_json(
            "/api/access/cloudflare",
            &json!({"hostname": "laplus.example.com"}),
        )
        .await;

    let verified = server.post_json("/api/access/cloudflare/test", &json!({})).await;
    assert_eq!(verified.body["verificationState"], "verified");
    server.stop().await;
}

/// Ticket 07's whole acceptance matrix is external / adopted / laplus-created,
/// and none of it was represented anywhere: the snapshot printed
/// `"ownership":"laplus"` and `"remoteOwnership":"cloudflare"` as string
/// literals and the singleton row held origin and verification state only.
///
/// So the server is restarted between every read: what is under test is the
/// column, not a struct that happens to still be in memory. Adoption and
/// creation are tickets 05 and 06, so the rows are recorded through the store
/// the way those routes will record them.
#[tokio::test]
async fn tunnel_ownership_survives_a_restart_and_is_not_the_clients_to_change() {
    use laplus_server::public_exposure::TunnelOwnership;
    use laplus_server::store::{Database, DnsRecord, NewPublicExposure};

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    let record = DnsRecord {
        zone_id: "zone-1".into(),
        record_id: "record-1".into(),
        name: "laplus.example.com".into(),
    };
    let database = Database::open(&path).unwrap();

    for owned in [TunnelOwnership::Adopted, TunnelOwnership::LaplusCreated] {
        database
            .register_public_exposure_endpoint(NewPublicExposure {
                ownership: owned,
                tunnel_id: Some("44444444-4444-4444-4444-444444444444"),
                dns_record: (owned == TunnelOwnership::LaplusCreated).then_some(&record),
                credential_path: Some("/private/tunnel.json"),
                ..NewPublicExposure::external("https://laplus.example.com")
            })
            .unwrap();

        let server = TestServer::start_at(&path).await;
        let read_back = server.get("/api/access/cloudflare").await;
        assert_eq!(read_back.status, 200, "{}", read_back.text);
        assert_eq!(read_back.body["ownership"], owned.as_str(), "{owned} did not survive");
        assert_ne!(read_back.body["ownership"], "external");

        // Ownership is not a field a client may set. Registering a hostname
        // "somebody else operates" over a tunnel laplus owns would launder the
        // record of the resources laplus made and is the only owner of — which
        // is ticket 07's "including through repeated, stale, or forged client
        // requests" pointed the other way.
        let laundered = server
            .post_json(
                "/api/access/cloudflare",
                &json!({"hostname": "somebody-else.example.com"}),
            )
            .await;
        assert_eq!(laundered.status, 409, "{}", laundered.text);
        assert_eq!(laundered.body["_tag"], "EnvironmentPublicExposurePreconditionError");
        assert_eq!(laundered.body["reason"], "ownership-conflict");
        server.stop().await;

        let after = database.public_exposure_endpoint().unwrap().unwrap();
        assert_eq!(after.ownership, owned);
        assert_eq!(after.https_origin, "https://laplus.example.com");
        assert_eq!(after.tunnel_id.as_deref(), Some("44444444-4444-4444-4444-444444444444"));
        assert_eq!(after.credential_path.as_deref(), Some("/private/tunnel.json"));
        // Only the tunnel laplus created carries the DNS record it created,
        // and only that one may ever reach a Cloudflare deletion command.
        assert_eq!(
            after.ownership.deletable_at_cloudflare(),
            owned == TunnelOwnership::LaplusCreated
        );
    }

    // An external endpoint is registrable through the route as it always was,
    // and reads back as the one ownership that authorizes nothing.
    database.forget_public_exposure_endpoint().unwrap();
    let server = TestServer::start_at(&path).await;
    let registered = server
        .post_json(
            "/api/access/cloudflare",
            &json!({"hostname": "operator.example.com"}),
        )
        .await;
    assert_eq!(registered.status, 200, "{}", registered.text);
    assert_eq!(registered.body["ownership"], "external");
    assert_eq!(registered.body["health"]["connector"], "external");
    server.stop().await;

    let restarted = TestServer::start_at(&path).await;
    assert_eq!(restarted.get("/api/access/cloudflare").await.body["ownership"], "external");
    restarted.stop().await;
}

/// Tickets 06 and 07 both require journaled steps that survive restart and
/// resume idempotently: the mutations that already happened, so a retry does
/// not repeat them, and the ones started and never settled, so the wizard can
/// name the remaining work instead of claiming a rollback that did not occur.
#[tokio::test]
async fn a_half_finished_mutation_keeps_its_remaining_work_across_a_restart() {
    use laplus_server::public_exposure::{MutationIntent, MutationState, MutationStep};
    use laplus_server::store::Database;

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.sqlite");
    {
        let database = Database::open(&path).unwrap();
        let credential = database
            .begin_mutation_step(MutationIntent::Create, MutationStep::Credential, None)
            .unwrap();
        database
            .settle_mutation_step(credential, MutationState::Completed, Some("/private/tunnel.json"))
            .unwrap();
        let created = database
            .begin_mutation_step(MutationIntent::Create, MutationStep::TunnelCreate, Some("laplus"))
            .unwrap();
        database
            .settle_mutation_step(created, MutationState::Completed, Some("tunnel-uuid"))
            .unwrap();
        // The step the process died inside.
        database
            .begin_mutation_step(
                MutationIntent::Create,
                MutationStep::DnsRoute,
                Some("laplus.example.com"),
            )
            .unwrap();
    }

    // A server boots over it without objecting: an unfinished mutation is state
    // to reconcile, not a database this build refuses to open.
    let server = TestServer::start_at(&path).await;
    assert_eq!(server.get("/api/access/cloudflare").await.status, 200);
    server.stop().await;

    let database = Database::open(&path).unwrap();
    let journal = database.mutation_journal().unwrap();
    let completed: Vec<_> = journal
        .iter()
        .filter(|entry| entry.state == MutationState::Completed)
        .map(|entry| entry.step)
        .collect();
    let remaining: Vec<_> = journal
        .iter()
        .filter(|entry| entry.state == MutationState::Pending)
        .map(|entry| entry.step)
        .collect();
    assert_eq!(completed, [MutationStep::Credential, MutationStep::TunnelCreate]);
    assert_eq!(remaining, [MutationStep::DnsRoute]);
    // The resource a retry has to target, not the name it was asked for.
    assert_eq!(
        journal
            .iter()
            .find(|entry| entry.step == MutationStep::TunnelCreate)
            .and_then(|entry| entry.detail.as_deref()),
        Some("tunnel-uuid")
    );
}
