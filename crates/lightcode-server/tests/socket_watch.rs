//! The file tree staying true while the agent works, driven through the socket.
//!
//! Ticket 08's last requirement is that a change on disk produces the expected
//! sequence **through the socket boundary**, and this file is that. Nothing here
//! reaches into [`lightcode_server::filesystem`]: a workspace is opened by
//! sending the request `FileBrowserPanel` sends, and what the server now knows
//! about it is read by sending the request the composer's `@` mention sends.
//!
//! ## What "the expected sequence" is on this wire, and what it is not
//!
//! It is worth being exact, because the ticket's wording implies a mechanism
//! that does not exist here. There is **no file-tree subscription in the
//! contract**: `WS_METHODS` in `t3code/packages/contracts/src/rpc.ts` declares
//! eight `subscribe*` methods and not one of them is about files, so there is no
//! frame a server can send that would make a mounted tree redraw itself. See the
//! ticket's comments for the whole of that reading and what follows from it.
//!
//! What a change on disk therefore produces is a change in **what the next call
//! answers**: a file the agent created is offered by `projects.searchEntries`
//! without anything having told the server to go and look, where before this
//! ticket it would have been invisible until the project was listed again. That
//! is the sequence these tests assert, and it is the one a user meets — the
//! `@` mention is the picker they use while the agent is running.
//!
//! ## Why the waiting is a poll rather than a sleep
//!
//! An operating system reports a change when it reports it. A sleep would be a
//! guess that is either too short on a loaded machine or too long on every run;
//! a bounded poll fails with a sentence when nothing ever arrives, which is the
//! failure these tests are most likely to catch.

mod harness;

use std::path::Path;
use std::time::{Duration, Instant};

use harness::workspace::{paths, Workspace};
use harness::{Outcome, SocketClient, TestServer};
use serde_json::{json, Value};

/// How long a test will wait for a change on disk to reach an answer.
///
/// Not a claim about latency — on Windows a change is usually reported in tens
/// of milliseconds. It is the bound that turns "never" into a failure instead of
/// a hung suite.
const PATIENCE: Duration = Duration::from_secs(10);

async fn list_entries(client: &mut SocketClient, cwd: &Path) -> Value {
    client
        .call(
            "projects.listEntries",
            json!({"cwd": cwd.to_string_lossy()}),
        )
        .await
        .expect_success()
}

/// The composer's `@` mention, which is the call that reads what the server
/// currently believes rather than going to the disk.
async fn search_entries(client: &mut SocketClient, cwd: &Path, query: &str) -> Value {
    client
        .call(
            "projects.searchEntries",
            json!({"cwd": cwd.to_string_lossy(), "query": query, "limit": 80}),
        )
        .await
        .expect_success()
}

/// Search until the answer satisfies `wanted`, or fail saying what it was.
async fn search_until(
    client: &mut SocketClient,
    cwd: &Path,
    query: &str,
    wanted: impl Fn(&[&str]) -> bool,
) -> Value {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let answer = search_entries(client, cwd, query).await;
        if wanted(&paths(&answer)) {
            return answer;
        }
        assert!(
            Instant::now() < deadline,
            "a change on disk never reached a search for {query:?}; \
             the last answer was {:?}",
            paths(&answer)
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

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

/// The ticket's first two lines, and the pair they really are: a file the server
/// did not write appears, and one it did not delete goes away — both without the
/// project being listed again in between.
#[tokio::test]
async fn a_file_created_and_deleted_outside_the_app_is_noticed_without_a_refresh() {
    let workspace = Workspace::with(&["src/main.rs", "README.md"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Opening the project is what puts the server in a position to notice
    // anything: it is the moment there is a description that can go stale.
    let listed = list_entries(&mut client, workspace.path()).await;
    assert_eq!(paths(&listed), ["README.md", "src", "src/main.rs"]);
    server.await_watched_workspaces(1).await;

    let before = search_entries(&mut client, workspace.path(), "ghost").await;
    assert!(
        paths(&before).is_empty(),
        "the fixture already contains the file this test is about: {before}"
    );

    // The agent writes a file. Nothing tells the server.
    workspace.put("src/ghost.rs", "// written by something else");
    search_until(&mut client, workspace.path(), "ghost", |found| {
        found == ["src/ghost.rs"]
    })
    .await;

    // And the agent removes it again.
    std::fs::remove_file(workspace.path().join("src").join("ghost.rs"))
        .expect("removes the file");
    search_until(&mut client, workspace.path(), "ghost", |found| found.is_empty()).await;

    client.close().await;
    server.stop().await;
}

/// A rename has to read as a rename: the new name is offered and the old one is
/// not, at the same moment rather than as two half-truths a user could see
/// between.
///
/// What makes that true here is that the answer is a fresh description of the
/// workspace rather than a patch applied to an old one. There is no create event
/// and no delete event to arrive out of order, and so no window in which a file
/// is present under both names or neither — which is the failure the ticket's
/// wording is guarding against.
#[tokio::test]
async fn a_rename_is_reflected_as_one_name_replacing_another() {
    let workspace = Workspace::with(&["notes/before.md"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    list_entries(&mut client, workspace.path()).await;
    server.await_watched_workspaces(1).await;
    assert_eq!(
        paths(&search_entries(&mut client, workspace.path(), "notes/").await),
        ["notes/before.md"]
    );

    std::fs::rename(
        workspace.path().join("notes").join("before.md"),
        workspace.path().join("notes").join("after.md"),
    )
    .expect("renames the file");

    let answer = search_until(&mut client, workspace.path(), "notes/", |found| {
        found == ["notes/after.md"]
    })
    .await;
    assert!(
        !paths(&answer).contains(&"notes/before.md"),
        "the old name outlived the rename: {answer}"
    );

    client.close().await;
    server.stop().await;
}

/// A moved file is the same story across two directories, which is the half a
/// rename within one directory does not exercise: the entry has to leave one
/// parent and appear under another.
#[tokio::test]
async fn a_file_moved_between_directories_is_reflected_in_both() {
    let workspace = Workspace::with(&["from/moving.txt", "to/"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    list_entries(&mut client, workspace.path()).await;
    server.await_watched_workspaces(1).await;

    std::fs::rename(
        workspace.path().join("from").join("moving.txt"),
        workspace.path().join("to").join("moving.txt"),
    )
    .expect("moves the file");

    search_until(&mut client, workspace.path(), "moving", |found| {
        found == ["to/moving.txt"]
    })
    .await;

    client.close().await;
    server.stop().await;
}

/// A burst is coalesced rather than flooding anything: two hundred files land
/// as fast as the disk will take them, and one call afterwards describes all of
/// them.
///
/// The coalescing is not a timer. A change forgets what the server was holding,
/// and forgetting something already forgotten is free — so a thousand changes
/// and one search cost one scan however fast the thousand arrived. What this
/// asserts is the consequence a user would notice if it were wrong: the answer
/// is complete, not a prefix of the burst.
#[tokio::test]
async fn a_burst_of_changes_is_answered_once_and_completely() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    list_entries(&mut client, workspace.path()).await;
    server.await_watched_workspaces(1).await;

    for index in 0..200 {
        workspace.put(&format!("src/burst-{index:03}.rs"), "// one of many");
    }

    let answer = search_until(&mut client, workspace.path(), "burst-", |found| {
        found.len() == 80
    })
    .await;
    assert_eq!(
        answer["truncated"],
        json!(true),
        "the client asked for eighty of two hundred: {answer}"
    );

    // The last file of the burst is as visible as the first — a listing that
    // had caught only part of it would be the failure worth catching.
    assert_eq!(
        paths(&search_entries(&mut client, workspace.path(), "burst-199").await),
        ["src/burst-199.rs"]
    );

    client.close().await;
    server.stop().await;
}

/// What a repository ignores stays out, however it arrives.
///
/// This is the outcome half of "watching does not recurse into ignored
/// directories". A recursive watch cannot be told to skip a subtree — Windows'
/// `ReadDirectoryChangesW` is all-or-nothing — so the exclusion happens on the
/// events, using the last listing as the rule; that decision is pinned case by
/// case in `filesystem::tests::a_change_matters_only_where_the_listing_names_its_parent`.
/// What can be asserted from out here is that a dependency tree filling up
/// during a session never reaches the user's file tree or their `@` mention,
/// while their own new file, created afterwards, does.
#[tokio::test]
async fn a_dependency_tree_filling_up_never_reaches_the_tree() {
    let workspace = Workspace::with(&[".gitignore", "src/main.rs"]);
    workspace.put(".gitignore", "node_modules/\n");
    workspace.init_repository();

    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let listed = list_entries(&mut client, workspace.path()).await;
    assert_eq!(paths(&listed), [".gitignore", "src", "src/main.rs"]);
    server.await_watched_workspaces(1).await;

    for index in 0..50 {
        workspace.put(
            &format!("node_modules/package-{index:02}/index.js"),
            "module.exports = {}",
        );
    }
    // The user's own file, written after the install and therefore behind all
    // of its noise.
    workspace.put("src/mine.rs", "// mine");

    search_until(&mut client, workspace.path(), "mine", |found| {
        found == ["src/mine.rs"]
    })
    .await;
    assert!(
        paths(&search_entries(&mut client, workspace.path(), "node_modules").await).is_empty(),
        "an ignored dependency tree reached the mention picker"
    );
    assert_eq!(
        paths(&list_entries(&mut client, workspace.path()).await),
        [".gitignore", "src", "src/main.rs", "src/mine.rs"]
    );

    client.close().await;
    server.stop().await;
}

/// Closing a project gives back what was held for it. `project.delete` is what
/// "closed" means on this wire — see `crate::orchestration` — and there is no
/// other frame the UI sends that says a project is finished with.
#[tokio::test]
async fn closing_a_project_releases_its_watcher() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", workspace.path()),
        )
        .await
        .expect_success();
    list_entries(&mut client, workspace.path()).await;
    server.await_watched_workspaces(1).await;

    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "project.delete",
                "commandId": "test:delete:project-1",
                "projectId": "project-1",
            }),
        )
        .await
        .expect_success();

    assert_eq!(
        server.watched_workspaces(),
        0,
        "the closed project's watch was not released"
    );

    client.close().await;
    server.stop().await;
}

/// A disconnection must not leave a watch behind, and a reconnection must not
/// take a second one for the same project. The registry is keyed by the folder
/// rather than by the socket, which is what makes both true — two windows on one
/// project are looking at one filesystem.
#[tokio::test]
async fn reconnecting_does_not_accumulate_watches() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;

    for _ in 0..3 {
        let mut client = server.connect().await;
        list_entries(&mut client, workspace.path()).await;
        server.await_watched_workspaces(1).await;
        client.close().await;
        server.await_live_connections(0).await;
    }

    assert_eq!(server.watched_workspaces(), 1);
    server.stop().await;
}

/// A workspace that is deleted out from under the server is a failed listing,
/// not a panic and not a watch on a folder that is not there.
#[tokio::test]
async fn a_workspace_that_goes_away_fails_its_listing_rather_than_the_connection() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let path = workspace.path().to_path_buf();
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    list_entries(&mut client, &path).await;
    server.await_watched_workspaces(1).await;

    drop(workspace);

    let refusal = client
        .call("projects.listEntries", json!({"cwd": path.to_string_lossy()}))
        .await;
    assert!(
        matches!(refusal, Outcome::Failure(_)),
        "a workspace that is gone answered with {refusal:?}"
    );
    refusal.expect_declared("ProjectListEntriesError");

    // The connection is untouched by it.
    assert_eq!(client.ping().await["_tag"], json!("Pong"));

    client.close().await;
    server.stop().await;
}
