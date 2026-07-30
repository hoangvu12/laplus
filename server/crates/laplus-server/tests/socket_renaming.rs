//! The two rename controls, driven the way the UI drives them.
//!
//! Ticket 03 of the thread-lifecycle effort, at the seam its spec calls primary:
//! a real socket, the commands `client-runtime/src/operations/commands.ts`
//! builds, and the two subscriptions the real client folds. Nothing here reaches
//! into the server.
//!
//! Both commands were refused by name before this, so a conversation was stuck
//! with the title it was seeded with and a project was stuck with its folder's
//! name. **`thread.meta.update` was never only the rename control**, which is
//! what made a refusal expensive: the composer sends it on every send whose model
//! or branch differs from the thread's, from `ChatView.tsx`'s
//! `persistThreadSettingsForNextTurn`, and it sends it *first* — with the two mode
//! commands and the turn itself behind an `if (failure === null)`. So the refusal
//! swallowed the message as well as the rename. That is
//! [`the_payload_the_composer_sends_before_a_message_is_answered`].
//!
//! Each command is asserted the three ways the spec asks for: the sequence it
//! answers with, the events that reach a subscriber on the thread and on the
//! project list — including one on a second connection — and what a subscriber
//! that arrives *afterwards* is handed, which is the half that proves the title
//! was kept rather than merely broadcast.

mod harness;

use harness::conversation::{create_project, create_thread, kinds};
use harness::workspace::Workspace;
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// The `thread.meta.update` the sidebar's rename builds: a title and nothing
/// else.
fn rename_thread(thread_id: &str, title: &str) -> Value {
    thread_meta_update(thread_id, json!({"title": title}))
}

/// The same command carrying whichever fields the caller means to move, because
/// *which fields are present* is what this command turns on.
fn thread_meta_update(thread_id: &str, fields: Value) -> Value {
    let mut command = json!({
        "type": "thread.meta.update",
        "commandId": format!("test:meta:{thread_id}"),
        "threadId": thread_id,
    });
    let envelope = command.as_object_mut().expect("the envelope is an object");
    for (field, value) in fields.as_object().expect("the fields are an object") {
        envelope.insert(field.clone(), value.clone());
    }
    command
}

/// The `project.meta.update` the sidebar's rename dialog builds.
fn rename_project(project_id: &str, title: &str) -> Value {
    json!({
        "type": "project.meta.update",
        "commandId": format!("test:project-meta:{project_id}"),
        "projectId": project_id,
        "title": title,
    })
}

/// Register a project and a conversation in it. No turn: a title is a field on
/// each, and nothing here needs an agent.
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
async fn as_a_fresh_subscriber_sees_it(server: &TestServer, thread_id: &str) -> Value {
    server
        .connect()
        .await
        .into_thread_snapshot(thread_id)
        .await["thread"]
        .clone()
}

/// The conversation's summary on the project list, which is what the sidebar
/// renders a title from.
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

/// The project as a fresh subscriber sees it, which is the sidebar's own view.
async fn project_on_the_list(server: &TestServer, project_id: &str) -> Value {
    let snapshot = server.connect().await.into_shell_snapshot().await;
    snapshot["projects"]
        .as_array()
        .expect("the list carries its projects")
        .iter()
        .find(|project| project["id"] == project_id)
        .unwrap_or_else(|| panic!("{project_id} is not on the list: {snapshot:#?}"))
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

/// A renamed thread answers with the sequence it committed at and publishes on
/// its own feed *and* on the project list at that number.
///
/// Both feeds, because the UI reads them in different places and neither can be
/// derived from the other: the thread view folds the event and the sidebar folds
/// the summary. A rename only one of them heard about would leave the developer
/// looking at two names for one conversation.
#[tokio::test]
async fn renaming_a_thread_is_answered_with_a_sequence_and_published_on_both_feeds() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let shell = open_shell(&mut client).await;
    let thread = client.watch_conversation("thread-1").await;

    let renamed = client
        .call(
            "orchestration.dispatchCommand",
            rename_thread("thread-1", "Renaming a thread and a project"),
        )
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.meta-updated"]);
    let event = &on_the_thread[0]["event"];
    assert_eq!(
        event["sequence"], renamed["sequence"],
        "the event and the answer name the same commit"
    );
    assert_eq!(event["payload"]["threadId"], "thread-1");
    assert_eq!(
        event["payload"]["title"],
        "Renaming a thread and a project"
    );
    assert!(
        event["payload"]["updatedAt"].is_string(),
        "the reducer reads the thread's new updatedAt out of the payload: {event}"
    );
    // Only what was asked for. Asserted as *absent keys* rather than as nulls,
    // which is the distinction that matters: the reducer applies every field it
    // is not `undefined` for, so a `branch: null` this command never sent would
    // be folded as "clear the branch" — and a null and a missing key read the
    // same through `Value` indexing.
    for unasked in ["modelSelection", "branch", "worktreePath"] {
        assert_eq!(
            event["payload"].get(unasked),
            None,
            "a title-only rename named {unasked}: {event}"
        );
    }

    let listed = next_items(&mut client, &shell).await;
    assert_eq!(listed.len(), 1, "{listed:#?}");
    assert_eq!(listed[0]["kind"], "thread-upserted");
    assert_eq!(listed[0]["sequence"], renamed["sequence"]);
    assert_eq!(
        listed[0]["thread"]["title"],
        "Renaming a thread and a project"
    );

    client.interrupt(&thread).await;
    client.interrupt(&shell).await;
    client.close().await;

    let fresh = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(fresh["title"], "Renaming a thread and a project");
    let summary = on_the_project_list(&server, "thread-1").await;
    assert_eq!(summary["title"], "Renaming a thread and a project");

    server.stop().await;
}

/// A renamed project answers with a sequence and publishes on the project list.
///
/// The event is `project-upserted` — the same one a creation publishes, carrying
/// the whole project. The client's shell reducer upserts by id, so one shape
/// serves both and the list needs no separate "renamed" to learn.
#[tokio::test]
async fn renaming_a_project_is_answered_with_a_sequence_and_published_on_the_project_list() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let shell = open_shell(&mut client).await;

    let before = project_on_the_list(&server, "project-1").await;

    let renamed = client
        .call(
            "orchestration.dispatchCommand",
            rename_project("project-1", "The developer's own word for it"),
        )
        .await
        .expect_success();

    let listed = next_items(&mut client, &shell).await;
    assert_eq!(listed.len(), 1, "{listed:#?}");
    assert_eq!(listed[0]["kind"], "project-upserted");
    assert_eq!(
        listed[0]["sequence"], renamed["sequence"],
        "the event and the answer name the same commit"
    );
    let project = &listed[0]["project"];
    assert_eq!(project["title"], "The developer's own word for it");
    // A rename is not a move: the folder the agent runs in is untouched, and so
    // is the date the project was added, which is what the list is ordered by.
    assert_eq!(project["workspaceRoot"], before["workspaceRoot"]);
    assert_eq!(project["createdAt"], before["createdAt"]);

    client.interrupt(&shell).await;
    client.close().await;

    let fresh = project_on_the_list(&server, "project-1").await;
    assert_eq!(fresh["title"], "The developer's own word for it");

    server.stop().await;
}

/// A second window has to see the first one's work, for both renames. The change
/// feed is the server's, not the connection's.
#[tokio::test]
async fn both_renames_reach_a_subscriber_on_another_connection() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;

    let mut author = server.connect().await;
    a_conversation(&mut author, &workspace).await;

    let mut watcher = server.connect().await;
    let shell = open_shell(&mut watcher).await;
    let thread = watcher.watch_conversation("thread-1").await;

    let renamed = author
        .call(
            "orchestration.dispatchCommand",
            rename_thread("thread-1", "Seen from the other window"),
        )
        .await
        .expect_success();

    let on_the_thread = next_items(&mut watcher, &thread).await;
    assert_eq!(
        on_the_thread[0]["event"]["type"], "thread.meta-updated",
        "{on_the_thread:#?}"
    );
    assert_eq!(on_the_thread[0]["event"]["sequence"], renamed["sequence"]);
    assert_eq!(
        on_the_thread[0]["event"]["payload"]["title"],
        "Seen from the other window"
    );

    let listed = next_items(&mut watcher, &shell).await;
    assert_eq!(listed[0]["kind"], "thread-upserted");
    assert_eq!(listed[0]["thread"]["title"], "Seen from the other window");

    let renamed = author
        .call(
            "orchestration.dispatchCommand",
            rename_project("project-1", "Also seen"),
        )
        .await
        .expect_success();

    let listed = next_items(&mut watcher, &shell).await;
    assert_eq!(listed[0]["kind"], "project-upserted");
    assert_eq!(listed[0]["sequence"], renamed["sequence"]);
    assert_eq!(listed[0]["project"]["title"], "Also seen");

    watcher.interrupt(&thread).await;
    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// Both titles survive a restart, which is the ticket's reason for existing.
///
/// A restart is a second server on the same file — nothing but the path on disk
/// carried the titles across. And it is read back as a *fresh* subscriber, which
/// is what proves the rename was stored rather than only announced.
#[tokio::test]
async fn both_titles_survive_a_restart() {
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
                rename_thread("thread-1", "Still here tomorrow"),
            )
            .await
            .expect_success();
        client
            .call(
                "orchestration.dispatchCommand",
                rename_project("project-1", "And so is this"),
            )
            .await
            .expect_success();
        client.close().await;
        server.stop().await;
    }

    let server = TestServer::start_at(&database).await;

    let restored = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(
        restored["title"], "Still here tomorrow",
        "the thread's title was forgotten: {restored:#?}"
    );
    assert_eq!(
        on_the_project_list(&server, "thread-1").await["title"],
        "Still here tomorrow"
    );
    assert_eq!(
        project_on_the_list(&server, "project-1").await["title"],
        "And so is this"
    );

    // Nothing is asserted about the sequence: a run resumes its numbering from
    // its last durable registry write, so numbers are reissued by the run after
    // it. That is ADR-0016, not something these commands changed.

    server.stop().await;
}

/// **The payload the composer sends before every message.**
///
/// `persistThreadSettingsForNextTurn` sends `thread.meta.update` first, carrying
/// the model and the branch and no title at all, and only sends the mode commands
/// and the message if it succeeded. While this command was refused by name, that
/// refusal stopped the message being sent — driving ticket 02 in a real window is
/// what found it. So the payload with *no title in it* is the ordinary case, and
/// it has to be answered, land, and leave the title alone.
#[tokio::test]
async fn the_payload_the_composer_sends_before_a_message_is_answered() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let thread = client.watch_conversation("thread-1").await;

    let answered = client
        .call(
            "orchestration.dispatchCommand",
            thread_meta_update(
                "thread-1",
                json!({
                    "modelSelection": {"instanceId": "claudeAgent", "model": "claude-sonnet-5"},
                    "branch": "feature/renaming",
                    "worktreePath": Value::Null,
                }),
            ),
        )
        .await
        .expect_success();

    let on_the_thread = next_items(&mut client, &thread).await;
    assert_eq!(kinds(&on_the_thread), vec!["thread.meta-updated"]);
    assert_eq!(
        on_the_thread[0]["event"]["sequence"], answered["sequence"],
        "the event and the answer name the same commit"
    );
    let payload = &on_the_thread[0]["event"]["payload"];
    assert_eq!(payload["modelSelection"]["model"], "claude-sonnet-5");
    assert_eq!(payload["branch"], "feature/renaming");
    // Present *and* null, which the reducer reads as "clear it" — and which is
    // the whole reason an absent field and a null one cannot be the same thing
    // here.
    assert_eq!(
        payload.get("worktreePath"),
        Some(&Value::Null),
        "a cleared field has to be on the wire to clear anything: {payload}"
    );
    assert_eq!(
        payload.get("title"),
        None,
        "a payload with no title in it named one: {payload}"
    );

    // And the mode command the composer sends next, which was unreachable behind
    // the refusal, now lands as well.
    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.runtime-mode.set",
                "commandId": "test:runtime-mode:thread-1",
                "threadId": "thread-1",
                "runtimeMode": "approval-required",
                "createdAt": "2026-07-26T00:23:04.909Z",
            }),
        )
        .await
        .expect_success();
    next_items(&mut client, &thread).await;

    client.interrupt(&thread).await;
    client.close().await;

    let fresh = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(fresh["modelSelection"]["model"], "claude-sonnet-5");
    assert_eq!(fresh["branch"], "feature/renaming");
    assert_eq!(fresh["worktreePath"], Value::Null);
    assert_eq!(fresh["runtimeMode"], "approval-required");
    assert_eq!(
        fresh["title"], "A conversation",
        "the conversation was renamed by a command that sent no title: {fresh:#?}"
    );

    server.stop().await;
}

/// A blank title and an unknown subject are refused with a sentence naming the
/// problem and the thing it applies to — and the refusal costs one call, not the
/// connection.
///
/// Asserted on the sentence because that is all `OrchestrationDispatchCommandError`
/// carries, so the sentence *is* the diagnostic the developer is shown.
#[tokio::test]
async fn a_blank_title_or_an_unknown_subject_is_refused_with_a_sentence() {
    let workspace = Workspace::with(&["src/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    a_conversation(&mut client, &workspace).await;
    let named = project_on_the_list(&server, "project-1").await["title"].clone();

    let message = refusal(
        client
            .call("orchestration.dispatchCommand", rename_thread("thread-1", "  "))
            .await,
    );
    assert!(
        message.contains("title") && message.contains("thread-1"),
        "{message}"
    );

    let message = refusal(
        client
            .call(
                "orchestration.dispatchCommand",
                rename_project("project-1", ""),
            )
            .await,
    );
    assert!(
        message.contains("title") && message.contains("project-1"),
        "{message}"
    );

    let message = refusal(
        client
            .call(
                "orchestration.dispatchCommand",
                rename_thread("never-created", "A name"),
            )
            .await,
    );
    assert!(message.contains("never-created"), "{message}");

    let message = refusal(
        client
            .call(
                "orchestration.dispatchCommand",
                rename_project("never-registered", "A name"),
            )
            .await,
    );
    assert!(message.contains("never-registered"), "{message}");

    // Nothing moved, and the connection is still usable: a refusal costs one
    // call.
    let held = as_a_fresh_subscriber_sees_it(&server, "thread-1").await;
    assert_eq!(held["title"], "A conversation");
    assert_eq!(project_on_the_list(&server, "project-1").await["title"], named);
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}
