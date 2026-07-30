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
/// this server advertises `connectionProbe` — probes by calling `server.probe`.
/// All of that has to work on one connection.
///
/// The config is fetched again at the end anyway, because the probe answering is
/// only half of what this is for: the connection has to still be the same one
/// afterwards, and an empty success proves nothing about that on its own.
#[tokio::test]
async fn the_connection_survives_the_keepalive_and_the_probe() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    assert_eq!(
        first["environment"]["capabilities"]["connectionProbe"],
        json!(true),
        "the client picks its probe method from this flag: {first}"
    );

    for _ in 0..3 {
        assert_eq!(client.ping().await, json!({"_tag": "Pong"}));
    }

    for _ in 0..3 {
        assert_eq!(
            client.call("server.probe", json!({})).await.expect_success(),
            json!({}),
            "the probe answers empty and says nothing about the server's state"
        );
    }

    let after = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    assert_eq!(first, after, "the connection outlived its probes");

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

/// A method the contract declares and this server has not built, and the UI's own
/// boot sequence asks for several of them. Each refusal must cost one call and
/// nothing else.
///
/// Since ticket 39 the refusal also has to be *readable*:
/// `previewAutomation.respond` declares
/// `PreviewAutomationError | EnvironmentAuthorizationError`, and an error outside
/// that union costs the call and then puts the schema decoder's complaint on the
/// screen. `crate::refusals` holds the per-method table and the test that reads it
/// back out of the contract.
///
/// **The subject is chosen to outlive the parity work**, which is why it is not
/// `orchestration.replayEvents` any more — that method left the contract rather
/// than getting an implementation. Preview automation is last in
/// `.scratch/contract-parity/ledger.md`'s order and the one cluster whose
/// usefulness waits on something off-contract entirely: there is no MCP server
/// here to ask for a click. `crate::rpc`'s enumeration names the same method for
/// the same reason.
///
/// Both spell it out as a literal rather than sharing a constant, which is the
/// choice `crate::refusals` already makes for the refusal sentence and makes for
/// the same reason: a shared constant lets the two agree with each other while
/// both drift from the contract. So re-pointing one means grepping the method
/// name and re-pointing the other.
#[tokio::test]
async fn an_unimplemented_method_is_reported_without_dropping_the_connection() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    match client.call("previewAutomation.respond", json!({})).await {
        Outcome::Failure(cause) => {
            assert_eq!(cause.len(), 1, "one cause entry: {cause:?}");
            assert_eq!(cause[0]["_tag"], "Fail");
            assert_eq!(cause[0]["error"]["_tag"], "EnvironmentAuthorizationError");
            assert_eq!(cause[0]["error"]["requiredScope"], "orchestration:read");
            assert_eq!(
                cause[0]["error"]["message"],
                "Method not implemented by this server: previewAutomation.respond"
            );
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

/// The credential shapes from ticket 01 that this server issues, each opening a
/// socket that can then be called on.
///
/// **Rewritten by ticket 73**, and the rewrite is the point. This used to
/// present an invented cookie, an invented ticket, an invented bearer and
/// nothing at all, and assert that all four were accepted — which they were,
/// because nothing was verified. Now each is a credential this server actually
/// minted, and the invented ones have their own test below.
#[tokio::test]
async fn every_credential_shape_this_server_issues_opens_a_socket() {
    let server = TestServer::start().await;

    let identities = [
        ("browser cookie", server.browser()),
        ("websocket ticket", server.ticketed().await),
        ("bearer token", server.bearer()),
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

/// The other half, and the change ticket 73 is fundamentally about: a
/// credential this server did not issue does not open a socket, and neither
/// does none at all.
///
/// Until ticket 73 every one of these was accepted — deliberately, and safely,
/// while loopback was the boundary. `docs/adr/0019` is why that stopped being
/// true and `crate::auth` carries the summary.
#[tokio::test]
async fn a_credential_this_server_did_not_issue_opens_nothing() {
    let server = TestServer::start().await;

    let identities = [
        ("no credential at all", ClientIdentity::anonymous()),
        (
            "an invented cookie",
            ClientIdentity {
                cookie: Some("t3_session=eyJ2IjoxfQ.c2ln".to_string()),
                ..ClientIdentity::default()
            },
        ),
        (
            "an invented ticket",
            ClientIdentity {
                ticket: Some("eyJ2IjoxfQ.c2ln".to_string()),
                ..ClientIdentity::default()
            },
        ),
        (
            "an invented bearer",
            ClientIdentity {
                authorization: Some("Bearer eyJ2IjoxfQ.c2ln".to_string()),
                ..ClientIdentity::default()
            },
        ),
        // Advertised in the descriptor because the shape is read, and refused
        // because this server implements no proof-of-possession. Accepting one
        // would be taking a credential while ignoring the proof that is the
        // whole point of the scheme.
        (
            "a DPoP token, which this server does not implement",
            ClientIdentity {
                authorization: Some("DPoP eyJ2IjoxfQ.c2ln".to_string()),
                ..ClientIdentity::default()
            },
        ),
    ];

    for (name, identity) in identities {
        let refusal = server
            .connect_as(identity)
            .await
            .err()
            .unwrap_or_else(|| panic!("{name} should be refused"));
        assert_eq!(refusal.status, 401, "{name}");
        assert_eq!(refusal.body["_tag"], "EnvironmentAuthInvalidError", "{name}");
    }

    // Nothing was left behind by any of them.
    server.await_live_connections(0).await;
    server.stop().await;
}

/// A socket ticket opens one socket and not two, at the upgrade rather than at
/// the route that minted it.
///
/// This is why the ticket shape exists at all: it rides in a query string,
/// which is the one place in the chain a credential lands in a proxy log, and
/// single use is what makes a ticket in a log worth nothing.
#[tokio::test]
async fn a_socket_ticket_is_spent_by_the_upgrade_that_uses_it() {
    let server = TestServer::start().await;
    let ticketed = server.ticketed().await;

    let client = server
        .connect_as(ticketed.clone())
        .await
        .expect("the first upgrade spends the ticket");
    client.close().await;
    server.await_live_connections(0).await;

    let refusal = server
        .connect_as(ticketed)
        .await
        .expect_err("the same ticket does not open a second socket");
    assert_eq!(refusal.status, 401);

    server.stop().await;
}

/// The refusal body in full, against the capture's fields.
///
/// **This used to be the origin test**, and asserted that a page on
/// `evil.example` was turned away. `crate::auth` no longer checks an origin —
/// see its `## Why there is no origin rule` — so the premise moved to the thing
/// that *is* checked: a credential this server did not issue. The body is the
/// same one either way, which is the point of `Rejection`'s closed union, and it
/// is worth asserting in full somewhere rather than only by `_tag` as
/// `a_credential_this_server_did_not_issue_opens_nothing` does across its five
/// shapes.
#[tokio::test]
async fn a_credential_that_does_not_verify_is_refused_with_the_captured_error_body() {
    let server = TestServer::start().await;

    let refusal = server
        .connect_as(ClientIdentity {
            cookie: Some("t3_session=eyJ2IjoxfQ.c2ln".to_string()),
            ..ClientIdentity::default()
        })
        .await
        .expect_err("a cookie this server did not mint is refused");

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

/// **The credential is the whole boundary, and the origin is not part of it.**
///
/// The first three are the ones the desktop window and the Vite dev server
/// send. The fourth is the one this file used to assert a refusal for, and is
/// here deliberately: a socket opened from `evil.example` is accepted because it
/// carried a credential this server minted, and would be accepted from a phone
/// on a tunnel or a tailnet for the same reason — which is what makes the
/// headless-Linux effort possible without an allowlist to maintain.
///
/// What that gives up is in [`laplus_server::auth::authorize`], stated rather
/// than implied.
#[tokio::test]
async fn the_origin_a_page_came_from_is_not_what_this_server_checks() {
    let server = TestServer::start().await;

    for origin in [
        "http://127.0.0.1:1420",
        "http://localhost:5173",
        "http://[::1]",
        "https://evil.example",
    ] {
        let client = server
            .connect_as(server.browser().with_origin(origin))
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

    // The browser's offer, verbatim in shape from `01-browser-session.ndjson`
    // — with a real session cookie, because since ticket 73 an invented one is
    // refused before the handshake is negotiated at all.
    let head = server
        .raw_upgrade(&format!(
            "GET /ws HTTP/1.1\r\n\
             Host: {addr}\r\n\
             Connection: Upgrade\r\n\
             Upgrade: websocket\r\n\
             Origin: http://{addr}\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: sXlZ+AnHRboR6K8AWi8sxw==\r\n\
             Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\n\
             Cookie: {cookie}\r\n\r\n",
            addr = server.addr(),
            cookie = server
                .browser()
                .cookie
                .expect("the harness paired itself at startup"),
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

/// Ticket 33, at the boundary rather than at the type. Every capture in
/// `fixtures/socket-wire/` has a string request id, so a server written from
/// them alone requires one — and a current Effect client sends numbers, which
/// this server dropped as malformed. The symptom was the worst kind: no error,
/// no `Exit`, a UI holding an open socket and waiting forever.
#[tokio::test]
async fn a_request_whose_id_is_a_number_is_answered() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .send(json!({
            "_tag": "Request",
            "id": 0,
            "tag": "server.getConfig",
            "payload": {},
            "headers": [],
        }))
        .await;
    let frame = client.recv().await;

    assert_eq!(frame["_tag"], json!("Exit"), "{frame}");
    assert!(
        frame["exit"]["value"]["environment"].is_object(),
        "a numeric id should be answered with the same config as a string one: {frame}"
    );
    // The client keys its in-flight calls by the id it sent, so `"0"` here
    // would be an answer nothing is waiting for — the same silence, later.
    assert_eq!(
        frame["requestId"],
        json!(0),
        "the id has to come back the way it was sent: {frame}"
    );

    client.close().await;
    server.stop().await;
}

/// The streaming half of the same: a subscription opened with a numeric id has
/// to be one that `Ack` and `Interrupt` can name.
#[tokio::test]
async fn a_subscription_opened_with_a_numeric_id_can_be_fed_and_cancelled() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .send(json!({
            "_tag": "Request",
            "id": 4,
            "tag": "subscribeServerConfig",
            "payload": {},
            "headers": [],
        }))
        .await;

    let snapshot = client.recv().await;
    assert_eq!(snapshot["_tag"], json!("Chunk"), "{snapshot}");
    assert_eq!(snapshot["requestId"], json!(4), "{snapshot}");

    // An `Ack` the registry cannot match is a stream that stops after one
    // chunk, which is ticket 28's shape of bug and invisible from here — so
    // this asserts on the cancellation, which the acknowledged stream must
    // still be alive to report.
    client.send(json!({"_tag": "Ack", "requestId": 4})).await;
    client
        .send(json!({"_tag": "Interrupt", "requestId": 4}))
        .await;

    let exit = client.recv().await;
    assert_eq!(exit["_tag"], json!("Exit"), "{exit}");
    assert_eq!(exit["requestId"], json!(4), "{exit}");
    assert_eq!(
        exit["exit"]["cause"][0]["_tag"],
        json!("Interrupt"),
        "a cancelled subscription ends as an interrupt: {exit}"
    );

    client.close().await;
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
