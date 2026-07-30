//! Deleting a conversation, driven the way the UI drives it.
//!
//! Ticket 10 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, the command `client-runtime/src/operations/commands.ts` builds,
//! and the two subscriptions the real client folds. Nothing here reaches into the
//! server.
//!
//! ## Deleting is soft, and that is what most of this asserts
//!
//! The row stays, and so do its transcript, its work log and its checkpoints —
//! because the checkpoint refs a turn wrote are real git objects in the
//! developer's own repository, because the threads table cascades, and because
//! the contract carries a deletion time that is only meaningful if the thread
//! survives to carry it. So the tests come in two halves: what the developer sees
//! (a conversation gone from both lists, refusing every command and every fresh
//! subscription) and what is still there behind it.
//!
//! **Reading the second half needs the one door a deletion leaves open.** A fresh
//! subscription is refused and so is the HTTP snapshot, deliberately: the client
//! seeds a pane from that route and then subscribes with a cursor, so a route
//! that answered would leave a window drawing a conversation it could never be
//! told was deleted. A *resume* — a client saying it already holds the
//! conversation — still opens, and that is the rule this ticket left alone, so it
//! is how [`what_is_left_behind`] reads the transcript back.

mod harness;

use harness::agent::ScriptedAgent;
use harness::conversation::{create_project, create_thread, kinds, start_turn};
use harness::workspace::Workspace;
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// A turn that writes a file and says so — enough to leave a transcript, a work
/// log and a checkpoint behind, which is what "the deletion kept everything"
/// needs to be about.
const INIT: &str = r#"{"type":"system","subtype":"init","session_id":"s","model":"claude-opus-5","cwd":".","permissionMode":"bypassPermissions","tools":["Write"]}"#;
const SAID: &str =
    r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]}}"#;
const DONE: &str = r#"{"type":"result","subtype":"success","is_error":false,"stop_reason":"end_turn","num_turns":1,"duration_ms":10,"total_cost_usd":0.001}"#;

/// The `thread.delete` the sidebar's context menu builds, once the developer has
/// answered its confirmation.
///
/// Two fields and no third: the contract's own command is a `threadId` and a
/// `commandId`, and this server does not remember command ids.
fn delete(thread_id: &str) -> Value {
    json!({
        "type": "thread.delete",
        "commandId": format!("test:delete:{thread_id}"),
        "threadId": thread_id,
    })
}

fn archive(thread_id: &str) -> Value {
    json!({
        "type": "thread.archive",
        "commandId": format!("test:archive:{thread_id}"),
        "threadId": thread_id,
    })
}

fn settle(thread_id: &str) -> Value {
    json!({
        "type": "thread.settle",
        "commandId": format!("test:settle:{thread_id}"),
        "threadId": thread_id,
    })
}

fn rename(thread_id: &str, title: &str) -> Value {
    json!({
        "type": "thread.meta.update",
        "commandId": format!("test:rename:{thread_id}"),
        "threadId": thread_id,
        "title": title,
    })
}

/// Register a project and a conversation in it. No turn: a deletion is one field
/// on the thread, and the tests that need a transcript say so.
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
    let mut client = server.connect().await;
    let snapshot = client
        .call("orchestration.getArchivedShellSnapshot", json!({}))
        .await
        .expect_success();
    client.close().await;
    ids(&snapshot)
}

fn ids(snapshot: &Value) -> Vec<String> {
    snapshot["threads"]
        .as_array()
        .unwrap_or_else(|| panic!("a snapshot carries its conversations: {snapshot:#?}"))
        .iter()
        .map(|thread| thread["id"].as_str().unwrap_or("<no id>").to_string())
        .collect()
}

/// The conversation a client that **already holds it** is handed — the one door
/// a deletion leaves open, and the way to see that nothing was destroyed.
///
/// A cursor of zero, which is a client saying "I have this conversation, and I
/// am a long way behind": the server answers every such cursor with the whole
/// conversation, because a snapshot replaces what the client holds rather than
/// being folded into it (ADR-0016).
async fn as_a_client_that_holds_it_sees_it(server: &TestServer, thread_id: &str) -> Value {
    let mut client = server.connect().await;
    let subscription = client
        .subscribe(
            "orchestration.subscribeThread",
            json!({"threadId": thread_id, "afterSequence": 0}),
        )
        .await;
    let opening = client.next_chunk(&subscription).await;
    let snapshot = opening
        .into_iter()
        .find(|item| item["kind"] == "snapshot")
        .unwrap_or_else(|| panic!("a resume of {thread_id} opened with no conversation"));
    client.close().await;
    snapshot["snapshot"]["thread"].clone()
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

/// Deleting answers with the sequence it committed at, publishes on the
/// conversation's own feed, and tells the project list the conversation is gone.
///
/// The project list is told a *removal* rather than a summary, and that is the
/// half worth being careful about: `OrchestrationThreadShell` does not declare
/// `deletedAt`, so a client could not filter a deleted conversation out of the
/// list the way it filters an archived one on `archivedAt`. `thread-removed` is
/// the vocabulary its reducer already has (`shellReducer.ts`).
#[tokio::test]
async fn deleting_is_answered_with_a_sequence_and_leaves_both_lists() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let shell = open_shell(&mut client).await;
    let thread = client.watch_conversation("thread-1").await;

    let deleted = client
        .call("orchestration.dispatchCommand", delete("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.deleted"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(
        event["sequence"], deleted["sequence"],
        "the event and the answer name the same commit"
    );
    assert_eq!(event["payload"]["threadId"], "thread-1");
    assert!(
        event["payload"]["deletedAt"].is_string(),
        "a deletion says when: {event}"
    );
    // The contract's `ThreadDeletedPayload` is two keys, and the second one the
    // other lifecycle payloads carry is deliberately not among them: the client's
    // reducer keeps none of the thread after folding this, so there is nothing
    // left for an `updatedAt` to describe.
    assert_eq!(
        event["payload"].as_object().expect("an object").len(),
        2,
        "{event}"
    );

    let on_the_list = next_items(&mut client, &shell).await;
    assert_eq!(on_the_list.len(), 1, "{on_the_list:#?}");
    assert_eq!(on_the_list[0]["kind"], "thread-removed");
    assert_eq!(on_the_list[0]["sequence"], deleted["sequence"]);
    assert_eq!(on_the_list[0]["threadId"], "thread-1");

    client.interrupt(&thread).await;
    client.interrupt(&shell).await;
    client.close().await;

    // Both lists, and the archived one is the half that was checked against the
    // client's own reducer rather than assumed: the settings panel takes that
    // snapshot whole and groups it by project, filtering on neither field, so a
    // conversation archived and then deleted would be drawn there with an
    // unarchive control on it.
    assert_eq!(listed(&server).await, Vec::<String>::new());
    assert_eq!(archived(&server).await, Vec::<String>::new());

    server.stop().await;
}

/// An archived conversation that is then deleted leaves the archived list too.
///
/// The one behaviour ticket 10 was told to confirm against the client rather than
/// choose, and the answer is above: `SettingsPanels.tsx`'s `archivedGroups`
/// filters on neither `archivedAt` nor `deletedAt`, so what this server withholds
/// is the whole of what keeps it out of the panel.
#[tokio::test]
async fn an_archived_conversation_that_is_deleted_leaves_the_archive_as_well() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    client
        .call("orchestration.dispatchCommand", archive("thread-1"))
        .await
        .expect_success();
    assert_eq!(archived(&server).await, vec!["thread-1".to_string()]);

    client
        .call("orchestration.dispatchCommand", delete("thread-1"))
        .await
        .expect_success();

    assert_eq!(archived(&server).await, Vec::<String>::new());
    assert_eq!(listed(&server).await, Vec::<String>::new());

    client.close().await;
    server.stop().await;
}

/// Nothing was destroyed: the transcript, the work log and the checkpoint rows
/// are all where the turn left them, and so are the git refs behind them.
///
/// This is the criterion the whole "deleting is soft" decision exists for. The
/// refs are asserted from the developer's own repository rather than from the
/// conversation, because they are the thing a hard delete would have orphaned —
/// real git objects in a repository this server does not own.
#[tokio::test]
async fn what_is_left_behind() {
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

    let before = as_a_client_that_holds_it_sees_it(&server, "thread-1").await;
    let refs_before = laplus_refs(&workspace);
    assert!(
        !before["messages"].as_array().expect("messages").is_empty()
            && !before["activities"].as_array().expect("a work log").is_empty()
            && !before["checkpoints"]
                .as_array()
                .expect("checkpoints")
                .is_empty()
            && !refs_before.is_empty(),
        "the turn has to leave all four behind or this test proves nothing: {before:#?}"
    );

    client
        .call("orchestration.dispatchCommand", delete("thread-1"))
        .await
        .expect_success();

    let after = as_a_client_that_holds_it_sees_it(&server, "thread-1").await;
    assert_eq!(after["messages"], before["messages"]);
    assert_eq!(after["activities"], before["activities"]);
    assert_eq!(after["checkpoints"], before["checkpoints"]);
    assert_eq!(
        laplus_refs(&workspace),
        refs_before,
        "a deletion touched the checkpoint refs in the developer's repository"
    );
    // The one field that moved, and the moment it moved at.
    assert_eq!(after["deletedAt"], after["updatedAt"]);
    assert_eq!(after["archivedAt"], Value::Null);

    client.interrupt(&thread).await;
    client.close().await;
    server.stop().await;
}

/// Every checkpoint ref this server has written into the developer's repository.
fn laplus_refs(workspace: &Workspace) -> Vec<String> {
    workspace
        .git(&["for-each-ref", "--format=%(refname) %(objectname)", "refs/laplus"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// A deleted conversation refuses every command, so a stale window cannot go on
/// driving one the developer removed — and refuses a fresh subscription, so it
/// cannot draw one either.
///
/// A *resume* is the exception and is asserted beside them: a client that says it
/// already holds the conversation is owed the events it has not folded yet, and
/// refusing it would leave that client drawing a conversation it will never be
/// told is gone. That rule is ticket 28's and this ticket does not touch it.
#[tokio::test]
async fn a_deleted_conversation_refuses_commands_and_fresh_subscriptions() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    client
        .call("orchestration.dispatchCommand", delete("thread-1"))
        .await
        .expect_success();

    for command in [
        archive("thread-1"),
        settle("thread-1"),
        rename("thread-1", "a new name"),
        start_turn("thread-1", "message-2", "are you still there?"),
    ] {
        let message = refused(&mut client, command).await;
        assert!(
            message.contains("thread-1") && message.contains("deleted"),
            "{message}"
        );
    }

    // A fresh subscription — a second window opening the conversation from a
    // stale link — is refused with the error the method declares, which is what
    // the client treats as "not here" rather than as a broken connection.
    let request = client
        .send_request(
            "orchestration.subscribeThread",
            json!({"threadId": "thread-1", "requestCompletionMarker": true}),
        )
        .await;
    let frame = client.next_frame_for(&request).await;
    assert_eq!(frame["_tag"], "Exit", "{frame}");
    assert_eq!(
        frame["exit"]["cause"][0]["error"]["_tag"],
        "OrchestrationGetSnapshotError"
    );

    // And the HTTP door answers the same way, because the client seeds a pane
    // from it and then resumes past the event that would have told it.
    let over_http = server.get("/api/orchestration/threads/thread-1").await;
    assert_eq!(over_http.status, 404);
    assert_eq!(over_http.body["reason"], "thread_not_found");

    // The resume is not refused, and it hands back the conversation stamped.
    let held = as_a_client_that_holds_it_sees_it(&server, "thread-1").await;
    assert!(held["deletedAt"].is_string(), "{held:#?}");

    client.close().await;
    server.stop().await;
}

/// The change reaches a subscriber on a *second* connection, on both feeds.
///
/// The change feed is the server's, not the connection's: two windows on one
/// conversation must not disagree about whether it still exists.
#[tokio::test]
async fn the_deletion_reaches_a_subscriber_on_another_connection() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;

    let mut author = server.connect().await;
    a_conversation(&mut author, &workspace).await;

    let mut watcher = server.connect().await;
    let shell = open_shell(&mut watcher).await;
    let thread = watcher.watch_conversation("thread-1").await;

    let answered = author
        .call("orchestration.dispatchCommand", delete("thread-1"))
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.deleted"]);
    assert_eq!(on_the_thread[0]["event"]["sequence"], answered["sequence"]);

    let on_the_list = next_items(&mut watcher, &shell).await;
    assert_eq!(on_the_list[0]["kind"], "thread-removed");
    assert_eq!(on_the_list[0]["threadId"], "thread-1");

    watcher.interrupt(&thread).await;
    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// A deletion survives a restart: the conversation does not come back on the
/// list, and it is still refused.
///
/// A restart is a second server on the same file — nothing but the path on disk
/// carried the state across, which is what makes this a test of the column rather
/// than of the broadcast.
#[tokio::test]
async fn a_deletion_survives_a_restart() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);

    let watched = {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        a_conversation(&mut client, &workspace).await;
        let thread = client.watch_conversation("thread-1").await;

        client
            .call("orchestration.dispatchCommand", delete("thread-1"))
            .await
            .expect_success();
        let seen = next_items(&mut client, &thread).await;
        let watched = seen[0]["event"]["payload"]["deletedAt"].clone();

        client.interrupt(&thread).await;
        client.close().await;
        server.stop().await;
        watched
    };

    let server = TestServer::start_at(&database).await;

    assert_eq!(listed(&server).await, Vec::<String>::new());
    assert_eq!(archived(&server).await, Vec::<String>::new());

    let restored = as_a_client_that_holds_it_sees_it(&server, "thread-1").await;
    assert_eq!(
        restored["deletedAt"], watched,
        "the stamp a subscriber watched is not the stamp the restart read back: {restored:#?}"
    );

    let mut client = server.connect().await;
    let message = refused(&mut client, archive("thread-1")).await;
    assert!(message.contains("deleted"), "{message}");
    client.close().await;

    server.stop().await;
}

/// Every refusal, on the sentence — which is all
/// `OrchestrationDispatchCommandError` carries, so the sentence *is* the
/// diagnostic the developer is shown.
///
/// A repeat is refused rather than answered, which is where this parts company
/// with the settle and snooze commands: those are a standing answer that folding
/// twice lands on either way, and this is a conversation leaving a list, so a
/// second delete is a click on a control that is no longer there.
#[tokio::test]
async fn a_blank_or_unknown_conversation_and_a_repeat_are_refused_with_a_sentence() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;

    let message = refused(&mut client, delete("  ")).await;
    assert!(message.contains("threadId"), "{message}");

    let message = refused(&mut client, delete("never-created")).await;
    assert!(message.contains("never-created"), "{message}");

    client
        .call("orchestration.dispatchCommand", delete("thread-1"))
        .await
        .expect_success();

    let message = refused(&mut client, delete("thread-1")).await;
    assert!(
        message.contains("thread-1") && message.contains("already deleted"),
        "{message}"
    );

    // The connection is still usable: a refusal costs one call.
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}
