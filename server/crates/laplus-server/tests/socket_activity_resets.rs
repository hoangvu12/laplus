//! A settled conversation coming back on its own, because there is real work in
//! it again.
//!
//! Ticket 08 of the thread-lifecycle effort. Ticket 07 shipped a known hole: a
//! conversation the developer settled while it was quiet, whose agent later asked
//! for permission, would sit outside the inbox while blocked on a decision only
//! the developer can make. The settle invariants refuse to create that state at
//! settle time; these resets are what stop it being reachable a minute later.
//!
//! **Only the first of the three triggers is driven here, and that is not an
//! omission.** The other two cannot be reached through this server's own
//! dispatch: `thread.turn.start` requests the turn *before* it marks the session
//! starting, and every work-log row of a turn arrives after that, so by the time
//! a session or an approval could reset an override the turn request already has.
//! They are asserted where the state can be built — `threads::tests`, beside the
//! predicate that decides them — which is the same road ticket 07 took with the
//! queued-turn guard, for the same reason.
//!
//! What is here is the wire: the reset's own event, its reason, the sequence it
//! took, both feeds, and a restart.

mod harness;

use harness::agent::ScriptedAgent;
use harness::conversation::{kinds, start_turn};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// A session that says hello, answers, and finishes — the shortest real turn
/// there is. A settle needs a conversation with no agent working in it, so every
/// test here settles *after* a turn rather than during one.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"default","tools":["Read"]}"#;
const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

fn a_finishing_agent() -> ScriptedAgent {
    ScriptedAgent::emitting(&[INIT, SAID, DONE])
}

fn settle(thread_id: &str) -> Value {
    json!({
        "type": "thread.settle",
        "commandId": format!("test:settle:{thread_id}"),
        "threadId": thread_id,
    })
}

/// The unsettle a *developer* sends, which pins the conversation active. The
/// neutral reset the tests below watch for is the server's own and cannot be
/// asked for.
fn unsettle(thread_id: &str) -> Value {
    json!({
        "type": "thread.unsettle",
        "commandId": format!("test:unsettle:{thread_id}"),
        "threadId": thread_id,
        "reason": "user",
    })
}

fn archive(thread_id: &str) -> Value {
    json!({
        "type": "thread.archive",
        "commandId": format!("test:archive:{thread_id}"),
        "threadId": thread_id,
    })
}

/// Open the project list and take its opening chunk.
async fn open_shell(client: &mut SocketClient) -> String {
    let id = client
        .subscribe("orchestration.subscribeShell", json!({}))
        .await;
    client.next_chunk(&id).await;
    client.ack(&id).await;
    id
}

/// The conversation as a subscriber that arrives *afterwards* is handed it —
/// which is the half that proves the state was stored rather than broadcast.
async fn as_a_fresh_subscriber_sees_it(server: &TestServer, thread_id: &str) -> Value {
    server
        .connect()
        .await
        .into_thread_snapshot(thread_id)
        .await["thread"]
        .clone()
}

/// Every `thread.unsettled` in a run of events, in order.
fn resets(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.unsettled")
        .collect()
}

/// Run one turn to completion, so the conversation is quiet enough to settle —
/// which every test here needs, because the invariants refuse to settle a
/// conversation with an agent working in it.
async fn a_finished_turn(
    client: &mut SocketClient,
    thread_id: &str,
    subscription: &str,
    message_id: &str,
) {
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn(thread_id, message_id, "say something"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(subscription).await;
}

/// A turn requested on a settled conversation brings it back, on the
/// conversation's own feed and on the project list.
///
/// The reset accompanies the turn it was caused by rather than replacing it: both
/// events are published, the reset at its own number, and the dispatch answers
/// with the last of the numbers it committed — the same shape a turn request
/// already had when it committed three.
#[tokio::test]
async fn a_turn_requested_on_a_settled_conversation_brings_it_back() {
    let agent = a_finishing_agent();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let thread = client.open_conversation(&workspace, "thread-1").await;
    a_finished_turn(&mut client, "thread-1", &thread, "message-1").await;

    client
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();
    assert_eq!(
        as_a_fresh_subscriber_sees_it(&server, "thread-1").await["settledOverride"],
        "settled"
    );

    // The list is opened after the settle, so what it hears next is the reset and
    // the turn rather than the settle as well.
    let shell = open_shell(&mut client).await;

    let answered = client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-2", "actually, one more thing"),
        )
        .await
        .expect_success();

    let events = client.events_through_the_turn(&thread).await;
    let reset = match resets(&events).as_slice() {
        [only] => (*only).clone(),
        other => panic!(
            "one reset, once — got {} in {:?}",
            other.len(),
            kinds(&events)
        ),
    };

    // The reason is the server's own. The contract lets a command send only
    // `user`, which *pins* a conversation active — a reset that carried it would
    // hold in the inbox a conversation nobody had asked to hold there.
    assert_eq!(reset["payload"]["reason"], "activity");
    assert_eq!(reset["payload"]["threadId"], "thread-1");
    assert!(reset["payload"]["updatedAt"].is_string(), "{reset}");

    // Beside the turn request rather than instead of it, and after it: the turn is
    // what caused the reset.
    let seen = kinds(&events);
    let requested = seen
        .iter()
        .position(|kind| *kind == "thread.turn-start-requested")
        .unwrap_or_else(|| panic!("the turn was requested: {seen:?}"));
    let woken = seen
        .iter()
        .position(|kind| *kind == "thread.unsettled")
        .expect("the reset is in the same run of events");
    assert_eq!(
        woken,
        requested + 1,
        "the reset did not follow the turn that caused it: {seen:?}"
    );

    // The answer is the *last* of the command's numbers, so everything the
    // command published — including the reset — is already out by the time the
    // client reads it.
    let reset_at = reset["sequence"].as_i64().expect("a sequence");
    let answered_at = answered["sequence"].as_i64().expect("a sequence");
    assert!(
        reset_at < answered_at,
        "the answer ran ahead of the reset: {reset_at} against {answered_at}"
    );

    // And the project list, which is where the conversation reappears: the
    // sidebar folds the summary, the thread view folds the event, and neither can
    // be derived from the other.
    let on_the_list = client
        .values_until(&shell, |item| item["sequence"].as_i64() == Some(reset_at))
        .await;
    let listed = on_the_list
        .last()
        .unwrap_or_else(|| panic!("the project list did not hear the reset: {on_the_list:#?}"));
    assert_eq!(listed["kind"], "thread-upserted");
    assert_eq!(listed["thread"]["settledOverride"], Value::Null);
    assert_eq!(listed["thread"]["settledAt"], Value::Null);

    client.interrupt(&thread).await;
    client.interrupt(&shell).await;
    client.close().await;
    server.stop().await;
}

/// A conversation the developer pinned *active* returns to neutral, and one with
/// no override at all is left alone.
///
/// Both halves are the same guard read twice. The pin exists so the client's
/// auto-settle stays suppressed until real work moves it on — this is real work
/// moving it on, and neutral rather than settled is the answer, so the
/// conversation can settle itself again once the burst goes stale. And a
/// conversation nobody had settled must publish nothing: a reset with no override
/// behind it would be a no-op event carrying a stale `updatedAt`, which would
/// reorder a list the developer had not changed anything in.
#[tokio::test]
async fn a_pin_is_released_by_work_and_a_conversation_with_no_override_is_left_alone() {
    let agent = a_finishing_agent();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let thread = client.open_conversation(&workspace, "thread-1").await;
    a_finished_turn(&mut client, "thread-1", &thread, "message-1").await;

    client
        .call("orchestration.dispatchCommand", unsettle("thread-1"))
        .await
        .expect_success();
    assert_eq!(
        as_a_fresh_subscriber_sees_it(&server, "thread-1").await["settledOverride"],
        "active",
        "a user unsettle pins the conversation active"
    );
    // Read the developer's own unsettle off the feed before the work starts, so
    // what the turn publishes is the only thing left to read — and so that the
    // reset the work causes is told apart from the pin it releases by more than
    // its reason.
    let pinned = client
        .values_until(&thread, |item| item["event"]["type"] == "thread.unsettled")
        .await;
    assert_eq!(
        pinned.last().expect("the pin")["event"]["payload"]["reason"],
        "user"
    );

    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-2", "carry on"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&thread).await;

    assert_eq!(resets(&events).len(), 1, "{:?}", kinds(&events));
    assert_eq!(resets(&events)[0]["payload"]["reason"], "activity");
    let neutral = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        neutral["settledOverride"],
        Value::Null,
        "the pin survived the work that was supposed to release it"
    );

    // The conversation now has no override, so the next turn must wake nothing.
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-3", "and again"),
        )
        .await
        .expect_success();
    let quiet = client.events_through_the_turn(&thread).await;
    assert!(
        resets(&quiet).is_empty(),
        "a conversation nobody had settled was woken anyway: {:?}",
        kinds(&quiet)
    );
    assert_eq!(
        as_a_fresh_subscriber_sees_it(&server, "thread-1").await["settledOverride"],
        Value::Null
    );

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// An archived conversation keeps the inbox state the developer left on it, and a
/// turn in it wakes nothing.
///
/// The archive is the stronger statement, and both settle commands are already
/// refused on one — so clearing an override that `thread.unsettle` itself will not
/// touch would lose the developer's decision the moment they unarchived it.
/// Reachable from here because nothing refuses a *turn* in an archived
/// conversation: archiving is about the developer's list and never about the
/// agent.
#[tokio::test]
async fn work_in_an_archived_conversation_leaves_its_inbox_state_alone() {
    let agent = a_finishing_agent();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let thread = client.open_conversation(&workspace, "thread-1").await;
    a_finished_turn(&mut client, "thread-1", &thread, "message-1").await;

    for put_away in [settle("thread-1"), archive("thread-1")] {
        client
            .call("orchestration.dispatchCommand", put_away)
            .await
            .expect_success();
    }

    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-2", "one more thing"),
        )
        .await
        .expect_success();
    let events = client.events_through_the_turn(&thread).await;

    assert!(
        resets(&events).is_empty(),
        "an archived conversation was woken to an inbox it is not in: {:?}",
        kinds(&events)
    );
    let untouched = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        untouched["settledOverride"], "settled",
        "work in an archived conversation cleared a decision no command could: {untouched:#?}"
    );
    assert!(untouched["settledAt"].is_string(), "{untouched:#?}");

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// The reset is stored rather than merely broadcast: a conversation that woke
/// itself yesterday is awake today.
///
/// A restart is a second server on the same file, so nothing but the path on disk
/// carries the state across. The failure this catches is the one that matters —
/// a conversation the developer would find still settled tomorrow, with the work
/// that woke it in its own transcript.
#[tokio::test]
async fn a_reset_survives_a_restart() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let agent = a_finishing_agent();
    let workspace = Workspace::with(&["src/"]);

    {
        let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
        let mut client = server.connect().await;
        let thread = client.open_conversation(&workspace, "thread-1").await;
        a_finished_turn(&mut client, "thread-1", &thread, "message-1").await;

        client
            .call("orchestration.dispatchCommand", settle("thread-1"))
            .await
            .expect_success();
        client
            .call(
                "orchestration.dispatchCommand",
                start_turn("thread-1", "message-2", "one more thing"),
            )
            .await
            .expect_success();
        client.events_through_the_turn(&thread).await;

        client.interrupt(&thread).await;
        client.close().await;
        server.stop().await;
    }

    let server = TestServer::start_at_with_agent(&database, &agent.configured()).await;
    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        restored["settledOverride"],
        Value::Null,
        "the conversation came back settled, with the work that woke it in its own \
         transcript: {restored:#?}"
    );
    assert_eq!(restored["settledAt"], Value::Null);
    server.stop().await;
}
