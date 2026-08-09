//! Archiving a conversation and getting it back, driven the way the UI drives
//! it.
//!
//! Ticket 06 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, the commands `client-runtime/src/operations/commands.ts`
//! builds, and the two subscriptions the real client folds. Nothing here reaches
//! into the server.
//!
//! **This is the first slice that lets the inbox be cleared.** Before it the
//! project list carried every conversation ever started, forever, so the one that
//! needed attention was buried among the ones that did not — and both commands
//! were refused by name, so there was no way to move any of them.
//!
//! ## The two lists
//!
//! Archiving is a move between two snapshots rather than a deletion. The project
//! list stops carrying the conversation; `orchestration.getArchivedShellSnapshot`
//! starts, and is the only way back, because the unarchive control lives on the
//! settings panel drawn from it. So most of what is asserted here is asserted
//! twice — once on each list — and
//! [`the_two_snapshots_are_one_snapshot_filtered_two_ways`] is the test that says
//! they are the same object rather than two that agree today.
//!
//! Every command is asserted the three ways the spec asks for: the sequence it
//! answers with, the events that reach a subscriber on the thread and on the
//! project list — including one on a second connection — and what a subscriber
//! that arrives *afterwards* is handed, which is the half that proves the state
//! was stored rather than merely broadcast.

mod harness;

use harness::agent::ScriptedAgent;
use harness::conversation::{create_project, create_thread, kinds, start_turn};
use harness::workspace::Workspace;
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// The lines of a turn that says one thing and stops. The same three the diff
/// suites use, minus anything that touches the project: this file is about a
/// conversation surviving a round trip, not about what a turn did to a tree.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Write"]}"#;
const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

/// The `thread.archive` the sidebar's context menu builds.
///
/// Two fields and no third: the contract's own command is a `threadId` and a
/// `commandId`, and this server does not remember command ids.
fn archive(thread_id: &str) -> Value {
    json!({
        "type": "thread.archive",
        "commandId": format!("test:archive:{thread_id}"),
        "threadId": thread_id,
    })
}

/// The `thread.unarchive` the archived section of the settings panel builds —
/// the only control that reverses the one above.
fn unarchive(thread_id: &str) -> Value {
    json!({
        "type": "thread.unarchive",
        "commandId": format!("test:unarchive:{thread_id}"),
        "threadId": thread_id,
    })
}

/// Register a project and a conversation in it. No turn: archiving is a field on
/// the thread, and the tests that need a transcript say so.
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

/// The ids on the project list, as a subscriber arriving now is handed them.
async fn listed(server: &TestServer) -> Vec<String> {
    ids(&server.connect().await.into_shell_snapshot().await)
}

/// The ids on the archived list, asked for the way the settings panel asks.
async fn archived(server: &TestServer) -> Vec<String> {
    ids(&archived_snapshot(server).await)
}

/// The whole `orchestration.getArchivedShellSnapshot` answer.
async fn archived_snapshot(server: &TestServer) -> Value {
    let mut client = server.connect().await;
    let snapshot = client
        .call("orchestration.getArchivedShellSnapshot", json!({}))
        .await
        .expect_success();
    client.close().await;
    snapshot
}

fn ids(snapshot: &Value) -> Vec<String> {
    snapshot["threads"]
        .as_array()
        .unwrap_or_else(|| panic!("a snapshot carries its conversations: {snapshot:#?}"))
        .iter()
        .map(|thread| thread["id"].as_str().unwrap_or("<no id>").to_string())
        .collect()
}

/// One conversation out of a snapshot's list.
fn thread_in(snapshot: &Value, thread_id: &str) -> Value {
    snapshot["threads"]
        .as_array()
        .expect("a snapshot carries its conversations")
        .iter()
        .find(|thread| thread["id"] == thread_id)
        .unwrap_or_else(|| panic!("{thread_id} is not on this list: {snapshot:#?}"))
        .clone()
}

/// The conversation as a subscriber that arrives *afterwards* is handed it.
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

/// Archiving answers with the sequence it committed at and publishes on the
/// conversation's own feed *and* on the project list at that number.
///
/// Both feeds, because the UI reads them in different places and neither can be
/// derived from the other: the thread view folds the event and the sidebar folds
/// the summary. An archive only one of them heard about would leave the developer
/// looking at a conversation that is both put away and in front of them.
#[tokio::test]
async fn archiving_is_answered_with_a_sequence_and_published_on_both_feeds() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let shell = open_shell(&mut client).await;
    let thread = client.watch_conversation("thread-1").await;

    let archived_at = client
        .call("orchestration.dispatchCommand", archive("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.archived"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(
        event["sequence"], archived_at["sequence"],
        "the event and the answer name the same commit"
    );
    assert_eq!(event["payload"]["threadId"], "thread-1");
    // Both stamps, because the client's reducer writes both onto the thread
    // (`threadReducer.ts`, `case "thread.archived"`). A payload carrying only
    // one would leave a window that watched the archive disagreeing with one
    // that reloaded after it about when the conversation last changed.
    let stamped = event["payload"]["archivedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("an archive says when: {event}"));
    assert_eq!(
        event["payload"]["updatedAt"], json!(stamped),
        "the two stamps are one moment: {event}"
    );

    let on_the_list = next_items(&mut client, &shell).await;
    assert_eq!(on_the_list.len(), 1, "{on_the_list:#?}");
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(on_the_list[0]["sequence"], archived_at["sequence"]);
    // The summary carries the stamp rather than the conversation vanishing from
    // the feed: the client's shell reducer upserts by id and every view filters
    // on `archivedAt === null`, so this is what makes the sidebar drop it.
    assert_eq!(on_the_list[0]["thread"]["archivedAt"], json!(stamped));

    client.interrupt(&thread).await;
    client.interrupt(&shell).await;
    client.close().await;

    assert_eq!(listed(&server).await, Vec::<String>::new());
    assert_eq!(archived(&server).await, vec!["thread-1".to_string()]);

    server.stop().await;
}

/// Unarchiving puts the conversation back on the list, and it is the same
/// conversation: transcript, work log and checkpoints all still there.
///
/// A real turn first, because that is what puts something in all three. This is
/// the criterion that says archiving is not deleting — the fields the archive
/// moved are the only fields it moved, and everything a developer would come back
/// for is still where they left it.
#[tokio::test]
async fn an_unarchived_conversation_comes_back_with_everything_in_it() {
    let agent = ScriptedAgent::emitting(&[INIT, SAID, DONE]);
    let workspace = Workspace::with(&["src/"]);
    // A repository with something in it, because a checkpoint is a photograph of
    // a working tree and there is no photographing an empty one.
    workspace.put("kept.txt", "one\n");
    workspace.init_repository().commit("the beginning");
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
    client.events_through_the_checkpoint(&thread, 1).await;

    let before = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert!(
        !before["messages"].as_array().expect("messages").is_empty()
            && !before["activities"].as_array().expect("a work log").is_empty()
            && !before["checkpoints"]
                .as_array()
                .expect("checkpoints")
                .is_empty(),
        "the turn has to leave all three behind or this test proves nothing: {before:#?}"
    );

    client
        .call("orchestration.dispatchCommand", archive("thread-1"))
        .await
        .expect_success();
    assert_eq!(listed(&server).await, Vec::<String>::new());

    client
        .call("orchestration.dispatchCommand", unarchive("thread-1"))
        .await
        .expect_success();

    assert_eq!(listed(&server).await, vec!["thread-1".to_string()]);
    assert_eq!(archived(&server).await, Vec::<String>::new());

    let after = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(after["messages"], before["messages"]);
    assert_eq!(after["activities"], before["activities"]);
    assert_eq!(after["checkpoints"], before["checkpoints"]);
    assert_eq!(after["archivedAt"], Value::Null);
    // The round trip is visible in exactly one other place, and it is the one
    // the contract says it should be: the conversation's own `updatedAt`.
    assert_ne!(after["updatedAt"], before["updatedAt"]);

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// The archived answer is the project list's own snapshot, filtered.
///
/// The ticket asks for one builder by name, and this is what that buys
/// observably: a conversation's summary is byte-for-byte the same on whichever
/// list is carrying it, apart from the two fields the archive moved. Two builders
/// would let the world a client draws depend on which of them answered.
///
/// The projects are asserted too, and they are *not* filtered: the settings panel
/// groups archived threads by project and looks each one up in this same answer
/// (`SettingsPanels.tsx`), so a project list narrowed alongside the threads would
/// silently drop them.
#[tokio::test]
async fn the_two_snapshots_are_one_snapshot_filtered_two_ways() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    client
        .call(
            "orchestration.dispatchCommand",
            create_thread("project-1", "thread-2"),
        )
        .await
        .expect_success();

    let on_the_list = thread_in(
        &server.connect().await.into_shell_snapshot().await,
        "thread-1",
    );

    client
        .call("orchestration.dispatchCommand", archive("thread-1"))
        .await
        .expect_success();

    // Only the archived one moved. The other conversation stays exactly where it
    // was, which is the whole reason the developer archived the first.
    assert_eq!(listed(&server).await, vec!["thread-2".to_string()]);
    assert_eq!(archived(&server).await, vec!["thread-1".to_string()]);

    let snapshot = archived_snapshot(&server).await;
    let put_away = thread_in(&snapshot, "thread-1");
    for (field, moved) in [
        ("archivedAt", true),
        ("updatedAt", true),
        ("id", false),
        ("projectId", false),
        ("title", false),
        ("modelSelection", false),
        ("runtimeMode", false),
        ("interactionMode", false),
        ("createdAt", false),
        ("latestTurn", false),
        ("session", false),
        ("latestUserMessageAt", false),
        ("hasPendingApprovals", false),
        ("hasPendingUserInput", false),
        ("settledOverride", false),
        ("deletedAt", false),
    ] {
        let same = put_away[field] == on_the_list[field];
        assert_eq!(
            same, !moved,
            "{field}: {} on the project list, {} on the archived one",
            on_the_list[field], put_away[field]
        );
    }

    // The registry, whole. `project-1` still has a live conversation in it, and
    // it would still have to be here if it did not.
    assert_eq!(
        snapshot["projects"]
            .as_array()
            .expect("the registry")
            .len(),
        1,
        "{snapshot:#?}"
    );
    assert!(
        snapshot["snapshotSequence"].is_i64() && snapshot["updatedAt"].is_string(),
        "the archived answer is a whole shell snapshot: {snapshot:#?}"
    );
    // `updatedAt` describes the snapshot it is on rather than the registry as a
    // whole, so the archived answer's is the archived conversation's moment and
    // not the one still on the project list.
    assert_eq!(
        snapshot["updatedAt"], put_away["updatedAt"],
        "the archived answer is timestamped by a conversation it does not carry: {snapshot:#?}"
    );

    client.close().await;
    server.stop().await;
}

/// The archived answer of a server with nothing archived is empty, and says so
/// without borrowing a timestamp from the conversations it does not carry.
///
/// The state the settings panel opens in for almost every developer, and the case
/// a `updatedAt` read across every thread would get wrong: it would report a
/// change to a list that has never had anything on it.
#[tokio::test]
async fn an_empty_archive_describes_itself_from_the_registry() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    let working = server.connect().await.into_shell_snapshot().await;
    let snapshot = archived_snapshot(&server).await;
    assert_eq!(snapshot["threads"], json!([]), "{snapshot:#?}");
    assert_eq!(
        snapshot["updatedAt"], working["updatedAt"],
        "an empty archive borrowed a moment from the project list: {snapshot:#?}"
    );

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
        .call("orchestration.dispatchCommand", archive("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.archived"]);
    assert_eq!(on_the_thread[0]["event"]["sequence"], answered["sequence"]);
    let stamped = on_the_thread[0]["event"]["payload"]["archivedAt"].clone();
    assert!(stamped.is_string(), "{on_the_thread:#?}");

    let on_the_list = next_items(&mut watcher, &shell).await;
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(on_the_list[0]["thread"]["archivedAt"], stamped);

    let answered = author
        .call("orchestration.dispatchCommand", unarchive("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.unarchived"]);
    assert_eq!(on_the_thread[0]["event"]["sequence"], answered["sequence"]);
    assert_eq!(on_the_thread[0]["event"]["payload"]["threadId"], "thread-1");
    // The contract's `ThreadUnarchivedPayload` is a thread and a timestamp, and
    // the reducer clears the stamp rather than reading a new one: there is no
    // such thing as when a conversation stopped being archived.
    assert!(
        on_the_thread[0]["event"]["payload"]["updatedAt"].is_string(),
        "{on_the_thread:#?}"
    );

    let on_the_list = next_items(&mut watcher, &shell).await;
    assert_eq!(on_the_list[0]["kind"], "thread-upserted");
    assert_eq!(on_the_list[0]["thread"]["archivedAt"], Value::Null);

    watcher.interrupt(&thread).await;
    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// An archive survives a restart, and a subscriber that arrives after one holds
/// what a subscriber that watched it happen holds.
///
/// A restart is a second server on the same file — nothing but the path on disk
/// carried the state across. The agreement is the second half and is the point of
/// asserting it: the conversation has to be on the *same* list either way, or
/// which list a developer finds it on depends on when they opened the window.
#[tokio::test]
async fn archive_state_survives_a_restart_and_a_fresh_subscriber_agrees() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);

    let watched = {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        a_conversation(&mut client, &workspace).await;
        let thread = client.watch_conversation("thread-1").await;

        client
            .call("orchestration.dispatchCommand", archive("thread-1"))
            .await
            .expect_success();
        let seen = next_items(&mut client, &thread).await;
        let watched = seen[0]["event"]["payload"]["archivedAt"].clone();

        client.interrupt(&thread).await;
        client.close().await;
        server.stop().await;
        watched
    };

    let server = TestServer::start_at(&database).await;

    assert_eq!(listed(&server).await, Vec::<String>::new());
    assert_eq!(archived(&server).await, vec!["thread-1".to_string()]);
    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        restored["archivedAt"], watched,
        "the stamp a subscriber watched is not the stamp a fresh one is handed: {restored:#?}"
    );

    // And it can still be brought back after a restart, which is what makes the
    // archive a place rather than a hole.
    let mut client = server.connect().await;
    client
        .call("orchestration.dispatchCommand", unarchive("thread-1"))
        .await
        .expect_success();
    client.close().await;
    assert_eq!(listed(&server).await, vec!["thread-1".to_string()]);

    server.stop().await;
}

/// Every refusal, on the sentence — which is all
/// `OrchestrationDispatchCommandError` carries, so the sentence *is* the
/// diagnostic the developer is shown.
///
/// A repeat is refused rather than answered, which is where these two commands
/// part company with the renames and the mode pickers: this is a move between two
/// lists rather than a write of a value, and a second archive is a click on a
/// control that is no longer there.
#[tokio::test]
async fn a_blank_or_unknown_conversation_and_a_repeat_are_refused_with_a_sentence() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    for blank in [archive("  "), unarchive("  ")] {
        let message = refusal(client.call("orchestration.dispatchCommand", blank).await);
        assert!(message.contains("threadId"), "{message}");
    }

    for unknown in [archive("never-created"), unarchive("never-created")] {
        let message = refusal(client.call("orchestration.dispatchCommand", unknown).await);
        assert!(message.contains("never-created"), "{message}");
    }

    // Not archived yet, so there is nothing to bring back.
    let message = refusal(
        client
            .call("orchestration.dispatchCommand", unarchive("thread-1"))
            .await,
    );
    assert!(
        message.contains("thread-1") && message.contains("not archived"),
        "{message}"
    );

    client
        .call("orchestration.dispatchCommand", archive("thread-1"))
        .await
        .expect_success();

    let message = refusal(
        client
            .call("orchestration.dispatchCommand", archive("thread-1"))
            .await,
    );
    assert!(
        message.contains("thread-1") && message.contains("already archived"),
        "{message}"
    );

    // Nothing moved, and the connection is still usable: a refusal costs one
    // call.
    assert_eq!(archived(&server).await, vec!["thread-1".to_string()]);
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}
