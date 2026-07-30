//! Putting a conversation to sleep until a time the developer chose, and waking
//! it, driven the way the UI drives it.
//!
//! Ticket 09 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, the commands `client-runtime/src/operations/commands.ts`
//! builds, and the two subscriptions the real client folds. Nothing here reaches
//! into the server.
//!
//! ## What is asserted, and what deliberately is not
//!
//! The server stores two timestamps, guards them, and emits the events. It does
//! **not** decide what the inbox shows, and it does not decide when a snooze is
//! over: `effectiveSnoozed` reads `snoozedUntil` against the client's own clock,
//! and `threadRaisedHandWhileSnoozed` wakes a conversation early without
//! touching either field. Both live in the bundled client runtime, which ships
//! unmodified (ADR-0012) and has its own suite. So nothing here asserts that a
//! snoozed conversation is hidden — only that it is recorded as snoozed, on both
//! feeds and across a restart.
//!
//! **There is no scheduler, and that is asserted by there being nothing to
//! assert.** A snooze expires by being *read*: no event fires when a wake time
//! passes, so a test that waited for one would be waiting for something this
//! server deliberately does not do. What is driven instead is the one thing that
//! genuinely clears a snooze — the developer sending a new message — and the two
//! kinds of work that deliberately do not.
//!
//! The invariants **are** asserted, because the client's `canSnooze` is
//! explicitly a twin that exists to avoid a round trip and this is the
//! authoritative copy. The two that need a real agent are driven against one;
//! the running-session case is driven too, and it is the only one here that
//! asserts a snooze *succeeds* — a live session is not a blocker, because snooze
//! governs the developer's attention and never the agent.

mod harness;

use harness::agent::{ScriptedAgent, AWAIT_ANSWER, PAUSE};
use harness::conversation::{
    create_project, create_thread, kinds, respond_to_approval, respond_to_user_input, start_turn,
    start_turn_in,
};
use harness::workspace::Workspace;
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// The mode whose whole meaning is that the agent asks first.
const ASKS_FIRST: &str = "approval-required";

/// A session that has said hello, listing the tool that asks questions.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"default","tools":["Read","AskUserQuestion"]}"#;

/// The question, as the agent asks it — `socket_user_input.rs`'s.
const ASKS: &str = r#"{"type":"control_request","request_id":"req-question-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which database should this use?","header":"Database","multiSelect":false,"options":[{"label":"Postgres","description":"Relational, and the one the team already runs."},{"label":"SQLite","description":"One file, and nothing to operate."}]}]},"tool_use_id":"toolu_question_1"}}"#;

const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

/// A wake time an hour from now, in the shape `Date.toISOString()` renders and
/// the sidebar's presets send (`Sidebar.snooze.ts`).
///
/// Drawn from the clock rather than written out: a hard-coded wake time is a
/// test that passes until that date and then fails for a reason that has nothing
/// to do with what it asserts.
fn an_hour_from_now() -> String {
    in_millis(3_600_000)
}

fn an_hour_ago() -> String {
    in_millis(-3_600_000)
}

/// The server's own renderer, used from out here on purpose: a wake time is the
/// one value on this wire a *client* originates, and the shape the real client
/// sends is the shape this server renders. A second renderer in the test crate
/// would be a test that could agree with itself while disagreeing with both.
fn in_millis(offset: i64) -> String {
    let now = laplus_server::clock::now_epoch_millis() as i64;
    laplus_server::clock::iso_from_epoch_millis(
        (now + offset).try_into().expect("an instant after the epoch"),
    )
}

/// The `thread.snooze` the sidebar's snooze presets build.
fn snooze(thread_id: &str, until: &str) -> Value {
    json!({
        "type": "thread.snooze",
        "commandId": format!("test:snooze:{thread_id}"),
        "threadId": thread_id,
        "snoozedUntil": until,
    })
}

/// The `thread.unsnooze` beside it — "wake it now".
///
/// The reason is `user` and can be nothing else: the *event* carries two reasons
/// and the command carries one, because the neutral wake is the server's own and
/// a client that could send `activity` could forge it.
fn unsnooze(thread_id: &str) -> Value {
    json!({
        "type": "thread.unsnooze",
        "commandId": format!("test:unsnooze:{thread_id}"),
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

/// Register a project and a conversation in it. No turn: a snooze is two fields
/// on the thread, and the tests that need an agent say so.
async fn a_conversation(client: &mut SocketClient, workspace: &Workspace) {
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
            create_thread("project-1", "thread-1"),
        )
        .await
        .expect_success();
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

/// The next chunk for a subscription, acknowledged so the next one can follow.
async fn next_items(client: &mut SocketClient, id: &str) -> Vec<Value> {
    let values = client.next_chunk(id).await;
    client.ack(id).await;
    values
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

/// The message from a refused command. `OrchestrationDispatchCommandError`
/// carries a message and nothing else machine-readable, so this string is the
/// whole diagnostic the interface can show — and it renders it verbatim.
fn refusal(outcome: Outcome) -> String {
    outcome.expect_declared("OrchestrationDispatchCommandError")["message"]
        .as_str()
        .expect("a message")
        .to_string()
}

async fn refused(client: &mut SocketClient, command: Value) -> String {
    refusal(client.call("orchestration.dispatchCommand", command).await)
}

/// Both commands answer with the sequence they committed at and publish on the
/// conversation's own feed *and* on the project list at that number.
///
/// Both feeds, because the UI reads them in different places and neither can be
/// derived from the other: the thread view folds the event and the sidebar's
/// snoozed section folds the summary.
#[tokio::test]
async fn snoozing_is_answered_with_a_sequence_and_published_on_both_feeds() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let shell = open_shell(&mut client).await;
    let thread = client.watch_conversation("thread-1").await;
    let wake = an_hour_from_now();

    let snoozed_at = client
        .call("orchestration.dispatchCommand", snooze("thread-1", &wake))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.snoozed"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(
        event["sequence"], snoozed_at["sequence"],
        "the event and the answer name the same commit"
    );
    assert_eq!(event["payload"]["threadId"], "thread-1");
    // The wake time is echoed exactly as it was sent rather than re-rendered:
    // the client parses it back with `Date.parse` and compares it against its
    // own clock, so a second spelling of one moment would be a field this server
    // and that one describe differently.
    assert_eq!(
        event["payload"]["snoozedUntil"], json!(wake),
        "the wake time came back changed: {event}"
    );
    // Both stamps, because the client's reducer writes both onto the thread
    // (`threadReducer.ts`, `case "thread.snoozed"`) — and the second is what a
    // raised hand is later measured against.
    let asked_at = event["payload"]["snoozedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("a snooze says when it was asked for: {event}"));
    assert_eq!(
        event["payload"]["updatedAt"], json!(asked_at),
        "the two stamps are one moment: {event}"
    );

    let on_the_list = next_items(&mut client, &shell).await;
    assert_eq!(on_the_list.len(), 1, "{on_the_list:#?}");
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(on_the_list[0]["sequence"], snoozed_at["sequence"]);
    assert_eq!(on_the_list[0]["thread"]["snoozedUntil"], json!(wake));
    assert_eq!(on_the_list[0]["thread"]["snoozedAt"], json!(asked_at));

    let woken_at = client
        .call("orchestration.dispatchCommand", unsnooze("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.unsnoozed"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(event["sequence"], woken_at["sequence"]);
    assert_eq!(event["payload"]["reason"], "user");
    assert!(event["payload"]["updatedAt"].is_string(), "{event}");

    // Both fields, never one: a `snoozedAt` left behind renders a "Woke"
    // indicator (`threadWokeAt`) for a wake the developer just performed.
    let on_the_list = next_items(&mut client, &shell).await;
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(on_the_list[0]["thread"]["snoozedUntil"], Value::Null);
    assert_eq!(on_the_list[0]["thread"]["snoozedAt"], Value::Null);

    client.interrupt(&thread).await;
    client.interrupt(&shell).await;
    client.close().await;
    server.stop().await;
}

/// A second window has to see the first one's work, for both commands. The
/// change feed is the server's, not the connection's.
#[tokio::test]
async fn both_changes_reach_a_subscriber_on_another_connection() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;

    let mut author = server.connect().await;
    a_conversation(&mut author, &workspace).await;

    let mut watcher = server.connect().await;
    let shell = open_shell(&mut watcher).await;
    let thread = watcher.watch_conversation("thread-1").await;
    let wake = an_hour_from_now();

    let answered = author
        .call("orchestration.dispatchCommand", snooze("thread-1", &wake))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.snoozed"]);
    assert_eq!(on_the_thread[0]["event"]["sequence"], answered["sequence"]);

    let on_the_list = next_items(&mut watcher, &shell).await;
    assert_eq!(on_the_list[0]["thread"]["snoozedUntil"], json!(wake));

    author
        .call("orchestration.dispatchCommand", unsnooze("thread-1"))
        .await
        .expect_success();

    assert_eq!(
        kinds(&next_items(&mut watcher, &thread).await),
        vec!["thread.unsnoozed"]
    );
    assert_eq!(
        next_items(&mut watcher, &shell).await[0]["thread"]["snoozedUntil"],
        Value::Null
    );

    watcher.interrupt(&thread).await;
    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// A second snooze to the same wake time lands on the same state and does not
/// move the conversation in a list ordered by when things changed — and waking a
/// conversation nobody snoozed is harmless the same way.
///
/// Both are answered rather than refused, which is where they part company with
/// the archive commands: a second archive is a click on a control that is no
/// longer there, and a snooze is a standing arrangement that folding again lands
/// on either way.
#[tokio::test]
async fn snoozing_twice_or_waking_an_awake_conversation_moves_nothing() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let wake = an_hour_from_now();

    // Waking one nobody snoozed, before anything else: it has to be harmless on
    // a conversation that has never carried either field.
    client
        .call("orchestration.dispatchCommand", unsnooze("thread-1"))
        .await
        .expect_success();
    let untouched = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(untouched["snoozedUntil"], Value::Null);

    client
        .call("orchestration.dispatchCommand", snooze("thread-1", &wake))
        .await
        .expect_success();
    let asleep = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;

    client
        .call("orchestration.dispatchCommand", snooze("thread-1", &wake))
        .await
        .expect_success();
    let again = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;

    assert_eq!(again["snoozedUntil"], asleep["snoozedUntil"]);
    assert_eq!(
        again["snoozedAt"], asleep["snoozedAt"],
        "a second snooze restamped when the developer asked for it"
    );
    assert_eq!(
        again["updatedAt"], asleep["updatedAt"],
        "a second snooze moved the conversation in a list ordered by when things changed"
    );

    client
        .call("orchestration.dispatchCommand", unsnooze("thread-1"))
        .await
        .expect_success();
    let awake = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(awake["snoozedUntil"], Value::Null);

    client
        .call("orchestration.dispatchCommand", unsnooze("thread-1"))
        .await
        .expect_success();
    assert_eq!(
        as_a_fresh_subscriber_sees_it(&server, "thread-1").await["updatedAt"],
        awake["updatedAt"],
        "a second wake moved the conversation"
    );

    client.close().await;
    server.stop().await;
}

/// Choosing a later time is a new decision rather than a repeat: both stamps
/// move.
///
/// `snoozedAt` moving is the half that matters. The client measures a raised
/// hand against it — a session that failed or a turn that completed *after* the
/// snooze wakes the conversation early — so a second snooze carrying the first
/// one's stamp would be woken immediately by the work the developer had just
/// decided to sleep through.
#[tokio::test]
async fn snoozing_to_a_different_time_is_a_new_decision() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    client
        .call(
            "orchestration.dispatchCommand",
            snooze("thread-1", &an_hour_from_now()),
        )
        .await
        .expect_success();
    let first = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;

    let later = in_millis(7_200_000);
    client
        .call("orchestration.dispatchCommand", snooze("thread-1", &later))
        .await
        .expect_success();
    let second = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;

    assert_eq!(second["snoozedUntil"], json!(later));
    // The two stamps are one moment, which is what a snooze that took the clock
    // looks like. That a *different* wake time is not a repeat at all is decided
    // where no clock is involved — `threads::fold`'s
    // `a_snooze_to_another_time_is_not_a_repeat_of_the_first` — because two
    // calls this close together can land in the same millisecond and comparing
    // the two snoozes here would assert the clock's resolution instead.
    assert_eq!(
        second["snoozedAt"], second["updatedAt"],
        "the second snooze's two stamps came from different moments: {second:#?}"
    );
    assert!(
        second["updatedAt"].as_str() >= first["updatedAt"].as_str(),
        "{second:#?}"
    );

    client.close().await;
    server.stop().await;
}

/// Everything a conversation with nothing running can be refused for, on the
/// sentence — which is all `OrchestrationDispatchCommandError` carries.
///
/// The wake time is the field that is new here. A time that has passed, this
/// very instant, and a string that is not a time at all are one refusal: a wake
/// time this server cannot place on its own clock is not one it can call future
/// either. Each names the time as well as the conversation, because a snooze is
/// sent from a preset menu and "that time will not do" without saying which time
/// is not something a developer can act on.
#[tokio::test]
async fn a_blank_unknown_archived_forged_or_elapsed_command_is_refused_with_a_sentence() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let wake = an_hour_from_now();

    for blank in [snooze("  ", &wake), unsnooze("  ")] {
        let message = refused(&mut client, blank).await;
        assert!(message.contains("threadId"), "{message}");
    }

    for unknown in [snooze("never-created", &wake), unsnooze("never-created")] {
        let message = refused(&mut client, unknown).await;
        assert!(message.contains("never-created"), "{message}");
    }

    // A wake time that is not strictly ahead of now, three ways — and a fourth
    // that is not a moment at all. `now` itself is in the list because the
    // comparison is strictly future: a wake time equal to this instant has
    // already elapsed by the time anything reads it, so the conversation it
    // would produce is snoozed and awake at once.
    for elapsed in [
        an_hour_ago(),
        in_millis(0),
        "tomorrow".to_string(),
        "2026-13-45T09:00:00.000Z".to_string(),
    ] {
        let message = refused(&mut client, snooze("thread-1", &elapsed)).await;
        assert!(
            message.contains(&elapsed) && message.contains("thread-1"),
            "the sentence names neither the time nor the conversation: {message}"
        );
    }

    // The neutral wake is not a client's to send: an unsnooze claiming activity
    // is refused rather than quietly treated as the developer's own, because the
    // two are different accounts of why the conversation came back.
    let forged = json!({
        "type": "thread.unsnooze",
        "commandId": "test:forged",
        "threadId": "thread-1",
        "reason": "activity",
    });
    let message = refused(&mut client, forged).await;
    assert!(
        message.contains("activity") && message.contains("thread-1"),
        "{message}"
    );

    client
        .call("orchestration.dispatchCommand", archive("thread-1"))
        .await
        .expect_success();

    for put_away in [snooze("thread-1", &wake), unsnooze("thread-1")] {
        let message = refused(&mut client, put_away).await;
        assert!(
            message.contains("thread-1") && message.contains("archived"),
            "{message}"
        );
    }

    // Nothing moved, and the connection is still usable: a refusal costs one
    // call.
    let untouched = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(untouched["snoozedUntil"], Value::Null);
    assert_eq!(untouched["snoozedAt"], Value::Null);
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// Snooze state survives a restart, and a subscriber that arrives after one
/// holds what a subscriber that watched it happen holds.
///
/// A restart is a second server on the same file — nothing but the path on disk
/// carried the state across. This is what makes "there is no scheduler" a
/// design rather than a gap: a wake time is a *fact about the conversation*, so
/// it comes back from a restart with nothing to re-register, and the wake it
/// describes needs no process to have been running for it.
#[tokio::test]
async fn a_snooze_survives_a_restart_and_a_fresh_subscriber_agrees() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);
    let wake = an_hour_from_now();

    let watched = {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        a_conversation(&mut client, &workspace).await;
        let thread = client.watch_conversation("thread-1").await;

        client
            .call("orchestration.dispatchCommand", snooze("thread-1", &wake))
            .await
            .expect_success();
        let seen = next_items(&mut client, &thread).await;
        let watched = seen[0]["event"]["payload"]["snoozedAt"].clone();

        client.interrupt(&thread).await;
        client.close().await;
        server.stop().await;
        watched
    };

    let server = TestServer::start_at(&database).await;
    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(restored["snoozedUntil"], json!(wake));
    assert_eq!(
        restored["snoozedAt"], watched,
        "the stamp a subscriber watched is not the one a fresh one is handed: {restored:#?}"
    );

    // And it can still be woken afterwards, which is what makes a snooze an
    // arrangement rather than a one-way door.
    let mut client = server.connect().await;
    client
        .call("orchestration.dispatchCommand", unsnooze("thread-1"))
        .await
        .expect_success();
    client.close().await;
    assert_eq!(
        as_a_fresh_subscriber_sees_it(&server, "thread-1").await["snoozedUntil"],
        Value::Null
    );

    server.stop().await;
}

/// Sending a new message spends the return ticket, and the wake is published
/// beside the turn that caused it.
///
/// The one thing that genuinely clears a snooze. The developer came back of
/// their own accord, so there is nothing left to bring them back to — and the
/// reason is the server's own `activity`, which no command can send.
#[tokio::test]
async fn a_new_message_spends_the_return_ticket() {
    let agent = ScriptedAgent::emitting(&[INIT, SAID, DONE]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let thread = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            snooze("thread-1", &an_hour_from_now()),
        )
        .await
        .expect_success();
    assert_eq!(
        kinds(&next_items(&mut client, &thread).await),
        vec!["thread.snoozed"]
    );

    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "actually, one more thing"),
        )
        .await
        .expect_success();

    // The wake follows the turn rather than preceding it: a conversation woken
    // by work that had not happened yet would be a conversation woken by
    // nothing.
    let announced = next_items(&mut client, &thread).await;
    let seen = kinds(&announced);
    let wake = seen
        .iter()
        .position(|kind| *kind == "thread.unsnoozed")
        .unwrap_or_else(|| panic!("the message did not spend the snooze: {seen:?}"));
    let requested = seen
        .iter()
        .position(|kind| *kind == "thread.turn-start-requested")
        .expect("a turn was requested");
    assert!(requested < wake, "{seen:?}");
    assert_eq!(
        announced[wake]["event"]["payload"]["reason"], "activity",
        "the wake claimed to be the developer's own"
    );

    client.events_through_the_turn(&thread).await;
    let awake = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(awake["snoozedUntil"], Value::Null);
    assert_eq!(awake["snoozedAt"], Value::Null);

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// A live session is not a blocker for a snooze, and the agent is not disturbed
/// by one.
///
/// The entry where `canSnooze` and `canSettle` differ, and the whole of what
/// makes snooze an overlay: the work carries on to its natural end and only
/// where the conversation is *drawn* changes. The turn completing is what says
/// the agent was not spoken to — and it does not clear the snooze either, because
/// the snooze never paused it.
#[tokio::test]
async fn a_working_agent_can_be_snoozed_and_is_not_disturbed_by_it() {
    let agent = ScriptedAgent::emitting(&[INIT, PAUSE, SAID, DONE]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let thread = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "say something"),
        )
        .await
        .expect_success();

    // The turn's own command marks the session `starting` before it answers, so
    // by here there is an agent for the snooze to be tested against without
    // reading a single event. It is the message *before* the snooze that matters
    // here: the return ticket is bought after the developer last spoke, so
    // nothing has spent it.
    let wake = an_hour_from_now();
    client
        .call("orchestration.dispatchCommand", snooze("thread-1", &wake))
        .await
        .expect_success();

    client.events_through_the_turn(&thread).await;

    let asleep = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        asleep["snoozedUntil"], json!(wake),
        "the turn running to completion spent a snooze it never paused: {asleep:#?}"
    );
    assert_eq!(
        asleep["latestTurn"]["state"], "completed",
        "the agent was disturbed by a decision about the developer's attention: {asleep:#?}"
    );

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// A conversation with an unanswered permission request cannot be snoozed.
///
/// The agent has stopped and is waiting on the developer, so nothing is
/// *running* — and this is exactly what a snooze must not hide: a decision only
/// they can make, parked somewhere they have just told the interface not to
/// look.
#[tokio::test]
async fn a_conversation_waiting_on_a_permission_decision_cannot_be_snoozed() {
    let agent = ScriptedAgent::replaying("07-permission-approved");
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let thread = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn_in("thread-1", "message-1", "write note.txt", ASKS_FIRST),
        )
        .await
        .expect_success();
    let (_, request_id) = client.events_until_permission(&thread).await;

    let message = refused(&mut client, snooze("thread-1", &an_hour_from_now())).await;
    assert!(
        message.contains("thread-1") && message.contains("permission"),
        "{message}"
    );

    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_approval("thread-1", &request_id, "accept"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&thread).await;

    client
        .call(
            "orchestration.dispatchCommand",
            snooze("thread-1", &an_hour_from_now()),
        )
        .await
        .expect_success();

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// A conversation with an unanswered question cannot be snoozed either.
///
/// Its own test rather than a second case in the one above, for the reason the
/// two folds are separate in the client: a question that arrived as an approval
/// is the bug those suites exist to catch, so a guard that read either flag as
/// the other would pass on it.
#[tokio::test]
async fn a_conversation_waiting_on_an_answer_cannot_be_snoozed() {
    let agent = ScriptedAgent::emitting(&[INIT, ASKS, AWAIT_ANSWER, SAID, DONE]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let thread = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "pick a database"),
        )
        .await
        .expect_success();
    let (_, request_id) = client.events_until_user_input(&thread).await;

    let message = refused(&mut client, snooze("thread-1", &an_hour_from_now())).await;
    assert!(
        message.contains("thread-1") && message.contains("question"),
        "{message}"
    );

    client
        .call(
            "orchestration.dispatchCommand",
            respond_to_user_input(
                "thread-1",
                &request_id,
                json!({ "Which database should this use?": "Postgres" }),
            ),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&thread).await;

    client
        .call(
            "orchestration.dispatchCommand",
            snooze("thread-1", &an_hour_from_now()),
        )
        .await
        .expect_success();

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}
