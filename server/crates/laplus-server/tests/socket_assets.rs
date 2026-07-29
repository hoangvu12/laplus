//! A project's own icon, from the method that issues its URL to the route that
//! honours it.
//!
//! The unit tests beside `laplus_server::assets` and
//! `laplus_server::project_favicon` cover the rules — which file in a project
//! is its icon, what a token is, which forgeries are refused. What can only be
//! said here is that the two halves are wired to each other: the socket method
//! and the HTTP route are reached through different parts of `axum` and share
//! nothing but a key in SQLite, so a URL that verifies against a key one of
//! them loaded and the other did not is the failure this file exists to catch.
//!
//! Everything is SVG rather than PNG because [`harness::HttpResponse`] carries
//! the body as a `String` — an icon that is text is an icon this can compare.

mod harness;

use harness::workspace::Workspace;
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

const ICON: &str = r#"<svg xmlns="http://www.w3.org/2000/svg"><circle r="8"/></svg>"#;

async fn create_url(client: &mut SocketClient, resource: Value) -> Outcome {
    client
        .call("assets.createUrl", json!({"resource": resource}))
        .await
}

async fn favicon_url(client: &mut SocketClient, workspace: &Workspace) -> Value {
    create_url(
        client,
        json!({"_tag": "project-favicon", "cwd": workspace.cwd()}),
    )
    .await
    .expect_success()
}

/// The whole feature, end to end: the sidebar asks for a project's icon, gets a
/// URL, and the browser fetches the icon from it.
#[tokio::test]
async fn a_projects_icon_is_issued_over_the_socket_and_fetched_over_http() {
    let workspace = Workspace::with(&[]);
    workspace.put("public/favicon.svg", ICON);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let issued = favicon_url(&mut client, &workspace).await;
    let url = issued["relativeUrl"].as_str().expect("a relative url");
    assert!(url.starts_with("/api/assets/"), "{url}");
    assert!(url.ends_with("/favicon.svg"), "{url}");
    assert!(issued["expiresAt"].as_i64().is_some_and(|at| at > 0));

    let fetched = server.get(url).await;
    assert_eq!(fetched.status, 200);
    assert_eq!(fetched.text, ICON);
    assert_eq!(
        fetched.header("content-type").as_deref(),
        Some("image/svg+xml")
    );
    // A developer's own project, not a public file, and no longer cacheable
    // than the token that names it is honoured.
    assert_eq!(
        fetched.header("cache-control").as_deref(),
        Some("private, max-age=3600")
    );
    assert_eq!(
        fetched.header("x-content-type-options").as_deref(),
        Some("nosniff")
    );

    client.close().await;
    server.stop().await;
}

/// The ordinary case — most projects have no icon — and the reason it is not an
/// error: the client is handed a URL whose *filename* is the marker, recognises
/// it, and draws its folder without making a request at all.
#[tokio::test]
async fn a_project_without_an_icon_is_answered_rather_than_refused() {
    let workspace = Workspace::with(&[]);
    workspace.put("README.md", "# no icon here\n");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let issued = favicon_url(&mut client, &workspace).await;
    let url = issued["relativeUrl"].as_str().expect("a relative url");
    assert!(url.ends_with("/project-favicon-missing"), "{url}");

    // And a client that fetches it anyway is asking for a file that is not
    // there.
    assert_eq!(server.get(url).await.status, 404);

    client.close().await;
    server.stop().await;
}

/// The route trusts the token and nothing else, so a token that has been
/// touched is a 404 — not a 400, and not a different 404 from a missing file.
#[tokio::test]
async fn a_url_nobody_signed_is_not_served() {
    let workspace = Workspace::with(&[]);
    workspace.put("favicon.svg", ICON);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let issued = favicon_url(&mut client, &workspace).await;
    let url = issued["relativeUrl"].as_str().expect("a relative url");
    assert_eq!(server.get(url).await.status, 200);

    // Split the token off the filename *first*: the filename ends in `.svg`,
    // so splitting the whole URL on the last dot would tamper with that
    // instead — which the route ignores, and the assertion would pass while
    // testing nothing.
    let (token, name) = url
        .trim_start_matches("/api/assets/")
        .split_once('/')
        .expect("a token and a name");
    let (payload, signature) = token.split_once('.').expect("a signed token");
    let flipped: String = signature
        .chars()
        .enumerate()
        // Both branches, so the flip changes a digit whatever the signature
        // happened to start with — one that only rewrote non-zeroes would pass
        // for fifteen keys in sixteen and test nothing for the other.
        .map(|(at, digit)| match (at, digit) {
            (0, '0') => '1',
            (0, _) => '0',
            _ => digit,
        })
        .collect();
    assert_ne!(flipped, signature, "the flip has to change something");
    assert_eq!(
        server
            .get(&format!("/api/assets/{payload}.{flipped}/{name}"))
            .await
            .status,
        404
    );
    assert_eq!(server.get("/api/assets/nonsense/favicon.svg").await.status, 404);

    client.close().await;
    server.stop().await;
}

/// The filename is decoration. The claims say which file this is, so a request
/// that renames it in the URL still gets the icon — and cannot get anything
/// else by asking for it there.
#[tokio::test]
async fn the_filename_in_the_url_chooses_nothing() {
    let workspace = Workspace::with(&[]);
    workspace.put("favicon.svg", ICON);
    workspace.put("secret.svg", "<svg>not this one</svg>");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let issued = favicon_url(&mut client, &workspace).await;
    let url = issued["relativeUrl"].as_str().expect("a relative url");
    let token = url
        .trim_start_matches("/api/assets/")
        .split_once('/')
        .expect("a token and a name")
        .0
        .to_string();

    let renamed = server.get(&format!("/api/assets/{token}/secret.svg")).await;
    assert_eq!(renamed.status, 200);
    assert_eq!(renamed.text, ICON);

    client.close().await;
    server.stop().await;
}

/// The two resources this method does not answer for. Refused by name and with
/// the sentence `tools/ui-driver/surface-walk.mjs` counts, because a control
/// that reaches one of these still does nothing — PARITY-LEDGER M7.
#[tokio::test]
async fn the_resources_this_server_does_not_issue_are_refused_by_name() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for resource in [
        json!({"_tag": "attachment", "attachmentId": "a1"}),
        json!({"_tag": "workspace-file", "threadId": "t1", "path": "shot.png"}),
    ] {
        let tag = resource["_tag"].as_str().expect("a tag").to_string();
        let refused = create_url(&mut client, resource).await;
        let error = refused.expect_declared("EnvironmentAuthorizationError");

        let message = error["message"].as_str().expect("a message");
        assert!(
            message.contains("Method not implemented by this server"),
            "{message}"
        );
        assert!(message.contains(&tag), "{message}");
    }

    client.close().await;
    server.stop().await;
}

/// Two calls for one project answer with two different URLs — the expiry moves
/// — and both work. The key is loaded per call, so this is the check that it is
/// the *same* key each time; a fresh one would make the older URL a 404.
#[tokio::test]
async fn a_second_url_for_the_same_project_does_not_invalidate_the_first() {
    let workspace = Workspace::with(&[]);
    workspace.put("favicon.svg", ICON);

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = favicon_url(&mut client, &workspace).await;
    let second = favicon_url(&mut client, &workspace).await;

    for issued in [&first, &second] {
        let url = issued["relativeUrl"].as_str().expect("a relative url");
        assert_eq!(server.get(url).await.status, 200, "{url}");
    }

    client.close().await;
    server.stop().await;
}
