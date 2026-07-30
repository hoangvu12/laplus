//! The six lifecycle fields, on the wire.
//!
//! Ticket 01 of the thread-lifecycle effort. Nothing a developer can do changes
//! here — there is no command yet — so what these tests are about is the one
//! thing that did: the thread read model can now *express* an archived, settled,
//! snoozed or deleted conversation, and both renderings of a thread carry the
//! six fields as stored state rather than as hardcoded `null`.
//!
//! That makes the assertions unusual for this directory. Every other socket
//! suite drives a command; this one writes to the store the way a later ticket's
//! command will and then asks the two subscriptions what they see. The claim is
//! precisely "a value that is in the database reaches a client", which is the
//! precondition every one of archive, settle, snooze and delete is waiting on.
//!
//! The store is written through `crate::store::Database` rather than by raw SQL
//! on purpose: that is the seam the commands will use, so a schema change that
//! broke them breaks this too.

mod harness;

use harness::conversation::{create_project, create_thread};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use laplus_server::store::Database;
use laplus_server::threads::Lifecycle;
use laplus_server::transcripts::Write;
use serde_json::{json, Value};
use std::path::Path;

/// The six keys, as the contract spells them.
const LIFECYCLE_KEYS: [&str; 6] = [
    "archivedAt",
    "settledOverride",
    "settledAt",
    "snoozedUntil",
    "snoozedAt",
    "deletedAt",
];

/// A conversation whose every lifecycle field is set, and to a *different*
/// value: a field wired to the wrong column then fails these tests rather than
/// passing by coincidence.
fn a_curated_lifecycle() -> Lifecycle {
    Lifecycle {
        archived_at: Some("2026-07-26T01:00:00.000Z".to_string()),
        settled_override: Some("settled"),
        settled_at: Some("2026-07-26T02:00:00.000Z".to_string()),
        snoozed_until: Some("2026-07-27T03:00:00.000Z".to_string()),
        snoozed_at: Some("2026-07-26T04:00:00.000Z".to_string()),
        deleted_at: Some("2026-07-26T05:00:00.000Z".to_string()),
    }
}

/// What [`a_curated_lifecycle`] looks like once it has crossed the wire.
fn on_the_wire() -> Value {
    json!({
        "archivedAt": "2026-07-26T01:00:00.000Z",
        "settledOverride": "settled",
        "settledAt": "2026-07-26T02:00:00.000Z",
        "snoozedUntil": "2026-07-27T03:00:00.000Z",
        "snoozedAt": "2026-07-26T04:00:00.000Z",
        "deletedAt": "2026-07-26T05:00:00.000Z",
    })
}

/// The six keys of a rendering, on their own, so a failure prints the six rather
/// than a whole conversation.
fn lifecycle_of(thread: &Value) -> Value {
    let mut fields = serde_json::Map::new();
    for key in LIFECYCLE_KEYS {
        fields.insert(
            key.to_string(),
            thread
                .get(key)
                .unwrap_or_else(|| panic!("{key} is missing from {thread:#?}"))
                .clone(),
        );
    }
    Value::Object(fields)
}

/// Register a project and a conversation in it. No turn: the lifecycle is a
/// property of the conversation and none of it involves an agent.
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

/// The conversation as a subscriber that arrives afterwards is handed it.
async fn as_a_fresh_subscriber_sees_it(server: &TestServer, thread_id: &str) -> Value {
    server
        .connect()
        .await
        .into_thread_snapshot(thread_id)
        .await["thread"]
        .clone()
}

/// The conversation's summary on the project list.
async fn on_the_project_list(server: &TestServer, thread_id: &str) -> Value {
    summary_in(
        &server.connect().await.into_shell_snapshot().await,
        thread_id,
    )
}

/// The conversation's summary on the *other* list.
///
/// Ticket 06 took archived conversations off the project list, so the curated
/// lifecycle below — which sets all six fields, `archivedAt` among them — is
/// found here instead. The shape is the same shape: both snapshots are one
/// builder filtered two ways, which is what `socket_archiving.rs` asserts
/// directly.
async fn on_the_archived_list(server: &TestServer, thread_id: &str) -> Value {
    let mut client = server.connect().await;
    let snapshot = client
        .call("orchestration.getArchivedShellSnapshot", json!({}))
        .await
        .expect_success();
    client.close().await;
    summary_in(&snapshot, thread_id)
}

fn summary_in(snapshot: &Value, thread_id: &str) -> Value {
    snapshot["threads"]
        .as_array()
        .expect("the list carries its conversations")
        .iter()
        .find(|thread| thread["id"] == thread_id)
        .unwrap_or_else(|| panic!("{thread_id} is not on the list: {snapshot:#?}"))
        .clone()
}

/// Curate a conversation's lifecycle the way a later ticket's command will:
/// read the row, move the six fields, write it back.
///
/// Called with the server stopped, so nothing is racing this for the file.
fn curate(database: &Path, thread_id: &str, lifecycle: Lifecycle) {
    let store = Database::open(database).expect("opens the database the server just closed");
    let mut row = store
        .conversations()
        .expect("reads the conversations")
        .into_iter()
        .find(|conversation| conversation.thread.id == thread_id)
        .unwrap_or_else(|| panic!("{thread_id} is not in the database"))
        .thread;
    row.lifecycle = lifecycle;
    store
        .transcribe(&[Write::Thread(Box::new(row))])
        .expect("stores the curated conversation");
}

/// The ticket's own line: a fresh subscriber on the project list and on a
/// thread's own feed sees the six fields.
///
/// Present and `null` on a conversation nothing has been done to, which is the
/// half that matters for a client — the contract declares four of the six as
/// required, so a missing key fails the decode of the whole snapshot and the
/// developer is shown no conversation at all rather than a slightly wrong one.
#[tokio::test]
async fn both_feeds_carry_the_six_fields_on_a_conversation_nothing_has_been_done_to() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    client.close().await;

    let fresh = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    let summary = on_the_project_list(&server, "thread-1").await;
    for key in LIFECYCLE_KEYS {
        assert_eq!(
            fresh.get(key),
            Some(&Value::Null),
            "the thread's own feed is missing or wrong about {key}: {fresh:#?}"
        );
        assert_eq!(
            summary.get(key),
            Some(&Value::Null),
            "the project list is missing or wrong about {key}: {summary:#?}"
        );
    }

    server.stop().await;
}

/// The whole of what this ticket buys: a lifecycle in the database is a
/// lifecycle on the wire, on both feeds, after a restart.
///
/// A restart is a second server on the same file, so nothing but the six columns
/// carried the state across — which is the claim, and the reason the fields had
/// to stop being literals. Both renderings are asserted as a whole rather than
/// key by key, because they are built from one shape and the failure worth
/// catching is one of them drifting from the other.
#[tokio::test]
async fn a_curated_lifecycle_survives_a_restart_and_reaches_both_feeds() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);

    {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        a_conversation(&mut client, &workspace).await;
        client.close().await;
        server.stop().await;
    }

    curate(&database, "thread-1", a_curated_lifecycle());

    let server = TestServer::start_at(&database).await;
    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        lifecycle_of(&restored),
        on_the_wire(),
        "the thread's own feed lost the lifecycle across the restart"
    );

    let summary = on_the_archived_list(&server, "thread-1").await;
    assert_eq!(
        lifecycle_of(&summary),
        on_the_wire(),
        "the shell summary lost the lifecycle across the restart"
    );

    server.stop().await;
}

/// The lifecycle is not spent by the conversation carrying on.
///
/// Every change to a thread rewrites the whole row — that is what
/// `crate::transcripts` does — so the fields could be dropped by any later
/// write rather than by the restart. Renaming is the cheapest change that
/// rewrites the row without touching a lifecycle field, so it is the one that
/// asks the question.
#[tokio::test]
async fn a_later_change_to_the_conversation_does_not_clear_the_lifecycle() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let workspace = Workspace::with(&["src/"]);

    {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        a_conversation(&mut client, &workspace).await;
        client.close().await;
        server.stop().await;
    }

    curate(&database, "thread-1", a_curated_lifecycle());

    let server = TestServer::start_at(&database).await;
    let mut client = server.connect().await;
    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.meta.update",
                "commandId": "test:rename:thread-1",
                "threadId": "thread-1",
                "title": "A better name",
            }),
        )
        .await
        .expect_success();
    client.close().await;
    server.stop().await;

    let server = TestServer::start_at(&database).await;
    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(restored["title"], "A better name", "the rename was lost");
    assert_eq!(
        lifecycle_of(&restored),
        on_the_wire(),
        "renaming the conversation cleared its lifecycle"
    );

    server.stop().await;
}
