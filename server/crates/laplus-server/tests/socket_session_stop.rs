//! Ending an agent process, driven the way the UI drives it.
//!
//! Ticket 04 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, a real child process, and the command
//! `client-runtime/src/operations/commands.ts` builds. Nothing here reaches into
//! the server.
//!
//! **The command was refused by name**, so the only ways to get rid of an agent
//! were to interrupt a turn — a different act, aimed at a turn rather than at the
//! process — or to restart the server. The two places the real client sends this
//! are neither of them a stop button: before deleting a thread
//! (`useThreadActions.ts`) and when moving a conversation to another worktree
//! (`BranchToolbarBranchSelector.tsx`).
//!
//! ## What the assertions are made of
//!
//! Three kinds, and the third is the one that makes this ticket different from
//! the events half of the others:
//!
//! - **What the client would render**: the receipt on the thread's own feed, the
//!   session status, the latest turn, and the conversation a subscriber arriving
//!   afterwards is handed.
//! - **What reaches the project list**, which draws session state beside every
//!   conversation and cannot derive it from the thread feed.
//! - **What happened to the process.** `TestServer::live_agents` is the gauge, and
//!   `ScriptedAgent::starts` is what tells "the same child took the next turn"
//!   from "a second one did" — the gauge reads 1 either way.
//!
//! The stopped session's own `thread.session-set` is published *after* the child
//! has been reaped (`crate::turn`, where `Agent::stop` precedes it), so waiting
//! for that event is how these tests observe a reaping without asserting on a
//! clock.

mod harness;

use harness::agent::{ScriptedAgent, PAUSE};
use harness::conversation::{
    create_project, create_thread, find_activity, follow_up, last_session, start_turn,
};
use harness::workspace::Workspace;
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// The `thread.session.stop` `stopThreadSession` builds.
///
/// `createdAt` is sent because the contract requires it of the client. This
/// server ignores it — see `orchestration::StopSessionPayload` — and the event it
/// publishes carries a stamp of its own, which is what the assertions below read.
fn stop_session(thread_id: &str) -> Value {
    json!({
        "type": "thread.session.stop",
        "commandId": format!("test:stop:{thread_id}"),
        "threadId": thread_id,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The agent's announcement of itself, naming the session the conversation is
/// resumed by. `s-1` is what a `--resume` later has to carry.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s-1","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Read"]}"#;

/// A turn, said and finished.
fn says(text: &str) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"{text}"}}]}}}}"#
    )
}

const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":11,"total_cost_usd":0.001}"#;

/// A conversation whose scripts belong to the **conversation** rather than to the
/// process: a start carrying `--resume` picks up at the second script.
///
/// `ScriptedAgent::resuming_after_a_death` is named for the case it was written
/// for and is the right double for this one too, for the same reason — a stop ends
/// the process and the next turn is answered by a *replacement*, whose own turn
/// counter starts at zero. Without this the follow-up would be answered with the
/// first script, and "the conversation continued" would be indistinguishable from
/// "the conversation started again".
fn an_agent_that_can_be_picked_back_up() -> ScriptedAgent {
    let (first, second) = (says("first"), says("second"));
    ScriptedAgent::resuming_after_a_death(&[
        vec![INIT, first.as_str(), DONE],
        vec![INIT, second.as_str(), DONE],
    ])
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_string()
}

fn is_receipt(item: &Value) -> bool {
    item["event"]["type"] == "thread.session-stop-requested"
}

fn is_stopped(item: &Value) -> bool {
    item["event"]["type"] == "thread.session-set"
        && item["event"]["payload"]["session"]["status"] == "stopped"
}

/// Read a thread subscription up to and including **both** halves of a stop: the
/// receipt the command publishes, and the session change the driver publishes
/// once the child has actually gone.
///
/// Two waits rather than one, and the order between them is deliberately not
/// asserted. The receipt is published by the connection's own dispatch and the
/// ending by the driver's task, so which lands first is a fact about scheduling —
/// a reader that stopped at the first would sometimes leave the other unread and
/// fail on a busy machine for a reason it is not about.
async fn events_through_the_stop(client: &mut SocketClient, subscription: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    let (mut receipt, mut gone) = (false, false);
    while !(receipt && gone) {
        let batch = client
            .values_until(subscription, |item| is_receipt(item) || is_stopped(item))
            .await;
        receipt = receipt || batch.iter().any(is_receipt);
        gone = gone || batch.iter().any(is_stopped);
        seen.extend(batch);
    }
    seen
}

/// The first event of a kind in a run of events.
fn event_of<'a>(events: &'a [Value], kind: &str) -> &'a Value {
    events
        .iter()
        .map(|item| &item["event"])
        .find(|event| event["type"] == kind)
        .unwrap_or_else(|| {
            panic!(
                "no {kind} among {:?}",
                harness::conversation::kinds(events)
            )
        })
}

/// The conversation as a subscriber that arrives afterwards is handed it — which
/// is what "the conversation survives" has to mean, because an event that reached
/// an open subscription proves only that it was broadcast.
async fn as_a_fresh_subscriber_sees_it(server: &TestServer, thread_id: &str) -> Value {
    server
        .connect()
        .await
        .into_thread_snapshot(thread_id)
        .await["thread"]
        .clone()
}

/// The message from a refused command. The contract gives
/// `OrchestrationDispatchCommandError` a message and nothing else
/// machine-readable, so this string is the whole diagnostic the UI can show.
fn refusal(outcome: Outcome) -> String {
    outcome.expect_declared("OrchestrationDispatchCommandError")["message"]
        .as_str()
        .expect("a message")
        .to_string()
}

/// Register a project and a conversation in it, without starting a turn.
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

/// The whole ticket in one conversation: the process goes, nothing else does, and
/// the next turn continues the same conversation in a new session.
///
/// Asserted together rather than in six tests because they are one act, and a
/// stop that got five of them right is not five sixths of a working stop — a
/// conversation whose process was reaped and whose agent session id went with it
/// is a conversation the developer cannot pick back up.
#[tokio::test]
async fn stopping_a_session_reaps_the_process_and_leaves_the_conversation_resumable() {
    let agent = an_agent_that_can_be_picked_back_up();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_success();
    let turn = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        last_session(&turn, "the first turn")["payload"]["session"]["status"],
        "ready"
    );
    assert_eq!(server.live_agents(), 1, "there is nothing to stop");

    let answer = client
        .call("orchestration.dispatchCommand", stop_session("thread-1"))
        .await
        .expect_success();
    let stopping = events_through_the_stop(&mut client, &subscription).await;

    // **The receipt.** The developer's click, published before the process has
    // finished going — the client folds it onto the session it holds and stops
    // drawing the conversation as alive, which is what makes the button feel like
    // it did something.
    let receipt = event_of(&stopping, "thread.session-stop-requested");
    assert_eq!(receipt["sequence"], answer["sequence"]);
    assert_eq!(receipt["payload"]["threadId"], "thread-1");
    assert!(
        receipt["payload"]["createdAt"].is_string(),
        "the reducer puts this on the session, so it has to be there: {receipt}"
    );

    // **The process.** The stopped session is published after the child has been
    // reaped, so seeing it means the reaping happened rather than was asked for —
    // and the gauge is back where it was before the conversation started.
    let ended = last_session(&stopping, "the stopped session");
    assert_eq!(ended["payload"]["session"]["status"], "stopped");
    assert_eq!(
        ended["payload"]["session"]["lastError"],
        Value::Null,
        "a session the developer ended is not an error to show them"
    );
    server.await_live_agents(0).await;
    assert_eq!(agent.starts(), 1, "the stop started something");

    // **The conversation.** Everything the developer had is still there, and the
    // turn it belongs to is still reported as having finished.
    let held = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        held["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| text(&message["role"]))
            .collect::<Vec<String>>(),
        vec!["user", "assistant"],
        "{held}"
    );
    assert_eq!(
        held["latestTurn"]["state"], "completed",
        "stopping the session moved the turn it had already finished: {}",
        held["latestTurn"]
    );
    assert!(
        !held["activities"].as_array().expect("activities").is_empty(),
        "the work log went with the process: {held}"
    );
    assert_eq!(held["session"]["status"], "stopped", "{}", held["session"]);

    // **The continuation.** A turn after the stop is a *new* session — a second
    // process — and it continues the conversation the first one was holding,
    // because the agent's own handle on it survived. Hearing the second script is
    // the proof: the first process never played it.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up("thread-1", "message-2", "carry on"),
        )
        .await
        .expect_success();
    let after = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        harness::conversation::assistant_sends(&after)
            .last()
            .map(|(said, _)| said.clone()),
        Some("second".to_string()),
        "the conversation could not take a turn after the stop"
    );
    assert_eq!(
        last_session(&after, "the turn after the stop")["payload"]["session"]["status"],
        "ready"
    );
    assert_eq!(agent.starts(), 2, "the stopped process took the next turn");
    assert_eq!(
        agent.resumed(),
        vec!["s-1".to_string()],
        "the second session did not continue the first one's conversation: {:?}",
        agent.arguments()
    );

    let continued = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        continued["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|message| text(&message["role"]))
            .collect::<Vec<String>>(),
        vec!["user", "assistant", "user", "assistant"],
        "the conversation restarted rather than continuing: {continued}"
    );

    client.close().await;
    server.stop().await;
}

/// Stopping a conversation with no agent behind it is answered rather than
/// treated as an error.
///
/// There is nothing to stop and nothing went wrong — and the client sends exactly
/// this, because a conversation whose session is `stopped` or absent is one it
/// still guards a delete with. What it must not do is conjure a session to have
/// stopped: a conversation with no process behind it is the ordinary state after a
/// restart.
#[tokio::test]
async fn stopping_a_conversation_with_no_session_is_answered_and_leaves_it_usable() {
    let agent = an_agent_that_can_be_picked_back_up();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let subscription = client.watch_conversation("thread-1").await;

    for _ in 0..2 {
        client
            .call("orchestration.dispatchCommand", stop_session("thread-1"))
            .await
            .expect_success();
    }
    assert_eq!(server.live_agents(), 0);

    let untouched = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(untouched["session"], Value::Null, "{untouched}");
    assert_eq!(untouched["latestTurn"], Value::Null);

    // And the conversation still works, which is the half that would catch a stop
    // that had quietly wedged something on its way past.
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_success();
    let turn = client.events_through_the_turn(&subscription).await;
    assert_eq!(
        last_session(&turn, "the turn after the no-op stops")["payload"]["session"]["status"],
        "ready"
    );
    assert_eq!(agent.starts(), 1);

    client.close().await;
    server.stop().await;
}

/// A conversation this server has never heard of is refused by name.
///
/// The one thing here that is not a race. Stopping a session that is not running
/// is the developer getting what they asked for; a command naming a conversation
/// that does not exist is a client bug, and the sentence says which id so a
/// developer can tell which one it meant.
#[tokio::test]
async fn stopping_a_conversation_that_does_not_exist_is_refused_by_name() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    let message = refusal(
        client
            .call("orchestration.dispatchCommand", stop_session("thread-2"))
            .await,
    );
    assert!(message.contains("thread-2"), "{message}");

    // A refusal costs one call and nothing else.
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// A stop on one connection reaches a subscriber on another, and reaches the
/// project list — which draws session state beside every conversation.
///
/// Both feeds, because the UI reads them in different places and neither can be
/// derived from the other: the thread view folds the event, the sidebar folds the
/// summary. A stop only one of them heard about would leave two views of one
/// conversation disagreeing about whether an agent is running.
#[tokio::test]
async fn a_stop_reaches_a_second_connection_and_the_project_list() {
    let agent = an_agent_that_can_be_picked_back_up();
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;

    let mut author = server.connect().await;
    let authors_view = author.open_conversation(&workspace, "thread-1").await;

    let mut watcher = server.connect().await;
    let shell = watcher
        .subscribe("orchestration.subscribeShell", json!({}))
        .await;
    watcher.next_chunk(&shell).await;
    watcher.ack(&shell).await;
    let watchers_view = watcher.watch_conversation("thread-1").await;

    author
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "hello"),
        )
        .await
        .expect_success();
    author.events_through_the_turn(&authors_view).await;

    let answer = author
        .call("orchestration.dispatchCommand", stop_session("thread-1"))
        .await
        .expect_success();

    // The second window's own copy of the thread feed. It has the whole turn
    // ahead of the stop, which is what a window that was open the whole time
    // sees.
    let seen = events_through_the_stop(&mut watcher, &watchers_view).await;
    let receipt = event_of(&seen, "thread.session-stop-requested");
    assert_eq!(receipt["sequence"], answer["sequence"]);
    assert_eq!(
        last_session(&seen, "the stopped session")["payload"]["session"]["status"],
        "stopped"
    );

    // The project list, where the summary is republished whole. Read until the
    // summary says the session stopped rather than at the next chunk: the turn
    // published several of these before the stop.
    let listed = watcher
        .values_until(&shell, |item| {
            item["kind"] == "thread-upserted"
                && item["thread"]["session"]["status"] == "stopped"
        })
        .await;
    let summary = listed
        .last()
        .expect("the list said something about the conversation");
    assert_eq!(summary["thread"]["id"], "thread-1");
    assert_eq!(summary["thread"]["session"]["activeTurnId"], Value::Null);

    watcher.interrupt(&watchers_view).await;
    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// A stop while a turn is running ends the session as stopped rather than as a
/// failure, and the turn with it.
///
/// The case the ticket does not aim at — "a wedged or idle agent … has no turn to
/// interrupt" — and the one where getting it wrong is invisible until a developer
/// meets it. A driver whose agent goes away mid-turn reports that the agent
/// stopped before the turn finished and settles the session as `error`; a
/// developer who ended the session themselves does not need telling, and an error
/// on a conversation whose only fault was being stopped is worse than no message
/// at all.
///
/// **The turn is settled, and here it settles as `completed`.** That is not a
/// choice made here and is worth writing down, because it looks like one: the
/// receipt stops the session, and the partial reply is then closed with a buffered
/// message — which settles the latest turn *once the session is no longer running
/// it*. The client's reducer is the same rule in the same order
/// (`threadReducer.ts`, `case "thread.message-sent"`, `turnStillRunning`), so this
/// server and every window folding the same events land on the same turn. Making
/// it read `interrupted` here would mean disagreeing with the client about a turn
/// the developer is looking at, which is the one thing the fold exists to prevent.
///
/// It is `completed` *because this turn had streamed something to close*, which is
/// what the script above arranges. A stop with no assistant text in flight — mid
/// tool call, say — has no such message and settles through the session's own
/// `stopped` instead, as `interrupted`. Both are one rule read at two moments, and
/// the client reads it the same way at both.
///
/// It gets **no checkpoint** either way, and the workspace is a real repository so
/// that absence means something: there is no checkpoint status for a session the
/// developer ended, and both the ones available relabel the turn.
#[tokio::test]
async fn stopping_mid_turn_ends_the_turn_as_stopped_rather_than_as_a_failure() {
    let agent = ScriptedAgent::emitting(&[
        INIT,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"half a "}}}"#,
        PAUSE,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"half a thought"}]}}"#,
        DONE,
    ]);
    let workspace = Workspace::with(&[]);
    workspace.put("kept.txt", "one\n");
    workspace.init_repository().commit("the beginning");
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "take your time"),
        )
        .await
        .expect_success();
    // As far as the first piece of assistant text. The agent then pauses for about
    // a second, so the turn is genuinely in flight when the stop lands.
    let so_far = client.events_until_streaming(&subscription).await;
    assert_eq!(
        last_session(&so_far, "the running turn")["payload"]["session"]["status"],
        "running",
        "the turn had already finished, so this proves nothing"
    );

    client
        .call("orchestration.dispatchCommand", stop_session("thread-1"))
        .await
        .expect_success();
    let stopping = events_through_the_stop(&mut client, &subscription).await;

    let receipt = event_of(&stopping, "thread.session-stop-requested");
    assert_eq!(receipt["payload"]["threadId"], "thread-1");
    let ended = last_session(&stopping, "the stopped session");
    assert_eq!(ended["payload"]["session"]["status"], "stopped");
    assert_eq!(
        ended["payload"]["session"]["lastError"],
        Value::Null,
        "the developer asked for this: {ended}"
    );
    assert!(
        find_activity(&stopping, "session.failed").is_none(),
        "a session the developer ended was reported as one that fell over: {:?}",
        harness::conversation::activities(&stopping)
    );
    server.await_live_agents(0).await;

    let held = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        held["latestTurn"]["state"], "completed",
        "the turn was left running, so the conversation reads as one still being \
         worked on by a process that has gone: {}",
        held["latestTurn"]
    );
    assert!(
        !held["latestTurn"]["completedAt"].is_null(),
        "a settled turn with no completedAt reads as one still needing attention"
    );
    assert_eq!(
        held["checkpoints"],
        json!([]),
        "a turn cut short by a stop was offered for review, which relabels it: {}",
        held["checkpoints"]
    );

    // What the agent had said is kept, and settled rather than left growing.
    let reply = held["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .rfind(|message| message["role"] == "assistant")
        .cloned()
        .unwrap_or_else(|| panic!("nothing the agent said survived: {held}"));
    assert_eq!(text(&reply["text"]), "half a ");
    assert_eq!(
        reply["streaming"],
        json!(false),
        "a message left streaming stays streaming for the life of the thread"
    );

    client.close().await;
    server.stop().await;
}
