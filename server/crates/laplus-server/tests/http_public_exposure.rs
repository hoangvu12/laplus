mod harness;

use harness::{ClientIdentity, TestServer};
use serde_json::json;

const GRANT_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange";
const BOOTSTRAP_TOKEN_TYPE: &str = "urn%3At3%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap";
const ACCESS_TOKEN_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token";

async fn client_with(server: &TestServer, scopes: &[&str]) -> ClientIdentity {
    let minted = server.post_json("/api/auth/pairing-token", &json!({ "scopes": scopes })).await;
    let credential = minted.body["credential"].as_str().unwrap();
    let exchanged = server.post_form(
        "/oauth/token",
        &format!("grant_type={GRANT_TYPE}&subject_token={credential}&subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type={ACCESS_TOKEN_TYPE}"),
    ).await;
    ClientIdentity::anonymous().with_bearer(exchanged.body["access_token"].as_str().unwrap())
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
async fn registration_rejects_probe_shaped_and_private_destinations() {
    let server = TestServer::start().await;
    for hostname in ["http://example.com", "https://example.com/a", "https://127.0.0.1"] {
        let response = server.post_json("/api/access/cloudflare", &json!({"hostname": hostname})).await;
        assert_eq!(response.status, 400, "{hostname}: {}", response.text);
    }
    server.stop().await;
}
