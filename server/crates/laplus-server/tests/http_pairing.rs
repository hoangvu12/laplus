//! Pairing, at the wire.
//!
//! Ticket 73's second piece: the five routes `EnvironmentAuthHttpApi` declares,
//! driven the way the client drives them. The storage layer underneath has its
//! own tests in `store.rs` — including both concurrency tests — and what is
//! under test here is the part those cannot reach: that the paths are the
//! contract's, that the bodies decode as the schemas the client is built from,
//! and that a refusal wears the status its body claims.
//!
//! **The load-bearing test is the first one.** Four requests in the order a
//! phone makes them, ending in an open socket. Everything below it is one step
//! of that chain going wrong.
//!
//! What is *not* here: anything about a non-loopback origin. That is ticket
//! 73's third piece, and until it lands every route in this file is
//! loopback-only — see `crate::server`'s note above `token_exchange`. A test
//! asserting today's refusal of a tunnel origin would be a test that has to be
//! deleted rather than one that has to pass.

mod harness;

use harness::{ClientIdentity, TestServer};
use serde_json::{json, Value};

/// The three literals `AuthTokenExchangeRequest` pins, as the client encodes
/// them. Percent-encoded here for the same reason the client's form encoder
/// does it: a `:` in a form value is legal unencoded and this is what actually
/// goes over the wire.
const GRANT_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Atoken-exchange";
const BOOTSTRAP_TOKEN_TYPE: &str = "urn%3At3%3Aparams%3Aoauth%3Atoken-type%3Aenvironment-bootstrap";
const ACCESS_TOKEN_TYPE: &str = "urn%3Aietf%3Aparams%3Aoauth%3Atoken-type%3Aaccess_token";

/// A token-exchange body carrying `credential`, and nothing optional.
fn exchange(credential: &str) -> String {
    format!(
        "grant_type={GRANT_TYPE}&subject_token={credential}\
         &subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type={ACCESS_TOKEN_TYPE}"
    )
}

/// The same, asking for a particular scope list.
fn exchange_for(credential: &str, scope: &str) -> String {
    format!("{}&scope={}", exchange(credential), scope.replace(' ', "+"))
}

fn text(value: &Value) -> &str {
    value.as_str().expect("a string")
}

/// Mint a code, trade it for a bearer, trade that for a ticket, open the socket.
///
/// The whole ticket in one test. Each step's output is the next step's input,
/// so nothing here is arranged — if the credential in step two is not the one
/// step one minted, step two fails.
#[tokio::test]
async fn a_minted_code_carries_a_client_all_the_way_to_an_open_socket() {
    let server = TestServer::start().await;

    let minted = server
        .post_json("/api/auth/pairing-token", &json!({ "label": "Phone" }))
        .await;
    assert_eq!(minted.status, 200, "{}", minted.text);
    let credential = text(&minted.body["credential"]).to_string();

    let exchanged = server.post_form("/oauth/token", &exchange(&credential)).await;
    assert_eq!(exchanged.status, 200, "{}", exchanged.text);
    let bearer = text(&exchanged.body["access_token"]).to_string();

    let ticketed = server
        .post_as(
            "/api/auth/websocket-ticket",
            &ClientIdentity::anonymous().with_bearer(&bearer),
        )
        .await;
    assert_eq!(ticketed.status, 200, "{}", ticketed.text);
    let ticket = text(&ticketed.body["ticket"]).to_string();

    let mut client = server
        .connect_as(ClientIdentity::anonymous().with_ticket(&ticket))
        .await
        .expect("a ticket minted by this server opens a socket");
    let config = client.call("server.getConfig", json!({})).await;
    assert!(config.expect_success()["environment"]["environmentId"].is_string());

    client.close().await;
    server.stop().await;
}

/// The pair code the user reads off one screen and types into another: twelve
/// characters from the alphabet that has no `0`/`O` and no `1`/`I` in it.
///
/// The alphabet itself is pinned in `pairing.rs`. What this adds is that the
/// code which reaches the *client* is that one — a route that hashed it, or
/// truncated it, or sent the row id by mistake would pass every unit test.
#[tokio::test]
async fn the_minted_code_is_twelve_characters_the_user_can_retype() {
    let server = TestServer::start().await;

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    assert_eq!(minted.status, 200, "{}", minted.text);

    let credential = text(&minted.body["credential"]);
    assert_eq!(credential.len(), 12, "{credential}");
    assert!(
        credential
            .chars()
            .all(|character| "23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(character)),
        "{credential} is not from the pairing alphabet"
    );

    server.stop().await;
}

/// Single use, over HTTP rather than over the storage layer. The second attempt
/// is a 401 and not a second bearer.
#[tokio::test]
async fn a_code_exchanges_once_and_the_second_attempt_is_refused() {
    let server = TestServer::start().await;

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let credential = text(&minted.body["credential"]).to_string();

    let first = server.post_form("/oauth/token", &exchange(&credential)).await;
    assert_eq!(first.status, 200, "{}", first.text);

    let second = server.post_form("/oauth/token", &exchange(&credential)).await;
    assert_eq!(second.status, 401, "{}", second.text);
    assert_eq!(second.body["_tag"], "EnvironmentAuthInvalidError");
    assert_eq!(second.body["reason"], "invalid_credential");

    server.stop().await;
}

/// A code nobody minted. The same 401 as a spent one, because the contract's
/// `EnvironmentAuthInvalidReason` has no member that tells them apart — the
/// distinction lives in the log, which is where it can do some good.
#[tokio::test]
async fn a_code_this_server_never_minted_is_refused() {
    let server = TestServer::start().await;

    let refused = server
        .post_form("/oauth/token", &exchange("ZZZZZZZZZZZZ"))
        .await;
    assert_eq!(refused.status, 401, "{}", refused.text);
    assert_eq!(refused.body["code"], "auth_invalid");

    server.stop().await;
}

/// No `subject_token` at all is the union's *other* member. A phone whose
/// storage was cleared and a phone whose code was already spent are different
/// things to tell the user, and this is the only place the difference is
/// carried.
#[tokio::test]
async fn a_token_exchange_carrying_no_code_says_the_credential_is_missing() {
    let server = TestServer::start().await;

    let refused = server
        .post_form(
            "/oauth/token",
            &format!(
                "grant_type={GRANT_TYPE}&subject_token=\
                 &subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type={ACCESS_TOKEN_TYPE}"
            ),
        )
        .await;
    assert_eq!(refused.status, 401, "{}", refused.text);
    assert_eq!(refused.body["reason"], "missing_credential");

    server.stop().await;
}

/// A grant this server does not implement. The contract types all three of
/// these as literals, so a body naming another one is a client built against a
/// different server rather than a user doing anything.
#[tokio::test]
async fn a_token_exchange_naming_another_grant_is_refused_as_a_bad_request() {
    let server = TestServer::start().await;

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let credential = text(&minted.body["credential"]).to_string();

    for body in [
        format!("grant_type=authorization_code&subject_token={credential}&subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type={ACCESS_TOKEN_TYPE}"),
        format!("grant_type={GRANT_TYPE}&subject_token={credential}&subject_token_type=urn%3Asomething%3Aelse&requested_token_type={ACCESS_TOKEN_TYPE}"),
        format!("grant_type={GRANT_TYPE}&subject_token={credential}&subject_token_type={BOOTSTRAP_TOKEN_TYPE}&requested_token_type=urn%3Asomething%3Aelse"),
        String::new(),
    ] {
        let refused = server.post_form("/oauth/token", &body).await;
        assert_eq!(refused.status, 400, "{}", refused.text);
        assert_eq!(refused.body["_tag"], "EnvironmentRequestInvalidError");
        assert_eq!(refused.body["reason"], "invalid_command");
    }

    // And the code was not spent by any of them — the check runs first.
    let exchanged = server.post_form("/oauth/token", &exchange(&credential)).await;
    assert_eq!(exchanged.status, 200, "{}", exchanged.text);

    server.stop().await;
}

/// The scope a client asks for, honoured and reported back.
///
/// `AuthAccessTokenResult.scope` is what the client records as its own
/// permissions, so a server that granted one thing and said another would be
/// lying to the UI that displays it.
#[tokio::test]
async fn an_exchange_reports_the_scopes_it_granted() {
    let server = TestServer::start().await;

    let minted = server
        .post_json(
            "/api/auth/pairing-token",
            &json!({ "scopes": ["orchestration:read", "orchestration:operate", "relay:read"] }),
        )
        .await;
    assert_eq!(minted.status, 200, "{}", minted.text);
    let credential = text(&minted.body["credential"]).to_string();

    // Asking for nothing asks for everything the code granted.
    let all = server.post_form("/oauth/token", &exchange(&credential)).await;
    assert_eq!(all.status, 200, "{}", all.text);
    assert_eq!(
        all.body["scope"],
        "orchestration:read orchestration:operate relay:read"
    );
    assert_eq!(all.body["token_type"], "Bearer");
    assert_eq!(
        all.body["issued_token_type"],
        "urn:ietf:params:oauth:token-type:access_token"
    );
    // Thirty days, as a number of seconds the client counts down from. Asserted
    // as an ordering rather than a value: this is the database's clock, and a
    // test that pinned the exact second would be a test about how long the two
    // statements took.
    let expires_in = all.body["expires_in"].as_i64().expect("a number");
    assert!(
        expires_in > 29 * 24 * 60 * 60 && expires_in <= 30 * 24 * 60 * 60,
        "{expires_in} is not thirty days"
    );

    server.stop().await;
}

/// Narrowing works and over-reaching does not. A code minted for three scopes
/// can be spent for one of them and cannot be spent for a fourth.
#[tokio::test]
async fn an_exchange_can_narrow_the_grant_but_not_widen_it() {
    let server = TestServer::start().await;

    let minted = server
        .post_json(
            "/api/auth/pairing-token",
            &json!({ "scopes": ["orchestration:read", "orchestration:operate", "relay:read"] }),
        )
        .await;
    let narrow = text(&minted.body["credential"]).to_string();

    let narrowed = server
        .post_form(
            "/oauth/token",
            &exchange_for(&narrow, "orchestration:read relay:read"),
        )
        .await;
    assert_eq!(narrowed.status, 200, "{}", narrowed.text);
    assert_eq!(narrowed.body["scope"], "orchestration:read relay:read");

    let minted = server
        .post_json(
            "/api/auth/pairing-token",
            &json!({ "scopes": ["orchestration:read"] }),
        )
        .await;
    let narrower = text(&minted.body["credential"]).to_string();

    let refused = server
        .post_form(
            "/oauth/token",
            &exchange_for(&narrower, "orchestration:read access:write"),
        )
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert_eq!(refused.body["reason"], "scope_not_granted");

    server.stop().await;
}

/// A scope outside the contract's eight, at both ends. It has to be refused
/// rather than recorded: `AuthEnvironmentScope` is a literal union on the
/// client, and a pairing link carrying `orchestration:destroy` back would fail
/// the decode that renders the whole Settings panel.
#[tokio::test]
async fn a_scope_the_contract_does_not_declare_is_refused_at_both_ends() {
    let server = TestServer::start().await;

    let refused = server
        .post_json(
            "/api/auth/pairing-token",
            &json!({ "scopes": ["orchestration:destroy"] }),
        )
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert_eq!(refused.body["reason"], "invalid_scope");

    // An empty list and a repeated one, which the reference server refuses in
    // the same breath and for the same reason: the client builds this from a
    // checkbox list, so either means the list is wrong.
    for scopes in [json!([]), json!(["relay:read", "relay:read"])] {
        let refused = server
            .post_json("/api/auth/pairing-token", &json!({ "scopes": scopes }))
            .await;
        assert_eq!(refused.status, 400, "{}", refused.text);
        assert_eq!(refused.body["reason"], "invalid_scope");
    }

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let credential = text(&minted.body["credential"]).to_string();
    let refused = server
        .post_form(
            "/oauth/token",
            &exchange_for(&credential, "orchestration:destroy"),
        )
        .await;
    assert_eq!(refused.status, 400, "{}", refused.text);
    assert_eq!(refused.body["reason"], "invalid_scope");

    server.stop().await;
}

/// What Settings shows: the codes that can still be handed to a phone.
///
/// The list is `AuthPairingLink`, which carries the code in plaintext so the
/// user can re-read one they minted a minute ago. That is the decision recorded
/// in `pairing.rs` and this is the behaviour that depends on it.
#[tokio::test]
async fn the_link_list_shows_live_codes_and_drops_spent_ones() {
    let server = TestServer::start().await;

    let kept = server
        .post_json("/api/auth/pairing-token", &json!({ "label": "Phone" }))
        .await;
    let spent = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let spent_credential = text(&spent.body["credential"]).to_string();
    server.post_form("/oauth/token", &exchange(&spent_credential)).await;

    let listed = server.get("/api/auth/pairing-links").await;
    assert_eq!(listed.status, 200, "{}", listed.text);
    let links = listed.body.as_array().expect("an array");
    assert_eq!(links.len(), 1, "only the unspent code is listed: {links:?}");

    let link = &links[0];
    assert_eq!(link["id"], kept.body["id"]);
    assert_eq!(link["credential"], kept.body["credential"]);
    assert_eq!(link["label"], "Phone");
    assert_eq!(link["subject"], "one-time-token");
    assert_eq!(
        link["scopes"],
        json!([
            "orchestration:read",
            "orchestration:operate",
            "terminal:operate",
            "review:write",
            "relay:read"
        ]),
        "an unspecified scope list is the standard preset"
    );
    for timestamp in ["createdAt", "expiresAt"] {
        assert!(
            text(&link[timestamp]).ends_with('Z'),
            "{timestamp} is not an ISO instant: {}",
            link[timestamp]
        );
    }

    server.stop().await;
}

/// A revoked code cannot be spent, and leaves the list.
#[tokio::test]
async fn a_revoked_code_is_gone_from_the_list_and_cannot_be_spent() {
    let server = TestServer::start().await;

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let id = text(&minted.body["id"]).to_string();
    let credential = text(&minted.body["credential"]).to_string();

    let revoked = server
        .post_json("/api/auth/pairing-links/revoke", &json!({ "id": id }))
        .await;
    assert_eq!(revoked.status, 200, "{}", revoked.text);
    assert_eq!(revoked.body, json!({ "revoked": true }));

    let listed = server.get("/api/auth/pairing-links").await;
    assert_eq!(listed.body.as_array().expect("an array").len(), 0);

    let refused = server.post_form("/oauth/token", &exchange(&credential)).await;
    assert_eq!(refused.status, 401, "{}", refused.text);

    server.stop().await;
}

/// Revoking twice, and revoking something that was never there.
///
/// `revoked: false` rather than a 404, because the contract gives this route no
/// `EnvironmentResourceNotFoundError` — and because the state the caller wanted
/// holds either way: that code cannot be spent.
#[tokio::test]
async fn revoking_what_is_already_gone_reports_that_nothing_changed() {
    let server = TestServer::start().await;

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let id = text(&minted.body["id"]).to_string();

    server
        .post_json("/api/auth/pairing-links/revoke", &json!({ "id": id }))
        .await;
    let again = server
        .post_json("/api/auth/pairing-links/revoke", &json!({ "id": id }))
        .await;
    assert_eq!(again.body, json!({ "revoked": false }));

    let nothing = server
        .post_json(
            "/api/auth/pairing-links/revoke",
            &json!({ "id": "no-such-link" }),
        )
        .await;
    assert_eq!(nothing.status, 200, "{}", nothing.text);
    assert_eq!(nothing.body, json!({ "revoked": false }));

    server.stop().await;
}

/// The socket ticket is single use: it opens one socket and not two.
///
/// A ticket travels in a query string, which is the one place a credential in
/// this chain lands in a log. Five minutes and one upgrade is what makes that
/// survivable.
#[tokio::test]
async fn a_socket_ticket_opens_one_socket_and_not_a_second() {
    let server = TestServer::start().await;

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let credential = text(&minted.body["credential"]).to_string();
    let exchanged = server.post_form("/oauth/token", &exchange(&credential)).await;
    let bearer = text(&exchanged.body["access_token"]).to_string();

    let ticketed = server
        .post_as(
            "/api/auth/websocket-ticket",
            &ClientIdentity::anonymous().with_bearer(&bearer),
        )
        .await;
    let ticket = text(&ticketed.body["ticket"]).to_string();
    assert!(
        text(&ticketed.body["expiresAt"]).ends_with('Z'),
        "expiresAt is not an ISO instant: {}",
        ticketed.body["expiresAt"]
    );

    let client = server
        .connect_as(ClientIdentity::anonymous().with_ticket(&ticket))
        .await
        .expect("the first upgrade spends the ticket");
    client.close().await;
    server.await_live_connections(0).await;

    // The bearer is still good, so a second socket is one request away — what
    // is spent is the ticket, not the pairing.
    let again = server
        .post_as(
            "/api/auth/websocket-ticket",
            &ClientIdentity::anonymous().with_bearer(&bearer),
        )
        .await;
    assert_eq!(again.status, 200, "{}", again.text);
    assert_ne!(text(&again.body["ticket"]), ticket);

    server.stop().await;
}

/// A bearer that names no session, and no bearer at all. Two members of one
/// union, and the client tells the user different things about them.
#[tokio::test]
async fn minting_a_socket_ticket_needs_a_bearer_that_verifies() {
    let server = TestServer::start().await;

    let missing = server
        .post_as("/api/auth/websocket-ticket", &ClientIdentity::anonymous())
        .await;
    assert_eq!(missing.status, 401, "{}", missing.text);
    assert_eq!(missing.body["reason"], "missing_credential");

    let invalid = server
        .post_as(
            "/api/auth/websocket-ticket",
            &ClientIdentity::anonymous().with_bearer("not-a-session"),
        )
        .await;
    assert_eq!(invalid.status, 401, "{}", invalid.text);
    assert_eq!(invalid.body["reason"], "invalid_credential");
    assert_eq!(invalid.body["_tag"], "EnvironmentAuthInvalidError");

    server.stop().await;
}

/// Every refusal these routes make carries a correlation id, and no two share
/// one. It is the handle that makes a failure the user saw findable in the log,
/// and two refusals sharing one would be two events that cannot be told apart.
#[tokio::test]
async fn every_refusal_carries_its_own_trace_id() {
    let server = TestServer::start().await;

    let mut seen: Vec<String> = Vec::new();
    for refused in [
        server.post_form("/oauth/token", "grant_type=nonsense").await,
        server.post_form("/oauth/token", &exchange("ZZZZZZZZZZZZ")).await,
        server
            .post_as("/api/auth/websocket-ticket", &ClientIdentity::anonymous())
            .await,
        server
            .post_json("/api/auth/pairing-token", &json!({ "scopes": [] }))
            .await,
    ] {
        let trace_id = text(&refused.body["traceId"]).to_string();
        assert_eq!(trace_id.len(), 32, "{trace_id}");
        assert!(trace_id.chars().all(|digit| digit.is_ascii_hexdigit()));
        assert!(!seen.contains(&trace_id), "{trace_id} was reused");
        seen.push(trace_id);
    }

    server.stop().await;
}

/// **The constraint ticket 73 says will bite.** The desktop window keeps
/// working — not by being exempt from the credential check, which is what it
/// used to be, but by holding a credential like everything else.
///
/// The harness pairs itself at startup exactly as the window does: it reads the
/// boot code out of `Server::window_url`'s fragment and trades it for a session.
/// So `server.browser()` here *is* the window's posture, and this test is the
/// one that would fail if the shell could no longer let itself in.
#[tokio::test]
async fn the_window_reaches_these_routes_with_the_credential_it_booted_with() {
    let server = TestServer::start().await;
    let origin = format!("http://{}", server.addr());
    let window = server.browser().with_origin(&origin);

    let minted = server
        .post_json_as("/api/auth/pairing-token", &window, &json!({}))
        .await;
    assert_eq!(minted.status, 200, "{}", minted.text);

    let listed = server.get_as("/api/auth/pairing-links", &window).await;
    assert_eq!(listed.status, 200, "{}", listed.text);
    assert_eq!(
        listed.body.as_array().expect("an array").len(),
        1,
        "the boot grant is not offered as a code to hand to a phone: {}",
        listed.text
    );

    let client = server
        .connect_as(window)
        .await
        .expect("the window upgrades with the session it booted with");
    client.close().await;

    server.stop().await;
}

/// `/api/auth/session` answers what actually verified, which is what tells the
/// window it still has to pair.
///
/// This route used to answer `authenticated: true` to everyone, and that single
/// field is what kept the window off its own socket for the whole of ticket 73:
/// `bootstrapServerAuth` reads it *first* and exchanges its boot credential
/// only if the answer is `false`, so a client that was told it was signed in
/// never opened a session, presented nothing at the upgrade, and reconnected
/// for ever. The same field is what Settings gates the local-environment panel
/// on — `scopes` is absent unless something verified — so the button that mints
/// a pairing code was in a section that could not render either.
///
/// Both halves are asserted here because they are one bug: a probe that lies
/// about being authenticated and a probe that cannot report a scope are the
/// same missing credential check.
#[tokio::test]
async fn the_session_route_reports_what_verified_and_not_a_hardcoded_yes() {
    let server = TestServer::start().await;

    // Nobody. The shape a client sees before it has paired — and `auth` is
    // still there, because `bootstrapMethods` is how it learns what to do next.
    let anonymous = server
        .get_as("/api/auth/session", &ClientIdentity::anonymous())
        .await;
    assert_eq!(anonymous.status, 200, "a probe is not a refusal: {}", anonymous.text);
    assert_eq!(anonymous.body["authenticated"], json!(false));
    assert!(
        anonymous.body.get("scopes").is_none(),
        "nothing verified, so there are no scopes to report: {}",
        anonymous.text
    );
    assert_eq!(
        anonymous.body["auth"]["bootstrapMethods"],
        json!(["one-time-token"]),
        "an unpaired client reads its way in off this response: {}",
        anonymous.text
    );

    // The window, holding the session it booted with. `access:write` is the
    // one scope `canManageLocalBackend` looks for.
    let window = server.browser();
    let session = server.get_as("/api/auth/session", &window).await;
    assert_eq!(session.status, 200, "{}", session.text);
    assert_eq!(session.body["authenticated"], json!(true));
    assert_eq!(session.body["sessionMethod"], json!("browser-session-cookie"));
    let scopes = session.body["scopes"].as_array().expect("scopes");
    assert!(
        scopes.contains(&json!("access:write")),
        "the window boots with administrative scopes and has to be told so: {}",
        session.text
    );

    // A phone that paired for one scope is told that, and not the window's.
    let minted = server
        .post_json_as(
            "/api/auth/pairing-token",
            &window,
            &json!({ "scopes": ["orchestration:read"] }),
        )
        .await;
    let bearer = server
        .post_form(
            "/oauth/token",
            &exchange(text(&minted.body["credential"])),
        )
        .await;
    let phone = ClientIdentity::anonymous().with_bearer(text(&bearer.body["access_token"]));
    let phone_session = server.get_as("/api/auth/session", &phone).await;
    assert_eq!(phone_session.body["authenticated"], json!(true));
    assert_eq!(phone_session.body["sessionMethod"], json!("bearer-access-token"));
    assert_eq!(phone_session.body["scopes"], json!(["orchestration:read"]));

    server.stop().await;
}

/// A minted code reaches the screen that minted it.
///
/// Settings does not read the response to `POST /api/auth/pairing-token`, and
/// it does not call `GET /api/auth/pairing-links` either — both of which this
/// ticket built. It opens `subscribeAuthAccess` and folds the snapshot
/// (`ConnectionsSettings.tsx:1559`). Until that method existed the panel drew
/// "Method not implemented by this server: subscribeAuthAccess" over an empty
/// list, so a code could be minted and never seen, which is the same as not
/// being able to mint one.
///
/// Driven over the socket rather than asserted on the store, because the gap
/// this closes was entirely in which transport the client chose.
#[tokio::test]
async fn a_minted_code_appears_on_the_access_subscription_and_leaves_when_revoked() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe("subscribeAuthAccess", json!({})).await;
    let opening = client.next_event(&subscription).await;
    assert_eq!(opening["version"], json!(1));
    assert_eq!(opening["type"], json!("snapshot"));
    assert_eq!(
        opening["payload"]["pairingLinks"],
        json!([]),
        "the boot grant is filtered out of this list the way it is out of the \
         HTTP one, so a fresh server opens empty: {opening}"
    );

    let minted = server
        .post_json("/api/auth/pairing-token", &json!({ "label": "Phone" }))
        .await;
    assert_eq!(minted.status, 200, "{}", minted.text);

    let announced = client.next_event(&subscription).await;
    let links = announced["payload"]["pairingLinks"]
        .as_array()
        .expect("an array");
    assert_eq!(links.len(), 1, "{announced}");
    assert_eq!(links[0]["label"], json!("Phone"));
    assert_eq!(
        links[0]["credential"],
        minted.body["credential"],
        "the code on the screen has to be the code that was minted: {announced}"
    );

    let revoked = server
        .post_json(
            "/api/auth/pairing-links/revoke",
            &json!({ "id": text(&minted.body["id"]) }),
        )
        .await;
    assert_eq!(revoked.body["revoked"], json!(true), "{}", revoked.text);

    let after = client.next_event(&subscription).await;
    assert_eq!(after["payload"]["pairingLinks"], json!([]), "{after}");

    client.close().await;
    server.stop().await;
}

/// The window survives a reload, which is the whole reason its boot grant is
/// re-usable where a phone's code is not.
///
/// Pressing F5 re-runs the page's bootstrap against the same `#token=` in the
/// address bar. A strictly single-use boot grant would let that happen exactly
/// once and then lock the developer out of their own window.
#[tokio::test]
async fn the_boot_credential_survives_being_spent_so_a_reload_still_opens() {
    let server = TestServer::start().await;

    // Three reloads, each opening a fresh session from the same code.
    for reload in 1..=3 {
        let opened = server
            .post_json(
                "/api/auth/browser-session",
                &json!({ "credential": server.boot_credential() }),
            )
            .await;
        assert_eq!(opened.status, 200, "reload {reload}: {}", opened.text);
        assert_eq!(opened.body["authenticated"], json!(true));
        assert_eq!(opened.body["sessionMethod"], "browser-session-cookie");
        assert!(
            opened.header("set-cookie").is_some_and(|cookie| {
                cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax")
            }),
            "reload {reload} did not set a session cookie: {:?}",
            opened.header("set-cookie")
        );
    }

    server.stop().await;
}

/// A code minted for a phone is the opposite, and the difference is the point:
/// it is read aloud off a screen, so the second use of one is somebody who
/// should not have it.
#[tokio::test]
async fn a_phones_code_opens_one_browser_session_and_not_two() {
    let server = TestServer::start().await;

    let minted = server.post_json("/api/auth/pairing-token", &json!({})).await;
    let credential = text(&minted.body["credential"]).to_string();

    let first = server
        .post_json(
            "/api/auth/browser-session",
            &json!({ "credential": credential }),
        )
        .await;
    assert_eq!(first.status, 200, "{}", first.text);

    let second = server
        .post_json(
            "/api/auth/browser-session",
            &json!({ "credential": credential }),
        )
        .await;
    assert_eq!(second.status, 401, "{}", second.text);
    assert_eq!(second.body["reason"], "invalid_credential");

    server.stop().await;
}

/// A page somewhere else, asking the user's own browser to mint it a pairing
/// code. The five new routes refuse it exactly as the two snapshot routes do,
/// because they share one check.
///
/// This is today's rule, not the end state — a tunnel origin is refused here
/// too, which is the gap ticket 73's third piece closes. What it pins is that
/// the new routes are behind the *same* check as everything else, so that when
/// the check changes they change with it rather than being forgotten.
#[tokio::test]
async fn a_page_on_another_origin_is_refused_by_every_one_of_these_routes() {
    let server = TestServer::start().await;
    let elsewhere = ClientIdentity::anonymous().with_origin("https://evil.example");

    let minted = server
        .post_json_as("/api/auth/pairing-token", &elsewhere, &json!({}))
        .await;
    assert_eq!(minted.status, 401, "{}", minted.text);

    let listed = server.get_as("/api/auth/pairing-links", &elsewhere).await;
    assert_eq!(listed.status, 401, "{}", listed.text);

    let revoked = server
        .post_json_as(
            "/api/auth/pairing-links/revoke",
            &elsewhere,
            &json!({ "id": "anything" }),
        )
        .await;
    assert_eq!(revoked.status, 401, "{}", revoked.text);

    let ticketed = server
        .post_as("/api/auth/websocket-ticket", &elsewhere)
        .await;
    assert_eq!(ticketed.status, 401, "{}", ticketed.text);

    let exchanged = server
        .post_form_as("/oauth/token", &elsewhere, &exchange("ZZZZZZZZZZZZ"))
        .await;
    assert_eq!(exchanged.status, 401, "{}", exchanged.text);

    server.stop().await;
}

/// A body that is not the payload the route declares.
///
/// The two answers differ, and the difference is the contract's rather than a
/// choice: `pairing-token` declares an `EnvironmentRequestInvalidError` and can
/// say what was wrong, while `pairing-links/revoke` declares only a 403 and a
/// 500 — so a 400 there would be a status the client cannot decode, and the
/// honest decodable answer is that the revoke did not happen.
#[tokio::test]
async fn a_body_that_is_not_the_payload_is_refused_in_the_shape_the_route_declares() {
    let server = TestServer::start().await;

    for body in ["not json at all", "[]", "{\"scopes\": \"relay:read\"}"] {
        let refused = server
            .post_form_as("/api/auth/pairing-token", &server.browser(), body)
            .await;
        assert_eq!(refused.status, 400, "{body}: {}", refused.text);
        assert_eq!(refused.body["_tag"], "EnvironmentRequestInvalidError");
        assert_eq!(refused.body["reason"], "invalid_command");
    }

    for body in [json!({}), json!({ "id": "" }), json!({ "id": 7 })] {
        let refused = server.post_json("/api/auth/pairing-links/revoke", &body).await;
        assert_eq!(refused.status, 500, "{body}: {}", refused.text);
        assert_eq!(refused.body["_tag"], "EnvironmentInternalError");
        assert_eq!(refused.body["reason"], "pairing_link_revoke_failed");
    }

    server.stop().await;
}

/// An absent body is an empty object, because both of this payload's fields are
/// optional and a client that sends nothing for a payload requiring nothing is
/// not wrong.
#[tokio::test]
async fn minting_a_code_with_no_body_at_all_is_the_same_as_an_empty_one() {
    let server = TestServer::start().await;

    let minted = server.post_as("/api/auth/pairing-token", &server.browser()).await;
    assert_eq!(minted.status, 200, "{}", minted.text);
    assert_eq!(minted.body["credential"].as_str().expect("a code").len(), 12);
    // `label` is omitted rather than null: the contract types it `optionalKey`,
    // where absent decodes and null does not.
    assert!(
        minted.body.as_object().expect("an object").get("label").is_none(),
        "an unset label is absent, not null: {}",
        minted.text
    );

    server.stop().await;
}
