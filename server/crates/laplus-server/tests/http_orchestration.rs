//! The two snapshots the UI asks for over HTTP before it falls back to the
//! socket.
//!
//! Ticket 31. The client loads the shell snapshot and each thread snapshot with
//! a `fetch` and uses the socket-embedded copy when that fails — so until these
//! routes existed, *every* snapshot took the fallback and logged a 404 and a
//! warning on the way. The socket path is unchanged and remains the fallback;
//! what these buy is that the fast path is no longer a guaranteed miss.
//!
//! Unlike `http_boot.rs` these are not written from a guess. Both routes are
//! pinned by `EnvironmentOrchestrationHttpApi` in
//! `packages/contracts/src/environmentHttp.ts` — paths, params, headers, the
//! success schemas and the error union — and the payloads are the ones the
//! socket already carries, which is what the agreement tests below are for.

mod harness;

use std::time::Duration;

use harness::conversation::{create_project, create_thread};
use harness::workspace::Workspace;
use harness::{ClientIdentity, TestServer};
use serde_json::{json, Value};

const THREAD: &str = "thread-1";

/// Long enough that a chunk the server was going to send would have arrived.
/// The same figure and the same reasoning as `socket_streaming.rs`.
const SILENCE: Duration = Duration::from_millis(200);

/// Register a project and a conversation over the socket, so that both
/// snapshots have something in them.
async fn a_server_with_a_conversation() -> TestServer {
    let server = TestServer::start().await;
    let workspace = Workspace::with(&["src/main.rs"]);

    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            create_thread("project-1", THREAD),
        )
        .await
        .expect_success();
    client.close().await;

    // The workspace is dropped here, and deliberately: nothing either route
    // does reads the folder. Both answer from the registry and from memory.
    server
}

/// `OrchestrationShellSnapshot` is four keys and the client's decode needs all
/// of them — a missing one is a failed decode, which is the fallback again with
/// an extra round-trip in front of it.
#[tokio::test]
async fn the_shell_snapshot_is_served_over_http() {
    let server = a_server_with_a_conversation().await;

    let response = server.get("/api/orchestration/shell").await;

    assert_eq!(response.status, 200);
    assert!(
        response.head.to_lowercase().contains("application/json"),
        "expected json, got head: {}",
        response.head
    );
    for field in ["snapshotSequence", "projects", "threads", "updatedAt"] {
        assert!(
            response.body.get(field).is_some(),
            "the shell snapshot is missing {field}: {}",
            response.body
        );
    }
    assert!(response.body["snapshotSequence"].is_i64());
    assert_eq!(response.body["projects"][0]["id"], json!("project-1"));
    assert_eq!(response.body["threads"][0]["id"], json!(THREAD));

    server.stop().await;
}

/// The same test the environment descriptor gets, for the same reason: the
/// client takes whichever of the two answers it can get, so two different
/// answers would mean the shell a developer sees depends on which transport
/// won.
#[tokio::test]
async fn the_shell_snapshot_agrees_with_the_one_the_socket_opens_with() {
    let server = a_server_with_a_conversation().await;

    let over_http = server.get("/api/orchestration/shell").await.body;
    let over_socket = server.connect().await.into_shell_snapshot().await;

    assert_eq!(over_http, over_socket);

    server.stop().await;
}

/// `OrchestrationThreadDetailSnapshot` is the sequence and the conversation.
/// This is the payload the ticket calls potentially multi-KB, and the reason
/// the client wanted it off the socket in the first place.
#[tokio::test]
async fn the_thread_snapshot_is_served_over_http() {
    let server = a_server_with_a_conversation().await;

    let response = server
        .get(&format!("/api/orchestration/threads/{THREAD}"))
        .await;

    assert_eq!(response.status, 200);
    assert!(
        response.head.to_lowercase().contains("application/json"),
        "expected json, got head: {}",
        response.head
    );
    assert!(response.body["snapshotSequence"].is_i64());
    assert_eq!(response.body["thread"]["id"], json!(THREAD));
    assert_eq!(response.body["thread"]["projectId"], json!("project-1"));

    server.stop().await;
}

#[tokio::test]
async fn the_thread_snapshot_agrees_with_the_one_the_socket_opens_with() {
    let server = a_server_with_a_conversation().await;

    let over_http = server
        .get(&format!("/api/orchestration/threads/{THREAD}"))
        .await
        .body;
    let over_socket = server
        .connect()
        .await
        .into_thread_snapshot(THREAD)
        .await;

    assert_eq!(over_http, over_socket);

    server.stop().await;
}

/// A draft pane subscribes to a thread that does not exist yet, and asks for
/// its snapshot too. The answer has to be the contract's typed `not_found`
/// rather than a bare 404: the client catches that tag by name and defers to
/// the socket quietly, where anything else is a warning in the console
/// `tools/ui-driver` reads.
#[tokio::test]
async fn an_unknown_thread_is_a_typed_404() {
    let server = a_server_with_a_conversation().await;

    let response = server.get("/api/orchestration/threads/never-created").await;

    assert_eq!(response.status, 404);
    assert_eq!(
        response.body["_tag"],
        json!("EnvironmentResourceNotFoundError")
    );
    assert_eq!(response.body["code"], json!("not_found"));
    assert_eq!(response.body["reason"], json!("thread_not_found"));
    assert_eq!(
        response.body["traceId"]
            .as_str()
            .expect("a traceId")
            .len(),
        32
    );

    server.stop().await;
}

/// A blank thread id names no conversation either, and it arrives here
/// percent-encoded rather than as an empty segment — the contract types
/// `threadId` as a trimmed non-empty string, so this is a client that sent
/// something the contract says it cannot.
#[tokio::test]
async fn a_thread_id_that_names_nothing_is_the_same_typed_404() {
    let server = a_server_with_a_conversation().await;

    let response = server.get("/api/orchestration/threads/%20").await;

    assert_eq!(response.status, 404);
    assert_eq!(response.body["code"], json!("not_found"));

    // The empty segment is not a route at all, so it is the plain 404 an
    // unimplemented path has always been. What matters is that it is not the
    // UI's entry point — see `the_servers_own_surface_is_never_answered_with_the_ui`.
    assert_eq!(server.get("/api/orchestration/threads/").await.status, 404);

    server.stop().await;
}

/// **A credential good enough to open the socket is good enough to read a
/// snapshot.** Anything else and the fallback becomes the only working path
/// again, which is the whole of what ticket 31 set out to stop.
///
/// The list is shorter since ticket 73. It used to include "nothing at all",
/// because until then the real client sent no credential on a primary local
/// connection and the server accepted that — so the *absence* of one was the
/// case rather than an edge case. It is now a refusal at both the socket and
/// these routes, which is exactly the property this test still asserts: the two
/// answer alike. `docs/adr/0019`.
#[tokio::test]
async fn every_credential_that_opens_the_socket_reads_a_snapshot() {
    let server = a_server_with_a_conversation().await;
    let same_origin = format!("http://{}", server.addr());

    // Each shape is minted afresh for each use rather than reused across the
    // three, because a socket ticket is **single use** — spending one on the
    // first snapshot would leave nothing for the second, and the failure would
    // read as "this route refuses tickets" rather than "that ticket was already
    // spent".
    for what in [
        "a websocket ticket",
        "the browser's cookie",
        "a bearer token",
        "the window's own origin",
    ] {
        let identity = |server: &TestServer| match what {
            "the browser's cookie" => Some(server.browser()),
            "a bearer token" => Some(server.bearer()),
            "the window's own origin" => Some(server.browser().with_origin(&same_origin)),
            _ => None,
        };

        for path in [
            "/api/orchestration/shell",
            &format!("/api/orchestration/threads/{THREAD}"),
        ] {
            let presented = match identity(&server) {
                Some(presented) => presented,
                None => server.ticketed().await,
            };
            let response = server.get_as(path, &presented).await;
            assert_eq!(
                response.status, 200,
                "{path} refused {what}: {}",
                response.text
            );
        }

        // And the same shape opens a socket, which is the claim being made.
        let presented = match identity(&server) {
            Some(presented) => presented,
            None => server.ticketed().await,
        };
        server
            .connect_as(presented)
            .await
            .unwrap_or_else(|refusal| panic!("{what} was refused at the upgrade: {refusal:?}"))
            .close()
            .await;
    }

    server.stop().await;
}

/// What these routes check, and what they do not.
///
/// **This test asserted the opposite until the origin rule was removed** — that
/// a fetch from `evil.example` was refused with a 401. `crate::auth` reads
/// `Origin` and consults it nowhere, so the credential is the whole boundary
/// here exactly as it is at the upgrade, and the honest assertion is the one
/// below: presenting nothing is refused whatever the origin, and presenting a
/// credential this server minted is answered whatever the origin.
///
/// **Ticket 02 of the headless-Linux effort changed the second half**, on
/// purpose, and this is where it announced itself. It used to end by asserting
/// there was no `Access-Control-Allow-Origin` on the answer — so a browser handed
/// the page a CORS error rather than the project list, and a foreign *page* could
/// not read what a foreign *program* holding a stolen cookie always could.
///
/// That absence is gone, because the desktop application fetching a *remote*
/// laplus is a second origin that has to be answered and there is no way to
/// admit it without admitting every other page too — narrowing `*` to a
/// configured list is the allowlist that was removed on purpose. So the honest
/// statement is the one asserted below: a page anywhere may now read this, and
/// what stands between it and the project list is the credential, which is where
/// `crate::auth` always said the boundary was. `tests/http_cors.rs` carries the
/// whole argument and the header set.
#[tokio::test]
async fn these_routes_check_the_credential_and_not_the_origin() {
    let server = a_server_with_a_conversation().await;
    let paths = [
        "/api/orchestration/shell".to_string(),
        format!("/api/orchestration/threads/{THREAD}"),
    ];

    let elsewhere_with_nothing = ClientIdentity::anonymous().with_origin("https://evil.example");
    for path in &paths {
        let response = server.get_as(path, &elsewhere_with_nothing).await;
        assert_eq!(response.status, 401, "{path}");
        assert_eq!(
            response.body["_tag"],
            json!("EnvironmentAuthInvalidError"),
            "{path}"
        );
        assert_eq!(response.body["code"], json!("auth_invalid"), "{path}");

        // And nothing of the registry came back with the refusal.
        assert!(
            !response.text.contains("project-1"),
            "the refusal leaked the project list: {}",
            response.text
        );
    }

    let elsewhere = server.browser().with_origin("https://evil.example");
    for path in &paths {
        let response = server.get_as(path, &elsewhere).await;
        assert_eq!(
            response.status, 200,
            "{path} — the credential is what is checked"
        );

        // And since ticket 02 the page that asked may read the answer. The
        // credential it had to present to get one is the boundary; the header is
        // what lets the desktop application's own window read a remote laplus at
        // all.
        assert_eq!(
            response.header("access-control-allow-origin").as_deref(),
            Some("*"),
            "{path}"
        );
    }

    server.stop().await;
}

/// **The saving these routes exist for, on the wire.** Ticket 31 made the fast
/// path work; ADR-0016 is what stops the payload travelling a second time
/// behind it.
///
/// The subscription payload here is the real client's, verbatim: laplus
/// advertises neither `shellResumeCompletionMarker` nor its thread twin, so
/// `makeSubscribeInput` in `packages/client-runtime/src/state/shell.ts` sends
/// the cursor alone. Nothing is owed in answer to it, so the subscription sends
/// **no chunk at all** — and then behaves like any other, delivering the next
/// change where a second copy of the registry used to be.
///
/// The silence is asserted before the change is made rather than inferred from
/// what arrives after it, because those are two different claims and only the
/// first one is this file's. The wait is also what makes the second half
/// deterministic: the pump runs on its own task, so a command dispatched
/// immediately after the subscribe can legitimately land in front of the
/// opening description and turn it back into a snapshot.
#[tokio::test]
async fn a_shell_snapshot_read_over_http_does_not_travel_again_on_the_socket() {
    let server = a_server_with_a_conversation().await;
    let workspace = Workspace::with(&["src/main.rs"]);

    let over_http = server.get("/api/orchestration/shell").await.body;
    let mut client = server.connect().await;
    let subscription = client
        .subscribe(
            "orchestration.subscribeShell",
            json!({"afterSequence": over_http["snapshotSequence"]}),
        )
        .await;

    client.expect_silence(SILENCE).await;

    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-2", workspace.path()),
        )
        .await
        .expect_success();

    let first = client.next_chunk(&subscription).await;
    assert_eq!(
        first
            .iter()
            .map(|item| item["kind"].clone())
            .collect::<Vec<_>>(),
        vec![json!("project-upserted")],
        "the registry travelled twice: {first:#?}"
    );
    assert_eq!(first[0]["project"]["id"], json!("project-2"));

    client.close().await;
    server.stop().await;
}

/// The same rule on the conversation, which is the payload the ticket calls
/// multi-KB and the reason any of this was wanted.
///
/// This one asks for the completion marker, which the real client does not —
/// not to describe it, but so the test has something to read. A marker is owed
/// once whatever else is sent, so it makes the opening observable without
/// changing what the rule decides. That the marker still arrives is the point:
/// the client is told it is up to date exactly when it is.
#[tokio::test]
async fn a_thread_snapshot_read_over_http_does_not_travel_again_on_the_socket() {
    let server = a_server_with_a_conversation().await;

    let over_http = server
        .get(&format!("/api/orchestration/threads/{THREAD}"))
        .await
        .body;
    let mut client = server.connect().await;
    let subscription = client
        .subscribe(
            "orchestration.subscribeThread",
            json!({
                "threadId": THREAD,
                "afterSequence": over_http["snapshotSequence"],
                "requestCompletionMarker": true,
            }),
        )
        .await;

    let opening = client.next_chunk(&subscription).await;
    assert_eq!(
        opening,
        vec![json!({"kind": "synchronized"})],
        "the conversation travelled twice: {opening:#?}"
    );

    // And a cursor this server cannot replay from still gets the whole thing,
    // because that is the only other answer it has.
    let stale = over_http["snapshotSequence"].as_i64().expect("a sequence") - 1;
    let refetched = client
        .subscribe(
            "orchestration.subscribeThread",
            json!({"threadId": THREAD, "afterSequence": stale}),
        )
        .await;
    let opening = client.next_chunk(&refetched).await;
    assert_eq!(opening[0]["kind"], json!("snapshot"), "{opening:#?}");
    assert_eq!(opening[0]["snapshot"], over_http);

    client.close().await;
    server.stop().await;
}

/// Nothing else under the prefix was invented along with these two. The
/// contract declares `/api/orchestration/snapshot` and `.../dispatch` as well,
/// and `packages/client-runtime` calls neither — this file exists because
/// `src/http.rs` already carries enough routes written from a guess.
#[tokio::test]
async fn only_the_two_routes_the_client_calls_are_served() {
    let server = a_server_with_a_conversation().await;

    for path in [
        "/api/orchestration/snapshot",
        "/api/orchestration/dispatch",
        "/api/orchestration",
    ] {
        let response = server.get(path).await;
        assert_eq!(response.status, 404, "{path} should stay a 404");
        assert_eq!(response.body, Value::Null, "{path} should be a plain 404");
    }

    server.stop().await;
}
