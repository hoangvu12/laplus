//! Working tree status through the socket boundary, against real repositories.
//!
//! Ticket 19's last line asks for exactly this, and nothing here reaches into
//! `lightcode_server::git`: a repository is made with the `git` binary, changed
//! with ordinary file writes, and read by sending the two requests the UI sends
//! — `vcs.refreshStatus` for the panel's refresh button and `subscribeVcsStatus`
//! for the panel itself.
//!
//! ## Why the live half is a wait rather than a sleep
//!
//! A change on disk reaches a status through three hops that are none of them
//! synchronous with the write: the operating system reports the change when it
//! reports it, the server gathers a burst before reading, and the read is a
//! child process. A sleep would be a guess that is too short on a loaded machine
//! and too long on every other run. Every wait here is bounded and fails with a
//! sentence saying what it was still waiting for.

mod harness;

use std::time::Duration;

use harness::workspace::Workspace;
use harness::{SocketClient, TestServer};
use serde_json::{json, Value};

/// Ask for the status once — the panel's refresh button.
async fn refresh(client: &mut SocketClient, workspace: &Workspace) -> Value {
    client
        .call("vcs.refreshStatus", json!({"cwd": workspace.cwd()}))
        .await
        .expect_success()
}

/// Open the status subscription — the panel itself.
async fn watch(client: &mut SocketClient, workspace: &Workspace) -> String {
    client
        .subscribe("subscribeVcsStatus", json!({"cwd": workspace.cwd()}))
        .await
}

/// The `path` of every changed file in a status, in the order it was sent.
fn changed(status: &Value) -> Vec<&str> {
    status["workingTree"]["files"]
        .as_array()
        .unwrap_or_else(|| panic!("an array of changed files: {status}"))
        .iter()
        .map(|file| {
            file["path"]
                .as_str()
                .unwrap_or_else(|| panic!("a path: {file}"))
        })
        .collect()
}

/// One changed file's counts, or a failure naming what was there instead.
fn counts(status: &Value, path: &str) -> (u64, u64) {
    let file = status["workingTree"]["files"]
        .as_array()
        .expect("an array of changed files")
        .iter()
        .find(|file| file["path"] == json!(path))
        .unwrap_or_else(|| panic!("{path} is not in {:?}", changed(status)));
    (
        file["insertions"].as_u64().expect("insertions"),
        file["deletions"].as_u64().expect("deletions"),
    )
}

/// The local half of a stream event, which is the same shape the unary call
/// answers with.
fn local(event: &Value) -> &Value {
    assert_eq!(event["_tag"], json!("snapshot"), "unexpected event: {event}");
    &event["local"]
}

/// Read the subscription until a status satisfies `wanted`, and say how many
/// statuses that took.
///
/// The count is what the coalescing test asserts on, and it is the honest
/// measure: how many *statuses the client had to apply*, not how many chunks
/// they were batched into, which is decided by how fast this test acknowledges.
async fn status_until(
    client: &mut SocketClient,
    request_id: &str,
    wanted: impl Fn(&Value) -> bool,
) -> (Value, usize) {
    let seen = client
        .values_until(request_id, |event| wanted(local(event)))
        .await;
    let count = seen.len();
    let last = seen.into_iter().last().expect("at least one status");
    (local(&last).clone(), count)
}

/// A repository with one commit behind it, which is what a status is measured
/// against.
fn committed(paths: &[&str]) -> Workspace {
    let workspace = Workspace::with(paths);
    workspace.init_repository().commit("first");
    workspace
}

/// The ticket's first line: the four kinds of change a developer needs to see,
/// with the sizes that tell them how much the agent did.
#[tokio::test]
async fn the_working_tree_names_what_changed_and_by_how_much() {
    let workspace = committed(&["src/main.rs", "README.md"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    // Modified, deleted, untracked, and staged — the last one because a status
    // that only showed unstaged work would go blank the moment anything was
    // added, which is the opposite of what "what did the agent do" needs.
    workspace.put("src/main.rs", "one\ntwo\nthree\n");
    workspace.remove("README.md");
    workspace.put("notes.txt", "a\nb\n");
    workspace.put("src/added.rs", "x\n");
    workspace.git(&["add", "src/added.rs"]);

    let status = refresh(&mut client, &workspace).await;

    assert_eq!(status["isRepo"], json!(true));
    assert_eq!(status["hasWorkingTreeChanges"], json!(true));
    assert_eq!(
        changed(&status),
        ["README.md", "notes.txt", "src/added.rs", "src/main.rs"]
    );

    assert_eq!(counts(&status, "src/main.rs"), (3, 1), "a modified file");
    assert_eq!(counts(&status, "README.md"), (0, 1), "a deleted file");
    assert_eq!(counts(&status, "src/added.rs"), (1, 0), "a staged addition");
    assert_eq!(counts(&status, "notes.txt"), (2, 0), "an untracked file");

    // The totals are over every changed file, which is what the panel's header
    // shows.
    assert_eq!(status["workingTree"]["insertions"], json!(6));
    assert_eq!(status["workingTree"]["deletions"], json!(2));

    server.stop().await;
}

/// A repository nobody has touched says so, rather than saying nothing.
#[tokio::test]
async fn a_clean_repository_reports_itself_clean() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let status = refresh(&mut client, &workspace).await;

    assert_eq!(status["isRepo"], json!(true));
    assert_eq!(status["hasWorkingTreeChanges"], json!(false));
    assert_eq!(changed(&status), [] as [&str; 0]);
    assert_eq!(status["workingTree"]["insertions"], json!(0));

    server.stop().await;
}

/// The ticket's third line. The branch is also how a developer tells one
/// worktree from another, so it has to be the branch they are actually on.
#[tokio::test]
async fn the_current_branch_is_shown_and_follows_a_switch() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let status = refresh(&mut client, &workspace).await;
    assert_eq!(status["refName"], json!("main"));
    // With no remote there is no recorded default branch, only the convention —
    // and `main` is it.
    assert_eq!(status["isDefaultRef"], json!(true));
    assert_eq!(status["hasPrimaryRemote"], json!(false));

    workspace.git(&["checkout", "-b", "feature/status"]);

    let status = refresh(&mut client, &workspace).await;
    assert_eq!(status["refName"], json!("feature/status"));
    assert_eq!(status["isDefaultRef"], json!(false));

    server.stop().await;
}

/// The ticket's fourth line. A project with no repository is a normal thing to
/// have open — it is what ticket 21's `vcs.init` exists for — so it must read as
/// a status with `isRepo` false rather than as a call that failed.
#[tokio::test]
async fn a_project_that_is_not_a_repository_is_reported_as_such() {
    let workspace = Workspace::with(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let status = refresh(&mut client, &workspace).await;

    assert_eq!(status["isRepo"], json!(false));
    assert_eq!(status["refName"], Value::Null);
    assert_eq!(status["hasWorkingTreeChanges"], json!(false));
    assert_eq!(changed(&status), [] as [&str; 0]);
    // The remote half of the flattened answer is still there, because the
    // contract requires every field of it.
    assert_eq!(status["hasUpstream"], json!(false));
    assert_eq!(status["pr"], Value::Null);

    // The panel says the same thing, and its remote half is the `null` the
    // stream's schema allows rather than a set of zeroes claiming a branch.
    let watching = watch(&mut client, &workspace).await;
    let event = client.next_event(&watching).await;
    assert_eq!(event["_tag"], json!("snapshot"));
    assert_eq!(event["local"]["isRepo"], json!(false));
    assert_eq!(event["remote"], Value::Null);

    client.interrupt(&watching).await;
    server.stop().await;
}

/// A project that is a package *inside* a repository — a monorepo, which is the
/// arrangement this whole server exists to work in.
///
/// Two things could go wrong here and only one of them is obvious. git names
/// every changed path relative to the **repository** root rather than the
/// project root, which is what the UI is given; and the line counts for an
/// untracked file are read off the disk, so they have to be read from the same
/// place git named them from. Joining an untracked path onto the project root
/// instead would look for `packages/web/packages/web/…`, find nothing, and
/// report the agent's new file as `+0`.
#[tokio::test]
async fn a_project_inside_a_larger_repository_is_read_from_the_repository_root() {
    let repository = Workspace::with(&["packages/web/index.ts"]);
    repository.init_repository().commit("first");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let package = repository.path().join("packages").join("web");
    std::fs::write(package.join("added.ts"), "one\ntwo\nthree\n").expect("writes the file");

    let status = client
        .call(
            "vcs.refreshStatus",
            json!({"cwd": package.to_string_lossy()}),
        )
        .await
        .expect_success();

    assert_eq!(status["isRepo"], json!(true));
    assert_eq!(changed(&status), ["packages/web/added.ts"]);
    assert_eq!(
        counts(&status, "packages/web/added.ts"),
        (3, 0),
        "the untracked file's lines were counted from the wrong place"
    );

    server.stop().await;
}

/// The ticket's second line, and the one the whole subscription exists for: a
/// change the app did not make reaches the panel without anything asking.
#[tokio::test]
async fn a_status_refreshes_as_files_change_without_being_asked() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let watching = watch(&mut client, &workspace).await;
    let (opening, _) = status_until(&mut client, &watching, |status| {
        status["isRepo"] == json!(true)
    })
    .await;
    assert_eq!(changed(&opening), [] as [&str; 0], "the tree starts clean");
    // Opening the panel is what puts the server in a position to notice
    // anything.
    server.await_watched_workspaces(1).await;

    // Nothing here is a request. This is the agent writing a file.
    workspace.put("src/agent-wrote-this.rs", "fn generated() {}\n");

    let (after, _) = status_until(&mut client, &watching, |status| {
        !changed(status).is_empty()
    })
    .await;
    assert_eq!(changed(&after), ["src/agent-wrote-this.rs"]);
    assert_eq!(counts(&after, "src/agent-wrote-this.rs"), (1, 0));

    // …and it goes away again when the file does, which is the half that says
    // the status is being recomputed rather than accumulated.
    workspace.remove("src/agent-wrote-this.rs");

    let (settled, _) = status_until(&mut client, &watching, |status| {
        changed(status).is_empty()
    })
    .await;
    assert_eq!(settled["hasWorkingTreeChanges"], json!(false));

    client.interrupt(&watching).await;
    server.await_live_subscriptions(0).await;
    server.stop().await;
}

/// The ticket's sixth line. A build writes thousands of files in seconds, and a
/// server that read the repository once per file would pin a core and send the
/// panel a thousand statuses to throw away.
///
/// The assertion is a bound rather than an exact count, because how a burst
/// splits across the coalescing window depends on how fast the disk is. What it
/// rules out is the failure that matters: a status per file.
#[tokio::test]
async fn a_burst_of_changes_is_coalesced_rather_than_read_per_file() {
    const WRITTEN: usize = 60;
    /// Generous — a burst this size should settle in one or two reads. It is
    /// two orders of magnitude below "one read per file", which is the thing
    /// being ruled out.
    const AT_MOST: usize = 10;

    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let watching = watch(&mut client, &workspace).await;
    status_until(&mut client, &watching, |status| {
        status["isRepo"] == json!(true)
    })
    .await;
    server.await_watched_workspaces(1).await;

    for index in 0..WRITTEN {
        workspace.put(&format!("generated/file-{index}.rs"), "fn generated() {}\n");
    }

    let (last, statuses) = status_until(&mut client, &watching, |status| {
        changed(status).len() == WRITTEN
    })
    .await;

    assert_eq!(changed(&last).len(), WRITTEN);
    assert!(
        statuses <= AT_MOST,
        "{WRITTEN} files produced {statuses} statuses; a burst is meant to be \
         gathered into a handful of reads rather than one per file"
    );

    client.interrupt(&watching).await;
    server.stop().await;
}

/// The ticket's sixth line, first half: reading a status does not stall the UI.
///
/// "Very large repository" is not something a test can honestly build — it would
/// be minutes of setup for a wall-clock assertion that fails on a slow CI box.
/// What *is* the property, and what a large repository would only make more
/// obvious, is that a read never happens anywhere the connection is waiting: the
/// unary call is deferred off the read loop and the subscription does not run
/// git at all. So the assertion is that the connection keeps answering while
/// reads are in flight — a `Ping` still gets its `Pong`, and an unrelated call
/// still returns, with a burst of changes underway and a status subscription
/// open on the same socket.
#[tokio::test]
async fn the_connection_keeps_answering_while_a_status_is_being_read() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let watching = watch(&mut client, &workspace).await;
    status_until(&mut client, &watching, |status| {
        status["isRepo"] == json!(true)
    })
    .await;
    server.await_watched_workspaces(1).await;

    // Keep the refresh thread busy for the whole of what follows.
    for index in 0..40 {
        workspace.put(&format!("generated/file-{index}.rs"), "fn generated() {}\n");
    }

    // Both of these are answered on the read loop. If a read were happening
    // there, they would queue behind it — and every read here is timed out, so
    // "stalled" is a failure with a sentence rather than a hang.
    assert_eq!(client.ping().await, json!({"_tag": "Pong"}));
    let config = client.call("server.getConfig", json!({})).await;
    assert!(config.expect_success()["settings"].is_object());
    assert_eq!(client.ping().await, json!({"_tag": "Pong"}));

    client.interrupt(&watching).await;
    server.stop().await;
}

/// The ticket's seventh line, first half. A detached HEAD is a repository with
/// no branch — a normal state after checking out a commit to look at it — and
/// the contract has a null `refName` for exactly this.
#[tokio::test]
async fn a_detached_head_is_a_repository_with_no_branch() {
    let workspace = committed(&["src/main.rs"]);
    workspace.put("src/main.rs", "second\n");
    workspace.commit("second");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    workspace.git(&["checkout", "--detach", "HEAD~1"]);
    workspace.put("src/main.rs", "changed while detached\n");

    let status = refresh(&mut client, &workspace).await;

    assert_eq!(status["isRepo"], json!(true));
    assert_eq!(status["refName"], Value::Null);
    assert_eq!(status["isDefaultRef"], json!(false));
    // The working tree is still read, which is the point: a developer looking
    // at an old commit still needs to see what they have changed in it.
    assert_eq!(changed(&status), ["src/main.rs"]);

    server.stop().await;
}

/// The ticket's seventh line, second half. A merge that stopped on a conflict
/// leaves entries in a record shape nothing else produces, and the status has to
/// carry them rather than fall over.
#[tokio::test]
async fn a_repository_stopped_mid_merge_still_reports_its_status() {
    let workspace = committed(&["shared.txt"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    workspace.git(&["checkout", "-b", "theirs"]);
    workspace.put("shared.txt", "their version\n");
    workspace.commit("theirs");

    workspace.git(&["checkout", "main"]);
    workspace.put("shared.txt", "our version\n");
    workspace.commit("ours");

    let merge = workspace.try_git(&["merge", "theirs"]);
    assert!(!merge.status.success(), "the merge was meant to conflict");
    assert!(
        workspace.path().join(".git").join("MERGE_HEAD").exists(),
        "the repository is not actually mid-merge"
    );

    let status = refresh(&mut client, &workspace).await;

    assert_eq!(status["isRepo"], json!(true));
    assert_eq!(status["refName"], json!("main"));
    assert_eq!(status["hasWorkingTreeChanges"], json!(true));
    assert_eq!(changed(&status), ["shared.txt"]);

    server.stop().await;
}

/// A repository with nothing committed yet — which is every project the moment
/// after `git init` — has no `HEAD` to measure against. Everything in it is an
/// addition, and it must not read as a failure.
#[tokio::test]
async fn a_repository_with_no_commits_reports_everything_as_added() {
    let workspace = Workspace::with(&["src/main.rs"]);
    workspace.init_repository();
    workspace.put("src/main.rs", "one\ntwo\n");
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let status = refresh(&mut client, &workspace).await;

    assert_eq!(status["isRepo"], json!(true));
    assert_eq!(status["refName"], json!("main"));
    assert_eq!(changed(&status), ["src/main.rs"]);
    assert_eq!(counts(&status, "src/main.rs"), (2, 0));

    server.stop().await;
}

/// A workspace root the server cannot use is refused with the error the two
/// methods declare — `GitCommandError` — rather than with the unknown-method
/// error, which would tell the client this server cannot read git at all.
#[tokio::test]
async fn a_workspace_root_that_is_not_there_is_refused_with_the_declared_error() {
    let workspace = Workspace::with(&[]);
    let missing = workspace.path().join("not-there").to_string_lossy().into_owned();
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    for tag in ["vcs.refreshStatus", "subscribeVcsStatus"] {
        let error = client
            .call(tag, json!({"cwd": missing}))
            .await
            .expect_declared("GitCommandError");

        assert_eq!(error["operation"], json!(tag));
        assert_eq!(error["cwd"], json!(missing));
        assert!(
            error["detail"]
                .as_str()
                .expect("a detail")
                .contains("does not exist"),
            "{error}"
        );
        // `message` is a getter on the client's own error class, so a server
        // that sent one would be sending a field the reference server does not.
        assert!(error.get("message").is_none(), "{error}");
    }

    let blank = client
        .call("vcs.refreshStatus", json!({"cwd": "   "}))
        .await
        .expect_declared("GitCommandError");
    assert!(blank["detail"].as_str().expect("a detail").contains("workspace root"));

    server.stop().await;
}

/// Two windows on one project share one working tree, so pressing refresh in
/// one is seen by the panel in the other. The alternative — a status per
/// connection — would let two views of the same folder disagree.
#[tokio::test]
async fn a_refresh_on_one_connection_reaches_a_subscriber_on_another() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut watching_client = server.connect().await;
    let mut refreshing_client = server.connect().await;

    let watching = watch(&mut watching_client, &workspace).await;
    status_until(&mut watching_client, &watching, |status| {
        status["isRepo"] == json!(true)
    })
    .await;

    workspace.put("src/main.rs", "changed\n");
    let asked = refresh(&mut refreshing_client, &workspace).await;
    assert_eq!(changed(&asked), ["src/main.rs"]);

    let (told, _) = status_until(&mut watching_client, &watching, |status| {
        !changed(status).is_empty()
    })
    .await;
    assert_eq!(told, asked_local(&asked));

    watching_client.interrupt(&watching).await;
    server.stop().await;
}

/// The local half of a unary answer, which is the flattened one minus the
/// remote fields — so the two shapes can be compared.
fn asked_local(status: &Value) -> Value {
    let mut local = status.clone();
    for remote in ["hasUpstream", "aheadCount", "behindCount", "pr"] {
        local.as_object_mut().expect("an object").remove(remote);
    }
    local
}

/// Cancelling the subscription releases it, and nothing arrives afterwards. The
/// panel is unmounted every time a developer switches to another project, so
/// this is the common case rather than the tidy-up one.
#[tokio::test]
async fn unsubscribing_releases_the_stream() {
    let workspace = committed(&["src/main.rs"]);
    let server = TestServer::start().await;
    let mut client = server.connect().await;

    let watching = watch(&mut client, &workspace).await;
    status_until(&mut client, &watching, |status| {
        status["isRepo"] == json!(true)
    })
    .await;
    assert_eq!(server.live_subscriptions(), 1);

    client.interrupt(&watching).await;
    server.await_live_subscriptions(0).await;
    // The cancellation is answered by a terminal frame for the same request,
    // which a client reads as the normal end of a subscription.
    let ended = client.next_frame_for(&watching).await;
    assert_eq!(ended["_tag"], json!("Exit"), "{ended}");

    // A change now has nowhere to go, and must not produce a frame.
    workspace.put("src/after.rs", "fn after() {}\n");
    client.expect_silence(Duration::from_millis(600)).await;

    server.stop().await;
}
