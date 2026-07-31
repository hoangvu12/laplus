//! The worktree methods a developer drives, through the socket boundary.
//!
//! Ticket 02 of the vcs effort. Nothing here reaches into `laplus_server::refs`
//! or `laplus_server::git`: a repository is built with the `git` binary, a second
//! checkout of it is made the way a developer makes one, and then the request the
//! UI sends is sent.
//!
//! **The one flow in this effort with a live UI path.** `useThreadActions.ts`
//! offers to remove the worktree when a conversation that lives in one is
//! deleted, and answering yes sends exactly the payload below —
//! `{cwd, path, force: true}`. Until this ticket the conversation went, the
//! worktree stayed, and the developer got an error toast after the fact.
//!
//! Two things are checked about every call that changes something, because
//! either one alone would pass while the feature was broken: **what happened on
//! disk**, which is whether the developer's folders actually went, and **what the
//! status panel now says**, which is whether they can tell. That is
//! `socket_branches.rs`'s doctrine and this file inherits it.

mod harness;

use harness::conversation::{create_project, create_thread_at};
use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// A repository with one commit behind it, which is what a worktree is made
/// from.
fn committed(paths: &[&str]) -> Workspace {
    let workspace = Workspace::with(paths);
    workspace.init_repository().commit("first");
    workspace
}

/// Ask for the removal the delete-conversation flow asks for.
async fn remove(client: &mut SocketClient, project: &Workspace, path: &str, force: bool) -> Value {
    let mut payload = json!({"cwd": project.cwd(), "path": path});
    if force {
        payload["force"] = json!(true);
    }
    client
        .call("vcs.removeWorktree", payload)
        .await
        .expect_success()
}

/// The `name` of every ref the branch picker would draw.
async fn picker(client: &mut SocketClient, project: &Workspace) -> Vec<String> {
    let listing = client
        .call("vcs.listRefs", json!({"cwd": project.cwd()}))
        .await
        .expect_success();
    listing["refs"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of refs: {listing}"))
        .iter()
        .map(|entry| entry["name"].as_str().expect("a name").to_string())
        .collect()
}

/// Every path the status panel is listing as changed.
fn changed(status: &Value) -> Vec<&str> {
    status["workingTree"]["files"]
        .as_array()
        .unwrap_or_else(|| panic!("a working tree: {status}"))
        .iter()
        .map(|file| file["path"].as_str().expect("a path"))
        .collect()
}

// ---------------------------------------------------------------------------
// Removing
// ---------------------------------------------------------------------------

/// The ticket's first two lines, which are one test because they are one claim:
/// removing a checkout is not deleting a branch.
///
/// A developer who removed a worktree and found the branch gone with it would
/// have lost work no message warned them about, so the picker is asked
/// afterwards rather than the ref file being looked at — the picker is what the
/// developer would look at.
#[tokio::test]
async fn a_worktree_is_removed_and_the_ref_it_held_is_still_in_the_picker() {
    let project = committed(&["src/main.rs"]);
    let worktree = project.worktree("feature");
    assert!(worktree.path().join("src/main.rs").exists(), "the fixture is not set up");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let answer = remove(&mut client, &project, &worktree.cwd(), false).await;

    // `vcs.removeWorktree` declares no success value, and `Schema.Void` encodes
    // to `null` over this wire.
    assert_eq!(answer, Value::Null);
    assert!(
        !worktree.path().exists(),
        "the worktree is still on disk at {}",
        worktree.path().display()
    );
    assert!(
        picker(&mut client, &project).await.contains(&"feature".to_string()),
        "removing a checkout deleted the branch it held"
    );
    // The project's own checkout is untouched, which is the folder the developer
    // is still standing in.
    assert!(project.path().join("src/main.rs").exists());

    server.stop().await;
}

/// The ticket's third and fourth lines, and they belong together: it is *the
/// same worktree* that is refused and then removed, so the refusal cannot have
/// been about anything except the force flag.
///
/// git's own sentence is carried through rather than paraphrased — it is the one
/// that says both what is in the way and what to do about it, and softening it is
/// how laplus would come to quietly discard work.
#[tokio::test]
async fn a_dirty_worktree_is_refused_until_force_is_asked_for() {
    let project = committed(&["src/main.rs"]);
    let worktree = project.worktree("feature");
    worktree.put("src/main.rs", "half-finished, not committed\n");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call(
            "vcs.removeWorktree",
            json!({"cwd": project.cwd(), "path": worktree.cwd()}),
        )
        .await
        .expect_declared("GitCommandError");

    assert_eq!(error["operation"], json!("vcs.removeWorktree"));
    let detail = error["detail"].as_str().expect("a detail");
    assert!(
        detail.contains("modified") || detail.contains("untracked"),
        "the refusal must say what is in the way: {detail}"
    );
    assert!(
        detail.contains("force"),
        "the refusal must say what to do about it: {detail}"
    );
    assert!(error["exitCode"].is_number(), "{error}");

    // Nothing was discarded, which is the half of this that matters more than
    // the sentence.
    assert_eq!(worktree.read("src/main.rs"), "half-finished, not committed\n");

    // And the developer who is sure gets their way.
    let answer = remove(&mut client, &project, &worktree.cwd(), true).await;
    assert_eq!(answer, Value::Null);
    assert!(!worktree.path().exists(), "--force did not remove the worktree");
    assert!(picker(&mut client, &project).await.contains(&"feature".to_string()));

    server.stop().await;
}

/// The ticket's fifth line. A mistyped path is the case where a pre-check would
/// be tempting and a deletion would be unforgivable — so git is asked, git
/// refuses in its own words, and the folder is still there.
#[tokio::test]
async fn a_path_that_is_not_a_worktree_is_refused_and_nothing_is_deleted() {
    let project = committed(&["src/main.rs"]);
    let elsewhere = Workspace::with(&["notes.txt"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call(
            "vcs.removeWorktree",
            json!({"cwd": project.cwd(), "path": elsewhere.cwd()}),
        )
        .await
        .expect_declared("GitCommandError");

    assert_eq!(error["operation"], json!("vcs.removeWorktree"));
    assert!(
        error["detail"]
            .as_str()
            .expect("a detail")
            .contains("working tree"),
        "git's own reason is better than a pre-check's: {error}"
    );
    assert!(
        elsewhere.path().join("notes.txt").exists(),
        "a wrong path deleted a folder that was not a worktree"
    );

    server.stop().await;
}

/// The ticket's sixth line: the whole ref family fails the same way, so a client
/// that handles one refusal handles all of them.
#[tokio::test]
async fn a_folder_that_is_not_a_repository_is_refused_under_the_ref_familys_error() {
    let project = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let error = client
        .call(
            "vcs.removeWorktree",
            json!({"cwd": project.cwd(), "path": project.cwd()}),
        )
        .await
        .expect_declared("GitCommandError");

    assert_eq!(error["operation"], json!("vcs.removeWorktree"));
    assert_eq!(error["command"], json!("git"));
    assert_eq!(error["cwd"], json!(project.cwd()));
    assert!(
        !error["detail"].as_str().expect("a detail").is_empty(),
        "{error}"
    );

    server.stop().await;
}

/// The ticket's seventh line. A removal changes what git says about the folder
/// the developer is watching, and this server is the one that changed it — so
/// the panel finds out without anybody pressing refresh.
///
/// **The worktree is nested inside the project on purpose**, which is the one
/// layout where the removal is visible in the *project's* status: git reports a
/// linked worktree under the main one as a single untracked directory. A worktree
/// beside the project changes nothing a status panel is showing, so a test using
/// one would have nothing to look at.
///
/// **What this asserts is the outcome, not the mechanism.** The watcher is
/// already recursive over a subscribed workspace, so it would notice the folder
/// going on its own; `Repositories::disturb` is what makes the panel prompt
/// rather than eventual, and prompt is a claim about time, which no test in this
/// tree asserts on (see `READ_TIMEOUT` in `tests/harness/mod.rs`). So this test
/// passes against a removal that forgot to disturb — and it is still the test
/// worth having, because the thing that would be *broken* is a panel that never
/// caught up at all.
#[tokio::test]
async fn the_status_panel_notices_a_removed_worktree_without_being_asked() {
    let project = committed(&["src/main.rs"]);
    project.git(&["worktree", "add", "-b", "feature", "nested"]);
    let nested = project.path().join("nested");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let watching = client
        .subscribe("subscribeVcsStatus", json!({"cwd": project.cwd()}))
        .await;
    client.status_until(&watching, |status| {
        changed(status).contains(&"nested/")
    })
    .await;

    remove(&mut client, &project, &nested.to_string_lossy(), false).await;

    assert!(!nested.exists(), "the worktree is still on disk");
    let told = client.status_until(&watching, |status| {
        !changed(status).contains(&"nested/")
    })
    .await;
    assert_eq!(told["isRepo"], json!(true));

    client.interrupt(&watching).await;
    server.stop().await;
}

// ---------------------------------------------------------------------------
// The flow this method exists for
// ---------------------------------------------------------------------------

/// The delete-conversation flow's own sequence, in the order
/// `useThreadActions.ts` sends it.
///
/// The method above is answered; this is the claim that the *flow* is. It is
/// three calls, and the failure this ticket was written about happened on the
/// second, after the first had already succeeded — the conversation was gone, so
/// there was nothing to retry and nothing to undo, and what the developer got was
/// a toast about a worktree that was still on disk.
///
/// Not a substitute for driving the window, which is the ticket's own last
/// section: this sends the payloads the client builds, not the clicks that build
/// them. What it does cover is the part between them — that the three calls
/// succeed *in this order*, with the deletion first.
#[tokio::test]
async fn deleting_a_conversation_that_lives_in_a_worktree_tidies_the_worktree_up() {
    let project = committed(&["src/main.rs"]);
    let worktree = project.worktree("feature");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    client
        .call(
            "orchestration.dispatchCommand",
            create_project("project-1", project.path()),
        )
        .await
        .expect_success();
    client
        .call(
            "orchestration.dispatchCommand",
            create_thread_at("project-1", "thread-1", Some(worktree.path())),
        )
        .await
        .expect_success();

    // One: the conversation goes.
    client
        .call(
            "orchestration.dispatchCommand",
            json!({
                "type": "thread.delete",
                "commandId": "test:delete:thread-1",
                "threadId": "thread-1",
            }),
        )
        .await
        .expect_success();

    // Two: and the developer said yes to the offer, so the worktree goes with
    // it. `force: true` is what the client sends — the dialogue is where that
    // question was asked.
    let removed = remove(&mut client, &project, &worktree.cwd(), true).await;
    assert_eq!(removed, Value::Null);
    assert!(!worktree.path().exists(), "the offer to tidy up did not tidy up");

    // Three: the panel is asked for the status the client does not wait for the
    // subscription to bring.
    let status = client
        .call("vcs.refreshStatus", json!({"cwd": project.cwd()}))
        .await
        .expect_success();
    assert_eq!(status["isRepo"], json!(true));
    assert_eq!(status["refName"], json!("main"));

    // The branch the conversation was working on is still there to start another
    // conversation on, which is the whole difference between removing a checkout
    // and deleting work.
    assert!(picker(&mut client, &project).await.contains(&"feature".to_string()));

    client.close().await;
    server.stop().await;
}
