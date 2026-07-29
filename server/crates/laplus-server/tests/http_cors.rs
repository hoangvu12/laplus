//! What a page on another origin is allowed to read, and what still refuses it.
//!
//! Ticket 02 of the headless-Linux effort. The desktop window's page is served
//! by its *own* server on `http://127.0.0.1:4773`, so every call it makes to a
//! **remote** laplus is cross-origin — and until this ticket the attempt died at
//! the first of them, `GET /.well-known/t3/environment`, with the user shown
//! "could not reach the backend" for a server that had answered fine.
//!
//! The expected values here are `pingdotgg/t3code:apps/server/src/httpCors.ts`
//! verbatim, which is the independent source of truth for them: the client is
//! built from the same contract upstream's server is, so the header set is not
//! this server's to choose.

mod harness;

use harness::{ClientIdentity, HttpResponse, TestServer, DESKTOP_WINDOW_ORIGIN};
use serde_json::json;

/// Every route a cross-origin browser calls, and the method it calls it with.
///
/// The list is the client's rather than this server's:
/// `preparePairingRegistration` walks the descriptor and `/oauth/token` in
/// order, and the rest are what a registered remote connection reads on every
/// load.
///
/// `/api/auth/pairing-token` and the two pairing-link routes are deliberately
/// **not** here. They are how Settings mints and revokes a code for the backend
/// it is sitting on, same-origin, and a remote environment's Settings is a
/// question this ticket does not ask.
///
/// **This list mirrors the `cross_origin(…)` calls in `crate::server` by hand**,
/// and nothing makes it do so — an eighth route added there and not here leaves
/// this test quietly one short. The alternative is exporting the router's route
/// table for a test to read, which would put a seam in the server to describe
/// itself; the mirror is the cheaper of two imperfect things, said out loud.
const CROSS_ORIGIN_ROUTES: &[(&str, &str)] = &[
    ("GET", "/.well-known/t3/environment"),
    ("POST", "/oauth/token"),
    ("GET", "/api/auth/session"),
    ("POST", "/api/auth/browser-session"),
    ("POST", "/api/auth/websocket-ticket"),
    ("GET", "/api/orchestration/shell"),
    ("GET", "/api/orchestration/threads/a-thread"),
];

/// The three headers, and the fourth that must never appear.
fn assert_a_browser_may_read_this(response: &HttpResponse, what: &str) {
    assert_eq!(
        response.header("access-control-allow-origin").as_deref(),
        Some("*"),
        "{what} is unreadable by any page not on this server's own origin: {}",
        response.head
    );
    assert_eq!(
        response.header("access-control-allow-methods").as_deref(),
        Some("GET, POST, OPTIONS"),
        "{what} does not name the methods the client uses: {}",
        response.head
    );
    assert_eq!(
        response.header("access-control-allow-headers").as_deref(),
        Some("authorization, b3, traceparent, content-type, dpop"),
        "{what} does not name the headers the client sends: {}",
        response.head
    );
    // Invalid beside `*`, and a browser rejects the whole response for it. The
    // remote path is bearer-based end to end, so there is nothing it would buy:
    // `bootstrapRemoteBearerSession` stores an `access_token`, and the cookie is
    // for the same-origin case only.
    assert_eq!(
        response.header("access-control-allow-credentials"),
        None,
        "{what} sends Allow-Credentials, which is invalid with `*`: {}",
        response.head
    );
}

/// The first call `preparePairingRegistration` makes, and the one the whole
/// attempt used to die at. Unauthenticated by design — a client that holds
/// nothing has to be able to discover what it is talking to — so the response
/// was always correct and simply never reached the page.
#[tokio::test]
async fn the_descriptor_can_be_read_by_a_page_on_another_origin() {
    let server = TestServer::start().await;

    let response = server.get("/.well-known/t3/environment").await;

    assert_eq!(response.status, 200);
    assert_a_browser_may_read_this(&response, "the environment descriptor");

    server.stop().await;
}

/// Without this, nothing above matters. A JSON body and an `Authorization`
/// header each force a preflight, so the browser asks `OPTIONS` first and never
/// sends the real request if that is refused — and this router registered no
/// `OPTIONS` handler anywhere, so every one of them came back `405` from the
/// `MethodRouter`'s own default.
///
/// **Answered without a credential, deliberately.** A preflight carries none —
/// it is the browser asking on the page's behalf, before the page's request
/// exists — so a check here would refuse every cross-origin call in advance.
#[tokio::test]
async fn every_cross_origin_route_answers_a_preflight_without_a_credential() {
    let server = TestServer::start().await;

    for (method, path) in CROSS_ORIGIN_ROUTES {
        let response = server.preflight(method, path).await;

        // A browser needs only a 2xx here; 204 is the one this server sends,
        // pinned because both the handler's doc comment and the ticket say so and
        // an assertion on a range would let either quietly become wrong.
        assert_eq!(
            response.status, 204,
            "the preflight for {method} {path} was answered {}: {}",
            response.status, response.head
        );
        assert_a_browser_may_read_this(&response, &format!("the preflight for {method} {path}"));
    }

    server.stop().await;
}

/// The one route that is deliberately left out, now pinned rather than described
/// in a comment.
///
/// A WebSocket handshake is not governed by CORS at all — the client reaches
/// `/ws` with a `?wsTicket=` precisely because a browser cannot set a header on
/// one — so the header buys a refused caller nothing here. What it would cost is
/// that any page on any origin could read the 401 it provoked, and the argument
/// that admits it on the routes above ("the refused request is the one being
/// helped") does not reach this one.
///
/// The upgrade is spoken by hand because that is the only way to see a 401 from
/// it: the `WebSocketUpgrade` extractor refuses a plain `GET` before the handler
/// that checks the credential ever runs.
#[tokio::test]
async fn the_socket_upgrade_still_lets_no_page_read_its_refusal() {
    let server = TestServer::start().await;

    let head = server
        .raw_upgrade(&format!(
            "GET /ws HTTP/1.1\r\n\
             Host: {addr}\r\n\
             Connection: Upgrade\r\n\
             Upgrade: websocket\r\n\
             Origin: {DESKTOP_WINDOW_ORIGIN}\r\n\
             Sec-WebSocket-Version: 13\r\n\
             Sec-WebSocket-Key: sXlZ+AnHRboR6K8AWi8sxw==\r\n\r\n",
            addr = server.addr(),
        ))
        .await;

    assert!(
        head.starts_with("HTTP/1.1 401 "),
        "an upgrade presenting nothing should be refused, got: {head}"
    );
    assert!(
        !head.to_lowercase().contains("access-control-"),
        "the upgrade's refusal must carry no CORS header: {head}"
    );

    server.stop().await;
}

/// The reason these headers go on the refusals too, and not only on the answers.
///
/// A remote environment whose bearer has expired refuses the snapshot with a
/// 401, and the client's whole recovery — re-pair, or say so — depends on
/// reading which refusal it was. Without the header the page sees a CORS error
/// instead, which is indistinguishable from the server being down and is what
/// "could not reach the backend" was.
#[tokio::test]
async fn a_refused_cross_origin_request_is_still_readable_by_the_page() {
    let server = TestServer::start().await;

    let response = server
        .get_as("/api/orchestration/shell", &ClientIdentity::anonymous())
        .await;

    assert_eq!(response.status, 401);
    assert_a_browser_may_read_this(&response, "a 401 from the shell snapshot");
    // The body it may now read: the same typed refusal the socket upgrade
    // sends, so the client can tell a missing credential from a dead server.
    assert_eq!(response.body["_tag"], json!("EnvironmentAuthInvalidError"));
    assert_eq!(response.body["reason"], json!("missing_credential"));

    server.stop().await;
}
