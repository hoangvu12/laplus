//! The project registry, driven the way the UI drives it.
//!
//! Ticket 05's last requirement is that add, list, remove and restart are all
//! exercised **through the socket boundary**, and this file is that. Nothing
//! here reaches into the server: a project is added by sending the command the
//! captured client sends, and the list is read by opening the subscription the
//! captured client opens.
//!
//! Two things the shape of these tests is trying to say:
//!
//! - **There is no "list projects" call.** The list is the shell subscription's
//!   snapshot. Reading it means subscribing, which is why every test here opens
//!   one instead of making a request.
//! - **A restart is a second server on the same file.** No second process is
//!   needed to test persistence — what "survives a restart" means is that
//!   nothing but the path on disk carried the state across, and starting a
//!   fresh server on that path is exactly that claim.

mod harness;

use std::path::Path;

use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// The captured `project.create` payload from
/// `fixtures/socket-wire/05-orchestration-and-backpressure.ndjson`, with the
/// folder swapped for one this test made. `createWorkspaceRootIfMissing` is
/// sent as `true` because the real UI always sends it as `true`.
fn create_project(id: &str, folder: &Path) -> Value {
    json!({
        "type": "project.create",
        "commandId": format!("test:create:{id}"),
        "projectId": id,
        "title": "",
        "workspaceRoot": folder.to_string_lossy(),
        "createWorkspaceRootIfMissing": true,
        "defaultModelSelection": Value::Null,
        "createdAt": "2026-07-26T00:23:04.909Z",
    })
}

fn delete_project(id: &str) -> Value {
    json!({
        "type": "project.delete",
        "commandId": format!("test:delete:{id}"),
        "projectId": id,
    })
}

/// Open the shell subscription and take its opening chunk.
///
/// Returns the subscription's request id and the items it opened with — the
/// snapshot, and the completion marker when one was asked for. Both arrive in
/// one chunk because both come from the same description of the world.
async fn open_shell(client: &mut SocketClient, marker: bool) -> (String, Vec<Value>) {
    let id = client
        .subscribe(
            "orchestration.subscribeShell",
            json!({ "requestCompletionMarker": marker }),
        )
        .await;
    let opening = next_items(client, &id).await;
    (id, opening)
}

/// The next chunk for a subscription, acknowledged so the next one can follow.
async fn next_items(client: &mut SocketClient, id: &str) -> Vec<Value> {
    let values = client.next_chunk(id).await;
    client.ack(id).await;
    values
}

/// The projects in a `snapshot` item.
fn projects_in(snapshot: &Value) -> &Vec<Value> {
    assert_eq!(snapshot["kind"], "snapshot", "not a snapshot: {snapshot}");
    snapshot["snapshot"]["projects"]
        .as_array()
        .unwrap_or_else(|| panic!("a snapshot's projects are an array: {snapshot}"))
}

/// The project list as a fresh subscriber would see it. This is what "the
/// project list" means to the UI, so it is what these tests assert on.
async fn listed(server: &TestServer) -> Vec<Value> {
    let mut client = server.connect().await;
    let (id, opening) = open_shell(&mut client, false).await;
    let projects = projects_in(&opening[0]).clone();
    client.interrupt(&id).await;
    client.close().await;
    projects
}

/// The message from a refused command. The contract gives
/// `OrchestrationDispatchCommandError` a message and nothing else
/// machine-readable, so this string is the whole diagnostic the UI can show —
/// which is why every test that refuses something asserts on it.
fn refusal(outcome: Outcome) -> String {
    outcome.expect_declared("OrchestrationDispatchCommandError")["message"]
        .as_str()
        .expect("a message")
        .to_string()
}

/// The ticket's first line: a folder is added and appears in the project list.
///
/// Both halves of "appears" are checked, because the UI depends on each
/// separately — the open subscription is how the window that added it updates,
/// and the fresh snapshot is how a window opened afterwards learns about it.
#[tokio::test]
async fn a_folder_added_over_the_socket_appears_in_the_project_list() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let folder = directory.path().join("my-project");
    std::fs::create_dir(&folder).expect("creates the folder");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let (shell, opening) = open_shell(&mut client, true).await;
    assert!(
        projects_in(&opening[0]).is_empty(),
        "a new registry starts empty: {opening:#?}"
    );
    assert_eq!(
        opening[1],
        json!({"kind": "synchronized"}),
        "the completion marker was requested: {opening:#?}"
    );

    let created = client
        .call("orchestration.dispatchCommand", create_project("p1", &folder))
        .await
        .expect_success();
    assert_eq!(created["sequence"], json!(1));

    let event = next_items(&mut client, &shell).await;
    assert_eq!(event.len(), 1, "{event:#?}");
    assert_eq!(event[0]["kind"], "project-upserted");
    assert_eq!(
        event[0]["sequence"], created["sequence"],
        "the event and the answer name the same commit"
    );
    assert_eq!(event[0]["project"]["id"], "p1");
    assert_eq!(event[0]["project"]["workspaceRoot"], json!(folder.to_string_lossy()));
    assert_eq!(event[0]["project"]["title"], "my-project");

    client.interrupt(&shell).await;
    client.close().await;

    let projects = listed(&server).await;
    assert_eq!(projects.len(), 1, "{projects:#?}");
    assert_eq!(projects[0]["id"], "p1");
    assert_eq!(projects[0]["workspaceRoot"], json!(folder.to_string_lossy()));

    server.stop().await;
}

/// A second window has to see the first one's work. The change feed is the
/// server's, not the connection's, and this is the test that says so.
#[tokio::test]
async fn a_project_added_on_one_connection_reaches_a_subscriber_on_another() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let folder = directory.path().join("shared");
    std::fs::create_dir(&folder).expect("creates the folder");

    let server = TestServer::start().await;
    let mut watcher = server.connect().await;
    let (shell, _) = open_shell(&mut watcher, false).await;

    let mut author = server.connect().await;
    let created = author
        .call("orchestration.dispatchCommand", create_project("p1", &folder))
        .await
        .expect_success();

    let event = next_items(&mut watcher, &shell).await;
    assert_eq!(event[0]["kind"], "project-upserted");
    assert_eq!(event[0]["sequence"], created["sequence"]);
    assert_eq!(event[0]["project"]["id"], "p1");

    watcher.interrupt(&shell).await;
    watcher.close().await;
    author.close().await;
    server.stop().await;
}

/// The ticket's second line. A restart is a second server on the same file,
/// and the sequence has to survive it too — a client caches a snapshot and
/// ignores events at or below the sequence it holds, so a counter that
/// restarted at zero would make the next few changes invisible.
#[tokio::test]
async fn the_project_list_survives_a_server_restart() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("state.sqlite");
    let folder = directory.path().join("kept");
    std::fs::create_dir(&folder).expect("creates the folder");

    let sequence = {
        let server = TestServer::start_at(&database).await;
        let mut client = server.connect().await;
        let created = client
            .call("orchestration.dispatchCommand", create_project("p1", &folder))
            .await
            .expect_success();
        client.close().await;
        server.stop().await;
        created["sequence"].clone()
    };

    let server = TestServer::start_at(&database).await;
    let mut client = server.connect().await;
    let (shell, opening) = open_shell(&mut client, false).await;

    let projects = projects_in(&opening[0]);
    assert_eq!(projects.len(), 1, "the registry was forgotten: {opening:#?}");
    assert_eq!(projects[0]["id"], "p1");
    assert_eq!(projects[0]["workspaceRoot"], json!(folder.to_string_lossy()));
    assert_eq!(projects[0]["createdAt"], "2026-07-26T00:23:04.909Z");
    assert_eq!(
        opening[0]["snapshot"]["snapshotSequence"], sequence,
        "the sequence restarted, so the client would ignore the next changes"
    );

    client.interrupt(&shell).await;
    client.close().await;
    server.stop().await;
}

/// The ticket's third line. The registry lets go of the project; the disk keeps
/// the folder and everything in it.
#[tokio::test]
async fn removing_a_project_takes_it_off_the_list_and_leaves_the_folder_on_disk() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let folder = directory.path().join("keep-me");
    std::fs::create_dir(&folder).expect("creates the folder");
    let work = folder.join("work.txt");
    std::fs::write(&work, "important").expect("writes the file");

    let server = TestServer::start().await;
    let mut client = server.connect().await;
    let (shell, _) = open_shell(&mut client, false).await;

    client
        .call("orchestration.dispatchCommand", create_project("p1", &folder))
        .await
        .expect_success();
    next_items(&mut client, &shell).await;

    let removed = client
        .call("orchestration.dispatchCommand", delete_project("p1"))
        .await
        .expect_success();

    let event = next_items(&mut client, &shell).await;
    assert_eq!(
        event[0],
        json!({
            "kind": "project-removed",
            "sequence": removed["sequence"],
            "projectId": "p1",
        })
    );

    client.interrupt(&shell).await;
    client.close().await;

    assert!(listed(&server).await.is_empty());
    assert!(folder.is_dir(), "the folder was removed from disk");
    assert_eq!(
        std::fs::read_to_string(&work).expect("the file survives"),
        "important"
    );

    server.stop().await;
}

/// The ticket's fourth line, at the seam the user meets it. Each refusal
/// arrives as the method's own declared error, so the UI renders the sentence
/// rather than treating the connection as broken.
#[tokio::test]
async fn a_folder_that_will_not_serve_is_refused_with_a_message_naming_the_problem() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let missing = directory.path().join("not-there");
    let file = directory.path().join("a-file.txt");
    std::fs::write(&file, "not a folder").expect("writes the file");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let message = refusal(
        client
            .call("orchestration.dispatchCommand", create_project("p1", &missing))
            .await,
    );
    assert!(
        message.contains("does not exist") && message.contains(&*missing.to_string_lossy()),
        "{message}"
    );
    assert!(
        !missing.exists(),
        "the server created the folder it was asked to refuse"
    );

    let message = refusal(
        client
            .call("orchestration.dispatchCommand", create_project("p2", &file))
            .await,
    );
    assert!(
        message.contains("is not a directory") && message.contains(&*file.to_string_lossy()),
        "{message}"
    );

    // And the connection is otherwise untouched: a refusal costs one call.
    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));
    assert!(listed(&server).await.is_empty());

    client.close().await;
    server.stop().await;
}

/// The ticket's fifth line. The refusal names the project already holding the
/// folder, because "already added" without saying *as what* is not something a
/// user can act on.
#[tokio::test]
async fn adding_the_same_folder_twice_does_not_create_a_duplicate() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let folder = directory.path().join("only-once");
    std::fs::create_dir(&folder).expect("creates the folder");

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .call("orchestration.dispatchCommand", create_project("p1", &folder))
        .await
        .expect_success();

    let message = refusal(
        client
            .call("orchestration.dispatchCommand", create_project("p2", &folder))
            .await,
    );
    assert!(
        message.contains("already exists") && message.contains(&*folder.to_string_lossy()),
        "{message}"
    );

    let projects = listed(&server).await;
    assert_eq!(projects.len(), 1, "{projects:#?}");
    assert_eq!(projects[0]["id"], "p1");

    client.close().await;
    server.stop().await;
}

/// The ticket's sixth line. Nothing on the machine is prepared in advance: the
/// path names a file that is not there, inside a directory that is not there
/// either, and the server is expected to arrive at a working registry from
/// that.
#[tokio::test]
async fn the_database_is_created_on_first_run() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let database = directory.path().join("laplus").join("state.sqlite");
    assert!(!database.exists());

    let server = TestServer::start_at(&database).await;
    assert!(
        database.exists(),
        "the database was not created at {}",
        database.display()
    );

    let folder = directory.path().join("first");
    std::fs::create_dir(&folder).expect("creates the folder");

    let mut client = server.connect().await;
    let created = client
        .call("orchestration.dispatchCommand", create_project("p1", &folder))
        .await
        .expect_success();
    assert_eq!(created["sequence"], json!(1));

    client.close().await;
    assert_eq!(listed(&server).await.len(), 1);

    server.stop().await;
}

/// Roughly twenty command types exist and laplus implements nine. An
/// unimplemented one has to fail its own call and name itself — not take the
/// connection down, and not be mistaken for the method being missing.
#[tokio::test]
async fn an_unimplemented_command_fails_only_its_own_call() {
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let message = refusal(
        client
            .call(
                "orchestration.dispatchCommand",
                json!({"type": "thread.create", "commandId": "c", "threadId": "t"}),
            )
            .await,
    );
    assert!(message.contains("thread.create"), "{message}");

    assert!(matches!(
        client.call("server.getConfig", json!({})).await,
        Outcome::Success(_)
    ));

    client.close().await;
    server.stop().await;
}

/// A subscription is released when the client unsubscribes and when the client
/// simply vanishes. The shell is the longest-lived subscription the UI opens,
/// so a leak here would be the one that mattered.
#[tokio::test]
async fn shell_subscriptions_are_released_on_unsubscribe_and_on_disconnect() {
    let server = TestServer::start().await;

    let mut client = server.connect().await;
    let (shell, _) = open_shell(&mut client, true).await;
    assert_eq!(server.live_subscriptions(), 1);

    client.interrupt(&shell).await;
    let ended = client.next_frame_for(&shell).await;
    assert_eq!(ended["_tag"], "Exit");
    assert_eq!(ended["exit"]["cause"][0]["_tag"], "Interrupt");
    server.await_live_subscriptions(0).await;

    let mut client = server.connect().await;
    open_shell(&mut client, false).await;
    assert_eq!(server.live_subscriptions(), 1);
    client.abandon();
    server.await_live_subscriptions(0).await;

    server.stop().await;
}
