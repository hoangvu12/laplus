//! The two mode pickers, driven the way the UI drives them.
//!
//! Ticket 02 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, the commands `client-runtime/src/operations/commands.ts`
//! builds, and the two subscriptions the real client folds. Nothing here reaches
//! into the server.
//!
//! Both modes were **write-once** before this. They were set when the thread was
//! created and could only be moved by the per-turn override that rides along with
//! a turn request, so the pickers changed nothing on their own — and a developer
//! who reopened the window found the mode they had chosen replaced by the one the
//! conversation started with. That is why the assertions here lean on what a
//! *fresh* subscriber sees and on what survives a restart: an event that reaches
//! an open subscription proves only that it was broadcast.
//!
//! Each command is asserted three ways, which is the shape the spec asks for:
//! the sequence it answers with, the events that reach a subscriber on the thread
//! and on the project list — including one on a second connection — and what a
//! subscriber that arrives afterwards is handed.

mod harness;

use harness::agent::{ScriptedAgent, PAUSE};
use harness::conversation::{create_project, create_thread, kinds, start_turn};
use harness::workspace::Workspace;
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// The `thread.runtime-mode.set` `setThreadRuntimeMode` builds.
///
/// `createdAt` is sent because the contract requires it of the client. This
/// server ignores it — see `orchestration::SetRuntimeMode` — and the tests below
/// are why that is safe to assert rather than merely believe: nothing they check
/// reads it back.
fn set_runtime_mode(thread_id: &str, mode: &str) -> Value {
    json!({
        "type": "thread.runtime-mode.set",
        "commandId": format!("test:runtime-mode:{thread_id}:{mode}"),
        "threadId": thread_id,
        "runtimeMode": mode,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// The `thread.interaction-mode.set` `setThreadInteractionMode` builds.
fn set_interaction_mode(thread_id: &str, mode: &str) -> Value {
    json!({
        "type": "thread.interaction-mode.set",
        "commandId": format!("test:interaction-mode:{thread_id}:{mode}"),
        "threadId": thread_id,
        "interactionMode": mode,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// A follow-up turn carrying **no** per-turn override.
///
/// Deliberately not `harness::conversation::follow_up`, which sends the
/// composer's current selection the way the real client does. Absent means
/// "unchanged" (`orchestration::StartTurn`), so this is the payload that asks the
/// question this file is about: with nothing on the turn to say otherwise, which
/// mode does the next turn run under?
fn follow_up_taking_the_threads_word(thread_id: &str, message_id: &str) -> Value {
    json!({
        "type": "thread.turn.start",
        "commandId": format!("test:turn:{message_id}"),
        "threadId": thread_id,
        "message": {
            "messageId": message_id,
            "role": "user",
            "text": "again",
            "attachments": [],
        },
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// Register a project and a conversation in it, without starting a turn.
///
/// Most of these tests want a thread and no agent: the modes are fields on the
/// conversation, and only the last two have anything to do with a session.
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

/// The conversation as a subscriber that arrives *afterwards* is handed it.
///
/// The picker reads `thread.runtimeMode` and `thread.interactionMode`
/// (`ChatView.tsx:1411`, where the composer's own selection falls back to the
/// thread's), so this is what "what the picker shows next time" means.
async fn as_a_fresh_subscriber_sees_it(server: &TestServer, thread_id: &str) -> Value {
    server
        .connect()
        .await
        .into_thread_snapshot(thread_id)
        .await["thread"]
        .clone()
}

/// The conversation's summary on the project list, which renders the mode too.
async fn on_the_project_list(server: &TestServer, thread_id: &str) -> Value {
    let snapshot = server.connect().await.into_shell_snapshot().await;
    snapshot["threads"]
        .as_array()
        .expect("the list carries its conversations")
        .iter()
        .find(|thread| thread["id"] == thread_id)
        .unwrap_or_else(|| panic!("{thread_id} is not on the list: {snapshot:#?}"))
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

/// The ticket's first three lines together: each command answers with the
/// sequence it committed at, and each change is published on the thread's own
/// feed *and* on the project list at that number.
///
/// Both feeds, because the UI reads them in different places and neither can be
/// derived from the other — the thread view folds the event, and the sidebar
/// folds the summary. A change only one of them heard about would leave two
/// views of one conversation disagreeing about what the agent may do.
#[tokio::test]
async fn each_mode_change_is_answered_with_a_sequence_and_published_on_both_feeds() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let shell = open_shell(&mut client).await;
    let thread = client.watch_conversation("thread-1").await;

    let moved = client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "approval-required"),
        )
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.runtime-mode-set"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(
        event["sequence"], moved["sequence"],
        "the event and the answer name the same commit"
    );
    assert_eq!(event["payload"]["threadId"], "thread-1");
    assert_eq!(event["payload"]["runtimeMode"], "approval-required");
    assert!(
        event["payload"]["updatedAt"].is_string(),
        "the reducer reads the thread's new updatedAt out of the payload: {event}"
    );

    let listed = next_items(&mut client, &shell).await;
    assert_eq!(listed.len(), 1, "{listed:#?}");
    assert_eq!(listed[0]["kind"], "thread-upserted");
    assert_eq!(listed[0]["sequence"], moved["sequence"]);
    assert_eq!(listed[0]["thread"]["runtimeMode"], "approval-required");

    let moved = client
        .call(
            "orchestration.dispatchCommand",
            set_interaction_mode("thread-1", "plan"),
        )
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.interaction-mode-set"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(event["sequence"], moved["sequence"]);
    assert_eq!(event["payload"]["threadId"], "thread-1");
    assert_eq!(event["payload"]["interactionMode"], "plan");

    let listed = next_items(&mut client, &shell).await;
    assert_eq!(listed[0]["kind"], "thread-upserted");
    assert_eq!(listed[0]["sequence"], moved["sequence"]);
    assert_eq!(listed[0]["thread"]["interactionMode"], "plan");

    client.interrupt(&thread).await;
    client.interrupt(&shell).await;
    client.close().await;

    // And what a subscriber arriving now is handed, which is the half that
    // proves the modes were *kept* rather than merely announced.
    let fresh = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(fresh["runtimeMode"], "approval-required");
    assert_eq!(fresh["interactionMode"], "plan");
    let summary = on_the_project_list(&server, "thread-1").await;
    assert_eq!(summary["runtimeMode"], "approval-required");
    assert_eq!(summary["interactionMode"], "plan");

    server.stop().await;
}

/// A second window has to see the first one's work. The change feed is the
/// server's, not the connection's.
#[tokio::test]
async fn a_mode_set_on_one_connection_reaches_a_subscriber_on_another() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;

    let mut author = server.connect().await;
    a_conversation(&mut author, &workspace).await;

    let mut watcher = server.connect().await;
    let shell = open_shell(&mut watcher).await;
    let thread = watcher.watch_conversation("thread-1").await;

    let moved = author
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "auto-accept-edits"),
        )
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(
        on_the_thread[0]["event"]["type"],
        "thread.runtime-mode-set",
        "{on_the_thread:#?}"
    );
    assert_eq!(on_the_thread[0]["event"]["sequence"], moved["sequence"]);
    assert_eq!(
        on_the_thread[0]["event"]["payload"]["runtimeMode"],
        "auto-accept-edits"
    );

    let listed = next_items(&mut watcher, &shell).await;
    assert_eq!(listed[0]["thread"]["runtimeMode"], "auto-accept-edits");

    watcher.interrupt(&thread).await;
    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// The ticket's reason for existing: the mode survives a restart and is what the
/// picker shows next time.
///
/// A restart is a second server on the same file, which is exactly the claim —
/// nothing but the path on disk carried the modes across. Before this ticket the
/// columns were there and were written only at creation, so a developer's choice
/// lasted until they closed the window.
#[tokio::test]
async fn the_modes_survive_a_restart_and_are_what_the_picker_shows_next_time() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);

    {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        a_conversation(&mut client, &workspace).await;
        client
            .call(
                "orchestration.dispatchCommand",
                set_runtime_mode("thread-1", "auto"),
            )
            .await
            .expect_success();
        client
            .call(
                "orchestration.dispatchCommand",
                set_interaction_mode("thread-1", "plan"),
            )
            .await
            .expect_success();
        client.close().await;
        server.stop().await;
    }

    let server = TestServer::start_at(&database).await;
    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        restored["runtimeMode"], "auto",
        "the runtime mode was forgotten: {restored:#?}"
    );
    assert_eq!(
        restored["interactionMode"], "plan",
        "the interaction mode was forgotten: {restored:#?}"
    );

    let summary = on_the_project_list(&server, "thread-1").await;
    assert_eq!(summary["runtimeMode"], "auto");
    assert_eq!(summary["interactionMode"], "plan");

    // Nothing is asserted about the sequence. A run resumes its numbering from
    // its last durable *registry* write, so the numbers a mode change took are
    // reissued by the run after it — which is ADR-0016's "I cannot tell you what
    // you missed; here is everything", not something these two commands changed.
    // What has to survive a restart is the mode, and that is what is checked.

    server.stop().await;
}

/// A mode the contract does not name is refused rather than rounded to the
/// nearest one this server understands, and an unknown thread is refused by name.
///
/// Rounding is the failure worth guarding: the nearest mode this server has a
/// `--permission-mode` for is `full-access`, so a typo that got rounded would
/// *widen* what the agent may do — the opposite of what a developer reaching for
/// the picker usually wants. Each refusal is asserted on the sentence, because
/// the sentence is all the contract's dispatch error carries.
#[tokio::test]
async fn an_unnameable_mode_or_an_unknown_thread_is_refused_with_a_sentence() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    let message = refusal(
        client
            .call(
                "orchestration.dispatchCommand",
                set_runtime_mode("thread-1", "bypassPermissions"),
            )
            .await,
    );
    assert!(
        message.contains("bypassPermissions")
            && message.contains("thread-1")
            && message.contains("full-access"),
        "{message}"
    );

    let message = refusal(
        client
            .call(
                "orchestration.dispatchCommand",
                set_interaction_mode("thread-1", "planning"),
            )
            .await,
    );
    assert!(
        message.contains("planning") && message.contains("thread-1") && message.contains("plan"),
        "{message}"
    );

    let message = refusal(
        client
            .call(
                "orchestration.dispatchCommand",
                set_runtime_mode("never-created", "auto"),
            )
            .await,
    );
    assert!(message.contains("never-created"), "{message}");

    // The conversation is untouched, and the connection is: a refusal costs one
    // call.
    let held = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(held["runtimeMode"], "full-access");
    assert_eq!(held["interactionMode"], "default");
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// Setting a mode to the value it already holds is harmless.
///
/// The real client only dispatches when the picker's value differs from the
/// thread's (`ChatView.tsx:3231`), so this is reached by a double-click, a retry,
/// or a second window that has not folded the first one's event yet. It is
/// answered rather than refused, and folding what it publishes a second time
/// lands on the same state — which is what "harmless" has to mean when this
/// server remembers no command ids.
#[tokio::test]
async fn setting_a_mode_to_the_value_it_already_holds_is_answered_rather_than_refused() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let thread = client.watch_conversation("thread-1").await;

    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "auto"),
        )
        .await
        .expect_success();
    next_items(&mut client, &thread).await;

    let again = client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "auto"),
        )
        .await
        .expect_success();
    let repeated = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&repeated), vec!["thread.runtime-mode-set"]);
    assert_eq!(repeated[0]["event"]["sequence"], again["sequence"]);
    assert_eq!(repeated[0]["event"]["payload"]["runtimeMode"], "auto");

    client.interrupt(&thread).await;
    client.close().await;

    let held = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(held["runtimeMode"], "auto");

    server.stop().await;
}

/// A mode set while a turn is running does not change that turn, and the next
/// turn starts under the new mode.
///
/// The rules an agent is working under must not move under its feet, so the set
/// lands on the *thread* and leaves the session alone — a session is a process
/// that was launched with a mode, and `session.runtimeMode` on the wire is that
/// launch's. The turn in flight therefore publishes nothing here, which is what
/// the absence of a `thread.session-set` between the two events proves.
///
/// The follow-up carries no per-turn override on purpose — see
/// [`follow_up_taking_the_threads_word`] — because that is the only payload for
/// which "the next turn starts under the new mode" is a claim about the stored
/// mode rather than about what the composer happened to send.
///
/// **What this does not claim** is that a *reused* CLI process honours the new
/// mode. One child serves a whole conversation
/// (`socket_turn.rs::one_subprocess_serves_the_conversation_…`), the CLI is given
/// `--permission-mode` once at launch, and the agent protocol has no request to
/// move it afterwards. That gap is older than these two commands — the per-turn
/// override has always had it — and is not this ticket's to close.
#[tokio::test]
async fn a_mode_set_mid_turn_leaves_that_turn_alone_and_the_next_turn_starts_under_it() {
    let agent = ScriptedAgent::emitting(&[
        r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":[]}"#,
        r#"{"type":"stream_event","event":{"type":"message_start"}}"#,
        r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"working"}}}"#,
        PAUSE,
        r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"working, done"}]}}"#,
        r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":1100,"total_cost_usd":0.01}"#,
    ]);
    let workspace = Workspace::with(&["src/"]);
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

    // Read as far as the first piece of assistant text. The agent has not sent
    // its buffered message and will not for about a second, so the turn is
    // genuinely in flight when the mode moves.
    let so_far = client.events_until_streaming(&subscription).await;
    let running = so_far
        .iter()
        .map(|item| &item["event"])
        .rfind(|event| event["type"] == "thread.session-set")
        .expect("the session announced itself");
    assert_eq!(
        running["payload"]["session"]["status"], "running",
        "the turn had already finished, so this proves nothing: {running}"
    );
    assert_eq!(running["payload"]["session"]["runtimeMode"], "full-access");

    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "approval-required"),
        )
        .await
        .expect_success();

    // The rest of the turn, which had already been decided. The mode change is
    // the only thing that reached the thread in the meantime — no
    // `thread.session-set` between it and the turn's own ending, which is what
    // "the turn in flight is untouched" looks like on the wire.
    let rest = client.events_through_the_turn(&subscription).await;
    let moved = rest
        .iter()
        .position(|item| item["event"]["type"] == "thread.runtime-mode-set")
        .unwrap_or_else(|| panic!("the mode never moved: {:?}", kinds(&rest)));
    assert!(
        !kinds(&rest[..moved]).contains(&"thread.session-set"),
        "the running session was republished before the mode moved: {:?}",
        kinds(&rest)
    );
    assert_eq!(
        rest[moved]["event"]["payload"]["runtimeMode"],
        "approval-required"
    );

    // The next turn takes the thread's word for it.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_taking_the_threads_word("thread-1", "message-2"),
        )
        .await
        .expect_success();
    let second = client.events_through_the_turn(&subscription).await;

    let requested = second
        .iter()
        .map(|item| &item["event"])
        .find(|event| event["type"] == "thread.turn-start-requested")
        .unwrap_or_else(|| panic!("no turn was requested: {:?}", kinds(&second)));
    assert_eq!(
        requested["payload"]["runtimeMode"], "approval-required",
        "the second turn was requested under the mode the thread was created \
         with rather than the one that was set"
    );

    let started = second
        .iter()
        .map(|item| &item["event"])
        .find(|event| {
            event["type"] == "thread.session-set"
                && event["payload"]["session"]["status"] == "starting"
        })
        .unwrap_or_else(|| panic!("no session started: {:?}", kinds(&second)));
    assert_eq!(
        started["payload"]["session"]["runtimeMode"], "approval-required",
        "the session for the second turn reports the old mode: {started}"
    );

    client.close().await;
    server.stop().await;
}

/// The last of the ticket's lines that needs a running agent: a mode set while
/// nothing is running is the ordinary case, and it must not conjure a session.
///
/// A conversation with no session is normal — after a restart every thread has
/// none — and a mode command that published one would put a badge on a
/// conversation with no process behind it.
#[tokio::test]
async fn a_mode_set_with_nothing_running_does_not_conjure_a_session() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "approval-required"),
        )
        .await
        .expect_success();
    client.close().await;

    let held = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(held["runtimeMode"], "approval-required");
    assert_eq!(held["session"], Value::Null, "{held:#?}");
    assert_eq!(held["latestTurn"], Value::Null);
    assert_eq!(server.live_agents(), 0);

    server.stop().await;
}

// ---------------------------------------------------------------------------
// Ticket 11: the change reaching the process
// ---------------------------------------------------------------------------
//
// Everything above this line is about the *conversation* holding a mode. What
// follows is about the `claude` already serving it, which the tests above could
// not have caught: a mode moved the picker, survived a restart and reached the
// next turn's request, and the agent went on working under the mode the
// conversation opened with because `--permission-mode` is read once, at launch.
//
// The push these drive is recorded in
// `fixtures/claude-cli/20-modes-changed-mid-conversation.ndjson`, against a real
// `claude` 2.1.220: a mode tightened between two turns, and the second turn
// asking permission where the first had not.

/// A plain reply, for a turn whose content is beside the point.
const REPLY: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const ENDED: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","duration_ms":10,"total_cost_usd":0.01}"#;

/// The `user` line `claude` writes itself when a `set_model` push lands.
///
/// The one trap this mechanism ships with, recorded verbatim in
/// `fixtures/claude-cli/20-modes-changed-mid-conversation.ndjson`: it is marked
/// `isReplay`, and its `content` is a bare string where every other user line
/// carries a list of blocks.
const NARRATED: &str = r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>Set model to claude-haiku-4-5 (claude-haiku-4-5-20251001)</local-command-stdout>"},"session_id":"s","parent_tool_use_id":null,"uuid":"u","timestamp":"2026-07-30T19:45:11.527Z","isReplay":true}"#;

/// A follow-up carrying the composer's model picker, which is the *other* door a
/// mode or model comes through — the per-turn override, which has had this same
/// hole for longer than the picker's own command has.
fn follow_up_choosing(thread_id: &str, message_id: &str, model: &str) -> Value {
    json!({
        "type": "thread.turn.start",
        "commandId": format!("test:turn:{message_id}"),
        "threadId": thread_id,
        "message": {
            "messageId": message_id,
            "role": "user",
            "text": "again",
            "attachments": [],
        },
        "modelSelection": {"instanceId": "claudeAgent", "model": model},
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

/// Every `set_permission_mode` and `set_model` the agent was written, in order.
///
/// Read off the stand-in rather than out of an event, because the point is what
/// reached the *process*: a server that published the change and told the child
/// nothing would pass every other assertion in this file.
fn pushes(agent: &ScriptedAgent) -> Vec<Value> {
    agent
        .requests()
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|line| line["request"]["subtype"] != "get_context_usage")
        .collect()
}

/// The ticket's first four acceptance criteria in one conversation: a mode
/// tightened between turns is *pushed* to the child that is already serving the
/// conversation, as the CLI's `default` rather than as no push at all, and only
/// when it has actually changed.
///
/// Three turns rather than two, and the third is the guard: it runs under the
/// same mode as the second, so a server pushing on every dispatch would send two
/// and this would say so.
#[tokio::test]
async fn a_tightened_mode_is_pushed_to_the_agent_already_serving_the_conversation() {
    let agent = ScriptedAgent::per_turn(&[[REPLY, ENDED], [REPLY, ENDED], [REPLY, ENDED]]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "first"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    assert!(
        pushes(&agent).is_empty(),
        "the opening turn's mode is a launch flag, so there was nothing to push: {:?}",
        pushes(&agent)
    );

    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "approval-required"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_taking_the_threads_word("thread-1", "message-2"),
        )
        .await
        .expect_success();
    let second = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        pushes(&agent),
        vec![json!({
            "type": "control_request",
            "request_id": "retune-1",
            "request": {"subtype": "set_permission_mode", "mode": "default"},
        })],
        "`approval-required` has to be pushed as the CLI's `default`, which is \
         the mode whose behaviour is to ask — not as the nothing the launch \
         table maps it to"
    );

    // Every session event for this turn agrees. Before ticket 11 the `starting`
    // one came from the thread and the `running` one from the driver's capture,
    // so one turn announced two modes and the badge flipped and flipped back.
    let announced: Vec<(&str, &Value)> = second
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.session-set")
        .map(|event| {
            (
                event["payload"]["session"]["status"].as_str().unwrap_or("?"),
                &event["payload"]["session"]["runtimeMode"],
            )
        })
        .collect();
    assert!(
        announced.iter().any(|(status, _)| *status == "starting")
            && announced.iter().any(|(status, _)| *status == "running"),
        "the turn did not announce itself: {announced:?}"
    );
    for (status, mode) in &announced {
        assert_eq!(
            *mode, "approval-required",
            "the {status} session reports a mode the agent is not running under: {announced:?}"
        );
    }

    // A third turn under the same mode, which is what says the guard is real.
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_taking_the_threads_word("thread-1", "message-3"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    assert_eq!(
        pushes(&agent).len(),
        1,
        "a turn whose mode had not moved sent a request anyway: {:?}",
        pushes(&agent)
    );
    // And the session was never replaced to achieve any of it — one process took
    // all three turns, which is the whole reason a push beats a restart.
    assert_eq!(agent.starts(), 1);
    assert_eq!(server.live_agents(), 1);

    client.close().await;
    server.stop().await;
}

/// The model's half of the same defect, through the per-turn override rather
/// than a picker command: a developer who switches model mid-conversation is
/// answered by the model they picked, and the session is not replaced to manage
/// it.
///
/// The CLI's own narration of the change is replayed here, because it is the one
/// trap this mechanism ships with: `set_model` makes `claude` write itself a
/// `user` line, and this server folded every user line into the transcript. The
/// assertion is that the developer's conversation does not grow a turn they
/// never typed.
#[tokio::test]
async fn a_model_changed_mid_conversation_reaches_the_agent_without_adding_a_turn() {
    let agent = ScriptedAgent::per_turn(&[
        vec![REPLY, ENDED],
        vec![NARRATED, REPLY, ENDED],
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "first"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_choosing("thread-1", "message-2", "claude-haiku-4-5"),
        )
        .await
        .expect_success();
    let turn = client.events_through_the_turn(&subscription).await;

    assert_eq!(
        pushes(&agent),
        vec![json!({
            "type": "control_request",
            "request_id": "retune-1",
            "request": {"subtype": "set_model", "model": "claude-haiku-4-5"},
        })],
        "the slug the thread holds is what the push carries — the CLI resolves \
         it itself — and the mode has not moved, so nothing else is sent"
    );
    assert_eq!(agent.starts(), 1, "the session was replaced to change a model");

    let said: Vec<&str> = turn
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.message-sent")
        .filter_map(|event| event["payload"]["message"]["text"].as_str())
        .collect();
    assert!(
        !said.iter().any(|text| text.contains("local-command-stdout")),
        "the CLI's narration of the model change became a message: {said:?}"
    );

    // Nor did it register as a format this build cannot read, which is what a
    // line that failed to parse would have been counted as.
    let completed = turn
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .rfind(|activity| activity["kind"] == "turn.completed")
        .expect("the turn ended");
    assert_eq!(completed["payload"]["parseErrors"], 0, "{completed}");
    assert_eq!(completed["payload"]["unknownEvents"], 0, "{completed}");

    client.close().await;
    server.stop().await;
}

/// A push the CLI will not take, end to end: the id this server minted, the
/// refusal that names it, the sentence the developer reads, and the badge
/// correcting itself rather than going on claiming a mode nothing is running
/// under.
///
/// The unit tests in `crate::turn` decide what the *answer* means; this is the
/// only place that says the id the driver mints and the id it matches against
/// are the same one.
#[tokio::test]
async fn a_push_the_agent_refuses_is_said_and_the_badge_goes_back() {
    let refused = r#"{"type":"control_response","response":{"subtype":"error","request_id":"retune-1","error":"Cannot set permission mode: must be one of acceptEdits, auto, bypassPermissions, default, dontAsk, plan"}}"#;
    let agent = ScriptedAgent::per_turn(&[vec![REPLY, ENDED], vec![refused, REPLY, ENDED]]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "first"),
        )
        .await
        .expect_success();
    client.events_through_the_turn(&subscription).await;

    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "approval-required"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_taking_the_threads_word("thread-1", "message-2"),
        )
        .await
        .expect_success();
    let turn = client.events_through_the_turn(&subscription).await;

    let said = turn
        .iter()
        .map(|item| &item["event"])
        .filter(|event| event["type"] == "thread.activity-appended")
        .map(|event| &event["payload"]["activity"])
        .find(|activity| activity["kind"] == "session.retune-refused")
        .unwrap_or_else(|| panic!("the refusal was never said: {:?}", kinds(&turn)));
    let summary = said["summary"].as_str().expect("a sentence");
    assert!(
        summary.contains("approval-required") && summary.contains("full-access"),
        "the sentence has to name what was refused and what is still running: {summary}"
    );

    // And the last thing said about the session reports what the child is really
    // running under rather than what it was asked for.
    let last = turn
        .iter()
        .map(|item| &item["event"])
        .rfind(|event| event["type"] == "thread.session-set")
        .expect("the session said something");
    assert_eq!(
        last["payload"]["session"]["runtimeMode"], "full-access",
        "the session went on claiming a mode the agent had refused: {last}"
    );

    client.close().await;
    server.stop().await;
}

/// Two turns queued behind a running one, with the picker moved between them.
///
/// The pairing this rests on is why what a turn wants travels *with the prompt*
/// rather than in a slot beside the queue: a single latest-wins slot collapses
/// these two onto the second's mode, and the first turn is then answered under
/// rules it was never requested under.
#[tokio::test]
async fn a_turn_queued_behind_another_keeps_its_own_mode() {
    let agent = ScriptedAgent::per_turn(&[
        vec![REPLY, PAUSE, ENDED],
        vec![REPLY, ENDED],
        vec![REPLY, ENDED],
    ]);
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start_with_agent(&agent.configured()).await;
    let mut client = server.connect().await;

    let subscription = client.open_conversation(&workspace, "thread-1").await;
    client
        .call(
            "orchestration.dispatchCommand",
            start_turn("thread-1", "message-1", "first"),
        )
        .await
        .expect_success();

    // Both follow-ups are dispatched while the first turn is still running, so
    // they are queued rather than served — and each carries a different mode.
    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "auto-accept-edits"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_taking_the_threads_word("thread-1", "message-2"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            set_runtime_mode("thread-1", "approval-required"),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            follow_up_taking_the_threads_word("thread-1", "message-3"),
        )
        .await
        .expect_success();

    // Read until all three turns have ended, counted rather than assumed: a
    // follow-up dispatched while another turn is running claims the conversation
    // before the turn ahead of it finishes, so the settle that turn would have
    // published is skipped — see `spend` — and how many times the session goes
    // quiet is therefore not the number of turns.
    let mut ended = 0;
    while ended < 3 {
        ended += client
            .events_through_the_turn(&subscription)
            .await
            .iter()
            .filter(|item| item["event"]["payload"]["activity"]["kind"] == "turn.completed")
            .count();
    }

    let modes: Vec<Value> = pushes(&agent)
        .iter()
        .map(|push| push["request"]["mode"].clone())
        .collect();
    assert_eq!(
        modes,
        vec![json!("acceptEdits"), json!("default")],
        "the second turn was answered under the third's mode: {:?}",
        pushes(&agent)
    );
    assert_eq!(agent.starts(), 1);

    client.close().await;
    server.stop().await;
}
