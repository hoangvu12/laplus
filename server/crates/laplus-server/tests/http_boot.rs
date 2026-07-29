//! The two HTTP answers the UI needs before it will open a socket.
//!
//! These are the least well-founded part of the ticket: ticket 01's proxy
//! recorded `/ws` connections only, so unlike everything else here there is no
//! capture to conform to — only the contract. The tests below assert what the
//! contract requires and what the UI's boot path reads, and nothing more,
//! because anything beyond that would be a guess dressed as a check.

mod harness;

use harness::TestServer;
use serde_json::json;

/// Ticket 05 of the headless-Linux effort, and the cheapest test in this file.
///
/// A test server binds whatever its config says, and its config came from
/// `ServerConfig::detect` — which reads the developer's real
/// `remote-access.json`. On a machine where network access had been switched on
/// that meant `0.0.0.0`, and two things followed, neither of them a real
/// failure of the code under test: [`TestServer::addr`] answered with the
/// wildcard, which Windows refuses to connect *to*, so **298 tests failed with
/// `AddrNotAvailable`** across every HTTP and socket binary on one machine
/// while passing everywhere else — the shape of bug that gets blamed on
/// whatever change is under review; and every test binary raised a Windows
/// Defender Firewall prompt, once per binary and again after every rebuild,
/// because cargo names them by hash.
///
/// So the property is worth asserting rather than assuming. A test server is
/// one process talking to itself and has no business being reachable from
/// another machine.
#[tokio::test]
async fn a_test_server_is_reachable_from_this_machine_and_no_other() {
    let server = TestServer::start().await;

    assert!(
        server.addr().ip().is_loopback(),
        "the suite should never bind an address the network can reach, got {}",
        server.addr()
    );
}

/// Without this the UI never registers a connection, never starts a
/// supervisor, and never opens the socket. The failure is swallowed and
/// retried every three seconds, so its absence looks like a UI that simply
/// never connects.
#[tokio::test]
async fn the_environment_descriptor_is_served_unauthenticated() {
    let server = TestServer::start().await;

    let response = server.get("/.well-known/t3/environment").await;

    assert_eq!(response.status, 200);
    assert!(
        response.head.to_lowercase().contains("application/json"),
        "expected json, got head: {}",
        response.head
    );

    for field in [
        "environmentId",
        "label",
        "platform",
        "serverVersion",
        "capabilities",
    ] {
        assert!(
            response.body.get(field).is_some(),
            "descriptor is missing {field}: {}",
            response.body
        );
    }
    assert!(response.body["platform"]["os"].is_string());
    assert!(response.body["platform"]["arch"].is_string());

    server.stop().await;
}

/// The UI compares the environment it discovered over HTTP against the one the
/// socket reports. Two answers to "which machine is this?" would read as
/// having connected to the wrong one.
#[tokio::test]
async fn the_descriptor_agrees_with_the_one_the_socket_reports() {
    let server = TestServer::start().await;

    let over_http = server.get("/.well-known/t3/environment").await.body;

    let mut client = server.connect().await;
    let over_socket = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();

    assert_eq!(over_http, over_socket["environment"]);

    client.close().await;
    server.stop().await;
}

/// The UI's root route awaits this before rendering anything, so a server
/// without it leaves a blank window rather than an error.
#[tokio::test]
async fn the_session_endpoint_reports_an_authenticated_local_client() {
    let server = TestServer::start().await;

    let response = server.get("/api/auth/session").await;

    assert_eq!(response.status, 200);
    // Permissive by design: v1 has no identity store, and answering `false`
    // would send the UI to a pairing screen with no pairing flow behind it.
    assert_eq!(response.body["authenticated"], json!(true));

    let auth = &response.body["auth"];
    assert!(auth["policy"].is_string());
    assert!(auth["bootstrapMethods"].is_array());
    assert!(auth["sessionMethods"].is_array());
    assert_eq!(auth["sessionCookieName"], json!("t3_session"));

    server.stop().await;
}

#[tokio::test]
async fn the_session_endpoint_agrees_with_the_socket_on_the_auth_descriptor() {
    let server = TestServer::start().await;

    let over_http = server.get("/api/auth/session").await.body;

    let mut client = server.connect().await;
    let over_socket = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();

    assert_eq!(over_http["auth"], over_socket["auth"]);

    client.close().await;
    server.stop().await;
}

/// Both are fetched before any credential exists — the descriptor on a cold
/// load with no cookie at all, the session endpoint precisely to find out
/// whether there is one. Requiring a credential would deadlock the boot.
#[tokio::test]
async fn neither_endpoint_requires_a_credential() {
    let server = TestServer::start().await;

    for path in ["/.well-known/t3/environment", "/api/auth/session"] {
        let response = server.get(path).await;
        assert_eq!(response.status, 200, "{path} should not require auth");
    }

    server.stop().await;
}

/// Everything else is still a 404 rather than something inventive. The
/// contract declares a great deal this server does not implement, and a route
/// invented to fill one of those gaps would be a guess the client then has to
/// decode.
///
/// `/api/orchestration/shell` used to be on this list. It is answered now — see
/// `http_orchestration.rs` — because the UI asks for it on every load and takes
/// the socket fallback when it 404s. `/api/auth/browser-session` left the list
/// for ticket 73: it is how the window and a paired phone both open a session,
/// and `http_pairing.rs` drives it.
#[tokio::test]
async fn an_unimplemented_http_route_is_a_plain_404() {
    let server = TestServer::start().await;

    for path in ["/", "/api/orchestration/snapshot", "/api/auth/clients"] {
        let response = server.get(path).await;
        assert_eq!(response.status, 404, "{path} should be a 404");
    }

    server.stop().await;
}
