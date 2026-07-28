//! The UI, served by the server the UI talks to.
//!
//! Ticket 23's "the UI is served from the embedded application rather than a
//! development server", driven at the wire. The bundle here is four files
//! standing in for four hundred — what is under test is the policy in
//! `laplus_server::ui` and its route into `axum`, not the vendored assets,
//! which are the shell's and are the same bytes whatever this says about them.
//!
//! The load-bearing test is the first one. Everything else about this design
//! follows from the page and the socket sharing an origin.

mod harness;

use harness::TestServer;
use laplus_server::ui::Assets;
use serde_json::json;

const PAGE: &[u8] = b"<!doctype html><title>laplus</title><div id=root></div>";
const SCRIPT: &[u8] = b"export const app = 1";
const ICON: &[u8] = b"\x00\x00\x01\x00 icon bytes, not text";

/// What the bundle above calls itself. A number this crate's own version can
/// never be, which is the point of it — see the version test below.
const BUNDLE_VERSION: &str = "0.0.28";

fn bundle() -> Assets {
    Assets::from_static(
        &[
            ("index.html", PAGE),
            ("assets/index-a1b2c3.js", SCRIPT),
            ("favicon.ico", ICON),
        ],
        BUNDLE_VERSION,
    )
}

/// The whole reason the assets are served from here rather than from Tauri's
/// own scheme handler: the page and the socket are the same origin, so the
/// upgrade the webview makes is one [`laplus_server::auth`] already accepts,
/// and the UI's relative boot fetches reach this server without the bundle
/// being rebuilt to know where it is.
#[tokio::test]
async fn the_page_and_the_socket_share_an_origin() {
    let server = TestServer::start_serving(bundle()).await;
    let origin = format!("http://{}", server.addr());

    let page = server.get("/").await;
    assert_eq!(page.status, 200);
    assert_eq!(page.text, String::from_utf8_lossy(PAGE));

    // The same origin the window will report, presented at the upgrade.
    let mut client = server
        .connect_as(server.browser().with_origin(&origin))
        .await
        .expect("the window's own origin is accepted at the upgrade");
    let config = client.call("server.getConfig", json!({})).await;
    assert!(config.expect_success()["environment"]["environmentId"].is_string());

    client.close().await;
    server.stop().await;
}

#[tokio::test]
async fn the_root_is_the_page_and_it_is_html() {
    let server = TestServer::start_serving(bundle()).await;

    for path in ["/", "/index.html"] {
        let response = server.get(path).await;
        assert_eq!(response.status, 200, "{path}");
        assert_eq!(
            response.header("content-type").as_deref(),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
        assert_eq!(response.text, String::from_utf8_lossy(PAGE), "{path}");
    }

    server.stop().await;
}

/// The UI routes in the browser, so a window opened on `/settings` — or
/// reloaded there — asks this server for a path it has never heard of. It has
/// to get the app, not a 404.
#[tokio::test]
async fn a_route_the_client_owns_is_answered_with_the_page() {
    let server = TestServer::start_serving(bundle()).await;

    for route in ["/settings", "/projects/1a2b/threads/3c4d"] {
        let response = server.get(route).await;
        assert_eq!(response.status, 200, "{route}");
        assert_eq!(response.text, String::from_utf8_lossy(PAGE), "{route}");
    }

    server.stop().await;
}

/// A script served as a fallback page would be *run* as one. The developer
/// would read a syntax error in a file they never wrote, instead of "that file
/// is not there".
#[tokio::test]
async fn a_missing_file_is_a_404_rather_than_the_page() {
    let server = TestServer::start_serving(bundle()).await;

    let response = server.get("/assets/index-deadbeef.js").await;

    assert_eq!(response.status, 404);
    assert!(!response.text.contains("<!doctype html>"), "{}", response.text);

    server.stop().await;
}

/// Attaching a UI must not change a single answer the client already decodes.
/// The server's own routes win over the fallback; anything under `/api` this
/// server has not implemented stays the plain 404 `http_boot.rs` pins.
///
/// The orchestration routes are here because they are the ones this matters
/// most for. Their paths have no extension, so without a route of their own the
/// fallback would read `/api/orchestration/threads/1a2b` as one of the UI's
/// client-side routes and answer a `fetch` with an HTML page — which is what
/// `laplus_server::ui`'s `SERVER_SURFACE` prefix list is for, and it is checked
/// from both sides.
#[tokio::test]
async fn the_servers_own_answers_are_not_shadowed_by_the_ui() {
    let server = TestServer::start_serving(bundle()).await;

    for path in [
        "/.well-known/t3/environment",
        "/api/auth/session",
        "/api/orchestration/shell",
    ] {
        let response = server.get(path).await;
        assert_eq!(response.status, 200, "{path}");
        assert!(
            response.head.to_lowercase().contains("application/json"),
            "{path} should still be json: {}",
            response.head
        );
    }

    // A thread this server does not hold, and an unimplemented sibling: both
    // 404, and neither is the page.
    for path in [
        "/api/orchestration/threads/1a2b",
        "/api/orchestration/threads/",
        "/api/orchestration/snapshot",
        "/.well-known/openid-configuration",
    ] {
        let response = server.get(path).await;
        assert_eq!(response.status, 404, "{path} should stay a 404");
        assert!(
            !response.text.contains("<!doctype html>"),
            "{path} was answered with the UI: {}",
            response.text
        );
    }

    server.stop().await;
}

/// Content-hashed output may be kept forever; the page that names it may not.
/// This is what makes the second launch quick without ever showing the previous
/// build's page.
#[tokio::test]
async fn the_hashed_bundle_is_cached_and_the_page_is_revalidated() {
    let server = TestServer::start_serving(bundle()).await;

    let script = server.get("/assets/index-a1b2c3.js").await;
    assert_eq!(script.status, 200);
    assert_eq!(
        script.header("content-type").as_deref(),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(
        script.header("cache-control").as_deref(),
        Some("public, max-age=31536000, immutable")
    );

    assert_eq!(
        server.get("/").await.header("cache-control").as_deref(),
        Some("no-cache")
    );

    server.stop().await;
}

/// Not every asset is text. Checked by length rather than by content, because
/// the harness reads a response as a string and would mangle the bytes on the
/// way — which is exactly the failure this is guarding against.
#[tokio::test]
async fn a_binary_asset_is_served_whole() {
    let server = TestServer::start_serving(bundle()).await;

    let icon = server.get("/favicon.ico").await;

    assert_eq!(icon.status, 200);
    assert_eq!(icon.header("content-type").as_deref(), Some("image/x-icon"));
    assert_eq!(
        icon.header("content-length"),
        Some(ICON.len().to_string()),
        "the icon was not sent whole"
    );

    server.stop().await;
}

/// Attaching a UI must not turn "there is nothing here" into "there is
/// something here but not for you". `http_boot.rs` calls an unimplemented route
/// a **plain 404**, and a `POST` to one is still unimplemented — a 405 would
/// tell a client the path exists, which is both untrue and a different thing to
/// handle.
#[tokio::test]
async fn a_path_the_ui_does_not_have_is_a_404_whatever_the_method() {
    let server = TestServer::start_serving(bundle()).await;

    for method in ["POST", "PUT", "DELETE"] {
        let response = server
            .raw_request(&format!(
                "{method} /settings HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                server.addr()
            ))
            .await;
        assert!(
            response.starts_with("HTTP/1.1 404"),
            "{method} /settings should be a 404, got: {}",
            response.lines().next().unwrap_or_default()
        );
    }

    server.stop().await;
}

/// A server with no UI is the server every other test file drives, and the one
/// the plain binary runs. `/` stays a 404 there.
#[tokio::test]
async fn a_server_with_no_ui_still_answers_nothing_at_the_root() {
    let server = TestServer::start().await;

    assert_eq!(server.get("/").await.status, 404);
    assert_eq!(server.get("/settings").await.status, 404);

    server.stop().await;
}

/// Ticket 26: the version a server reports is the version of the UI it is
/// serving, so the client — which compares that number against the one compiled
/// into the page this same server just sent — finds nothing to warn about.
///
/// Both answers are checked because the UI reads both: `/.well-known/t3/environment`
/// on boot, and `server.getConfig` once the socket is open. A skew banner raised
/// by whichever of the two disagreed would be no better than the one this
/// removes.
#[tokio::test]
async fn a_server_serving_a_ui_reports_that_uis_version_as_its_own() {
    let server = TestServer::start_serving(bundle()).await;

    let over_http = server.get("/.well-known/t3/environment").await;
    assert_eq!(over_http.body["serverVersion"], json!(BUNDLE_VERSION));

    let mut client = server.connect().await;
    let config = client.call("server.getConfig", json!({})).await.expect_success();
    assert_eq!(config["environment"]["serverVersion"], json!(BUNDLE_VERSION));

    client.close().await;
    server.stop().await;
}

/// And a server that brought no UI keeps its own version, which is the answer
/// that means something for the plain binary — whatever is pointed at it was
/// built somewhere else and a difference there is a real one.
#[tokio::test]
async fn a_server_with_no_ui_reports_its_own_version() {
    let server = TestServer::start().await;

    let descriptor = server.get("/.well-known/t3/environment").await;

    assert_eq!(
        descriptor.body["serverVersion"],
        json!(env!("CARGO_PKG_VERSION"))
    );

    server.stop().await;
}
