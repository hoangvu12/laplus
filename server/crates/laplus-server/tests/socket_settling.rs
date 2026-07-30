//! Settling a finished conversation and pinning it back, driven the way the UI
//! drives it.
//!
//! Ticket 07 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, the commands `client-runtime/src/operations/commands.ts`
//! builds, and the two subscriptions the real client folds. Nothing here reaches
//! into the server.
//!
//! **Settling here is not `crate::settling`'s settling.** That word means
//! reading a session status as how a *turn* went, and it has seniority in this
//! repository. These commands are about whether a *thread* belongs in the
//! developer's inbox — `docs/adr/0024` and the **Inbox state** entry in
//! `CONTEXT.md` carry the collision and what was done about it.
//!
//! ## What is asserted, and what deliberately is not
//!
//! The server stores the two fields, enforces the invariants and emits the
//! events. It does **not** decide what the inbox shows: `effectiveSettled` reads
//! these fields alongside four other things and lives in the bundled client
//! runtime, which ships unmodified (ADR-0012) and has its own suite. So nothing
//! here asserts that a settled conversation is hidden — only that it is recorded
//! as settled, on both feeds and across a restart.
//!
//! The invariants *are* asserted, because the client's copy of them is
//! explicitly a twin that exists to avoid a round trip and this is the
//! authoritative one. Three of the four are driven against a real agent, which is
//! the only honest way to have a conversation that is genuinely working or
//! genuinely waiting on the developer.
//!
//! **The fourth is not reachable from here.** A turn requested and not yet
//! adopted is a state this server passes through in the space between two
//! statements: `thread.turn.start` writes the message and then the turn, so the
//! turn's `requestedAt` is never older than the message, and the session is
//! marked `starting` in the same command. The guard exists because it is the
//! client's rule and the client sees shells this server did not write — so it is
//! tested where the state can be built, in `threads::tests`, beside the window it
//! is measured against.

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

/// The question, as the agent asks it — `socket_user_input.rs`'s, which is where
/// the shape of one is argued.
const ASKS: &str = r#"{"type":"control_request","request_id":"req-question-1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{"question":"Which database should this use?","header":"Database","multiSelect":false,"options":[{"label":"Postgres","description":"Relational, and the one the team already runs."},{"label":"SQLite","description":"One file, and nothing to operate."}]}]},"tool_use_id":"toolu_question_1"}}"#;

const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

/// The `thread.settle` the sidebar's context menu and the chat view's menu both
/// build.
fn settle(thread_id: &str) -> Value {
    json!({
        "type": "thread.settle",
        "commandId": format!("test:settle:{thread_id}"),
        "threadId": thread_id,
    })
}

/// The `thread.unsettle` beside it.
///
/// The reason is `user` and can be nothing else: the *event* carries two reasons
/// and the command carries one, because the neutral reset is the server's own and
/// a client that could send `activity` could forge it.
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

/// Register a project and a conversation in it. No turn: settling is two fields
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

/// Dispatch a settle and expect it to be refused, handing back the sentence.
async fn refused(client: &mut SocketClient, command: Value) -> String {
    refusal(client.call("orchestration.dispatchCommand", command).await)
}

/// Both commands answer with the sequence they committed at and publish on the
/// conversation's own feed *and* on the project list at that number.
///
/// Both feeds, because the UI reads them in different places and neither can be
/// derived from the other: the thread view folds the event and the sidebar folds
/// the summary.
#[tokio::test]
async fn settling_is_answered_with_a_sequence_and_published_on_both_feeds() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let shell = open_shell(&mut client).await;
    let thread = client.watch_conversation("thread-1").await;

    let settled_at = client
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.settled"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(
        event["sequence"], settled_at["sequence"],
        "the event and the answer name the same commit"
    );
    assert_eq!(event["payload"]["threadId"], "thread-1");
    // Both stamps, because the client's reducer writes both onto the thread
    // (`threadReducer.ts`, `case "thread.settled"`): a settle records the
    // override *and* the moment it settled at, and a payload carrying one would
    // leave a window that watched it disagreeing with one that reloaded after.
    let stamped = event["payload"]["settledAt"]
        .as_str()
        .unwrap_or_else(|| panic!("a settle says when: {event}"));
    assert_eq!(
        event["payload"]["updatedAt"], json!(stamped),
        "the two stamps are one moment: {event}"
    );

    let on_the_list = next_items(&mut client, &shell).await;
    assert_eq!(on_the_list.len(), 1, "{on_the_list:#?}");
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(on_the_list[0]["sequence"], settled_at["sequence"]);
    assert_eq!(on_the_list[0]["thread"]["settledOverride"], "settled");
    assert_eq!(on_the_list[0]["thread"]["settledAt"], json!(stamped));

    let pinned_at = client
        .call("orchestration.dispatchCommand", unsettle("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.unsettled"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(event["sequence"], pinned_at["sequence"]);
    // The reason is the whole of what makes the two directions asymmetrical: a
    // *user* unsettle pins the conversation active, so the client's own
    // auto-settle stays suppressed until real work moves it on.
    assert_eq!(event["payload"]["reason"], "user");
    assert!(event["payload"]["updatedAt"].is_string(), "{event}");

    let on_the_list = next_items(&mut client, &shell).await;
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(
        on_the_list[0]["thread"]["settledOverride"], "active",
        "a user unsettle cleared the override to neutral instead of pinning it"
    );
    assert_eq!(on_the_list[0]["thread"]["settledAt"], Value::Null);

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

    let answered = author
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.settled"]);
    assert_eq!(on_the_thread[0]["event"]["sequence"], answered["sequence"]);
    let stamped = on_the_thread[0]["event"]["payload"]["settledAt"].clone();
    assert!(stamped.is_string(), "{on_the_thread:#?}");

    let on_the_list = next_items(&mut watcher, &shell).await;
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(on_the_list[0]["thread"]["settledAt"], stamped);

    let answered = author
        .call("orchestration.dispatchCommand", unsettle("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.unsettled"]);
    assert_eq!(on_the_thread[0]["event"]["sequence"], answered["sequence"]);
    assert_eq!(on_the_thread[0]["event"]["payload"]["reason"], "user");

    let on_the_list = next_items(&mut watcher, &shell).await;
    assert_eq!(on_the_list[0]["thread"]["settledOverride"], "active");

    watcher.interrupt(&thread).await;
    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// A repeat of either command is answered rather than refused, and reports the
/// moment the conversation already carried.
///
/// **Where these two part company with the archive commands.** A second archive
/// is a click on a control that is no longer there, so it is refused; both
/// directions of a settle are a standing answer the developer gave, so folding
/// the event again lands on the same state either way. What a repeat must not do
/// is churn — the thread list is ordered by when things changed, so a
/// double-click that restamped the clock would move a conversation the developer
/// did not touch.
#[tokio::test]
async fn settling_or_unsettling_twice_is_harmless_and_moves_nothing() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    client
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();
    let settled = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;

    client
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();
    let again = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;

    assert_eq!(again["settledOverride"], settled["settledOverride"]);
    assert_eq!(again["settledAt"], settled["settledAt"]);
    assert_eq!(
        again["updatedAt"], settled["updatedAt"],
        "a second settle moved the conversation in a list ordered by when things changed"
    );

    client
        .call("orchestration.dispatchCommand", unsettle("thread-1"))
        .await
        .expect_success();
    let pinned = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(pinned["settledOverride"], "active");

    client
        .call("orchestration.dispatchCommand", unsettle("thread-1"))
        .await
        .expect_success();
    let again = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(again["settledOverride"], "active");
    assert_eq!(
        again["updatedAt"], pinned["updatedAt"],
        "a second unsettle moved the conversation"
    );

    client.close().await;
    server.stop().await;
}

/// Inbox state survives a restart, and a subscriber that arrives after one holds
/// what a subscriber that watched it happen holds.
///
/// A restart is a second server on the same file — nothing but the path on disk
/// carried the state across. The agreement is the second half and is the point of
/// asserting it: a conversation the developer settled yesterday has to still be
/// settled today, or the state they curated was theatre.
#[tokio::test]
async fn inbox_state_survives_a_restart_and_a_fresh_subscriber_agrees() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);

    let watched = {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        a_conversation(&mut client, &workspace).await;
        let thread = client.watch_conversation("thread-1").await;

        client
            .call("orchestration.dispatchCommand", settle("thread-1"))
            .await
            .expect_success();
        let seen = next_items(&mut client, &thread).await;
        let watched = seen[0]["event"]["payload"]["settledAt"].clone();

        client.interrupt(&thread).await;
        client.close().await;
        server.stop().await;
        watched
    };

    let server = TestServer::start_at(&database).await;
    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(restored["settledOverride"], "settled");
    assert_eq!(
        restored["settledAt"], watched,
        "the stamp a subscriber watched is not the stamp a fresh one is handed: {restored:#?}"
    );

    // And it can still be pinned back afterwards, which is what makes a settle a
    // decision rather than a one-way door.
    let mut client = server.connect().await;
    client
        .call("orchestration.dispatchCommand", unsettle("thread-1"))
        .await
        .expect_success();
    client.close().await;
    assert_eq!(
        as_a_fresh_subscriber_sees_it(&server, "thread-1").await["settledOverride"],
        "active"
    );

    server.stop().await;
}

/// The refusals a conversation with nothing running can produce, on the sentence
/// — which is all `OrchestrationDispatchCommandError` carries.
///
/// An archived conversation is refused in *both* directions, and that is one rule
/// rather than two: it is not in the inbox, so there is nothing to take it out of
/// and no inbox to pin it back into.
#[tokio::test]
async fn a_blank_unknown_archived_or_forged_command_is_refused_with_a_sentence() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    for blank in [settle("  "), unsettle("  ")] {
        let message = refused(&mut client, blank).await;
        assert!(message.contains("threadId"), "{message}");
    }

    for unknown in [settle("never-created"), unsettle("never-created")] {
        let message = refused(&mut client, unknown).await;
        assert!(message.contains("never-created"), "{message}");
    }

    // The neutral reset is not a client's to send: an unsettle claiming activity
    // is refused rather than quietly pinned, because the two leave the
    // conversation in different states.
    let forged = json!({
        "type": "thread.unsettle",
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

    for put_away in [settle("thread-1"), unsettle("thread-1")] {
        let message = refused(&mut client, put_away).await;
        assert!(
            message.contains("thread-1") && message.contains("archived"),
            "{message}"
        );
    }

    // Nothing moved, and the connection is still usable: a refusal costs one
    // call.
    let untouched = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(untouched["settledOverride"], Value::Null);
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// A conversation whose agent is working cannot be settled, and can be the moment
/// it stops.
///
/// The first of the three invariants that need a real agent. Settling this would
/// hide work in progress from the developer who is doing it — which is the whole
/// reason the guard is the server's and not only the interface's.
#[tokio::test]
async fn a_conversation_whose_agent_is_working_cannot_be_settled() {
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
    // by here there is an agent to be refused for without reading a single event.
    let message = refused(&mut client, settle("thread-1")).await;
    assert!(
        message.contains("thread-1") && message.contains("work in progress"),
        "{message}"
    );

    client.events_through_the_turn(&thread).await;

    client
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();
    assert_eq!(
        as_a_fresh_subscriber_sees_it(&server, "thread-1").await["settledOverride"],
        "settled",
        "a finished conversation is exactly what settling is for"
    );

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// A conversation with an unanswered permission request cannot be settled.
///
/// The agent has stopped and is waiting on the developer, so nothing is
/// *running* — this is the case a guard that only asked about the session would
/// let through, and it would park a request for a decision somewhere the
/// developer had just told the interface not to look.
#[tokio::test]
async fn a_conversation_waiting_on_a_permission_decision_cannot_be_settled() {
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

    let message = refused(&mut client, settle("thread-1")).await;
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
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// A conversation with an unanswered question cannot be settled either.
///
/// Its own test rather than a second case in the one above, for the reason the
/// two folds are separate in the client: a question that arrived as an approval
/// is the bug those suites exist to catch, so a guard that read either flag as
/// the other would pass on it.
#[tokio::test]
async fn a_conversation_waiting_on_an_answer_cannot_be_settled() {
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

    let message = refused(&mut client, settle("thread-1")).await;
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
        .call("orchestration.dispatchCommand", settle("thread-1"))
        .await
        .expect_success();

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}
