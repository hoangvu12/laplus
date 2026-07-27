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

use harness::conversation::{create_project, create_thread};
use harness::workspace::Workspace;
use harness::{ClientIdentity, TestServer};
use serde_json::{json, Value};

const THREAD: &str = "thread-1";

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
/// again, which is the whole of what this ticket set out to stop — and for the
/// primary local connection the real client sends no credential at all
/// (`buildEnvironmentAuthHeaders` returns `{}` when the connection carries
/// none), so "no credential" is not an edge case here but the case.
#[tokio::test]
async fn every_credential_that_opens_the_socket_reads_a_snapshot() {
    let server = a_server_with_a_conversation().await;
    let same_origin = format!("http://{}", server.addr());

    for (what, identity) in [
        ("nothing at all", ClientIdentity::anonymous()),
        ("a websocket ticket", ClientIdentity::ticket()),
        ("the browser's cookie", ClientIdentity::browser()),
        ("a bearer token", ClientIdentity::bearer()),
        (
            "the window's own origin",
            ClientIdentity::anonymous().with_origin(&same_origin),
        ),
    ] {
        for path in [
            "/api/orchestration/shell",
            &format!("/api/orchestration/threads/{THREAD}"),
        ] {
            let response = server.get_as(path, &identity).await;
            assert_eq!(
                response.status, 200,
                "{path} refused {what}: {}",
                response.text
            );
        }

        // And the same identity opens a socket, which is the claim being made.
        server
            .connect_as(identity)
            .await
            .unwrap_or_else(|refusal| panic!("{what} was refused at the upgrade: {refusal:?}"))
            .close()
            .await;
    }

    server.stop().await;
}

/// The one refusal [`laplus_server::auth`] makes, reaching these routes as it
/// reaches the upgrade. Binding to loopback does not stop a page elsewhere from
/// asking the user's own browser to fetch the project list on its behalf.
#[tokio::test]
async fn a_non_local_origin_is_refused_with_the_auth_error() {
    let server = a_server_with_a_conversation().await;
    let elsewhere = ClientIdentity::browser().with_origin("https://evil.example");

    for path in [
        "/api/orchestration/shell",
        &format!("/api/orchestration/threads/{THREAD}"),
    ] {
        let response = server.get_as(path, &elsewhere).await;
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

    // A refusal must not be readable by the page that provoked it either: the
    // upgrade sets `Access-Control-Allow-Origin: *` on its 401 so a browser
    // reads the body rather than a CORS error, and that is right for a socket
    // handshake and wrong for a fetch a foreign page made.
    let response = server.get_as("/api/orchestration/shell", &elsewhere).await;
    assert_eq!(response.header("access-control-allow-origin"), None);

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
