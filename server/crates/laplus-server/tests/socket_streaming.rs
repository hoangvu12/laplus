//! Server-streaming subscriptions, driven through the socket.
//!
//! Streaming is a second framing mechanism alongside request/response, and
//! eight subscriptions plus the whole agent session lifecycle ride on it.
//! Ticket 04 proves it once on the least complicated case — the server
//! configuration — so that every later ticket inherits a working mechanism
//! rather than reinventing one.
//!
//! What is proven here, and where each thing is asserted:
//!
//! - a subscription is an ordinary `Request` and its values arrive as `Chunk`s
//! - a later server-side change reaches an open subscriber
//! - a client-initiated `Interrupt` terminates the stream, and nothing follows
//! - `Ack` is real back-pressure, not an advisory
//! - subscribers are independent, and their server-side resources go away
//!
//! Frame-level conformance to the ticket 01 captures lives next door in
//! `socket_conformance.rs`, with the rest of the capture comparisons.

mod harness;

use std::time::Duration;

use harness::{Outcome, TestServer};
use laplus_server::config::{Provider, ServerConfig};
use laplus_server::config_store::ConfigChange;
use laplus_server::process::Search;
use laplus_server::provider;
use serde_json::json;

const SUBSCRIBE_SERVER_CONFIG: &str = "subscribeServerConfig";

/// Long enough that a chunk the server was going to send would have arrived,
/// short enough not to dominate the suite. The reference server's own
/// back-pressure capture waited two seconds; this is a tenth of that, over
/// loopback with no work in between.
const SILENCE: Duration = Duration::from_millis(200);

#[tokio::test]
async fn a_subscription_receives_a_snapshot_as_its_first_chunk() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    let event = client.next_event(&subscription).await;

    assert_eq!(event["version"], json!(1));
    assert_eq!(event["type"], json!("snapshot"));
    assert!(event["config"].is_object());

    client.close().await;
    server.stop().await;
}

/// The subscription and the unary method describe the same server. If they
/// disagreed the UI would show one thing on boot and another a moment later.
#[tokio::test]
async fn the_snapshot_carries_the_same_config_the_unary_method_returns() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let unary = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    let snapshot = client.next_event(&subscription).await;

    assert_eq!(snapshot["config"], unary);

    client.close().await;
    server.stop().await;
}

/// The second criterion: a change made after the subscriber attached reaches
/// it. Everything else in this file is machinery around this one sentence.
#[tokio::test]
async fn a_server_side_change_pushes_a_further_chunk() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;

    let streaming = server.toggle_a_setting();
    let update = client.next_event(&subscription).await;

    assert_eq!(update["version"], json!(1));
    assert_eq!(update["type"], json!("settingsUpdated"));
    assert_eq!(
        update["payload"]["settings"]["enableAssistantStreaming"],
        json!(streaming)
    );

    client.close().await;
    server.stop().await;
}

/// Each member of the contract's closed union of update events, so the
/// vocabulary is pinned rather than only the one a test happened to use. The
/// client projects each onto its cached config by `type`, and an event it
/// cannot decode is dropped silently.
#[tokio::test]
async fn every_kind_of_configuration_change_reaches_the_subscriber() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;

    server.change_config(ConfigChange::Providers(vec![a_provider()]));
    let providers = client.next_event(&subscription).await;
    assert_eq!(providers["type"], json!("providerStatuses"));
    assert_eq!(
        providers["payload"]["providers"][0]["instanceId"],
        json!("claudeAgent")
    );

    server.change_config(ConfigChange::Keybindings {
        keybindings: Vec::new(),
        issues: Vec::new(),
    });
    let keybindings = client.next_event(&subscription).await;
    assert_eq!(keybindings["type"], json!("keybindingsUpdated"));
    assert!(keybindings["payload"]["keybindings"].is_array());
    assert!(keybindings["payload"]["issues"].is_array());

    client.close().await;
    server.stop().await;
}

/// A change is not only announced — it is remembered. A subscriber that
/// attaches afterwards must see it in its snapshot, or the server would be
/// telling two clients different things about itself.
#[tokio::test]
async fn a_change_is_visible_to_whoever_subscribes_next() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let streaming = server.toggle_a_setting();

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    let snapshot = client.next_event(&subscription).await;
    assert_eq!(
        snapshot["config"]["settings"]["enableAssistantStreaming"],
        json!(streaming)
    );

    // And through the unary method, which reads the same store.
    let unary = client
        .call("server.getConfig", json!({}))
        .await
        .expect_success();
    assert_eq!(unary["settings"]["enableAssistantStreaming"], json!(streaming));

    client.close().await;
    server.stop().await;
}

/// The third criterion. A client-initiated unsubscribe comes back as
/// `Failure`/`Interrupt` rather than `Success` — see the ticket 01 capture and
/// `docs/socket-wire-format.md`. A client reads it as a normal end.
#[tokio::test]
async fn an_unsubscribe_terminates_the_stream_with_an_interrupt_cause() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;

    client.interrupt(&subscription).await;
    match client.await_outcome(&subscription).await {
        Outcome::Failure(cause) => {
            assert_eq!(cause.len(), 1, "one cause, as in the capture: {cause:#?}");
            assert_eq!(cause[0]["_tag"], json!("Interrupt"));
            assert!(
                cause[0]["fiberId"].is_u64(),
                "the capture carries a numeric fiberId, got {}",
                cause[0]["fiberId"]
            );
        }
        other => panic!("expected an interrupt failure, got {other:?}"),
    }

    client.close().await;
    server.stop().await;
}

/// The terminal `Exit` is terminal. A chunk arriving after it would be written
/// into a client entry that has already been closed.
#[tokio::test]
async fn nothing_arrives_after_the_terminating_exit() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;

    client.interrupt(&subscription).await;
    client.await_outcome(&subscription).await;

    // A change that a still-open subscription would have chunked.
    server.toggle_a_setting();
    client.expect_silence(SILENCE).await;

    client.close().await;
    server.stop().await;
}

/// The fifth criterion, unsubscribe half.
#[tokio::test]
async fn unsubscribing_releases_the_subscription() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;
    assert_eq!(server.live_subscriptions(), 1);

    client.interrupt(&subscription).await;
    client.await_outcome(&subscription).await;
    server.await_live_subscriptions(0).await;

    // The connection itself is untouched — an unsubscribe is not a disconnect.
    assert_eq!(server.live_connections(), 1);
    assert_eq!(client.ping().await, json!({"_tag": "Pong"}));

    client.close().await;
    server.stop().await;
}

/// The fifth criterion, abrupt-disconnect half. A pump that outlived its
/// socket would hold a broadcast receiver forever and go on being woken by
/// every configuration change for the life of the process.
#[tokio::test]
async fn an_abrupt_disconnect_releases_every_subscription_on_the_connection() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    let second = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&first).await;
    client.next_event(&second).await;
    assert_eq!(server.live_subscriptions(), 2);

    // No close frame, no interrupt: the socket simply goes away.
    client.abandon();

    server.await_live_subscriptions(0).await;
    server.await_live_connections(0).await;
    server.stop().await;
}

/// A clean close releases them too, by a different path — the read loop ends
/// on a `Close` frame rather than on a transport error.
#[tokio::test]
async fn closing_the_connection_releases_its_subscriptions() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;

    client.close().await;

    server.await_live_subscriptions(0).await;
    server.await_live_connections(0).await;
    server.stop().await;
}

/// The sixth criterion. Two connections, each with its own subscription: both
/// see the same change, and each is acknowledged independently.
#[tokio::test]
async fn concurrent_subscribers_on_separate_connections_each_receive_their_own_updates() {
    let server = TestServer::start().await;
    let mut first = server.connect().await;
    let mut second = server.connect().await;

    let first_subscription = first.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    let second_subscription = second.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    first.next_event(&first_subscription).await;
    second.next_event(&second_subscription).await;
    assert_eq!(server.live_subscriptions(), 2);

    let streaming = server.toggle_a_setting();

    for (client, subscription) in [
        (&mut first, &first_subscription),
        (&mut second, &second_subscription),
    ] {
        let update = client.next_event(subscription).await;
        assert_eq!(update["type"], json!("settingsUpdated"));
        assert_eq!(
            update["payload"]["settings"]["enableAssistantStreaming"],
            json!(streaming)
        );
    }

    // Ending one leaves the other streaming.
    first.interrupt(&first_subscription).await;
    first.await_outcome(&first_subscription).await;
    server.await_live_subscriptions(1).await;

    let streaming = server.toggle_a_setting();
    let update = second.next_event(&second_subscription).await;
    assert_eq!(
        update["payload"]["settings"]["enableAssistantStreaming"],
        json!(streaming)
    );

    first.close().await;
    second.close().await;
    server.stop().await;
}

/// Two subscriptions on *one* socket. They share a connection, an id space and
/// a frame queue, so this is where a registry keyed by the wrong thing shows
/// up — one interrupt taking both down, or one client's `Ack` releasing the
/// other's chunk.
#[tokio::test]
async fn two_subscriptions_on_one_connection_are_independent() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let first = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    let second = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    assert_ne!(first, second);
    client.next_event(&first).await;
    client.next_event(&second).await;

    server.toggle_a_setting();
    assert_eq!(client.next_event(&first).await["type"], json!("settingsUpdated"));
    assert_eq!(client.next_event(&second).await["type"], json!("settingsUpdated"));

    client.interrupt(&first).await;
    client.await_outcome(&first).await;
    server.await_live_subscriptions(1).await;

    let streaming = server.toggle_a_setting();
    let update = client.next_event(&second).await;
    assert_eq!(
        update["payload"]["settings"]["enableAssistantStreaming"],
        json!(streaming)
    );

    client.close().await;
    server.stop().await;
}

/// `Ack` is genuine back-pressure. `docs/socket-wire-format.md` is emphatic
/// that ignoring it turns a busy subscription's memory from bounded to
/// unbounded, and fixture 05 demonstrates the reference server stalling a
/// committed change behind a withheld acknowledgement.
#[tokio::test]
async fn the_server_holds_at_most_one_unacknowledged_chunk() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_chunk(&subscription).await; // deliberately not acknowledged

    let streaming = server.toggle_a_setting();
    client.expect_silence(SILENCE).await;

    client.ack(&subscription).await;
    let update = client.next_event(&subscription).await;
    assert_eq!(
        update["payload"]["settings"]["enableAssistantStreaming"],
        json!(streaming),
        "the change was queued behind the missing Ack, not lost"
    );

    client.close().await;
    server.stop().await;
}

/// `values` batches — `subscribeServerLifecycle` sent two in one frame in the
/// capture — and a conforming client iterates it. Changes that pile up behind
/// a withheld acknowledgement are what produces a batch here.
#[tokio::test]
async fn changes_that_arrive_while_a_chunk_is_unacknowledged_are_batched() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_chunk(&subscription).await; // deliberately not acknowledged

    server.toggle_a_setting();
    server.change_config(ConfigChange::Providers(vec![a_provider()]));
    let streaming = server.toggle_a_setting();

    client.ack(&subscription).await;
    let batch = client.next_chunk(&subscription).await;

    assert_eq!(
        batch.len(),
        3,
        "three changes behind one Ack arrive as one chunk: {batch:#?}"
    );
    assert_eq!(batch[0]["type"], json!("settingsUpdated"));
    assert_eq!(batch[1]["type"], json!("providerStatuses"));
    assert_eq!(batch[2]["type"], json!("settingsUpdated"));
    assert_eq!(
        batch[2]["payload"]["settings"]["enableAssistantStreaming"],
        json!(streaming)
    );

    client.close().await;
    server.stop().await;
}

/// A subscriber can only fall so far behind before keeping its backlog costs
/// more than resending the world. At that point it is sent a fresh snapshot,
/// which supersedes every update it missed — the client's projection treats a
/// snapshot as a reset, so this is a resynchronisation rather than a gap.
#[tokio::test]
async fn a_subscriber_that_falls_far_behind_is_resynchronised_with_a_snapshot() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_chunk(&subscription).await; // deliberately not acknowledged

    // The backlog is read from the server rather than written as a literal:
    // it is declared policy — `docs/socket-wire-format.md` answers open
    // question 1 with it — not an implementation detail, and a literal would
    // silently stop reaching the lag path if the policy ever changed.
    //
    // An odd count, so the flag ends up somewhere the subscribe-time snapshot
    // was not — otherwise a resync that quietly resent the *old* snapshot
    // would pass.
    let mut streaming = false;
    for _ in 0..(laplus_server::subscriptions::BACKLOG * 2 + 1) {
        streaming = server.toggle_a_setting();
    }

    client.ack(&subscription).await;
    let resync = client.next_chunk(&subscription).await;

    assert_eq!(resync.len(), 1, "a snapshot replaces the backlog it supersedes");
    assert_eq!(resync[0]["type"], json!("snapshot"));
    assert_eq!(
        resync[0]["config"]["settings"]["enableAssistantStreaming"],
        json!(streaming),
        "the resynchronised snapshot is current, not the one sent at subscribe time"
    );

    // And *nothing follows it*. The events the snapshot was sent instead of
    // are all older than the snapshot, and the client applies each one as a
    // wholesale replacement — so delivering them afterwards would walk its
    // configuration backwards through values the server had already left.
    client.ack(&subscription).await;
    client.expect_silence(SILENCE).await;

    // Still a working subscription, not a stalled one.
    let streaming = server.toggle_a_setting();
    let update = client.next_event(&subscription).await;
    assert_eq!(update["type"], json!("settingsUpdated"));
    assert_eq!(
        update["payload"]["settings"]["enableAssistantStreaming"],
        json!(streaming)
    );

    client.close().await;
    server.stop().await;
}

/// The connection loop must not be blocked by an open subscription: the whole
/// point of the loose end ticket 03 left behind. A unary call answered while a
/// stream is live is the observable form of that.
#[tokio::test]
async fn a_unary_call_is_answered_while_a_subscription_is_open() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;

    assert!(client
        .call("server.getConfig", json!({}))
        .await
        .expect_success()
        .is_object());
    assert_eq!(client.ping().await, json!({"_tag": "Pong"}));

    // And the subscription is still live afterwards.
    server.toggle_a_setting();
    assert_eq!(
        client.next_event(&subscription).await["type"],
        json!("settingsUpdated")
    );

    client.close().await;
    server.stop().await;
}

/// Stray cancellation and acknowledgement traffic. Neither is an error — the
/// client may `Interrupt` a unary call that has already been answered, and an
/// `Ack` can race a stream's termination — so neither may take the connection
/// down or be counted as protocol drift.
#[tokio::test]
async fn acknowledging_or_interrupting_something_that_is_not_streaming_is_ignored() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client.ack("41").await;
    client.interrupt("41").await;
    client.expect_silence(SILENCE).await;

    // Including the id of a unary call that has already been answered.
    let answered = client.send_request("server.getConfig", json!({})).await;
    client.await_outcome(&answered).await;
    client.interrupt(&answered).await;
    client.expect_silence(SILENCE).await;

    assert_eq!(client.ping().await, json!({"_tag": "Pong"}));
    assert_eq!(server.unrecognized_messages(), 0);
    assert_eq!(server.unparseable_frames(), 0);

    client.close().await;
    server.stop().await;
}

/// Shutting the server down while a subscription is open. The pump holds a
/// task and a socket, and a graceful shutdown that waits for connections would
/// wait forever on a stream that never ends by itself.
#[tokio::test]
async fn shutting_the_server_down_releases_open_subscriptions() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let subscription = client.subscribe(SUBSCRIBE_SERVER_CONFIG, json!({})).await;
    client.next_event(&subscription).await;
    assert_eq!(server.live_subscriptions(), 1);

    // `stop` returns only once the listener and its connections are gone, so
    // the failure mode is a hang. Time it out to get a failure instead.
    tokio::time::timeout(Duration::from_secs(5), server.stop())
        .await
        .expect("the server stops rather than waiting on a stream that never ends");
}

/// A provider snapshot that is not the one the configuration starts with, which
/// is all a streaming test needs of it.
///
/// Ticket 09 turned this from a hand-built stand-in into the real thing — the
/// result of a lookup that finds nothing, which is a perfectly ordinary snapshot
/// and differs from the pending one every server starts with. A test about the
/// change feed should be moving the value the feed actually carries; a literal
/// here would go on compiling after the payload it imitates had changed shape.
fn a_provider() -> Provider {
    let settings = ServerConfig::detect().settings.providers.claude_agent;
    provider::describe(&settings, &Search::over(&[]), &[])
}
