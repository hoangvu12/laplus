//! The tracer bullet, driven at the boundary the UI actually uses.
//!
//! Every test here starts a real server on a real loopback port, opens a real
//! socket, and asserts only on what a client can observe. Nothing reaches into
//! server internals — the one exception is the live-connection gauge, which is
//! itself an observable the server publishes and the only way to check that a
//! disconnect leaves nothing behind.

mod harness;

use std::time::Duration;

use harness::{ClientIdentity, Outcome, TestServer};
use serde_json::json;

/// The whole point of the ticket: the client's first call is answered, and it
/// is answered with something shaped like a server config.
#[tokio::test]
async fn the_clients_first_call_is_answered_with_a_server_config() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let config = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();

    for field in [
        "environment",
        "auth",
        "cwd",
        "keybindingsConfigPath",
        "keybindings",
        "issues",
        "providers",
        "availableEditors",
        "observability",
        "settings",
    ] {
        assert!(
            config.get(field).is_some(),
            "config is missing {field}: {config}"
        );
    }
    assert_eq!(config, server.config());

    client.close().await;
    server.stop().await;
}

/// The client fetches config, keeps the socket alive with `Ping`, and — since
/// this server does not advertise `connectionProbe` — probes by calling
/// `server.getConfig` again. All of that has to work on one connection.
#[tokio::test]
async fn the_connection_survives_the_keepalive_and_the_probe() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();

    for _ in 0..3 {
        assert_eq!(client.ping().await, json!({"_tag": "Pong"}));
    }

    let probed = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    assert_eq!(first, probed, "the probe must see the same config");

    client.close().await;
    server.stop().await;
}

/// The UI genuinely issues concurrent calls — ids 7, 8, 9, 10 in flight at
/// once in `01-browser-session.ndjson`. Awaiting them out of order proves the
/// answers carry enough to be correlated rather than merely counted.
#[tokio::test]
async fn concurrent_calls_are_correlated_by_request_id() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = client.send_request("server.getConfig", json!({})).await;
    let second = client.send_request("server.getConfig", json!({})).await;
    let third = client.send_request("server.getConfig", json!({})).await;
    assert_ne!(first, second);
    assert_ne!(second, third);

    for id in [&third, &first, &second] {
        assert!(matches!(
            client.await_outcome(id).await,
            Outcome::Success(_)
        ));
    }

    client.close().await;
    server.stop().await;
}

/// Fifty-nine of the sixty methods are unimplemented at this ticket, and the
/// UI's own boot sequence asks for four of them. Each refusal must cost one
/// call and nothing else.
#[tokio::test]
async fn an_unimplemented_method_is_reported_without_dropping_the_connection() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    match client.call("orchestration.subscribeShell", json!({})).await {
        Outcome::Failure(cause) => {
            assert_eq!(cause.len(), 1, "one cause entry: {cause:?}");
            assert_eq!(cause[0]["_tag"], "Fail");
            assert_eq!(cause[0]["error"]["_tag"], "ServerMethodNotImplementedError");
            assert_eq!(cause[0]["error"]["method"], "orchestration.subscribeShell");
        }
        other => panic!("expected a typed failure, got {other:?}"),
    }

    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// The failure has to come back correlated. A `Defect` carries no `requestId`
/// and the client responds by failing *everything* in flight — so the thing
/// worth asserting is that a call in flight beside an unimplemented one still
/// gets its own answer.
#[tokio::test]
async fn an_unimplemented_method_does_not_disturb_a_call_in_flight_beside_it() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let good = client.send_request("server.getConfig", json!({})).await;
    let bad = client.send_request("server.subscribeServerConfig", json!({})).await;

    assert!(matches!(
        client.await_outcome(&bad).await,
        Outcome::Failure(_)
    ));
    assert!(matches!(
        client.await_outcome(&good).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// A frame the server cannot parse, and a frame whose `_tag` it does not know.
/// Both are counted and neither is fatal — the same degradation rule the CLI
/// protocol follows, applied to the socket.
#[tokio::test]
async fn unparseable_and_unrecognised_frames_are_counted_rather_than_fatal() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client.send_text("this is not json").await;
    client.send(json!({"_tag": "Eof", "requestId": "0"})).await;
    client.send(json!({"_tag": "Ack", "requestId": "0"})).await;
    client
        .send(json!({"_tag": "Interrupt", "requestId": "0"}))
        .await;

    // None of the four warrants a reply, so the next frame must be the answer
    // to the call that follows them.
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    assert_eq!(server.unparseable_frames(), 1);
    assert_eq!(server.unrecognized_messages(), 1);

    client.close().await;
    server.stop().await;
}

/// `Ack` and `Interrupt` are ordinary traffic for a client with a subscription
/// open. Nothing streams yet, so the correct answer is nothing at all — not an
/// error, and not a stray frame that would desynchronise the client.
#[tokio::test]
async fn ack_and_interrupt_draw_no_reply() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client.send(json!({"_tag": "Ack", "requestId": "0"})).await;
    client
        .send(json!({"_tag": "Interrupt", "requestId": "0"}))
        .await;
    client.expect_silence(Duration::from_millis(250)).await;

    client.close().await;
    server.stop().await;
}

/// All three credential shapes from ticket 01, plus none at all. v1 verifies
/// none of them; what it must not do is refuse one.
#[tokio::test]
async fn every_credential_shape_is_accepted() {
    let server = TestServer::start().await;

    let identities = [
        ("browser cookie", ClientIdentity::browser()),
        ("websocket ticket", ClientIdentity::ticket()),
        (
            "bearer token",
            ClientIdentity {
                authorization: Some("Bearer eyJ2IjoxfQ.c2ln".to_string()),
                ..ClientIdentity::default()
            },
        ),
        (
            "dpop token",
            ClientIdentity {
                authorization: Some("DPoP eyJ2IjoxfQ.c2ln".to_string()),
                ..ClientIdentity::default()
            },
        ),
        ("no credential", ClientIdentity::anonymous()),
    ];

    for (name, identity) in identities {
        let mut client = server
            .connect_as(identity)
            .await
            .unwrap_or_else(|refusal| panic!("{name} should be accepted, got {refusal:?}"));
        assert!(
            matches!(client.call("server.getConfig", json!({})).await, Outcome::Success(_)),
            "{name} should be able to call"
        );
        client.close().await;
    }

    server.stop().await;
}

/// Loopback binding stops another machine reaching the server. It does not
/// stop a page on another origin asking the user's own browser to connect for
/// it, which is what the origin check is for.
#[tokio::test]
async fn a_non_local_origin_is_refused_with_the_captured_error_body() {
    let server = TestServer::start().await;

    let refusal = server
        .connect_as(ClientIdentity::browser().with_origin("https://evil.example"))
        .await
        .expect_err("a non-local origin is refused");

    assert_eq!(refusal.status, 401);
    assert_eq!(refusal.body["_tag"], "EnvironmentAuthInvalidError");
    assert_eq!(refusal.body["code"], "auth_invalid");
    assert_eq!(refusal.body["reason"], "invalid_credential");
    assert!(refusal.body["traceId"].is_string());

    // And the server is otherwise unaffected.
    server.await_live_connections(0).await;
    let mut client = server.connect().await;
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn a_loopback_origin_is_accepted() {
    let server = TestServer::start().await;

    for origin in ["http://127.0.0.1:1420", "http://localhost:5173", "http://[::1]"] {
        let client = server
            .connect_as(ClientIdentity::browser().with_origin(origin))
            .await
            .unwrap_or_else(|refusal| panic!("{origin} should be accepted, got {refusal:?}"));
        client.close().await;
    }

    server.stop().await;
}

/// The app is opened and closed, or the webview is reloaded, many times over a
/// session. Each cycle has to leave the server exactly as it found it.
#[tokio::test]
async fn reconnecting_repeatedly_leaves_nothing_behind() {
    let server = TestServer::start().await;
    assert_eq!(server.live_connections(), 0);

    for _ in 0..5 {
        let mut client = server.connect().await;
        assert!(matches!(
            client.call("server.getConfig", json!({})).await,
            Outcome::Success(_)
        ));
        assert_eq!(server.live_connections(), 1);
        client.close().await;
        server.await_live_connections(0).await;
    }

    // Several at once, then all gone.
    let mut clients = Vec::new();
    for _ in 0..3 {
        clients.push(server.connect().await);
    }
    for client in &mut clients {
        assert!(matches!(
            client.call("server.getConfig", json!({})).await,
            Outcome::Success(_)
        ));
    }
    assert_eq!(server.live_connections(), 3);
    for client in clients {
        client.close().await;
    }
    server.await_live_connections(0).await;

    server.stop().await;
}

/// A client that vanishes without a close frame — a crashed tab, a killed
/// process — must be reaped the same way a polite one is.
#[tokio::test]
async fn a_client_that_disappears_without_closing_is_reaped() {
    let server = TestServer::start().await;

    let mut client = server.connect().await;
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));
    drop(client);

    server.await_live_connections(0).await;
    server.stop().await;
}

/// Two things ticket 01 pinned that live in the handshake rather than in any
/// frame. A WebSocket library normalises both away, so this speaks HTTP by
/// hand.
#[tokio::test]
async fn the_upgrade_declines_compression_and_negotiates_no_subprotocol() {
    let server = TestServer::start().await;

    // The browser's offer, verbatim in shape from `01-browser-session.ndjson`.
    let head = server
        .raw_upgrade(&format!(
            "GET /ws HTTP/1.1\r\n\
             Host: {}\r\n\
             Connection: Upgrade\r\n\
             Upgrade: websocket\r\n\
             Origin: http://{}\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: sXlZ+AnHRboR6K8AWi8sxw==\r\n\
             Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\n\
             Cookie: t3_session=eyJ2IjoxfQ.c2ln\r\n\r\n",
            server.addr(),
            server.addr()
        ))
        .await;

    assert!(
        head.starts_with("HTTP/1.1 101 "),
        "expected a 101, got: {head}"
    );
    let lowercased = head.to_lowercase();
    assert!(
        !lowercased.contains("sec-websocket-extensions"),
        "permessage-deflate must be declined, so every frame stays uncompressed: {head}"
    );
    assert!(
        !lowercased.contains("sec-websocket-protocol"),
        "no subprotocol is negotiated: {head}"
    );

    server.stop().await;
}

/// Shutting the server down closes open sockets rather than waiting on them.
/// Without this the Tauri shell would hang on quit whenever the webview still
/// held a connection — which is always.
#[tokio::test]
async fn shutdown_closes_an_open_socket() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    tokio::time::timeout(Duration::from_secs(5), server.stop())
        .await
        .expect("shutdown does not wait for the open socket");
}
